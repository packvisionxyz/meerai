use super::fork::latest_closed_turn_in_events;
use super::*;

impl Session {
    pub fn blob_store(&self) -> Option<BlobStore> {
        Some(BlobStore::new(
            self.inner.store.clone(),
            self.inner.id.clone(),
        ))
    }
    pub async fn read_blob(&self, key: &str) -> Result<Option<String>> {
        if key.contains('/') || key.contains('\\') || key.contains("..") {
            anyhow::bail!("invalid blob key");
        }
        match self.blob_store() {
            Some(blobs) => blobs.get(key),
            None => Ok(None),
        }
    }
    pub fn list_blobs(&self) -> Result<Vec<(String, u64)>> {
        match self.blob_store() {
            Some(blobs) => blobs.list(),
            None => Ok(Vec::new()),
        }
    }
    pub fn events(&self) -> Vec<LoggedEvent> {
        read_lock(&self.inner.events).clone()
    }
    pub fn event_count(&self) -> usize {
        read_lock(&self.inner.events).len()
    }
    pub fn id(&self) -> &SessionId {
        &self.inner.id
    }
    pub fn dir(&self) -> Option<&Path> {
        self.inner.dir.as_deref()
    }
    pub fn snapshot_store(&self) -> Option<crate::SnapshotStore> {
        Some(crate::SnapshotStore::new(
            self.inner.store.clone(),
            self.inner.id.clone(),
        ))
    }
    pub fn config(&self) -> Option<&SessionConfig> {
        self.inner.config.as_ref()
    }
    pub fn meta(&self) -> SessionMeta {
        read_lock(&self.inner.meta).clone()
    }
    pub fn derive_messages(&self, policy: &ProjectionPolicy) -> Vec<ChatMessage> {
        let events = self.events();
        derive_messages(&events, policy)
    }
    pub fn search_events(&self, query: &str, limit: usize) -> Result<Vec<LoggedEvent>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = i64::try_from(limit.max(1))?;
        self.inner.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT e.seq, e.payload_json
                 FROM session_events_fts
                 JOIN session_events e
                   ON e.session_id = session_events_fts.session_id
                  AND e.seq = session_events_fts.seq
                 WHERE session_events_fts.session_id = ?1
                   AND session_events_fts MATCH ?2
                 ORDER BY e.seq
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![self.inner.id.as_str(), fts, limit], |row| {
                let seq = Seq(row.get(0)?);
                let raw: String = row.get(1)?;
                Ok((seq, raw))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (seq, raw) = row?;
                let event = serde_json::from_str(&raw)
                    .with_context(|| format!("decode session event {}", seq.0))?;
                events.push(LoggedEvent { seq, event });
            }
            Ok(events)
        })
    }
    pub fn latest_closed_turn(&self) -> Option<TurnId> {
        latest_closed_turn_in_events(&self.events())
    }
}

pub(super) fn fts_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|token| !matches!(*token, "AND" | "OR" | "NOT" | "NEAR"))
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();
    (!tokens.is_empty()).then(|| tokens.join(" "))
}
