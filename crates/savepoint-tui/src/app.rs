use savepoint_core::{Checkpoint, Transaction};

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

pub struct TimelineApp {
    pub tx: Transaction,
    pub selected_index: usize,
    pub should_quit: bool,
    pub action: Option<TimelineAction>,
    pub mode: ViewMode,
}

impl TimelineApp {
    pub fn new(tx: Transaction) -> Self {
        let initial_idx = tx.checkpoints.len().saturating_sub(1);
        Self {
            tx,
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
        if !self.tx.checkpoints.is_empty() && self.selected_index < self.tx.checkpoints.len() - 1 {
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

    pub fn selected_checkpoint(&self) -> Option<&Checkpoint> {
        self.tx.checkpoints.get(self.selected_index)
    }

    pub fn request_rewind_confirmation(&mut self) {
        if let Some(cp) = self.selected_checkpoint() {
            self.mode = ViewMode::ConfirmingRewind(cp.step_index);
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
