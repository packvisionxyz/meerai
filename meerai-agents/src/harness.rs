use std::sync::Arc;

use meerai_common::settings::{
    ContextSettings, DEFAULT_TEMPERATURE, EffortLevel, RoutingSettings, TaskType, Temperature,
};
use meerai_policy::PolicyEngine;
use meerai_session::Session;

use crate::routing::RoutingCoordinator;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HarnessConfig {
    pub context_window_tokens: u64,
    pub high_watermark_pct: u8,
    pub spill_max_inline_tokens: u64,
    pub verbatim_window_turns: usize,
    pub reasoning_pruning: bool,
    pub prompt_caching: bool,
    pub plan_usage: bool,
    pub rollback_on_verify_failure: bool,
    pub recovery_model: Option<String>,
    pub max_steps_per_turn: usize,
    pub no_progress_fail_steps: usize,
    pub step_limit_continuations: usize,
    pub auto_memory: bool,
    pub tool_timeout_ms: u64,
    pub max_session_tokens: Option<u64>,
    pub max_turn_secs: u64,
    pub summarizer_model: Option<String>,
    pub temperature: Option<f32>,
    pub effort: Option<EffortLevel>,
    pub effort_levels: Option<Vec<EffortLevel>>,
    pub token_pricing: Option<TokenPricing>,
    pub routing: RoutingSettings,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            context_window_tokens: 128_000,
            high_watermark_pct: 75,
            spill_max_inline_tokens: 12_500,
            verbatim_window_turns: 4,
            reasoning_pruning: true,
            prompt_caching: true,
            plan_usage: false,
            rollback_on_verify_failure: false,
            recovery_model: None,
            max_steps_per_turn: 0,
            no_progress_fail_steps: 12,
            step_limit_continuations: 2,
            auto_memory: true,
            tool_timeout_ms: 120_000,
            max_session_tokens: None,
            max_turn_secs: 1_800,
            summarizer_model: None,
            temperature: Some(DEFAULT_TEMPERATURE),
            effort: Some(EffortLevel::Medium),
            effort_levels: None,
            token_pricing: None,
            routing: RoutingSettings::default(),
        }
    }
}

impl HarnessConfig {
    pub fn from_context_settings(context: Option<&ContextSettings>) -> Self {
        let mut cfg = Self::default();
        let Some(context) = context else {
            return cfg;
        };
        if let Some(value) = context.context_window_tokens {
            cfg.context_window_tokens = value;
        }
        if let Some(value) = context.high_watermark_pct {
            cfg.high_watermark_pct = value;
        }
        if let Some(value) = context.spill_max_inline_tokens {
            cfg.spill_max_inline_tokens = value;
        }
        if let Some(value) = context.verbatim_window_turns {
            cfg.verbatim_window_turns = value;
        }
        if let Some(value) = context.reasoning_pruning {
            cfg.reasoning_pruning = value;
        }
        if let Some(value) = context.prompt_caching {
            cfg.prompt_caching = value;
        }
        if let Some(value) = context.rollback_on_verify_failure {
            cfg.rollback_on_verify_failure = value;
        }
        if let Some(value) = &context.recovery_model {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                cfg.recovery_model = Some(trimmed.to_string());
            }
        }
        if let Some(value) = context.no_progress_fail_steps {
            // The diagnostic notice must still fire before the turn fails.
            cfg.no_progress_fail_steps = value.max(3);
        }
        if let Some(value) = context.max_steps_per_turn {
            cfg.max_steps_per_turn = value;
        }
        if let Some(value) = context.step_limit_continuations {
            cfg.step_limit_continuations = value;
        }
        if let Some(value) = context.auto_memory {
            cfg.auto_memory = value;
        }
        if let Some(value) = context.tool_timeout_ms {
            cfg.tool_timeout_ms = value;
        }
        if let Some(value) = context.max_session_tokens {
            cfg.max_session_tokens = Some(value);
        }
        if let Some(value) = context.max_turn_secs {
            cfg.max_turn_secs = value;
        }
        if let Some(value) = &context.summarizer_model {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                cfg.summarizer_model = Some(trimmed.to_string());
            }
        }
        let effort = context.effort.unwrap_or_default();
        cfg.effort = Some(effort);
        cfg.temperature = context
            .temperature
            .and_then(Temperature::effective)
            .or_else(|| Temperature::new(effort.default_temperature()).effective());
        cfg
    }

    pub fn with_routing(mut self, routing: RoutingSettings) -> Self {
        self.routing = routing;
        self
    }

    pub fn route_model(&self, task: &TaskType, fallback: &str) -> String {
        crate::routing::route_model(&self.routing, task, fallback)
    }
}

pub struct AgentHarness {
    pub session: Session,
    pub policy: PolicyEngine,
    pub config: HarnessConfig,
    pub routing: Option<Arc<RoutingCoordinator>>,
}

impl AgentHarness {
    pub fn new(session: Session, policy: PolicyEngine) -> Self {
        Self {
            session,
            policy,
            config: HarnessConfig::default(),
            routing: None,
        }
    }
}

impl std::fmt::Debug for AgentHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHarness")
            .field("session", self.session.id())
            .field("policy", &self.policy)
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_common::settings::Temperature;

    #[test]
    fn context_temperature_flows_into_the_harness() {
        let config = HarnessConfig::from_context_settings(Some(&ContextSettings {
            temperature: Some(Temperature::new(0.7)),
            ..ContextSettings::default()
        }));
        assert_eq!(config.temperature, Some(0.7));

        let config = HarnessConfig::from_context_settings(Some(&ContextSettings {
            temperature: Some(Temperature::new(5.0)),
            ..ContextSettings::default()
        }));
        assert_eq!(config.temperature, Some(DEFAULT_TEMPERATURE));
        assert_eq!(config.effort, Some(EffortLevel::Medium));

        let high = HarnessConfig::from_context_settings(Some(&ContextSettings {
            effort: Some(EffortLevel::High),
            ..ContextSettings::default()
        }));
        assert_eq!(high.effort, Some(EffortLevel::High));
        assert_eq!(high.temperature, Some(0.5));
    }

    #[test]
    fn step_budget_defaults_to_unlimited_and_caps_flow_from_settings() {
        assert_eq!(HarnessConfig::default().max_steps_per_turn, 0);
        assert_eq!(HarnessConfig::default().step_limit_continuations, 2);

        let capped = HarnessConfig::from_context_settings(Some(&ContextSettings {
            max_steps_per_turn: Some(7),
            ..ContextSettings::default()
        }));
        assert_eq!(capped.max_steps_per_turn, 7);

        let unlimited = HarnessConfig::from_context_settings(Some(&ContextSettings {
            max_steps_per_turn: Some(0),
            ..ContextSettings::default()
        }));
        assert_eq!(unlimited.max_steps_per_turn, 0);

        let no_continuations = HarnessConfig::from_context_settings(Some(&ContextSettings {
            step_limit_continuations: Some(0),
            ..ContextSettings::default()
        }));
        assert_eq!(no_continuations.step_limit_continuations, 0);
    }

    #[test]
    fn plan_usage_defaults_off() {
        assert!(!HarnessConfig::default().plan_usage);
        assert!(!HarnessConfig::from_context_settings(None).plan_usage);
    }

    #[test]
    fn no_progress_limit_flows_from_context_settings() {
        assert_eq!(HarnessConfig::default().no_progress_fail_steps, 12);
        let config = HarnessConfig::from_context_settings(Some(&ContextSettings {
            no_progress_fail_steps: Some(30),
            ..ContextSettings::default()
        }));
        assert_eq!(config.no_progress_fail_steps, 30);
    }

    #[test]
    fn no_progress_limit_keeps_room_for_the_diagnostic() {
        for requested in [0, 1, 2] {
            let config = HarnessConfig::from_context_settings(Some(&ContextSettings {
                no_progress_fail_steps: Some(requested),
                ..ContextSettings::default()
            }));
            assert_eq!(config.no_progress_fail_steps, 3, "requested {requested}");
        }
    }
}
