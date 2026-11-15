use std::time::Instant;

use meerai_agents::{
    AgentEvent, ErrorCode, ItemDelta, ItemId, ItemStatus, SessionItem, TurnEndReason,
};
use meerai_core::ObservationStatus;
use meerai_session::{
    ContentBlock, InputSource, LoggedEvent, SeqRange, SessionEvent, join_text_blocks,
};

use super::*;

impl AppState {
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TurnStarted { turn_id, model } => {
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
                        }));
                    self.tool_items.insert(id, call_id);
                }
                SessionItem::SubagentRun {
                    id,
                    agent,
                    status,
                    report,
                } => {
                    self.finalize_pending_assistant();
                    let idx = self.transcript.len();
                    self.transcript
                        .push(TranscriptEntry::Subagent(SubagentEntry {
                            id: id.to_string(),
                            agent,
                            status: map_item_status(status),
                            report,
                        }));
                    self.subagent_items.insert(id, idx);
                }
                SessionItem::Compaction { freed_tokens, .. } => {
                    self.transcript.push(TranscriptEntry::System {
                        text: format!("Compaction freed {freed_tokens} tokens"),
                    });
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
                    } else {
                        self.finalize_pending_assistant();
                        self.transcript
                            .push(TranscriptEntry::Subagent(SubagentEntry {
                                id: id_str,
                                agent,
                                status,
                                report,
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
            AgentEvent::ApprovalRequested { request } => match &mut self.modal {
                Some(ActiveModal::Approval { queue, .. }) => queue.push(request),
                _ => {
                    self.modal = Some(ActiveModal::Approval {
                        queue: vec![request],
                        reason: String::new(),
                        editing_reason: false,
                    });
                }
            },
            AgentEvent::ApprovalResolved { request_id } => {
                let mut close = false;
                if let Some(ActiveModal::Approval { queue, .. }) = &mut self.modal {
                    queue.retain(|request| request.request_id != request_id);
                    close = queue.is_empty();
                }
                if close {
                    self.modal = None;
                }
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
                self.metrics.update_speed(usage.completion_tokens);
                self.usage = UsageStats {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
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
            }
        }
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
                entry.status = fallback.clone();
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
    let reverted: Vec<SeqRange> = events
        .iter()
        .filter_map(|logged| match &logged.event {
            SessionEvent::RevertApplied { replaced, .. } => Some(*replaced),
            _ => None,
        })
        .collect();
    let mut transcript = Vec::new();
    for logged in events {
        if reverted.iter().any(|range| range.contains(logged.seq)) {
            continue;
        }
        match &logged.event {
            SessionEvent::UserMessage {
                source: InputSource::User | InputSource::Steering,
                content,
            } => {
                let text = join_text_blocks(content);
                if !text.is_empty() {
                    transcript.push(TranscriptEntry::User { text });
                }
            }
            SessionEvent::AssistantMessage { content, .. } => {
                let text = join_text_blocks(content);
                let reasoning = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Reasoning(text) => Some(text.as_str()),
                        ContentBlock::Text(_) => None,
                    })
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !text.is_empty() || !reasoning.is_empty() {
                    transcript.push(TranscriptEntry::Assistant {
                        text,
                        reasoning,
                        streaming: false,
                        reasoning_expanded: false,
                    });
                }
            }
            SessionEvent::ToolCall {
                call_id,
                name,
                args,
            } => {
                transcript.push(TranscriptEntry::ToolCall(ToolCallEntry {
                    call_id: call_id.to_string(),
                    name: name.clone(),
                    args: args.clone(),
                    status: ToolCallStatus::Running,
                    output: None,
                }));
            }
            SessionEvent::ToolResult { call_id, output } => {
                let call_id = call_id.to_string();
                let status = observation_status(output.status);
                let rendered = output.render();
                let existing = transcript.iter_mut().rev().find_map(|entry| match entry {
                    TranscriptEntry::ToolCall(entry) if entry.call_id == call_id => Some(entry),
                    _ => None,
                });
                if let Some(entry) = existing {
                    entry.status = status;
                    if !rendered.is_empty() {
                        entry.output = Some(rendered);
                    }
                }
            }
            SessionEvent::CompactionSummary { .. } => {
                transcript.push(TranscriptEntry::System {
                    text: "Compacted history".to_string(),
                });
            }
            _ => {}
        }
    }
    for entry in &mut transcript {
        if let TranscriptEntry::ToolCall(entry) = entry
            && entry.status == ToolCallStatus::Running
        {
            entry.status = ToolCallStatus::Cancelled;
            entry.output = None;
        }
    }
    transcript
}

pub fn session_usage_from_events(events: &[LoggedEvent]) -> Option<meerai_agents::Usage> {
    events.iter().rev().find_map(|logged| match &logged.event {
        SessionEvent::UsageUpdated { cumulative, .. } => Some(*cumulative),
        _ => None,
    })
}

fn observation_status(status: ObservationStatus) -> ToolCallStatus {
    match status {
        ObservationStatus::Success => ToolCallStatus::Succeeded,
        ObservationStatus::Error | ObservationStatus::Refused => ToolCallStatus::Failed,
        ObservationStatus::Stopped => ToolCallStatus::Stopped,
    }
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
