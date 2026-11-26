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

mod memory;
mod models;
mod prompts;
mod providers;
mod respond;
mod sessions;
mod settings;
mod statics;
mod workspace;

use memory::*;
use models::*;
use prompts::*;
use providers::*;
use respond::*;
use sessions::*;
use settings::*;
use statics::*;
use workspace::*;

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
