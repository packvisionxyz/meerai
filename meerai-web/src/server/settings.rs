use super::*;

pub(super) async fn handle_update_settings(
    state: &AppState,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let body: UpdateSettingsRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let update = mapper::map_settings_update(body);
    match state.opener.update_settings(update).await {
        Ok(outcome) => {
            state.registry.emit_reload(&outcome);
            json_response(
                StatusCode::OK,
                &ReloadResponse {
                    host_info: mapper::map_host_info(outcome.host_info),
                    warnings: outcome.warnings,
                },
            )
        }
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &err.to_string(),
        ),
    }
}
pub(super) async fn handle_reload(state: &AppState) -> Response<RespBody> {
    match state.opener.reload_config().await {
        Ok(outcome) => {
            state.registry.emit_reload(&outcome);
            json_response(
                StatusCode::OK,
                &ReloadResponse {
                    host_info: mapper::map_host_info(outcome.host_info),
                    warnings: outcome.warnings,
                },
            )
        }
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("reload failed: {err}"),
        ),
    }
}
