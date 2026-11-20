use std::collections::{HashMap, HashSet};
use std::io::{self, Write as _};
use std::time::Duration;

use anyhow::Result;
use meerai_agents::events::{
    ApprovalDecision, ApprovalKind, ApprovalRequest, ItemStatus, RequestId,
};
use meerai_agents::{AgentCommand, AgentEvent, ItemId, MultiTurnAgent, SessionItem, spawn_worker};
use tokio::sync::mpsc;

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
    if let Some(cached) = usage.cached_tokens {
        parts.push(format!("cached={cached}"));
    }
    (!parts.is_empty()).then(|| format!("tokens: {}", parts.join(" ")))
}

pub(crate) fn question_cancel_command(request: &meerai_agents::QuestionRequest) -> AgentCommand {
    AgentCommand::AnswerQuestion {
        request_id: request.request_id.clone(),
        answers: None,
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum HeadlessApprovalAction {
    Cancel,
    Decline { request_id: RequestId },
}

impl HeadlessApprovalAction {
    pub(crate) fn for_request(request: &ApprovalRequest) -> Self {
        match request.kind {
            ApprovalKind::Spend | ApprovalKind::ExternalAgent => Self::Decline {
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

    pub(crate) fn reset_after_recovery(&mut self) {
        self.failed = false;
        if !self.approval_cancelled {
            self.code = 1;
        }
    }
}

pub(crate) struct PlainEventPrinter {
    item_labels: HashMap<ItemId, String>,
    streamed_text: HashSet<ItemId>,
    line_open: bool,
}

impl PlainEventPrinter {
    pub(crate) fn new() -> Self {
        Self {
            item_labels: HashMap::new(),
            streamed_text: HashSet::new(),
            line_open: false,
        }
    }

    fn close_open_line(&mut self, out: &mut impl io::Write) -> Result<()> {
        if self.line_open {
            writeln!(out)?;
            self.line_open = false;
        }
        Ok(())
    }

    fn write_status_line(
        &mut self,
        out: &mut impl io::Write,
        label: &str,
        status: ItemStatus,
    ) -> Result<()> {
        self.close_open_line(out)?;
        writeln!(out, "▸ {label} {}", item_status_label(status))?;
        out.flush()?;
        Ok(())
    }

    pub(crate) fn handle(&mut self, event: &AgentEvent, out: &mut impl io::Write) -> Result<()> {
        match event {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::ToolCall {
                        id, name, status, ..
                    },
            } => {
                self.item_labels.insert(id.clone(), name.clone());
                self.write_status_line(out, name, *status)?;
            }
            AgentEvent::ItemStarted {
                item:
                    SessionItem::SubagentRun {
                        id, agent, status, ..
                    },
            } => {
                let label = format!("subagent {agent}");
                self.item_labels.insert(id.clone(), label.clone());
                self.write_status_line(out, &label, *status)?;
            }
            AgentEvent::ItemDelta {
                item_id,
                delta: meerai_agents::ItemDelta::AgentText(chunk),
            } => {
                if !chunk.is_empty() {
                    self.streamed_text.insert(item_id.clone());
                    write!(out, "{chunk}")?;
                    out.flush()?;
                    self.line_open = !chunk.ends_with('\n');
                }
            }
            AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage { id, text },
            } => {
                if !self.streamed_text.remove(id) && !text.is_empty() {
                    self.close_open_line(out)?;
                    writeln!(out, "{text}")?;
                    out.flush()?;
                }
            }
            AgentEvent::ItemDelta {
                item_id,
                delta: meerai_agents::ItemDelta::StatusChanged(status),
            } => {
                if let Some(label) = self.item_labels.get(item_id).cloned() {
                    self.write_status_line(out, &label, *status)?;
                }
            }
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        id, name, status, ..
                    },
            } => {
                self.item_labels.remove(id);
                self.write_status_line(out, name, *status)?;
            }
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::SubagentRun {
                        id, agent, status, ..
                    },
            } => {
                self.item_labels.remove(id);
                self.write_status_line(out, &format!("subagent {agent}"), *status)?;
            }
            AgentEvent::Error { message, .. } => {
                self.close_open_line(out)?;
                eprintln!("error: {message}");
            }
            AgentEvent::TurnCompleted { reason, .. } => {
                self.close_open_line(out)?;
                if let meerai_agents::TurnEndReason::Failed { error } = reason {
                    eprintln!("error: {error}");
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self, out: &mut impl io::Write) -> Result<()> {
        self.close_open_line(out)?;
        out.flush()?;
        Ok(())
    }
}

pub(crate) async fn run_headless(agent: MultiTurnAgent, prompt: String, json: bool) -> Result<i32> {
    let (cmd_tx, event_rx) = spawn_worker(agent);
    run_headless_channels(cmd_tx, event_rx, prompt, json).await
}

async fn shutdown_worker(
    cmd_tx: &mpsc::UnboundedSender<AgentCommand>,
    event_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
) {
    let _ = cmd_tx.send(AgentCommand::Quit);
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        while event_rx.recv().await.is_some() {}
    })
    .await;
}

pub(crate) async fn run_headless_channels(
    cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    prompt: String,
    json: bool,
) -> Result<i32> {
    let _ = cmd_tx.send(AgentCommand::SubmitPrompt(prompt));

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut printer = PlainEventPrinter::new();
    let mut exit = HeadlessExit::new();
    let mut last_usage: Option<meerai_agents::Usage> = None;
    let mut recovery_pending = false;

    while let Some(event) = event_rx.recv().await {
        if json {
            writeln!(out, "{}", serde_json::to_string(&event)?)?;
            out.flush()?;
        }

        match &event {
            AgentEvent::QuestionAsked { request } => {
                eprintln!(
                    "question required but headless mode cannot answer: {}; rerun in the TUI",
                    request
                        .questions
                        .first()
                        .map(|question| question.header.as_str())
                        .unwrap_or("question")
                );
                let _ = cmd_tx.send(question_cancel_command(request));
            }
            AgentEvent::ApprovalRequested { request } => {
                let action = HeadlessApprovalAction::for_request(request);
                match &action {
                    HeadlessApprovalAction::Decline { .. }
                        if request.kind == ApprovalKind::ExternalAgent =>
                    {
                        eprintln!(
                            "external agent permission required; use the TUI or configure agy to allow the action"
                        );
                    }
                    HeadlessApprovalAction::Decline { .. } => eprintln!(
                        "session token budget reached; raise context.max_session_tokens in the user settings or unset it"
                    ),
                    HeadlessApprovalAction::Cancel => {
                        eprintln!("approval required: {}", request.preview.title);
                        eprintln!("rerun with --mode auto, or use the TUI");
                    }
                }
                let _ = cmd_tx.send(action.command());
            }
            _ => {}
        }

        if !json {
            if let AgentEvent::UsageUpdated { cumulative, .. } = &event {
                last_usage = Some(*cumulative);
            }
            printer.handle(&event, &mut out)?;
        }
        exit.observe(headless_outcome(&event));

        match &event {
            AgentEvent::RecoveryStarted { attempt } => {
                recovery_pending = true;
                if !json {
                    eprintln!("recovering after a failed turn (attempt {attempt})");
                }
            }
            AgentEvent::TurnStarted { .. } if recovery_pending => {
                recovery_pending = false;
                exit.reset_after_recovery();
            }
            AgentEvent::TurnCompleted { .. } => {
                if recovery_pending {
                    continue;
                }
                shutdown_worker(&cmd_tx, &mut event_rx).await;
                break;
            }
            AgentEvent::Error { message, .. } => {
                if recovery_pending {
                    continue;
                }
                if json {
                    eprintln!("error: {message}");
                }
                exit.mark_failed();
                shutdown_worker(&cmd_tx, &mut event_rx).await;
                break;
            }
            _ => {}
        }
    }

    printer.finish(&mut out)?;
    drop(out);

    if !json && let Some(summary) = last_usage.as_ref().and_then(format_token_summary) {
        eprintln!("{summary}");
    }

    Ok(exit.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_agents::events::ErrorCode;
    use meerai_agents::{TurnEndReason, TurnId};

    #[tokio::test]
    async fn headless_waits_for_a_recovery_turn_and_exits_zero() {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let turn = TurnId::from_raw("turn-1");
        tokio::spawn(async move {
            let _ = event_tx.send(AgentEvent::RecoveryStarted { attempt: 1 });
            let _ = event_tx.send(AgentEvent::Error {
                code: ErrorCode::Provider,
                message: "boom".to_string(),
            });
            let _ = event_tx.send(AgentEvent::TurnCompleted {
                turn_id: turn.clone(),
                reason: TurnEndReason::Failed {
                    error: "boom".to_string(),
                },
            });
            let _ = event_tx.send(AgentEvent::TurnStarted {
                turn_id: turn.clone(),
                model: "test".to_string(),
            });
            let _ = event_tx.send(AgentEvent::TurnCompleted {
                turn_id: turn,
                reason: TurnEndReason::Completed,
            });
        });

        let code = run_headless_channels(cmd_tx, event_rx, "go".to_string(), false)
            .await
            .unwrap();
        assert_eq!(code, 0);
    }
}
