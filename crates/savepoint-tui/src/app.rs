use savepoint_core::{Checkpoint, FileDelta, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineAction {
    Rewind(usize),
    Commit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Normal,
    ConfirmingRewind(usize),
}

#[derive(Debug, Clone)]
pub enum TimelineRow {
    Live { deltas: Vec<FileDelta> },
    Saved(Checkpoint),
}

pub struct TimelineApp {
    pub tx: Transaction,
    pub rows: Vec<TimelineRow>,
    pub selected_index: usize,
    pub should_quit: bool,
    pub action: Option<TimelineAction>,
    pub mode: ViewMode,
}

impl TimelineApp {
    pub fn new(tx: Transaction, live_deltas: Vec<FileDelta>) -> Self {
        let mut rows = Vec::new();

        // 1. If there are live uncommitted changes, show at top
        if !live_deltas.is_empty() {
            rows.push(TimelineRow::Live { deltas: live_deltas });
        }

        // 2. Add saved checkpoints (newest first for natural reading)
        for cp in tx.checkpoints.iter().rev() {
            rows.push(TimelineRow::Saved(cp.clone()));
        }

        // If live changes exist, default cursor to the previous checkpoint (index 1)
        // so pressing Enter immediately offers to rewind back!
        let initial_idx = if rows.len() > 1 && matches!(rows[0], TimelineRow::Live { .. }) {
            1
        } else {
            0
        };

        Self {
            tx,
            rows,
            selected_index: initial_idx,
            should_quit: false,
            action: None,
            mode: ViewMode::Normal,
        }
    }

    pub fn next(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        if !self.rows.is_empty() && self.selected_index < self.rows.len() - 1 {
            self.selected_index += 1;
        }
    }

    pub fn previous(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn selected_row(&self) -> Option<&TimelineRow> {
        self.rows.get(self.selected_index)
    }

    pub fn request_rewind_confirmation(&mut self) {
        if let Some(row) = self.selected_row() {
            match row {
                TimelineRow::Saved(cp) => {
                    self.mode = ViewMode::ConfirmingRewind(cp.step_index);
                }
                TimelineRow::Live { .. } => {
                    // Already at live state
                }
            }
        }
    }

    pub fn confirm_rewind(&mut self) {
        if let ViewMode::ConfirmingRewind(step) = self.mode {
            self.action = Some(TimelineAction::Rewind(step));
            self.should_quit = true;
        }
    }

    pub fn cancel_confirmation(&mut self) {
        self.mode = ViewMode::Normal;
    }

    pub fn request_commit(&mut self) {
        self.action = Some(TimelineAction::Commit);
        self.should_quit = true;
    }

    pub fn cancel(&mut self) {
        if self.mode != ViewMode::Normal {
            self.mode = ViewMode::Normal;
        } else {
            self.action = Some(TimelineAction::Cancel);
            self.should_quit = true;
        }
    }
}
