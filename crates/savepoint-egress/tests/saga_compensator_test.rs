use savepoint_egress::saga::{CompensatingStep, SagaAction, SagaLog};
use tempfile::tempdir;

#[tokio::test]
async fn test_saga_compensator_executes_lifo_undo_actions() {
    let mut saga = SagaLog::new();
    let tx_id = "tx_payment_flow";

    // Action 1: Create Stripe customer (earlier)
    saga.record_action(
        tx_id,
        SagaAction {
            id: "act_1".into(),
            description: "POST /v1/customers".into(),
            compensator: CompensatingStep::MockUndo {
                undo_label: "DELETE /v1/customers/cus_999".into(),
                payload: b"{}".to_vec(),
            },
        },
    );

    // Action 2: Create Stripe charge (later)
    saga.record_action(
        tx_id,
        SagaAction {
            id: "act_2".into(),
            description: "POST /v1/charges".into(),
            compensator: CompensatingStep::MockUndo {
                undo_label: "POST /v1/refunds".into(),
                payload: b"{\"charge\":\"ch_888\"}".to_vec(),
            },
        },
    );

    assert_eq!(saga.action_count(tx_id), 2);

    // Rollback: trigger compensating actions in LIFO order
    let executions = saga.rollback(tx_id).await.unwrap();
    assert_eq!(executions.len(), 2);
    // LIFO check: Action 2 (refund charge) MUST execute before Action 1 (delete customer)!
    assert_eq!(executions[0].undo_label, "POST /v1/refunds");
    assert_eq!(executions[1].undo_label, "DELETE /v1/customers/cus_999");
    assert_eq!(saga.action_count(tx_id), 0, "All actions for tx should be cleared after rollback");
}

#[tokio::test]
async fn test_saga_partial_failure_logs_unresolved_manifest() {
    let dir = tempdir().unwrap();
    let unresolved_path = dir.path().join("saga_unresolved.json");

    let mut saga = SagaLog::new();
    let tx_id = "tx_partial_fail";

    saga.record_action(
        tx_id,
        SagaAction {
            id: "act_ok".into(),
            description: "Normal action".into(),
            compensator: CompensatingStep::MockUndo {
                undo_label: "UNDO_OK".into(),
                payload: b"{}".to_vec(),
            },
        },
    );

    saga.record_action(
        tx_id,
        SagaAction {
            id: "act_broken".into(),
            description: "Failing action".into(),
            compensator: CompensatingStep::FailAlways {
                reason: "Simulated 500 Network Timeout on AWS S3".into(),
            },
        },
    );

    let err = saga.rollback_with_journal(tx_id, &unresolved_path).await.unwrap_err();
    assert!(err.to_string().contains("Saga rollback failure"));
    assert!(unresolved_path.exists(), "Unresolved saga report must be written");

    let content = std::fs::read_to_string(&unresolved_path).unwrap();
    assert!(content.contains("Simulated 500 Network Timeout on AWS S3"));
    assert!(content.contains("act_broken"));
}
