pub mod budget;
pub mod client;
pub mod config;
pub mod error;
pub mod merge;
pub mod registry;
pub mod setup;
pub mod stderr;
pub mod tool;
pub mod toolset;

pub use budget::{
    AdmissionMode, DEFAULT_FULL_BUDGET_TOKENS, DEFAULT_MAX_LOADED_TOKENS, DEFAULT_MAX_LOADED_TOOLS,
    index_entry, select_mode, total_tokens,
};
pub use client::{
    EnvResolver, McpConnection, SpawnPlan, call_tool, connect, connect_with, default_resolver,
    render_result, spawn_plan,
};
pub use config::{
    MCP_CONFIG_FILE, McpConfig, McpServerConfig, McpTransport, ToolFilter, content_hash,
    parse_config, parse_tool_name, read_config_file, tool_name, validate_server_name,
};
pub use error::{McpError, McpResult};
pub use merge::{MergeOutcome, merge_configs};
pub use registry::McpRegistry;
pub use setup::{
    McpSetupReport, ServerReport, ServerStatus, apply_connection, build_setup, load_project_config,
    load_user_config, mark_failed, read_only_servers, ready_servers,
};
pub use tool::{McpTool, WITHHELD, screen_description};
pub use toolset::{AdmissionPolicy, LOAD_TOOL_SCHEMA, McpToolset, ToolCaller};
