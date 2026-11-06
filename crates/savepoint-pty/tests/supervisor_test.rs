use savepoint_pty::{PtySupervisor, SupervisorEvent};
use std::time::Duration;

#[test]
fn test_pty_supervisor_echo() {
    let supervisor = PtySupervisor::spawn("echo", &["savepoint_test_pty"], None)
        .expect("Failed to spawn PTY echo");

    // Wait for process exit event
    let event = supervisor
        .events_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("Did not receive event in time");

    assert_eq!(event, SupervisorEvent::ChildExited(0));
}
