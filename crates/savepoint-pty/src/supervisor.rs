use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use savepoint_core::{Result, SavepointError};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;

#[derive(Debug, PartialEq, Eq)]
pub enum SupervisorEvent {
    RewindRequested,
    CheckpointRequested,
    ChildExited(u32),
}

pub struct PtySupervisor {
    pub events_rx: Receiver<SupervisorEvent>,
    running: Arc<AtomicBool>,
}

/// Helper function to filter hotkeys from incoming stdin stream.
/// Returns normal bytes to forward to the child PTY, plus any detected events.
pub fn filter_input_bytes(input: &[u8]) -> (Vec<u8>, Vec<SupervisorEvent>) {
    let mut forwarded = Vec::with_capacity(input.len());
    let mut events = Vec::new();

    for &byte in input {
        match byte {
            0x12 => events.push(SupervisorEvent::RewindRequested), // Ctrl+R
            0x13 => events.push(SupervisorEvent::CheckpointRequested), // Ctrl+S
            other => forwarded.push(other),
        }
    }

    (forwarded, events)
}

impl PtySupervisor {
    /// Spawns a child command in an isolated PTY, streaming I/O.
    /// Intercepts Ctrl+R (0x12) and Ctrl+S (0x13) to emit `SupervisorEvent`.
    pub fn spawn(program: &str, args: &[&str], cwd: Option<&std::path::Path>) -> Result<Self> {
        let pty_system = native_pty_system();
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| SavepointError::Other(e.to_string()))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SavepointError::Other(e.to_string()))?;

        // Drop slave in parent process so EOF works correctly
        drop(pair.slave);

        let (events_tx, events_rx) = channel();
        let running = Arc::new(AtomicBool::new(true));

        // Thread 1: Reader (Child stdout -> Host stdout)
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SavepointError::Other(e.to_string()))?;
        let running_reader = Arc::clone(&running);
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut host_stdout = std::io::stdout();
            while running_reader.load(Ordering::Relaxed) {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let _ = host_stdout.write_all(&buf[..n]);
                        let _ = host_stdout.flush();
                    }
                    Err(_) => break,
                }
            }
        });

        // Thread 2: Child process watcher
        let running_child = Arc::clone(&running);
        let events_tx_child = events_tx.clone();
        thread::spawn(move || {
            let status = child.wait();
            let exit_code = match status {
                Ok(s) => s.exit_code(),
                Err(_) => 1,
            };
            running_child.store(false, Ordering::Relaxed);
            let _ = events_tx_child.send(SupervisorEvent::ChildExited(exit_code));
        });

        // Thread 3: Writer (Host stdin -> Child stdin with hotkey filter)
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| SavepointError::Other(e.to_string()))?;
        let running_writer = Arc::clone(&running);
        thread::spawn(move || {
            let mut host_stdin = std::io::stdin();
            let mut buf = [0u8; 1024];
            while running_writer.load(Ordering::Relaxed) {
                match host_stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let (forwarded, events) = filter_input_bytes(&buf[..n]);
                        for event in events {
                            let _ = events_tx.send(event);
                        }
                        if !forwarded.is_empty() {
                            let _ = writer.write_all(&forwarded);
                            let _ = writer.flush();
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self { events_rx, running })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}
