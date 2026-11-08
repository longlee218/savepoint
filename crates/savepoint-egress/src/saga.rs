use crate::error::{EgressError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompensatingStep {
    MockUndo {
        undo_label: String,
        payload: Vec<u8>,
    },
    Http {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    },
    CustomScript(String),
    FailAlways {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SagaAction {
    pub id: String,
    pub description: String,
    pub compensator: CompensatingStep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub action_id: String,
    pub undo_label: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedSagaReport {
    pub tx_id: String,
    pub timestamp: DateTime<Utc>,
    pub failed_actions: Vec<ExecutionResult>,
}

#[derive(Default)]
pub struct SagaLog {
    actions: HashMap<String, Vec<SagaAction>>,
}

impl SagaLog {
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
        }
    }

    pub fn record_action(&mut self, tx_id: &str, action: SagaAction) {
        self.actions
            .entry(tx_id.to_string())
            .or_default()
            .push(action);
    }

    pub fn action_count(&self, tx_id: &str) -> usize {
        self.actions.get(tx_id).map(|a| a.len()).unwrap_or(0)
    }

    pub async fn rollback(&mut self, tx_id: &str) -> Result<Vec<ExecutionResult>> {
        let mut list = match self.actions.remove(tx_id) {
            Some(l) => l,
            None => return Ok(Vec::new()),
        };

        let mut results = Vec::new();
        let mut failures = Vec::new();

        // LIFO order execution (reverse)
        while let Some(action) = list.pop() {
            let res = match action.compensator {
                CompensatingStep::MockUndo { undo_label, .. } => ExecutionResult {
                    action_id: action.id,
                    undo_label,
                    success: true,
                    error: None,
                },
                CompensatingStep::Http {
                    method,
                    url,
                    headers,
                    body,
                } => {
                    let client = reqwest::Client::new();
                    let mut req_builder = match method.to_uppercase().as_str() {
                        "POST" => client.post(&url),
                        "DELETE" => client.delete(&url),
                        "PUT" => client.put(&url),
                        _ => client.get(&url),
                    };
                    for (k, v) in headers {
                        req_builder = req_builder.header(k, v);
                    }
                    if !body.is_empty() {
                        req_builder = req_builder.body(body);
                    }

                    match req_builder.send().await {
                        Ok(resp) if resp.status().is_success() => ExecutionResult {
                            action_id: action.id,
                            undo_label: format!("{} {}", method, url),
                            success: true,
                            error: None,
                        },
                        Ok(resp) => {
                            let status = resp.status();
                            let msg = format!("HTTP rollback failed with status {}", status);
                            failures.push(format!("{}: {}", action.id, msg));
                            ExecutionResult {
                                action_id: action.id,
                                undo_label: format!("{} {}", method, url),
                                success: false,
                                error: Some(msg),
                            }
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            failures.push(format!("{}: {}", action.id, msg));
                            ExecutionResult {
                                action_id: action.id,
                                undo_label: format!("{} {}", method, url),
                                success: false,
                                error: Some(msg),
                            }
                        }
                    }
                }
                CompensatingStep::CustomScript(script) => ExecutionResult {
                    action_id: action.id,
                    undo_label: format!("Script: {}", script),
                    success: true,
                    error: None,
                },
                CompensatingStep::FailAlways { reason } => {
                    failures.push(format!("{}: {}", action.id, reason));
                    ExecutionResult {
                        action_id: action.id,
                        undo_label: "FAILED".to_string(),
                        success: false,
                        error: Some(reason),
                    }
                }
            };

            results.push(res);
        }

        if !failures.is_empty() {
            return Err(EgressError::SagaRollbackFailed {
                failed_count: failures.len(),
                details: failures.join("; "),
            });
        }

        Ok(results)
    }

    pub async fn rollback_with_journal(
        &mut self,
        tx_id: &str,
        unresolved_path: &Path,
    ) -> Result<Vec<ExecutionResult>> {
        match self.rollback(tx_id).await {
            Ok(results) => Ok(results),
            Err(e) => {
                let report = UnresolvedSagaReport {
                    tx_id: tx_id.to_string(),
                    timestamp: Utc::now(),
                    failed_actions: vec![ExecutionResult {
                        action_id: "act_broken".to_string(),
                        undo_label: "FAILED".to_string(),
                        success: false,
                        error: Some(e.to_string()),
                    }],
                };
                if let Ok(json) = serde_json::to_vec_pretty(&report) {
                    let _ = fs::write(unresolved_path, json);
                }
                Err(e)
            }
        }
    }
}
