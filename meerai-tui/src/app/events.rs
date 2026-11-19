use std::time::Instant;

use meerai_agents::events::RequestId;
use meerai_agents::{
    AgentEvent, ErrorCode, ItemDelta, ItemId, ItemStatus, SessionItem, TurnEndReason,
};
use meerai_session::replay::{ReplayEntry, ReplaySubagent, ReplayToolCall, replay_from_events};
use meerai_session::{LoggedEvent, SessionEvent};

pub use meerai_session::replay::session_usage_from_events;

use super::*;

pub fn user_turn_map(
    transcript: &[TranscriptEntry],
    events: &[LoggedEvent],
) -> std::collections::HashMap<usize, TurnId> {
    let user_indices: Vec<usize> = transcript
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, TranscriptEntry::User { .. }))
        .map(|(idx, _)| idx)
        .collect();
    let mut map = std::collections::HashMap::new();
    let mut current: Option<TurnId> = None;
    let mut ordinal = 0usize;
    for logged in events {
        match &logged.event {
            SessionEvent::TurnStarted { turn_id, .. } => current = Some(turn_id.clone()),
            SessionEvent::UserMessage { .. } => {
                if let (Some(turn), Some(idx)) = (current.clone(), user_indices.get(ordinal)) {
                    map.insert(*idx, turn);
                }
                ordinal += 1;
            }
            _ => {}
        }
    }
    map
}

impl AppState {
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        let tab_id = self.tab().tab_id;
        self.handle_tab_agent_event(tab_id, event);
    }

    pub fn handle_tab_agent_event(&mut self, tab_id: u64, event: AgentEvent) {
        let Some(tab_idx) = self.tab_index_by_id(tab_id) else {
            return;
        };
        let is_active = tab_idx == self.active_tab;

        match event {
            AgentEvent::ApprovalRequested { request } => {
                if is_active {
                    match &mut self.modal {
                        Some(ActiveModal::Approval { queue, .. }) => queue.push(request),
                        None => {
                            self.modal = Some(ActiveModal::Approval {
                                queue: vec![request],
                                reason: String::new(),
                                editing_reason: false,
                            });
                        }
                        Some(_) => {
                            self.tabs[tab_idx].pending_approvals.push(request);
                        }
                    }
                } else {
                    self.tabs[tab_idx].pending_approvals.push(request);
                }
            }
            AgentEvent::RewindCompleted {
                restored, error, ..
            } => {
                if is_active {
                    self.status_notice = Some(match error {
                        Some(error) => format!("Rewind failed: {error}"),
                        None => format!("Rewind restored {} file(s)", restored.len()),
                    });
                }
            }
            AgentEvent::QuestionAsked { request } => {
                self.tabs[tab_idx]
                    .transcript
                    .push(TranscriptEntry::Question {
                        request_id: request.request_id.to_string(),
                        questions: request.questions.clone(),
                        answers: None,
                        resolved: false,
                    });
                self.tabs[tab_idx].transcript_version =
                    self.tabs[tab_idx].transcript_version.wrapping_add(1);
                if is_active && self.modal.is_none() {
                    self.open_question(request);
                } else {
                    self.tabs[tab_idx].pending_questions.push(request);
                }
            }
            AgentEvent::QuestionResolved { request_id } => {
                let id = request_id.to_string();
                if let Some(TranscriptEntry::Question { resolved, .. }) = self.tabs[tab_idx]
                    .transcript
                    .iter_mut()
                    .rev()
                    .find(|entry| {
                        matches!(
                            entry,
                            TranscriptEntry::Question { request_id, .. } if *request_id == id
                        )
                    })
                {
                    *resolved = true;
                }
                self.tabs[tab_idx].transcript_version =
                    self.tabs[tab_idx].transcript_version.wrapping_add(1);
                self.tabs[tab_idx]
                    .pending_questions
                    .retain(|request| request.request_id != request_id);
                if is_active {
                    let close = matches!(
                        &self.modal,
                        Some(ActiveModal::Question { request, .. })
                            if request.request_id == request_id
                    );
                    if close {
                        self.modal = None;
                        self.open_pending_question();
                    }
                }
            }
            AgentEvent::ApprovalResolved { request_id } => {
                self.tabs[tab_idx]
                    .pending_approvals
                    .retain(|request| request.request_id != request_id);
                if is_active {
                    let mut close = false;
                    if let Some(ActiveModal::Approval { queue, .. }) = &mut self.modal {
                        queue.retain(|request| request.request_id != request_id);
                        close = queue.is_empty();
                    }
                    if close {
                        self.modal = None;
                    }
                }
            }
            other => {
                let mark_unread = !is_active
                    && matches!(
                        other,
                        AgentEvent::TurnCompleted { .. }
                            | AgentEvent::Error { .. }
                            | AgentEvent::ItemCompleted { .. }
                    );
                let before_queued = self.tabs[tab_idx].queued.len();
                self.tabs[tab_idx].apply_agent_event(other);
                let after_queued = self.tabs[tab_idx].queued.len();
                if is_active
                    && after_queued < before_queued
                    && let Some(ActiveModal::QueuedEdit { index, .. }) = &mut self.modal
                {
                    let drained = before_queued - after_queued;
                    if *index < drained {
                        self.modal = None;
                        self.status_notice = Some("Queued message was already sent".to_string());
                    } else {
                        *index -= drained;
                    }
                }
                if mark_unread {
                    self.tabs[tab_idx].has_unread = true;
                }
            }
        }
    }
}

impl AppState {
    pub(super) fn open_question(&mut self, request: QuestionRequest) {
        let answers = vec![Vec::new(); request.questions.len()];
        let custom = vec![String::new(); request.questions.len()];
        self.modal = Some(ActiveModal::Question {
            request,
            answers,
            custom,
            editing_custom: false,
            current: 0,
            focus: 0,
        });
    }

    pub(super) fn record_question_answer(
        &mut self,
        request_id: &RequestId,
        answers: Option<Vec<QuestionAnswer>>,
    ) {
        let request_id = request_id.to_string();
        if let Some(TranscriptEntry::Question {
            answers: slot,
            resolved,
            ..
        }) = self.tab_mut().transcript.iter_mut().rev().find(|entry| {
            matches!(
                entry,
                TranscriptEntry::Question { request_id: id, .. } if *id == request_id
            )
        }) {
            *slot = answers;
            *resolved = true;
        }
    }

    pub(super) fn open_pending_question(&mut self) {
        if self.modal.is_some() {
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        if let Some(request) = tab.pending_questions.first().cloned() {
            tab.pending_questions.remove(0);
            self.open_question(request);
        }
    }
}

impl SessionTab {
    pub(super) fn apply_agent_event(&mut self, event: AgentEvent) {
        self.transcript_version = self.transcript_version.wrapping_add(1);
        match event {
            AgentEvent::TurnStarted { turn_id, model } => {
                if let Some(idx) = self
                    .transcript
                    .iter()
                    .rposition(|entry| matches!(entry, TranscriptEntry::User { .. }))
                    && !self.user_turns.contains_key(&idx)
                {
                    self.user_turns.insert(idx, turn_id.clone());
                }
                self.is_running = true;
                self.turn_id = Some(turn_id);
                self.usage = UsageStats::default();
                self.metrics.turn_started_at = Some(Instant::now());
                self.metrics.first_token_at = None;
                let display_model = model
                    .split_once("::")
                    .map(|(_, m)| m.to_string())
                    .unwrap_or(model);
                self.model = display_model;
                self.active_tool = None;
                self.pending_assistant_idx = None;
                self.assistant_items.clear();
                self.tool_items.clear();
                self.subagent_items.clear();
            }
            AgentEvent::RecoveryStarted { attempt } => {
                self.transcript.push(TranscriptEntry::System {
                    text: format!("Recovery attempt {attempt} after a failed turn."),
                });
            }
            AgentEvent::ItemStarted { item } => match item {
                SessionItem::AgentMessage { id, .. } => {
                    let idx = self.ensure_pending_assistant();
                    self.assistant_items.insert(id, idx);
                }
                SessionItem::Reasoning { id, summary } => {
                    let idx = self.ensure_pending_assistant();
                    self.assistant_items.insert(id, idx);
                    if !summary.is_empty()
                        && let Some(TranscriptEntry::Assistant { reasoning, .. }) =
                            self.transcript.get_mut(idx)
                    {
                        reasoning.push_str(&summary);
                    }
                }
                SessionItem::ToolCall {
                    id,
                    call_id,
                    name,
                    args,
                    status,
                    ..
                } => {
                    self.finalize_pending_assistant();
                    self.active_tool = Some(name.clone());
                    let call_id = call_id.to_string();
                    self.transcript
                        .push(TranscriptEntry::ToolCall(ToolCallEntry {
                            call_id: call_id.clone(),
                            name,
                            args,
                            status: map_item_status(status),
                            output: None,
                            expanded: false,
                        }));
                    self.tool_items.insert(id, call_id);
                }
                SessionItem::SubagentRun {
                    id,
                    agent,
                    status,
                    report,
                    child_session_id,
                } => {
                    self.finalize_pending_assistant();
                    let idx = self.transcript.len();
                    self.transcript
                        .push(TranscriptEntry::Subagent(SubagentEntry {
                            id: id.to_string(),
                            agent,
                            status: map_item_status(status),
                            report,
                            child_session_id,
                            expanded: false,
                            nested: None,
                            nested_refreshed_at: None,
                            nested_loading: false,
                        }));
                    self.subagent_items.insert(id, idx);
                }
                _ => {}
            },
            AgentEvent::ItemDelta { item_id, delta } => match delta {
                ItemDelta::AgentText(chunk) => {
                    self.metrics.record_first_token();
                    let idx = self.assistant_item_idx(item_id);
                    if let Some(TranscriptEntry::Assistant { text, .. }) =
                        self.transcript.get_mut(idx)
                    {
                        text.push_str(&chunk);
                    }
                }
                ItemDelta::ReasoningSummary(chunk) => {
                    self.metrics.record_first_token();
                    let idx = self.assistant_item_idx(item_id);
                    if let Some(TranscriptEntry::Assistant { reasoning, .. }) =
                        self.transcript.get_mut(idx)
                    {
                        reasoning.push_str(&chunk);
                    }
                }
                ItemDelta::StatusChanged(status) => {
                    let mapped = map_item_status(status);
                    if let Some(call_id) = self.tool_items.get(&item_id).cloned()
                        && let Some(entry) = self.find_tool_call_mut(&call_id)
                    {
                        entry.status = mapped;
                    } else if let Some(&idx) = self.subagent_items.get(&item_id)
                        && let Some(TranscriptEntry::Subagent(entry)) = self.transcript.get_mut(idx)
                        && entry.id == item_id.as_str()
                    {
                        entry.status = mapped;
                    }
                }
                ItemDelta::CommandOutput(_) | ItemDelta::PlanUpdated(_) => {}
            },
            AgentEvent::ItemCompleted { item } => match item {
                SessionItem::AgentMessage { id, text } => {
                    let idx = self
                        .assistant_items
                        .remove(&id)
                        .or(self.pending_assistant_idx);
                    if let Some(idx) = idx {
                        self.pending_assistant_idx = None;
                        if let Some(TranscriptEntry::Assistant {
                            text: existing,
                            streaming,
                            ..
                        }) = self.transcript.get_mut(idx)
                        {
                            *streaming = false;
                            if !text.is_empty() {
                                *existing = text;
                            }
                        }
                    } else if !text.is_empty() {
                        self.transcript.push(TranscriptEntry::Assistant {
                            text,
                            reasoning: String::new(),
                            streaming: false,
                            reasoning_expanded: false,
                        });
                    }
                }
                SessionItem::Reasoning { id, summary } => {
                    let idx = self
                        .assistant_items
                        .remove(&id)
                        .or(self.pending_assistant_idx);
                    if !summary.is_empty() {
                        if let Some(idx) = idx {
                            if let Some(TranscriptEntry::Assistant { reasoning, .. }) =
                                self.transcript.get_mut(idx)
                            {
                                *reasoning = summary;
                            }
                        } else {
                            self.transcript.push(TranscriptEntry::Assistant {
                                text: String::new(),
                                reasoning: summary,
                                streaming: false,
                                reasoning_expanded: false,
                            });
                        }
                    }
                }
                SessionItem::ToolCall {
                    id,
                    call_id,
                    name,
                    status,
                    output,
                    ..
                } => {
                    let call_id = self
                        .tool_items
                        .remove(&id)
                        .unwrap_or_else(|| call_id.to_string());
                    let status = map_completed_item_status(status);
                    if let Some(entry) = self.find_tool_call_mut(&call_id) {
                        entry.status = status;
                        if output.is_some() {
                            entry.output = output;
                        }
                    } else {
                        self.transcript
                            .push(TranscriptEntry::ToolCall(ToolCallEntry {
                                call_id,
                                name: name.clone(),
                                args: String::new(),
                                status,
                                output,
                                expanded: false,
                            }));
                    }
                    if self.active_tool.as_deref() == Some(name.as_str()) {
                        self.active_tool = None;
                    }
                }
                SessionItem::SubagentRun {
                    id,
                    agent,
                    status,
                    report,
                    child_session_id,
                } => {
                    let status = map_completed_item_status(status);
                    let id_str = id.to_string();
                    let target_idx = self
                        .subagent_items
                        .remove(&id)
                        .filter(|&i| {
                            matches!(
                                self.transcript.get(i),
                                Some(TranscriptEntry::Subagent(entry)) if entry.id == id_str
                            )
                        })
                        .or_else(|| {
                            self.transcript.iter().rposition(|entry| match entry {
                                TranscriptEntry::Subagent(entry) => entry.id == id_str,
                                _ => false,
                            })
                        });
                    if let Some(TranscriptEntry::Subagent(entry)) =
                        target_idx.and_then(|i| self.transcript.get_mut(i))
                    {
                        entry.agent = agent;
                        entry.status = status;
                        if report.is_some() {
                            entry.report = report;
                        }
                        if child_session_id.is_some() {
                            entry.child_session_id = child_session_id;
                        }
                    } else {
                        self.finalize_pending_assistant();
                        self.transcript
                            .push(TranscriptEntry::Subagent(SubagentEntry {
                                id: id_str,
                                agent,
                                status,
                                report,
                                child_session_id,
                                expanded: false,
                                nested: None,
                                nested_refreshed_at: None,
                                nested_loading: false,
                            }));
                    }
                }
                SessionItem::Compaction { freed_tokens, .. } => {
                    self.transcript.push(TranscriptEntry::System {
                        text: format!("Compaction freed {freed_tokens} tokens"),
                    });
                }
                _ => {}
            },
            AgentEvent::ApprovalRequested { .. } | AgentEvent::ApprovalResolved { .. } => {}
            AgentEvent::QuestionAsked { .. } | AgentEvent::QuestionResolved { .. } => {}
            AgentEvent::RewindCompleted { .. } => {}
            AgentEvent::HookCompleted {
                event,
                decision,
                reason,
            } => {
                let mut text = format!("hook {event}: {decision}");
                if let Some(reason) = reason {
                    text.push_str(" — ");
                    text.extend(reason.chars().take(200));
                }
                self.transcript.push(TranscriptEntry::System { text });
            }
            AgentEvent::UsageUpdated {
                usage, cumulative, ..
            } => {
                let session_prompt = cumulative.prompt_tokens.unwrap_or(0).max(0) as u64;
                let session_completion = cumulative.completion_tokens.unwrap_or(0).max(0) as u64;
                let session_total = cumulative
                    .total_tokens
                    .map(|t| t.max(0) as u64)
                    .unwrap_or(session_prompt + session_completion);
                self.metrics.session_prompt_tokens = session_prompt;
                self.metrics.session_completion_tokens = session_completion;
                self.metrics.session_total_tokens = session_total;
                self.metrics.session_cached_tokens =
                    cumulative.cached_tokens.unwrap_or(0).max(0) as u64;
                self.metrics.update_speed(usage.completion_tokens);
                self.usage = UsageStats {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                    cached_tokens: usage.cached_tokens,
                };
            }
            AgentEvent::TurnCompleted { reason, .. } => {
                self.is_running = false;
                self.active_tool = None;
                self.turn_id = None;
                self.metrics.turn_count += 1;
                if let Some(ms) = self.metrics.elapsed_ms() {
                    self.metrics.last_turn_ms = Some(ms);
                }
                self.metrics.turn_started_at = None;
                match reason {
                    TurnEndReason::Completed => {
                        self.metrics.update_speed(self.usage.completion_tokens);
                        self.finalize_pending_assistant();
                        self.finalize_pending_subagents(ToolCallStatus::Failed);
                    }
                    TurnEndReason::Aborted { .. } => {
                        self.finalize_pending_assistant();
                        self.finalize_pending_subagents(ToolCallStatus::Cancelled);
                        self.transcript.push(TranscriptEntry::System {
                            text: "Turn cancelled.".to_string(),
                        });
                    }
                    TurnEndReason::Failed { error } => {
                        self.finalize_pending_assistant();
                        self.finalize_pending_subagents(ToolCallStatus::Failed);
                        if !matches!(self.transcript.last(), Some(TranscriptEntry::Error { .. })) {
                            self.push_error(format!("turn failed: {error}"));
                        }
                    }
                    _ => {
                        self.finalize_pending_assistant();
                        self.finalize_pending_subagents(ToolCallStatus::Failed);
                    }
                }
                self.flush_queued();
            }
            AgentEvent::Error { code, message } => {
                self.is_running = false;
                self.active_tool = None;
                if let Some(ms) = self.metrics.elapsed_ms() {
                    self.metrics.last_turn_ms = Some(ms);
                }
                self.metrics.turn_started_at = None;
                self.finalize_pending_assistant();
                self.finalize_pending_subagents(ToolCallStatus::Failed);
                let text = format!("{} error: {message}", error_code_label(code));
                self.push_error(text);
                self.flush_queued();
            }
        }
    }

    fn flush_queued(&mut self) {
        if self.is_running || self.queued.is_empty() {
            return;
        }
        let next = self.queued.remove(0);
        if self.title.is_none() {
            self.title = normalize_tab_title(next.clone());
        }
        self.transcript
            .push(TranscriptEntry::User { text: next.clone() });
        self.auto_follow = true;
        self.is_running = true;
        let _ = self.cmd_tx.send(AgentCommand::SubmitPrompt(next));
    }

    fn assistant_item_idx(&mut self, item_id: ItemId) -> usize {
        if let Some(idx) = self.assistant_items.get(&item_id) {
            return *idx;
        }
        let idx = self.ensure_pending_assistant();
        self.assistant_items.insert(item_id, idx);
        idx
    }

    fn push_error(&mut self, text: String) {
        let duplicate = matches!(
            self.transcript.last(),
            Some(TranscriptEntry::Error { text: existing, .. }) if *existing == text
        );
        if !duplicate {
            self.transcript.push(TranscriptEntry::Error {
                text,
                expanded: false,
            });
        }
    }

    fn ensure_pending_assistant(&mut self) -> usize {
        if let Some(idx) = self.pending_assistant_idx {
            return idx;
        }
        let idx = self.transcript.len();
        self.transcript.push(TranscriptEntry::Assistant {
            text: String::new(),
            reasoning: String::new(),
            streaming: true,
            reasoning_expanded: false,
        });
        self.pending_assistant_idx = Some(idx);
        idx
    }

    fn finalize_pending_assistant(&mut self) {
        if let Some(idx) = self.pending_assistant_idx.take()
            && let Some(TranscriptEntry::Assistant { streaming, .. }) = self.transcript.get_mut(idx)
        {
            *streaming = false;
        }
    }

    fn finalize_pending_subagents(&mut self, fallback: ToolCallStatus) {
        let indices: Vec<usize> = self.subagent_items.drain().map(|(_, idx)| idx).collect();
        for idx in indices {
            if let Some(TranscriptEntry::Subagent(entry)) = self.transcript.get_mut(idx)
                && matches!(
                    entry.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                )
            {
                entry.status = fallback;
            }
        }
    }

    fn find_tool_call_mut(&mut self, call_id: &str) -> Option<&mut ToolCallEntry> {
        self.transcript
            .iter_mut()
            .rev()
            .find_map(|entry| match entry {
                TranscriptEntry::ToolCall(tc) if tc.call_id == call_id => Some(tc),
                _ => None,
            })
    }
}

fn map_item_status(status: ItemStatus) -> ToolCallStatus {
    match status {
        ItemStatus::Proposed => ToolCallStatus::Proposed,
        ItemStatus::Pending | ItemStatus::InProgress => ToolCallStatus::Running,
        ItemStatus::Succeeded => ToolCallStatus::Succeeded,
        ItemStatus::Failed | ItemStatus::Denied | ItemStatus::Cancelled => ToolCallStatus::Failed,
        ItemStatus::Stopped => ToolCallStatus::Stopped,
    }
}

fn map_completed_item_status(status: ItemStatus) -> ToolCallStatus {
    match map_item_status(status) {
        ToolCallStatus::Proposed => ToolCallStatus::Running,
        other => other,
    }
}

pub fn transcript_from_events(events: &[LoggedEvent]) -> Vec<TranscriptEntry> {
    replay_from_events(events)
        .into_iter()
        .map(TranscriptEntry::from)
        .collect()
}

impl From<ReplayToolCall> for ToolCallEntry {
    fn from(call: ReplayToolCall) -> Self {
        Self {
            call_id: call.call_id,
            name: call.name,
            args: call.args,
            status: call.status,
            output: call.output,
            expanded: false,
        }
    }
}

impl From<ReplaySubagent> for SubagentEntry {
    fn from(subagent: ReplaySubagent) -> Self {
        Self {
            id: subagent.id,
            agent: subagent.agent,
            status: subagent.status,
            report: subagent.report,
            child_session_id: subagent.child_session_id,
            expanded: false,
            nested: None,
            nested_refreshed_at: None,
            nested_loading: false,
        }
    }
}

impl From<ReplayEntry> for TranscriptEntry {
    fn from(entry: ReplayEntry) -> Self {
        match entry {
            ReplayEntry::User { text } => TranscriptEntry::User { text },
            ReplayEntry::Assistant { text, reasoning } => TranscriptEntry::Assistant {
                text,
                reasoning,
                streaming: false,
                reasoning_expanded: false,
            },
            ReplayEntry::ToolCall(call) => TranscriptEntry::ToolCall(call.into()),
            ReplayEntry::Subagent(subagent) => TranscriptEntry::Subagent(subagent.into()),
            ReplayEntry::Question(question) => TranscriptEntry::Question {
                request_id: question.request_id,
                questions: question.questions,
                answers: question.answers,
                resolved: question.resolved,
            },
            ReplayEntry::System { text } => TranscriptEntry::System { text },
        }
    }
}

impl AppState {
    pub(super) fn request_child_session_load(&mut self, item_id: &str) {
        let action = {
            let tab = self.tab_mut();
            let Some(entry) = tab.transcript.iter_mut().find_map(|entry| match entry {
                TranscriptEntry::Subagent(sub) if sub.id == item_id => Some(sub),
                _ => None,
            }) else {
                return;
            };
            if !entry.expanded || entry.nested_loading {
                return;
            }
            if entry.nested.is_some()
                && entry
                    .nested_refreshed_at
                    .is_some_and(|t| t.elapsed() < CHILD_REFRESH_INTERVAL)
            {
                return;
            }
            let Some(child_id) = entry.child_session_id.clone() else {
                return;
            };
            entry.nested_loading = true;
            tab.transcript_version = tab.transcript_version.wrapping_add(1);
            PendingSessionAction::ReadChildSession {
                tab_id: tab.tab_id,
                item_id: item_id.to_string(),
                session_id: SessionId::from_raw(child_id),
            }
        };
        self.pending_session_actions.push(action);
    }

    pub(super) fn queue_child_session_refreshes(&mut self) {
        if !self.tab().is_running {
            return;
        }
        let stale: Vec<String> = self
            .tab()
            .transcript
            .iter()
            .filter_map(|entry| match entry {
                TranscriptEntry::Subagent(sub)
                    if sub.expanded
                        && matches!(
                            sub.status,
                            ToolCallStatus::Running | ToolCallStatus::Proposed
                        )
                        && sub
                            .nested_refreshed_at
                            .is_none_or(|t| t.elapsed() >= CHILD_REFRESH_INTERVAL) =>
                {
                    Some(sub.id.clone())
                }
                _ => None,
            })
            .collect();
        for id in stale {
            self.request_child_session_load(&id);
        }
    }

    pub fn apply_child_session_read(
        &mut self,
        tab_id: u64,
        item_id: &str,
        result: anyhow::Result<Vec<LoggedEvent>>,
    ) {
        let Some(idx) = self.tab_index_by_id(tab_id) else {
            return;
        };
        let tab = &mut self.tabs[idx];
        let Some(entry) = tab.transcript.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::Subagent(sub) if sub.id == item_id => Some(sub),
            _ => None,
        }) else {
            return;
        };
        entry.nested_loading = false;
        entry.nested_refreshed_at = Some(Instant::now());
        if let Ok(events) = result {
            entry.nested = Some(newest_nested(transcript_from_events(&events)));
        }
        tab.transcript_version = tab.transcript_version.wrapping_add(1);
    }
}

const MAX_NESTED_ENTRIES: usize = 60;

fn newest_nested(mut entries: Vec<TranscriptEntry>) -> Vec<TranscriptEntry> {
    if entries.len() > MAX_NESTED_ENTRIES {
        entries.drain(..entries.len() - MAX_NESTED_ENTRIES);
    }
    entries
}

fn error_code_label(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Provider => "provider",
        ErrorCode::Tool => "tool",
        ErrorCode::Config => "config",
        ErrorCode::ContextOverflow => "context overflow",
        ErrorCode::Budget => "budget",
        ErrorCode::LoopDetected => "loop detected",
        ErrorCode::Unsupported => "unsupported",
        ErrorCode::Internal => "internal",
    }
}
