use std::fmt::Debug;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::async_trait;
use crate::approval::ToolPermissionSpec;

#[derive(Clone, Debug)]
pub struct CommandOutput {
    pub output: String,
}

impl std::fmt::Display for CommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.output)
    }
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("executor error: {0:#}")]
    ExecutorError(#[from] anyhow::Error),

    #[error("command failed with NonZeroExit: {0}")]
    NonZeroExit(CommandOutput),
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid function name: {0:#}")]
    InvalidFunctionName(String),

    #[error("arguments for tool failed to parse: {0:#}")]
    WrongArguments(#[from] serde_json::Error),

    #[error("tool execution failed: {0:#}")]
    ExecutionFailed(#[from] CommandError),

    #[error(transparent)]
    Unknown(#[from] anyhow::Error),
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum ToolOutput {
    Text(String),

    Fail(String),

    Stop(String),
}

impl std::fmt::Display for ToolOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolOutput::Text(text) => write!(f, "Success: {}", text),
            ToolOutput::Fail(text) => write!(f, "Failure: {}", text),
            ToolOutput::Stop(text) => write!(f, "Stop: {}", text),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Read,
    Write,
    Execute,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Success,
    Error,
    Refused,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ToolObservation {
    pub status: ObservationStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub hint: Option<String>,
}

pub const SANDBOX_DENIED_HINT: &str =
    "the OS sandbox denied write access; a sandbox escape can rerun this command";

impl ToolObservation {
    pub fn sandbox_denied(&self) -> bool {
        self.hint.as_deref() == Some(SANDBOX_DENIED_HINT)
    }

    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            status: ObservationStatus::Success,
            exit_code: None,
            stdout: stdout.into(),
            stderr: String::new(),
            hint: None,
        }
    }

    pub fn error(stderr: impl Into<String>) -> Self {
        Self {
            status: ObservationStatus::Error,
            exit_code: None,
            stdout: String::new(),
            stderr: stderr.into(),
            hint: None,
        }
    }

    pub fn refused(reason: impl Into<String>) -> Self {
        Self {
            status: ObservationStatus::Refused,
            exit_code: None,
            stdout: String::new(),
            stderr: reason.into(),
            hint: None,
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        match self.status {
            ObservationStatus::Success => out.push_str(&self.stdout),
            ObservationStatus::Stopped => {
                out.push_str("Stop: ");
                out.push_str(&self.stdout);
            }
            ObservationStatus::Error => {
                out.push_str("Failure: ");
                out.push_str(&self.stderr);
            }
            ObservationStatus::Refused => {
                out.push_str("Refused: ");
                out.push_str(&self.stderr);
            }
        }
        if let Some(code) = self.exit_code
            && code != 0
        {
            out.push_str(&format!(" (exit code {code})"));
        }
        if let Some(hint) = &self.hint {
            out.push_str("\nHint: ");
            out.push_str(hint);
        }
        out
    }
}

impl From<ToolOutput> for ToolObservation {
    fn from(output: ToolOutput) -> Self {
        match output {
            ToolOutput::Text(text) => ToolObservation::success(text),
            ToolOutput::Fail(text) => ToolObservation::error(text),
            ToolOutput::Stop(reason) => Self {
                status: ObservationStatus::Stopped,
                exit_code: None,
                stdout: reason,
                stderr: String::new(),
                hint: None,
            },
        }
    }
}

impl From<ToolObservation> for ToolOutput {
    fn from(observation: ToolObservation) -> Self {
        match observation.status {
            ObservationStatus::Success => ToolOutput::Text(observation.stdout),
            ObservationStatus::Stopped => ToolOutput::Stop(observation.stdout),
            ObservationStatus::Error => ToolOutput::Fail(observation.render()),
            ObservationStatus::Refused => ToolOutput::Fail(observation.render()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub kind: FileChangeKind,
    pub unified_diff: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolRun {
    pub output: ToolOutput,
    pub observation: ToolObservation,
    pub file_changes: Vec<FileDiff>,
}

impl ToolRun {
    pub fn from_output(output: ToolOutput) -> Self {
        Self {
            observation: ToolObservation::from(output.clone()),
            output,
            file_changes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolDefinition {
    pub r#type: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn to_genai(&self) -> genai::chat::Tool {
        genai::chat::Tool::new(self.name.clone())
            .with_description(self.description.clone())
            .with_schema(self.parameters.clone())
    }
}

impl From<&ToolDefinition> for genai::chat::Tool {
    fn from(def: &ToolDefinition) -> Self {
        def.to_genai()
    }
}

#[async_trait]
pub trait Toolset: Send + Sync {
    fn name(&self) -> String;

    fn definition(&self) -> Vec<ToolDefinition>;

    fn contain(&self, fn_name: &str) -> bool;

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        let _ = fn_name;
        EffectClass::Read
    }

    fn effect_of_call(&self, fn_name: &str, _args: &str) -> EffectClass {
        self.effect_of(fn_name)
    }

    fn permission_spec(&self, _fn_name: &str) -> Option<ToolPermissionSpec> {
        None
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        true
    }

    fn grant_path_root(&self, _root: &std::path::Path) {}

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError>;

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        let output = self.invoke(fn_name, args).await?;
        Ok(ToolRun::from_output(output))
    }
}

impl Debug for dyn Toolset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Toolset {}", self.name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

impl From<&genai::chat::ToolCall> for ToolCall {
    fn from(call: &genai::chat::ToolCall) -> Self {
        Self {
            id: call.call_id.clone(),
            name: call.fn_name.clone(),
            args: call.fn_arguments.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool() {
        struct MyToolset;

        #[async_trait]
        impl Toolset for MyToolset {
            fn name(&self) -> String {
                "my_tool".to_string()
            }

            fn definition(&self) -> Vec<ToolDefinition> {
                vec![ToolDefinition {
                    r#type: "function".to_string(),
                    name: self.name(),
                    description: "My tool".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "arg1": {
                                "type": "string",
                                "description": "Argument 1"
                            }
                        }
                    }),
                }]
            }

            fn contain(&self, fn_name: &str) -> bool {
                self.name() == fn_name
            }

            async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput::Text("Hello world".to_string()))
            }
        }

        let tool = MyToolset;
        assert_eq!(tool.name(), "my_tool");
        assert!(tool.contain("my_tool"));
        assert_eq!(tool.definition().len(), 1);
    }
}
