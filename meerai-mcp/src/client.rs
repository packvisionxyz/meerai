use std::collections::BTreeMap;

use meerai_core::ToolObservation;
use meerai_core::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, JsonObject, ProtocolVersion, Tool,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, Peer, RoleClient, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use tokio::process::Command;

use crate::config::{McpServerConfig, McpTransport};
use crate::error::{McpError, McpResult};
use crate::stderr::StderrTail;
use crate::tool::{McpTool, screen_description};

pub const MAX_INLINE_RESULT_CHARS: usize = 64 * 1024;

type Client = RunningService<RoleClient, ()>;
type HttpTransport = StreamableHttpClientTransport<reqwest::Client>;

pub struct McpConnection {
    name: String,
    peer: Peer<RoleClient>,
    service: Client,
    tools: Vec<McpTool>,
    warnings: Vec<String>,
    stderr: StderrTail,
}

impl McpConnection {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
    }

    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn stderr_tail(&self) -> Option<String> {
        self.stderr.snapshot()
    }

    pub async fn shutdown(mut self) -> McpResult<()> {
        self.service
            .close()
            .await
            .map(|_| ())
            .map_err(|error| McpError::Transport(format!("shutdown {}: {error}", self.name)))
    }
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConnection")
            .field("name", &self.name)
            .field("tools", &self.tools.len())
            .finish()
    }
}

#[async_trait]
impl crate::toolset::ToolCaller for McpConnection {
    fn server_name(&self) -> &str {
        &self.name
    }

    async fn call(&self, tool: &McpTool, args: &str) -> Result<ToolObservation, String> {
        call_tool(&self.peer, tool, args)
            .await
            .map_err(|error| error.to_string())
    }
}

pub type EnvResolver = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub fn default_resolver() -> EnvResolver {
    std::sync::Arc::new(meerai_core::expand::env_lookup)
}

pub async fn connect(server: &McpServerConfig) -> McpResult<McpConnection> {
    connect_with(server, default_resolver(), None).await
}

pub async fn connect_with(
    server: &McpServerConfig,
    resolver: EnvResolver,
    base_dir: Option<&std::path::Path>,
) -> McpResult<McpConnection> {
    let mut stderr = StderrTail::default();
    let service = match &server.transport {
        McpTransport::Stdio { .. } => {
            let plan = spawn_plan(server, &resolver, base_dir)?;
            let (child, tail) = spawn_command(&plan)?;
            stderr = tail;
            ().serve_with_lifecycle(child, lifecycle_mode())
                .await
                .map_err(|error| {
                    McpError::Transport(format!(
                        "connect {} over stdio: {error}{}",
                        server.name,
                        stderr.suffix()
                    ))
                })?
        }
        McpTransport::Http { url, headers } => {
            let transport = http_transport(url, headers, &resolver)?;
            ().serve_with_lifecycle(transport, lifecycle_mode())
                .await
                .map_err(|error| {
                    McpError::Transport(format!("connect {} over http: {error}", server.name))
                })?
        }
        McpTransport::Unsupported { kind } => {
            return Err(McpError::Transport(format!(
                "server '{}' uses unsupported transport '{kind}'",
                server.name
            )));
        }
    };

    let peer = service.peer().clone();
    let (tools, warnings) = list_tools(server, &peer).await?;
    Ok(McpConnection {
        name: server.name.clone(),
        peer,
        service,
        tools,
        warnings,
        stderr,
    })
}

fn lifecycle_mode() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<std::path::PathBuf>,
}

pub fn spawn_plan(
    server: &McpServerConfig,
    resolver: &EnvResolver,
    base_dir: Option<&std::path::Path>,
) -> McpResult<SpawnPlan> {
    let McpTransport::Stdio {
        command,
        args,
        env,
        cwd,
    } = &server.transport
    else {
        return Err(McpError::Config(format!(
            "server '{}' is not a stdio server",
            server.name
        )));
    };
    let lookup = |name: &str| resolver(name);
    let required = |label: &str, value: &str| -> McpResult<String> {
        let expanded = meerai_core::expand::expand_env_reference_trimmed(value, &lookup);
        expanded.ok_or_else(|| {
            McpError::Config(format!(
                "server '{}' {label} is empty after environment expansion",
                server.name
            ))
        })
    };

    let program = required("command", command)?;
    let mut resolved_args = Vec::with_capacity(args.len());
    for arg in args {
        resolved_args.push(meerai_core::expand::expand_env_reference(arg, &lookup));
    }
    let mut resolved_env = BTreeMap::new();
    for (key, value) in env {
        let name = required("environment name", key)?;
        resolved_env.insert(
            name,
            meerai_core::expand::expand_env_reference(value, &lookup),
        );
    }
    let cwd = match cwd {
        Some(raw) => {
            let expanded = required("cwd", raw)?;
            Some(resolve_cwd(&expanded, base_dir))
        }
        None => base_dir.map(std::path::Path::to_path_buf),
    };

    Ok(SpawnPlan {
        program,
        args: resolved_args,
        env: resolved_env,
        cwd,
    })
}

fn resolve_cwd(cwd: &str, base_dir: Option<&std::path::Path>) -> std::path::PathBuf {
    let path = std::path::Path::new(cwd);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match base_dir {
        Some(base) => base.join(path),
        None => path.to_path_buf(),
    }
}

fn spawn_command(plan: &SpawnPlan) -> McpResult<(TokioChildProcess, StderrTail)> {
    let mut command = Command::new(&plan.program);
    command.args(&plan.args);
    for name in meerai_core::redact::sensitive_env_names(&[]) {
        command.env_remove(name);
    }
    command.envs(&plan.env);
    if let Some(cwd) = &plan.cwd {
        command.current_dir(cwd);
    }
    command.kill_on_drop(true);
    let (child, stderr) = TokioChildProcess::builder(command)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| McpError::Transport(format!("spawn '{}': {error}", plan.program)))?;
    let tail = stderr.map(StderrTail::spawn).unwrap_or_default();
    Ok((child, tail))
}

fn http_transport(
    url: &str,
    headers: &BTreeMap<String, String>,
    resolver: &EnvResolver,
) -> McpResult<HttpTransport> {
    let lookup = |name: &str| resolver(name);
    let expanded_url = meerai_core::expand::expand_env_reference_trimmed(url, &lookup)
        .ok_or_else(|| McpError::Config("mcp url is empty after expansion".to_string()))?;
    let mut config = StreamableHttpClientTransportConfig::with_uri(expanded_url);
    if !headers.is_empty() {
        let mut map = std::collections::HashMap::new();
        for (name, value) in headers {
            let expanded = meerai_core::expand::expand_env_reference(value, &lookup);
            let key = http::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                McpError::Config(format!("invalid header name '{name}': {error}"))
            })?;
            let value = http::HeaderValue::from_str(&expanded).map_err(|error| {
                McpError::Config(format!("invalid header value for '{name}': {error}"))
            })?;
            map.insert(key, value);
        }
        config = config.custom_headers(map);
    }
    Ok(StreamableHttpClientTransport::from_config(config))
}

async fn list_tools(
    server: &McpServerConfig,
    peer: &Peer<RoleClient>,
) -> McpResult<(Vec<McpTool>, Vec<String>)> {
    let mut warnings = Vec::new();
    let listed: Vec<Tool> = peer.list_all_tools().await.map_err(|error| {
        McpError::Protocol(format!("list tools for '{}': {error}", server.name))
    })?;

    let mut tools = Vec::with_capacity(listed.len());
    for tool in listed {
        let name = tool.name.to_string();
        if let Some(filter) = &server.tools
            && !filter.allows(&name)
        {
            continue;
        }
        let read_only = tool
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint)
            .unwrap_or(false);
        tools.push(McpTool {
            server: server.name.clone(),
            description: screen_description(tool.description.as_deref().unwrap_or_default()),
            name,
            schema: serde_json::Value::Object((*tool.input_schema).clone()),
            read_only,
        });
    }

    if server.read_only && tools.iter().any(|tool| !tool.read_only) {
        warnings.push(format!(
            "server '{}' is marked read_only but declares tools without readOnlyHint; those tools stay unavailable in plan mode",
            server.name
        ));
    }

    tools.sort_by(|left, right| left.name.cmp(&right.name));
    Ok((tools, warnings))
}

pub async fn call_tool(
    peer: &Peer<RoleClient>,
    tool: &McpTool,
    args: &str,
) -> McpResult<ToolObservation> {
    let arguments: JsonObject = if args.trim().is_empty() {
        JsonObject::new()
    } else {
        serde_json::from_str(args)
            .map_err(|error| McpError::Protocol(format!("invalid tool arguments: {error}")))?
    };
    let params = CallToolRequestParams::new(tool.name.clone()).with_arguments(arguments);
    let result: CallToolResult = peer
        .call_tool(params)
        .await
        .map_err(|error| McpError::Protocol(format!("call {}: {error}", tool.runtime_name())))?;
    Ok(render_result(result))
}

pub fn render_result(result: CallToolResult) -> ToolObservation {
    let mut text = String::new();
    for block in &result.content {
        match block {
            ContentBlock::Text(content) => push_line(&mut text, &content.text),
            ContentBlock::Image(image) => {
                push_line(&mut text, &format!("[image: {}]", image.mime_type));
            }
            ContentBlock::Resource(resource) => {
                let uri = match &resource.resource {
                    rmcp::model::ResourceContents::TextResourceContents { uri, .. }
                    | rmcp::model::ResourceContents::BlobResourceContents { uri, .. } => {
                        uri.as_str()
                    }
                    _ => "unknown",
                };
                push_line(&mut text, &format!("[resource: {uri}]"));
            }
            ContentBlock::ResourceLink(link) => {
                push_line(&mut text, &format!("[resource link: {}]", link.uri));
            }
            _ => push_line(&mut text, "[unsupported content block]"),
        }
    }

    if text.is_empty()
        && let Some(structured) = &result.structured_content
    {
        text = structured.to_string();
    }

    if text.chars().count() > MAX_INLINE_RESULT_CHARS {
        let capped: String = text.chars().take(MAX_INLINE_RESULT_CHARS).collect();
        text = format!("{capped}\n... [mcp result truncated]");
    }

    if result.is_error == Some(true) {
        return ToolObservation::error(text);
    }
    ToolObservation::success(text)
}

fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}
