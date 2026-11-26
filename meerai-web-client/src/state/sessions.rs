use super::commands::*;
use super::completion::*;
use super::misc::*;
use super::runs::*;
use super::types::*;
use super::views::*;
use super::*;

pub fn activate_and_switch_session(state: &mut AppState, session_id: &str) {
    if !state.sessions.iter().any(|s| s.id == session_id) {
        return;
    }
    switch_to_session(state, session_id);
    if let Some(t) = crate::transport::current() {
        t.activate_session(session_id.to_owned());
    }
}

pub fn switch_to_session(state: &mut AppState, session_id: &str) {
    if !state.sessions.iter().any(|s| s.id == session_id) {
        return;
    }
    state.hidden_session_ids.remove(session_id);
    let changed = state.active_session_id.as_deref() != Some(session_id);
    if let Some(prev) = state.active_session_id.clone()
        && prev != session_id
    {
        let prev_draft = state.draft.text.clone();
        state.session_tabs.entry(prev).or_default().draft_text = prev_draft;
    }
    if changed
        && matches!(
            state.modal,
            Some(
                ActiveModal::Approval { .. }
                    | ActiveModal::Question { .. }
                    | ActiveModal::RewindMenu { .. }
                    | ActiveModal::SubagentOverlay { .. }
            )
        )
    {
        state.modal = None;
    }
    state.reset_confirm_pending = false;
    state.active_session_id = Some(session_id.to_owned());
    let (next_draft, needs_hydrate) = {
        let tab = state.session_tabs.entry(session_id.to_owned()).or_default();
        tab.unread = false;
        (tab.draft_text.clone(), !tab.hydrated)
    };
    state.draft.replace_text(next_draft);
    state.error_banner = None;
    update_completion(state);
    if needs_hydrate && let Some(t) = crate::transport::current() {
        t.fetch_session_detail(session_id.to_owned());
    }
    drain_next_queued_if_idle(state, session_id);
    sync_pending_modal_for_active(state);
}

pub fn select_tab_index(state: &mut AppState, idx: usize) {
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    if let Some(id) = visible_ids.get(idx).cloned() {
        activate_and_switch_session(state, &id);
    }
}

pub fn move_tab_to(state: &mut AppState, from_id: &str, to_id: &str) {
    if from_id == to_id {
        return;
    }
    sync_tab_order(state);
    let Some(from_idx) = state.tab_order.iter().position(|id| id == from_id) else {
        return;
    };
    let Some(to_idx) = state.tab_order.iter().position(|id| id == to_id) else {
        return;
    };
    let item = state.tab_order.remove(from_idx);
    state.tab_order.insert(to_idx, item);
}

pub fn cycle_tab(state: &mut AppState, delta: isize) {
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    if visible_ids.len() < 2 {
        return;
    }
    let cur = state
        .active_session_id
        .as_deref()
        .and_then(|id| visible_ids.iter().position(|x| x == id))
        .unwrap_or(0);
    let len = visible_ids.len() as isize;
    let next = ((cur as isize + delta).rem_euclid(len)) as usize;
    select_tab_index(state, next);
}

pub fn reorder_active_tab(state: &mut AppState, delta: isize) {
    sync_tab_order(state);
    let Some(active) = state.active_session_id.as_deref() else {
        return;
    };
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    let Some(pos) = visible_ids.iter().position(|id| id == active) else {
        return;
    };
    let target = pos as isize + delta;
    if target < 0 || target >= visible_ids.len() as isize {
        return;
    }
    let a = &visible_ids[pos];
    let b = &visible_ids[target as usize];
    if let (Some(ia), Some(ib)) = (
        state.tab_order.iter().position(|x| x == a),
        state.tab_order.iter().position(|x| x == b),
    ) {
        state.tab_order.swap(ia, ib);
    }
}

pub fn detach_tab_view(state: &mut AppState, session_id: &str) {
    let visible = state.visible_sessions();
    if visible.len() <= 1 {
        state.pending_replace_session = Some(session_id.to_owned());
        set_status_notice(state, "Closing the last tab, opening a new session");
        if let Some(t) = crate::transport::current() {
            t.create_session(None);
        }
        return;
    }
    let visible_ids: Vec<String> = visible.into_iter().map(|s| s.id.clone()).collect();
    let next_id = visible_ids
        .iter()
        .position(|id| id == session_id)
        .and_then(|pos| {
            visible_ids
                .get(pos + 1)
                .or_else(|| pos.checked_sub(1).and_then(|i| visible_ids.get(i)))
        })
        .cloned();
    state.hidden_session_ids.insert(session_id.to_owned());
    if state.active_session_id.as_deref() == Some(session_id)
        && let Some(next) = next_id
    {
        activate_and_switch_session(state, &next);
    }
}

pub fn submit_or_queue_composer(state: &mut AppState) {
    if !state.can_submit_composer() {
        return;
    }
    let trimmed = state.draft.text.trim().to_owned();
    state.completion = CompletionState::default();
    state.prompt_history_idx = None;
    record_prompt_history(state, &trimmed);
    state.draft.clear();
    if let Some(tab) = state.active_tab_mut() {
        tab.draft_text.clear();
    }

    if let Some(shell_cmd) = trimmed.strip_prefix('!') {
        execute_user_shell(state, shell_cmd.trim());
        return;
    }

    if trimmed.starts_with('/') {
        execute_slash_command(state, &trimmed);
        return;
    }

    dispatch_prompt_text(state, trimmed);
}

pub fn dispatch_prompt_text(state: &mut AppState, trimmed: String) {
    let mentions = extract_mentions(&trimmed, &state.host_info);
    if !mentions.skills.is_empty() || mentions.agent.is_some() {
        let Some(sid) = state.active_session_id.clone() else {
            state.error_banner = Some("No active session".to_owned());
            return;
        };
        if let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(sid.clone(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(sid.clone(), agent.clone());
            }
        }
        if mentions.mention_only {
            let tab = state.session_tabs.entry(sid).or_default();
            let mut applied = Vec::new();
            for s in mentions.skills {
                applied.push(format!("skill:{s}"));
            }
            if let Some(a) = mentions.agent {
                applied.push(format!("agent:{a}"));
            }
            push_local_system(tab, format!("Applied {}", applied.join(", ")));
            return;
        }
    }

    let Some(sid) = state.active_session_id.clone() else {
        state.error_banner = Some("No active session".to_owned());
        return;
    };
    let prompt = if mentions.skills.is_empty() && mentions.agent.is_none() {
        trimmed.clone()
    } else {
        mentions.cleaned
    };

    if state.is_busy() {
        state
            .session_tabs
            .entry(sid)
            .or_default()
            .pending_queue
            .push_back(prompt);
        set_status_notice(state, "Queued prompt (click item to steer now)");
        return;
    }

    state.sending = true;
    state.error_banner = None;
    state
        .session_tabs
        .entry(sid.clone())
        .or_default()
        .pending_submit = Some(prompt.clone());
    if let Some(t) = crate::transport::current() {
        t.submit_prompt(sid, prompt, Some(trimmed));
    }
}

pub fn open_queued_edit(state: &mut AppState, index: usize) {
    let Some(prompt) = state
        .active_tab()
        .and_then(|tab| tab.pending_queue.get(index))
        .cloned()
    else {
        return;
    };
    state.modal = Some(ActiveModal::QueuedEdit {
        index,
        input: TextBuffer::new(prompt),
        error: None,
    });
}

pub fn commit_queued_edit(state: &mut AppState) {
    let Some(ActiveModal::QueuedEdit { index, input, .. }) = state.modal.clone() else {
        return;
    };
    let text = input.text.trim().to_owned();
    if text.is_empty() {
        if let Some(ActiveModal::QueuedEdit { error, .. }) = &mut state.modal {
            *error = Some("Message is required".to_owned());
        }
        return;
    }
    if let Some(tab) = state.active_tab_mut()
        && let Some(slot) = tab.pending_queue.get_mut(index)
    {
        *slot = text;
        state.modal = None;
        set_status_notice(state, "Queued message updated");
    }
}

pub fn remove_queued(state: &mut AppState, index: usize) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    if let Some(tab) = state.active_tab_mut()
        && index < tab.pending_queue.len()
    {
        tab.pending_queue.remove(index);
        adjust_queued_edit_modal(state, &sid, index);
        set_status_notice(state, "Removed queued message");
    }
}

pub fn steer_or_send_queued(state: &mut AppState, index: usize) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    if let Some(prompt) = state
        .session_tabs
        .get_mut(&sid)
        .and_then(|tab| tab.pending_queue.remove(index))
    {
        adjust_queued_edit_modal(state, &sid, index);
        let mentions = extract_mentions(&prompt, &state.host_info);
        if let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(sid.clone(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(sid.clone(), agent.clone());
            }
        }
        let has_mentions = !mentions.skills.is_empty() || mentions.agent.is_some();
        let clean = if has_mentions {
            mentions.cleaned
        } else {
            prompt.clone()
        };
        if clean.is_empty() {
            let tab = state.session_tabs.entry(sid).or_default();
            push_applied_mentions_notice(tab, &mentions.skills, mentions.agent.as_deref());
            return;
        }
        let display = has_mentions.then_some(prompt);
        if !state.is_busy() {
            state.sending = true;
            state
                .session_tabs
                .entry(sid.clone())
                .or_default()
                .pending_submit = Some(clean.clone());
        }
        let Some(t) = crate::transport::current() else {
            return;
        };
        if state.is_busy() {
            t.steer_prompt(sid, clean, display);
        } else {
            t.submit_prompt(sid, clean, display);
        }
        return;
    }

    if index == 0 && state.is_busy() {
        let trimmed = state.draft.text.trim().to_owned();
        if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('!') {
            return;
        }
        let mentions = extract_mentions(&trimmed, &state.host_info);
        if let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(sid.clone(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(sid.clone(), agent.clone());
            }
        }
        let prompt = if mentions.skills.is_empty() && mentions.agent.is_none() {
            trimmed.clone()
        } else {
            mentions.cleaned
        };
        if prompt.is_empty() {
            return;
        }
        state.completion = CompletionState::default();
        state.prompt_history_idx = None;
        record_prompt_history(state, &trimmed);
        state.draft.clear();
        if let Some(tab) = state.active_tab_mut() {
            tab.draft_text.clear();
        }
        if let Some(t) = crate::transport::current() {
            t.steer_prompt(sid, prompt, Some(trimmed));
        }
    }
}

pub fn drain_next_queued_if_idle(state: &mut AppState, session_id: &str) {
    if state.sending || state.connection == ConnectionState::Disconnected {
        return;
    }
    let is_session_idle = state
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .is_some_and(|s| s.status == SessionStatus::Idle && !s.user_shell_running);
    if !is_session_idle {
        return;
    }
    while let Some(queued_text) = state
        .session_tabs
        .get_mut(session_id)
        .and_then(|tab| tab.pending_queue.pop_front())
    {
        adjust_queued_edit_modal(state, session_id, 0);
        let mentions = extract_mentions(&queued_text, &state.host_info);
        let has_mentions = !mentions.skills.is_empty() || mentions.agent.is_some();
        if has_mentions && let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(session_id.to_owned(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(session_id.to_owned(), agent.clone());
            }
        }
        let prompt = if has_mentions {
            mentions.cleaned
        } else {
            queued_text.clone()
        };
        if prompt.is_empty() {
            if let Some(tab) = state.session_tabs.get_mut(session_id) {
                push_applied_mentions_notice(tab, &mentions.skills, mentions.agent.as_deref());
            }
            continue;
        }
        state.sending = true;
        if let Some(tab) = state.session_tabs.get_mut(session_id) {
            tab.pending_submit = Some(prompt.clone());
        }
        if let Some(t) = crate::transport::current() {
            t.submit_prompt(
                session_id.to_owned(),
                prompt,
                has_mentions.then_some(queued_text),
            );
        }
        break;
    }
}

pub(crate) fn clear_streaming_tab(tab: &mut SessionTabState) {
    tab.streaming_item_id = None;
    tab.streaming_text.clear();
    tab.streaming_reasoning.clear();
}

pub(crate) fn sync_tab_order(state: &mut AppState) {
    let live: HashSet<String> = state.sessions.iter().map(|s| s.id.clone()).collect();
    state.tab_order.retain(|id| live.contains(id));
    for s in &state.sessions {
        if !state.tab_order.contains(&s.id) {
            state.tab_order.push(s.id.clone());
        }
    }
}

pub(crate) fn upsert_session(state: &mut AppState, summary: SessionSummaryDto) {
    let id = summary.id.clone();
    if let Some(existing) = state.sessions.iter_mut().find(|s| s.id == id) {
        *existing = summary;
    } else {
        state.sessions.push(summary);
    }
    if !state.tab_order.contains(&id) {
        state.tab_order.push(id);
    }
}

pub(crate) fn record_prompt_history(state: &mut AppState, prompt: &str) {
    if state.prompt_history.last().map(String::as_str) != Some(prompt) {
        state.prompt_history.push(prompt.to_owned());
        if state.prompt_history.len() > 200 {
            state.prompt_history.remove(0);
        }
    }
}
