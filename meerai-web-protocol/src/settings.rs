use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeTokensDto {
    pub background: String,
    pub surface: String,
    pub surface_high: String,
    pub text: String,
    pub muted: String,
    pub faint: String,
    pub accent: String,
    pub interactive: String,
    pub border: String,
    pub error: String,
    pub warning: String,
    pub success: String,
    pub info: String,
    pub diff_added: String,
    pub diff_removed: String,
    pub diff_context: String,
    pub code_function: String,
}
impl Default for ThemeTokensDto {
    fn default() -> Self {
        Self {
            background: "#0b0d10".to_owned(),
            surface: "#12151a".to_owned(),
            surface_high: "#1a1f27".to_owned(),
            text: "#eceef2".to_owned(),
            muted: "#9aa3b2".to_owned(),
            faint: "#6b7485".to_owned(),
            accent: "#2dd4bf".to_owned(),
            interactive: "#60a5fa".to_owned(),
            border: "#262c36".to_owned(),
            error: "#f07178".to_owned(),
            warning: "#f5a962".to_owned(),
            success: "#6ee7a0".to_owned(),
            info: "#56c2d6".to_owned(),
            diff_added: "#4fd6be".to_owned(),
            diff_removed: "#e06c75".to_owned(),
            diff_context: "#8892a0".to_owned(),
            code_function: "#c792ea".to_owned(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeOverrideDto {
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSettingsDto {
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
    pub no_progress_fail_steps: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_limit_continuations: Option<usize>,
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
    pub auto_memory: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_prompt_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_levels: Vec<EffortDto>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortDto {
    Low,
    Medium,
    High,
    ExtraHigh,
    Max,
}
impl EffortDto {
    pub const ALL: [Self; 5] = [
        Self::Low,
        Self::Medium,
        Self::High,
        Self::ExtraHigh,
        Self::Max,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" | "minimal" => Some(Self::Low),
            "medium" | "mid" => Some(Self::Medium),
            "high" => Some(Self::High),
            "extrahigh" | "extra-high" | "xhigh" => Some(Self::ExtraHigh),
            "max" | "maximum" => Some(Self::Max),
            _ => None,
        }
    }

    pub fn keyword(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::ExtraHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::ExtraHigh => 3,
            Self::Max => 4,
        }
    }

    pub fn clamp_to(self, allowed: &[Self]) -> Self {
        if allowed.is_empty() || allowed.contains(&self) {
            return self;
        }
        allowed
            .iter()
            .copied()
            .filter(|level| level.rank() <= self.rank())
            .max_by_key(|level| level.rank())
            .unwrap_or(allowed[0])
    }

    pub fn next_in(self, allowed: &[Self]) -> Self {
        let allowed = if allowed.is_empty() {
            &Self::ALL[..]
        } else {
            allowed
        };
        let current = self.clamp_to(allowed);
        match allowed.iter().position(|level| *level == current) {
            Some(index) => allowed[(index + 1) % allowed.len()],
            None => current,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_prune_days: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<usize>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_for_scripts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_for_shell: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_env: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshotDto {
    pub theme: ThemeTokensDto,
    #[serde(default)]
    pub raw_theme: ThemeOverrideDto,
    #[serde(default)]
    pub context: ContextSettingsDto,
    #[serde(default)]
    pub session: SessionSettingsDto,
    #[serde(default)]
    pub sandbox: SandboxSettingsDto,
    #[serde(default)]
    pub permissions_summary: Vec<String>,
    #[serde(default)]
    pub config_writable: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    #[serde(default)]
    pub reset_theme: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeOverrideDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forget_model: Option<ForgetModelDto>,
}
