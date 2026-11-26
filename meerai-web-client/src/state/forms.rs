use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Connections,
    Session,
    Theme,
    Context,
    Sessions,
    Sandbox,
    Permissions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModalReturn {
    Close,
    ProviderSelect,
    SettingsConnections,
}

#[derive(Clone, Debug)]
pub struct SettingsFormState {
    pub section: SettingsSection,
    pub provider_filter: TextBuffer,
    pub reset_theme: bool,
    pub color_scheme: Option<String>,
    pub theme_fields: BTreeMap<&'static str, TextBuffer>,
    pub inputs: BTreeMap<&'static str, TextBuffer>,
    pub reasoning_pruning: bool,
    pub prompt_caching: bool,
    pub rollback_on_verify_failure: bool,
    pub post_edit_diagnostics: bool,
    pub effort: Option<EffortDto>,
    pub effort_levels: Vec<EffortDto>,
    pub require_for_scripts: bool,
    pub require_for_shell: bool,
    pub strict_env: bool,
    pub status: Option<String>,
    pub busy: bool,
}

impl SettingsFormState {
    pub fn from_host(info: &HostInfoDto) -> Self {
        let s = &info.settings;
        let mut theme_fields = BTreeMap::new();
        let raw = &s.raw_theme;
        let pairs: [(&'static str, &Option<String>); 17] = [
            ("background", &raw.background),
            ("surface", &raw.surface),
            ("surface_high", &raw.surface_high),
            ("text", &raw.text),
            ("muted", &raw.muted),
            ("faint", &raw.faint),
            ("accent", &raw.accent),
            ("interactive", &raw.interactive),
            ("border", &raw.border),
            ("error", &raw.error),
            ("warning", &raw.warning),
            ("success", &raw.success),
            ("info", &raw.info),
            ("diff_added", &raw.diff_added),
            ("diff_removed", &raw.diff_removed),
            ("diff_context", &raw.diff_context),
            ("code_function", &raw.code_function),
        ];
        for (k, v) in pairs {
            theme_fields.insert(k, TextBuffer::new(v.clone().unwrap_or_default()));
        }
        let c = &s.context;
        let opt_u64 = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_default();
        let opt_usize = |v: Option<usize>| v.map(|n| n.to_string()).unwrap_or_default();
        let mut inputs = BTreeMap::new();
        for (k, val) in [
            ("context_window", opt_u64(c.context_window_tokens)),
            (
                "high_watermark_pct",
                c.high_watermark_pct
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            ),
            ("spill_max_inline", opt_u64(c.spill_max_inline_tokens)),
            ("verbatim_window_turns", opt_usize(c.verbatim_window_turns)),
            ("max_steps_per_turn", opt_usize(c.max_steps_per_turn)),
            (
                "no_progress_fail_steps",
                opt_usize(c.no_progress_fail_steps),
            ),
            ("tool_timeout_ms", opt_u64(c.tool_timeout_ms)),
            ("max_session_tokens", opt_u64(c.max_session_tokens)),
            ("max_turn_secs", opt_u64(c.max_turn_secs)),
            (
                "recovery_model",
                c.recovery_model.clone().unwrap_or_default(),
            ),
            (
                "summarizer_model",
                c.summarizer_model.clone().unwrap_or_default(),
            ),
            (
                "temperature",
                c.temperature.map(|f| format!("{f}")).unwrap_or_default(),
            ),
            ("auto_prune_days", opt_u64(s.session.auto_prune_days)),
            ("session_keep", opt_usize(s.session.keep)),
        ] {
            inputs.insert(k, TextBuffer::new(val));
        }
        Self {
            section: SettingsSection::Connections,
            provider_filter: TextBuffer::new(""),
            reset_theme: false,
            color_scheme: raw.color_scheme.clone(),
            theme_fields,
            inputs,
            reasoning_pruning: c.reasoning_pruning.unwrap_or(true),
            prompt_caching: c.prompt_caching.unwrap_or(true),
            rollback_on_verify_failure: c.rollback_on_verify_failure.unwrap_or(false),
            post_edit_diagnostics: c.post_edit_diagnostics.unwrap_or(false),
            effort: c.effort,
            effort_levels: c.effort_levels.clone(),
            require_for_scripts: s.sandbox.require_for_scripts.unwrap_or(false),
            require_for_shell: s.sandbox.require_for_shell.unwrap_or(false),
            strict_env: s.sandbox.strict_env.unwrap_or(false),
            status: None,
            busy: false,
        }
    }

    pub fn field_text(&self, key: &str) -> &str {
        self.inputs.get(key).map(|e| e.text.as_str()).unwrap_or("")
    }

    pub fn to_request(&self) -> Result<UpdateSettingsRequest, String> {
        let parse_u64 = |label: &str, key: &str| -> Result<Option<u64>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("Invalid {label}: expected positive integer"))
            }
        };
        let parse_usize = |label: &str, key: &str| -> Result<Option<usize>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<usize>()
                    .map(Some)
                    .map_err(|_| format!("Invalid {label}: expected integer"))
            }
        };
        let parse_pct = |key: &str| -> Result<Option<u8>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                let v = t
                    .parse::<u8>()
                    .map_err(|_| "Invalid high watermark %".to_owned())?;
                if v == 0 || v > 100 {
                    return Err("High watermark % must be 1..=100".to_owned());
                }
                Ok(Some(v))
            }
        };
        let parse_temp = |key: &str| -> Result<Option<f32>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                let v = t
                    .parse::<f32>()
                    .map_err(|_| "Invalid temperature (0.0..=2.0)".to_owned())?;
                if !(0.0..=2.0).contains(&v) {
                    return Err("Temperature must be between 0.0 and 2.0".to_owned());
                }
                Ok(Some(v))
            }
        };

        let opt_color = |key: &str| -> Result<Option<String>, String> {
            let v = self
                .theme_fields
                .get(key)
                .map(|e| e.text.trim().to_owned())
                .unwrap_or_default();
            if v.is_empty() {
                return Ok(None);
            }
            if !crate::theme::is_valid_color(&v) {
                return Err(format!(
                    "Invalid color for {key}: {v} (use #rgb, #rrggbb, or a CSS color name)"
                ));
            }
            Ok(Some(v))
        };

        let theme = if self.reset_theme {
            None
        } else {
            Some(ThemeOverrideDto {
                color_scheme: self.color_scheme.clone(),
                background: opt_color("background")?,
                surface: opt_color("surface")?,
                surface_high: opt_color("surface_high")?,
                text: opt_color("text")?,
                muted: opt_color("muted")?,
                faint: opt_color("faint")?,
                accent: opt_color("accent")?,
                interactive: opt_color("interactive")?,
                border: opt_color("border")?,
                error: opt_color("error")?,
                warning: opt_color("warning")?,
                success: opt_color("success")?,
                info: opt_color("info")?,
                diff_added: opt_color("diff_added")?,
                diff_removed: opt_color("diff_removed")?,
                diff_context: opt_color("diff_context")?,
                code_function: opt_color("code_function")?,
            })
        };

        let opt_str = |key: &str| {
            let t = self.field_text(key).trim();
            (!t.is_empty()).then(|| t.to_owned())
        };

        Ok(UpdateSettingsRequest {
            reset_theme: self.reset_theme,
            theme,
            context: Some(ContextSettingsDto {
                context_window_tokens: parse_u64("context window", "context_window")?,
                high_watermark_pct: parse_pct("high_watermark_pct")?,
                spill_max_inline_tokens: parse_u64("spill threshold", "spill_max_inline")?,
                verbatim_window_turns: parse_usize("verbatim turns", "verbatim_window_turns")?,
                reasoning_pruning: Some(self.reasoning_pruning),
                prompt_caching: Some(self.prompt_caching),
                rollback_on_verify_failure: Some(self.rollback_on_verify_failure),
                recovery_model: opt_str("recovery_model"),
                max_steps_per_turn: parse_usize("max steps/turn", "max_steps_per_turn")?,
                no_progress_fail_steps: parse_usize("no-progress steps", "no_progress_fail_steps")?,
                step_limit_continuations: None,
                tool_timeout_ms: parse_u64("tool timeout ms", "tool_timeout_ms")?,
                max_session_tokens: parse_u64("max session tokens", "max_session_tokens")?,
                max_turn_secs: parse_u64("max turn secs", "max_turn_secs")?,
                summarizer_model: opt_str("summarizer_model"),
                post_edit_diagnostics: Some(self.post_edit_diagnostics),
                auto_memory: None,
                memory_prompt_bytes: None,
                temperature: parse_temp("temperature")?,
                effort: self.effort,
                effort_levels: Vec::new(),
            }),
            forget_model: None,
            session: Some(SessionSettingsDto {
                auto_prune_days: parse_u64("auto prune days", "auto_prune_days")?,
                keep: parse_usize("keep count", "session_keep")?,
            }),
            sandbox: Some(SandboxSettingsDto {
                require_for_scripts: Some(self.require_for_scripts),
                require_for_shell: Some(self.require_for_shell),
                strict_env: Some(self.strict_env),
            }),
        })
    }
}

#[derive(Clone, Debug)]
pub struct QuestionDraft {
    pub answers: Vec<Vec<String>>,
    pub custom_inputs: Vec<TextBuffer>,
    pub step: usize,
    pub focused_option: usize,
}

impl QuestionDraft {
    pub fn from_request(req: &QuestionRequestDto) -> Self {
        let len = req.questions.len();
        Self {
            answers: vec![Vec::new(); len],
            custom_inputs: vec![TextBuffer::new(""); len],
            step: 0,
            focused_option: 0,
        }
    }
}
