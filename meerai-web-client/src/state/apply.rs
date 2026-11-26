use super::digest::*;
use super::forms::*;
use super::misc::*;
use super::providers::*;
use super::sessions::*;
use super::settings::*;
use super::types::*;
use super::views::*;
use super::*;

pub fn apply_snapshot(state: &mut AppState, snapshot: SnapshotResponse) {
    state.connection = ConnectionState::Connected;
    state.last_event_id = snapshot.event_id;
    state.error_banner = None;
    state.file_changes = snapshot.file_changes;
    if let Some(info) = snapshot.host_info {
        state.host_info = info;
    }

    let previous_active = state.active_session_id.clone();
    state.sessions = snapshot.sessions;
    sync_tab_order(state);

    let active_id = snapshot
        .active_session_id
        .or_else(|| previous_active.filter(|id| state.sessions.iter().any(|s| &s.id == id)))
        .or_else(|| state.sessions.first().map(|s| s.id.clone()));

    state.active_session_id = active_id.clone();
    if let Some(detail) = snapshot.active_session {
        let detail_id = detail.summary.id.clone();
        state.hidden_session_ids.remove(&detail_id);
        apply_active_detail(state, detail);
        if state.active_session_id.is_none() {
            state.active_session_id = Some(detail_id);
        }
    } else if let Some(id) = &active_id {
        let _ = state.session_tabs.entry(id.clone()).or_default();
    }
}

pub fn apply_active_detail(state: &mut AppState, detail: SessionDetailDto) {
    let id = detail.summary.id.clone();
    upsert_session(state, detail.summary);
    let tab = state.session_tabs.entry(id.clone()).or_default();
    tab.entries = detail.entries;
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.hydrated = true;
    tab.streaming_item_id = detail.streaming_item_id;
    tab.streaming_text = detail.streaming_text;
    tab.streaming_reasoning = detail.streaming_reasoning;
    tab.pending_approvals = detail.pending_approvals;
    tab.pending_questions = detail.pending_questions;
    tab.usage = detail.usage;
    tab.metrics = detail.metrics;
    tab.context = detail.context;
    tab.loop_state = detail.loop_state;
    tab.unread = false;
    sync_pending_modal_for_active(state);
}

pub fn apply_session_detail(state: &mut AppState, detail: SessionDetailDto) {
    let id = detail.summary.id.clone();
    upsert_session(state, detail.summary);
    let tab = state.session_tabs.entry(id.clone()).or_default();
    tab.entries = detail.entries;
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.hydrated = true;
    tab.streaming_item_id = detail.streaming_item_id;
    tab.streaming_text = detail.streaming_text;
    tab.streaming_reasoning = detail.streaming_reasoning;
    tab.pending_approvals = detail.pending_approvals;
    tab.pending_questions = detail.pending_questions;
    tab.usage = detail.usage;
    tab.metrics = detail.metrics;
    tab.context = detail.context;
    tab.loop_state = detail.loop_state;
    if state.active_session_id.as_deref() == Some(id.as_str()) {
        tab.unread = false;
        sync_pending_modal_for_active(state);
    }
}

pub fn apply_created_session(state: &mut AppState, session: SessionSummaryDto) {
    let id = session.id.clone();
    if let Some(prev) = state.pending_replace_session.take()
        && prev != id
    {
        state.hidden_session_ids.insert(prev);
    }
    state.hidden_session_ids.remove(&id);
    upsert_session(state, session);
    switch_to_session(state, &id);
    let tab = state.session_tabs.entry(id).or_default();
    tab.entries.clear();
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.hydrated = true;
    clear_streaming_tab(tab);
    state.modal = None;
    state.error_banner = None;
}

pub fn apply_opened_session(state: &mut AppState, session: SessionSummaryDto) {
    let id = session.id.clone();
    state.hidden_session_ids.remove(&id);
    upsert_session(state, session);
    if let Some(tab) = state.session_tabs.get_mut(&id) {
        tab.hydrated = false;
    }
    switch_to_session(state, &id);
}

pub fn apply_view_event(state: &mut AppState, event: ViewEvent) -> EventEffect {
    match event {
        ViewEvent::SessionOpened { session } => {
            let id = session.id.clone();
            state.hidden_session_ids.remove(&id);
            upsert_session(state, session);
            if state.active_session_id.is_none() {
                state.active_session_id = Some(id);
            }
            EventEffect::None
        }
        ViewEvent::SessionUpdated { session } => {
            let id = session.id.clone();
            let became_idle = session.status == SessionStatus::Idle && !session.user_shell_running;
            upsert_session(state, session);
            if became_idle {
                drain_next_queued_if_idle(state, &id);
            }
            EventEffect::None
        }
        ViewEvent::SessionActivated { session_id } => {
            state.hidden_session_ids.remove(&session_id);
            switch_to_session(state, &session_id);
            EventEffect::None
        }
        ViewEvent::SessionClosed {
            session_id,
            next_active_session_id,
        } => {
            state.sessions.retain(|s| s.id != session_id);
            state.tab_order.retain(|id| id != &session_id);
            state.session_tabs.remove(&session_id);
            state.hidden_session_ids.remove(&session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.active_session_id = next_active_session_id
                    .filter(|id| state.sessions.iter().any(|s| &s.id == id))
                    .or_else(|| state.visible_sessions().first().map(|s| s.id.clone()));
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::SessionReset { session_id } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.entries.clear();
            tab.entries_version = tab.entries_version.wrapping_add(1);
            tab.pending_submit = None;
            clear_streaming_tab(tab);
            tab.pending_approvals.clear();
            tab.pending_questions.clear();
            tab.usage = UsageDto::default();
            tab.metrics = TurnMetricsDto::default();
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.reset_confirm_pending = false;
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::SessionRewound {
            session_id,
            turn_id,
            scope,
            restored,
            error,
            entries,
            restored_prompt,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if let Some(new_entries) = entries {
                tab.entries = new_entries;
            }
            tab.entries_version = tab.entries_version.wrapping_add(1);
            clear_streaming_tab(tab);
            let msg = if let Some(err) = error {
                format!("Rewind error: {err}")
            } else {
                format!(
                    "Rewound ({scope:?}) to {turn_id}; restored {} file(s).",
                    restored.len()
                )
            };
            push_local_system(tab, msg);
            if state.active_session_id.as_deref() == Some(session_id.as_str())
                && let Some(prompt) = restored_prompt
            {
                state.draft.replace_text(prompt.clone());
                if let Some(tab) = state.active_tab_mut() {
                    tab.draft_text = prompt;
                }
            }
            EventEffect::None
        }
        ViewEvent::TurnStarted {
            session_id,
            turn_id,
            model,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::Running;
                session.active_turn_id = Some(turn_id);
                if !model.is_empty() {
                    session.model = model;
                }
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            clear_streaming_tab(tab);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::UserPromptSubmitted {
            session_id,
            entry_id,
            text,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_submit = None;
            if !tab.entries.iter().any(|e| entry_id_of(e) == entry_id) {
                tab.entries.push(TranscriptEntryDto::User {
                    id: entry_id,
                    turn_id: None,
                    text,
                });
                tab.entries_version = tab.entries_version.wrapping_add(1);
            }
            EventEffect::None
        }
        ViewEvent::TextDelta {
            session_id,
            item_id,
            delta,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if tab.streaming_item_id.as_deref() != Some(item_id.as_str()) {
                tab.streaming_item_id = Some(item_id);
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
            }
            tab.streaming_text.push_str(&delta);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::ReasoningDelta {
            session_id,
            item_id,
            delta,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if tab.streaming_item_id.as_deref() != Some(item_id.as_str()) {
                tab.streaming_item_id = Some(item_id);
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
            }
            tab.streaming_reasoning.push_str(&delta);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::StreamDiscarded { session_id } => {
            if let Some(tab) = state.session_tabs.get_mut(&session_id) {
                clear_streaming_tab(tab);
            }
            EventEffect::None
        }
        ViewEvent::EntryCompleted { session_id, entry } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if matches!(entry, TranscriptEntryDto::Assistant { .. }) {
                clear_streaming_tab(tab);
            }
            upsert_entry(&mut tab.entries, entry);
            tab.entries_version = tab.entries_version.wrapping_add(1);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::EntryUpdated { session_id, entry } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            upsert_entry(&mut tab.entries, entry);
            tab.entries_version = tab.entries_version.wrapping_add(1);
            EventEffect::None
        }
        ViewEvent::ApprovalRequested {
            session_id,
            request,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::WaitingApproval;
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if !tab
                .pending_approvals
                .iter()
                .any(|a| a.request_id == request.request_id)
            {
                tab.pending_approvals.push(request);
            }
            mark_unread_if_bg(state, &session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::ApprovalResolved {
            session_id,
            request_id,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_approvals.retain(|a| a.request_id != request_id);
            if matches!(
                &state.modal,
                Some(ActiveModal::Approval { session_id: sid, request, .. })
                if sid == &session_id && request.request_id == request_id
            ) {
                state.modal = None;
            }
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::QuestionAsked {
            session_id,
            request,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::WaitingQuestion;
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if !tab
                .pending_questions
                .iter()
                .any(|q| q.request_id == request.request_id)
            {
                tab.pending_questions.push(request);
            }
            mark_unread_if_bg(state, &session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::QuestionResolved {
            session_id,
            request_id,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_questions.retain(|q| q.request_id != request_id);
            if matches!(
                &state.modal,
                Some(ActiveModal::Question { session_id: sid, request, .. })
                if sid == &session_id && request.request_id == request_id
            ) {
                state.modal = None;
            }
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::UsageUpdated {
            session_id,
            usage,
            metrics,
        } => {
            let tab = state.session_tabs.entry(session_id).or_default();
            tab.usage = usage;
            tab.metrics = metrics;
            EventEffect::None
        }
        ViewEvent::ContextUpdated {
            session_id,
            breakdown,
        } => {
            let tab = state.session_tabs.entry(session_id).or_default();
            tab.context = Some(breakdown.clone());
            if let Some(ActiveModal::WorkspacePanel {
                tab: WorkspacePanelTab::Context,
                context,
                ..
            }) = state.modal.as_mut()
            {
                *context = Some(Box::new(breakdown));
            }
            EventEffect::None
        }
        ViewEvent::LoopUpdated {
            session_id,
            state: loop_state,
        } => {
            let tab = state.session_tabs.entry(session_id).or_default();
            tab.loop_state = Some(loop_state.clone());
            if let Some(ActiveModal::WorkspacePanel {
                tab: WorkspacePanelTab::Loop,
                loop_state: panel_state,
                ..
            }) = state.modal.as_mut()
            {
                *panel_state = Some(Box::new(loop_state));
            }
            EventEffect::None
        }
        ViewEvent::TurnCompleted {
            session_id,
            turn_id: _,
            reason: _,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::Idle;
                session.active_turn_id = None;
                session.active_tool = None;
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            clear_streaming_tab(tab);
            mark_unread_if_bg(state, &session_id);
            drain_next_queued_if_idle(state, &session_id);
            EventEffect::None
        }
        ViewEvent::Error {
            session_id,
            code,
            message,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_submit = None;
            clear_streaming_tab(tab);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.error_banner = Some(format!("{code}: {message}"));
            }
            EventEffect::None
        }
        ViewEvent::FileChangesUpdated { changes } => {
            state.file_changes = changes;
            sync_diff_modal_on_changes(state);
            EventEffect::None
        }
        ViewEvent::Reloaded {
            host_info,
            warnings,
        } => {
            state.host_info = *host_info;
            let notice = if !warnings.is_empty() {
                format!("Reload warnings: {}", warnings.join("; "))
            } else {
                "Reloaded configuration".to_owned()
            };
            set_status_notice(state, notice.clone());
            if let Some(ActiveModal::Settings(form)) = &mut state.modal {
                let prev_section = form.section;
                let prev_filter = form.provider_filter.clone();
                *form = SettingsFormState::from_host(&state.host_info);
                form.section = prev_section;
                form.provider_filter = prev_filter;
                form.status = Some(notice);
            }
            EventEffect::None
        }
        ViewEvent::ProvidersUpdated => {
            if let Some(t) = crate::transport::current() {
                t.fetch_providers();
            }
            EventEffect::None
        }
        ViewEvent::ChatGptLoginUpdated {
            attempt_id,
            provider_id,
            state: login_state,
            url,
            account,
            message,
        } => {
            let mut completed_return = None;
            if let Some(ActiveModal::ChatGptLogin {
                provider_id: pid,
                attempt_id: att,
                url: modal_url,
                state: st,
                error,
                busy,
                return_to,
                ..
            }) = &mut state.modal
                && pid == &provider_id
                && (att.is_none() || att.as_deref() == Some(attempt_id.as_str()))
            {
                *att = Some(attempt_id);
                if let Some(u) = url {
                    *modal_url = Some(u);
                }
                *st = Some(login_state);
                match login_state {
                    ChatGptLoginStateDto::Started => {
                        *busy = false;
                        *error = None;
                    }
                    ChatGptLoginStateDto::Completed => {
                        completed_return = Some((*return_to, account));
                    }
                    ChatGptLoginStateDto::Failed => {
                        *busy = false;
                        *error = message;
                    }
                    ChatGptLoginStateDto::Cancelled => {
                        let origin = *return_to;
                        return_from_provider_modal(state, origin);
                    }
                }
            }
            if let Some((origin, account)) = completed_return {
                let acct = account.unwrap_or_else(|| "signed in".to_owned());
                set_status_notice(state, format!("Connected {provider_id} ({acct})"));
                if origin == ProviderModalReturn::SettingsConnections {
                    open_settings(state, Some(SettingsSection::Connections));
                } else {
                    open_model_input_for(state, Some(provider_id), origin);
                }
                if let Some(t) = crate::transport::current() {
                    t.fetch_providers();
                }
            }
            EventEffect::None
        }
        ViewEvent::ResyncRequired => {
            state.connection = ConnectionState::Resyncing;
            EventEffect::Resync
        }
        ViewEvent::HostDisconnected => {
            state.connection = ConnectionState::Disconnected;
            EventEffect::None
        }
    }
}

pub fn apply_reload_response(state: &mut AppState, resp: ReloadResponse) {
    state.host_info = resp.host_info;
    let notice = if resp.warnings.is_empty() {
        "Reloaded configuration".to_owned()
    } else {
        format!("Reloaded with warnings: {}", resp.warnings.join("; "))
    };
    set_status_notice(state, notice.clone());
    if let Some(ActiveModal::Settings(form)) = &mut state.modal {
        let prev_section = form.section;
        let prev_filter = form.provider_filter.clone();
        *form = SettingsFormState::from_host(&state.host_info);
        form.section = prev_section;
        form.provider_filter = prev_filter;
        form.status = Some(notice);
    }
}

pub fn apply_host_action_result(state: &mut AppState, text: String) {
    let Some(session_id) = state.active_session_id.clone() else {
        set_status_notice(state, text);
        return;
    };
    let tab = state.session_tabs.entry(session_id).or_default();
    let id = format!("host-action-{}-{}", tab.entries.len(), state.notice_seq);
    state.notice_seq = state.notice_seq.wrapping_add(1);
    tab.entries.push(TranscriptEntryDto::System { id, text });
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.unread = false;
}
