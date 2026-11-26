use super::*;

impl Session {
    pub async fn rewind(
        &self,
        turn_id: &TurnId,
        scope: RewindScope,
        restored: Vec<PathBuf>,
    ) -> Result<()> {
        let events = self.events();
        let start = turn_start(&events, turn_id)
            .ok_or_else(|| anyhow::anyhow!("unknown turn: {turn_id}"))?;
        let close_conversation = matches!(scope, RewindScope::Both | RewindScope::Conversation);
        let closing: Vec<SessionEvent> = if close_conversation {
            open_turns(&events)
                .into_iter()
                .map(|open| SessionEvent::TurnEnded {
                    turn_id: open,
                    reason: TurnEndReason::Aborted {
                        cause: AbortCause::User,
                    },
                })
                .collect()
        } else {
            Vec::new()
        };
        let replaced = close_conversation.then(|| SeqRange {
            start,
            end_inclusive: Seq(
                events.last().map_or(start.0, |logged| logged.seq.0) + closing.len() as u64
            ),
        });
        // One batch: the closing turns and the rewind marker land together or not at all.
        self.append_batch(
            &closing,
            Some(SessionEvent::RewindApplied {
                turn_id: turn_id.clone(),
                scope,
                restored,
                replaced,
            }),
        )
        .await
    }
    pub async fn heal_dangling_tool_calls(&self) -> Result<()> {
        let events = self.events();
        let shadowed = shadowed_ranges(&events);
        let mut pending: Vec<CallId> = Vec::new();
        for logged in &events {
            if shadowed.iter().any(|range| range.contains(logged.seq)) {
                continue;
            }
            match &logged.event {
                SessionEvent::AssistantMessage { tool_calls, .. } => {
                    pending = tool_calls
                        .iter()
                        .map(|call| CallId::from(call.id.as_str()))
                        .collect();
                }
                SessionEvent::ToolResult { call_id, .. } => {
                    pending.retain(|id| id != call_id);
                }
                _ => {}
            }
        }
        for call_id in pending {
            self.append(SessionEvent::ToolResult {
                call_id,
                output: ToolObservation::error(TOOL_INTERRUPTED),
            })
            .await?;
        }
        Ok(())
    }
}
