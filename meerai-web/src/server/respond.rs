use super::*;

pub(super) fn unauthorized_response(message: &str) -> Response<RespBody> {
    let mut resp = json_error(
        StatusCode::UNAUTHORIZED,
        ApiErrorCode::Unauthorized,
        message,
    );
    resp.headers_mut().insert(
        http::header::WWW_AUTHENTICATE,
        HeaderValue::from_static(WWW_AUTHENTICATE_VALUE),
    );
    resp
}
pub(super) fn method_not_allowed() -> Response<RespBody> {
    json_error(
        StatusCode::METHOD_NOT_ALLOWED,
        ApiErrorCode::BadRequest,
        "method not allowed",
    )
}
pub(super) fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}
pub(super) fn extract_single_session_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/sessions/")?;
    if rest.is_empty() || rest == "history" || rest.contains('/') {
        None
    } else {
        Some(rest)
    }
}
pub(super) fn extract_session_action<'a>(path: &'a str, action: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let suffix = format!("/{action}");
    let id = rest.strip_suffix(&suffix)?;
    if id.is_empty() || id.contains('/') {
        None
    } else {
        Some(id)
    }
}
pub(super) fn extract_session_subaction<'a>(
    path: &'a str,
    section: &str,
) -> Option<(&'a str, &'a str)> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (id, tail) = rest.split_once('/')?;
    let item_id = tail.strip_prefix(&format!("{section}/"))?;
    if id.is_empty() || item_id.is_empty() || item_id.contains('/') {
        None
    } else {
        Some((id, item_id))
    }
}
pub(super) fn extract_provider_action<'a>(path: &'a str, suffix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/api/providers/")?;
    let id = rest.strip_suffix(&format!("/{suffix}"))?;
    if id.is_empty() || id.contains('/') {
        None
    } else {
        Some(id)
    }
}
pub(super) fn extract_provider_chatgpt_action<'a>(
    path: &'a str,
    action: &str,
) -> Option<(&'a str, &'a str)> {
    let rest = path.strip_prefix("/api/providers/")?;
    let (id, tail) = rest.split_once("/chatgpt/")?;
    let attempt = tail.strip_prefix("")?.strip_suffix(&format!("/{action}"))?;
    if id.is_empty() || id.contains('/') || attempt.is_empty() || attempt.contains('/') {
        None
    } else {
        Some((id, attempt))
    }
}
#[allow(clippy::result_large_err)]
pub(super) fn parse_session_id(raw: &str) -> Result<SessionId, Response<RespBody>> {
    raw.parse().map_err(|_| {
        json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "invalid session id",
        )
    })
}
#[allow(clippy::result_large_err)]
pub(super) async fn parse_json<T: DeserializeOwned>(
    req: Request<Incoming>,
) -> Result<T, Response<RespBody>> {
    let body_bytes = read_json_body(req)
        .await
        .map_err(|msg| json_error(StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest, msg))?;
    serde_json::from_slice(&body_bytes).map_err(|err| {
        json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &format!("invalid JSON body: {err}"),
        )
    })
}
#[allow(clippy::result_large_err)]
pub(super) async fn parse_optional_json<T: DeserializeOwned + Default>(
    req: Request<Incoming>,
) -> Result<T, Response<RespBody>> {
    let body_bytes = read_json_body(req)
        .await
        .map_err(|msg| json_error(StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest, msg))?;
    if body_bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(&body_bytes).map_err(|err| {
        json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            &format!("invalid JSON body: {err}"),
        )
    })
}
pub(super) fn host_err_response(err: HostError) -> Response<RespBody> {
    let (status, api_err) = mapper::map_host_error(err);
    json_response(status, &api_err)
}
pub(super) fn is_safe_relative_workspace_spec(spec: &str) -> bool {
    let (old_rel, new_rel) = spec.split_once(" -> ").unwrap_or((spec, spec));
    [old_rel, new_rel].iter().all(|part| {
        let p = Path::new(part);
        !part.is_empty()
            && !p.is_absolute()
            && !p.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
    })
}
pub(super) fn parse_query_params(raw_query: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(raw) = raw_query else {
        return map;
    };
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}
pub(super) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push(((h << 4) | l) as u8);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}
pub(super) async fn read_json_body(req: Request<Incoming>) -> Result<Bytes, &'static str> {
    let limited = Limited::new(req.into_body(), MAX_JSON_BODY_BYTES);
    match limited.collect().await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(_) => Err("failed to read request body or payload too large"),
    }
}
pub(super) fn text_response(status: StatusCode, msg: &str) -> Response<RespBody> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(msg.to_owned())).boxed())
        .unwrap()
}
pub(super) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<RespBody> {
    let bytes = serde_json::to_vec(value).expect("JSON response serializes");
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(bytes)).boxed())
        .unwrap()
}
pub(super) fn json_error(
    status: StatusCode,
    code: ApiErrorCode,
    message: &str,
) -> Response<RespBody> {
    let err = ApiError {
        code,
        message: message.to_string(),
    };
    json_response(status, &err)
}
