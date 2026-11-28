use crate::config::{McpConfig, McpServerConfig, ToolFilter};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    pub config: McpConfig,
    pub warnings: Vec<String>,
    pub project_untrusted: bool,
    pub untrusted_servers: Vec<McpServerConfig>,
}

pub fn merge_configs(
    user: McpConfig,
    project: Option<McpConfig>,
    project_hash: Option<&str>,
    trusted_hash: Option<&str>,
) -> MergeOutcome {
    let mut warnings = Vec::new();
    let Some(project) = project else {
        return MergeOutcome {
            config: user,
            warnings,
            project_untrusted: false,
            untrusted_servers: Vec::new(),
        };
    };

    let trusted = match (project_hash, trusted_hash) {
        (Some(hash), Some(trusted)) => hash == trusted,
        _ => false,
    };

    let mut merged = user;

    if !trusted {
        if !project.servers.is_empty() {
            let names: Vec<&str> = project
                .servers
                .iter()
                .map(|server| server.name.as_str())
                .collect();
            warnings.push(format!(
                "project {} declares mcp server(s) {}; run `/mcp trust` to accept them",
                crate::config::MCP_CONFIG_FILE,
                names.join(", ")
            ));
        }
        warnings.extend(project.warnings);
        let untrusted_servers = project.servers.clone();
        return MergeOutcome {
            config: merged,
            warnings,
            project_untrusted: !project.servers.is_empty(),
            untrusted_servers,
        };
    }

    for server in project.servers {
        match merged
            .servers
            .iter_mut()
            .find(|existing| existing.name == server.name)
        {
            Some(existing) => tighten_server(existing, server, &mut warnings),
            None => merged.servers.push(server),
        }
    }
    merged
        .servers
        .sort_by(|left, right| left.name.cmp(&right.name));
    warnings.extend(project.warnings);
    warnings.extend(merged.warnings.clone());

    MergeOutcome {
        config: merged,
        warnings,
        project_untrusted: false,
        untrusted_servers: Vec::new(),
    }
}

fn tighten_server(
    existing: &mut McpServerConfig,
    project: McpServerConfig,
    warnings: &mut Vec<String>,
) {
    if project.enabled && !existing.enabled {
        warnings.push(format!(
            "ignoring project mcp server '{}' enable: a project file cannot enable a disabled server",
            existing.name
        ));
    }
    if !project.enabled {
        existing.enabled = false;
    }

    if project.read_only {
        existing.read_only = true;
    } else if existing.read_only {
        warnings.push(format!(
            "ignoring project mcp server '{}' read_only=false: a project file cannot widen access",
            existing.name
        ));
    }

    if let Some(project_tools) = project.tools {
        existing.tools = Some(match existing.tools.take() {
            None => project_tools,
            Some(user_tools) => narrow(user_tools, project_tools, warnings),
        });
    }
}

fn narrow(user: ToolFilter, project: ToolFilter, warnings: &mut Vec<String>) -> ToolFilter {
    match (&user, &project) {
        (_, ToolFilter::All) => user,
        (ToolFilter::All, ToolFilter::Only(patterns)) => ToolFilter::Only(patterns.clone()),
        (ToolFilter::Only(granted), ToolFilter::Only(patterns)) => {
            let mut kept = Vec::new();
            for pattern in patterns {
                if granted.iter().any(|allowed| covers(allowed, pattern)) {
                    kept.push(pattern.clone());
                } else {
                    warnings.push(format!(
                        "dropped project mcp tool filter '{pattern}': it is wider than the user allowlist"
                    ));
                }
            }
            ToolFilter::Only(kept)
        }
    }
}

fn covers(granted: &str, requested: &str) -> bool {
    if granted == "*" {
        return true;
    }
    match granted.strip_suffix('*') {
        Some(prefix) => requested.strip_suffix('*').map_or_else(
            || requested.starts_with(prefix),
            |body| body.starts_with(prefix),
        ),
        None => granted == requested,
    }
}
