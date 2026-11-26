use super::*;

pub(super) async fn handle_submit_prompt(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let prompt_req: SubmitPromptRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    match state
        .registry
        .submit_prompt(&session_id, prompt_req.prompt, prompt_req.display)
    {
        Ok(()) => json_response(
            StatusCode::OK,
            &SubmitPromptResponse {
                accepted: true,
                session_id: session_id.to_string(),
            },
        ),
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_steer_prompt(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let prompt_req: SubmitPromptRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    match state
        .registry
        .steer_prompt(&session_id, prompt_req.prompt, prompt_req.display)
    {
        Ok(()) => json_response(
            StatusCode::OK,
            &SubmitPromptResponse {
                accepted: true,
                session_id: session_id.to_string(),
            },
        ),
        Err(err) => host_err_response(err),
    }
}
pub(super) fn handle_simple_session_cmd(
    _state: &AppState,
    raw_id: &str,
    op: impl FnOnce(&SessionId) -> Result<(), HostError>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match op(&session_id) {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(err) => host_err_response(err),
    }
}
pub(super) async fn handle_user_shell(
    state: &AppState,
    raw_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if state
        .registry
        .session_summary(&session_id)
        .is_some_and(|summary| summary.transient)
    {
        return json_error(
            StatusCode::FORBIDDEN,
            ApiErrorCode::Forbidden,
            "btw is read-only",
        );
    }
    let body: RunShellRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let command = body.command.trim().to_string();
    if command.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "shell command must not be empty",
        );
    }
    let run_id = match state
        .registry
        .begin_user_shell(&session_id, &command, body.run_id)
    {
        Ok(id) => id,
        Err(err) => return host_err_response(err),
    };
    let result = state.opener.run_user_shell(&command).await;
    if let Some(root) = state.opener.workspace_root() {
        refresh_file_changes_once(&root, &state.registry).await;
    }
    match result {
        Ok(output) => {
            state
                .registry
                .finish_user_shell(&session_id, &run_id, Ok(output.clone()));
            json_response(StatusCode::OK, &RunShellResponse { output })
        }
        Err(err) => {
            let msg = err.to_string();
            state
                .registry
                .finish_user_shell(&session_id, &run_id, Err(msg.clone()));
            json_error(StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest, &msg)
        }
    }
}
