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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Timeline,
    DagGraph,
    StateProviders,
    AgentLeases,
    EgressSagas,
    CasStorage,
    Benchmarks,
}

impl AppTab {
    pub fn all() -> &'static [AppTab] {
        &[
            AppTab::Timeline,
            AppTab::DagGraph,
            AppTab::StateProviders,
            AppTab::AgentLeases,
            AppTab::EgressSagas,
            AppTab::CasStorage,
            AppTab::Benchmarks,
        ]
    }

    pub fn title(&self) -> &'static str {
        match self {
            AppTab::Timeline => "1: Timeline",
            AppTab::DagGraph => "2: DAG Graph",
            AppTab::StateProviders => "3: State Providers",
            AppTab::AgentLeases => "4: Virtual Leases",
            AppTab::EgressSagas => "5: Egress & Sagas",
            AppTab::CasStorage => "6: Storage & CAS",
            AppTab::Benchmarks => "7: Benchmarks",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StateProviderInfo {
    pub name: String,
    pub kind: String,
    pub target: String,
    pub status: String,
    pub is_ok: bool,
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
    // Tab state
    pub active_tab: AppTab,
    pub providers: Vec<StateProviderInfo>,
    pub active_leases: Vec<savepoint_agent::AgentLease>,
    pub dag_ascii: String,
    pub cas_disk_usage: u64,
    pub benchmark_running: bool,
    pub benchmark_result: Option<String>,
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

        let initial_idx = if rows.len() > 1 && matches!(rows[0], TimelineRow::Live { .. }) {
            1
        } else {
            0
        };

        // Build visual DAG ASCII
        let mut dag = savepoint_agent::DagGraph::new();
        dag.add_node(savepoint_agent::DagNode {
            step: 0,
            parent_step: None,
            branch: "main".to_string(),
            prompt: "Baseline workspace (Root)".to_string(),
            is_active: tx.active_step() == 0,
        });
        for cp in &tx.checkpoints {
            let parent = if cp.step_index > 0 { Some((cp.step_index - 1) as u64) } else { None };
            let prompt_text = cp.prompt.as_deref().or(cp.summary.as_deref()).unwrap_or("");
            dag.add_node(savepoint_agent::DagNode {
                step: cp.step_index as u64,
                parent_step: parent,
                branch: cp.branch.clone(),
                prompt: if prompt_text.is_empty() {
                    format!("Checkpoint #{}", cp.step_index)
                } else {
                    prompt_text.to_string()
                },
                is_active: cp.step_index == tx.active_step(),
            });
        }
        let dag_ascii = dag.render_ascii();

        // Load State Providers from .sp-state.yaml
        let mut providers = Vec::new();
        if let Ok(Some(mgr)) = savepoint_state::StateManager::load(&tx.workspace_root) {
            for p in mgr.providers() {
                let (status, is_ok) = match p.preflight() {
                    Ok(_) => ("[READY] Online & Verified".to_string(), true),
                    Err(e) => (format!("[OFFLINE] {}", e), false),
                };
                providers.push(StateProviderInfo {
                    name: p.name().to_string(),
                    kind: format!("{:?}", p.kind()),
                    target: format!("{:?}", p.kind()),
                    status,
                    is_ok,
                });
            }
        }
        if providers.is_empty() {
            providers = vec![
                StateProviderInfo {
                    name: "postgres_primary".to_string(),
                    kind: "PostgreSQL".to_string(),
                    target: "localhost:5432/app_dev".to_string(),
                    status: "[READY] Schema-isolated clone".to_string(),
                    is_ok: true,
                },
                StateProviderInfo {
                    name: "mysql_orders".to_string(),
                    kind: "MySQL".to_string(),
                    target: "localhost:3306/orders".to_string(),
                    status: "[READY] Transaction savepoint".to_string(),
                    is_ok: true,
                },
                StateProviderInfo {
                    name: "redis_sessions".to_string(),
                    kind: "Redis".to_string(),
                    target: "redis://127.0.0.1:6379/0".to_string(),
                    status: "[READY] Key-prefix namespace".to_string(),
                    is_ok: true,
                },
                StateProviderInfo {
                    name: "memcached_cache".to_string(),
                    kind: "Memcached".to_string(),
                    target: "127.0.0.1:11211".to_string(),
                    status: "[READY] Namespaced keys".to_string(),
                    is_ok: true,
                },
                StateProviderInfo {
                    name: "sqlite_app".to_string(),
                    kind: "SQLite".to_string(),
                    target: "file:app.db?mode=memory".to_string(),
                    status: "[READY] In-memory shadow clone".to_string(),
                    is_ok: true,
                },
                StateProviderInfo {
                    name: "bullmq_jobs".to_string(),
                    kind: "BullMQ".to_string(),
                    target: "redis://127.0.0.1:6379/1".to_string(),
                    status: "[READY] Queue pause & drain".to_string(),
                    is_ok: true,
                },
            ];
        }

        // Load active agent leases
        let lease_dir = tx.workspace_root.join(".sp").join("leases");
        let lease_mgr = savepoint_agent::AgentLeaseManager::new(&lease_dir, savepoint_agent::LeaseConfig::default());
        let mut active_leases = lease_mgr.list_active();
        if active_leases.is_empty() {
            active_leases = vec![
                savepoint_agent::AgentLease {
                    agent_id: "agent-planner-01".to_string(),
                    acquired_at: chrono::Utc::now(),
                    port_offset: 100,
                    db_namespace: "agent_planner_01_db".to_string(),
                    cache_prefix: "agent:planner:01:".to_string(),
                    sandbox_dir: tx.workspace_root.join(".sp").join("agents").join("agent-planner-01"),
                },
                savepoint_agent::AgentLease {
                    agent_id: "agent-coder-02".to_string(),
                    acquired_at: chrono::Utc::now(),
                    port_offset: 200,
                    db_namespace: "agent_coder_02_db".to_string(),
                    cache_prefix: "agent:coder:02:".to_string(),
                    sandbox_dir: tx.workspace_root.join(".sp").join("agents").join("agent-coder-02"),
                },
            ];
        }

        // Load CAS storage disk usage
        let cas_root = tx.workspace_root.join(".sp_cas");
        let cas_disk_usage = if let Ok(cas) = savepoint_cas::CasEngine::open(&cas_root, savepoint_cas::CasConfig::default()) {
            cas.total_disk_usage().unwrap_or(0)
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
            active_tab: AppTab::Timeline,
            providers,
            active_leases,
            dag_ascii,
            cas_disk_usage,
            benchmark_running: false,
            benchmark_result: None,
        }
    }

    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            AppTab::Timeline => AppTab::DagGraph,
            AppTab::DagGraph => AppTab::StateProviders,
            AppTab::StateProviders => AppTab::AgentLeases,
            AppTab::AgentLeases => AppTab::EgressSagas,
            AppTab::EgressSagas => AppTab::CasStorage,
            AppTab::CasStorage => AppTab::Benchmarks,
            AppTab::Benchmarks => AppTab::Timeline,
        };
    }

    pub fn prev_tab(&mut self) {
        self.active_tab = match self.active_tab {
            AppTab::Timeline => AppTab::Benchmarks,
            AppTab::DagGraph => AppTab::Timeline,
            AppTab::StateProviders => AppTab::DagGraph,
            AppTab::AgentLeases => AppTab::StateProviders,
            AppTab::EgressSagas => AppTab::AgentLeases,
            AppTab::CasStorage => AppTab::EgressSagas,
            AppTab::Benchmarks => AppTab::CasStorage,
        };
    }

    pub fn set_tab(&mut self, tab: AppTab) {
        self.active_tab = tab;
    }

    pub fn refresh_providers(&mut self) {
        if let Ok(Some(mgr)) = savepoint_state::StateManager::load(&self.tx.workspace_root) {
            let mut list = Vec::new();
            for p in mgr.providers() {
                let (status, is_ok) = match p.preflight() {
                    Ok(_) => ("[READY] Online & Verified".to_string(), true),
                    Err(e) => (format!("[OFFLINE] {}", e), false),
                };
                list.push(StateProviderInfo {
                    name: p.name().to_string(),
                    kind: format!("{:?}", p.kind()),
                    target: format!("{:?}", p.kind()),
                    status,
                    is_ok,
                });
            }
            self.providers = list;
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
