use crate::app::{TimelineAction, TimelineApp, ViewMode};
use crate::ui::render_ui;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use savepoint_core::{FileDelta, Transaction};
use std::io::{stdout, Write};
use std::time::Duration;

/// Runs the interactive TUI timeline for the provided transaction and live deltas.
/// Returns the action chosen by the user (`Rewind(step)`, `Commit`, or `Cancel`).
pub fn run_timeline(
    tx: Transaction,
    live_deltas: Vec<FileDelta>,
) -> anyhow::Result<TimelineAction> {
    // Set panic hook to ensure terminal is always restored even on unhandled panic
    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
        let _ = std::io::stdout().flush();
        default_panic_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TimelineApp::new(tx, live_deltas);

    let res = run_loop(&mut terminal, &mut app);

    // Guarantee clean terminal restoration in correct order:
    // 1. Leave alternate screen and show cursor while still in terminal context
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = terminal.backend_mut().flush();

    // 2. Disable raw mode AFTER exiting alternate screen
    let _ = disable_raw_mode();
    let _ = std::io::stdout().flush();

    res?;
    Ok(app.action.unwrap_or(TimelineAction::Cancel))
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut TimelineApp,
) -> anyhow::Result<()> {
    while !app.should_quit {
        terminal.draw(|f| render_ui(f, app))?;

        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.mode {
                        ViewMode::ConfirmingRewind(_) => match key.code {
                            KeyCode::Char('c')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                app.cancel_confirmation();
                            }
                            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                app.confirm_rewind();
                            }
                            KeyCode::Char('n')
                            | KeyCode::Char('N')
                            | KeyCode::Esc
                            | KeyCode::Char('q') => {
                                app.cancel_confirmation();
                            }
                            _ => {}
                        },
                        ViewMode::Normal => match key.code {
                            KeyCode::Char('c')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                app.cancel();
                            }
                            KeyCode::Up | KeyCode::Char('k') => app.previous(),
                            KeyCode::Down | KeyCode::Char('j') => app.next(),
                            KeyCode::Home | KeyCode::Char('g') => app.first(),
                            KeyCode::End | KeyCode::Char('G') => app.last(),
                            KeyCode::PageDown | KeyCode::Char('J') | KeyCode::Char(']') => {
                                app.scroll_details_down();
                            }
                            KeyCode::PageUp | KeyCode::Char('K') | KeyCode::Char('[') => {
                                app.scroll_details_up();
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.scroll_details_down();
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.scroll_details_up();
                            }
                            KeyCode::Char('u') => app.request_undo(),
                            KeyCode::Char('r') | KeyCode::Enter => {
                                app.request_rewind_confirmation();
                            }
                            KeyCode::Char('c') => app.request_commit(),
                            KeyCode::Char('q') | KeyCode::Esc => app.cancel(),
                            _ => {}
                        },
                    }
                }
            }
        }
    }
    Ok(())
}
