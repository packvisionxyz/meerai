use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("mcp configuration error: {0}")]
    Config(String),

    #[error("mcp transport error: {0}")]
    Transport(String),

    #[error("mcp protocol error: {0}")]
    Protocol(String),

    #[error("mcp server '{server}' has no tool '{tool}'")]
    UnknownTool { server: String, tool: String },
}

pub type McpResult<T> = Result<T, McpError>;
