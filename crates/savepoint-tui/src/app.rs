use savepoint_core::{Checkpoint, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineAction {
    Rewind(usize),
    Commit,
    Cancel,
}

pub struct TimelineApp {
    pub tx: Transaction,
    pub selected_index: usize,
    pub should_quit: bool,
    pub action: Option<TimelineAction>,
}

impl TimelineApp {
    pub fn new(tx: Transaction) -> Self {
        let initial_idx = tx.checkpoints.len().saturating_sub(1);
        Self {
            tx,
            selected_index: initial_idx,
            should_quit: false,
            action: None,
        }
    }

    pub fn next(&mut self) {
        if !self.tx.checkpoints.is_empty() && self.selected_index < self.tx.checkpoints.len() - 1 {
            self.selected_index += 1;
        }
    }

    pub fn previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn selected_checkpoint(&self) -> Option<&Checkpoint> {
        self.tx.checkpoints.get(self.selected_index)
    }

    pub fn request_rewind(&mut self) {
        if let Some(cp) = self.selected_checkpoint() {
            self.action = Some(TimelineAction::Rewind(cp.step_index));
            self.should_quit = true;
        }
    }

    pub fn request_commit(&mut self) {
        self.action = Some(TimelineAction::Commit);
        self.should_quit = true;
    }

    pub fn cancel(&mut self) {
        self.action = Some(TimelineAction::Cancel);
        self.should_quit = true;
    }
}
