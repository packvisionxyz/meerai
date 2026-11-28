use std::path::{Component, Path, PathBuf};

use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolOutput, ToolPermissionSpec,
    async_trait,
};
use serde_json::Value;

use meerai_session::{QuestionAnswer, QuestionOption, QuestionPrompt};

pub const WORKSPACE_CD_TOOL: &str = "workspace-cd";

const SWITCH_ROOT: &str = "Switch to that directory";
const OPEN_NEW: &str = "Open a new session there";
const CARRIED: &str = "carried";
const OPENED: &str = "opened";
const CANCEL: &str = "Keep the current directory";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceAction {
    Carry,
    OpenNew,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCd {
    pub path: PathBuf,
    pub action: WorkspaceAction,
}

impl WorkspaceCd {
    pub fn encode(&self) -> String {
        let action = match self.action {
            WorkspaceAction::Carry => CARRIED,
            WorkspaceAction::OpenNew => OPENED,
        };
        serde_json::json!({ "action": action, "path": self.path.display().to_string() }).to_string()
    }
}

pub fn parse_workspace_cd(args: &str) -> Result<WorkspaceCd, String> {
    let value: Value =
        serde_json::from_str(args).map_err(|error| format!("invalid JSON: {error}"))?;
    let raw = value
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "workspace-cd needs a path".to_string())?;
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => Path::new(&home).join(rest),
            None => return Err("cannot expand ~ without HOME".to_string()),
        },
        None if raw == "~" => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home),
            None => return Err("cannot expand ~ without HOME".to_string()),
        },
        None => PathBuf::from(raw),
    };
    if !expanded.is_absolute() {
        return Err(format!("path must be absolute: {}", expanded.display()));
    }
    let mut normalized = PathBuf::new();
    for component in expanded.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    let action = match value
        .get("mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    {
        None => {
            return Ok(WorkspaceCd {
                path: normalized,
                action: WorkspaceAction::Carry,
            });
        }
        Some("carry") => WorkspaceAction::Carry,
        Some("new") => WorkspaceAction::OpenNew,
        Some(other) => {
            return Err(format!("mode must be 'carry' or 'new', not '{other}'"));
        }
    };
    Ok(WorkspaceCd {
        path: normalized,
        action,
    })
}

pub fn decision_question(path: &Path) -> Vec<QuestionPrompt> {
    vec![QuestionPrompt {
        header: "Directory".to_string(),
        question: format!(
            "Switch the workspace to {}? The current session cannot move while a turn runs.",
            path.display()
        ),
        options: vec![
            QuestionOption {
                label: SWITCH_ROOT.to_string(),
                description: Some(
                    "After this turn ends the session reopens on the new directory.".to_string(),
                ),
            },
            QuestionOption {
                label: OPEN_NEW.to_string(),
                description: Some(
                    "This session keeps its current directory; a new session opens there."
                        .to_string(),
                ),
            },
        ],
        multiple: false,
    }]
}

pub fn action_from_answer(answer: &QuestionAnswer) -> Option<WorkspaceAction> {
    let selected = answer.selected.first()?;
    match selected.as_str() {
        SWITCH_ROOT => Some(WorkspaceAction::Carry),
        OPEN_NEW => Some(WorkspaceAction::OpenNew),
        _ => None,
    }
}

pub struct WorkspaceToolset;

#[async_trait]
impl meerai_core::Toolset for WorkspaceToolset {
    fn name(&self) -> String {
        "workspace".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: WORKSPACE_CD_TOOL.to_string(),
            description: "Change the workspace directory. An absolute path inside the current \
                          workspace switches silently. A path outside it asks the user whether to \
                          carry this session there or open a new session. A carry takes effect \
                          after the current turn ends."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute directory path, or a ~-prefixed path"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["carry", "new"],
                        "description": "carry reopens this session there after the turn; new opens another session"
                    }
                },
                "required": ["path"]
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == WORKSPACE_CD_TOOL
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::External
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        (fn_name == WORKSPACE_CD_TOOL).then_some(ToolPermissionSpec {
            action: PermissionAction::Read,
            resource_arg: Some("path"),
        })
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Fail(
            "workspace-cd is handled by the agent harness and needs an interactive client"
                .to_string(),
        ))
    }
}

pub fn format_decision(action: WorkspaceAction, path: &Path) -> String {
    match action {
        WorkspaceAction::Carry => serde_json::json!({
            "action": CARRIED,
            "path": path.display().to_string(),
            "effective": "after this turn ends",
            "cancel": CANCEL,
        })
        .to_string(),
        WorkspaceAction::OpenNew => serde_json::json!({
            "action": OPENED,
            "path": path.display().to_string(),
            "effective": "a new session opens there",
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_path_and_defaults_to_carry() {
        let parsed = parse_workspace_cd(r#"{"path":"/tmp/elsewhere"}"#).unwrap();
        assert_eq!(parsed.path, PathBuf::from("/tmp/elsewhere"));
        assert_eq!(parsed.action, WorkspaceAction::Carry);
    }

    #[test]
    fn normalizes_dots_and_rejects_relative_paths() {
        let parsed = parse_workspace_cd(r#"{"path":"/tmp/a/./b/../c"}"#).unwrap();
        assert_eq!(parsed.path, PathBuf::from("/tmp/a/c"));
        assert!(parse_workspace_cd(r#"{"path":"relative/dir"}"#).is_err());
        assert!(parse_workspace_cd(r#"{"path":""}"#).is_err());
    }

    #[test]
    fn rejects_unknown_mode() {
        assert!(parse_workspace_cd(r#"{"path":"/tmp/x","mode":"teleport"}"#).is_err());
        let parsed = parse_workspace_cd(r#"{"path":"/tmp/x","mode":"new"}"#).unwrap();
        assert_eq!(parsed.action, WorkspaceAction::OpenNew);
    }

    #[test]
    fn maps_answer_labels_to_actions() {
        let carry = QuestionAnswer {
            header: "Directory".to_string(),
            selected: vec![SWITCH_ROOT.to_string()],
        };
        let open = QuestionAnswer {
            header: "Directory".to_string(),
            selected: vec![OPEN_NEW.to_string()],
        };
        let cancel = QuestionAnswer {
            header: "Directory".to_string(),
            selected: vec![CANCEL.to_string()],
        };
        assert_eq!(action_from_answer(&carry), Some(WorkspaceAction::Carry));
        assert_eq!(action_from_answer(&open), Some(WorkspaceAction::OpenNew));
        assert_eq!(action_from_answer(&cancel), None);
    }

    #[test]
    fn encodes_a_host_readable_decision() {
        let decision = WorkspaceCd {
            path: PathBuf::from("/tmp/x"),
            action: WorkspaceAction::OpenNew,
        };
        let encoded = decision.encode();
        assert!(encoded.contains("\"action\":\"opened\""));
        assert!(encoded.contains("/tmp/x"));
    }
}
