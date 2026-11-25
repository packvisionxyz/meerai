use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub items: Vec<LoopItemDto>,
    #[serde(default)]
    pub phase: LoopPhaseDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<u32>,
    #[serde(default)]
    pub rounds: usize,
    #[serde(default)]
    pub turns: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopItemDto {
    pub id: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
    pub status: LoopItemStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhaseDto {
    #[default]
    Plan,
    Act,
    Observe,
    Adjust,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopItemStatusDto {
    #[default]
    Open,
    Done,
    Failed,
}
