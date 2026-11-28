use std::path::Path;

use crate::client::McpConnection;
use crate::config::{MCP_CONFIG_FILE, McpConfig, McpServerConfig, McpTransport, read_config_file};
use crate::merge::merge_configs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    Disabled,
    Unsupported,
    Untrusted,
    Configured,
    Connected,
    Failed,
}

impl ServerStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Unsupported => "unsupported",
            Self::Untrusted => "needs trust",
            Self::Configured => "configured",
            Self::Connected => "connected",
            Self::Failed => "failed",
        }
    }

    pub fn is_connected(self) -> bool {
        self == Self::Connected
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerReport {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub status: ServerStatus,
    pub tools: Vec<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpSetupReport {
    pub servers: Vec<ServerReport>,
    pub configs: Vec<McpServerConfig>,
    pub warnings: Vec<String>,
    pub project_untrusted: bool,
    pub project_config_present: bool,
}

impl McpSetupReport {
    pub fn connectable_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|server| server.status == ServerStatus::Configured)
            .map(|server| server.name.clone())
            .collect()
    }

    pub fn report_mut(&mut self, name: &str) -> Option<&mut ServerReport> {
        self.servers.iter_mut().find(|server| server.name == name)
    }
}

pub fn load_user_config(config_dir: Option<&Path>) -> (McpConfig, Vec<String>) {
    let Some(dir) = config_dir else {
        return (McpConfig::default(), Vec::new());
    };
    read_optional(&dir.join(MCP_CONFIG_FILE))
}

pub fn load_project_config(
    project_root: &Path,
) -> (Option<McpConfig>, Option<String>, Vec<String>) {
    let path = project_root.join(MCP_CONFIG_FILE);
    if !path.is_file() {
        return (None, None, Vec::new());
    }
    match read_config_file(&path) {
        Ok((config, hash)) => {
            let warnings = config.warnings.clone();
            (Some(config), Some(hash), warnings)
        }
        Err(error) => (
            None,
            None,
            vec![format!("ignoring project {MCP_CONFIG_FILE}: {error}")],
        ),
    }
}

fn read_optional(path: &Path) -> (McpConfig, Vec<String>) {
    if !path.is_file() {
        return (McpConfig::default(), Vec::new());
    }
    match read_config_file(path) {
        Ok((config, _)) => {
            let warnings = config.warnings.clone();
            (config, warnings)
        }
        Err(error) => (
            McpConfig::default(),
            vec![format!("ignoring {}: {error}", path.display())],
        ),
    }
}

pub fn build_setup(
    config_dir: Option<&Path>,
    project_root: &Path,
    trusted_hash: Option<&str>,
) -> McpSetupReport {
    let (user, mut warnings) = load_user_config(config_dir);
    let (project, project_hash, project_warnings) = load_project_config(project_root);
    warnings.extend(project_warnings);

    let project_config_present = project.is_some();
    let merged = merge_configs(user, project, project_hash.as_deref(), trusted_hash);
    warnings.extend(merged.warnings.clone());

    let mut servers: Vec<ServerReport> = merged
        .config
        .servers
        .iter()
        .map(|server| report_for(server, false))
        .chain(
            merged
                .untrusted_servers
                .iter()
                .filter(|server| {
                    !merged
                        .config
                        .servers
                        .iter()
                        .any(|user| user.name == server.name)
                })
                .map(|server| report_for(server, true)),
        )
        .collect();
    servers.sort_by(|left, right| left.name.cmp(&right.name));

    McpSetupReport {
        configs: merged.config.servers.clone(),
        servers,
        warnings,
        project_untrusted: merged.project_untrusted,
        project_config_present,
    }
}

fn report_for(server: &McpServerConfig, project_untrusted: bool) -> ServerReport {
    let (status, detail) = if project_untrusted {
        (ServerStatus::Untrusted, Some("run /mcp trust".to_string()))
    } else if !server.enabled {
        (ServerStatus::Disabled, None)
    } else if matches!(server.transport, McpTransport::Unsupported { .. }) {
        (
            ServerStatus::Unsupported,
            Some(format!(
                "transport '{}' is not supported yet",
                server.kind()
            )),
        )
    } else {
        (ServerStatus::Configured, None)
    };

    ServerReport {
        name: server.name.clone(),
        kind: server.kind().to_string(),
        enabled: server.enabled,
        status,
        tools: Vec::new(),
        detail,
    }
}

pub fn apply_connection(report: &mut ServerReport, connection: &McpConnection) {
    report.status = ServerStatus::Connected;
    report.tools = connection
        .tools()
        .iter()
        .map(|tool| tool.runtime_name())
        .collect();
    report.detail = if connection.warnings().is_empty() {
        None
    } else {
        Some(connection.warnings().join("; "))
    };
}

pub fn mark_failed(report: &mut ServerReport, error: &str) {
    report.status = ServerStatus::Failed;
    report.tools.clear();
    report.detail = Some(error.to_string());
}

pub fn ready_servers(config: &McpConfig) -> Vec<&McpServerConfig> {
    config.connectable().collect()
}

pub fn read_only_servers(config: &McpConfig) -> Vec<&McpServerConfig> {
    config
        .connectable()
        .filter(|server| server.read_only)
        .collect()
}
