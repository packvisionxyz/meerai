use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{McpError, McpResult};

pub const MCP_CONFIG_FILE: &str = ".mcp.json";
pub const MAX_SERVER_NAME_CHARS: usize = 24;
pub const MAX_TOOL_NAME_CHARS: usize = 33;
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const MIN_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;

const RESERVED_SERVER_NAMES: [&str; 10] = [
    "code",
    "lsp",
    "sac",
    "skill",
    "loop",
    "memory",
    "question",
    "retrieval",
    "subagent",
    "workspace",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
    Unsupported {
        kind: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolFilter {
    All,
    Only(Vec<String>),
}

impl ToolFilter {
    pub fn allows(&self, tool: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(patterns) => patterns.iter().any(|pattern| glob_match(pattern, tool)),
        }
    }

    pub fn patterns(&self) -> &[String] {
        match self {
            Self::All => &[],
            Self::Only(patterns) => patterns,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    pub read_only: bool,
    pub timeout_ms: u64,
    pub tools: Option<ToolFilter>,
}

impl McpServerConfig {
    pub fn kind(&self) -> &str {
        match &self.transport {
            McpTransport::Stdio { .. } => "stdio",
            McpTransport::Http { .. } => "http",
            McpTransport::Unsupported { kind } => kind,
        }
    }

    pub fn is_connectable(&self) -> bool {
        self.enabled && !matches!(self.transport, McpTransport::Unsupported { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpConfig {
    pub servers: Vec<McpServerConfig>,
    pub warnings: Vec<String>,
}

impl McpConfig {
    pub fn server(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.iter().find(|server| server.name == name)
    }

    pub fn connectable(&self) -> impl Iterator<Item = &McpServerConfig> {
        self.servers.iter().filter(|server| server.is_connectable())
    }
}

pub fn expand_env(value: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'$'
            && bytes.get(index + 1) == Some(&b'{')
            && let Some(end) = value[index + 2..].find('}')
        {
            let name = &value[index + 2..index + 2 + end];
            if is_env_name(name) {
                out.push_str(&lookup(name).unwrap_or_default());
                index += 2 + end + 1;
                continue;
            }
        }
        let Some(ch) = value[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn is_env_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|ch: char| ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn read_config_file(path: &Path) -> McpResult<(McpConfig, String)> {
    let bytes = fs::read(path)
        .map_err(|error| McpError::Config(format!("read {}: {error}", path.display())))?;
    let hash = content_hash(&bytes);
    let config = parse_config(&bytes)?;
    Ok((config, hash))
}

pub fn parse_config(bytes: &[u8]) -> McpResult<McpConfig> {
    let raw: RawFile = serde_json::from_slice(bytes)
        .map_err(|error| McpError::Config(format!("parse {MCP_CONFIG_FILE}: {error}")))?;
    Ok(build_config(raw))
}

fn build_config(raw: RawFile) -> McpConfig {
    let mut config = McpConfig::default();
    for (name, entry) in raw.mcp_servers {
        match build_server(&name, entry) {
            Ok(server) => config.servers.push(server),
            Err(warning) => config.warnings.push(warning),
        }
    }
    config
        .servers
        .sort_by(|left, right| left.name.cmp(&right.name));
    config
}

fn build_server(name: &str, raw: RawServer) -> Result<McpServerConfig, String> {
    validate_server_name(name)?;
    let kind = raw.kind.as_deref().map(str::trim).unwrap_or_default();
    let (command, mut leading_args) = split_command(raw.command.as_ref())?;
    let mut env = raw.environment;
    env.extend(raw.env);

    let transport = match (kind, &command, &raw.url) {
        ("remote", _, _) if raw.url.is_none() => {
            return Err(format!(
                "mcp server '{name}' is type 'remote' but declares no 'url'"
            ));
        }
        ("local", None, _) => {
            return Err(format!(
                "mcp server '{name}' is type 'local' but declares no 'command'"
            ));
        }
        ("", _, Some(url))
        | ("http", _, Some(url))
        | ("streamable-http", _, Some(url))
        | ("remote", _, Some(url)) => non_empty(url).map(|url| McpTransport::Http {
            url: url.to_string(),
            headers: raw.headers,
        })?,
        ("", Some(command), _) | ("stdio", Some(command), _) | ("local", Some(command), _) => {
            let command = non_empty(command)?;
            leading_args.extend(raw.args);
            McpTransport::Stdio {
                command: command.to_string(),
                args: leading_args,
                env,
                cwd: raw.cwd,
            }
        }
        ("sse" | "ws" | "websocket", _, _) => McpTransport::Unsupported {
            kind: kind.to_string(),
        },
        (other, _, _) => {
            return Err(format!(
                "mcp server '{name}' declares neither 'command' nor 'url' (type '{other}')"
            ));
        }
    };
    Ok(McpServerConfig {
        name: name.to_string(),
        transport,
        enabled: raw.enabled.unwrap_or(true),
        read_only: raw.read_only.or(raw.read_only_camel).unwrap_or(false),
        timeout_ms: resolve_timeout(raw.timeout_ms, raw.timeout),
        tools: raw.tools.map(|tools| {
            if tools.allow.is_empty() {
                ToolFilter::All
            } else {
                ToolFilter::Only(tools.allow)
            }
        }),
    })
}

fn split_command(raw: Option<&RawCommand>) -> Result<(Option<String>, Vec<String>), String> {
    match raw {
        None => Ok((None, Vec::new())),
        Some(RawCommand::Line(line)) => Ok((Some(line.clone()), Vec::new())),
        Some(RawCommand::List(parts)) => {
            let mut parts = parts.iter();
            let Some(first) = parts.next() else {
                return Err("mcp command array must not be empty".to_string());
            };
            Ok((Some(first.clone()), parts.cloned().collect()))
        }
    }
}

fn non_empty(value: &str) -> Result<&str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("value must not be empty".to_string());
    }
    Ok(trimmed)
}

fn resolve_timeout(timeout_ms: Option<u64>, timeout_secs: Option<u64>) -> u64 {
    let requested = timeout_ms.or_else(|| timeout_secs.map(|secs| secs.saturating_mul(1_000)));
    requested
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
}

pub fn validate_server_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("mcp server name must not be empty".to_string());
    }
    if name.chars().count() > MAX_SERVER_NAME_CHARS {
        return Err(format!(
            "mcp server name '{name}' exceeds {MAX_SERVER_NAME_CHARS} characters"
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
    {
        return Err(format!(
            "mcp server name '{name}' must use only lowercase letters, digits, '_' or '-'"
        ));
    }
    if RESERVED_SERVER_NAMES.contains(&name) {
        return Err(format!(
            "mcp server name '{name}' collides with a built-in toolset"
        ));
    }
    Ok(())
}

pub fn tool_name(server: &str, tool: &str) -> String {
    let raw = format!("mcp__{server}__{tool}");
    if raw.chars().count() <= 64 {
        return raw;
    }
    let mut trimmed: String = tool.chars().take(MAX_TOOL_NAME_CHARS).collect();
    trimmed = format!("mcp__{server}__{trimmed}");
    trimmed
}

pub fn parse_tool_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    pattern == value
}

#[derive(Debug, Default, Deserialize)]
struct RawFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, RawServer>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawCommand {
    Line(String),
    List(Vec<String>),
}

#[derive(Debug, Default, Deserialize)]
struct RawServer {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    command: Option<RawCommand>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, rename = "read_only")]
    read_only: Option<bool>,
    #[serde(default, rename = "readOnly")]
    read_only_camel: Option<bool>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    tools: Option<RawTools>,
}

#[derive(Debug, Default, Deserialize)]
struct RawTools {
    #[serde(default)]
    allow: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedProject {
    pub project_root: PathBuf,
    pub hash: String,
}
