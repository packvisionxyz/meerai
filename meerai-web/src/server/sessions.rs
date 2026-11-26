use super::*;

pub(super) async fn handle_create_session(
    state: &AppState,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let create_req: CreateSessionRequest = match parse_optional_json(req).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };

    let opened = match create_req.backend {
        Some(BackendKind::Acp { preset, .. }) => {
            if !state.auth.is_enabled() {
                return unauthorized_response("authentication required for ACP sessions");
            }
            state.opener.create_acp_session(&preset).await
        }
        None | Some(BackendKind::Native) => state.opener.create_session().await,
    };

    let opened = match opened {
        Ok(opened) => opened,
        Err(err) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                &format!("failed to create session: {err}"),
            );
        }
    };
    let summary = state.registry.register_opened_active(opened);
    let resp = CreateSessionResponse {
        session: mapper::map_summary(summary),
    };
    json_response(StatusCode::CREATED, &resp)
}
pub(super) async fn handle_sessions_history(state: &AppState) -> Response<RespBody> {
    let stored = match state.opener.list_sessions().await {
        Ok(list) => list,
        Err(err) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                &format!("failed to list sessions: {err}"),
            );
        }
    };
    let sessions = stored
        .into_iter()
        .map(|s| StoredSessionDto {
            is_open: state.registry.has_session(&s.id),
            id: s.id.to_string(),
            title: s.title,
            model: s.model,
            created_at_ms: s.created_at_ms,
            updated_at_ms: s.updated_at_ms,
            bytes: s.bytes,
            is_child: s.parent_session_id.is_some(),
        })
        .collect();
    json_response(StatusCode::OK, &SessionHistoryResponse { sessions })
}
pub(super) fn handle_session_detail(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let Some(detail) = state.registry.session_detail(&session_id) else {
        return json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "session not found",
        );
    };
    json_response(StatusCode::OK, &mapper::map_detail(detail))
}
pub(super) async fn handle_session_events(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if let Some(detail) = state.registry.session_detail(&session_id) {
        let mapped = mapper::map_detail(detail);
        return json_response(
            StatusCode::OK,
            &SessionEventsResponse {
                session_id: session_id.to_string(),
                entries: mapped.entries,
                usage: mapped.usage,
            },
        );
    }
    match state.opener.read_session_events(&session_id).await {
        Ok(events) => {
            let (entries, usage) = mapper::map_stored_events(&events);
            json_response(
                StatusCode::OK,
                &SessionEventsResponse {
                    session_id: session_id.to_string(),
                    entries,
                    usage,
                },
            )
        }
        Err(err) => json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            &format!("failed to read session events: {err}"),
        ),
    }
}
pub(super) async fn handle_open_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if let Some(summary) = state.registry.session_summary(&session_id) {
        let _ = state.registry.set_active_session(&session_id);
        return json_response(
            StatusCode::OK,
            &CreateSessionResponse {
                session: mapper::map_summary(summary),
            },
        );
    }
    let opened = match state.opener.open_session(&session_id).await {
        Ok(opened) => opened,
        Err(err) => {
            return json_error(
                StatusCode::CONFLICT,
                ApiErrorCode::ConflictStale,
                &format!("failed to open session: {err}"),
            );
        }
    };
    let summary = state.registry.register_opened_active(opened);
    json_response(
        StatusCode::OK,
        &CreateSessionResponse {
            session: mapper::map_summary(summary),
        },
    )
}
pub(super) fn handle_activate_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.registry.set_active_session(&session_id) {
        Ok(()) => {
            let summary = state.registry.session_summary(&session_id).unwrap();
            json_response(
                StatusCode::OK,
                &CreateSessionResponse {
                    session: mapper::map_summary(summary),
                },
            )
        }
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_close_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.registry.close_and_wait_session(&session_id).await {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "closed": true })),
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_delete_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if state.registry.has_session(&session_id) {
        let _ = state.registry.close_and_wait_session(&session_id).await;
    }
    match state.opener.delete_session(&session_id).await {
        Ok(deleted) => json_response(StatusCode::OK, &serde_json::json!({ "deleted": deleted })),
        Err(err) => json_error(
            StatusCode::CONFLICT,
            ApiErrorCode::ConflictStale,
            &format!("failed to delete session: {err}"),
        ),
    }
}
pub(super) async fn handle_fork_session(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let fork_req: ForkSessionRequest = match parse_optional_json(req).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };
    let turn_id = match fork_req.turn_id {
        Some(raw) if !raw.trim().is_empty() => TurnId::from_raw(raw.trim()),
        _ => match state.opener.read_session_events(&session_id).await {
            Ok(events) => match events
                .into_iter()
                .rev()
                .find_map(|logged| match logged.event {
                    SessionEvent::TurnEnded { turn_id, .. } => Some(turn_id),
                    _ => None,
                }) {
                Some(turn_id) => turn_id,
                None => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        ApiErrorCode::BadRequest,
                        "session has no closed turns to fork",
                    );
                }
            },
            Err(err) => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    ApiErrorCode::BadRequest,
                    &format!("failed to inspect source session for fork: {err}"),
                );
            }
        },
    };

    match state.opener.fork_session(&session_id, &turn_id).await {
        Ok(opened) => {
            let summary = state.registry.register_opened_active(opened);
            json_response(
                StatusCode::CREATED,
                &CreateSessionResponse {
                    session: mapper::map_summary(summary),
                },
            )
        }
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &format!("failed to fork session: {err}"),
        ),
    }
}
pub(super) async fn handle_btw_session(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let btw_req: BtwSessionRequest = match parse_optional_json(req).await {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };
    let opened = match state.opener.btw_session(&session_id).await {
        Ok(opened) => opened,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                &format!("failed to open btw session: {err}"),
            );
        }
    };
    let btw_id = opened.session_id.clone();
    let summary = state.registry.register_opened_active(opened);
    if let Some(text) = btw_req.text.filter(|text| !text.trim().is_empty())
        && let Err(err) = state.registry.submit_prompt(&btw_id, text, None)
    {
        return host_err_response(err);
    }
    json_response(
        StatusCode::CREATED,
        &CreateSessionResponse {
            session: mapper::map_summary(summary),
        },
    )
}
pub(super) async fn handle_rewind_session(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let rewind_req: RewindRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let turn_id = TurnId::from_raw(rewind_req.turn_id.trim());
    let scope = mapper::unmap_rewind_scope(rewind_req.scope);
    match state.registry.rewind(&session_id, turn_id, scope) {
        Ok(restored_prompt) => json_response(
            StatusCode::OK,
            &RewindResponse {
                accepted: true,
                restored_prompt,
            },
        ),
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_approval_decision(
    state: &AppState,
    raw_id: &str,
    raw_req_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let body: ApprovalDecisionRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let request_id = RequestId::from_raw(raw_req_id);
    let decision = mapper::unmap_approval_decision(body.decision);
    let scope = body.scope.map(mapper::unmap_grant_scope);
    match state
        .registry
        .decide_approval(&session_id, request_id, decision, scope)
    {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_question_answer(
    state: &AppState,
    raw_id: &str,
    raw_req_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let body: QuestionAnswerRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let request_id = RequestId::from_raw(raw_req_id);
    let answers = (!body.cancel).then(|| mapper::unmap_question_answers(body.answers));
    match state
        .registry
        .answer_question(&session_id, request_id, answers)
    {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(err) => host_err_response(err),
    }
}
pub(super) fn handle_session_context(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = SessionId::from_raw(raw_id);
    match state.registry.context_for(&session_id) {
        Some(breakdown) => json_response(StatusCode::OK, &mapper::map_context_breakdown(breakdown)),
        None => json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "no context report for this session yet",
        ),
    }
}
pub(super) async fn handle_session_blob(
    state: &AppState,
    raw_id: &str,
    blob_key: &str,
) -> Response<RespBody> {
    let session_id = SessionId::from_raw(raw_id);
    match state.opener.read_session_blob(&session_id, blob_key).await {
        Ok(content) => json_response(StatusCode::OK, &BlobResponse { content }),
        Err(err) => json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            &format!("failed to read blob: {err:#}"),
        ),
    }
}
pub(super) fn extract_blob_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (id, rest) = rest.split_once("/blobs/")?;
    (!id.is_empty() && !rest.is_empty()).then_some((id, rest))
}
