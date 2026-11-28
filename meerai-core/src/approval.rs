use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAction {
    Read,
    Edit,
    Shell,
    Subagent,
    #[serde(rename = "webfetch")]
    WebFetch,
    ExternalDirectory,
    Mcp,
}

impl PermissionAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Shell => "shell",
            Self::Subagent => "subagent",
            Self::WebFetch => "webfetch",
            Self::ExternalDirectory => "external_directory",
            Self::Mcp => "mcp",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "shell" => Some(Self::Shell),
            "subagent" => Some(Self::Subagent),
            "webfetch" => Some(Self::WebFetch),
            "external_directory" => Some(Self::ExternalDirectory),
            "mcp" => Some(Self::Mcp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolPermissionSpec {
    pub action: PermissionAction,
    pub resource_arg: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalPreview {
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    Turn,
    Session,
}
