use super::*;
impl RegistryState {
    pub(super) fn apply_input_event(
        &mut self,
        broadcast_tx: &broadcast::Sender<RegistryEvent>,
        session_id: &SessionId,
        event: AgentEvent,
    ) {
        let state = self;
        match event {
            AgentEvent::ApprovalRequested { request } => {
                let req_clone = request.clone();
                update_pending_status(state, broadcast_tx, session_id, |record| {
                    if !record
                        .pending_approvals
                        .iter()
                        .any(|existing| existing.request_id == request.request_id)
                    {
                        record.pending_approvals.push_back(request);
                    }
                });
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::ApprovalRequested {
                        session_id: session_id.clone(),
                        request: req_clone,
                    },
                );
            }

            AgentEvent::ApprovalResolved { request_id } => {
                update_pending_status(state, broadcast_tx, session_id, |record| {
                    record
                        .pending_approvals
                        .retain(|req| req.request_id != request_id);
                });
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::ApprovalResolved {
                        session_id: session_id.clone(),
                        request_id,
                    },
                );
            }

            AgentEvent::QuestionAsked { request } => {
                let req_clone = request.clone();
                let q_entry = LiveEntry::Question {
                    id: request.request_id.to_string(),
                    questions: request.questions.clone(),
                    answers: None,
                    resolved: false,
                };
                {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    record.entries.push(q_entry.clone());
                }
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry: q_entry,
                    },
                );
                update_pending_status(state, broadcast_tx, session_id, |record| {
                    if !record
                        .pending_questions
                        .iter()
                        .any(|existing| existing.request_id == request.request_id)
                    {
                        record.pending_questions.push_back(request);
                    }
                });
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::QuestionAsked {
                        session_id: session_id.clone(),
                        request: req_clone,
                    },
                );
            }

            AgentEvent::QuestionResolved { request_id } => {
                update_pending_status(state, broadcast_tx, session_id, |record| {
                    record
                        .pending_questions
                        .retain(|req| req.request_id != request_id);
                });
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::QuestionResolved {
                        session_id: session_id.clone(),
                        request_id,
                    },
                );
            }

            _ => {}
        }
    }
}
