use crate::vcr::{HttpRequestSnapshot, HttpResponseSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardPolicy {
    /// Safe GET/HEAD allowed, mutations simulated with synthetic 200/201 mock
    MockAlways,
    /// Allow real network mutations, but automatically register compensating actions in SagaLog
    AllowWithSaga,
    /// Block all mutation verbs (POST, PUT, DELETE, PATCH)
    Block,
}

#[derive(Debug)]
pub enum GuardDecision {
    AllowRead,
    AllowMutateWithSaga,
    MockResponse(HttpResponseSnapshot),
    Block { reason: String },
}

pub struct MutationGuard {
    policy: GuardPolicy,
}

impl MutationGuard {
    pub fn new(policy: GuardPolicy) -> Self {
        Self { policy }
    }

    pub fn is_mutating(method: &str) -> bool {
        matches!(
            method.to_uppercase().as_str(),
            "POST" | "PUT" | "DELETE" | "PATCH"
        )
    }

    pub fn inspect(&self, request: &HttpRequestSnapshot) -> GuardDecision {
        if !Self::is_mutating(&request.method) {
            return GuardDecision::AllowRead;
        }

        match self.policy {
            GuardPolicy::Block => GuardDecision::Block {
                reason: format!(
                    "External mutation verb {} to {} is blocked by policy",
                    request.method, request.url
                ),
            },
            GuardPolicy::AllowWithSaga => GuardDecision::AllowMutateWithSaga,
            GuardPolicy::MockAlways => {
                let mock = Self::synthesize_mock_response(request);
                GuardDecision::MockResponse(mock)
            }
        }
    }

    pub fn synthesize_mock_response(request: &HttpRequestSnapshot) -> HttpResponseSnapshot {
        let url_lower = request.url.to_lowercase();
        let method_upper = request.method.to_uppercase();

        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert("x-savepoint-egress".to_string(), "mocked".to_string());

        let (status, body) = if url_lower.contains("stripe.com") {
            if url_lower.contains("/v1/charges") {
                let charge_id = format!("ch_mock_{}", Uuid::new_v4().simple());
                (
                    201,
                    serde_json::json!({
                        "id": charge_id,
                        "object": "charge",
                        "amount": 2000,
                        "paid": true,
                        "status": "succeeded",
                        "currency": "usd"
                    }),
                )
            } else if url_lower.contains("/v1/customers") {
                let cust_id = format!("cus_mock_{}", Uuid::new_v4().simple());
                (
                    201,
                    serde_json::json!({
                        "id": cust_id,
                        "object": "customer",
                        "email": "test@example.com"
                    }),
                )
            } else if url_lower.contains("/v1/refunds") {
                (
                    200,
                    serde_json::json!({
                        "id": format!("re_mock_{}", Uuid::new_v4().simple()),
                        "status": "succeeded"
                    }),
                )
            } else {
                (
                    200,
                    serde_json::json!({
                        "id": format!("sp_mock_{}", Uuid::new_v4().simple()),
                        "status": "ok"
                    }),
                )
            }
        } else if url_lower.contains("amazonaws.com") {
            (
                200,
                serde_json::json!({
                    "ETag": format!("\"{}\"", Uuid::new_v4().simple()),
                    "status": "succeeded"
                }),
            )
        } else {
            let status = if method_upper == "POST" { 201 } else { 200 };
            (
                status,
                serde_json::json!({
                    "mocked": true,
                    "id": format!("sp_mock_{}", Uuid::new_v4().simple()),
                    "status": "ok"
                }),
            )
        };

        HttpResponseSnapshot {
            status,
            headers,
            body: serde_json::to_vec_pretty(&body).unwrap_or_default(),
        }
    }
}
