use std::collections::{HashMap, HashSet};
use std::io::{self, Write as _};

use anyhow::Result;
use meerai_agents::events::{
    ApprovalDecision, ApprovalKind, ApprovalRequest, ItemStatus, RequestId,
};
use meerai_agents::{AgentCommand, AgentEvent, ItemId, MultiTurnAgent, SessionItem};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) const EXIT_APPROVAL_REQUIRED: i32 = 2;

pub(crate) fn item_status_label(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Proposed => "proposed",
        ItemStatus::Pending => "pending",
        ItemStatus::InProgress => "in-progress",
        ItemStatus::Succeeded => "succeeded",
        ItemStatus::Failed => "failed",
        ItemStatus::Stopped => "stopped",
        ItemStatus::Denied => "denied",
        ItemStatus::Cancelled => "cancelled",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeadlessOutcome {
    Continue,
    ApprovalRequired,
    SpendDeclined,
    Completed,
    Failed,
}

pub(crate) fn headless_outcome(event: &AgentEvent) -> HeadlessOutcome {
    match event {
        AgentEvent::ApprovalRequested { request } => match request.kind {
            ApprovalKind::Spend => HeadlessOutcome::SpendDeclined,
            _ => HeadlessOutcome::ApprovalRequired,
        },
        AgentEvent::TurnCompleted { reason, .. } => match reason {
            meerai_agents::TurnEndReason::Completed => HeadlessOutcome::Completed,
            _ => HeadlessOutcome::Failed,
        },
        AgentEvent::Error { .. } => HeadlessOutcome::Failed,
        _ => HeadlessOutcome::Continue,
    }
}

pub(crate) fn format_token_summary(usage: &meerai_agents::Usage) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(prompt) = usage.prompt_tokens {
        parts.push(format!("prompt={prompt}"));
    }
    if let Some(completion) = usage.completion_tokens {
        parts.push(format!("completion={completion}"));
    }
    if let Some(total) = usage.total_tokens {
        parts.push(format!("total={total}"));
    }
    (!parts.is_empty()).then(|| format!("tokens: {}", parts.join(" ")))
}

#[derive(Debug, PartialEq)]
pub(crate) enum HeadlessApprovalAction {
    Cancel,
    Decline { request_id: RequestId },
}

impl HeadlessApprovalAction {
    pub(crate) fn for_request(request: &ApprovalRequest) -> Self {
        match request.kind {
            ApprovalKind::Spend => Self::Decline {
                request_id: request.request_id.clone(),
            },
            _ => Self::Cancel,
        }
    }

    pub(crate) fn command(&self) -> AgentCommand {
        match self {
            Self::Cancel => AgentCommand::Cancel,
            Self::Decline { request_id } => AgentCommand::Approve {
                request_id: request_id.clone(),
                decision: ApprovalDecision::Decline,
                scope: None,
            },
        }
    }
}

pub(crate) struct HeadlessExit {
    pub(crate) code: i32,
    pub(crate) approval_cancelled: bool,
    failed: bool,
}

impl HeadlessExit {
    pub(crate) fn new() -> Self {
        Self {
            code: 1,
            approval_cancelled: false,
            failed: false,
        }
    }

    pub(crate) fn mark_failed(&mut self) {
        self.failed = true;
        if !self.approval_cancelled {
            self.code = 1;
        }
    }

    pub(crate) fn observe(&mut self, outcome: HeadlessOutcome) {
        match outcome {
            HeadlessOutcome::Continue => {}
            HeadlessOutcome::SpendDeclined | HeadlessOutcome::Failed => {
                self.mark_failed();
            }
            HeadlessOutcome::ApprovalRequired => {
                self.code = EXIT_APPROVAL_REQUIRED;
                self.approval_cancelled = true;
            }
            HeadlessOutcome::Completed => {
                if !self.approval_cancelled && !self.failed {
                    self.code = 0;
                }
            }
        }
    }
}

pub(crate) async fn run_headless(
    mut agent: MultiTurnAgent,
    prompt: String,
    json: bool,
) -> Result<i32> {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let turn = tokio::spawn(async move {
        agent
            .prompt_streaming_with_commands(
                &prompt,
                &event_tx,
                CancellationToken::new(),
                &mut cmd_rx,
            )
            .await
    });

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut tool_names: HashMap<ItemId, String> = HashMap::new();
    let mut streamed_text: HashSet<ItemId> = HashSet::new();
    let mut exit = HeadlessExit::new();
    let mut line_open = false;
    let mut last_usage: Option<meerai_agents::Usage> = None;

    while let Some(event) = event_rx.recv().await {
        if let AgentEvent::ApprovalRequested { request } = &event {
            let action = HeadlessApprovalAction::for_request(request);
            match &action {
                HeadlessApprovalAction::Decline { .. } => eprintln!(
                    "session token budget reached; raise context.max_session_tokens in settings.yml or unset it"
                ),
                HeadlessApprovalAction::Cancel => {
                    eprintln!("approval required: {}", request.preview.title);
                    eprintln!("rerun with --mode auto-edit or --mode yolo, or use the TUI");
                }
            }
            let _ = cmd_tx.send(action.command());
        }
        exit.observe(headless_outcome(&event));

        if json {
            writeln!(out, "{}", serde_json::to_string(&event)?)?;
            out.flush()?;
            continue;
        }

        match &event {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::ToolCall {
                        id, name, status, ..
                    },
            } => {
                tool_names.insert(id.clone(), name.clone());
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                writeln!(out, "▸ {name} {}", item_status_label(*status))?;
                out.flush()?;
            }
            AgentEvent::ItemDelta {
                item_id,
                delta: meerai_agents::ItemDelta::AgentText(chunk),
            } => {
                if !chunk.is_empty() {
                    streamed_text.insert(item_id.clone());
                    write!(out, "{chunk}")?;
                    out.flush()?;
                    line_open = !chunk.ends_with('\n');
                }
            }
            AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage { id, text },
            } => {
                if !streamed_text.remove(id) && !text.is_empty() {
                    if line_open {
                        writeln!(out)?;
                    }
                    writeln!(out, "{text}")?;
                    out.flush()?;
                    line_open = false;
                }
            }
            AgentEvent::ItemDelta {
                item_id,
                delta: meerai_agents::ItemDelta::StatusChanged(status),
            } => {
                if let Some(name) = tool_names.get(item_id) {
                    if line_open {
                        writeln!(out)?;
                        line_open = false;
                    }
                    writeln!(out, "▸ {name} {}", item_status_label(*status))?;
                    out.flush()?;
                }
            }
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        id, name, status, ..
                    },
            } => {
                tool_names.remove(id);
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                writeln!(out, "▸ {name} {}", item_status_label(*status))?;
                out.flush()?;
            }
            AgentEvent::Error { message, .. } => {
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                eprintln!("error: {message}");
            }
            AgentEvent::TurnCompleted { reason, .. } => {
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                if let meerai_agents::TurnEndReason::Failed { error } = reason {
                    eprintln!("error: {error}");
                }
            }
            AgentEvent::UsageUpdated { cumulative, .. } => {
                last_usage = Some(*cumulative);
            }
            _ => {}
        }
    }

    if line_open {
        writeln!(out)?;
    }
    out.flush()?;
    drop(out);

    match turn.await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            eprintln!("error: {err:#}");
            exit.mark_failed();
        }
        Err(err) => {
            eprintln!("error: headless turn task failed: {err}");
            exit.mark_failed();
        }
    }

    if !json && let Some(summary) = last_usage.as_ref().and_then(format_token_summary) {
        eprintln!("{summary}");
    }

    Ok(exit.code)
}
