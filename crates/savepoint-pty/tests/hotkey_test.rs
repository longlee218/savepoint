use savepoint_pty::{filter_input_bytes, SupervisorEvent};

#[test]
fn test_hotkey_filtering() {
    // Normal typing: "hello world"
    let input = b"hello world\n";
    let (forwarded, events) = filter_input_bytes(input);
    assert_eq!(&forwarded, input);
    assert!(events.is_empty());

    // Typing with Ctrl+R (0x12) inserted: "prompt" + Ctrl+R + "more"
    let mut mixed = Vec::new();
    mixed.extend_from_slice(b"prompt ");
    mixed.push(0x12); // Ctrl+R
    mixed.extend_from_slice(b"more");

    let (forwarded, events) = filter_input_bytes(&mixed);
    assert_eq!(&forwarded, b"prompt more");
    assert_eq!(events, vec![SupervisorEvent::RewindRequested]);

    // Typing with Ctrl+S (0x13)
    let save_input = vec![0x13];
    let (forwarded, events) = filter_input_bytes(&save_input);
    assert!(forwarded.is_empty());
    assert_eq!(events, vec![SupervisorEvent::CheckpointRequested]);
}
