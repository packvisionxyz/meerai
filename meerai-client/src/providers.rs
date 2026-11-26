use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use meerai_common::settings::{
    AppSettings, EffortLevel, ProviderOptions, resolve_config_api_key, user_providers_path,
};
use serde::Deserialize;

use crate::secrets::{Credential, SecretStore};

const BUNDLED_PROVIDERS_YAML: &str = include_str!("../providers.yml");
const BUNDLED_MODELS_YAML: &str = include_str!("../models.yml");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
    ApiKey,
    OAuth,
    Either,
    None,
}

impl ProviderAuthKind {
    pub fn supports_api_key(self) -> bool {
        matches!(self, Self::ApiKey | Self::Either)
    }

    pub fn supports_oauth(self) -> bool {
        matches!(self, Self::OAuth | Self::Either)
    }

    pub fn requires_credential(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub display_name: String,
    pub adapter: String,
    pub default_base_url: Option<String>,
    pub env_vars: Vec<String>,
    pub auth: ProviderAuthKind,
    pub default_model: Option<String>,
    pub config_api_key: Option<String>,
}

impl ProviderInfo {
    pub fn genai_model_spec(&self, model_id: &str) -> String {
        let clean = model_id
            .strip_prefix(&format!("{}/", self.id))
            .unwrap_or(model_id);
        format!("{}::{}", self.adapter, clean)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    Stored,
    OAuth {
        expires_at_ms: u64,
        needs_relogin: bool,
    },
    Config,
    Env(String),
    NotRequired,
    Missing,
}

impl KeyStatus {
    pub fn is_configured(&self) -> bool {
        !matches!(self, Self::Missing)
            && !matches!(
                self,
                Self::OAuth {
                    needs_relogin: true,
                    ..
                }
            )
    }

    pub fn badge(&self) -> String {
        match self {
            Self::Stored => "saved".to_string(),
            Self::OAuth { .. } => "plan".to_string(),
            Self::Config => "config".to_string(),
            Self::Env(var) => format!("env:{var}"),
            Self::NotRequired => "local".to_string(),
            Self::Missing => "not set".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub input_cost_per_mtok: Option<f64>,
    #[serde(default)]
    pub output_cost_per_mtok: Option<f64>,
    #[serde(default)]
    pub effort_levels: Option<Vec<EffortLevel>>,
}

impl ModelInfo {
    pub fn supports_reasoning(&self) -> bool {
        !matches!(self.effort_levels.as_deref(), Some([]))
    }
}

#[derive(Debug, Deserialize)]
struct ProvidersFile {
    #[serde(default)]
    providers: Vec<ProviderConfigEntry>,
    #[serde(default)]
    models: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
struct ProviderConfigEntry {
    id: String,
    display_name: Option<String>,
    adapter: Option<String>,
    base_url: Option<String>,
    #[serde(default)]
    env_vars: Vec<String>,
    requires_api_key: Option<bool>,
    auth: Option<String>,
    default_model: Option<String>,
    #[serde(default, alias = "apiKey")]
    api_key: Option<String>,
    #[serde(default)]
    options: Option<ProviderOptions>,
}

pub fn canonical_provider_id(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "google" | "gemini" => "gemini".to_string(),
        "open_router" | "openrouter" => "openrouter".to_string(),
        "opencode_go" | "opencode-go" => "opencode-go".to_string(),
        "opencode" => "opencode".to_string(),
        "openai" => "openai".to_string(),
        "anthropic" => "anthropic".to_string(),
        "deepseek" => "deepseek".to_string(),
        "xai" => "xai".to_string(),
        "mistral" | "mistralai" | "mistral-ai" => "mistral".to_string(),
        "moonshot" | "moonshotai" | "moonshot-ai" | "kimi" => "moonshot".to_string(),
        "zai" | "z-ai" | "zai_coding" | "zai-coding" | "glm" => "zai-coding".to_string(),
        "aliyun" | "alibaba" | "alibaba-token-plan" | "dashscope" | "qwen" | "qwen-cloud"
        | "qwen_cloud" => "qwen-cloud".to_string(),
        "ollama" => "ollama".to_string(),
        other => other.to_string(),
    }
}

fn parse_adapter(adapter: Option<&str>) -> String {
    match adapter
        .unwrap_or("openai")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "opencode_go" | "opencode-go" => "opencode_go",
        "open_router" | "openrouter" => "open_router",
        "gemini" | "google" => "gemini",
        "anthropic" => "anthropic",
        "deepseek" => "deepseek",
        "xai" => "xai",
        "zai" | "z-ai" => "zai",
        "moonshot" | "kimi" => "moonshot",
        "qwen_cloud" | "qwen-cloud" | "qwen" => "qwen_cloud",
        "aliyun" => "aliyun",
        "ollama" => "ollama",
        "mistral" => "mistral",
        "cohere" => "cohere",
        "openai_resp" | "openai-responses" | "openai_responses" => "openai_resp",
        _ => "openai",
    }
    .to_string()
}

fn parse_auth_kind(raw: Option<&str>, requires_api_key: Option<bool>) -> ProviderAuthKind {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("api_key" | "api-key" | "apikey") => ProviderAuthKind::ApiKey,
        Some("oauth") => ProviderAuthKind::OAuth,
        Some("either" | "both") => ProviderAuthKind::Either,
        Some("none" | "local") => ProviderAuthKind::None,
        _ if requires_api_key == Some(false) => ProviderAuthKind::None,
        _ => ProviderAuthKind::ApiKey,
    }
}

fn synthesize_provider(provider_id: &str) -> ProviderInfo {
    let upper = provider_id.to_ascii_uppercase().replace('-', "_");
    ProviderInfo {
        id: provider_id.to_string(),
        display_name: provider_id.to_string(),
        adapter: "openai".to_string(),
        default_base_url: None,
        env_vars: vec![format!("{upper}_API_KEY")],
        auth: ProviderAuthKind::ApiKey,
        default_model: None,
        config_api_key: None,
    }
}

#[derive(Clone, Debug)]
pub struct ProviderCatalog {
    providers: Vec<ProviderInfo>,
    models: Vec<ModelInfo>,
}

impl Default for ProviderCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderCatalog {
    // The bundled YAML is compiled in and covered by tests; a parse failure is a build bug.
    #[allow(clippy::expect_used)]
    pub fn new() -> Self {
        let mut catalog =
            Self::from_yaml(BUNDLED_PROVIDERS_YAML).expect("bundled providers.yml must parse");
        let models: ProvidersFile =
            serde_yaml::from_str(BUNDLED_MODELS_YAML).expect("bundled models.yml must parse");
        catalog.merge_file(models);
        catalog
    }

    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let parsed: ProvidersFile =
            serde_yaml::from_str(yaml).context("failed to parse providers.yml configuration")?;
        let mut catalog = Self {
            providers: Vec::new(),
            models: Vec::new(),
        };
        catalog.merge_file(parsed);
        Ok(catalog)
    }

    pub fn load_with_user_dir(config_dir: Option<&Path>) -> Self {
        Self::load_with_user_dir_checked(config_dir).0
    }

    pub fn load_with_user_dir_checked(config_dir: Option<&Path>) -> (Self, Vec<String>) {
        let mut catalog = Self::new();
        let mut warnings = Vec::new();
        if let Some(dir) = config_dir {
            let path = user_providers_path(dir);
            if path.is_file() {
                match fs::read_to_string(&path) {
                    Ok(raw) => match serde_yaml::from_str::<ProvidersFile>(&raw) {
                        Ok(parsed) => catalog.merge_file(parsed),
                        Err(err) => {
                            warnings.push(format!("Failed to parse {}: {err}", path.display()))
                        }
                    },
                    Err(err) => warnings.push(format!("Failed to read {}: {err}", path.display())),
                }
            }
        }
        (catalog, warnings)
    }

    fn merge_file(&mut self, file: ProvidersFile) {
        for entry in file.providers {
            let id = canonical_provider_id(&entry.id);
            let default_model = entry
                .default_model
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
            let config_api_key =
                resolve_config_api_key(entry.options.as_ref(), entry.api_key.as_deref());
            let auth = parse_auth_kind(entry.auth.as_deref(), entry.requires_api_key);

            if let Some(existing) = self.providers.iter_mut().find(|p| p.id == id) {
                if let Some(display_name) = entry.display_name {
                    existing.display_name = display_name;
                }
                if let Some(adapter) = entry.adapter.as_deref() {
                    existing.adapter = parse_adapter(Some(adapter));
                }
                if entry.base_url.is_some() {
                    existing.default_base_url = entry.base_url;
                }
                if !entry.env_vars.is_empty() {
                    existing.env_vars = entry.env_vars;
                }
                if entry.requires_api_key.is_some() || entry.auth.is_some() {
                    existing.auth = auth;
                }
                if default_model.is_some() {
                    existing.default_model = default_model;
                }
                if config_api_key.is_some() {
                    existing.config_api_key = config_api_key;
                }
            } else {
                self.providers.push(ProviderInfo {
                    id: id.clone(),
                    display_name: entry.display_name.unwrap_or_else(|| id.clone()),
                    adapter: parse_adapter(entry.adapter.as_deref()),
                    default_base_url: entry.base_url,
                    env_vars: entry.env_vars,
                    auth,
                    default_model,
                    config_api_key,
                });
            }
        }
        for entry in file.models {
            let provider = canonical_provider_id(&entry.provider);
            let id = entry.id.trim().to_string();
            if provider.is_empty() || id.is_empty() {
                continue;
            }
            let info = ModelInfo {
                provider: provider.clone(),
                id,
                context_window: entry.context_window,
                max_output_tokens: entry.max_output_tokens,
                input_cost_per_mtok: entry.input_cost_per_mtok,
                output_cost_per_mtok: entry.output_cost_per_mtok,
                effort_levels: entry.effort_levels,
            };
            if let Some(existing) = self
                .models
                .iter_mut()
                .find(|model| model.provider == provider && model.id == info.id)
            {
                if info.context_window.is_some() {
                    existing.context_window = info.context_window;
                }
                if info.max_output_tokens.is_some() {
                    existing.max_output_tokens = info.max_output_tokens;
                }
                if info.input_cost_per_mtok.is_some() {
                    existing.input_cost_per_mtok = info.input_cost_per_mtok;
                }
                if info.output_cost_per_mtok.is_some() {
                    existing.output_cost_per_mtok = info.output_cost_per_mtok;
                }
                if info.effort_levels.is_some() {
                    existing.effort_levels = info.effort_levels;
                }
            } else {
                self.models.push(info);
            }
        }
    }

    pub fn apply_settings(&mut self, settings: &AppSettings) {
        for (raw_id, saved) in &settings.providers {
            let id = canonical_provider_id(raw_id);
            let resolved_key = saved.resolved_api_key();
            if let Some(existing) = self.providers.iter_mut().find(|p| p.id == id) {
                if resolved_key.is_some() {
                    existing.config_api_key = resolved_key;
                }
            } else if resolved_key.is_some() {
                let mut synthesized = synthesize_provider(&id);
                synthesized.default_base_url = saved.base_url.clone();
                synthesized.default_model = saved.model.clone();
                synthesized.config_api_key = resolved_key;
                self.providers.push(synthesized);
            }
        }
    }

    pub fn providers(&self) -> &[ProviderInfo] {
        &self.providers
    }

    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    pub fn models_for_provider(&self, provider: &str) -> Vec<&ModelInfo> {
        let provider = canonical_provider_id(provider);
        self.models
            .iter()
            .filter(|info| info.provider == provider)
            .collect()
    }

    pub fn effort_levels_for(&self, provider: &str, model: &str) -> Option<Vec<EffortLevel>> {
        self.model_info(provider, model)
            .and_then(|info| info.effort_levels.clone())
    }

    pub fn model_info(&self, provider: &str, model: &str) -> Option<&ModelInfo> {
        let provider = canonical_provider_id(provider);
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let prefix = format!("{provider}/");
        let stripped = model.strip_prefix(prefix.as_str()).unwrap_or(model);
        let bare = stripped.rsplit("::").next().unwrap_or(stripped);
        self.models.iter().find(|info| {
            info.provider == provider
                && (info.id == model || info.id == stripped || info.id == bare)
        })
    }

    pub fn sensitive_env_vars(&self) -> Vec<String> {
        let mut vars: BTreeSet<String> = BTreeSet::new();
        for provider in &self.providers {
            for var in &provider.env_vars {
                let clean = var.trim();
                if !clean.is_empty() {
                    vars.insert(clean.to_string());
                }
            }
        }
        vars.into_iter().collect()
    }

    pub fn provider(&self, provider_id: &str) -> Option<&ProviderInfo> {
        let canonical = canonical_provider_id(provider_id);
        self.providers.iter().find(|p| p.id == canonical)
    }

    pub fn default_model_for_provider(&self, provider_id: &str) -> Option<String> {
        self.provider(provider_id)
            .and_then(|p| p.default_model.clone())
    }

    pub fn format_genai_spec(&self, provider_id: &str, model_id: &str) -> String {
        if model_id.contains("::") {
            return model_id.to_string();
        }
        if let Some(info) = self.provider(provider_id) {
            info.genai_model_spec(model_id)
        } else {
            let clean = model_id
                .strip_prefix(&format!("{provider_id}/"))
                .unwrap_or(model_id);
            format!("openai::{clean}")
        }
    }
}

pub fn check_key_status(provider: &ProviderInfo, secrets: &dyn SecretStore) -> KeyStatus {
    match secrets.read_credential(&provider.id) {
        Ok(Some(Credential::ApiKey(_))) => return KeyStatus::Stored,
        Ok(Some(Credential::OAuth(oauth))) => {
            return KeyStatus::OAuth {
                expires_at_ms: oauth.expires,
                needs_relogin: oauth.needs_relogin,
            };
        }
        _ => {}
    }
    if provider
        .config_api_key
        .as_deref()
        .is_some_and(|k| !k.trim().is_empty())
    {
        return KeyStatus::Config;
    }
    for var in &provider.env_vars {
        if let Ok(val) = std::env::var(var)
            && !val.trim().is_empty()
        {
            return KeyStatus::Env(var.clone());
        }
    }
    if !provider.auth.requires_credential() {
        return KeyStatus::NotRequired;
    }
    KeyStatus::Missing
}

pub fn resolve_credential(
    provider: &ProviderInfo,
    secrets: &dyn SecretStore,
) -> Result<Option<Credential>> {
    if let Some(credential) = secrets.read_credential(&provider.id)? {
        return Ok(Some(credential));
    }
    if let Some(key) = provider
        .config_api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        return Ok(Some(Credential::ApiKey(key.to_string())));
    }
    for var in &provider.env_vars {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(Some(Credential::ApiKey(trimmed)));
            }
        }
    }
    Ok(None)
}

pub fn resolve_api_key(
    provider: &ProviderInfo,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    Ok(
        resolve_credential(provider, secrets)?.map(|credential| match credential {
            Credential::ApiKey(key) => key,
            Credential::OAuth(oauth) => oauth.access,
        }),
    )
}

pub fn first_configured_provider(
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
) -> Option<String> {
    let mut seen = HashSet::new();
    for provider in catalog.providers() {
        if seen.insert(provider.id.as_str())
            && provider.auth.requires_credential()
            && check_key_status(provider, secrets).is_configured()
        {
            return Some(provider.id.clone());
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct RuntimeProviderState {
    pub active_provider: String,
    pub plan_usage: bool,
    pub base_urls: HashMap<String, String>,
    pub catalog: ProviderCatalog,
}

impl RuntimeProviderState {
    pub fn from_settings(settings: &AppSettings, mut catalog: ProviderCatalog) -> Self {
        catalog.apply_settings(settings);
        let active_provider = settings
            .active_provider
            .as_deref()
            .map(canonical_provider_id)
            .unwrap_or_else(|| "opencode-go".to_string());
        let base_urls = settings
            .providers
            .iter()
            .filter_map(|(k, v)| {
                v.base_url
                    .as_ref()
                    .map(|url| (canonical_provider_id(k), url.clone()))
            })
            .collect();
        Self {
            active_provider,
            plan_usage: false,
            base_urls,
            catalog,
        }
    }

    pub fn active_provider_info(&self) -> ProviderInfo {
        self.catalog
            .provider(&self.active_provider)
            .cloned()
            .unwrap_or_else(|| synthesize_provider(&self.active_provider))
    }

    pub fn effective_base_url(&self, provider: &ProviderInfo) -> Option<String> {
        if let Some(url) = self.base_urls.get(&provider.id) {
            return Some(url.clone());
        }
        if self.plan_usage && provider.id == "openai" {
            // genai adds `prompt_cache_options` for gpt-6 models whenever the endpoint string
            // matches `https://api.openai.com`; the plan route rejects that field. The explicit
            // default port reaches the same endpoint and bypasses that string check.
            return Some("https://api.openai.com:443/v1/".to_string());
        }
        provider.default_base_url.clone()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn plan_summary(oauth: &crate::secrets::OAuthCredential) -> String {
    let account = oauth.account.as_deref().unwrap_or("chatgpt");
    let remaining_ms = oauth.expires.saturating_sub(now_ms());
    let expiry = if oauth.needs_relogin {
        "sign in again".to_string()
    } else if remaining_ms == 0 {
        "expired".to_string()
    } else {
        format!("expires in {}m", remaining_ms / 60_000)
    };
    format!("{account}  {expiry}")
}

fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.trim().chars().collect();
    if chars.len() >= 10 {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{prefix}...{suffix}")
    } else if !chars.is_empty() {
        "****".to_string()
    } else {
        String::new()
    }
}

pub fn format_auth_list(
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
    auth_file: Option<&Path>,
    config_dir: Option<&Path>,
) -> String {
    format_auth_list_with_env(catalog, secrets, auth_file, config_dir, &|var| {
        std::env::var(var).ok()
    })
}

fn format_auth_list_with_env(
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
    auth_file: Option<&Path>,
    config_dir: Option<&Path>,
    env_lookup: &impl Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::new();
    let auth_label = auth_file
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "the encrypted vault".to_string());
    let _ = writeln!(out, "Credentials ({auth_label})");
    let mut stored_count = 0;
    let mut seen_stored = BTreeSet::new();
    for provider in catalog.providers() {
        let Ok(Some(credential)) = secrets.read_credential(&provider.id) else {
            continue;
        };
        seen_stored.insert(provider.id.clone());
        stored_count += 1;
        let line = match &credential {
            Credential::ApiKey(key) => format!(
                "  {} ({})  api  {}",
                provider.display_name,
                provider.id,
                mask_secret(key)
            ),
            Credential::OAuth(oauth) => format!(
                "  {} ({})  plan  {}",
                provider.display_name,
                provider.id,
                plan_summary(oauth)
            ),
        };
        let _ = writeln!(out, "{line}");
    }
    if let Ok(extra_ids) = secrets.list_providers() {
        for id in extra_ids {
            if seen_stored.contains(&id) {
                continue;
            }
            let Ok(Some(credential)) = secrets.read_credential(&id) else {
                continue;
            };
            stored_count += 1;
            let line = match &credential {
                Credential::ApiKey(key) => format!("  {}  api  {}", id, mask_secret(key)),
                Credential::OAuth(oauth) => format!("  {}  plan  {}", id, plan_summary(oauth)),
            };
            let _ = writeln!(out, "{line}");
        }
    }
    if stored_count == 0 {
        let _ = writeln!(out, "  (none)");
    }

    let config_label = config_dir
        .map(|d| format!("{}/settings.yml, providers.yml", d.display()))
        .unwrap_or_else(|| "settings.yml, providers.yml".to_string());
    let _ = writeln!(out, "\nConfig ({config_label})");
    let mut config_count = 0;
    for provider in catalog.providers() {
        if let Some(key) = provider
            .config_api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            config_count += 1;
            let _ = writeln!(
                out,
                "  {} ({})  config  {}",
                provider.display_name,
                provider.id,
                mask_secret(key)
            );
        }
    }
    if config_count == 0 {
        let _ = writeln!(out, "  (none)");
    }

    let _ = writeln!(out, "\nEnvironment");
    let mut env_count = 0;
    let mut seen_env = HashSet::new();
    for provider in catalog.providers() {
        for var in &provider.env_vars {
            if let Some(raw) = env_lookup(var) {
                let trimmed = raw.trim();
                if !trimmed.is_empty() && seen_env.insert((provider.id.as_str(), var.as_str())) {
                    env_count += 1;
                    let _ = writeln!(
                        out,
                        "  {} ({})  {}  {}",
                        provider.display_name,
                        provider.id,
                        var,
                        mask_secret(trimmed)
                    );
                }
            }
        }
    }
    if env_count == 0 {
        let _ = writeln!(out, "  (none)");
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{Credential, MemorySecretStore, OAuthCredential};

    #[test]
    fn bundled_providers_yml_loads_opencode_and_direct_providers() {
        let catalog = ProviderCatalog::new();

        let ids: Vec<&str> = catalog.providers().iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"opencode-go"));
        assert!(ids.contains(&"opencode"));
        assert!(ids.contains(&"openrouter"));
        assert!(ids.contains(&"qwen-cloud"));
        assert!(ids.contains(&"gemini"));
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"anthropic"));
        assert!(ids.contains(&"deepseek"));
        assert!(ids.contains(&"xai"));
        assert!(ids.contains(&"zai-coding"));
        assert!(ids.contains(&"moonshot"));
        assert!(ids.contains(&"mistral"));
        assert!(ids.contains(&"ollama"));
        assert!(!ids.contains(&"groq"));
        assert!(!ids.contains(&"alibaba-token-plan"));

        let ollama = catalog.provider("ollama").unwrap();
        assert_eq!(ollama.auth, ProviderAuthKind::None);

        let qwen = catalog.provider("qwen-cloud").unwrap();
        assert_eq!(qwen.adapter, "qwen_cloud");
        assert_eq!(
            qwen.default_base_url.as_deref(),
            Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/")
        );

        let zai = catalog.provider("zai-coding").unwrap();
        assert_eq!(zai.adapter, "zai");
        assert_eq!(
            zai.default_base_url.as_deref(),
            Some("https://api.z.ai/api/coding/paas/v4/")
        );

        assert_eq!(catalog.provider("moonshot").unwrap().adapter, "moonshot");
        assert_eq!(catalog.provider("mistral").unwrap().adapter, "mistral");

        let openai = catalog.provider("openai").unwrap();
        assert_eq!(openai.adapter, "openai_resp");
        assert_eq!(openai.auth, ProviderAuthKind::Either);
        assert_eq!(
            catalog.format_genai_spec("openai", "gpt-5"),
            "openai_resp::gpt-5"
        );

        assert_eq!(
            catalog.format_genai_spec("opencode-go", "deepseek-v4.1-flash"),
            "opencode_go::deepseek-v4.1-flash"
        );
        assert_eq!(
            catalog.format_genai_spec("openrouter", "anthropic/claude-sonnet-4.5"),
            "open_router::anthropic/claude-sonnet-4.5"
        );
        assert_eq!(
            catalog.format_genai_spec("zai-coding", "glm-5.3"),
            "zai::glm-5.3"
        );
        assert_eq!(
            catalog.format_genai_spec("qwen-cloud", "qwen3.8-max"),
            "qwen_cloud::qwen3.8-max"
        );
        assert_eq!(
            catalog.format_genai_spec("custom-proxy", "my-custom-model"),
            "openai::my-custom-model"
        );
    }

    #[test]
    fn bundled_default_models_exist_in_the_model_table() {
        let catalog = ProviderCatalog::new();
        for provider in catalog.providers() {
            let Some(default) = provider.default_model.as_deref() else {
                continue;
            };
            assert!(
                catalog.model_info(&provider.id, default).is_some(),
                "{} default model {} is not in models.yml",
                provider.id,
                default
            );
        }
    }

    #[test]
    fn provider_ids_canonicalize_aliases() {
        assert_eq!(canonical_provider_id("zai_coding"), "zai-coding");
        assert_eq!(canonical_provider_id("z-ai"), "zai-coding");
        assert_eq!(canonical_provider_id("moonshotai"), "moonshot");
        assert_eq!(canonical_provider_id("kimi"), "moonshot");
        assert_eq!(canonical_provider_id("mistralai"), "mistral");
        assert_eq!(canonical_provider_id("alibaba-token-plan"), "qwen-cloud");
        assert_eq!(canonical_provider_id("dashscope"), "qwen-cloud");
        assert_eq!(canonical_provider_id("qwen"), "qwen-cloud");
    }

    #[test]
    fn user_providers_yaml_merges_overrides_and_reports_errors() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "meerai_providers_test_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();

        fs::write(
            user_providers_path(&dir),
            "providers:\n  - id: openai\n    default_model: gpt-4.1\n  - id: local-vllm\n    display_name: Local vLLM\n    adapter: openai\n    base_url: http://127.0.0.1:8000/v1/\n    requires_api_key: false\n    default_model: qwen2.5-coder\n",
        )
        .unwrap();

        let (catalog, warnings) = ProviderCatalog::load_with_user_dir_checked(Some(&dir));
        assert!(warnings.is_empty());

        let openai = catalog.provider("openai").unwrap();
        assert_eq!(openai.env_vars, vec!["OPENAI_API_KEY".to_string()]);
        assert_eq!(openai.default_model.as_deref(), Some("gpt-4.1"));

        let vllm = catalog.provider("local-vllm").unwrap();
        assert_eq!(vllm.auth, ProviderAuthKind::None);
        assert_eq!(
            vllm.default_base_url.as_deref(),
            Some("http://127.0.0.1:8000/v1/")
        );
        assert_eq!(
            catalog.default_model_for_provider("local-vllm").as_deref(),
            Some("qwen2.5-coder")
        );

        fs::write(user_providers_path(&dir), "providers: [invalid").unwrap();
        let (_, bad_warnings) = ProviderCatalog::load_with_user_dir_checked(Some(&dir));
        assert_eq!(bad_warnings.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_key_from_secret_store() {
        let store = MemorySecretStore::new();
        let catalog = ProviderCatalog::new();
        let openai = catalog.provider("openai").unwrap();

        store.set_key("openai", "sk-keychain").unwrap();
        assert_eq!(
            resolve_api_key(openai, &store).unwrap().as_deref(),
            Some("sk-keychain")
        );
        assert_eq!(check_key_status(openai, &store), KeyStatus::Stored);
    }

    #[test]
    fn resolves_key_from_config_before_missing_and_formats_auth_list() {
        let store = MemorySecretStore::new();
        store.set_key("anthropic", "sk-ant-1234567890abcd").unwrap();

        let mut settings = AppSettings::default();
        settings.providers.insert(
            "openai".to_string(),
            meerai_common::settings::SavedProviderSettings {
                options: Some(ProviderOptions {
                    api_key: Some("sk-proj-abcdefgh9999".to_string()),
                }),
                ..Default::default()
            },
        );
        let mut catalog = ProviderCatalog::new();
        catalog.apply_settings(&settings);

        let openai = catalog.provider("openai").unwrap();
        assert_eq!(check_key_status(openai, &store), KeyStatus::Config);
        assert_eq!(
            resolve_api_key(openai, &store).unwrap().as_deref(),
            Some("sk-proj-abcdefgh9999")
        );

        let output = format_auth_list_with_env(
            &catalog,
            &store,
            Some(Path::new("/tmp/data/meerai/auth.json")),
            Some(Path::new("/tmp/cfg/meerai")),
            &|var| match var {
                "GEMINI_API_KEY" => Some("AIzaSyTestKey12345".to_string()),
                _ => None,
            },
        );
        assert!(output.contains("Credentials (/tmp/data/meerai/auth.json)"));
        assert!(output.contains("anthropic"));
        assert!(output.contains("sk-a...abcd"));
        assert!(output.contains("Config (/tmp/cfg/meerai/settings.yml, providers.yml)"));
        assert!(output.contains("openai"));
        assert!(output.contains("sk-p...9999"));
        assert!(output.contains("Environment"));
        assert!(output.contains("GEMINI_API_KEY"));
        assert!(output.contains("AIza...2345"));
    }

    #[test]
    fn oauth_credentials_drive_status_and_auth_listing() {
        let store = MemorySecretStore::new();
        let mut credential = OAuthCredential {
            access: "access-token".to_string(),
            refresh: "refresh-token".to_string(),
            expires: now_ms() + 42 * 60_000,
            client_id: "oaiapp_test".to_string(),
            scopes: vec!["chatgpt.tokens.use.direct".to_string()],
            account: Some("user@example.com".to_string()),
            needs_relogin: false,
        };
        store
            .write_credential("openai", &Credential::OAuth(credential.clone()))
            .unwrap();

        let catalog = ProviderCatalog::new();
        let openai = catalog.provider("openai").unwrap();
        assert_eq!(
            check_key_status(openai, &store),
            KeyStatus::OAuth {
                expires_at_ms: credential.expires,
                needs_relogin: false
            }
        );
        assert_eq!(
            resolve_api_key(openai, &store).unwrap().as_deref(),
            Some("access-token")
        );
        assert!(matches!(
            resolve_credential(openai, &store).unwrap(),
            Some(Credential::OAuth(_))
        ));

        let output = format_auth_list_with_env(
            &catalog,
            &store,
            Some(Path::new("/tmp/data/meerai/auth.json")),
            Some(Path::new("/tmp/cfg/meerai")),
            &|_| None,
        );
        assert!(output.contains("plan"));
        assert!(output.contains("user@example.com"));
        assert!(!output.contains("access-token"));

        credential.needs_relogin = true;
        store
            .write_credential("openai", &Credential::OAuth(credential.clone()))
            .unwrap();
        let status = check_key_status(openai, &store);
        assert!(!status.is_configured());
        assert_eq!(status.badge(), "plan");
    }

    #[test]
    fn plan_usage_routes_openai_through_the_explicit_default_port() {
        let settings = AppSettings::default();
        let mut runtime = RuntimeProviderState::from_settings(&settings, ProviderCatalog::new());
        let openai = runtime.catalog.provider("openai").unwrap().clone();
        assert_eq!(runtime.effective_base_url(&openai), None);

        runtime.plan_usage = true;
        assert_eq!(
            runtime.effective_base_url(&openai).as_deref(),
            Some("https://api.openai.com:443/v1/")
        );

        let ollama = runtime.catalog.provider("ollama").unwrap().clone();
        assert_eq!(runtime.effective_base_url(&ollama), None);
    }

    #[test]
    fn opencode_go_defaults_to_the_zen_endpoint() {
        let catalog = ProviderCatalog::new();
        let opencode_go = catalog.provider("opencode-go").unwrap();
        assert_eq!(
            opencode_go.default_base_url.as_deref(),
            Some("https://opencode.ai/zen/go/v1/")
        );
    }
}

#[cfg(test)]
mod model_metadata_tests {
    use super::*;

    #[test]
    fn bundled_models_expose_context_windows_and_effort_levels() {
        let catalog = ProviderCatalog::new();
        let info = catalog
            .model_info("opencode-go", "deepseek-v4.1-flash")
            .expect("bundled model metadata");
        assert_eq!(info.context_window, Some(1_000_000));
        assert_eq!(info.max_output_tokens, Some(384_000));
        assert_eq!(
            info.effort_levels.as_deref(),
            Some([EffortLevel::Low, EffortLevel::High, EffortLevel::Max].as_slice())
        );
        assert!(info.supports_reasoning());
        assert_eq!(
            catalog.effort_levels_for("opencode_go", "deepseek-v4.1-flash"),
            Some(vec![EffortLevel::Low, EffortLevel::High, EffortLevel::Max])
        );
    }

    #[test]
    fn bundled_catalog_covers_every_provider_and_caps_gateways() {
        let catalog = ProviderCatalog::new();
        assert!(catalog.models().len() >= 300);
        for provider in catalog.providers() {
            if provider.id == "ollama" {
                continue;
            }
            assert!(
                !catalog.models_for_provider(&provider.id).is_empty(),
                "{} has no models",
                provider.id
            );
            assert!(
                catalog.models_for_provider(&provider.id).len() <= 50,
                "{} exceeds the model cap",
                provider.id
            );
        }
        let openrouter = catalog.models_for_provider("openrouter");
        assert!(openrouter.len() <= 50);
        assert!(
            openrouter
                .iter()
                .any(|model| model.id.starts_with("openai/"))
        );
        assert!(
            openrouter
                .iter()
                .any(|model| model.id.starts_with("anthropic/"))
        );
    }

    #[test]
    fn model_lookup_strips_the_provider_prefix() {
        let catalog = ProviderCatalog::from_yaml(
            "providers: []\nmodels:\n  - provider: Gemini\n    id: gemini-2.5-pro\n    context_window: 1048576\n",
        )
        .unwrap();
        let info = catalog
            .model_info("gemini", "gemini/gemini-2.5-pro")
            .expect("prefixed lookup");
        assert_eq!(info.context_window, Some(1_048_576));
        let spec = catalog
            .model_info("gemini", "gemini::gemini-2.5-pro")
            .expect("genai spec lookup");
        assert_eq!(spec.context_window, Some(1_048_576));
    }

    #[test]
    fn user_models_override_bundled_entries_and_keep_pricing() {
        let mut catalog = ProviderCatalog::new();
        let bundled = catalog.models().len();
        catalog.merge_file(ProvidersFile {
            providers: Vec::new(),
            models: vec![ModelInfo {
                provider: "opencode-go".to_string(),
                id: "deepseek-v4.1-flash".to_string(),
                context_window: Some(64_000),
                max_output_tokens: None,
                input_cost_per_mtok: Some(0.2),
                output_cost_per_mtok: Some(0.8),
                effort_levels: None,
            }],
        });
        let info = catalog
            .model_info("opencode_go", "deepseek-v4.1-flash")
            .expect("overridden model");
        assert_eq!(info.context_window, Some(64_000));
        assert_eq!(info.input_cost_per_mtok, Some(0.2));
        assert_eq!(
            info.effort_levels.as_deref(),
            Some([EffortLevel::Low, EffortLevel::High, EffortLevel::Max].as_slice()),
            "a partial override keeps the bundled effort set"
        );
        assert_eq!(catalog.models().len(), bundled);
    }
}
