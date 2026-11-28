use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::client::{McpConnection, connect_with};
use crate::config::McpServerConfig;
use crate::error::McpResult;
use crate::setup::{ServerReport, ServerStatus};
use crate::toolset::McpToolset;

#[derive(Debug, Default)]
struct RegistryState {
    connections: BTreeMap<String, Arc<McpConnection>>,
    failures: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct McpRegistry {
    servers: Arc<Vec<McpServerConfig>>,
    workspace: PathBuf,
    state: Arc<Mutex<RegistryState>>,
}

impl McpRegistry {
    pub fn new(servers: Vec<McpServerConfig>, workspace: impl Into<PathBuf>) -> Self {
        let mut servers = servers;
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            servers: Arc::new(servers),
            workspace: workspace.into(),
            state: Arc::new(Mutex::new(RegistryState::default())),
        }
    }

    pub fn empty(workspace: impl Into<PathBuf>) -> Self {
        Self::new(Vec::new(), workspace)
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn servers(&self) -> &[McpServerConfig] {
        &self.servers
    }

    pub fn server(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.iter().find(|server| server.name == name)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn cached(&self, name: &str) -> Option<Arc<McpConnection>> {
        self.lock().connections.get(name).cloned()
    }

    pub fn failure(&self, name: &str) -> Option<String> {
        self.lock().failures.get(name).cloned()
    }

    pub async fn ensure(&self, name: &str) -> McpResult<Arc<McpConnection>> {
        if let Some(connection) = self.cached(name) {
            return Ok(connection);
        }
        let server = self
            .server(name)
            .ok_or_else(|| crate::error::McpError::Config(format!("unknown mcp server '{name}'")))?
            .clone();
        let connection = match connect_with(
            &server,
            crate::client::default_resolver(),
            Some(&self.workspace),
        )
        .await
        {
            Ok(connection) => Arc::new(connection),
            Err(error) => {
                let message = error.to_string();
                self.lock()
                    .failures
                    .insert(name.to_string(), message.clone());
                return Err(crate::error::McpError::Transport(message));
            }
        };
        let mut state = self.lock();
        state.failures.remove(name);
        state
            .connections
            .insert(name.to_string(), connection.clone());
        Ok(connection)
    }

    pub async fn toolsets(&self) -> Vec<(String, Arc<McpConnection>)> {
        self.toolsets_filtered(false).await
    }

    pub async fn toolsets_filtered(
        &self,
        read_only_only: bool,
    ) -> Vec<(String, Arc<McpConnection>)> {
        let mut connected = Vec::new();
        for server in self.servers.iter() {
            if !server.is_connectable() {
                continue;
            }
            if read_only_only && !server.read_only {
                continue;
            }
            match self.ensure(&server.name).await {
                Ok(connection) => connected.push((server.name.clone(), connection)),
                Err(error) => {
                    self.lock()
                        .failures
                        .insert(server.name.clone(), error.to_string());
                }
            }
        }
        connected
    }

    pub fn build_toolset(connection: Arc<McpConnection>, timeout_ms: u64) -> McpToolset {
        let tools = connection.tools().to_vec();
        McpToolset::new(connection, tools, timeout_ms)
    }

    pub fn reports(&self) -> Vec<ServerReport> {
        let state = self.lock();
        self.servers
            .iter()
            .map(|server| {
                let mut report = ServerReport {
                    name: server.name.clone(),
                    kind: server.kind().to_string(),
                    enabled: server.enabled,
                    status: if !server.enabled {
                        ServerStatus::Disabled
                    } else if !server.is_connectable() {
                        ServerStatus::Unsupported
                    } else {
                        ServerStatus::Configured
                    },
                    tools: Vec::new(),
                    detail: None,
                };
                if let Some(connection) = state.connections.get(&server.name) {
                    crate::setup::apply_connection(&mut report, connection);
                } else if let Some(error) = state.failures.get(&server.name) {
                    crate::setup::mark_failed(&mut report, error);
                }
                report
            })
            .collect()
    }

    pub async fn shutdown(&self) {
        let connections: Vec<Arc<McpConnection>> = {
            let mut state = self.lock();
            let drained = std::mem::take(&mut state.connections);
            drained.into_values().collect()
        };
        for connection in connections {
            if let Ok(connection) = Arc::try_unwrap(connection) {
                let _ = connection.shutdown().await;
            }
        }
    }
}
