use savepoint_egress::guard::{GuardDecision, GuardPolicy, MutationGuard};
use savepoint_egress::vcr::HttpRequestSnapshot;
use std::collections::HashMap;

#[test]
fn test_mutation_guard_identifies_verbs_and_synthesizes_mock_responses() {
    let guard = MutationGuard::new(GuardPolicy::MockAlways);

    // 1. Safe read methods (GET / HEAD) should be Allowed
    let get_req = HttpRequestSnapshot {
        method: "GET".to_string(),
        url: "https://api.stripe.com/v1/customers".to_string(),
        headers: HashMap::new(),
        body: vec![],
    };
    assert!(matches!(guard.inspect(&get_req), GuardDecision::AllowRead));

    // 2. Destructive POST to Stripe should synthesize mock response
    let post_stripe = HttpRequestSnapshot {
        method: "POST".to_string(),
        url: "https://api.stripe.com/v1/charges".to_string(),
        headers: HashMap::new(),
        body: b"{\"amount\": 2000, \"currency\": \"usd\"}".to_vec(),
    };
    match guard.inspect(&post_stripe) {
        GuardDecision::MockResponse(mock) => {
            assert_eq!(mock.status, 201);
            let json: serde_json::Value = serde_json::from_slice(&mock.body).unwrap();
            assert!(json["id"].as_str().unwrap().starts_with("ch_mock_"));
            assert_eq!(json["paid"], true);
        }
        other => panic!("Expected MockResponse, got {:?}", other),
    }

    // 3. Block policy blocks destructive verbs
    let strict_guard = MutationGuard::new(GuardPolicy::Block);
    assert!(matches!(strict_guard.inspect(&post_stripe), GuardDecision::Block { .. }));
}
