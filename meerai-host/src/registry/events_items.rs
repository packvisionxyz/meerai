use super::*;
impl RegistryState {
    pub(super) fn apply_items_event(
        &mut self,
        broadcast_tx: &broadcast::Sender<RegistryEvent>,
        session_id: &SessionId,
        event: AgentEvent,
    ) {
        let state = self;
        match event {
            AgentEvent::ItemStarted { item } => match item {
                SessionItem::AgentMessage { id, .. } => {
                    let Some(record) = state.sessions.get_mut(session_id) else {
                        return;
                    };
                    record.streaming_item_id = Some(id.to_string());
                }
                SessionItem::Reasoning { id, summary } => {
                    let Some(record) = state.sessions.get_mut(session_id) else {
                        return;
                    };
                    record.streaming_item_id = Some(id.to_string());
                    if !summary.is_empty() {
                        record.streaming_reasoning.push_str(&summary);
                    }
                }
                SessionItem::ToolCall {
                    id,
                    call_id,
                    name,
                    args,
                    status,
                    output,
                    ..
                } => {
                    state.flush_streaming(session_id, broadcast_tx);
                    let entry_id = call_id.to_string();
                    let edit_diff = tool_edit_diff(&name, &args, output.as_deref());
                    let entry = LiveEntry::ToolCall {
                        id: entry_id.clone(),
                        name: name.clone(),
                        args,
                        status: item_status_str(status).to_string(),
                        output,
                        edit_diff,
                    };
                    let (updated_summary, entry) = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        record.tool_items.insert(id, entry_id);
                        record.metrics.tool_calls += 1;
                        record.summary.active_tool = Some(name);
                        record.entries.push(entry.clone());
                        (record.summary.clone(), entry)
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
                        RegistryEventKind::SessionUpdated {
                            session: updated_summary,
                        },
                    );
                }
                SessionItem::SubagentRun {
                    id,
                    name,
                    agent,
                    status,
                    report,
                    child_session_id,
                    model,
                    approval_mode,
                    steps_used,
                    max_steps,
                    usage,
                    est_cost_usd,
                    active_tool,
                } => {
                    state.flush_streaming(session_id, broadcast_tx);
                    let entry_id = id.to_string();
                    let entry = LiveEntry::Subagent {
                        id: entry_id.clone(),
                        name,
                        agent: agent.clone(),
                        status: item_status_str(status).to_string(),
                        report,
                        child_session_id,
                        model,
                        approval_mode: approval_mode.label().to_string(),
                        steps_used,
                        max_steps,
                        usage,
                        est_cost_usd,
                        active_tool,
                    };
                    let (updated_summary, entry) = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        record.tool_items.insert(id, entry_id);
                        record.summary.active_tool = Some(agent);
                        record.entries.push(entry.clone());
                        (record.summary.clone(), entry)
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
                        RegistryEventKind::SessionUpdated {
                            session: updated_summary,
                        },
                    );
                }
                _ => {}
            },

            AgentEvent::ItemDelta { item_id, delta } => match delta {
                ItemDelta::AgentText(chunk) => {
                    let id_str = item_id.to_string();
                    let Some(record) = state.sessions.get_mut(session_id) else {
                        return;
                    };
                    record.streaming_item_id = Some(id_str.clone());
                    record.streaming_text.push_str(&chunk);
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::TextDelta {
                            session_id: session_id.clone(),
                            item_id: id_str,
                            delta: chunk,
                        },
                    );
                }
                ItemDelta::ReasoningSummary(chunk) => {
                    let id_str = item_id.to_string();
                    let Some(record) = state.sessions.get_mut(session_id) else {
                        return;
                    };
                    record.streaming_item_id = Some(id_str.clone());
                    record.streaming_reasoning.push_str(&chunk);
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::ReasoningDelta {
                            session_id: session_id.clone(),
                            item_id: id_str,
                            delta: chunk,
                        },
                    );
                }
                ItemDelta::StatusChanged(status) => {
                    let updated = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        if let Some(entry_id) = record.tool_items.get(&item_id).cloned()
                            && let Some(entry) =
                                record.entries.iter_mut().rev().find(|entry| match entry {
                                    LiveEntry::ToolCall { id, .. }
                                    | LiveEntry::Subagent { id, .. } => *id == entry_id,
                                    _ => false,
                                })
                        {
                            let status_text = item_status_str(status).to_string();
                            match entry {
                                LiveEntry::ToolCall { status: slot, .. } => *slot = status_text,
                                LiveEntry::Subagent { status: slot, .. } => *slot = status_text,
                                _ => {}
                            }
                            Some(entry.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(entry) = updated {
                        state.push_event(
                            broadcast_tx,
                            RegistryEventKind::EntryUpdated {
                                session_id: session_id.clone(),
                                entry,
                            },
                        );
                    }
                }
                ItemDelta::SubagentProgress {
                    steps_used,
                    max_steps,
                    usage,
                    est_cost_usd,
                    active_tool,
                } => {
                    let updated = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        let entry_id = record.tool_items.get(&item_id).cloned();
                        entry_id.and_then(|entry_id| {
                            record
                                .entries
                                .iter_mut()
                                .rev()
                                .find(|entry| {
                                    matches!(
                                        entry,
                                        LiveEntry::Subagent { id, .. } if *id == entry_id
                                    )
                                })
                                .and_then(|entry| match entry {
                                    LiveEntry::Subagent {
                                        steps_used: slot_steps,
                                        max_steps: slot_max,
                                        usage: slot_usage,
                                        est_cost_usd: slot_cost,
                                        active_tool: slot_tool,
                                        ..
                                    } => {
                                        *slot_steps = steps_used;
                                        *slot_max = max_steps;
                                        *slot_usage = usage;
                                        *slot_cost = est_cost_usd;
                                        *slot_tool = active_tool;
                                        Some(entry.clone())
                                    }
                                    _ => None,
                                })
                        })
                    };
                    if let Some(entry) = updated {
                        state.push_event(
                            broadcast_tx,
                            RegistryEventKind::EntryUpdated {
                                session_id: session_id.clone(),
                                entry,
                            },
                        );
                    }
                }
                ItemDelta::CommandOutput(_) | ItemDelta::PlanUpdated(_) => {}
            },

            AgentEvent::ItemCompleted { item } => match item {
                SessionItem::AgentMessage { id, text } => {
                    let id_str = id.to_string();
                    let entry = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        let final_text = if !text.is_empty() {
                            text
                        } else {
                            std::mem::take(&mut record.streaming_text)
                        };
                        record.streaming_text.clear();
                        if record.streaming_item_id.as_deref() == Some(id_str.as_str()) {
                            record.streaming_item_id = None;
                        }
                        if final_text.is_empty() {
                            None
                        } else {
                            let entry = LiveEntry::Assistant {
                                id: id_str,
                                text: final_text,
                                reasoning: String::new(),
                            };
                            record.entries.push(entry.clone());
                            Some(entry)
                        }
                    };
                    if let Some(entry) = entry {
                        state.push_event(
                            broadcast_tx,
                            RegistryEventKind::EntryCompleted {
                                session_id: session_id.clone(),
                                entry,
                            },
                        );
                    }
                }
                SessionItem::Reasoning { id, summary } => {
                    let id_str = id.to_string();
                    let entry = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        let final_reasoning = if !summary.is_empty() {
                            summary
                        } else {
                            std::mem::take(&mut record.streaming_reasoning)
                        };
                        record.streaming_reasoning.clear();
                        if record.streaming_item_id.as_deref() == Some(id_str.as_str()) {
                            record.streaming_item_id = None;
                        }
                        if final_reasoning.is_empty() {
                            None
                        } else {
                            let entry = LiveEntry::Assistant {
                                id: id_str,
                                text: String::new(),
                                reasoning: final_reasoning,
                            };
                            record.entries.push(entry.clone());
                            Some(entry)
                        }
                    };
                    if let Some(entry) = entry {
                        state.push_event(
                            broadcast_tx,
                            RegistryEventKind::EntryCompleted {
                                session_id: session_id.clone(),
                                entry,
                            },
                        );
                    }
                }
                SessionItem::ToolCall {
                    id,
                    call_id,
                    name,
                    args,
                    status,
                    output,
                    ..
                } => {
                    let (entry, updated_summary) = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        let entry = update_or_push_tool_entry(
                            record,
                            &id,
                            call_id.to_string(),
                            name.clone(),
                            args,
                            completed_item_status_str(status),
                            output,
                        );
                        let updated_summary =
                            if record.summary.active_tool.as_deref() == Some(name.as_str()) {
                                record.summary.active_tool = None;
                                Some(record.summary.clone())
                            } else {
                                None
                            };
                        (entry, updated_summary)
                    };
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryCompleted {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                    if let Some(session) = updated_summary {
                        state.push_event(
                            broadcast_tx,
                            RegistryEventKind::SessionUpdated { session },
                        );
                    }
                }
                SessionItem::SubagentRun {
                    id,
                    name,
                    agent,
                    status,
                    report,
                    child_session_id,
                    ..
                } => {
                    let (entry, updated_summary) = {
                        let Some(record) = state.sessions.get_mut(session_id) else {
                            return;
                        };
                        let entry = update_or_push_subagent_entry(
                            record,
                            &id,
                            name,
                            agent.clone(),
                            completed_item_status_str(status),
                            report,
                            child_session_id,
                        );
                        let updated_summary =
                            if record.summary.active_tool.as_deref() == Some(agent.as_str()) {
                                record.summary.active_tool = None;
                                Some(record.summary.clone())
                            } else {
                                None
                            };
                        (entry, updated_summary)
                    };
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryCompleted {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                    if let Some(session) = updated_summary {
                        state.push_event(
                            broadcast_tx,
                            RegistryEventKind::SessionUpdated { session },
                        );
                    }
                }
                SessionItem::Compaction {
                    id, freed_tokens, ..
                } => {
                    let entry = LiveEntry::System {
                        id: id.to_string(),
                        text: format!("Compaction freed {freed_tokens} tokens"),
                    };
                    let Some(record) = state.sessions.get_mut(session_id) else {
                        return;
                    };
                    record.entries.push(entry.clone());

                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryCompleted {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                _ => {}
            },

            AgentEvent::ItemDiscarded { .. } => {
                let cleared = {
                    let Some(record) = state.sessions.get_mut(session_id) else {
                        return;
                    };
                    let cleared = record.streaming_item_id.is_some()
                        || !record.streaming_text.is_empty()
                        || !record.streaming_reasoning.is_empty();
                    record.streaming_item_id = None;
                    record.streaming_text.clear();
                    record.streaming_reasoning.clear();
                    cleared
                };
                if cleared {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::StreamDiscarded {
                            session_id: session_id.clone(),
                        },
                    );
                }
            }

            _ => {}
        }
    }
}
