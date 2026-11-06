use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::stdout;
use std::time::Duration;
use savepoint_core::Transaction;
use crate::app::{TimelineAction, TimelineApp};
use crate::ui::render_ui;

/// Runs the interactive TUI timeline for the provided transaction.
/// Returns the action chosen by the user (`Rewind(step)`, `Commit`, or `Cancel`).
pub fn run_timeline(tx: Transaction) -> anyhow::Result<TimelineAction> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TimelineApp::new(tx);

    let res = run_loop(&mut terminal, &mut app);

    // Guarantee cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res?;
    Ok(app.action.unwrap_or(TimelineAction::Cancel))
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut TimelineApp,
) -> anyhow::Result<()> {
    while !app.should_quit {
        terminal.draw(|f| render_ui(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => app.previous(),
                        KeyCode::Down | KeyCode::Char('j') => app.next(),
                        KeyCode::Enter => app.request_rewind(),
                        KeyCode::Char('c') => app.request_commit(),
                        KeyCode::Char('q') | KeyCode::Esc => app.cancel(),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}
