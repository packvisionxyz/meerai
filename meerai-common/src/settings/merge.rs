use super::*;

pub fn find_project_root(start: &Path) -> PathBuf {
    for dir in start.ancestors() {
        if dir.as_os_str().is_empty() {
            break;
        }
        if dir.join(".git").exists() || dir.join(".meerai").join(PROJECT_SETTINGS_FILE).exists() {
            return dir.to_path_buf();
        }
    }
    start.to_path_buf()
}

pub fn load_project_settings(workspace: &Path) -> Result<Option<AppSettings>> {
    let path = workspace.join(".meerai").join(PROJECT_SETTINGS_FILE);
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
    if project.routing.is_some() {
        warnings.push(
            "ignoring project routing: only the user config controls model routes".to_string(),
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

pub(super) fn combine_rules(
    user: Option<serde_yaml::Value>,
    mut asks: Vec<serde_yaml::Value>,
    denies: Vec<serde_yaml::Value>,
    warnings: &mut Vec<String>,
) -> Option<serde_yaml::Value> {
    let user_rules = match user {
        None => Vec::new(),
        Some(serde_yaml::Value::Sequence(rules)) => rules,
        Some(other) => {
            warnings.push(
                "ignoring project permissions: user permissions are not a list of rules"
                    .to_string(),
            );
            return Some(other);
        }
    };
    // The policy engine applies explicit rules after its defaults. Keep sensitive-file
    // denies ahead of user overrides, but after project asks that can match them.
    if asks.iter().any(|ask| {
        ask.as_mapping()
            .and_then(|map| mapping_str(map, "action"))
            .is_some_and(|action| matches!(action, "*" | "read" | "edit"))
    }) {
        asks.extend(
            default_sensitive_rules().into_iter().map(|rule| {
                serde_yaml::to_value(rule).expect("permission rules serialize to YAML")
            }),
        );
    }
    asks.extend(user_rules);
    asks.extend(denies);
    Some(serde_yaml::Value::Sequence(asks))
}

pub(super) fn split_project_permissions(
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
    let rules = serde_yaml::Value::Sequence(asks.iter().chain(&denies).cloned().collect());
    if rules_from_yaml(&rules).is_err() {
        warnings.push("ignoring project permissions: invalid rule resource".to_string());
        return None;
    }
    Some((asks, denies))
}

pub(super) fn mapping_str<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    map.iter()
        .find(|(name, _)| name.as_str() == Some(key))
        .and_then(|(_, value)| value.as_str())
}

pub(super) fn merge_context(
    user: Option<ContextSettings>,
    project: Option<ContextSettings>,
) -> Option<ContextSettings> {
    let Some(project) = project else {
        return user;
    };
    let user = user.unwrap_or_default();
    Some(ContextSettings {
        context_window_tokens: project.context_window_tokens.or(user.context_window_tokens),
        high_watermark_pct: project.high_watermark_pct.or(user.high_watermark_pct),
        spill_max_inline_tokens: project
            .spill_max_inline_tokens
            .or(user.spill_max_inline_tokens),
        verbatim_window_turns: project.verbatim_window_turns.or(user.verbatim_window_turns),
        reasoning_pruning: project.reasoning_pruning.or(user.reasoning_pruning),
        prompt_caching: project.prompt_caching.or(user.prompt_caching),
        rollback_on_verify_failure: user.rollback_on_verify_failure,
        recovery_model: same_provider_model(project.recovery_model).or(user.recovery_model),
        max_steps_per_turn: merge_step_limit(user.max_steps_per_turn, project.max_steps_per_turn),
        no_progress_fail_steps: tighten_limit(
            user.no_progress_fail_steps,
            project.no_progress_fail_steps,
            12,
        ),
        step_limit_continuations: tighten_limit(
            user.step_limit_continuations,
            project.step_limit_continuations,
            2,
        ),
        tool_timeout_ms: tighten_limit(user.tool_timeout_ms, project.tool_timeout_ms, 120_000),
        max_session_tokens: merge_max_session_tokens(
            user.max_session_tokens,
            project.max_session_tokens,
        ),
        max_turn_secs: merge_max_turn_secs(user.max_turn_secs, project.max_turn_secs),
        summarizer_model: same_provider_model(project.summarizer_model).or(user.summarizer_model),
        post_edit_diagnostics: project.post_edit_diagnostics.or(user.post_edit_diagnostics),
        temperature: project.temperature.or(user.temperature),
        effort: project.effort.or(user.effort),
        auto_memory: project.auto_memory.or(user.auto_memory),
        memory_prompt_bytes: tighten_limit(
            user.memory_prompt_bytes,
            project.memory_prompt_bytes,
            8 * 1024,
        ),
    })
}

pub(super) fn same_provider_model(model: Option<String>) -> Option<String> {
    model.filter(|model| !model.trim().is_empty() && !model.contains(':'))
}

pub(super) fn tighten_limit<T: Ord>(user: Option<T>, project: Option<T>, default: T) -> Option<T> {
    match (user, project) {
        (Some(user), Some(project)) => Some(user.min(project)),
        (None, Some(project)) if project < default => Some(project),
        (user, _) => user,
    }
}

pub(super) fn merge_step_limit(user: Option<usize>, project: Option<usize>) -> Option<usize> {
    let rank = |steps: usize| if steps == 0 { usize::MAX } else { steps };
    let effective = match (user, project) {
        (None, None) => return None,
        (Some(user), None) => rank(user),
        (None, Some(project)) => rank(project),
        (Some(user), Some(project)) => rank(user).min(rank(project)),
    };
    (effective != usize::MAX).then_some(effective)
}

pub(super) fn merge_max_session_tokens(user: Option<u64>, project: Option<u64>) -> Option<u64> {
    match (user, project) {
        (Some(user), Some(project)) => Some(user.min(project)),
        (user, project) => user.or(project),
    }
}

pub(super) fn merge_max_turn_secs(user: Option<u64>, project: Option<u64>) -> Option<u64> {
    match user {
        None => None,
        Some(0) => Some(project.unwrap_or(0)),
        Some(user) => match project {
            Some(0) | None => Some(user),
            Some(project) => Some(user.min(project)),
        },
    }
}

pub(super) fn merge_sandbox(
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
                strict_env: project.strict_env.filter(|v| *v).or(user.strict_env),
            })
        }
    }
}
