use std::collections::BTreeMap;

use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthKindDto {
    ApiKey,
    OAuth,
    Either,
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatGptLoginStateDto {
    Started,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderItemDto {
    pub id: String,
    pub display_name: String,
    pub adapter: String,
    pub auth_kind: AuthKindDto,
    pub configured: bool,
    pub status_badge: String,
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub is_custom: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelItemDto {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub supports_reasoning: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub is_custom: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    pub providers: Vec<ProviderItemDto>,
    pub models: Vec<ModelItemDto>,
    #[serde(default)]
    pub recent_models: BTreeMap<String, Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetApiKeyRequest {
    pub api_key: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderBaseUrlRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptLoginStartResponse {
    pub attempt_id: String,
    pub provider_id: String,
    pub url: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptLoginPasteRequest {
    pub input: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetModelDto {
    pub provider: String,
    pub model: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadResponse {
    pub host_info: HostInfoDto,
    #[serde(default)]
    pub warnings: Vec<String>,
}
