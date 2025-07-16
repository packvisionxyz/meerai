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
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ActionSelector {
    Any,
    Action(PermissionAction),
}

impl ActionSelector {
    pub fn matches(&self, action: PermissionAction) -> bool {
        match self {
            Self::Any => true,
            Self::Action(selected) => *selected == action,
        }
    }
}

impl TryFrom<String> for ActionSelector {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value == "*" {
            return Ok(Self::Any);
        }
        PermissionAction::parse(&value)
            .map(Self::Action)
            .ok_or_else(|| format!("unknown permission action: {value}"))
    }
}

impl From<ActionSelector> for String {
    fn from(value: ActionSelector) -> Self {
        match value {
            ActionSelector::Any => "*".to_string(),
            ActionSelector::Action(action) => action.as_str().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub action: ActionSelector,
    pub resource: String,
    pub effect: Effect,
}
