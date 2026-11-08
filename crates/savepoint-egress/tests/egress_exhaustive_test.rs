use savepoint_egress::guard::{GuardDecision, GuardPolicy, MutationGuard};
use savepoint_egress::saga::{CompensatingStep, SagaAction, SagaLog};
use savepoint_egress::vcr::{HttpRequestSnapshot, HttpResponseSnapshot, VcrEngine};
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn test_guard_http_method_classification() {
    // Idempotent / Safe reads
    assert!(!MutationGuard::is_mutating("GET"));
    assert!(!MutationGuard::is_mutating("get"));
    assert!(!MutationGuard::is_mutating("HEAD"));
    assert!(!MutationGuard::is_mutating("OPTIONS"));

    // Mutating verbs
    assert!(MutationGuard::is_mutating("POST"));
    assert!(MutationGuard::is_mutating("post"));
    assert!(MutationGuard::is_mutating("PUT"));
    assert!(MutationGuard::is_mutating("PATCH"));
    assert!(MutationGuard::is_mutating("DELETE"));
    assert!(MutationGuard::is_mutating("delete"));
}

#[test]
fn test_guard_inspect_policies() {
    let guard_mock = MutationGuard::new(GuardPolicy::MockAlways);
    let guard_block = MutationGuard::new(GuardPolicy::Block);
    let guard_saga = MutationGuard::new(GuardPolicy::AllowWithSaga);

    let read_req = HttpRequestSnapshot {
        method: "GET".to_string(),
        url: "https://api.github.com/repos/test".to_string(),
        headers: HashMap::new(),
        body: Vec::new(),
    };
    match guard_mock.inspect(&read_req) {
        GuardDecision::AllowRead => (),
        _ => panic!("Expected AllowRead"),
    }

    let post_req = HttpRequestSnapshot {
        method: "POST".to_string(),
        url: "https://api.stripe.com/v1/charges".to_string(),
        headers: HashMap::new(),
        body: b"amount=1000".to_vec(),
    };
    match guard_mock.inspect(&post_req) {
        GuardDecision::MockResponse(resp) => {
            assert_eq!(resp.status, 201);
            assert!(String::from_utf8_lossy(&resp.body).contains("ch_mock_"));
        }
        _ => panic!("Expected MockResponse"),
    }

    match guard_block.inspect(&post_req) {
        GuardDecision::Block { reason } => {
            assert!(reason.contains("blocked by policy"));
        }
        _ => panic!("Expected Block"),
    }

    match guard_saga.inspect(&post_req) {
        GuardDecision::AllowMutateWithSaga => (),
        _ => panic!("Expected AllowMutateWithSaga"),
    }
}

#[test]
fn test_vcr_header_sanitization_comprehensive() {
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer secret_jwt_token".to_string());
    headers.insert("X-Api-Key".to_string(), "key_live_12345".to_string());
    headers.insert("Cookie".to_string(), "session=abc123secret".to_string());
    headers.insert("User-Agent".to_string(), "Savepoint-Agent/1.0".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    let sanitized = VcrEngine::sanitize_headers(&headers);
    assert_eq!(sanitized.get("Authorization").unwrap(), "***REDACTED***");
    assert_eq!(sanitized.get("X-Api-Key").unwrap(), "***REDACTED***");
    assert_eq!(sanitized.get("Cookie").unwrap(), "***REDACTED***");
    assert_eq!(sanitized.get("User-Agent").unwrap(), "Savepoint-Agent/1.0");
    assert_eq!(sanitized.get("Content-Type").unwrap(), "application/json");
}

#[test]
fn test_vcr_recording_cache_miss_and_hit() {
    let dir = tempdir().unwrap();
    let vcr = VcrEngine::new(dir.path());

    let req = HttpRequestSnapshot {
        method: "GET".to_string(),
        url: "https://api.example.com/status".to_string(),
        headers: HashMap::new(),
        body: Vec::new(),
    };

    assert!(vcr.replay("test_cassette", &req).unwrap().is_none());

    let resp = HttpResponseSnapshot {
        status: 200,
        headers: HashMap::new(),
        body: b"{\"status\": \"ok\"}".to_vec(),
    };

    vcr.record("test_cassette", &req, &resp).unwrap();

    let cached = vcr.replay("test_cassette", &req).unwrap().expect("Cache hit expected");
    assert_eq!(cached.status, 200);
    assert_eq!(cached.body, b"{\"status\": \"ok\"}");
}

#[tokio::test]
async fn test_saga_empty_compensator() {
    let mut saga = SagaLog::new();
    let res = saga.rollback("empty_tx").await.unwrap();
    assert!(res.is_empty());
}

#[test]
fn test_saga_recording_and_count() {
    let mut saga = SagaLog::new();

    let a1 = SagaAction {
        id: "action_1".to_string(),
        description: "Created S3 Object".to_string(),
        compensator: CompensatingStep::MockUndo {
            undo_label: "Delete S3 Object".to_string(),
            payload: b"key=test".to_vec(),
        },
    };
    let a2 = SagaAction {
        id: "action_2".to_string(),
        description: "Sent Slack Notification".to_string(),
        compensator: CompensatingStep::MockUndo {
            undo_label: "Send Slack cancellation message".to_string(),
            payload: b"msg=cancelled".to_vec(),
        },
    };

    saga.record_action("tx_123", a1);
    saga.record_action("tx_123", a2);

    assert_eq!(saga.action_count("tx_123"), 2);
    assert_eq!(saga.action_count("tx_456"), 0);
}

#[test]
fn test_guard_mock_s3_and_slack_endpoints() {
    let s3_req = HttpRequestSnapshot {
        method: "PUT".to_string(),
        url: "https://s3.amazonaws.com/my-bucket/key.txt".to_string(),
        headers: HashMap::new(),
        body: Vec::new(),
    };
    let s3_resp = MutationGuard::synthesize_mock_response(&s3_req);
    assert_eq!(s3_resp.status, 200);
    assert!(String::from_utf8_lossy(&s3_resp.body).contains("ETag"));

    let slack_req = HttpRequestSnapshot {
        method: "POST".to_string(),
        url: "https://hooks.slack.com/services/T00/B00/X00".to_string(),
        headers: HashMap::new(),
        body: Vec::new(),
    };
    let slack_resp = MutationGuard::synthesize_mock_response(&slack_req);
    assert_eq!(slack_resp.status, 201);
    assert!(String::from_utf8_lossy(&slack_resp.body).contains("mocked"));
}
