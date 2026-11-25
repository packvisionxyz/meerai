use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use meerai_agents::{AgentCommand, TurnId};
use meerai_host::diff::{build_file_diff, list_git_changes, stage_file, unstage_file};
use meerai_host::search::{
    SearchMatch, SearchMode, build_search_preview, discover_workspace_files, resolve_preview_path,
    search_content, search_files,
};
use meerai_host::{
    ChatGptLoginStatus, HostAction, HostActionContext, HostError, ProjectMemory, RegistryEventKind,
    SessionOpener, SessionRegistry, SubscribeResult, execute_host_action, render_context,
};

use meerai_session::{RequestId, SessionEvent, SessionId};
use meerai_web_protocol::{
    ApiError, ApiErrorCode, ApplyDefinitionRequest, ApprovalDecisionRequest, BackendKind,
    BlobResponse, BtwSessionRequest, ChatGptLoginPasteRequest, ChatGptLoginStartResponse,
    CreateSessionRequest, CreateSessionResponse, ForkSessionRequest, HostActionRequest,
    HostActionResponse, MemoriesResponse, PacksResponse, QuestionAnswerRequest, ReloadResponse,
    RewindRequest, RewindResponse, RoutesResponse, RunShellRequest, RunShellResponse,
    SessionEventsResponse, SessionHistoryResponse, SetApiKeyRequest, SetModeRequest,
    SetModelRequest, SetModelResponse, SetProviderBaseUrlRequest, StoredSessionDto,
    SubmitPromptRequest, SubmitPromptResponse, UpdateSettingsRequest, ViewEvent,
    WorkspaceFilesResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::{
    AuthBackoff, RequestCheckError, ServeAuth, WWW_AUTHENTICATE_VALUE, is_state_changing,
    log_rejection, validate_host, validate_state_changing, verify_basic_auth,
};
use crate::mapper;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_bundle.rs"));
}

const CSP_HEADER: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self'";
const MAX_JSON_BODY_BYTES: usize = 1024 * 1024;
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const SSE_CHANNEL_CAPACITY: usize = 256;

type RespBody = BoxBody<Bytes, Infallible>;

#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub bind_addr: SocketAddr,
    pub auth: ServeAuth,
}

pub struct RunningServer {
    pub addr: SocketAddr,
    pub shutdown_tx: oneshot::Sender<()>,
    pub handle: JoinHandle<anyhow::Result<()>>,
}

impl RunningServer {
    pub async fn shutdown(self) -> anyhow::Result<()> {
        let _ = self.shutdown_tx.send(());
        self.handle.await.map_err(anyhow::Error::from)?
    }
}

struct ActiveOAuthAttempt {
    provider_id: String,
    paste_tx: mpsc::UnboundedSender<String>,
    cancel_tx: Option<oneshot::Sender<()>>,
}

struct AppState {
    port: u16,
    auth: ServeAuth,
    backoff: AuthBackoff,
    opener: Arc<dyn SessionOpener>,
    registry: Arc<SessionRegistry>,
    shutdown_rx: watch::Receiver<bool>,
    oauth_seq: AtomicU64,
    oauth_attempts: Mutex<HashMap<String, ActiveOAuthAttempt>>,
}

pub async fn start_server(
    config: ServeConfig,
    opener: Arc<dyn SessionOpener>,
    registry: Arc<SessionRegistry>,
) -> anyhow::Result<RunningServer> {
    if !config.bind_addr.ip().is_loopback() {
        anyhow::bail!(
            "refusing to bind non-loopback address {}; use 127.0.0.1",
            config.bind_addr
        );
    }

    let listener = TcpListener::bind(config.bind_addr).await?;
    let addr = listener.local_addr()?;
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let (stop_tx, stop_rx) = watch::channel(false);

    let state = Arc::new(AppState {
        port: addr.port(),
        auth: config.auth,
        backoff: AuthBackoff::new(),
        opener,
        registry,
        shutdown_rx: stop_rx.clone(),
        oauth_seq: AtomicU64::new(0),
        oauth_attempts: Mutex::new(HashMap::new()),
    });

    spawn_file_changes_watcher(&state, stop_rx.clone());

    let handle = tokio::spawn(async move {
        let mut conns = JoinSet::new();

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    let _ = stop_tx.send(true);
                    break;
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let state = Arc::clone(&state);
                    let mut conn_stop = stop_rx.clone();
                    conns.spawn(async move {
                        let io = TokioIo::new(stream);
                        let service = service_fn(move |req| {
                            let state = Arc::clone(&state);
                            handle_request(state, req)
                        });
                        let mut conn = http1::Builder::new().serve_connection(io, service);
                        tokio::select! {
                            res = &mut conn => {
                                let _ = res;
                            }
                            _ = conn_stop.changed() => {
                                std::pin::Pin::new(&mut conn).graceful_shutdown();
                                let _ = tokio::time::timeout(Duration::from_secs(1), conn).await;
                            }
                        }
                    });
                }
            }
            while conns.try_join_next().is_some() {}
        }

        while conns.join_next().await.is_some() {}
        Ok(())
    });

    Ok(RunningServer {
        addr,
        shutdown_tx,
        handle,
    })
}

fn spawn_file_changes_watcher(state: &Arc<AppState>, mut stop_rx: watch::Receiver<bool>) {
    let Some(root) = state.opener.workspace_root() else {
        return;
    };
    let registry = Arc::clone(&state.registry);
    tokio::spawn(async move {
        refresh_file_changes_once(&root, &registry).await;
        let SubscribeResult::Replay { mut rx, .. } = registry.subscribe(None) else {
            return;
        };
        let mut poll = tokio::time::interval(std::time::Duration::from_secs(4));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        poll.tick().await;
        loop {
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
                _ = poll.tick() => {
                    refresh_file_changes_once(&root, &registry).await;
                }
                ev = rx.recv() => {
                    match ev {
                        Ok(event) if matches!(
                            event.kind,
                            RegistryEventKind::TurnCompleted { .. }
                                | RegistryEventKind::SessionRewound { .. }
                        ) => {
                            refresh_file_changes_once(&root, &registry).await;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            refresh_file_changes_once(&root, &registry).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

async fn refresh_file_changes_once(root: &Path, registry: &SessionRegistry) {
    let root = root.to_path_buf();
    if let Ok(changes) = tokio::task::spawn_blocking(move || list_git_changes(&root)).await {
        registry.emit_file_changes(changes);
    }
}

async fn handle_request(
    state: Arc<AppState>,
    req: Request<Incoming>,
) -> Result<Response<RespBody>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let resp = dispatch_request(state, req, &method, &path).await;
    Ok(finalize_response(&path, resp))
}

async fn dispatch_request(
    state: Arc<AppState>,
    req: Request<Incoming>,
    method: &Method,
    path: &str,
) -> Response<RespBody> {
    let validated_host = match validate_host(req.headers(), state.port) {
        Ok(host) => host,
        Err(err) => {
            log_rejection(method, path, err.as_str());
            let (status, code) = match err {
                RequestCheckError::MissingHost | RequestCheckError::MultipleHost => {
                    (StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest)
                }
                _ => (StatusCode::FORBIDDEN, ApiErrorCode::Forbidden),
            };
            return json_error(status, code, err.as_str());
        }
    };

    if state.auth.is_enabled() {
        if let Err(reason) = verify_basic_auth(req.headers(), &state.auth) {
            let delay = state.backoff.record_failure();
            log_rejection(method, path, reason.as_str());
            tokio::time::sleep(delay).await;
            return unauthorized_response(reason.as_str());
        }
        state.backoff.reset();
    }

    if method == Method::OPTIONS {
        log_rejection(method, path, "OPTIONS not allowed");
        return method_not_allowed();
    }

    if is_state_changing(method)
        && let Err(err) = validate_state_changing(req.headers(), &validated_host)
    {
        log_rejection(method, path, err.as_str());
        let (status, code) = match err {
            RequestCheckError::MissingContentType | RequestCheckError::UnsupportedContentType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, ApiErrorCode::BadRequest)
            }
            _ => (StatusCode::FORBIDDEN, ApiErrorCode::Forbidden),
        };
        return json_error(status, code, err.as_str());
    }

    match (method, path) {
        (&Method::GET, "/api/snapshot") => handle_snapshot(&state).await,
        (&Method::GET, "/api/events") => handle_events(&state, &req),
        (&Method::POST, "/api/sessions") => handle_create_session(&state, req).await,
        (&Method::POST, p) if extract_session_action(p, "prompt").is_some() => {
            let raw_id = extract_session_action(p, "prompt").unwrap().to_string();
            handle_submit_prompt(&state, &raw_id, req).await
        }
        (&Method::GET, p) if !is_api_path(p) => handle_static(p).await,
        _ => dispatch_privileged_route(&state, req, method, path).await,
    }
}

async fn dispatch_privileged_route(
    state: &Arc<AppState>,
    req: Request<Incoming>,
    method: &Method,
    path: &str,
) -> Response<RespBody> {
    if !is_api_path(path) {
        return method_not_allowed();
    }
    if matches!(path, "/api/snapshot" | "/api/events" | "/api/sessions")
        || extract_session_action(path, "prompt").is_some()
    {
        return method_not_allowed();
    }

    if !state.auth.is_enabled() {
        return unauthorized_response("authentication required for privileged routes");
    }

    match (method, path) {
        (&Method::GET, "/api/sessions/history") => handle_sessions_history(state).await,
        (&Method::GET, "/api/workspace/files") => handle_workspace_files(state).await,
        (&Method::GET, "/api/workspace/search") => handle_workspace_search(state, &req).await,
        (&Method::GET, "/api/workspace/preview") => handle_workspace_preview(state, &req).await,
        (&Method::GET, "/api/workspace/changes") => handle_workspace_changes(state).await,
        (&Method::GET, "/api/workspace/diff") => handle_workspace_diff(state, &req).await,
        (&Method::POST, "/api/workspace/stage") => handle_workspace_stage(state, req).await,
        (&Method::GET, "/api/providers") => handle_get_providers(state).await,
        (&Method::POST, "/api/actions") => handle_host_action(state, req).await,
        (&Method::GET, "/api/memories") => handle_memories(state, &req).await,
        (&Method::GET, "/api/routes") => handle_routes(state).await,
        (&Method::GET, "/api/packs") => handle_packs(state).await,
        (&Method::GET, "/api/definitions/diagnostics") => handle_definitions(state).await,
        (&Method::GET, p) if extract_session_action(p, "context").is_some() => {
            let raw_id = extract_session_action(p, "context").unwrap();
            handle_session_context(state, raw_id)
        }
        (&Method::GET, p) if extract_blob_path(p).is_some() => {
            let (raw_id, blob_key) = extract_blob_path(p).unwrap();
            handle_session_blob(state, raw_id, blob_key).await
        }
        (&Method::POST, "/api/settings") => handle_update_settings(state, req).await,
        (&Method::POST, "/api/reload") => handle_reload(state).await,
        (&Method::GET, p) if extract_single_session_id(p).is_some() => {
            let raw_id = extract_single_session_id(p).unwrap();
            handle_session_detail(state, raw_id)
        }
        (&Method::DELETE, p) if extract_single_session_id(p).is_some() => {
            let raw_id = extract_single_session_id(p).unwrap().to_string();
            handle_delete_session(state, &raw_id).await
        }
        (&Method::GET, p) if extract_session_action(p, "events").is_some() => {
            let raw_id = extract_session_action(p, "events").unwrap().to_string();
            handle_session_events(state, &raw_id).await
        }
        (&Method::POST, p) if extract_session_action(p, "open").is_some() => {
            let raw_id = extract_session_action(p, "open").unwrap().to_string();
            handle_open_session(state, &raw_id).await
        }
        (&Method::POST, p) if extract_session_action(p, "activate").is_some() => {
            let raw_id = extract_session_action(p, "activate").unwrap();
            handle_activate_session(state, raw_id)
        }
        (&Method::POST, p) if extract_session_action(p, "close").is_some() => {
            let raw_id = extract_session_action(p, "close").unwrap().to_string();
            handle_close_session(state, &raw_id).await
        }
        (&Method::POST, p) if extract_session_action(p, "fork").is_some() => {
            let raw_id = extract_session_action(p, "fork").unwrap().to_string();
            handle_fork_session(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "btw").is_some() => {
            let raw_id = extract_session_action(p, "btw").unwrap().to_string();
            handle_btw_session(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "steer").is_some() => {
            let raw_id = extract_session_action(p, "steer").unwrap().to_string();
            handle_steer_prompt(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "cancel").is_some() => {
            let raw_id = extract_session_action(p, "cancel").unwrap();
            handle_simple_session_cmd(state, raw_id, |id| state.registry.cancel_turn(id))
        }
        (&Method::POST, p) if extract_session_action(p, "compact").is_some() => {
            let raw_id = extract_session_action(p, "compact").unwrap();
            handle_simple_session_cmd(state, raw_id, |id| state.registry.compact(id))
        }
        (&Method::POST, p) if extract_session_action(p, "reset").is_some() => {
            let raw_id = extract_session_action(p, "reset").unwrap();
            handle_simple_session_cmd(state, raw_id, |id| state.registry.reset(id))
        }
        (&Method::POST, p) if extract_session_action(p, "rewind").is_some() => {
            let raw_id = extract_session_action(p, "rewind").unwrap().to_string();
            handle_rewind_session(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "model").is_some() => {
            let raw_id = extract_session_action(p, "model").unwrap().to_string();
            handle_set_model(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "mode").is_some() => {
            let raw_id = extract_session_action(p, "mode").unwrap().to_string();
            handle_set_mode(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "apply-agent").is_some() => {
            let raw_id = extract_session_action(p, "apply-agent")
                .unwrap()
                .to_string();
            handle_apply_agent(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "apply-skill").is_some() => {
            let raw_id = extract_session_action(p, "apply-skill")
                .unwrap()
                .to_string();
            handle_apply_skill(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_action(p, "shell").is_some() => {
            let raw_id = extract_session_action(p, "shell").unwrap().to_string();
            handle_user_shell(state, &raw_id, req).await
        }
        (&Method::POST, p) if extract_session_subaction(p, "approvals").is_some() => {
            let (raw_id, req_id) = extract_session_subaction(p, "approvals").unwrap();
            let raw_id = raw_id.to_string();
            let req_id = req_id.to_string();
            handle_approval_decision(state, &raw_id, &req_id, req).await
        }
        (&Method::POST, p) if extract_session_subaction(p, "questions").is_some() => {
            let (raw_id, req_id) = extract_session_subaction(p, "questions").unwrap();
            let raw_id = raw_id.to_string();
            let req_id = req_id.to_string();
            handle_question_answer(state, &raw_id, &req_id, req).await
        }
        (&Method::POST, p) if extract_provider_action(p, "api-key").is_some() => {
            let pid = extract_provider_action(p, "api-key").unwrap().to_string();
            handle_set_api_key(state, &pid, req).await
        }
        (&Method::DELETE, p) if extract_provider_action(p, "credential").is_some() => {
            let pid = extract_provider_action(p, "credential")
                .unwrap()
                .to_string();
            handle_remove_credential(state, &pid).await
        }
        (&Method::POST, p) if extract_provider_action(p, "base-url").is_some() => {
            let pid = extract_provider_action(p, "base-url").unwrap().to_string();
            handle_set_base_url(state, &pid, req).await
        }
        (&Method::POST, p) if extract_provider_action(p, "chatgpt/start").is_some() => {
            let pid = extract_provider_action(p, "chatgpt/start")
                .unwrap()
                .to_string();
            handle_chatgpt_start(state, &pid).await
        }
        (&Method::POST, p) if extract_provider_chatgpt_action(p, "paste").is_some() => {
            let (pid, attempt) = extract_provider_chatgpt_action(p, "paste").unwrap();
            let pid = pid.to_string();
            let attempt = attempt.to_string();
            handle_chatgpt_paste(state, &pid, &attempt, req).await
        }
        (&Method::POST, p) if extract_provider_chatgpt_action(p, "cancel").is_some() => {
            let (pid, attempt) = extract_provider_chatgpt_action(p, "cancel").unwrap();
            let pid = pid.to_string();
            let attempt = attempt.to_string();
            handle_chatgpt_cancel(state, &pid, &attempt)
        }
        _ => json_error(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "endpoint not found",
        ),
    }
}

fn unauthorized_response(message: &str) -> Response<RespBody> {
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

fn method_not_allowed() -> Response<RespBody> {
    json_error(
        StatusCode::METHOD_NOT_ALLOWED,
        ApiErrorCode::BadRequest,
        "method not allowed",
    )
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn extract_single_session_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/sessions/")?;
    if rest.is_empty() || rest == "history" || rest.contains('/') {
        None
    } else {
        Some(rest)
    }
}

fn extract_session_action<'a>(path: &'a str, action: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let suffix = format!("/{action}");
    let id = rest.strip_suffix(&suffix)?;
    if id.is_empty() || id.contains('/') {
        None
    } else {
        Some(id)
    }
}

fn extract_session_subaction<'a>(path: &'a str, section: &str) -> Option<(&'a str, &'a str)> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (id, tail) = rest.split_once('/')?;
    let item_id = tail.strip_prefix(&format!("{section}/"))?;
    if id.is_empty() || item_id.is_empty() || item_id.contains('/') {
        None
    } else {
        Some((id, item_id))
    }
}

fn extract_provider_action<'a>(path: &'a str, suffix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/api/providers/")?;
    let id = rest.strip_suffix(&format!("/{suffix}"))?;
    if id.is_empty() || id.contains('/') {
        None
    } else {
        Some(id)
    }
}

fn extract_provider_chatgpt_action<'a>(path: &'a str, action: &str) -> Option<(&'a str, &'a str)> {
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
fn parse_session_id(raw: &str) -> Result<SessionId, Response<RespBody>> {
    raw.parse().map_err(|_| {
        json_error(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::BadRequest,
            "invalid session id",
        )
    })
}

#[allow(clippy::result_large_err)]
async fn parse_json<T: DeserializeOwned>(req: Request<Incoming>) -> Result<T, Response<RespBody>> {
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
async fn parse_optional_json<T: DeserializeOwned + Default>(
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

async fn handle_snapshot(state: &AppState) -> Response<RespBody> {
    let host_info = if state.auth.is_enabled() {
        Some(state.opener.host_info().await)
    } else {
        None
    };
    let snapshot = mapper::map_snapshot(state.registry.snapshot(), host_info);
    json_response(StatusCode::OK, &snapshot)
}

fn handle_events(state: &AppState, req: &Request<Incoming>) -> Response<RespBody> {
    let after_seq = match parse_after_seq(req) {
        Ok(seq) => seq,
        Err(msg) => return json_error(StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest, &msg),
    };

    let subscription = state.registry.subscribe(after_seq);
    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(SSE_CHANNEL_CAPACITY);
    let shutdown_rx = state.shutdown_rx.clone();

    tokio::spawn(async move {
        pump_sse(tx, subscription, shutdown_rx).await;
    });

    let stream = ReceiverStream::new(rx);
    let body = StreamBody::new(stream).boxed();
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap()
}

fn parse_after_seq(req: &Request<Incoming>) -> Result<Option<u64>, String> {
    if let Some(header) = req.headers().get("last-event-id") {
        let raw = header
            .to_str()
            .map_err(|_| "invalid Last-Event-ID header".to_string())?
            .trim();
        if !raw.is_empty() {
            let seq = raw
                .parse::<u64>()
                .map_err(|_| "Last-Event-ID must be an unsigned integer".to_string())?;
            return Ok(Some(seq));
        }
    }

    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=')
                && key == "after"
                && !value.is_empty()
            {
                let seq = value
                    .parse::<u64>()
                    .map_err(|_| "after query parameter must be an unsigned integer".to_string())?;
                return Ok(Some(seq));
            }
        }
    }

    Ok(None)
}

async fn pump_sse(
    tx: mpsc::Sender<Result<Frame<Bytes>, Infallible>>,
    subscription: SubscribeResult,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let (events, mut broadcast_rx) = match subscription {
        SubscribeResult::Replay { events, rx } => (events, rx),
        SubscribeResult::ResyncRequired { current_seq, .. } => {
            let frame = format_sse_event(Some(current_seq), &ViewEvent::ResyncRequired);
            let _ = tx.send(Ok(Frame::data(Bytes::from(frame)))).await;
            return;
        }
    };

    let mut last_sent_seq = 0u64;
    for event in events {
        if event.seq > last_sent_seq {
            last_sent_seq = event.seq;
            let view_event = mapper::map_view_event(event.kind);
            let frame = format_sse_event(Some(event.seq), &view_event);
            if tx.send(Ok(Frame::data(Bytes::from(frame)))).await.is_err() {
                return;
            }
        }
    }

    let mut keepalive = tokio::time::interval(SSE_KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keepalive.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = tx.closed() => break,
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    let frame = format_sse_event(None, &ViewEvent::HostDisconnected);
                    let _ = tx.send(Ok(Frame::data(Bytes::from(frame)))).await;
                    break;
                }
            }
            _ = keepalive.tick() => {
                if tx.send(Ok(Frame::data(Bytes::from_static(b": keepalive\n\n")))).await.is_err() {
                    break;
                }
            }
            recv = broadcast_rx.recv() => {
                match recv {
                    Ok(event) => {
                        if event.seq > last_sent_seq {
                            last_sent_seq = event.seq;
                            let view_event = mapper::map_view_event(event.kind);
                            let frame = format_sse_event(Some(event.seq), &view_event);
                            if tx.send(Ok(Frame::data(Bytes::from(frame)))).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let frame = format_sse_event(None, &ViewEvent::ResyncRequired);
                        let _ = tx.send(Ok(Frame::data(Bytes::from(frame)))).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

fn format_sse_event(seq: Option<u64>, event: &ViewEvent) -> String {
    let data = serde_json::to_string(event).expect("ViewEvent serializes to JSON");
    match seq {
        Some(id) => format!("id: {id}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    }
}

async fn handle_create_session(state: &AppState, req: Request<Incoming>) -> Response<RespBody> {
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

async fn handle_sessions_history(state: &AppState) -> Response<RespBody> {
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

fn handle_session_detail(state: &AppState, raw_id: &str) -> Response<RespBody> {
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

async fn handle_session_events(state: &AppState, raw_id: &str) -> Response<RespBody> {
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

async fn handle_open_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
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

fn handle_activate_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
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

async fn handle_close_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
    let session_id = match parse_session_id(raw_id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.registry.close_and_wait_session(&session_id).await {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "closed": true })),
        Err(err) => host_err_response(err),
    }
}

async fn handle_delete_session(state: &AppState, raw_id: &str) -> Response<RespBody> {
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

async fn handle_fork_session(
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

async fn handle_btw_session(
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

async fn handle_submit_prompt(
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

async fn handle_steer_prompt(
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

fn handle_simple_session_cmd(
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

async fn handle_rewind_session(
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

async fn handle_set_model(
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

async fn handle_set_mode(
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

async fn handle_apply_agent(
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

async fn handle_apply_skill(
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

async fn handle_user_shell(
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

async fn handle_approval_decision(
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

async fn handle_question_answer(
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

async fn handle_workspace_files(state: &AppState) -> Response<RespBody> {
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

async fn handle_workspace_search(state: &AppState, req: &Request<Incoming>) -> Response<RespBody> {
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

async fn handle_workspace_preview(state: &AppState, req: &Request<Incoming>) -> Response<RespBody> {
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
    let preview = tokio::task::spawn_blocking(move || {
        let item = SearchMatch {
            path,
            line,
            column: None,
            snippet: String::new(),
        };
        build_search_preview(&root, &item, height)
    })
    .await
    .unwrap();
    json_response(StatusCode::OK, &mapper::map_search_preview(preview))
}

async fn handle_workspace_changes(state: &AppState) -> Response<RespBody> {
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

async fn handle_workspace_stage(state: &AppState, req: Request<Incoming>) -> Response<RespBody> {
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

async fn handle_workspace_diff(state: &AppState, req: &Request<Incoming>) -> Response<RespBody> {
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
    let diff = tokio::task::spawn_blocking(move || build_file_diff(&root, &path, &status))
        .await
        .unwrap();
    json_response(StatusCode::OK, &mapper::map_file_diff(diff))
}

async fn handle_host_action(state: &AppState, req: Request<Incoming>) -> Response<RespBody> {
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

async fn handle_memories(state: &AppState, req: &Request<Incoming>) -> Response<RespBody> {
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

async fn handle_routes(state: &AppState) -> Response<RespBody> {
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

async fn handle_packs(state: &AppState) -> Response<RespBody> {
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

async fn handle_definitions(state: &AppState) -> Response<RespBody> {
    match state.opener.definitions_diagnostics().await {
        Ok(diagnostics) => json_response(StatusCode::OK, &mapper::map_definitions(diagnostics)),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to inspect definitions: {err:#}"),
        ),
    }
}

fn handle_session_context(state: &AppState, raw_id: &str) -> Response<RespBody> {
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

async fn handle_session_blob(state: &AppState, raw_id: &str, blob_key: &str) -> Response<RespBody> {
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

fn extract_blob_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (id, rest) = rest.split_once("/blobs/")?;
    (!id.is_empty() && !rest.is_empty()).then_some((id, rest))
}

async fn handle_get_providers(state: &AppState) -> Response<RespBody> {
    match state.opener.providers_snapshot().await {
        Ok(snapshot) => json_response(StatusCode::OK, &mapper::map_providers(snapshot)),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &format!("failed to list providers: {err}"),
        ),
    }
}

async fn handle_set_api_key(
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

async fn handle_remove_credential(state: &AppState, provider_id: &str) -> Response<RespBody> {
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

async fn handle_set_base_url(
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

async fn handle_chatgpt_start(state: &Arc<AppState>, provider_id: &str) -> Response<RespBody> {
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
        let mut guard = state.oauth_attempts.lock().unwrap();
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
            .unwrap()
            .remove(&attempt_bg)
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

async fn handle_chatgpt_paste(
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
        let guard = state.oauth_attempts.lock().unwrap();
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

fn handle_chatgpt_cancel(
    state: &AppState,
    provider_id: &str,
    attempt_id: &str,
) -> Response<RespBody> {
    let cancel_tx = {
        let mut guard = state.oauth_attempts.lock().unwrap();
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

async fn handle_update_settings(state: &AppState, req: Request<Incoming>) -> Response<RespBody> {
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

async fn handle_reload(state: &AppState) -> Response<RespBody> {
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

fn host_err_response(err: HostError) -> Response<RespBody> {
    let (status, api_err) = mapper::map_host_error(err);
    json_response(status, &api_err)
}

fn is_safe_relative_workspace_spec(spec: &str) -> bool {
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

fn parse_query_params(raw_query: Option<&str>) -> HashMap<String, String> {
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

fn percent_decode(input: &str) -> String {
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

async fn read_json_body(req: Request<Incoming>) -> Result<Bytes, &'static str> {
    let limited = Limited::new(req.into_body(), MAX_JSON_BODY_BYTES);
    match limited.collect().await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(_) => Err("failed to read request body or payload too large"),
    }
}

async fn handle_static(path: &str) -> Response<RespBody> {
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

fn is_safe_asset_path(rel: &str) -> bool {
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

fn resolve_runtime_web_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MEERAI_WEB_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    Path::new("target").join("web")
}

fn asset_content_type(rel: &str) -> String {
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

fn asset_response(rel: &str, bytes: Bytes) -> Response<RespBody> {
    let content_type = asset_content_type(rel);
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, content_type)
        .body(Full::new(bytes).boxed())
        .unwrap()
}

fn text_response(status: StatusCode, msg: &str) -> Response<RespBody> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(msg.to_owned())).boxed())
        .unwrap()
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<RespBody> {
    let bytes = serde_json::to_vec(value).expect("JSON response serializes");
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(bytes)).boxed())
        .unwrap()
}

fn json_error(status: StatusCode, code: ApiErrorCode, message: &str) -> Response<RespBody> {
    let err = ApiError {
        code,
        message: message.to_string(),
    };
    json_response(status, &err)
}

fn finalize_response(path: &str, mut resp: Response<RespBody>) -> Response<RespBody> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_asset_safety_and_content_types() {
        assert!(is_safe_asset_path("index.html"));
        assert!(is_safe_asset_path("snippets/app.js"));
        assert!(!is_safe_asset_path("../Cargo.toml"));
        assert!(!is_safe_asset_path("a/../b"));
        assert!(!is_safe_asset_path("a\\b"));
        assert!(!is_safe_asset_path(".next/trace"));
        assert!(!is_safe_asset_path(".stamp"));
        assert!(!is_safe_asset_path("reload.js"));
        assert!(!is_safe_asset_path("%2e%2e/secret"));

        assert_eq!(asset_content_type("app_bg.wasm"), "application/wasm");
        assert_eq!(
            asset_content_type("app.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(asset_content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(asset_content_type("font.ttf"), "font/ttf");
        assert_eq!(asset_content_type("favicon.svg"), "image/svg+xml");
        assert_eq!(asset_content_type("favicon.ico"), "image/x-icon");
        assert_eq!(asset_content_type("favicon-32x32.png"), "image/png");
    }

    #[test]
    fn query_decoding_and_route_extractors_work() {
        let params =
            parse_query_params(Some("mode=content&q=hello+world&path=src%2Flib.rs&line=9"));
        assert_eq!(params.get("mode").map(String::as_str), Some("content"));
        assert_eq!(params.get("q").map(String::as_str), Some("hello world"));
        assert_eq!(params.get("path").map(String::as_str), Some("src/lib.rs"));
        assert_eq!(
            extract_session_subaction("/api/sessions/ses-1/approvals/apr-9", "approvals"),
            Some(("ses-1", "apr-9"))
        );
        assert_eq!(
            extract_provider_chatgpt_action(
                "/api/providers/openai-codex/chatgpt/att-1/paste",
                "paste"
            ),
            Some(("openai-codex", "att-1"))
        );
        assert!(!is_safe_relative_workspace_spec("../secret.txt"));
        assert!(is_safe_relative_workspace_spec("a.rs -> b.rs"));
    }
}
