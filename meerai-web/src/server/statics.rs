use super::*;

pub(super) async fn handle_static(path: &str) -> Response<RespBody> {
    let rel = if path == "/" {
        "index.html"
    } else {
        path.strip_prefix('/').unwrap_or(path)
    };

    if !is_safe_asset_path(rel) {
        return text_response(StatusCode::BAD_REQUEST, "invalid asset path");
    }

    if let Some(bytes) = embedded::embedded_asset(rel) {
        return asset_response(rel, Bytes::from_static(bytes));
    }

    let has_embedded = embedded::embedded_asset("index.html").is_some();
    if has_embedded {
        return text_response(StatusCode::NOT_FOUND, "asset not found");
    }

    let runtime_dir = resolve_runtime_web_dir();
    let Ok(base_canon) = tokio::fs::canonicalize(&runtime_dir).await else {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "web bundle is not built; run wasm build or set MEERAI_WEB_DIR",
        );
    };
    let Ok(index_meta) = tokio::fs::metadata(base_canon.join("index.html")).await else {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "web bundle is not built; run wasm build or set MEERAI_WEB_DIR",
        );
    };
    if !index_meta.is_file() {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "web bundle is not built; run wasm build or set MEERAI_WEB_DIR",
        );
    }

    let candidate = base_canon.join(rel);
    let Ok(file_canon) = tokio::fs::canonicalize(&candidate).await else {
        return text_response(StatusCode::NOT_FOUND, "asset not found");
    };
    if !file_canon.starts_with(&base_canon) {
        return text_response(StatusCode::FORBIDDEN, "forbidden");
    }

    match tokio::fs::read(&file_canon).await {
        Ok(bytes) => asset_response(rel, Bytes::from(bytes)),
        Err(_) => text_response(StatusCode::NOT_FOUND, "asset not found"),
    }
}
pub(super) fn is_safe_asset_path(rel: &str) -> bool {
    if rel.is_empty()
        || rel.contains("..")
        || rel.contains('\\')
        || rel.contains('\0')
        || rel.to_ascii_lowercase().contains("%2e")
        || rel.to_ascii_lowercase().contains("%2f")
        || rel.to_ascii_lowercase().contains("%5c")
    {
        return false;
    }

    let mut segments = rel.split('/');
    if segments.clone().any(|seg| {
        seg.is_empty()
            || seg == "."
            || seg == ".."
            || matches!(seg, ".next" | ".stamp" | "reload.js")
    }) {
        return false;
    }
    segments.next().is_some()
}
pub(super) fn resolve_runtime_web_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MEERAI_WEB_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    Path::new("target").join("web")
}
pub(super) fn asset_content_type(rel: &str) -> String {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("wasm") => "application/wasm".to_string(),
        Some("js" | "mjs") => "application/javascript; charset=utf-8".to_string(),
        Some("html") => "text/html; charset=utf-8".to_string(),
        Some("ttf") => "font/ttf".to_string(),
        Some("css") => "text/css; charset=utf-8".to_string(),
        Some("json") => "application/json".to_string(),
        Some("svg") => "image/svg+xml".to_string(),
        Some("ico") => "image/x-icon".to_string(),
        Some("png") => "image/png".to_string(),
        _ => mime_guess::from_path(rel)
            .first_or_octet_stream()
            .to_string(),
    }
}
pub(super) fn asset_response(rel: &str, bytes: Bytes) -> Response<RespBody> {
    let content_type = asset_content_type(rel);
    match Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, content_type)
        .body(Full::new(bytes).boxed())
    {
        Ok(response) => response,
        Err(_) => internal_error("failed to build asset response"),
    }
}
pub(super) fn finalize_response(path: &str, mut resp: Response<RespBody>) -> Response<RespBody> {
    let headers = resp.headers_mut();
    headers.insert(
        http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        http::header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP_HEADER),
    );
    if is_api_path(path) {
        headers.insert(
            http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
    }
    resp
}
