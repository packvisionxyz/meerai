use super::forms::*;
use super::misc::*;
use super::types::*;
use super::*;

pub fn toggle_diagnose(state: &mut AppState) {
    if state.sidebar_open {
        state.sidebar_diagnose_open = !state.sidebar_diagnose_open;
    } else {
        state.sidebar_open = true;
        state.sidebar_diagnose_open = true;
    }
}

pub fn toggle_changes(state: &mut AppState) {
    if state.sidebar_open {
        state.sidebar_changes_open = !state.sidebar_changes_open;
    } else {
        state.sidebar_open = true;
        state.sidebar_changes_open = true;
    }
}

pub fn effort_label(effort: EffortDto) -> &'static str {
    match effort {
        EffortDto::Low => "low",
        EffortDto::Medium => "medium",
        EffortDto::High => "high",
        EffortDto::ExtraHigh => "extra high",
        EffortDto::Max => "max",
    }
}

pub fn effort_default_temperature(effort: EffortDto) -> f32 {
    match effort {
        EffortDto::Low => 0.0,
        EffortDto::Medium => 0.2,
        EffortDto::High => 0.5,
        EffortDto::ExtraHigh => 0.7,
        EffortDto::Max => 0.9,
    }
}

pub fn set_effort_from_arg(state: &mut AppState, arg: &str) {
    let value = arg.trim().to_ascii_lowercase();
    let allowed = state.host_info.settings.context.effort_levels.clone();
    let allowed = if allowed.is_empty() {
        EffortDto::ALL.to_vec()
    } else {
        allowed
    };
    let current = state
        .host_info
        .settings
        .context
        .effort
        .unwrap_or(EffortDto::Medium)
        .clamp_to(&allowed);
    let next = if value.is_empty() {
        current.next_in(&allowed)
    } else if let Some(next) = EffortDto::parse(&value) {
        next
    } else {
        set_status_notice(
            state,
            format!(
                "Unknown effort '{value}' · use {}",
                allowed
                    .iter()
                    .map(|level| level.keyword())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        return;
    };
    let next = next.clamp_to(&allowed);
    if !state.host_info.settings.config_writable {
        set_status_notice(state, "User settings file is read-only on this host.");
        return;
    }
    state.host_info.settings.context.effort = Some(next);
    state.host_info.settings.context.temperature = None;
    let context = state.host_info.settings.context.clone();
    if let Some(t) = crate::transport::current() {
        t.update_settings(UpdateSettingsRequest {
            reset_theme: false,
            theme: None,
            context: Some(context),
            session: None,
            sandbox: None,
            forget_model: None,
        });
    }
    set_status_notice(
        state,
        format!(
            "Effort {} · temp {:.2}",
            effort_label(next),
            effort_default_temperature(next)
        ),
    );
}

pub fn set_approval_mode(state: &mut AppState, mode: ApprovalModeDto) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    if state.active_session().is_some_and(|s| s.transient) {
        set_status_notice(state, "btw is read-only.");
        return;
    }
    if mode == ApprovalModeDto::Plan
        && state
            .active_session()
            .is_some_and(|s| matches!(s.backend, BackendKind::Acp { .. }))
    {
        state.error_banner = Some("Plan mode is not available for ACP sessions.".to_owned());
        return;
    }
    if let Some(t) = crate::transport::current() {
        t.set_mode(sid, mode);
    }
}

pub fn cycle_approval_mode(state: &mut AppState) {
    let Some(session) = state.active_session() else {
        return;
    };
    let is_acp = matches!(session.backend, BackendKind::Acp { .. });
    let next = match session.approval_mode {
        ApprovalModeDto::Plan => ApprovalModeDto::Ask,
        ApprovalModeDto::Ask => ApprovalModeDto::Auto,
        ApprovalModeDto::Auto => {
            if is_acp {
                ApprovalModeDto::Ask
            } else {
                ApprovalModeDto::Plan
            }
        }
    };
    set_approval_mode(state, next);
}

pub fn open_settings(state: &mut AppState, section: Option<SettingsSection>) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    let mut form = SettingsFormState::from_host(&state.host_info);
    if let Some(sec) = section {
        form.section = sec;
    }
    state.modal = Some(ActiveModal::Settings(form));
}

pub fn sync_diff_modal_on_changes(state: &mut AppState) {
    let Some(ActiveModal::DiffView {
        selected_path,
        diff,
        loading,
        error,
        ..
    }) = state.modal.as_mut()
    else {
        return;
    };

    if let Some(current_path) = selected_path.as_ref()
        && let Some(entry) = state.file_changes.iter().find(|c| &c.path == current_path)
    {
        if diff.is_none() {
            *loading = true;
        }
        if let Some(t) = crate::transport::current() {
            t.fetch_workspace_diff(entry.path.clone(), Some(entry.status.clone()));
        }
        return;
    }

    if let Some(first) = state.file_changes.first().cloned() {
        *selected_path = Some(first.path.clone());
        *diff = None;
        *loading = true;
        *error = None;
        if let Some(t) = crate::transport::current() {
            t.fetch_workspace_diff(first.path, Some(first.status));
        }
    } else {
        *selected_path = None;
        *diff = None;
        *loading = false;
        *error = None;
    }
}
