use super::*;

pub(super) async fn handle_memories(
    state: &AppState,
    req: &Request<Incoming>,
) -> Response<RespBody> {
    let params = parse_query_params(req.uri().query());
    let project_root = state
        .opener
        .workspace_root()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let result: anyhow::Result<Vec<ProjectMemory>> = match params
        .get("query")
        .map(String::as_str)
        .filter(|query| !query.trim().is_empty())
    {
        Some(query) => state.opener.search_memories(&project_root, query).await,
        None => state.opener.list_memories(&project_root).await,
    };
    match result {
        Ok(memories) => json_response(
            StatusCode::OK,
            &MemoriesResponse {
                memories: memories.into_iter().map(mapper::map_memory).collect(),
            },
        ),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to list memories: {err:#}"),
        ),
    }
}
pub(super) async fn handle_routes(state: &AppState) -> Response<RespBody> {
    match state.opener.routes().await {
        Ok(routes) => json_response(
            StatusCode::OK,
            &RoutesResponse {
                routes: routes.routes.into_iter().map(mapper::map_route).collect(),
            },
        ),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to list routes: {err:#}"),
        ),
    }
}
pub(super) async fn handle_packs(state: &AppState) -> Response<RespBody> {
    match state.opener.list_packs().await {
        Ok(packs) => json_response(
            StatusCode::OK,
            &PacksResponse {
                packs: packs.into_iter().map(mapper::map_pack).collect(),
            },
        ),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to list packs: {err:#}"),
        ),
    }
}
pub(super) async fn handle_definitions(state: &AppState) -> Response<RespBody> {
    match state.opener.definitions_diagnostics().await {
        Ok(diagnostics) => json_response(StatusCode::OK, &mapper::map_definitions(diagnostics)),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to inspect definitions: {err:#}"),
        ),
    }
}
