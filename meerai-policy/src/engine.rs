use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use meerai_core::{ApprovalPreview, EffectClass, GrantScope};
use serde::{Deserialize, Serialize};

use crate::matcher::{Candidate, ResourcePattern};
use crate::rule::{ActionSelector, Effect, PermissionAction, PermissionRule};

const DENY_REASON: &str = "denied by permission rule";
const PLAN_READ_ONLY_REASON: &str = "plan mode is read-only";
const APPROVAL_TITLE_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Plan,
    #[default]
    Ask,
    Auto,
}

impl ApprovalMode {
    pub fn parse(raw: &str) -> Option<Self> {
        parse_approval_mode(raw)
    }

    pub fn label(self) -> &'static str {
        approval_mode_label(self)
    }

    pub fn badge(self) -> &'static str {
        approval_mode_badge(self)
    }
}

pub fn parse_approval_mode(raw: &str) -> Option<ApprovalMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "plan" => Some(ApprovalMode::Plan),
        "ask" => Some(ApprovalMode::Ask),
        "auto" => Some(ApprovalMode::Auto),
        _ => None,
    }
}

pub fn approval_mode_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Plan => "plan",
        ApprovalMode::Ask => "ask",
        ApprovalMode::Auto => "auto",
    }
}

pub fn approval_mode_badge(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Plan => "PLAN",
        ApprovalMode::Ask => "ASK",
        ApprovalMode::Auto => "AUTO",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionCheck {
    pub action: PermissionAction,
    pub resource: String,
    pub effect: EffectClass,
    pub tool_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub action: PermissionAction,
    pub resource: String,
    pub scope: GrantScope,
}

#[derive(Debug)]
struct CompiledRule {
    action: ActionSelector,
    effect: Effect,
    resource: ResourcePattern,
}

impl CompiledRule {
    fn matches(&self, candidate: &Candidate<'_>) -> bool {
        self.action.matches(candidate.action()) && self.resource.matches(candidate)
    }
}

#[derive(Debug)]
struct CompiledGrant {
    grant: Grant,
    resource: ResourcePattern,
}

impl CompiledGrant {
    fn matches(&self, candidate: &Candidate<'_>) -> bool {
        self.grant.action == candidate.action() && self.resource.matches(candidate)
    }
}

const SENSITIVE_FILE_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    ".envrc",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.keystore",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".netrc",
    "_netrc",
    ".npmrc",
    ".pypirc",
    "auth.json",
    "credentials.json",
    "**/.aws/credentials",
    "**/.docker/config.json",
    "**/.kube/config",
    "*secret*.json",
    "*secret*.yml",
    "*secret*.yaml",
    "*secret*.toml",
    "*secret*.txt",
    "*credential*.json",
    "*credential*.yml",
    "*credential*.yaml",
    "*credential*.toml",
];

pub fn default_sensitive_rules() -> Vec<PermissionRule> {
    let mut rules = Vec::with_capacity(SENSITIVE_FILE_PATTERNS.len() * 2 + 1);
    for pattern in SENSITIVE_FILE_PATTERNS {
        for action in [PermissionAction::Read, PermissionAction::Edit] {
            rules.push(PermissionRule {
                action: ActionSelector::Action(action),
                resource: (*pattern).to_string(),
                effect: Effect::Deny,
            });
        }
    }
    rules.push(PermissionRule {
        action: ActionSelector::Action(PermissionAction::Read),
        resource: ".env.example".to_string(),
        effect: Effect::Allow,
    });
    rules
}

#[derive(Debug)]
pub struct PolicyEngine {
    mode: ApprovalMode,
    workspace_root: PathBuf,
    default_rules: Vec<PermissionRule>,
    rules: Vec<PermissionRule>,
    agent_rules: Vec<PermissionRule>,
    compiled_default_rules: Vec<CompiledRule>,
    compiled_rules: Vec<CompiledRule>,
    compiled_agent_rules: Vec<CompiledRule>,
    grants: RwLock<Vec<CompiledGrant>>,
    warnings: RwLock<Vec<String>>,
}

impl PolicyEngine {
    pub fn new(mode: ApprovalMode) -> Self {
        let mut engine = Self {
            mode,
            workspace_root: PathBuf::from("."),
            default_rules: default_sensitive_rules(),
            rules: Vec::new(),
            agent_rules: Vec::new(),
            compiled_default_rules: Vec::new(),
            compiled_rules: Vec::new(),
            compiled_agent_rules: Vec::new(),
            grants: RwLock::new(Vec::new()),
            warnings: RwLock::new(Vec::new()),
        };
        engine.recompile();
        engine
    }

    pub fn with_default_sensitive_rules(mut self, enabled: bool) -> Self {
        self.default_rules = if enabled {
            default_sensitive_rules()
        } else {
            Vec::new()
        };
        self.recompile();
        self
    }

    pub fn with_rules(mut self, rules: Vec<PermissionRule>) -> Self {
        self.rules = rules;
        self.recompile();
        self
    }

    pub fn with_agent_rules(mut self, rules: Vec<PermissionRule>) -> Self {
        self.agent_rules = rules;
        self.recompile();
        self
    }

    pub fn with_workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_root = workspace_root.into();
        self.recompile();
        self
    }

    pub fn from_yaml(mode: ApprovalMode, permissions: &serde_yaml::Value) -> anyhow::Result<Self> {
        let rules = rules_from_yaml(permissions)?;
        Ok(Self::new(mode).with_rules(rules))
    }

    pub fn mode(&self) -> ApprovalMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ApprovalMode) {
        self.mode = mode;
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn rules(&self) -> &[PermissionRule] {
        &self.rules
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings_read().clone()
    }

    pub fn matched_effect(&self, check: &PermissionCheck) -> Option<Effect> {
        let candidate = Candidate::new(check.action, &check.resource, &self.workspace_root);
        self.last_matching_rule(&candidate)
    }

    pub fn decide(&self, check: &PermissionCheck) -> Decision {
        if self.mode == ApprovalMode::Plan && check.effect != EffectClass::Read {
            return Decision::Deny {
                reason: PLAN_READ_ONLY_REASON.to_string(),
            };
        }
        let candidate = Candidate::new(check.action, &check.resource, &self.workspace_root);
        if self.grant_applies(&candidate) {
            return Decision::Allow;
        }
        if let Some(effect) = self.last_matching_rule(&candidate) {
            return match effect {
                Effect::Allow => Decision::Allow,
                Effect::Ask => Decision::Ask,
                Effect::Deny => Decision::Deny {
                    reason: DENY_REASON.to_string(),
                },
            };
        }
        self.tier_default(check)
    }

    pub fn record_grant(&self, grant: Grant) {
        let pattern = ResourcePattern::compile(
            ActionSelector::Action(grant.action),
            &self.workspace_root,
            &globset::escape(&grant.resource),
        );
        match pattern {
            Ok(resource) => self.grants_mut().push(CompiledGrant { grant, resource }),
            Err(warning) => self.warnings_mut().push(format!(
                "skipped grant for action {}: {warning}",
                grant.action.as_str()
            )),
        }
    }

    pub fn grants(&self) -> Vec<Grant> {
        self.grants_read()
            .iter()
            .map(|compiled| compiled.grant.clone())
            .collect()
    }

    pub fn clear_scoped_grants(&self, scope: GrantScope) {
        self.grants_mut()
            .retain(|compiled| compiled.grant.scope != scope);
    }

    fn recompile(&mut self) {
        let mut warnings = Vec::new();
        self.compiled_default_rules = compile_rules(
            &self.workspace_root,
            &self.default_rules,
            "default permission rule",
            &mut warnings,
        );
        self.compiled_rules = compile_rules(
            &self.workspace_root,
            &self.rules,
            "permission rule",
            &mut warnings,
        );
        self.compiled_agent_rules = compile_rules(
            &self.workspace_root,
            &self.agent_rules,
            "agent permission rule",
            &mut warnings,
        );
        *self.warnings_mut() = warnings;
    }

    fn grant_applies(&self, candidate: &Candidate<'_>) -> bool {
        self.grants_read()
            .iter()
            .any(|compiled| compiled.matches(candidate))
    }

    fn last_matching_rule(&self, candidate: &Candidate<'_>) -> Option<Effect> {
        self.compiled_default_rules
            .iter()
            .chain(self.compiled_rules.iter())
            .chain(self.compiled_agent_rules.iter())
            .rfind(|rule| rule.matches(candidate))
            .map(|rule| rule.effect)
    }

    fn tier_default(&self, check: &PermissionCheck) -> Decision {
        match self.mode {
            ApprovalMode::Plan => match check.effect {
                EffectClass::Read => Decision::Allow,
                EffectClass::Write | EffectClass::Execute | EffectClass::External => {
                    Decision::Deny {
                        reason: PLAN_READ_ONLY_REASON.to_string(),
                    }
                }
            },
            ApprovalMode::Ask => match check.effect {
                EffectClass::Read => Decision::Allow,
                EffectClass::Write | EffectClass::Execute | EffectClass::External => Decision::Ask,
            },
            ApprovalMode::Auto => Decision::Allow,
        }
    }

    fn warnings_read(&self) -> RwLockReadGuard<'_, Vec<String>> {
        self.warnings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn warnings_mut(&self) -> RwLockWriteGuard<'_, Vec<String>> {
        self.warnings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn grants_read(&self) -> RwLockReadGuard<'_, Vec<CompiledGrant>> {
        self.grants
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn grants_mut(&self) -> RwLockWriteGuard<'_, Vec<CompiledGrant>> {
        self.grants
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn compile_rules(
    workspace_root: &Path,
    rules: &[PermissionRule],
    label: &str,
    warnings: &mut Vec<String>,
) -> Vec<CompiledRule> {
    rules
        .iter()
        .filter_map(|rule| {
            match ResourcePattern::compile(rule.action, workspace_root, &rule.resource) {
                Ok(resource) => Some(CompiledRule {
                    action: rule.action,
                    effect: rule.effect,
                    resource,
                }),
                Err(warning) => {
                    warnings.push(format!(
                        "skipped {label} for action {}: {warning}",
                        selector_label(rule.action)
                    ));
                    None
                }
            }
        })
        .collect()
}

fn selector_label(selector: ActionSelector) -> &'static str {
    match selector {
        ActionSelector::Any => "*",
        ActionSelector::Action(action) => action.as_str(),
    }
}

pub fn rules_from_yaml(value: &serde_yaml::Value) -> anyhow::Result<Vec<PermissionRule>> {
    let rules: Vec<PermissionRule> = serde_yaml::from_value(value.clone())
        .map_err(|error| anyhow::anyhow!("invalid permission rules: {error}"))?;
    for rule in &rules {
        ResourcePattern::compile(rule.action, Path::new("."), &rule.resource)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(rules)
}

pub fn approval_preview_for(check: &PermissionCheck) -> ApprovalPreview {
    ApprovalPreview {
        title: title_for(check),
        detail: format!(
            "tool: {}\neffect: {}\nresource: {}",
            check.tool_name,
            effect_class_label(check.effect),
            check.resource
        ),
    }
}

fn title_for(check: &PermissionCheck) -> String {
    let title = format!("{}: {}", check.action.as_str(), check.resource);
    if title.chars().count() <= APPROVAL_TITLE_LIMIT {
        return title;
    }
    let mut truncated: String = title.chars().take(APPROVAL_TITLE_LIMIT - 1).collect();
    truncated.push('…');
    truncated
}

fn effect_class_label(effect: EffectClass) -> &'static str {
    match effect {
        EffectClass::Read => "read",
        EffectClass::Write => "write",
        EffectClass::Execute => "execute",
        EffectClass::External => "external",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = r#"
permissions:
  - { action: read,  resource: "*",            effect: allow }
  - { action: read,  resource: "*.env*",       effect: deny }
  - { action: edit,  resource: "src/**",       effect: allow }
  - { action: shell, resource: "cargo *",      effect: allow }
  - { action: shell, resource: "git push *",   effect: ask }
  - { action: shell, resource: "rm *",         effect: deny }
"#;

    fn rule(action: ActionSelector, resource: &str, effect: Effect) -> PermissionRule {
        PermissionRule {
            action,
            resource: resource.to_string(),
            effect,
        }
    }

    fn read_any() -> ActionSelector {
        ActionSelector::Action(PermissionAction::Read)
    }

    fn check(action: PermissionAction, resource: &str, effect: EffectClass) -> PermissionCheck {
        PermissionCheck {
            action,
            resource: resource.to_string(),
            effect,
            tool_name: "code-test".to_string(),
        }
    }

    fn deny() -> Decision {
        Decision::Deny {
            reason: DENY_REASON.to_string(),
        }
    }

    fn plan_deny() -> Decision {
        Decision::Deny {
            reason: PLAN_READ_ONLY_REASON.to_string(),
        }
    }

    fn spec_engine() -> PolicyEngine {
        let config: serde_yaml::Value = serde_yaml::from_str(SPEC_EXAMPLE).unwrap();
        PolicyEngine::from_yaml(ApprovalMode::Ask, &config["permissions"]).unwrap()
    }

    #[test]
    fn mode_defaults_to_ask() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Ask);
    }

    #[test]
    fn mode_can_be_read_and_replaced() {
        let mut engine = PolicyEngine::new(ApprovalMode::Ask);
        assert_eq!(engine.mode(), ApprovalMode::Ask);

        engine.set_mode(ApprovalMode::Plan);
        assert_eq!(engine.mode(), ApprovalMode::Plan);
    }

    #[test]
    fn fallbacks_follow_the_mode_tier() {
        let plan = PolicyEngine::new(ApprovalMode::Plan);
        assert_eq!(
            plan.decide(&check(PermissionAction::Read, "a", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            plan.decide(&check(PermissionAction::Edit, "a", EffectClass::Write)),
            plan_deny()
        );
        assert_eq!(
            plan.decide(&check(PermissionAction::Shell, "ls", EffectClass::Execute)),
            plan_deny()
        );
        assert_eq!(
            plan.decide(&check(
                PermissionAction::WebFetch,
                "https://x.test",
                EffectClass::External
            )),
            plan_deny()
        );

        let ask = PolicyEngine::new(ApprovalMode::Ask);
        assert_eq!(
            ask.decide(&check(PermissionAction::Read, "a", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            ask.decide(&check(PermissionAction::Shell, "ls", EffectClass::Execute)),
            Decision::Ask
        );

        let auto = PolicyEngine::new(ApprovalMode::Auto);
        assert_eq!(
            auto.decide(&check(PermissionAction::Edit, "a", EffectClass::Write)),
            Decision::Allow
        );
        assert_eq!(
            auto.decide(&check(PermissionAction::Shell, "rm", EffectClass::Execute)),
            Decision::Allow
        );
    }

    #[test]
    fn preset_matrix_covers_every_mode_and_effect_class() {
        let cases = [
            (PermissionAction::Read, "a.rs", EffectClass::Read),
            (PermissionAction::Edit, "a.rs", EffectClass::Write),
            (PermissionAction::Shell, "ls", EffectClass::Execute),
            (
                PermissionAction::WebFetch,
                "https://x.test",
                EffectClass::External,
            ),
        ];
        let matrix: [(ApprovalMode, [Decision; 4]); 3] = [
            (
                ApprovalMode::Plan,
                [Decision::Allow, plan_deny(), plan_deny(), plan_deny()],
            ),
            (
                ApprovalMode::Ask,
                [Decision::Allow, Decision::Ask, Decision::Ask, Decision::Ask],
            ),
            (
                ApprovalMode::Auto,
                [
                    Decision::Allow,
                    Decision::Allow,
                    Decision::Allow,
                    Decision::Allow,
                ],
            ),
        ];

        for (mode, expected) in matrix {
            let engine = PolicyEngine::new(mode);
            for (index, (action, resource, effect)) in cases.iter().enumerate() {
                assert_eq!(
                    engine.decide(&check(*action, resource, *effect)),
                    expected[index],
                    "mode {mode:?} case {resource}"
                );
            }
        }
    }

    #[test]
    fn spec_yaml_rules_decide_by_last_match() {
        let engine = spec_engine();

        assert_eq!(
            engine.decide(&check(PermissionAction::Read, ".env", EffectClass::Read)),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "config/prod.env.local",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "a/b/c/.env.local",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Edit,
                "src/lib/mod.rs",
                EffectClass::Write
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "cargo test -p meerai-policy",
                EffectClass::Execute
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "git push --force",
                EffectClass::Execute
            )),
            Decision::Ask
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "rm -rf target",
                EffectClass::Execute
            )),
            deny()
        );
    }

    #[test]
    fn last_matching_global_rule_wins() {
        let deny_then_allow = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "*", Effect::Deny),
            rule(read_any(), "*", Effect::Allow),
        ]);
        assert_eq!(
            deny_then_allow.decide(&check(PermissionAction::Read, "a", EffectClass::Read)),
            Decision::Allow
        );

        let allow_then_deny = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "*", Effect::Allow),
            rule(read_any(), "*", Effect::Deny),
        ]);
        assert_eq!(
            allow_then_deny.decide(&check(PermissionAction::Read, "a", EffectClass::Read)),
            deny()
        );
    }

    #[test]
    fn auto_allows_by_default_but_respects_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Auto)
            .with_workspace_root("/tmp/ws")
            .with_rules(vec![
                rule(
                    ActionSelector::Action(PermissionAction::Shell),
                    "git push *",
                    Effect::Ask,
                ),
                rule(
                    ActionSelector::Action(PermissionAction::Shell),
                    "rm *",
                    Effect::Deny,
                ),
            ]);

        for (action, effect, resource) in [
            (PermissionAction::Read, EffectClass::Read, "src/main.rs"),
            (PermissionAction::Edit, EffectClass::Write, "src/main.rs"),
            (PermissionAction::Shell, EffectClass::Execute, "cargo test"),
            (
                PermissionAction::WebFetch,
                EffectClass::External,
                "https://x",
            ),
        ] {
            assert_eq!(
                engine.decide(&check(action, resource, effect)),
                Decision::Allow,
                "{action:?} {resource}"
            );
        }
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "git push origin main",
                EffectClass::Execute
            )),
            Decision::Ask
        );
        assert!(matches!(
            engine.decide(&check(
                PermissionAction::Shell,
                "rm -rf target",
                EffectClass::Execute
            )),
            Decision::Deny { .. }
        ));
        assert_eq!(ApprovalMode::parse("auto"), Some(ApprovalMode::Auto));
        assert_eq!(ApprovalMode::parse(" plan "), Some(ApprovalMode::Plan));
        assert_eq!(ApprovalMode::parse("wild"), None);
    }

    #[test]
    fn matched_effect_reports_explicit_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Ask)
            .with_workspace_root("/tmp/ws")
            .with_rules(vec![rule(
                ActionSelector::Action(PermissionAction::Shell),
                "git *",
                Effect::Ask,
            )]);
        let ask = check(PermissionAction::Shell, "git status", EffectClass::Execute);
        assert_eq!(engine.matched_effect(&ask), Some(Effect::Ask));
        assert_eq!(engine.decide(&ask), Decision::Ask);

        let other = check(PermissionAction::Shell, "ls -la", EffectClass::Execute);
        assert_eq!(engine.matched_effect(&other), None);
    }

    #[test]
    fn rules_match_exact_resource_or_wildcard() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "*", Effect::Allow),
            rule(read_any(), "secrets.txt", Effect::Deny),
        ]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "secrets.txt",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(PermissionAction::Read, "main.rs", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
    }

    #[test]
    fn glob_rules_match_paths() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "*", Effect::Allow),
            rule(read_any(), "*.env*", Effect::Deny),
            rule(read_any(), "src/**", Effect::Deny),
        ]);

        assert_eq!(
            engine.decide(&check(PermissionAction::Read, ".env", EffectClass::Read)),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/nested/main.rs",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "docs/readme.md",
                EffectClass::Read
            )),
            Decision::Allow
        );
    }

    #[test]
    fn single_star_does_not_cross_directories() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "*", Effect::Allow),
            rule(read_any(), "src/*.rs", Effect::Deny),
        ]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/main.rs",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/nested/main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
    }

    #[test]
    fn absolute_paths_match_relative_globs() {
        let engine = PolicyEngine::new(ApprovalMode::Ask)
            .with_workspace_root("/tmp/ws")
            .with_rules(vec![rule(read_any(), "src/**", Effect::Deny)]);

        assert_eq!(engine.workspace_root(), Path::new("/tmp/ws"));
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "/tmp/ws/src/main.rs",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "/tmp/ws/README.md",
                EffectClass::Read
            )),
            Decision::Allow
        );
    }

    #[test]
    fn workspace_root_can_be_set_after_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Ask)
            .with_rules(vec![rule(read_any(), "src/**", Effect::Deny)])
            .with_workspace_root("/tmp/ws");

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "/tmp/ws/src/main.rs",
                EffectClass::Read
            )),
            deny()
        );
    }

    #[test]
    fn tilde_paths_are_expanded_before_matching() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            ActionSelector::Action(PermissionAction::ExternalDirectory),
            "~/secrets/**",
            Effect::Deny,
        )]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::ExternalDirectory,
                "~/secrets/key.txt",
                EffectClass::External
            )),
            deny()
        );
    }

    #[test]
    fn shell_rules_match_command_prefix_globs() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(
                ActionSelector::Action(PermissionAction::Shell),
                "git push *",
                Effect::Ask,
            ),
            rule(
                ActionSelector::Action(PermissionAction::Shell),
                "cargo *",
                Effect::Allow,
            ),
            rule(
                ActionSelector::Action(PermissionAction::Shell),
                "rm *",
                Effect::Deny,
            ),
        ]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "git push origin main",
                EffectClass::Execute
            )),
            Decision::Ask
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "cargo run -- src/main.rs",
                EffectClass::Execute
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "rm -rf src",
                EffectClass::Execute
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "ls -la",
                EffectClass::Execute
            )),
            Decision::Ask
        );
    }

    #[test]
    fn agent_rules_refine_global_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Ask)
            .with_rules(vec![rule(
                ActionSelector::Action(PermissionAction::Shell),
                "rm",
                Effect::Deny,
            )])
            .with_agent_rules(vec![rule(
                ActionSelector::Action(PermissionAction::Shell),
                "rm",
                Effect::Allow,
            )]);

        assert_eq!(
            engine.decide(&check(PermissionAction::Shell, "rm", EffectClass::Execute)),
            Decision::Allow
        );
    }

    #[test]
    fn glob_agent_rules_refine_global_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Ask)
            .with_rules(vec![rule(
                ActionSelector::Action(PermissionAction::Shell),
                "*",
                Effect::Deny,
            )])
            .with_agent_rules(vec![rule(
                ActionSelector::Action(PermissionAction::Shell),
                "cargo *",
                Effect::Allow,
            )]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "cargo build",
                EffectClass::Execute
            )),
            Decision::Allow
        );
    }

    #[test]
    fn any_action_selector_matches_every_action() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            ActionSelector::Any,
            "*",
            Effect::Deny,
        )]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::WebFetch,
                "url",
                EffectClass::External
            )),
            deny()
        );
    }

    #[test]
    fn any_action_selector_supports_globs() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            ActionSelector::Any,
            "src/**",
            Effect::Deny,
        )]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/main.rs",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Edit,
                "src/main.rs",
                EffectClass::Write
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "cargo test",
                EffectClass::Execute
            )),
            Decision::Ask
        );
    }

    #[test]
    fn auto_mode_honors_explicit_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Auto).with_rules(vec![
            rule(
                ActionSelector::Action(PermissionAction::Shell),
                "rm *",
                Effect::Deny,
            ),
            rule(
                ActionSelector::Action(PermissionAction::Shell),
                "git push *",
                Effect::Ask,
            ),
        ]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "rm -rf target",
                EffectClass::Execute
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "git push origin main",
                EffectClass::Execute
            )),
            Decision::Ask
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "cargo test",
                EffectClass::Execute
            )),
            Decision::Allow
        );
    }

    #[test]
    fn auto_mode_without_rules_allows_everything() {
        let engine = PolicyEngine::new(ApprovalMode::Auto);

        assert_eq!(
            engine.decide(&check(PermissionAction::Shell, "rm", EffectClass::Execute)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Edit,
                "src/main.rs",
                EffectClass::Write
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::WebFetch,
                "https://x.test",
                EffectClass::External
            )),
            Decision::Allow
        );
    }

    #[test]
    fn plan_denial_reason_reaches_the_refusal_message() {
        let engine = PolicyEngine::new(ApprovalMode::Plan);

        let decision = engine.decide(&check(
            PermissionAction::Edit,
            "src/main.rs",
            EffectClass::Write,
        ));
        let Decision::Deny { reason } = decision else {
            panic!("expected plan mode to deny writes, got {decision:?}");
        };

        assert_eq!(reason, PLAN_READ_ONLY_REASON);
        let observation = meerai_core::ToolObservation::refused(reason);
        assert_eq!(
            observation.render(),
            "Refused: plan mode is read-only".to_string()
        );
    }

    #[test]
    fn grants_allow_repeated_checks() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            ActionSelector::Action(PermissionAction::Shell),
            "rm",
            Effect::Deny,
        )]);
        let target = check(PermissionAction::Shell, "rm", EffectClass::Execute);

        assert_eq!(engine.decide(&target), deny());

        engine.record_grant(Grant {
            action: PermissionAction::Shell,
            resource: "rm".to_string(),
            scope: GrantScope::Session,
        });

        assert_eq!(engine.decide(&target), Decision::Allow);
        assert_eq!(engine.grants().len(), 1);
    }

    #[test]
    fn literal_grants_do_not_expand_wildcards() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            read_any(),
            "*",
            Effect::Ask,
        )]);
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "*".to_string(),
            scope: GrantScope::Session,
        });
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "src/*".to_string(),
            scope: GrantScope::Session,
        });

        assert_eq!(
            engine.decide(&check(PermissionAction::Read, "src/*", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/other.rs",
                EffectClass::Read
            )),
            Decision::Ask
        );
        assert_eq!(
            engine.decide(&check(PermissionAction::Read, "a", EffectClass::Read)),
            Decision::Ask
        );
    }

    #[test]
    fn session_grants_are_literal_and_do_not_override_deny_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "src/**", Effect::Ask),
            rule(read_any(), "src/secrets.txt", Effect::Deny),
        ]);
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "src/*".to_string(),
            scope: GrantScope::Session,
        });

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/secrets.txt",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/other.rs",
                EffectClass::Read
            )),
            Decision::Ask
        );
    }

    #[test]
    fn grants_match_exact_paths_and_clear_by_scope() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            read_any(),
            "*",
            Effect::Ask,
        )]);
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "src/nested/main.rs".to_string(),
            scope: GrantScope::Turn,
        });
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "docs/guide.md".to_string(),
            scope: GrantScope::Session,
        });

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/nested/main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "docs/guide.md",
                EffectClass::Read
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "README.md",
                EffectClass::Read
            )),
            Decision::Ask
        );

        engine.clear_scoped_grants(GrantScope::Turn);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/nested/main.rs",
                EffectClass::Read
            )),
            Decision::Ask
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "docs/guide.md",
                EffectClass::Read
            )),
            Decision::Allow
        );

        engine.clear_scoped_grants(GrantScope::Session);
        assert!(engine.grants().is_empty());
    }

    #[test]
    fn literal_basename_grants_match_at_any_depth() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            read_any(),
            "*",
            Effect::Ask,
        )]);
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: ".env".to_string(),
            scope: GrantScope::Session,
        });
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: ".env.local".to_string(),
            scope: GrantScope::Session,
        });

        assert_eq!(
            engine.decide(&check(PermissionAction::Read, ".env", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "a/b/c/.env.local",
                EffectClass::Read
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "README.md",
                EffectClass::Read
            )),
            Decision::Ask
        );
    }

    #[test]
    fn grants_are_scoped() {
        let engine = PolicyEngine::new(ApprovalMode::Ask);
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "a".to_string(),
            scope: GrantScope::Turn,
        });
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "b".to_string(),
            scope: GrantScope::Session,
        });

        engine.clear_scoped_grants(GrantScope::Turn);

        let grants = engine.grants();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].resource, "b");
    }

    #[test]
    fn grants_escape_glob_metacharacters() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            read_any(),
            "*",
            Effect::Ask,
        )]);
        engine.record_grant(Grant {
            action: PermissionAction::Read,
            resource: "src/[".to_string(),
            scope: GrantScope::Session,
        });

        assert!(engine.warnings().is_empty());
        assert_eq!(engine.grants().len(), 1);
        assert_eq!(
            engine.decide(&check(PermissionAction::Read, "src/[", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(PermissionAction::Read, "src/a", EffectClass::Read)),
            Decision::Ask
        );
    }

    #[test]
    fn builder_skips_invalid_globs_with_warnings() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![
            rule(read_any(), "*", Effect::Allow),
            rule(read_any(), "src/[", Effect::Deny),
        ]);

        assert_eq!(engine.warnings().len(), 1);
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
    }

    #[test]
    fn from_yaml_builds_ordered_rules() {
        let permissions: serde_yaml::Value = serde_yaml::from_str(
            r#"
- { action: read, resource: "*", effect: allow }
- { action: read, resource: "secrets.txt", effect: deny }
- { action: shell, resource: "rm", effect: deny }
- { action: webfetch, resource: "*", effect: ask }
"#,
        )
        .unwrap();

        let engine = PolicyEngine::from_yaml(ApprovalMode::Ask, &permissions).unwrap();

        assert_eq!(
            engine.decide(&check(PermissionAction::Read, "main.rs", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "secrets.txt",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(PermissionAction::Shell, "rm", EffectClass::Execute)),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::WebFetch,
                "url",
                EffectClass::External
            )),
            Decision::Ask
        );
    }

    #[test]
    fn from_yaml_rejects_unknown_actions() {
        let permissions: serde_yaml::Value = serde_yaml::from_str(
            r#"
- { action: teleport, resource: "*", effect: allow }
"#,
        )
        .unwrap();

        let error = PolicyEngine::from_yaml(ApprovalMode::Ask, &permissions).unwrap_err();
        assert!(error.to_string().contains("unknown permission action"));
    }

    #[test]
    fn from_yaml_rejects_unknown_effects() {
        let permissions: serde_yaml::Value = serde_yaml::from_str(
            r#"
- { action: read, resource: "*", effect: maybe }
"#,
        )
        .unwrap();

        let error = PolicyEngine::from_yaml(ApprovalMode::Ask, &permissions).unwrap_err();
        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn from_yaml_rejects_invalid_globs() {
        let permissions: serde_yaml::Value = serde_yaml::from_str(
            r#"
- { action: read, resource: "src/[", effect: deny }
"#,
        )
        .unwrap();

        let error = PolicyEngine::from_yaml(ApprovalMode::Ask, &permissions).unwrap_err();
        assert!(error.to_string().contains("invalid glob pattern"));
    }

    #[test]
    fn from_yaml_rejects_wrong_shape() {
        let permissions: serde_yaml::Value = serde_yaml::from_str(
            r#"
read:
  resource: "*"
  effect: allow
"#,
        )
        .unwrap();

        assert!(PolicyEngine::from_yaml(ApprovalMode::Ask, &permissions).is_err());
    }

    #[test]
    fn rules_from_yaml_composes_engines() {
        let config: serde_yaml::Value = serde_yaml::from_str(SPEC_EXAMPLE).unwrap();
        let rules = rules_from_yaml(&config["permissions"]).unwrap();
        assert_eq!(rules.len(), 6);

        let ask = PolicyEngine::new(ApprovalMode::Ask).with_rules(rules.clone());
        let plan = PolicyEngine::new(ApprovalMode::Plan).with_rules(rules);

        assert_eq!(
            ask.decide(&check(
                PermissionAction::Shell,
                "cargo test",
                EffectClass::Execute
            )),
            Decision::Allow
        );
        assert_eq!(
            plan.decide(&check(
                PermissionAction::Shell,
                "cargo test",
                EffectClass::Execute
            )),
            plan_deny()
        );
        assert_eq!(
            plan.decide(&check(
                PermissionAction::Edit,
                "src/main.rs",
                EffectClass::Write
            )),
            plan_deny()
        );
        assert_eq!(
            plan.decide(&check(
                PermissionAction::Read,
                "src/main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
        assert_eq!(
            plan.decide(&check(PermissionAction::Read, ".env", EffectClass::Read)),
            deny()
        );
    }

    #[test]
    fn plan_mode_denies_writes_despite_allow_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Plan).with_rules(vec![
            rule(ActionSelector::Any, "*", Effect::Allow),
            rule(read_any(), "secrets.txt", Effect::Deny),
            rule(read_any(), "docs/**", Effect::Ask),
        ]);

        assert_eq!(
            engine.decide(&check(
                PermissionAction::Edit,
                "src/main.rs",
                EffectClass::Write
            )),
            plan_deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Shell,
                "cargo test",
                EffectClass::Execute
            )),
            plan_deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::WebFetch,
                "https://x.test",
                EffectClass::External
            )),
            plan_deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "src/main.rs",
                EffectClass::Read
            )),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "secrets.txt",
                EffectClass::Read
            )),
            deny()
        );
        assert_eq!(
            engine.decide(&check(
                PermissionAction::Read,
                "docs/guide.md",
                EffectClass::Read
            )),
            Decision::Ask
        );
    }

    #[test]
    fn approval_preview_records_action_and_resource() {
        let preview = approval_preview_for(&check(
            PermissionAction::Shell,
            "git push origin main",
            EffectClass::Execute,
        ));

        assert_eq!(preview.title, "shell: git push origin main");
        assert!(preview.detail.contains("code-test"));
        assert!(preview.detail.contains("execute"));
        assert!(preview.detail.contains("git push origin main"));
    }

    #[test]
    fn approval_preview_truncates_long_titles() {
        let resource = "x".repeat(500);
        let preview = approval_preview_for(&check(
            PermissionAction::Shell,
            &resource,
            EffectClass::Execute,
        ));

        assert_eq!(preview.title.chars().count(), 200);
        assert!(preview.title.starts_with("shell: x"));
        assert!(preview.title.ends_with('…'));
        assert!(preview.detail.contains(&resource));
    }

    #[test]
    fn action_selector_string_contract() {
        let any: ActionSelector = serde_yaml::from_str("\"*\"").unwrap();
        assert_eq!(any, ActionSelector::Any);
        assert_eq!(String::from(any), "*");

        let fetch: ActionSelector = serde_yaml::from_str("\"webfetch\"").unwrap();
        assert_eq!(fetch, ActionSelector::Action(PermissionAction::WebFetch));
        assert_eq!(String::from(fetch), "webfetch");

        let external: ActionSelector = serde_yaml::from_str("\"external_directory\"").unwrap();
        assert_eq!(
            external,
            ActionSelector::Action(PermissionAction::ExternalDirectory)
        );
        assert_eq!(String::from(external), "external_directory");

        assert!(serde_yaml::from_str::<ActionSelector>("\"bogus\"").is_err());
    }

    #[test]
    fn default_sensitive_rules_deny_reads_and_edits_in_ask_and_auto() {
        let sensitive_paths = [
            ".env",
            "config/.env.local",
            "prod.env",
            ".envrc",
            "private.pem",
            "certs/server.key",
            "cert.p12",
            "cert.pfx",
            "store.jks",
            "app.keystore",
            "id_rsa",
            "id_dsa",
            "id_ecdsa",
            ".ssh/id_ed25519",
            ".netrc",
            "_netrc",
            ".npmrc",
            ".pypirc",
            "auth.json",
            "credentials.json",
            ".aws/credentials",
            "/home/user/.aws/credentials",
            ".docker/config.json",
            ".kube/config",
            "secrets.json",
            "secrets.yml",
            "app-secret.yaml",
            "secret.toml",
            "secrets.txt",
            "credentials.yml",
            "db-credential.yaml",
            "credentials.toml",
        ];

        for mode in [ApprovalMode::Ask, ApprovalMode::Auto] {
            let engine = PolicyEngine::new(mode);
            assert!(engine.rules().is_empty());
            assert!(engine.warnings().is_empty());

            for path in sensitive_paths {
                assert_eq!(
                    engine.decide(&check(PermissionAction::Read, path, EffectClass::Read)),
                    deny(),
                    "mode {mode:?} read {path}"
                );
                assert_eq!(
                    engine.decide(&check(PermissionAction::Edit, path, EffectClass::Write)),
                    deny(),
                    "mode {mode:?} edit {path}"
                );
            }
        }
    }

    #[test]
    fn default_sensitive_rules_allow_source_files_and_env_example_read() {
        let allowed_reads = [
            "src/secrets.rs",
            "src/credentials.rs",
            ".env.example",
            "config/.env.example",
            "src/auth.rs",
        ];

        for mode in [ApprovalMode::Plan, ApprovalMode::Ask, ApprovalMode::Auto] {
            let engine = PolicyEngine::new(mode);
            for path in allowed_reads {
                assert_eq!(
                    engine.decide(&check(PermissionAction::Read, path, EffectClass::Read)),
                    Decision::Allow,
                    "mode {mode:?} read {path}"
                );
            }
        }
    }

    #[test]
    fn explicit_user_rule_can_override_default_sensitive_rules() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![rule(
            read_any(),
            ".env",
            Effect::Allow,
        )]);

        assert_eq!(
            engine.decide(&check(PermissionAction::Read, ".env", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(PermissionAction::Edit, ".env", EffectClass::Write)),
            deny()
        );
        assert_eq!(engine.rules().len(), 1);
    }

    #[test]
    fn default_sensitive_rules_can_be_disabled() {
        let engine = PolicyEngine::new(ApprovalMode::Ask).with_default_sensitive_rules(false);
        assert_eq!(
            engine.decide(&check(PermissionAction::Read, ".env", EffectClass::Read)),
            Decision::Allow
        );
        assert_eq!(
            engine.decide(&check(PermissionAction::Edit, ".env", EffectClass::Write)),
            Decision::Ask
        );
    }
}
