use agent_client_protocol::v1::schema::{
    ContentBlock, PermissionOption, PermissionOptionKind, PlanEntry, PlanEntryPriority,
    PlanEntryStatus, RequestPermissionRequest, SessionUpdate, StopReason, ToolCall,
    ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpEvent {
    Connected {
        agent_name: Option<String>,
        agent_version: Option<String>,
        session_id: String,
    },
    TextDelta(String),
    ThoughtDelta(String),
    ToolCall(AcpToolCall),
    ToolCallUpdate(AcpToolCallUpdate),
    Plan(Vec<AcpPlanStep>),
    Usage {
        used: u64,
        size: u64,
    },
    PermissionRequest(AcpPermissionRequest),
    Finished {
        stop_reason: String,
    },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpToolCall {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub raw_input: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpToolCallUpdate {
    pub id: String,
    pub status: String,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPlanStep {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionRequest {
    pub request_id: u64,
    pub tool_call: AcpToolCall,
    pub options: Vec<AcpPermissionOption>,
}

pub(crate) fn event_from_update(update: SessionUpdate) -> Option<AcpEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => text_of(chunk.content).map(AcpEvent::TextDelta),
        SessionUpdate::AgentThoughtChunk(chunk) => {
            text_of(chunk.content).map(AcpEvent::ThoughtDelta)
        }
        SessionUpdate::ToolCall(call) => Some(AcpEvent::ToolCall(tool_call(call))),
        SessionUpdate::ToolCallUpdate(update) => {
            Some(AcpEvent::ToolCallUpdate(tool_call_update(update)))
        }
        SessionUpdate::Plan(plan) => Some(AcpEvent::Plan(
            plan.entries.into_iter().map(plan_step).collect(),
        )),
        SessionUpdate::UsageUpdate(usage) => Some(AcpEvent::Usage {
            used: usage.used,
            size: usage.size,
        }),
        _ => None,
    }
}

pub(crate) fn permission_request(
    request_id: u64,
    request: RequestPermissionRequest,
) -> AcpPermissionRequest {
    AcpPermissionRequest {
        request_id,
        tool_call: tool_call_from_update(request.tool_call),
        options: request.options.into_iter().map(permission_option).collect(),
    }
}

pub(crate) fn stop_reason(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
    .to_string()
}

fn text_of(content: ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text),
        _ => None,
    }
}

fn tool_call(call: ToolCall) -> AcpToolCall {
    AcpToolCall {
        id: call.tool_call_id.to_string(),
        title: call.title,
        kind: tool_kind(call.kind),
        status: tool_status(call.status),
        raw_input: call.raw_input.map(|value| value.to_string()),
    }
}

fn tool_call_from_update(update: ToolCallUpdate) -> AcpToolCall {
    AcpToolCall {
        id: update.tool_call_id.to_string(),
        title: update.fields.title.unwrap_or_default(),
        kind: tool_kind(update.fields.kind.unwrap_or_default()),
        status: tool_status(update.fields.status.unwrap_or_default()),
        raw_input: update.fields.raw_input.map(|value| value.to_string()),
    }
}

fn tool_call_update(update: ToolCallUpdate) -> AcpToolCallUpdate {
    AcpToolCallUpdate {
        id: update.tool_call_id.to_string(),
        status: update.fields.status.map(tool_status).unwrap_or_default(),
        output: update
            .fields
            .content
            .as_ref()
            .and_then(|content| tool_content_text(content)),
    }
}

fn tool_content_text(content: &[ToolCallContent]) -> Option<String> {
    let text: Vec<String> = content
        .iter()
        .filter_map(|block| match block {
            ToolCallContent::Content(content) => text_of(content.content.clone()),
            _ => None,
        })
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

fn plan_step(entry: PlanEntry) -> AcpPlanStep {
    AcpPlanStep {
        content: entry.content,
        status: plan_status(&entry.status),
        priority: plan_priority(&entry.priority),
    }
}

fn permission_option(option: PermissionOption) -> AcpPermissionOption {
    AcpPermissionOption {
        option_id: option.option_id.to_string(),
        name: option.name,
        kind: permission_kind(option.kind),
    }
}

fn permission_kind(kind: PermissionOptionKind) -> AcpPermissionKind {
    match kind {
        PermissionOptionKind::AllowOnce => AcpPermissionKind::AllowOnce,
        PermissionOptionKind::AllowAlways => AcpPermissionKind::AllowAlways,
        PermissionOptionKind::RejectOnce => AcpPermissionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => AcpPermissionKind::RejectAlways,
        _ => AcpPermissionKind::Other,
    }
}

fn tool_kind(kind: ToolKind) -> String {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        _ => "other",
    }
    .to_string()
}

fn tool_status(status: ToolCallStatus) -> String {
    match status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "in_progress",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
        _ => "unknown",
    }
    .to_string()
}

fn plan_status(status: &PlanEntryStatus) -> String {
    match status {
        PlanEntryStatus::Pending => "pending",
        PlanEntryStatus::InProgress => "in_progress",
        PlanEntryStatus::Completed => "completed",
        _ => "unknown",
    }
    .to_string()
}

fn plan_priority(priority: &PlanEntryPriority) -> String {
    match priority {
        PlanEntryPriority::High => "high",
        PlanEntryPriority::Medium => "medium",
        PlanEntryPriority::Low => "low",
        _ => "unknown",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{
        Content, ContentChunk, Plan, PlanEntry, TextContent, ToolCallContent, ToolCallId,
        ToolCallUpdateFields, UsageUpdate,
    };

    fn text_chunk(text: &str) -> ContentChunk {
        ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
    }

    #[test]
    fn agent_message_chunk_maps_to_text_delta() {
        let event = event_from_update(SessionUpdate::AgentMessageChunk(text_chunk("OK")));
        assert_eq!(event, Some(AcpEvent::TextDelta("OK".to_string())));
    }

    #[test]
    fn agent_thought_chunk_maps_to_thought_delta() {
        let event = event_from_update(SessionUpdate::AgentThoughtChunk(text_chunk("thinking")));
        assert_eq!(event, Some(AcpEvent::ThoughtDelta("thinking".to_string())));
    }

    #[test]
    fn tool_call_maps_identity_kind_and_status() {
        let call = ToolCall::new(ToolCallId::new("call-1"), "Edit src/lib.rs")
            .kind(ToolKind::Edit)
            .status(ToolCallStatus::InProgress);
        let event = event_from_update(SessionUpdate::ToolCall(call));
        assert_eq!(
            event,
            Some(AcpEvent::ToolCall(AcpToolCall {
                id: "call-1".to_string(),
                title: "Edit src/lib.rs".to_string(),
                kind: "edit".to_string(),
                status: "in_progress".to_string(),
                raw_input: None,
            }))
        );
    }

    #[test]
    fn tool_call_update_maps_status_and_text_output() {
        let fields = ToolCallUpdateFields::new()
            .status(ToolCallStatus::Completed)
            .content(vec![ToolCallContent::Content(Content::new(
                ContentBlock::Text(TextContent::new("wrote 3 lines")),
            ))]);
        let update = ToolCallUpdate::new(ToolCallId::new("call-1"), fields);
        let event = event_from_update(SessionUpdate::ToolCallUpdate(update));
        assert_eq!(
            event,
            Some(AcpEvent::ToolCallUpdate(AcpToolCallUpdate {
                id: "call-1".to_string(),
                status: "completed".to_string(),
                output: Some("wrote 3 lines".to_string()),
            }))
        );
    }

    #[test]
    fn tool_call_update_without_status_keeps_status_empty() {
        let update = ToolCallUpdate::new(ToolCallId::new("call-2"), ToolCallUpdateFields::new());
        let event = event_from_update(SessionUpdate::ToolCallUpdate(update));
        assert_eq!(
            event,
            Some(AcpEvent::ToolCallUpdate(AcpToolCallUpdate {
                id: "call-2".to_string(),
                status: String::new(),
                output: None,
            }))
        );
    }

    #[test]
    fn plan_maps_every_entry() {
        let plan = Plan::new(vec![
            PlanEntry::new(
                "read the crate",
                PlanEntryPriority::High,
                PlanEntryStatus::Completed,
            ),
            PlanEntry::new(
                "write the bridge",
                PlanEntryPriority::Low,
                PlanEntryStatus::InProgress,
            ),
        ]);
        let event = event_from_update(SessionUpdate::Plan(plan));
        assert_eq!(
            event,
            Some(AcpEvent::Plan(vec![
                AcpPlanStep {
                    content: "read the crate".to_string(),
                    status: "completed".to_string(),
                    priority: "high".to_string(),
                },
                AcpPlanStep {
                    content: "write the bridge".to_string(),
                    status: "in_progress".to_string(),
                    priority: "low".to_string(),
                },
            ]))
        );
    }

    #[test]
    fn usage_update_maps_context_counts() {
        let event = event_from_update(SessionUpdate::UsageUpdate(UsageUpdate::new(16159, 1048576)));
        assert_eq!(
            event,
            Some(AcpEvent::Usage {
                used: 16159,
                size: 1048576,
            })
        );
    }

    #[test]
    fn permission_kinds_map_by_kind() {
        for (kind, expected) in [
            (
                PermissionOptionKind::AllowOnce,
                AcpPermissionKind::AllowOnce,
            ),
            (
                PermissionOptionKind::AllowAlways,
                AcpPermissionKind::AllowAlways,
            ),
            (
                PermissionOptionKind::RejectOnce,
                AcpPermissionKind::RejectOnce,
            ),
            (
                PermissionOptionKind::RejectAlways,
                AcpPermissionKind::RejectAlways,
            ),
        ] {
            assert_eq!(permission_kind(kind), expected);
        }
    }

    #[test]
    fn permission_request_maps_tool_call_and_options() {
        let fields = ToolCallUpdateFields::new()
            .title("Create notes.txt")
            .kind(ToolKind::Edit)
            .status(ToolCallStatus::Pending);
        let request = RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new(ToolCallId::new("call-9"), fields),
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        );
        assert_eq!(
            permission_request(7, request),
            AcpPermissionRequest {
                request_id: 7,
                tool_call: AcpToolCall {
                    id: "call-9".to_string(),
                    title: "Create notes.txt".to_string(),
                    kind: "edit".to_string(),
                    status: "pending".to_string(),
                    raw_input: None,
                },
                options: vec![
                    AcpPermissionOption {
                        option_id: "allow".to_string(),
                        name: "Allow".to_string(),
                        kind: AcpPermissionKind::AllowOnce,
                    },
                    AcpPermissionOption {
                        option_id: "deny".to_string(),
                        name: "Deny".to_string(),
                        kind: AcpPermissionKind::RejectOnce,
                    },
                ],
            }
        );
    }

    #[test]
    fn stop_reasons_map_to_protocol_names() {
        assert_eq!(stop_reason(&StopReason::EndTurn), "end_turn");
        assert_eq!(stop_reason(&StopReason::Cancelled), "cancelled");
        assert_eq!(stop_reason(&StopReason::MaxTokens), "max_tokens");
        assert_eq!(
            stop_reason(&StopReason::MaxTurnRequests),
            "max_turn_requests"
        );
        assert_eq!(stop_reason(&StopReason::Refusal), "refusal");
    }

    #[test]
    fn unrelated_updates_are_ignored() {
        let mode = agent_client_protocol::v1::schema::CurrentModeUpdate::new(
            agent_client_protocol::v1::schema::SessionModeId::new("plan"),
        );
        assert_eq!(
            event_from_update(SessionUpdate::CurrentModeUpdate(mode)),
            None
        );
    }
}
