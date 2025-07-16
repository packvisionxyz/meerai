use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use genai::adapter::AdapterKind;
use genai::chat::ChatOptions;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, Headers, ModelIden};
use serde::Deserialize;

use crate::secrets::SecretStore;
use crate::settings::{
    AppSettings, ProviderOptions, expand_config_secret, legacy_user_models_path,
    user_providers_path,
};

const BUNDLED_PROVIDERS_YAML: &str = include_str!("../providers.yml");
pub const MEERAI_USER_AGENT: &str = concat!("meerai/", env!("CARGO_PKG_VERSION"));

pub fn generate_opencode_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    format!("ses_{millis:012x}{pid:08x}{seq:08x}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub display_name: String,
    pub adapter_kind: AdapterKind,
    pub default_base_url: Option<String>,
    pub env_vars: Vec<String>,
    pub requires_api_key: bool,
    pub default_model: Option<String>,
    pub config_api_key: Option<String>,
}

impl ProviderInfo {
    pub fn genai_model_spec(&self, model_id: &str) -> String {
        let clean = model_id
            .strip_prefix(&format!("{}/", self.id))
            .unwrap_or(model_id);
        format!("{}::{}", self.adapter_kind.as_lower_str(), clean)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    Stored,
    Config,
    Env(String),
    NotRequired,
    Missing,
}

impl KeyStatus {
    pub fn is_configured(&self) -> bool {
        !matches!(self, Self::Missing)
    }

    pub fn badge(&self) -> String {
        match self {
            Self::Stored => "saved".to_string(),
            Self::Config => "config".to_string(),
            Self::Env(var) => format!("env:{var}"),
            Self::NotRequired => "local".to_string(),
            Self::Missing => "not set".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProvidersFile {
    providers: Vec<ProviderConfigEntry>,
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
        "groq" => "groq".to_string(),
        "xai" => "xai".to_string(),
        "mistral" => "mistral".to_string(),
        "ollama" => "ollama".to_string(),
        "aliyun" => "alibaba-token-plan".to_string(),
        other => other.to_string(),
    }
}

fn parse_adapter_kind(adapter: Option<&str>) -> AdapterKind {
    match adapter
        .unwrap_or("openai")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "opencode_go" | "opencode-go" => AdapterKind::OpenCodeGo,
        "open_router" | "openrouter" => AdapterKind::OpenRouter,
        "gemini" | "google" => AdapterKind::Gemini,
        "anthropic" => AdapterKind::Anthropic,
        "deepseek" => AdapterKind::DeepSeek,
        "groq" => AdapterKind::Groq,
        "xai" => AdapterKind::Xai,
        "aliyun" => AdapterKind::Aliyun,
        "ollama" => AdapterKind::Ollama,
        "mistral" => AdapterKind::Mistral,
        "cohere" => AdapterKind::Cohere,
        _ => AdapterKind::OpenAI,
    }
}

fn synthesize_provider(provider_id: &str) -> ProviderInfo {
    let upper = provider_id.to_ascii_uppercase().replace('-', "_");
    ProviderInfo {
        id: provider_id.to_string(),
        display_name: provider_id.to_string(),
        adapter_kind: AdapterKind::OpenAI,
        default_base_url: None,
        env_vars: vec![format!("{upper}_API_KEY")],
        requires_api_key: true,
        default_model: None,
        config_api_key: None,
    }
}

#[derive(Clone, Debug)]
pub struct ProviderCatalog {
    providers: Vec<ProviderInfo>,
}

impl Default for ProviderCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderCatalog {
    pub fn new() -> Self {
        Self::from_yaml(BUNDLED_PROVIDERS_YAML).expect("bundled providers.yml must parse")
    }

    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let parsed: ProvidersFile =
            serde_yaml::from_str(yaml).context("failed to parse providers.yml configuration")?;
        let mut catalog = Self {
            providers: Vec::new(),
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
            let primary = user_providers_path(dir);
            let path = if primary.is_file() {
                Some(primary)
            } else {
                let legacy = legacy_user_models_path(dir);
                legacy.is_file().then_some(legacy)
            };
            if let Some(custom_path) = path {
                match fs::read_to_string(&custom_path) {
                    Ok(raw) => match serde_yaml::from_str::<ProvidersFile>(&raw) {
                        Ok(parsed) => catalog.merge_file(parsed),
                        Err(err) => warnings
                            .push(format!("Failed to parse {}: {err}", custom_path.display())),
                    },
                    Err(err) => {
                        warnings.push(format!("Failed to read {}: {err}", custom_path.display()))
                    }
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
            let config_api_key = entry
                .options
                .as_ref()
                .and_then(|opts| opts.api_key.as_deref())
                .and_then(expand_config_secret)
                .or_else(|| entry.api_key.as_deref().and_then(expand_config_secret));

            if let Some(existing) = self.providers.iter_mut().find(|p| p.id == id) {
                if let Some(display_name) = entry.display_name {
                    existing.display_name = display_name;
                }
                if let Some(adapter) = entry.adapter.as_deref() {
                    existing.adapter_kind = parse_adapter_kind(Some(adapter));
                }
                if entry.base_url.is_some() {
                    existing.default_base_url = entry.base_url;
                }
                if !entry.env_vars.is_empty() {
                    existing.env_vars = entry.env_vars;
                }
                if let Some(requires_api_key) = entry.requires_api_key {
                    existing.requires_api_key = requires_api_key;
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
                    adapter_kind: parse_adapter_kind(entry.adapter.as_deref()),
                    default_base_url: entry.base_url,
                    env_vars: entry.env_vars,
                    requires_api_key: entry.requires_api_key.unwrap_or(true),
                    default_model,
                    config_api_key,
                });
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
    if let Ok(Some(_)) = secrets.get_key(&provider.id) {
        return KeyStatus::Stored;
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
    if !provider.requires_api_key {
        return KeyStatus::NotRequired;
    }
    KeyStatus::Missing
}

pub fn resolve_api_key(
    provider: &ProviderInfo,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    if let Some(key) = secrets.get_key(&provider.id)? {
        return Ok(Some(key));
    }
    if let Some(key) = provider
        .config_api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        return Ok(Some(key.to_string()));
    }
    for var in &provider.env_vars {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed));
            }
        }
    }
    Ok(None)
}

pub fn first_configured_provider(
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
) -> Option<String> {
    let mut seen = HashSet::new();
    for provider in catalog.providers() {
        if seen.insert(provider.id.as_str())
            && provider.requires_api_key
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
        self.base_urls
            .get(&provider.id)
            .cloned()
            .or_else(|| provider.default_base_url.clone())
    }
}

pub fn build_genai_client(
    runtime: Arc<RwLock<RuntimeProviderState>>,
    secrets: Arc<dyn SecretStore>,
) -> Result<Client> {
    let auth_runtime = runtime.clone();
    let auth_secrets = secrets;
    let target_runtime = runtime;
    let default_session_id = generate_opencode_session_id();

    let client = Client::builder()
        .with_chat_options(ChatOptions::default().with_extra_headers(Headers::from([
            ("x-opencode-session", default_session_id.as_str()),
            ("user-agent", MEERAI_USER_AGENT),
        ])))
        .with_auth_resolver_fn(move |_model_iden: ModelIden| {
            let state = auth_runtime.read().unwrap().clone();
            let provider = state.active_provider_info();
            match resolve_api_key(&provider, auth_secrets.as_ref()) {
                Ok(Some(key)) => Ok(Some(AuthData::from_single(key))),
                Ok(None) if !provider.requires_api_key => Ok(None),
                Ok(None) => Err(genai::resolver::Error::Custom(format!(
                    "No API key configured for provider '{}'. Use /connect to save one.",
                    provider.id
                ))),
                Err(err) => Err(genai::resolver::Error::Custom(format!(
                    "Failed to read API key for '{}': {err:#}",
                    provider.id
                ))),
            }
        })
        .with_service_target_resolver_fn(move |mut target: genai::ServiceTarget| {
            let state = target_runtime.read().unwrap().clone();
            let provider = state.active_provider_info();
            if let Some(url) = state.effective_base_url(&provider) {
                let normalized = if url.ends_with('/') {
                    url
                } else {
                    format!("{url}/")
                };
                target.endpoint = Endpoint::from_owned(normalized);
            }
            Ok(target)
        })
        .build()?;

    Ok(client)
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
        .unwrap_or_else(|| "auth.json".to_string());
    let _ = writeln!(out, "Credentials ({auth_label})");
    let mut stored_count = 0;
    let mut seen_stored = BTreeSet::new();
    for provider in catalog.providers() {
        if let Ok(Some(key)) = secrets.get_key(&provider.id) {
            seen_stored.insert(provider.id.clone());
            stored_count += 1;
            let _ = writeln!(
                out,
                "  {} ({})  api  {}",
                provider.display_name,
                provider.id,
                mask_secret(&key)
            );
        }
    }
    if let Ok(extra_ids) = secrets.list_providers() {
        for id in extra_ids {
            if !seen_stored.contains(&id)
                && let Ok(Some(key)) = secrets.get_key(&id)
            {
                stored_count += 1;
                let _ = writeln!(out, "  {}  api  {}", id, mask_secret(&key));
            }
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
    use crate::secrets::MemorySecretStore;

    #[test]
    fn bundled_providers_yml_loads_opencode_and_direct_providers() {
        let catalog = ProviderCatalog::new();

        let ids: Vec<&str> = catalog.providers().iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"opencode-go"));
        assert!(ids.contains(&"opencode"));
        assert!(ids.contains(&"openrouter"));
        assert!(ids.contains(&"alibaba-token-plan"));
        assert!(ids.contains(&"gemini"));
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"anthropic"));
        assert!(ids.contains(&"deepseek"));
        assert!(ids.contains(&"groq"));
        assert!(ids.contains(&"xai"));
        assert!(ids.contains(&"ollama"));

        let ollama = catalog.provider("ollama").unwrap();
        assert!(!ollama.requires_api_key);

        let alibaba = catalog.provider("alibaba-token-plan").unwrap();
        assert_eq!(alibaba.adapter_kind, AdapterKind::Aliyun);
        assert!(alibaba.default_base_url.is_some());

        assert_eq!(
            catalog.format_genai_spec("opencode-go", "deepseek-v4.1-flash"),
            "opencode_go::deepseek-v4.1-flash"
        );
        assert_eq!(
            catalog.format_genai_spec("openrouter", "anthropic/claude-sonnet-4.5"),
            "open_router::anthropic/claude-sonnet-4.5"
        );
        assert_eq!(
            catalog.format_genai_spec("custom-proxy", "my-custom-model"),
            "openai::my-custom-model"
        );
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
        assert!(!vllm.requires_api_key);
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
            crate::settings::SavedProviderSettings {
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
    fn opencode_go_defaults_and_session_ids() {
        let catalog = ProviderCatalog::new();
        let opencode_go = catalog.provider("opencode-go").unwrap();
        assert_eq!(
            opencode_go.default_base_url.as_deref(),
            Some("https://opencode.ai/zen/go/v1/")
        );
        let id1 = generate_opencode_session_id();
        let id2 = generate_opencode_session_id();
        assert!(id1.starts_with("ses_"));
        assert!(id2.starts_with("ses_"));
        assert_ne!(id1, id2);
    }
}
