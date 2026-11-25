use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDto {
    pub id: String,
    pub project_root: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    pub pinned: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDto {
    pub task_type: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDto {
    pub id: String,
    pub source_kind: String,
    pub source_uri: String,
    pub pinned_rev: String,
    pub enabled: bool,
    pub trusted: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionInfoDto {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_invocable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub token_estimate: u64,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub supporting_files: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowedDefinitionDto {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub path: String,
    pub kept_source: String,
    pub kept_path: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionsDiagnosticsDto {
    #[serde(default)]
    pub agents: Vec<DefinitionInfoDto>,
    #[serde(default)]
    pub skills: Vec<DefinitionInfoDto>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub shadows: Vec<ShadowedDefinitionDto>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoriesResponse {
    pub memories: Vec<MemoryDto>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutesResponse {
    pub routes: Vec<RouteDto>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PacksResponse {
    pub packs: Vec<PackDto>,
}
