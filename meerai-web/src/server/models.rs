use super::*;

pub(super) async fn handle_set_model(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let body: SetModelRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let resolved = match state
        .opener
        .select_model(body.provider.trim(), body.model.trim())
        .await
    {
        Ok(res) => res,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                &format!("failed to select model: {err}"),
            );
        }
    };
    match state.registry.set_model(
        &session_id,
        resolved.provider,
        resolved.display_model,
        resolved.command_model,
        resolved.context_window_tokens,
        resolved.effort_levels,
    ) {
        Ok(()) => {
            state.registry.emit_providers_updated();
            let summary = state.registry.session_summary(&session_id).unwrap();
            json_response(
                StatusCode::OK,
                &SetModelResponse {
                    session: mapper::map_summary(summary),
                },
            )
        }
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_set_mode(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let body: SetModeRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let mode = mapper::unmap_approval_mode(body.mode);
    match state.registry.set_approval_mode(&session_id, mode) {
        Ok(()) => {
            let summary = state.registry.session_summary(&session_id).unwrap();
            json_response(
                StatusCode::OK,
                &SetModelResponse {
                    session: mapper::map_summary(summary),
                },
            )
        }
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_apply_agent(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let body: ApplyDefinitionRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let Some(summary) = state.registry.session_summary(&session_id) else {
        return host_err_response(HostError::SessionNotFound(session_id));
    };
    let resolved = match state
        .opener
        .resolve_agent_application(body.name.trim(), &summary.provider)
        .await
    {
        Ok(app) => app,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                &err.to_string(),
            );
        }
    };
    match state.registry.apply_agent(&session_id, resolved) {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_apply_skill(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let body: ApplyDefinitionRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let resolved = match state
        .opener
        .resolve_skill_application(body.name.trim())
        .await
    {
        Ok(app) => app,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                &err.to_string(),
            );
        }
    };
    match state.registry.apply_skill(&session_id, resolved) {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(err) => host_err_response(err),
    }
}
