use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use meerai_policy::{Effect, PermissionAction};
use regex::Regex;
use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.yml";
const USER_PROVIDERS_FILE: &str = "providers.yml";
const MAX_RECENT_MODELS: usize = 20;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOptions {
    #[serde(default, alias = "apiKey", skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, alias = "apiKey", skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ProviderOptions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_models: Vec<String>,
}

impl SavedProviderSettings {
    fn is_empty(&self) -> bool {
        self.base_url.is_none()
            && self.model.is_none()
            && self.api_key.is_none()
            && self
                .options
                .as_ref()
                .is_none_or(|opts| opts.api_key.is_none())
            && self.recent_models.is_empty()
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        resolve_config_api_key(self.options.as_ref(), self.api_key.as_deref())
    }
}

pub fn resolve_config_api_key(
    options: Option<&ProviderOptions>,
    api_key: Option<&str>,
) -> Option<String> {
    options
        .and_then(|opts| opts.api_key.as_deref())
        .and_then(expand_config_secret)
        .or_else(|| api_key.and_then(expand_config_secret))
}

pub fn expand_config_secret(raw: &str) -> Option<String> {
    expand_config_secret_with(raw, &|name| std::env::var(name).ok())
}

fn expand_config_secret_with(
    raw: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    static ENV_BRACE_RE: OnceLock<Regex> = OnceLock::new();
    static DOLLAR_BRACE_RE: OnceLock<Regex> = OnceLock::new();

    let env_re = ENV_BRACE_RE
        .get_or_init(|| Regex::new(r"\{env:([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid env regex"));
    let dollar_re = DOLLAR_BRACE_RE.get_or_init(|| {
        Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:\|([^}]*))?\}").expect("valid dollar regex")
    });

    let stage1 = env_re.replace_all(raw, |caps: &regex::Captures| {
        lookup(&caps[1]).unwrap_or_default()
    });
    let stage2 = dollar_re.replace_all(&stage1, |caps: &regex::Captures| {
        lookup(&caps[1]).unwrap_or_else(|| caps.get(2).map_or("", |m| m.as_str()).to_string())
    });

    let clean = stage2.trim();
    if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Temperature(f32);

impl Temperature {
    pub fn new(value: f32) -> Self {
        Self(value)
    }

    pub fn value(self) -> f32 {
        self.0
    }

    pub fn effective(self) -> Option<f32> {
        let value = self.0;
        (0.0..=2.0).contains(&value).then_some(value)
    }
}

impl Eq for Temperature {}

pub const DEFAULT_TEMPERATURE: f32 = 0.2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    #[default]
    Medium,
    High,
}

impl EffortLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn default_temperature(self) -> f32 {
        match self {
            Self::Low => 0.0,
            Self::Medium => DEFAULT_TEMPERATURE,
            Self::High => 0.5,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Low,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_watermark_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_max_inline_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbatim_window_turns: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_pruning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_caching: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_on_verify_failure: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps_per_turn: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_session_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turn_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarizer_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_edit_diagnostics: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortLevel>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_prune_days: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HooksSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_project: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_projects: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_for_scripts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_for_shell: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_env: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_high: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_removed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_function: Option<String>,
}

impl ThemeSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn overlay(&mut self, other: Self) {
        macro_rules! overlay {
            ($($field:ident),* $(,)?) => {
                $(
                    if other.$field.is_some() {
                        self.$field = other.$field;
                    }
                )*
            };
        }
        overlay!(
            color_scheme,
            background,
            surface,
            surface_high,
            text,
            muted,
            faint,
            accent,
            interactive,
            border,
            error,
            warning,
            success,
            info,
            diff_added,
            diff_removed,
            diff_context,
            code_function,
        );
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_model: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, SavedProviderSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<serde_yaml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<SessionSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HooksSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeSettings>,
}

impl AppSettings {
    pub fn provider_model(&self, provider_id: &str) -> Option<&str> {
        self.providers
            .get(provider_id)
            .and_then(|p| p.model.as_deref())
            .filter(|m| !m.trim().is_empty())
    }

    pub fn provider_api_key(&self, provider_id: &str) -> Option<String> {
        self.providers
            .get(provider_id)
            .and_then(SavedProviderSettings::resolved_api_key)
    }

    pub fn recent_models_for(&self, provider_id: &str) -> &[String] {
        self.providers
            .get(provider_id)
            .map(|p| p.recent_models.as_slice())
            .unwrap_or(&[])
    }

    pub fn set_provider_base_url(&mut self, provider_id: &str, base_url: Option<String>) {
        let clean = base_url
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty());
        if let Some(url) = clean {
            self.providers
                .entry(provider_id.to_string())
                .or_default()
                .base_url = Some(url);
        } else if let Some(entry) = self.providers.get_mut(provider_id) {
            entry.base_url = None;
            if entry.is_empty() {
                self.providers.remove(provider_id);
            }
        }
    }

    pub fn record_provider_model(&mut self, provider_id: &str, model_id: &str) {
        let clean = model_id.trim().to_string();
        if clean.is_empty() {
            return;
        }
        self.active_provider = Some(provider_id.to_string());
        self.active_model = Some(clean.clone());

        let entry = self.providers.entry(provider_id.to_string()).or_default();
        entry.model = Some(clean.clone());
        entry.recent_models.retain(|m| m != &clean);
        entry.recent_models.insert(0, clean);
        entry.recent_models.truncate(MAX_RECENT_MODELS);
    }

    pub fn forget_provider_model(&mut self, provider_id: &str, model_id: &str) {
        let clean = model_id.trim();
        if clean.is_empty() {
            return;
        }
        if let Some(entry) = self.providers.get_mut(provider_id) {
            entry.recent_models.retain(|m| m != clean);
            if entry.model.as_deref() == Some(clean) {
                entry.model = None;
            }
            if entry.is_empty() {
                self.providers.remove(provider_id);
            }
        }
        if self.active_provider.as_deref() == Some(provider_id)
            && self.active_model.as_deref() == Some(clean)
        {
            self.active_model = None;
        }
    }
}

pub fn default_config_dir() -> Option<PathBuf> {
    meerai_core::env::config_dir()
}

pub fn default_data_dir() -> Option<PathBuf> {
    meerai_core::env::data_dir()
}

pub fn user_providers_path(dir: &Path) -> PathBuf {
    dir.join(USER_PROVIDERS_FILE)
}

pub fn load_settings_from(dir: &Path) -> Result<AppSettings> {
    let path = dir.join(SETTINGS_FILE);
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read settings at {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(AppSettings::default());
    }
    serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse settings at {}", path.display()))
}

pub fn save_settings_to(dir: &Path, settings: &AppSettings) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create config dir {}", dir.display()))?;
    let path = dir.join(SETTINGS_FILE);
    let yaml = serde_yaml::to_string(settings).context("failed to serialize settings")?;
    fs::write(&path, yaml)
        .with_context(|| format!("failed to write settings at {}", path.display()))
}

pub fn find_project_root(start: &Path) -> PathBuf {
    for dir in start.ancestors() {
        if dir.as_os_str().is_empty() {
            break;
        }
        if dir.join(".git").exists() || dir.join(".meerai").join(SETTINGS_FILE).exists() {
            return dir.to_path_buf();
        }
    }
    start.to_path_buf()
}

pub fn load_project_settings(workspace: &Path) -> Result<Option<AppSettings>> {
    let path = workspace.join(".meerai").join(SETTINGS_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read project settings at {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let settings = serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse project settings at {}", path.display()))?;
    Ok(Some(settings))
}

pub fn merge_project_settings(
    user: AppSettings,
    project: AppSettings,
) -> (AppSettings, Vec<String>) {
    let mut warnings = Vec::new();
    let mut merged = user;

    if project.active_provider.is_some() {
        warnings.push(
            "ignoring project active_provider: a repository cannot set the active provider"
                .to_string(),
        );
    }
    if project.active_model.is_some() {
        warnings.push(
            "ignoring project active_model: a repository cannot set the active model".to_string(),
        );
    }
    if !project.providers.is_empty() {
        warnings.push(
            "ignoring project providers: a repository cannot change provider endpoints".to_string(),
        );
    }
    if project.approval_mode.is_some() {
        warnings.push(
            "ignoring project approval_mode: a repository cannot change the approval mode"
                .to_string(),
        );
    }
    if project.sessions.is_some() {
        warnings.push(
            "ignoring project sessions: only the user config controls session retention"
                .to_string(),
        );
    }
    if project.hooks.is_some() {
        warnings.push(
            "ignoring project hooks settings: project hooks are read from .meerai/hooks.json after explicit trust"
                .to_string(),
        );
    }

    if let Some(permissions) = project.permissions.as_ref()
        && let Some((asks, denies)) = split_project_permissions(permissions, &mut warnings)
    {
        merged.permissions = combine_rules(merged.permissions.take(), asks, denies, &mut warnings);
    }

    merged.context = merge_context(merged.context.take(), project.context);
    merged.sandbox = merge_sandbox(merged.sandbox.take(), project.sandbox, &mut warnings);

    if let Some(project_theme) = project.theme {
        let mut theme = merged.theme.take().unwrap_or_default();
        theme.overlay(project_theme);
        merged.theme = Some(theme);
    }

    (merged, warnings)
}

fn combine_rules(
    user: Option<serde_yaml::Value>,
    mut asks: Vec<serde_yaml::Value>,
    denies: Vec<serde_yaml::Value>,
    warnings: &mut Vec<String>,
) -> Option<serde_yaml::Value> {
    match user {
        None => {
            asks.extend(denies);
            Some(serde_yaml::Value::Sequence(asks))
        }
        Some(serde_yaml::Value::Sequence(mut user_rules)) => {
            asks.append(&mut user_rules);
            asks.extend(denies);
            Some(serde_yaml::Value::Sequence(asks))
        }
        Some(other) => {
            warnings.push(
                "ignoring project permissions: user permissions are not a list of rules"
                    .to_string(),
            );
            Some(other)
        }
    }
}

fn split_project_permissions(
    value: &serde_yaml::Value,
    warnings: &mut Vec<String>,
) -> Option<(Vec<serde_yaml::Value>, Vec<serde_yaml::Value>)> {
    let Some(entries) = value.as_sequence() else {
        warnings.push("ignoring project permissions: expected a list of rules".to_string());
        return None;
    };
    let mut asks = Vec::new();
    let mut denies = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(map) = entry.as_mapping() else {
            warnings.push(format!(
                "ignoring project permissions: rule {} is not a mapping",
                index + 1
            ));
            return None;
        };
        let action = mapping_str(map, "action");
        let resource = mapping_str(map, "resource");
        let effect = mapping_str(map, "effect");
        let valid_action =
            action.is_some_and(|action| action == "*" || PermissionAction::parse(action).is_some());
        if !valid_action || resource.is_none() || effect.is_none() {
            warnings.push(format!(
                "ignoring project permissions: rule {} is not a valid rule",
                index + 1
            ));
            return None;
        }
        match effect.and_then(Effect::parse) {
            Some(Effect::Allow) => warnings.push(format!(
                "dropped project permission rule {} (action: {}, resource: {}, effect: allow)",
                index + 1,
                action.unwrap_or_default(),
                resource.unwrap_or_default()
            )),
            Some(Effect::Ask) => asks.push(entry.clone()),
            Some(Effect::Deny) => denies.push(entry.clone()),
            None => {
                warnings.push(format!(
                    "ignoring project permissions: rule {} has an unknown effect",
                    index + 1
                ));
                return None;
            }
        }
    }
    Some((asks, denies))
}

fn mapping_str<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    map.iter()
        .find(|(name, _)| name.as_str() == Some(key))
        .and_then(|(_, value)| value.as_str())
}

fn merge_context(
    user: Option<ContextSettings>,
    project: Option<ContextSettings>,
) -> Option<ContextSettings> {
    match (user, project) {
        (None, None) => None,
        (Some(user), None) => Some(user),
        (None, Some(project)) => Some(project),
        (Some(user), Some(project)) => Some(ContextSettings {
            context_window_tokens: project.context_window_tokens.or(user.context_window_tokens),
            high_watermark_pct: project.high_watermark_pct.or(user.high_watermark_pct),
            spill_max_inline_tokens: project
                .spill_max_inline_tokens
                .or(user.spill_max_inline_tokens),
            verbatim_window_turns: project.verbatim_window_turns.or(user.verbatim_window_turns),
            reasoning_pruning: project.reasoning_pruning.or(user.reasoning_pruning),
            prompt_caching: project.prompt_caching.or(user.prompt_caching),
            rollback_on_verify_failure: project
                .rollback_on_verify_failure
                .or(user.rollback_on_verify_failure),
            recovery_model: project
                .recovery_model
                .filter(|model| !model.contains(':'))
                .or(user.recovery_model),
            max_steps_per_turn: project.max_steps_per_turn.or(user.max_steps_per_turn),
            tool_timeout_ms: project.tool_timeout_ms.or(user.tool_timeout_ms),
            max_session_tokens: merge_max_session_tokens(
                user.max_session_tokens,
                project.max_session_tokens,
            ),
            max_turn_secs: merge_max_turn_secs(user.max_turn_secs, project.max_turn_secs),
            summarizer_model: project.summarizer_model.or(user.summarizer_model),
            post_edit_diagnostics: project.post_edit_diagnostics.or(user.post_edit_diagnostics),
            temperature: project.temperature.or(user.temperature),
            effort: project.effort.or(user.effort),
        }),
    }
}

fn merge_max_session_tokens(user: Option<u64>, project: Option<u64>) -> Option<u64> {
    match (user, project) {
        (Some(user), Some(project)) => Some(user.min(project)),
        (Some(user), None) => Some(user),
        (None, project) => project,
    }
}

fn merge_max_turn_secs(user: Option<u64>, project: Option<u64>) -> Option<u64> {
    match user {
        None => None,
        Some(0) => Some(project.unwrap_or(0)),
        Some(user) => match project {
            Some(0) | None => Some(user),
            Some(project) => Some(user.min(project)),
        },
    }
}

fn merge_sandbox(
    user: Option<SandboxSettings>,
    project: Option<SandboxSettings>,
    warnings: &mut Vec<String>,
) -> Option<SandboxSettings> {
    match (user, project) {
        (user, None) => user,
        (None, Some(project)) => {
            let merged = SandboxSettings {
                require_for_scripts: project.require_for_scripts.filter(|v| *v),
                require_for_shell: project.require_for_shell.filter(|v| *v),
                strict_env: project.strict_env.filter(|v| *v),
            };
            if merged == SandboxSettings::default() {
                None
            } else {
                Some(merged)
            }
        }
        (Some(user), Some(project)) => {
            let require_for_scripts = match (user.require_for_scripts, project.require_for_scripts)
            {
                (Some(true), Some(false)) => {
                    warnings.push(
                        "ignoring project sandbox.require_for_scripts=false: a repository cannot weaken sandbox enforcement"
                            .to_string(),
                    );
                    Some(true)
                }
                (u, Some(true)) => {
                    let _ = u;
                    Some(true)
                }
                (u, _) => u,
            };
            let require_for_shell = match (user.require_for_shell, project.require_for_shell) {
                (Some(true), Some(false)) => {
                    warnings.push(
                        "ignoring project sandbox.require_for_shell=false: a repository cannot weaken sandbox enforcement"
                            .to_string(),
                    );
                    Some(true)
                }
                (u, Some(true)) => {
                    let _ = u;
                    Some(true)
                }
                (u, _) => u,
            };
            Some(SandboxSettings {
                require_for_scripts,
                require_for_shell,
                strict_env: user.strict_env.or(project.strict_env.filter(|v| *v)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "meerai_common_{name}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn settings_roundtrip() {
        let dir = temp_dir("settings");
        assert_eq!(load_settings_from(&dir).unwrap(), AppSettings::default());

        let mut settings = AppSettings {
            active_provider: Some("opencode-go".to_string()),
            active_model: Some("deepseek-v4.1-flash".to_string()),
            ..AppSettings::default()
        };
        settings.set_provider_base_url("opencode", Some("https://opencode.ai/zen/v1/".to_string()));
        settings.record_provider_model("opencode", "glm-5.3-flash-free");
        settings.record_provider_model("opencode", "big-pickle");
        settings.record_provider_model("opencode", "glm-5.3-flash-free");
        assert_eq!(
            settings.recent_models_for("opencode"),
            &["glm-5.3-flash-free".to_string(), "big-pickle".to_string(),]
        );
        assert_eq!(
            settings.provider_model("opencode"),
            Some("glm-5.3-flash-free")
        );

        save_settings_to(&dir, &settings).unwrap();
        assert_eq!(load_settings_from(&dir).unwrap(), settings);

        settings.set_provider_base_url("opencode", None);
        assert_eq!(
            settings.provider_model("opencode"),
            Some("glm-5.3-flash-free")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn expands_config_secrets_and_provider_options_api_key() {
        let lookup = |name: &str| match name {
            "OPENAI_API_KEY" => Some("sk-from-env".to_string()),
            _ => None,
        };

        assert_eq!(
            expand_config_secret_with("{env:OPENAI_API_KEY}", &lookup).as_deref(),
            Some("sk-from-env")
        );
        assert_eq!(
            expand_config_secret_with("${OPENAI_API_KEY}", &lookup).as_deref(),
            Some("sk-from-env")
        );
        assert_eq!(
            expand_config_secret_with("${MISSING_KEY|fallback-key}", &lookup).as_deref(),
            Some("fallback-key")
        );
        assert_eq!(
            expand_config_secret_with("{env:MISSING_KEY}", &lookup),
            None
        );
        assert_eq!(
            expand_config_secret_with("  sk-literal  ", &lookup).as_deref(),
            Some("sk-literal")
        );

        let yaml = r#"
providers:
  opencode:
    options:
      apiKey: "  sk-inline-key  "
  openai:
    api_key: "sk-direct"
"#;
        let parsed: AppSettings = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            parsed.provider_api_key("opencode").as_deref(),
            Some("sk-inline-key")
        );
        assert_eq!(
            parsed.provider_api_key("openai").as_deref(),
            Some("sk-direct")
        );
    }

    #[test]
    fn legacy_settings_load_unchanged_and_omit_new_fields_on_save() {
        let dir = temp_dir("legacy_settings");
        fs::create_dir_all(&dir).unwrap();
        let legacy = concat!(
            "active_provider: opencode-go\n",
            "active_model: glm-5.3\n",
            "providers:\n",
            "  opencode-go:\n",
            "    model: glm-5.3\n",
        );
        fs::write(dir.join(SETTINGS_FILE), legacy).unwrap();

        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded.active_provider.as_deref(), Some("opencode-go"));
        assert_eq!(loaded.active_model.as_deref(), Some("glm-5.3"));
        assert_eq!(loaded.approval_mode, None);
        assert_eq!(loaded.permissions, None);
        assert_eq!(loaded.context, None);

        save_settings_to(&dir, &loaded).unwrap();
        let raw = fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(!raw.contains("approval_mode"));
        assert!(!raw.contains("permissions"));
        assert!(!raw.contains("context"));
        assert_eq!(load_settings_from(&dir).unwrap(), loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_permission_settings_roundtrip() {
        let dir = temp_dir("context_settings");
        let permissions: serde_yaml::Value = serde_yaml::from_str(
            "- { action: read, resource: \"*\", effect: allow }\n\
             - { action: shell, resource: \"rm *\", effect: deny }\n",
        )
        .unwrap();
        let settings = AppSettings {
            approval_mode: Some("auto".to_string()),
            permissions: Some(permissions),
            context: Some(ContextSettings {
                context_window_tokens: Some(200_000),
                high_watermark_pct: Some(80),
                spill_max_inline_tokens: Some(8_000),
                verbatim_window_turns: Some(6),
                reasoning_pruning: Some(false),
                prompt_caching: Some(true),
                rollback_on_verify_failure: Some(false),
                max_steps_per_turn: Some(48),
                tool_timeout_ms: Some(60_000),
                max_session_tokens: Some(1_500_000),
                max_turn_secs: Some(900),
                summarizer_model: Some("openrouter:small-model".to_string()),
                recovery_model: Some("openrouter:fallback-model".to_string()),
                post_edit_diagnostics: Some(true),
                ..ContextSettings::default()
            }),
            ..AppSettings::default()
        };

        save_settings_to(&dir, &settings).unwrap();
        assert_eq!(load_settings_from(&dir).unwrap(), settings);

        let raw = fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(raw.contains("approval_mode: auto"));
        assert!(raw.contains("context_window_tokens: 200000"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_context_settings_keep_unset_fields_absent() {
        let parsed: AppSettings = serde_yaml::from_str(
            "context:\n  high_watermark_pct: 70\n  reasoning_pruning: false\n",
        )
        .unwrap();
        let context = parsed.context.as_ref().unwrap();
        assert_eq!(context.high_watermark_pct, Some(70));
        assert_eq!(context.reasoning_pruning, Some(false));
        assert_eq!(context.context_window_tokens, None);
        assert_eq!(context.max_session_tokens, None);

        let raw = serde_yaml::to_string(&parsed).unwrap();
        assert!(!raw.contains("context_window_tokens"));
        assert!(!raw.contains("tool_timeout_ms"));
    }

    fn settings_from_yaml(raw: &str) -> AppSettings {
        serde_yaml::from_str(raw).unwrap()
    }

    fn rule_field<'a>(
        permissions: &'a serde_yaml::Value,
        index: usize,
        field: &str,
    ) -> Option<&'a str> {
        let map = permissions.as_sequence()?.get(index)?.as_mapping()?;
        map.iter()
            .find(|(name, _)| name.as_str() == Some(field))
            .and_then(|(_, value)| value.as_str())
    }

    #[test]
    fn project_settings_absent_or_empty_returns_none() {
        let dir = temp_dir("project_absent");
        assert_eq!(load_project_settings(&dir).unwrap(), None);

        fs::create_dir_all(dir.join(".meerai")).unwrap();
        assert_eq!(load_project_settings(&dir).unwrap(), None);

        fs::write(dir.join(".meerai/settings.yml"), "   \n").unwrap();
        assert_eq!(load_project_settings(&dir).unwrap(), None);

        fs::write(
            dir.join(".meerai/settings.yml"),
            "context:\n  high_watermark_pct: 75\n",
        )
        .unwrap();
        let loaded = load_project_settings(&dir).unwrap().unwrap();
        assert_eq!(
            loaded
                .context
                .and_then(|context| context.high_watermark_pct),
            Some(75)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_settings_malformed_yaml_returns_error() {
        let dir = temp_dir("project_malformed");
        fs::create_dir_all(dir.join(".meerai")).unwrap();
        fs::write(dir.join(".meerai/settings.yml"), "permissions: [unclosed\n").unwrap();

        let error = load_project_settings(&dir).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to parse project settings")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_orders_project_asks_before_user_rules_and_denies_after() {
        let user = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"rm *\", effect: deny }\n  - { action: edit, resource: \"src/**\", effect: allow }\n",
        );
        let project = settings_from_yaml(
            "permissions:\n  - { action: edit, resource: \"src/generated/**\", effect: deny }\n  - { action: shell, resource: \"cargo *\", effect: ask }\n  - { action: read, resource: \"*\", effect: allow }\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        let permissions = merged.permissions.as_ref().unwrap();

        assert_eq!(permissions.as_sequence().unwrap().len(), 4);
        assert_eq!(rule_field(permissions, 0, "effect"), Some("ask"));
        assert_eq!(rule_field(permissions, 0, "action"), Some("shell"));
        assert_eq!(rule_field(permissions, 1, "effect"), Some("deny"));
        assert_eq!(rule_field(permissions, 1, "action"), Some("shell"));
        assert_eq!(rule_field(permissions, 2, "effect"), Some("allow"));
        assert_eq!(rule_field(permissions, 2, "action"), Some("edit"));
        assert_eq!(rule_field(permissions, 3, "effect"), Some("deny"));
        assert_eq!(rule_field(permissions, 3, "action"), Some("edit"));

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("dropped project permission rule 3"));
        assert!(warnings[0].contains("action: read"));
        assert!(warnings[0].contains("effect: allow"));
    }

    #[test]
    fn find_project_root_walks_up_to_git_or_project_settings() {
        let git_root = temp_dir("project_root_git");
        fs::create_dir_all(git_root.join(".git")).unwrap();
        let nested = git_root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_project_root(&nested), git_root);

        let worktree_root = temp_dir("project_root_worktree");
        fs::create_dir_all(worktree_root.join("a")).unwrap();
        fs::write(worktree_root.join(".git"), "gitdir: ../.git/worktrees/wt\n").unwrap();
        assert_eq!(
            find_project_root(&worktree_root.join("a")),
            worktree_root,
            "a .git file must mark the project root"
        );

        let marked_root = temp_dir("project_root_marked");
        fs::create_dir_all(marked_root.join(".meerai")).unwrap();
        fs::write(
            marked_root.join(".meerai").join(SETTINGS_FILE),
            "context:\n  high_watermark_pct: 70\n",
        )
        .unwrap();
        let marked_nested = marked_root.join("a").join("b");
        fs::create_dir_all(&marked_nested).unwrap();
        assert_eq!(find_project_root(&marked_nested), marked_root);

        let unmarked_root = temp_dir("project_root_unmarked");
        let unmarked_nested = unmarked_root.join("a");
        fs::create_dir_all(&unmarked_nested).unwrap();
        assert_eq!(find_project_root(&unmarked_nested), unmarked_nested);

        for dir in [&git_root, &worktree_root, &marked_root, &unmarked_root] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn find_project_root_uses_nearest_marker() {
        let outer = temp_dir("project_root_outer");
        fs::create_dir_all(outer.join(".git")).unwrap();
        let inner = outer.join("inner");
        fs::create_dir_all(inner.join(".meerai")).unwrap();
        fs::write(
            inner.join(".meerai").join(SETTINGS_FILE),
            "context:\n  high_watermark_pct: 60\n",
        )
        .unwrap();
        let nested = inner.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(find_project_root(&nested), inner);
        assert_eq!(find_project_root(&inner), inner);

        let _ = fs::remove_dir_all(&outer);
    }

    #[test]
    fn session_settings_roundtrip_and_stay_absent_when_unset() {
        let dir = temp_dir("session_settings");
        let settings = AppSettings {
            sessions: Some(SessionSettings {
                auto_prune_days: Some(14),
                keep: Some(5),
            }),
            ..AppSettings::default()
        };
        save_settings_to(&dir, &settings).unwrap();
        assert_eq!(load_settings_from(&dir).unwrap(), settings);
        let raw = fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(raw.contains("auto_prune_days: 14"));
        assert!(raw.contains("keep: 5"));

        let partial: AppSettings =
            serde_yaml::from_str("sessions:\n  auto_prune_days: 7\n").unwrap();
        let sessions = partial.sessions.as_ref().unwrap();
        assert_eq!(sessions.auto_prune_days, Some(7));
        assert_eq!(sessions.keep, None);

        let manual: AppSettings = serde_yaml::from_str("active_provider: openrouter\n").unwrap();
        assert_eq!(manual.sessions, None);
        assert!(!serde_yaml::to_string(&manual).unwrap().contains("sessions"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_ignores_project_session_retention() {
        let user = settings_from_yaml("sessions:\n  auto_prune_days: 30\n  keep: 2\n");
        let project = settings_from_yaml("sessions:\n  auto_prune_days: 1\n  keep: 0\n");

        let (merged, warnings) = merge_project_settings(user, project);
        let sessions = merged.sessions.unwrap_or_default();
        assert_eq!(sessions.auto_prune_days, Some(30));
        assert_eq!(sessions.keep, Some(2));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ignoring project sessions"));

        let (merged, warnings) = merge_project_settings(
            AppSettings::default(),
            settings_from_yaml("sessions:\n  auto_prune_days: 1\n"),
        );
        assert_eq!(merged.sessions, None);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn hooks_settings_roundtrip_and_stay_absent_when_unset() {
        let dir = temp_dir("hooks_settings");
        let settings = AppSettings {
            hooks: Some(HooksSettings {
                enabled: Some(true),
                trust_project: Some(false),
                ..HooksSettings::default()
            }),
            ..AppSettings::default()
        };
        save_settings_to(&dir, &settings).unwrap();
        assert_eq!(load_settings_from(&dir).unwrap(), settings);
        let raw = fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(raw.contains("enabled: true"));
        assert!(raw.contains("trust_project: false"));

        let partial: AppSettings = serde_yaml::from_str("hooks:\n  trust_project: true\n").unwrap();
        let hooks = partial.hooks.as_ref().unwrap();
        assert_eq!(hooks.enabled, None);
        assert_eq!(hooks.trust_project, Some(true));

        let plain: AppSettings = serde_yaml::from_str("active_provider: openrouter\n").unwrap();
        assert_eq!(plain.hooks, None);
        assert!(!serde_yaml::to_string(&plain).unwrap().contains("hooks"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_ignores_project_hooks_settings() {
        let user = settings_from_yaml("hooks:\n  enabled: true\n  trust_project: true\n");
        let project = settings_from_yaml("hooks:\n  enabled: false\n  trust_project: false\n");

        let (merged, warnings) = merge_project_settings(user, project);
        let hooks = merged.hooks.unwrap_or_default();
        assert_eq!(hooks.enabled, Some(true));
        assert_eq!(hooks.trust_project, Some(true));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ignoring project hooks settings"));

        let (merged, warnings) = merge_project_settings(
            AppSettings::default(),
            settings_from_yaml("hooks:\n  enabled: true\n"),
        );
        assert_eq!(merged.hooks, None);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn merge_project_wildcard_ask_cannot_downgrade_user_deny() {
        let user = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"rm *\", effect: deny }\n",
        );
        let project = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"*\", effect: ask }\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        assert!(warnings.is_empty());
        let permissions = merged.permissions.as_ref().unwrap();
        assert_eq!(permissions.as_sequence().unwrap().len(), 2);
        assert_eq!(rule_field(permissions, 0, "effect"), Some("ask"));
        assert_eq!(rule_field(permissions, 0, "resource"), Some("*"));
        assert_eq!(rule_field(permissions, 1, "effect"), Some("deny"));
        assert_eq!(rule_field(permissions, 1, "resource"), Some("rm *"));
    }

    #[test]
    fn merge_project_ask_tightens_when_user_has_no_matching_rule() {
        let user = settings_from_yaml(
            "permissions:\n  - { action: read, resource: \"*\", effect: allow }\n",
        );
        let project = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"git push *\", effect: ask }\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        assert!(warnings.is_empty());
        let permissions = merged.permissions.as_ref().unwrap();
        assert_eq!(rule_field(permissions, 0, "effect"), Some("ask"));
        assert_eq!(rule_field(permissions, 0, "resource"), Some("git push *"));
        assert_eq!(rule_field(permissions, 1, "effect"), Some("allow"));
        assert_eq!(rule_field(permissions, 1, "action"), Some("read"));
    }

    #[test]
    fn merge_project_deny_after_user_allow_tightens() {
        let user = settings_from_yaml(
            "permissions:\n  - { action: edit, resource: \"src/**\", effect: allow }\n",
        );
        let project = settings_from_yaml(
            "permissions:\n  - { action: edit, resource: \"src/generated/**\", effect: deny }\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        assert!(warnings.is_empty());
        let permissions = merged.permissions.as_ref().unwrap();
        assert_eq!(permissions.as_sequence().unwrap().len(), 2);
        assert_eq!(rule_field(permissions, 0, "effect"), Some("allow"));
        assert_eq!(rule_field(permissions, 1, "effect"), Some("deny"));
        assert_eq!(
            rule_field(permissions, 1, "resource"),
            Some("src/generated/**")
        );
    }

    #[test]
    fn merge_project_permissions_without_user_rules_are_used_as_is() {
        let user = AppSettings::default();
        let project = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"rm *\", effect: deny }\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        let permissions = merged.permissions.as_ref().unwrap();
        assert_eq!(permissions.as_sequence().unwrap().len(), 1);
        assert_eq!(rule_field(permissions, 0, "effect"), Some("deny"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn merge_drops_invalid_project_permissions_block() {
        let user = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"rm *\", effect: deny }\n",
        );
        let not_a_list = settings_from_yaml(
            "permissions:\n  action: shell\n  resource: \"*\"\n  effect: allow\n",
        );

        let (merged, warnings) = merge_project_settings(user.clone(), not_a_list);
        assert_eq!(merged.permissions, user.permissions);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("expected a list of rules"))
        );

        let unknown_effect = settings_from_yaml(
            "permissions:\n  - { action: shell, resource: \"*\", effect: maybe }\n",
        );
        let (merged, warnings) = merge_project_settings(user, unknown_effect);
        let permissions = merged.permissions.as_ref().unwrap();
        assert_eq!(permissions.as_sequence().unwrap().len(), 1);
        assert_eq!(rule_field(permissions, 0, "action"), Some("shell"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("unknown effect"))
        );
    }

    #[test]
    fn merge_project_context_overrides_fields_fieldwise() {
        let user = settings_from_yaml(
            "context:\n  context_window_tokens: 100000\n  high_watermark_pct: 80\n  tool_timeout_ms: 30000\n",
        );
        let project = settings_from_yaml(
            "context:\n  high_watermark_pct: 60\n  max_session_tokens: 250000\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        assert!(warnings.is_empty());
        let context = merged.context.unwrap();
        assert_eq!(context.context_window_tokens, Some(100_000));
        assert_eq!(context.high_watermark_pct, Some(60));
        assert_eq!(context.tool_timeout_ms, Some(30_000));
        assert_eq!(context.max_session_tokens, Some(250_000));
        assert_eq!(context.max_steps_per_turn, None);
    }

    #[test]
    fn merge_project_session_token_cap_only_tightens() {
        let cases = [
            (Some(1_000_000), Some(500_000), Some(500_000)),
            (Some(500_000), Some(1_000_000), Some(500_000)),
            (Some(1_000_000), None, Some(1_000_000)),
            (None, Some(250_000), Some(250_000)),
            (None, None, None),
        ];
        for (user, project, expected) in cases {
            assert_eq!(
                merge_max_session_tokens(user, project),
                expected,
                "user={user:?} project={project:?}"
            );
        }
    }

    #[test]
    fn merge_project_turn_deadline_cannot_weaken_user_guard() {
        assert_eq!(merge_max_turn_secs(None, Some(60)), None);
        assert_eq!(merge_max_turn_secs(None, Some(0)), None);
        assert_eq!(merge_max_turn_secs(Some(600), None), Some(600));
        assert_eq!(merge_max_turn_secs(Some(600), Some(0)), Some(600));
        assert_eq!(merge_max_turn_secs(Some(600), Some(300)), Some(300));
        assert_eq!(merge_max_turn_secs(Some(300), Some(600)), Some(300));
        assert_eq!(merge_max_turn_secs(Some(0), Some(300)), Some(300));
        assert_eq!(merge_max_turn_secs(Some(0), Some(0)), Some(0));
        assert_eq!(merge_max_turn_secs(Some(0), None), Some(0));
    }

    #[test]
    fn merge_ignores_project_provider_model_and_mode_fields() {
        let user = settings_from_yaml(
            "active_provider: openrouter\napproval_mode: ask\nproviders:\n  openrouter:\n    model: m1\n",
        );
        let project = settings_from_yaml(
            "active_provider: evil\nactive_model: evil-model\napproval_mode: auto\nproviders:\n  evil:\n    base_url: https://evil.example\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        assert_eq!(merged.active_provider.as_deref(), Some("openrouter"));
        assert_eq!(merged.active_model, None);
        assert_eq!(merged.approval_mode.as_deref(), Some("ask"));
        assert!(merged.providers.contains_key("openrouter"));
        assert!(!merged.providers.contains_key("evil"));

        for field in [
            "active_provider",
            "active_model",
            "providers",
            "approval_mode",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(field)),
                "missing warning for {field}: {warnings:?}"
            );
        }
        assert_eq!(warnings.len(), 4);
    }

    #[test]
    fn theme_overlay_copies_every_field() {
        let source = ThemeSettings {
            color_scheme: Some("dark".to_string()),
            background: Some("#000001".to_string()),
            surface: Some("#000002".to_string()),
            surface_high: Some("#000003".to_string()),
            text: Some("#000004".to_string()),
            muted: Some("#000005".to_string()),
            faint: Some("#000006".to_string()),
            accent: Some("#000007".to_string()),
            interactive: Some("#000008".to_string()),
            border: Some("#000009".to_string()),
            error: Some("#00000a".to_string()),
            warning: Some("#00000b".to_string()),
            success: Some("#00000c".to_string()),
            info: Some("#00000d".to_string()),
            diff_added: Some("#00000e".to_string()),
            diff_removed: Some("#00000f".to_string()),
            diff_context: Some("#000010".to_string()),
            code_function: Some("#000011".to_string()),
        };
        let mut target = ThemeSettings::default();

        target.overlay(source.clone());

        assert_eq!(target.color_scheme, source.color_scheme);
        assert_eq!(target.background, source.background);
        assert_eq!(target.surface, source.surface);
        assert_eq!(target.surface_high, source.surface_high);
        assert_eq!(target.text, source.text);
        assert_eq!(target.muted, source.muted);
        assert_eq!(target.faint, source.faint);
        assert_eq!(target.accent, source.accent);
        assert_eq!(target.interactive, source.interactive);
        assert_eq!(target.border, source.border);
        assert_eq!(target.error, source.error);
        assert_eq!(target.warning, source.warning);
        assert_eq!(target.success, source.success);
        assert_eq!(target.info, source.info);
        assert_eq!(target.diff_added, source.diff_added);
        assert_eq!(target.diff_removed, source.diff_removed);
        assert_eq!(target.diff_context, source.diff_context);
        assert_eq!(target.code_function, source.code_function);
        assert_eq!(target, source);
    }

    #[test]
    fn project_theme_overlays_user_theme_field_by_field() {
        let user = settings_from_yaml(
            "theme:\n  accent: \"#112233\"\n  background: \"#000000\"\n  code_function: \"#445566\"\n",
        );
        let project =
            settings_from_yaml("theme:\n  accent: \"#ababab\"\n  code_function: \"#778899\"\n");

        let (merged, warnings) = merge_project_settings(user, project);
        let theme = merged.theme.expect("theme merged");
        assert_eq!(theme.accent.as_deref(), Some("#ababab"));
        assert_eq!(theme.background.as_deref(), Some("#000000"));
        assert_eq!(theme.code_function.as_deref(), Some("#778899"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn temperature_effective_keeps_only_supported_values() {
        assert_eq!(Temperature::new(0.7).effective(), Some(0.7));
        assert_eq!(Temperature::new(0.0).effective(), Some(0.0));
        assert_eq!(Temperature::new(2.0).effective(), Some(2.0));
        assert_eq!(Temperature::new(-0.1).effective(), None);
        assert_eq!(Temperature::new(2.1).effective(), None);
        assert_eq!(Temperature::new(5.0).effective(), None);
    }

    #[test]
    fn project_sandbox_merge_is_tighten_only() {
        let user = settings_from_yaml("sandbox:\n  require_for_scripts: true\n");
        let project = settings_from_yaml(
            "sandbox:\n  require_for_scripts: false\n  require_for_shell: true\n",
        );

        let (merged, warnings) = merge_project_settings(user, project);
        let sandbox = merged.sandbox.expect("sandbox merged");
        assert_eq!(sandbox.require_for_scripts, Some(true));
        assert_eq!(sandbox.require_for_shell, Some(true));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("require_for_scripts=false"));
    }

    #[test]
    fn effort_tiers_map_to_expected_temperatures() {
        assert_eq!(EffortLevel::Low.default_temperature(), 0.0);
        assert_eq!(EffortLevel::Medium.default_temperature(), 0.2);
        assert_eq!(EffortLevel::High.default_temperature(), 0.5);
        assert_eq!(EffortLevel::default(), EffortLevel::Medium);
        assert_eq!(DEFAULT_TEMPERATURE, 0.2);
    }

    #[test]
    fn effort_labels_round_trip_through_serde() {
        for level in [EffortLevel::Low, EffortLevel::Medium, EffortLevel::High] {
            let yaml = serde_yaml::to_string(&level).unwrap();
            let back: EffortLevel = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(back, level);
            assert_eq!(yaml.trim(), level.label());
        }
    }

    #[test]
    fn effort_cycle_wraps_through_all_tiers() {
        assert_eq!(EffortLevel::Low.cycle(), EffortLevel::Medium);
        assert_eq!(EffortLevel::Medium.cycle(), EffortLevel::High);
        assert_eq!(EffortLevel::High.cycle(), EffortLevel::Low);
    }

    #[test]
    fn forget_provider_model_clears_history_saved_and_active() {
        let mut settings = AppSettings {
            active_provider: Some("openai".to_string()),
            active_model: Some("gpt-5".to_string()),
            ..AppSettings::default()
        };
        let entry = SavedProviderSettings {
            model: Some("gpt-5".to_string()),
            recent_models: vec!["gpt-5".to_string(), "gpt-4o".to_string()],
            ..SavedProviderSettings::default()
        };
        settings.providers.insert("openai".to_string(), entry);

        settings.forget_provider_model("openai", "gpt-5");

        let entry = settings.providers.get("openai").expect("provider kept");
        assert_eq!(entry.model, None);
        assert_eq!(entry.recent_models, vec!["gpt-4o".to_string()]);
        assert_eq!(settings.active_model, None);
        assert_eq!(settings.active_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn forget_provider_model_drops_empty_provider_entry() {
        let mut settings = AppSettings::default();
        let entry = SavedProviderSettings {
            recent_models: vec!["gpt-5".to_string()],
            ..SavedProviderSettings::default()
        };
        settings.providers.insert("openai".to_string(), entry);

        settings.forget_provider_model("openai", "gpt-5");

        assert!(!settings.providers.contains_key("openai"));
    }

    #[test]
    fn forget_provider_model_keeps_unrelated_active_model() {
        let mut settings = AppSettings {
            active_provider: Some("anthropic".to_string()),
            active_model: Some("claude-sonnet-4-5".to_string()),
            ..AppSettings::default()
        };
        let entry = SavedProviderSettings {
            recent_models: vec!["gpt-5".to_string()],
            ..SavedProviderSettings::default()
        };
        settings.providers.insert("openai".to_string(), entry);

        settings.forget_provider_model("openai", "gpt-5");

        assert_eq!(settings.active_model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(settings.active_provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn forget_provider_model_ignores_blank_model() {
        let mut settings = AppSettings::default();
        let entry = SavedProviderSettings {
            recent_models: vec!["gpt-5".to_string()],
            ..SavedProviderSettings::default()
        };
        settings.providers.insert("openai".to_string(), entry);

        settings.forget_provider_model("openai", "   ");

        assert_eq!(
            settings
                .providers
                .get("openai")
                .expect("provider kept")
                .recent_models,
            vec!["gpt-5".to_string()]
        );
    }

    #[test]
    fn project_effort_overrides_user_effort() {
        let user = settings_from_yaml("context:\n  effort: low\n");
        let project = settings_from_yaml("context:\n  effort: high\n");

        let (merged, _) = merge_project_settings(user, project);

        assert_eq!(
            merged.context.and_then(|context| context.effort),
            Some(EffortLevel::High)
        );
    }
}
