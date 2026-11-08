use crate::error::{AgentError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseConfig {
    pub base_port: u16,
    pub port_stride: u16,
    pub max_agents: usize,
    pub db_prefix: String,
    pub cache_prefix: String,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            base_port: 3000,
            port_stride: 100,
            max_agents: 64,
            db_prefix: "app_".to_string(),
            cache_prefix: "sp_".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLease {
    pub agent_id: String,
    pub port_offset: u16,
    pub db_namespace: String,
    pub cache_prefix: String,
    pub sandbox_dir: PathBuf,
    pub acquired_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AgentLeaseManager {
    root_dir: PathBuf,
    config: LeaseConfig,
}

impl AgentLeaseManager {
    pub fn new(root_dir: impl Into<PathBuf>, config: LeaseConfig) -> Self {
        let root = root_dir.into();
        Self {
            root_dir: root,
            config,
        }
    }

    fn lease_file(&self, agent_id: &str) -> PathBuf {
        self.root_dir.join(format!("{}.json", agent_id))
    }

    pub fn acquire(&self, agent_id: &str) -> Result<AgentLease> {
        std::fs::create_dir_all(&self.root_dir)
            .map_err(|e| AgentError::io(&self.root_dir, e))?;

        let file = self.lease_file(agent_id);
        if file.exists() {
            return Err(AgentError::LeaseAlreadyExists(agent_id.to_string()));
        }

        let active_leases = self.list_active();
        if active_leases.iter().any(|l| l.agent_id == agent_id) {
            return Err(AgentError::LeaseAlreadyExists(agent_id.to_string()));
        }

        let used_ports: HashSet<u16> = active_leases.iter().map(|l| l.port_offset).collect();

        // Find next free port offset: slot 1..=max_agents
        let mut allocated_port = None;
        for slot in 1..=self.config.max_agents {
            let port = self.config.base_port + (slot as u16) * self.config.port_stride;
            if !used_ports.contains(&port) {
                allocated_port = Some(port);
                break;
            }
        }

        let port_offset = allocated_port.ok_or(AgentError::PortExhaustion(self.config.max_agents))?;

        let sandbox_dir = self.root_dir.join("sandboxes").join(agent_id);
        std::fs::create_dir_all(&sandbox_dir)
            .map_err(|e| AgentError::io(&sandbox_dir, e))?;

        let lease = AgentLease {
            agent_id: agent_id.to_string(),
            port_offset,
            db_namespace: format!("{}{}", self.config.db_prefix, agent_id),
            cache_prefix: format!("{}{}:", self.config.cache_prefix, agent_id),
            sandbox_dir,
            acquired_at: Utc::now(),
        };

        let json = serde_json::to_string_pretty(&lease)?;
        std::fs::write(&file, json).map_err(|e| AgentError::io(&file, e))?;

        Ok(lease)
    }

    pub fn release(&self, agent_id: &str) -> Result<()> {
        let file = self.lease_file(agent_id);
        if !file.exists() {
            return Err(AgentError::LeaseNotFound(agent_id.to_string()));
        }
        std::fs::remove_file(&file).map_err(|e| AgentError::io(&file, e))?;
        Ok(())
    }

    pub fn get_lease(&self, agent_id: &str) -> Result<AgentLease> {
        let file = self.lease_file(agent_id);
        if !file.exists() {
            return Err(AgentError::LeaseNotFound(agent_id.to_string()));
        }
        let data = std::fs::read_to_string(&file).map_err(|e| AgentError::io(&file, e))?;
        let lease = serde_json::from_str(&data)?;
        Ok(lease)
    }

    pub fn list_active(&self) -> Vec<AgentLease> {
        if !self.root_dir.exists() {
            return Vec::new();
        }

        let Ok(entries) = std::fs::read_dir(&self.root_dir) else {
            return Vec::new();
        };

        let mut leases = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Ok(data) = std::fs::read_to_string(&path) {
                    if let Ok(lease) = serde_json::from_str::<AgentLease>(&data) {
                        leases.push(lease);
                    }
                }
            }
        }
        leases.sort_by_key(|l| l.port_offset);
        leases
    }
}
