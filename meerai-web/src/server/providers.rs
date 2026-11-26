use super::*;

pub(super) async fn handle_get_providers(state: &AppState) -> Response<RespBody> {
    match state.opener.providers_snapshot().await {
        Ok(snapshot) => json_response(StatusCode::OK, &mapper::map_providers(snapshot)),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to list providers: {err}"),
        ),
    }
}
pub(super) async fn handle_set_api_key(
    state: &AppState,
    provider_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let body: SetApiKeyRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    if body.api_key.trim().is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "apiKey must not be empty",
        );
    }
    match state
        .opener
        .set_api_key(provider_id, body.api_key.trim())
        .await
    {
        Ok(()) => {
            state.registry.emit_providers_updated();
            json_response(StatusCode::OK, &serde_json::json!({ "ok": true }))
        }
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &err.to_string(),
        ),
    }
}
pub(super) async fn handle_remove_credential(
    state: &AppState,
    provider_id: &str,
) -> Response<RespBody> {
    match state.opener.remove_credential(provider_id).await {
        Ok(()) => {
            state.registry.emit_providers_updated();
            json_response(StatusCode::OK, &serde_json::json!({ "ok": true }))
        }
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &err.to_string(),
        ),
    }
}
pub(super) async fn handle_set_base_url(
    state: &AppState,
    provider_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let body: SetProviderBaseUrlRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    match state
        .opener
        .set_provider_base_url(provider_id, body.base_url)
        .await
    {
        Ok(()) => {
            state.registry.emit_providers_updated();
            json_response(StatusCode::OK, &serde_json::json!({ "ok": true }))
        }
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &err.to_string(),
        ),
    }
}
pub(super) async fn handle_chatgpt_start(
    state: &Arc<AppState>,
    provider_id: &str,
) -> Response<RespBody> {
    let login = match state.opener.start_chatgpt_login(provider_id).await {
        Ok(login) => login,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                &err.to_string(),
            );
        }
    };
    let seq = state.oauth_seq.fetch_add(1, Ordering::Relaxed) + 1;
    let attempt_id = format!("oauth-{seq}");
    let url = login.url;
    {
        let Ok(mut guard) = state.oauth_attempts.lock() else {
            return internal_error("oauth attempt registry was poisoned");
        };
        guard.insert(
            attempt_id.clone(),
            ActiveOAuthAttempt {
                provider_id: provider_id.to_string(),
                paste_tx: login.paste_tx,
                cancel_tx: Some(login.cancel_tx),
            },
        );
    }

    state
        .registry
        .emit_chatgpt_login(ChatGptLoginStatus::Started {
            attempt_id: attempt_id.clone(),
            provider_id: provider_id.to_string(),
            url: url.clone(),
        });

    let state_bg = Arc::clone(state);
    let attempt_bg = attempt_id.clone();
    let provider_bg = provider_id.to_string();
    tokio::spawn(async move {
        let res = login.result_rx.await;
        let still_present = state_bg
            .oauth_attempts
            .lock()
            .map(|mut guard| guard.remove(&attempt_bg))
            .unwrap_or(None)
            .is_some();
        if !still_present {
            return;
        }
        match res {
            Ok(Ok(outcome)) => {
                state_bg
                    .registry
                    .emit_chatgpt_login(ChatGptLoginStatus::Completed {
                        attempt_id: attempt_bg,
                        provider_id: outcome.provider_id,
                        account: outcome.account,
                    });
                state_bg.registry.emit_providers_updated();
            }
            Ok(Err(err)) => {
                state_bg
                    .registry
                    .emit_chatgpt_login(ChatGptLoginStatus::Failed {
                        attempt_id: attempt_bg,
                        provider_id: provider_bg,
                        message: err.to_string(),
                    });
            }
            Err(_) => {
                state_bg
                    .registry
                    .emit_chatgpt_login(ChatGptLoginStatus::Cancelled {
                        attempt_id: attempt_bg,
                        provider_id: provider_bg,
                    });
            }
        }
    });

    json_response(
        StatusCode::CREATED,
        &ChatGptLoginStartResponse {
            attempt_id,
            provider_id: provider_id.to_string(),
            url,
        },
    )
}
pub(super) async fn handle_chatgpt_paste(
    state: &AppState,
    provider_id: &str,
    attempt_id: &str,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let body: ChatGptLoginPasteRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let paste_tx = {
        let Ok(guard) = state.oauth_attempts.lock() else {
            return internal_error("oauth attempt registry was poisoned");
        };
        let Some(entry) = guard.get(attempt_id) else {
            return json_error(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "OAuth login attempt not found or already finished",
            );
        };
        if entry.provider_id != provider_id {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                "provider mismatch for login attempt",
            );
        }
        entry.paste_tx.clone()
    };
    if paste_tx.send(body.input).is_err() {
        return json_error(
            StatusCode::CONFLICT,
            ApiErrorCode::ConflictStale,
            "OAuth login attempt is no longer active",
        );
    }
    json_response(StatusCode::OK, &serde_json::json!({ "accepted": true }))
}
pub(super) fn handle_chatgpt_cancel(
    state: &AppState,
    provider_id: &str,
    attempt_id: &str,
) -> Response<RespBody> {
    let cancel_tx = {
        let Ok(mut guard) = state.oauth_attempts.lock() else {
            return internal_error("oauth attempt registry was poisoned");
        };
        let Some(entry) = guard.get(attempt_id) else {
            return json_error(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "OAuth login attempt not found",
            );
        };
        if entry.provider_id != provider_id {
            return json_error(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::BadRequest,
                "provider mismatch for login attempt",
            );
        }
        guard
            .remove(attempt_id)
            .and_then(|mut entry| entry.cancel_tx.take())
    };
    if let Some(tx) = cancel_tx {
        let _ = tx.send(());
    }
    state
        .registry
        .emit_chatgpt_login(ChatGptLoginStatus::Cancelled {
            attempt_id: attempt_id.to_string(),
            provider_id: provider_id.to_string(),
        });
    json_response(StatusCode::OK, &serde_json::json!({ "cancelled": true }))
}
