use super::*;

impl Session {
    pub async fn append(&self, event: SessionEvent) -> Result<Seq> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        let inner = self.inner.clone();
        storage::blocking(move || {
            let _guard = guard;
            let seq = Seq(inner.next_seq.load(Ordering::Relaxed));
            let logged = LoggedEvent { seq, event };
            let updated_meta = matches!(&logged.event, SessionEvent::TurnEnded { .. }).then(|| {
                let mut meta = read_lock(&inner.meta).clone();
                meta.updated_at_ms = now_millis();
                meta
            });
            store.immediate(|tx| {
                storage::insert_event(tx, &inner.id, &logged)?;
                if let Some(meta) = &updated_meta {
                    storage::update_meta(tx, meta)?;
                }
                Ok(())
            })?;
            inner.next_seq.store(seq.0 + 1, Ordering::Relaxed);
            write_lock(&inner.events).push(logged);
            if let Some(meta) = updated_meta {
                *write_lock(&inner.meta) = meta;
            }
            Ok(seq)
        })
        .await
    }
    pub async fn flush(&self) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        storage::blocking(move || {
            let _guard = guard;
            storage::flush(&store)
        })
        .await
    }
    pub async fn record_usage(
        &self,
        turn_id: Option<&TurnId>,
        provider: &str,
        model: &str,
        task_type: Option<&str>,
        usage: &crate::event::Usage,
        est_cost_usd: Option<f64>,
    ) -> Result<()> {
        let entry = meerai_store::UsageEntry {
            session_id: self.inner.id.as_str().to_string(),
            parent_session_id: self
                .meta()
                .parent_session_id
                .map(|id| id.as_str().to_string()),
            turn_id: turn_id.map(|turn| turn.as_str().to_string()),
            agent_id: None,
            task_type: task_type.map(str::to_string),
            provider: provider.to_string(),
            model: model.to_string(),
            prompt_tokens: usage.prompt_tokens.unwrap_or(0).max(0) as u64,
            completion_tokens: usage.completion_tokens.unwrap_or(0).max(0) as u64,
            cached_tokens: usage.cached_tokens.unwrap_or(0).max(0) as u64,
            est_cost_usd,
            ts_ms: now_millis(),
        };
        let store = self.inner.store.clone();
        storage::blocking(move || store.usage().record(&entry)).await
    }
    pub(super) async fn append_batch(
        &self,
        first: &[SessionEvent],
        last: Option<SessionEvent>,
    ) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        let inner = self.inner.clone();
        let first = first.to_vec();
        storage::blocking(move || {
            let _guard = guard;
            let mut next = inner.next_seq.load(Ordering::Relaxed);
            let mut logged = Vec::with_capacity(first.len() + usize::from(last.is_some()));
            for event in first.into_iter().chain(last) {
                logged.push(LoggedEvent {
                    seq: Seq(next),
                    event,
                });
                next += 1;
            }
            let mut meta = None;
            if logged
                .iter()
                .any(|event| matches!(event.event, SessionEvent::TurnEnded { .. }))
            {
                let mut updated = read_lock(&inner.meta).clone();
                updated.updated_at_ms = now_millis();
                meta = Some(updated);
            }
            store.immediate(|tx| {
                for event in &logged {
                    storage::insert_event(tx, &inner.id, event)?;
                }
                if let Some(meta) = &meta {
                    storage::update_meta(tx, meta)?;
                }
                Ok(())
            })?;
            inner.next_seq.store(next, Ordering::Relaxed);
            write_lock(&inner.events).extend(logged);
            if let Some(meta) = meta {
                *write_lock(&inner.meta) = meta;
            }
            Ok(())
        })
        .await
    }
    pub(super) async fn close_open_turns(&self) -> Result<()> {
        for turn_id in open_turns(&self.events()) {
            self.append(SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Interrupted,
            })
            .await?;
        }
        Ok(())
    }
    pub(super) async fn close_open_compactions(&self) -> Result<()> {
        for compaction_id in open_compactions(&self.events()) {
            self.append(SessionEvent::CompactionEnded {
                compaction_id,
                result: CompactionResult::Failed {
                    error: COMPACTION_INTERRUPTED.to_string(),
                },
            })
            .await?;
        }
        Ok(())
    }
    pub(super) async fn append_preserved(&self, events: &[LoggedEvent]) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        let session = self.clone();
        let events = events.to_vec();
        storage::blocking(move || {
            let _guard = guard;
            store.immediate(|tx| {
                for event in &events {
                    storage::insert_event(tx, session.id(), event)?;
                }
                Ok(())
            })?;
            write_lock(&session.inner.events).extend_from_slice(&events);
            session.advance_next_seq(&events);
            Ok(())
        })
        .await
    }
    pub(super) fn advance_next_seq(&self, events: &[LoggedEvent]) {
        if let Some(max) = events.iter().map(|logged| logged.seq.0).max() {
            self.inner.next_seq.fetch_max(max + 1, Ordering::Relaxed);
        }
    }
}
