use super::*;

impl Session {
    pub async fn fork(
        config: &SessionConfig,
        source: &Session,
        boundary_turn: &TurnId,
    ) -> Result<Session> {
        let events = source.events();
        let meta = source.meta();
        Self::fork_from_parts(config, source.id(), &meta, &events, Some(boundary_turn)).await
    }
    pub async fn fork_stored(
        config: &SessionConfig,
        source_id: &SessionId,
        boundary_turn: Option<&TurnId>,
    ) -> Result<Session> {
        let (meta, events) = read_sqlite_session(config, source_id).await?;
        Self::fork_from_parts(config, source_id, &meta, &events, boundary_turn).await
    }
    pub async fn stored_meta(config: &SessionConfig, id: &SessionId) -> Result<SessionMeta> {
        Ok(read_sqlite_session(config, id).await?.0)
    }
    pub async fn memory_fork(config: &SessionConfig, source_id: &SessionId) -> Result<Session> {
        let (source_meta, mut events) = read_sqlite_session(config, source_id).await?;
        for logged in &mut events {
            reset_inherited_usage(&mut logged.event);
        }
        let session = Self::memory(SessionId::new(), source_meta.cwd.clone());
        session.append_preserved(&events).await?;
        session
            .update_meta(|meta| {
                meta.provider = source_meta.provider;
                meta.model = source_meta.model;
                meta.approval_mode = Some("plan".to_string());
            })
            .await?;
        session.heal_dangling_tool_calls().await?;
        Ok(session)
    }
    pub(super) async fn fork_from_parts(
        config: &SessionConfig,
        source_id: &SessionId,
        source_meta: &SessionMeta,
        events: &[LoggedEvent],
        boundary_turn: Option<&TurnId>,
    ) -> Result<Session> {
        let resolved_boundary = match boundary_turn {
            Some(turn) => turn.clone(),
            None => latest_closed_turn_in_events(events).ok_or_else(|| {
                anyhow::anyhow!("session has no closed turns to fork: {source_id}")
            })?,
        };
        let prefix = fork_prefix(events, &resolved_boundary)?;
        let inherited_event_count = prefix.len() as u64;
        let approval_mode = source_meta.approval_mode.clone();
        let provider = source_meta.provider.clone();
        let model = source_meta.model.clone();
        let cwd = source_meta.cwd.clone();

        let session = Self::open_or_create(config, SessionId::new(), cwd).await?;
        if let Err(error) = Self::fill_forked(
            &session,
            source_id,
            inherited_event_count,
            &prefix,
            approval_mode,
            provider,
            model,
        )
        .await
        {
            discard_session(&session).await;
            return Err(error);
        }

        Ok(session)
    }
    pub(super) async fn fill_forked(
        session: &Session,
        source_id: &SessionId,
        inherited_event_count: u64,
        prefix: &[LoggedEvent],
        approval_mode: Option<String>,
        provider: Option<String>,
        model: Option<String>,
    ) -> Result<()> {
        session.append_preserved(prefix).await?;
        session
            .update_meta(|meta| {
                meta.forked_from = Some(source_id.clone());
                meta.inherited_event_count = Some(inherited_event_count);
                meta.approval_mode = approval_mode;
                meta.provider = provider;
                meta.model = model;
            })
            .await
    }
    pub async fn fork_latest(config: &SessionConfig, source: &Session) -> Result<Session> {
        let events = source.events();
        let meta = source.meta();
        Self::fork_from_parts(config, source.id(), &meta, &events, None).await
    }
    pub async fn create_child(
        &self,
        model: Option<String>,
        title: Option<String>,
    ) -> Result<Session> {
        let parent_id = self.id().clone();
        let parent = self.meta();
        let cwd = parent.cwd;
        let child = match self.config() {
            Some(config) => Self::open_or_create(config, SessionId::new(), cwd).await?,
            None => Self::memory(SessionId::new(), cwd),
        };
        if let Err(error) = Self::link_child(&child, parent_id, parent.provider, model, title).await
        {
            discard_session(&child).await;
            return Err(error);
        }
        Ok(child)
    }
    pub(super) async fn link_child(
        child: &Session,
        parent_id: SessionId,
        provider: Option<String>,
        model: Option<String>,
        title: Option<String>,
    ) -> Result<()> {
        child
            .update_meta(|meta| {
                meta.parent_session_id = Some(parent_id);
                meta.provider = provider;
                if model.is_some() {
                    meta.model = model;
                }
                if title.is_some() {
                    meta.title = title;
                }
            })
            .await
    }
}

pub(super) fn latest_closed_turn_in_events(events: &[LoggedEvent]) -> Option<TurnId> {
    events.iter().rev().find_map(|logged| match &logged.event {
        SessionEvent::TurnEnded { turn_id, .. } => Some(turn_id.clone()),
        _ => None,
    })
}
pub(super) fn fork_prefix(
    events: &[LoggedEvent],
    boundary_turn: &TurnId,
) -> Result<Vec<LoggedEvent>> {
    let start = events
        .iter()
        .position(|logged| {
            matches!(&logged.event, SessionEvent::TurnStarted { turn_id, .. } if turn_id == boundary_turn)
        })
        .ok_or_else(|| anyhow::anyhow!("unknown turn: {boundary_turn}"))?;
    let end = events
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, logged)| {
            matches!(&logged.event, SessionEvent::TurnEnded { turn_id, .. } if turn_id == boundary_turn)
        })
        .map(|(index, _)| index)
        .ok_or_else(|| anyhow::anyhow!("turn {boundary_turn} is still open"))?;

    let prefix = &events[..=end];
    if !open_turns(prefix).is_empty() {
        anyhow::bail!("cannot fork: prefix ends with an open turn");
    }
    let mut copied = prefix.to_vec();
    for logged in &mut copied {
        reset_inherited_usage(&mut logged.event);
    }
    Ok(copied)
}
pub(super) fn reset_inherited_usage(event: &mut SessionEvent) {
    match event {
        SessionEvent::AssistantMessage {
            usage,
            stream_digest,
            ..
        } => {
            *usage = Usage::default();
            stream_digest.usage = Usage::default();
        }
        SessionEvent::AssistantAttempt { usage, .. } => {
            *usage = Usage::default();
        }
        SessionEvent::UsageUpdated { usage, cumulative } => {
            *usage = Usage::default();
            *cumulative = Usage::default();
        }
        _ => {}
    }
}
