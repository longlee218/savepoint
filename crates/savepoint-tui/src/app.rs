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
    pub delta_scroll: u16,
    pub should_quit: bool,
    pub action: Option<TimelineAction>,
    pub mode: ViewMode,
}

impl TimelineApp {
    pub fn new(tx: Transaction, live_deltas: Vec<FileDelta>) -> Self {
        let mut rows = Vec::new();

        // 1. If there are live uncommitted changes, show at top
        if !live_deltas.is_empty() {
            rows.push(TimelineRow::Live {
                deltas: live_deltas,
            });
        }

        // 2. Add saved checkpoints (newest first for natural reading)
        for cp in tx.checkpoints.iter().rev() {
            rows.push(TimelineRow::Saved(cp.clone()));
        }

        // If live changes exist, default cursor to the latest checkpoint (index 1)
        // so pressing Enter immediately offers to discard live edits and restore HEAD.
        let initial_idx = if rows.len() > 1 && matches!(rows[0], TimelineRow::Live { .. }) {
            1
        } else {
            0
        };

        Self {
            tx,
            rows,
            selected_index: initial_idx,
            delta_scroll: 0,
            should_quit: false,
            action: None,
            mode: ViewMode::Normal,
        }
    }

    pub fn has_live_changes(&self) -> bool {
        matches!(self.rows.first(), Some(TimelineRow::Live { .. }))
    }

    pub fn next(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        if !self.rows.is_empty() && self.selected_index < self.rows.len() - 1 {
            self.selected_index += 1;
            self.delta_scroll = 0;
        }
    }

    pub fn previous(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.delta_scroll = 0;
        }
    }

    pub fn first(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        if !self.rows.is_empty() && self.selected_index != 0 {
            self.selected_index = 0;
            self.delta_scroll = 0;
        }
    }

    pub fn last(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        if !self.rows.is_empty() {
            let last_idx = self.rows.len() - 1;
            if self.selected_index != last_idx {
                self.selected_index = last_idx;
                self.delta_scroll = 0;
            }
        }
    }

    pub fn scroll_details_down(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        let max_deltas = match self.selected_row() {
            Some(TimelineRow::Live { deltas }) => deltas.len(),
            Some(TimelineRow::Saved(cp)) => cp.deltas.len(),
            None => 0,
        };
        if max_deltas > 0 {
            let max_scroll = (max_deltas.saturating_sub(1)) as u16;
            self.delta_scroll = (self.delta_scroll.saturating_add(3)).min(max_scroll.max(1));
        } else {
            self.delta_scroll = self.delta_scroll.saturating_add(1);
        }
    }

    pub fn scroll_details_up(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        self.delta_scroll = self.delta_scroll.saturating_sub(3);
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
                    // Selecting Enter/r on the uncommitted worktree row offers to restore HEAD
                    let head_step = self.tx.active_step();
                    if let Some(idx) = self.rows.iter().position(
                        |r| matches!(r, TimelineRow::Saved(cp) if cp.step_index == head_step),
                    ) {
                        self.selected_index = idx;
                        self.delta_scroll = 0;
                    }
                    self.mode = ViewMode::ConfirmingRewind(head_step);
                }
            }
        }
    }

    /// Quick-undo (`u`): targets the active checkpoint if the worktree is dirty,
    /// or the previous checkpoint before `active_step` if the worktree is clean.
    pub fn request_undo(&mut self) {
        if self.mode != ViewMode::Normal {
            return;
        }
        let active_step = self.tx.active_step();
        let target_step = if self.has_live_changes() {
            Some(active_step)
        } else {
            self.tx
                .checkpoints
                .iter()
                .filter(|cp| cp.step_index < active_step)
                .map(|cp| cp.step_index)
                .max()
        };

        if let Some(step) = target_step {
            if let Some(idx) = self
                .rows
                .iter()
                .position(|r| matches!(r, TimelineRow::Saved(cp) if cp.step_index == step))
            {
                self.selected_index = idx;
                self.delta_scroll = 0;
            }
            self.mode = ViewMode::ConfirmingRewind(step);
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
