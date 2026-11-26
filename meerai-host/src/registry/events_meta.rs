use super::*;
impl RegistryState {
    pub(super) fn apply_meta_event(
        &mut self,
        broadcast_tx: &broadcast::Sender<RegistryEvent>,
        session_id: &SessionId,
        event: AgentEvent,
    ) {
        let state = self;
        match event {
            AgentEvent::ContextUpdated { breakdown } => {
                {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    record.context = Some(breakdown.clone());
                }
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::ContextUpdated {
                        session_id: session_id.clone(),
                        breakdown,
                    },
                );
            }

            AgentEvent::LoopUpdated { state: loop_state } => {
                state.sessions.get_mut(session_id).unwrap().loop_state = Some(loop_state.clone());
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::LoopUpdated {
                        session_id: session_id.clone(),
                        state: loop_state,
                    },
                );
            }

            AgentEvent::LoopContinued { turn, max_turns } => {
                let entry = LiveEntry::System {
                    id: state.next_entry_id(),
                    text: format!("Loop continues (turn {turn}/{max_turns})"),
                };
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
                let entry = LiveEntry::System {
                    id: state.next_entry_id(),
                    text,
                };
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

            AgentEvent::UsageUpdated {
                usage,
                cumulative,
                est_cost_usd,
            } => {
                let (usage_snap, metrics_snap) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let session_prompt = cumulative.prompt_tokens.unwrap_or(0).max(0) as u64;
                    let session_completion =
                        cumulative.completion_tokens.unwrap_or(0).max(0) as u64;
                    let session_total = cumulative
                        .total_tokens
                        .map(|t| t.max(0) as u64)
                        .unwrap_or(session_prompt + session_completion);
                    record.metrics.session_prompt_tokens = session_prompt;
                    record.metrics.session_completion_tokens = session_completion;
                    record.metrics.session_total_tokens = session_total;
                    record.metrics.session_cached_tokens =
                        cumulative.cached_tokens.unwrap_or(0).max(0) as u64;
                    record.metrics.session_cache_creation_tokens =
                        cumulative.cache_creation_tokens.unwrap_or(0).max(0) as u64;
                    if let (Some(started), Some(tokens)) =
                        (record.turn_started_instant, usage.completion_tokens)
                    {
                        let secs = started.elapsed().as_secs_f64();
                        if secs > 0.05 && tokens > 0 {
                            record.metrics.tokens_per_sec = Some(tokens as f64 / secs);
                        }
                    }
                    record.usage = UsageStatsSnapshot {
                        prompt_tokens: usage.prompt_tokens,
                        completion_tokens: usage.completion_tokens,
                        total_tokens: usage.total_tokens,
                        cached_tokens: usage.cached_tokens,
                        cache_creation_tokens: usage.cache_creation_tokens,
                        est_cost_usd,
                    };
                    (record.usage.clone(), record.metrics.clone())
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::UsageUpdated {
                        session_id: session_id.clone(),
                        usage: usage_snap,
                        metrics: metrics_snap,
                    },
                );
            }

            AgentEvent::RewindCompleted {
                turn_id,
                scope,
                restored,
                error,
            } => {
                let (entries_after, restored_prompt) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    if error.is_none()
                        && matches!(scope, RewindScope::Both | RewindScope::Conversation)
                        && let Some(idx) = record.entries.iter().position(|entry| {
                            matches!(
                                entry,
                                LiveEntry::User { turn_id: Some(tid), .. } if tid == &turn_id
                            )
                        })
                    {
                        let prompt = match &record.entries[idx] {
                            LiveEntry::User { text, .. } => Some(text.clone()),
                            _ => None,
                        };
                        record.entries.truncate(idx);
                        record.summary.active_turn_id = None;
                        record.summary.active_tool = None;
                        (Some(record.entries.clone()), prompt)
                    } else {
                        (None, None)
                    }
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::SessionRewound {
                        session_id: session_id.clone(),
                        turn_id,
                        scope,
                        restored,
                        error,
                        entries: entries_after,
                        restored_prompt,
                    },
                );
            }

            _ => {}
        }
    }
}
