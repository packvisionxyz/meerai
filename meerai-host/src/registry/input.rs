use super::*;

impl SessionRegistry {
    pub fn decide_approval(
        &self,
        session_id: &SessionId,
        request_id: RequestId,
        decision: ApprovalDecision,
        scope: Option<GrantScope>,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get_mut(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            let Some(pos) = record
                .pending_approvals
                .iter()
                .position(|req| req.request_id == request_id)
            else {
                return Err(HostError::StaleRequest(request_id));
            };
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }
            record.pending_approvals.remove(pos);
            let prev = record.summary.clone();
            record.recompute_status();
            let updated = (record.summary != prev).then(|| record.summary.clone());
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::ApprovalResolved {
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                },
            );
            if let Some(session) = updated {
                state.push_event(
                    &self.inner.broadcast_tx,
                    RegistryEventKind::SessionUpdated { session },
                );
            }
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::Approve {
                request_id,
                decision,
                scope,
            })
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn answer_question(
        &self,
        session_id: &SessionId,
        request_id: RequestId,
        answers: Option<Vec<QuestionAnswer>>,
    ) -> Result<(), HostError> {
        let cmd_tx =
            {
                let mut state = self.lock_state()?;
                let Some(record) = state.sessions.get_mut(session_id) else {
                    return Err(HostError::SessionNotFound(session_id.clone()));
                };
                let Some(pos) = record
                    .pending_questions
                    .iter()
                    .position(|req| req.request_id == request_id)
                else {
                    return Err(HostError::StaleRequest(request_id));
                };
                if record.cmd_tx.is_closed() {
                    return Err(HostError::WorkerClosed);
                }
                record.pending_questions.remove(pos);
                let req_id_str = request_id.to_string();
                let mut updated_entry = None;
                if let Some(LiveEntry::Question {
                    answers: slot,
                    resolved,
                    ..
                }) = record.entries.iter_mut().rev().find(
                    |entry| matches!(entry, LiveEntry::Question { id, .. } if *id == req_id_str),
                ) {
                    *slot = answers.clone();
                    *resolved = true;
                }
                if let Some(entry) = record.entries.iter().rev().find(
                    |entry| matches!(entry, LiveEntry::Question { id, .. } if *id == req_id_str),
                ) {
                    updated_entry = Some(entry.clone());
                }
                let prev = record.summary.clone();
                record.recompute_status();
                let updated_summary = (record.summary != prev).then(|| record.summary.clone());
                let cmd_tx = record.cmd_tx.clone();
                if let Some(entry) = updated_entry {
                    state.push_event(
                        &self.inner.broadcast_tx,
                        RegistryEventKind::EntryUpdated {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                state.push_event(
                    &self.inner.broadcast_tx,
                    RegistryEventKind::QuestionResolved {
                        session_id: session_id.clone(),
                        request_id: request_id.clone(),
                    },
                );
                if let Some(session) = updated_summary {
                    state.push_event(
                        &self.inner.broadcast_tx,
                        RegistryEventKind::SessionUpdated { session },
                    );
                }
                cmd_tx
            };
        cmd_tx
            .send(AgentCommand::AnswerQuestion {
                request_id,
                answers,
            })
            .map_err(|_| HostError::WorkerClosed)
    }
}
