use savepoint_egress::vcr::{HttpRequestSnapshot, HttpResponseSnapshot, VcrEngine};
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn test_vcr_records_replays_and_sanitizes_headers() {
    let dir = tempdir().unwrap();
    let cassette_dir = dir.path().join("cassettes");
    let vcr = VcrEngine::new(&cassette_dir);

    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer sk_test_secret_123456789".to_string());
    headers.insert("X-Api-Key".to_string(), "top_secret_key_abc".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());

    let req = HttpRequestSnapshot {
        method: "GET".to_string(),
        url: "https://api.stripe.com/v1/customers/cus_123".to_string(),
        headers,
        body: vec![],
    };

    let resp = HttpResponseSnapshot {
        status: 200,
        headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: b"{\"id\":\"cus_123\",\"name\":\"Alice\"}".to_vec(),
    };

    let cassette_name = "stripe_get_customer";
    // 1. Record transaction
    vcr.record(cassette_name, &req, &resp).unwrap();

    // 2. Verify header sanitization in saved file
    let cassette_path = cassette_dir.join(format!("{}.json", cassette_name));
    assert!(cassette_path.exists());
    let raw_saved = std::fs::read_to_string(&cassette_path).unwrap();
    assert!(!raw_saved.contains("sk_test_secret_123456789"), "Secrets must never be saved to cassette");
    assert!(!raw_saved.contains("top_secret_key_abc"), "API keys must be masked");
    assert!(raw_saved.contains("***REDACTED***"));

    // 3. Replay with identical request (even if client presents original token)
    let replayed = vcr.replay(cassette_name, &req).unwrap().expect("Must find recorded entry");
    assert_eq!(replayed.status, 200);
    assert_eq!(replayed.body, b"{\"id\":\"cus_123\",\"name\":\"Alice\"}");

    // 4. Mismatched request should return None
    let diff_req = HttpRequestSnapshot {
        method: "GET".to_string(),
        url: "https://api.stripe.com/v1/customers/cus_OTHER".to_string(),
        headers: HashMap::new(),
        body: vec![],
    };
    assert!(vcr.replay(cassette_name, &diff_req).unwrap().is_none());
}
