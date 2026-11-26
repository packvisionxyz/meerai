use super::*;
impl RegistryState {
    pub(super) fn apply_turn_event(
        &mut self,
        broadcast_tx: &broadcast::Sender<RegistryEvent>,
        session_id: &SessionId,
        event: AgentEvent,
    ) {
        let state = self;
        match event {
            AgentEvent::TurnStarted { turn_id, model } => {
                let (summary, updated_user) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    record.recovery_pending = false;
                    record.pending_approvals.clear();
                    record.pending_questions.clear();
                    record.summary.status = SessionStatus::Running;
                    record.summary.active_turn_id = Some(turn_id.clone());
                    record.summary.active_tool = None;
                    record.summary.turn_started_at_ms = Some(now_millis());
                    record.turn_started_instant = Some(Instant::now());
                    record.summary.model = display_model(&model);
                    record.streaming_item_id = None;
                    record.streaming_text.clear();
                    record.streaming_reasoning.clear();
                    let mut user_entry = None;
                    for entry in record.entries.iter_mut().rev() {
                        if let LiveEntry::User { turn_id: slot, .. } = entry {
                            if slot.is_none() {
                                *slot = Some(turn_id.clone());
                                user_entry = Some(entry.clone());
                            }
                            break;
                        }
                    }
                    (record.summary.clone(), user_entry)
                };
                if let Some(entry) = updated_user {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryUpdated {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::TurnStarted {
                        session_id: session_id.clone(),
                        turn_id,
                        model,
                    },
                );
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::SessionUpdated { session: summary },
                );
            }

            AgentEvent::RecoveryStarted { attempt } => {
                let entry_id = state.next_entry_id();
                let entry = LiveEntry::System {
                    id: entry_id,
                    text: format!("Recovery attempt {attempt} after a failed turn."),
                };
                let record = state.sessions.get_mut(session_id).unwrap();
                record.recovery_pending = true;
                record.entries.push(entry.clone());
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
            }

            AgentEvent::TurnCompleted { turn_id, reason } => {
                state.flush_streaming(session_id, broadcast_tx);
                let cancelled_or_failed = match &reason {
                    TurnEndReason::Aborted { .. } => ToolCallStatus::Cancelled,
                    _ => ToolCallStatus::Failed,
                };
                let updated_subagents: Vec<LiveEntry> = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    finalize_open_entries(record, cancelled_or_failed)
                };
                for entry in updated_subagents {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryUpdated {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                let terminal_entry = match &reason {
                    TurnEndReason::Aborted { .. } => Some(LiveEntry::System {
                        id: state.next_entry_id(),
                        text: "Turn cancelled.".to_string(),
                    }),
                    TurnEndReason::Failed { error } => {
                        let already_error = matches!(
                            state
                                .sessions
                                .get(session_id)
                                .and_then(|r| r.entries.last()),
                            Some(LiveEntry::Error { .. })
                        );
                        if !already_error {
                            Some(LiveEntry::Error {
                                id: state.next_entry_id(),
                                message: format!("turn failed: {error}"),
                            })
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(entry) = terminal_entry {
                    state
                        .sessions
                        .get_mut(session_id)
                        .unwrap()
                        .entries
                        .push(entry.clone());
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryCompleted {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                let updated = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let prev = record.summary.clone();
                    record.metrics.turn_count += 1;
                    if let Some(started) = record.turn_started_instant.take() {
                        record.metrics.last_turn_ms = Some(started.elapsed().as_millis() as u64);
                    }
                    record.pending_approvals.clear();
                    record.pending_questions.clear();
                    record.summary.status = SessionStatus::Idle;
                    record.summary.active_turn_id = None;
                    record.summary.active_tool = None;
                    record.summary.turn_started_at_ms = None;
                    if record.summary != prev {
                        Some(record.summary.clone())
                    } else {
                        None
                    }
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::TurnCompleted {
                        session_id: session_id.clone(),
                        turn_id,
                        reason,
                    },
                );
                if let Some(session) = updated {
                    state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
                }
            }

            AgentEvent::Error { code, message } => {
                state.flush_streaming(session_id, broadcast_tx);
                let updated_subagents = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    finalize_open_entries(record, ToolCallStatus::Failed)
                };
                for entry in updated_subagents {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryUpdated {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                let entry_id = state.next_entry_id();
                let entry = LiveEntry::Error {
                    id: entry_id,
                    message: message.clone(),
                };
                let updated = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let prev = record.summary.clone();
                    if let Some(started) = record.turn_started_instant.take() {
                        record.metrics.last_turn_ms = Some(started.elapsed().as_millis() as u64);
                    }
                    record.pending_approvals.clear();
                    record.pending_questions.clear();
                    record.summary.status = SessionStatus::Idle;
                    record.summary.active_turn_id = None;
                    record.summary.active_tool = None;
                    record.summary.turn_started_at_ms = None;
                    record.entries.push(entry.clone());
                    if record.summary != prev {
                        Some(record.summary.clone())
                    } else {
                        None
                    }
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::Error {
                        session_id: session_id.clone(),
                        code,
                        message,
                    },
                );
                if let Some(session) = updated {
                    state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
                }
            }

            _ => {}
        }
    }
}
