use super::*;

pub(super) async fn handle_workspace_files(state: &AppState) -> Response<RespBody> {
    let Some(ws) = state.opener.workspace_query() else {
        return json_response(
            StatusCode::OK,
            &WorkspaceFilesResponse { files: Vec::new() },
        );
    };
    let files = tokio::task::spawn_blocking(move || discover_workspace_files(ws.as_ref()))
        .await
        .unwrap_or_default();
    json_response(StatusCode::OK, &WorkspaceFilesResponse { files })
}
pub(super) async fn handle_workspace_search(
    state: &AppState,
    req: &Request<Incoming>,
) -> Response<RespBody> {
    let params = parse_query_params(req.uri().query());
    let mode = match params.get("mode").map(String::as_str) {
        Some("content") => SearchMode::Content,
        _ => SearchMode::Files,
    };
    let query = params.get("q").cloned().unwrap_or_default();
    let Some(ws) = state.opener.workspace_query() else {
        return json_response(
            StatusCode::OK,
            &mapper::map_search_results(mode, query, Vec::new()),
        );
    };
    let q_clone = query.clone();
    let matches = tokio::task::spawn_blocking(move || {
        let files = discover_workspace_files(ws.as_ref());
        match mode {
            SearchMode::Files => search_files(&files, &q_clone),
            SearchMode::Content => search_content(ws.as_ref(), &files, &q_clone),
        }
    })
    .await
    .unwrap_or_default();
    json_response(
        StatusCode::OK,
        &mapper::map_search_results(mode, query, matches),
    )
}
pub(super) async fn handle_workspace_preview(
    state: &AppState,
    req: &Request<Incoming>,
) -> Response<RespBody> {
    let params = parse_query_params(req.uri().query());
    let Some(path) = params.get("path").cloned().filter(|p| !p.trim().is_empty()) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "missing path parameter",
        );
    };
    let Some(root) = state.opener.workspace_root() else {
        return json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "workspace root unavailable",
        );
    };
    if resolve_preview_path(&root, &path).is_none() && Path::new(&path).is_absolute() {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "path must be relative to workspace",
        );
    }
    let line = params.get("line").and_then(|v| v.parse::<usize>().ok());
    let height = params
        .get("height")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(40)
        .clamp(6, 200);
    match tokio::task::spawn_blocking(move || {
        let item = SearchMatch {
            path,
            line,
            column: None,
            snippet: String::new(),
        };
        build_search_preview(&root, &item, height)
    })
    .await
    {
        Ok(preview) => json_response(StatusCode::OK, &mapper::map_search_preview(preview)),
        Err(_) => internal_error("search preview task failed"),
    }
}
pub(super) async fn handle_workspace_changes(state: &AppState) -> Response<RespBody> {
    let Some(root) = state.opener.workspace_root() else {
        return json_response(
            StatusCode::OK,
            &Vec::<meerai_web_protocol::FileChangeDto>::new(),
        );
    };
    let changes = tokio::task::spawn_blocking(move || list_git_changes(&root))
        .await
        .unwrap_or_default();
    state.registry.emit_file_changes(changes.clone());
    let mapped: Vec<_> = changes.into_iter().map(mapper::map_file_change).collect();
    json_response(StatusCode::OK, &mapped)
}
pub(super) async fn handle_workspace_stage(
    state: &AppState,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let body: meerai_web_protocol::WorkspaceStageRequest = match parse_json(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    if !body.path.split(" -> ").all(is_safe_relative_workspace_spec) {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "invalid stage path",
        );
    }
    let Some(root) = state.opener.workspace_root() else {
        return json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "workspace root unavailable",
        );
    };
    let path = body.path;
    let stage = body.stage;
    let result = tokio::task::spawn_blocking(move || {
        if stage {
            stage_file(&root, &path)?;
        } else {
            unstage_file(&root, &path)?;
        }
        Ok::<_, String>(list_git_changes(&root))
    })
    .await;
    let changes = match result {
        Ok(Ok(changes)) => changes,
        Ok(Err(err)) => {
            return json_error(StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest, &err);
        }
        Err(err) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                &err.to_string(),
            );
        }
    };
    state.registry.emit_file_changes(changes.clone());
    let mapped: Vec<_> = changes.into_iter().map(mapper::map_file_change).collect();
    json_response(StatusCode::OK, &mapped)
}
pub(super) async fn handle_workspace_diff(
    state: &AppState,
    req: &Request<Incoming>,
) -> Response<RespBody> {
    let params = parse_query_params(req.uri().query());
    let Some(path) = params.get("path").cloned().filter(|p| !p.trim().is_empty()) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "missing path parameter",
        );
    };
    if !is_safe_relative_workspace_spec(&path) {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "invalid diff path",
        );
    }
    let status = params
        .get("status")
        .cloned()
        .unwrap_or_else(|| "M".to_string());
    let Some(root) = state.opener.workspace_root() else {
        return json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "workspace root unavailable",
        );
    };
    let diff =
        match tokio::task::spawn_blocking(move || build_file_diff(&root, &path, &status)).await {
            Ok(diff) => diff,
            Err(_) => return internal_error("diff task failed"),
        };
    json_response(StatusCode::OK, &mapper::map_file_diff(diff))
}
pub(super) async fn handle_host_action(
    state: &AppState,
    req: Request<Incoming>,
) -> Response<RespBody> {
    let body: HostActionRequest = match parse_json(req).await {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let Some(action) = meerai_host::parse_host_action(&body.command) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "unsupported host command",
        );
    };
    let snapshot = state.registry.snapshot();
    let session_id = snapshot.active_session_id;
    let cmd_tx = session_id.as_ref().and_then(|id| state.registry.cmd_tx(id));
    let context = session_id
        .as_ref()
        .and_then(|id| state.registry.context_for(id));
    let project_root = state
        .opener
        .workspace_root()
        .unwrap_or_else(|| PathBuf::from("."));
    let result = if matches!(action, HostAction::InspectContext) {
        if let Some(cmd_tx) = &cmd_tx {
            let _ = cmd_tx.send(AgentCommand::InspectContext);
        }
        Ok(render_context(context.as_ref()))
    } else {
        execute_host_action(
            HostActionContext {
                opener: state.opener.as_ref(),
                project_root: &project_root,
                session_id: session_id.as_ref(),
                cmd_tx: cmd_tx.as_ref(),
            },
            action,
        )
        .await
    };
    match result {
        Ok(text) => json_response(StatusCode::OK, &HostActionResponse { text }),
        Err(err) => json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &format!("{err:#}"),
        ),
    }
}
