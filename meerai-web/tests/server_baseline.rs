use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use meerai_agents::{
    AgentCommand, AgentEvent, ItemDelta, ItemId, SessionItem, TurnEndReason, TurnId,
};
use meerai_host::registry::CAPACITY;
use meerai_host::{Backend, OpenedSession, SessionOpener, SessionRegistry};
use meerai_policy::ApprovalMode;
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use meerai_web::{ServeAuth, ServeConfig, ServeSecret, start_server};
use meerai_web_protocol::{
    ApiError, ApiErrorCode, CreateSessionResponse, SessionStatus, SnapshotResponse,
    SubmitPromptRequest, SubmitPromptResponse, TranscriptEntryDto, ViewEvent,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const TEST_SECRET: &str = "test-secret-at-least-16-chars";

struct FakeWorkerHandle {
    session_id: SessionId,
    cmd_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<AgentCommand>>>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

struct FakeOpener {
    next_id: Mutex<u64>,
    created_workers: Mutex<Vec<FakeWorkerHandle>>,
}

impl FakeOpener {
    fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            created_workers: Mutex::new(Vec::new()),
        }
    }

    fn take_last_worker(&self) -> Option<FakeWorkerHandle> {
        self.created_workers.lock().unwrap().pop()
    }
}

#[async_trait]
impl SessionOpener for FakeOpener {
    async fn list_sessions(&self) -> anyhow::Result<Vec<SessionSummary>> {
        Ok(Vec::new())
    }

    async fn open_session(&self, id: &SessionId) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("open_session not implemented for {id}")
    }

    async fn create_session(&self) -> anyhow::Result<OpenedSession> {
        let mut guard = self.next_id.lock().unwrap();
        let seq = *guard;
        *guard += 1;
        let session_id = SessionId::from_raw(format!("ses-created-{seq}"));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.created_workers.lock().unwrap().push(FakeWorkerHandle {
            session_id: session_id.clone(),
            cmd_rx: Arc::new(tokio::sync::Mutex::new(cmd_rx)),
            event_tx,
        });
        Ok(OpenedSession {
            session_id,
            title: None,
            backend: Backend::Native,
            provider: "gemini".to_string(),
            model: "gemini-3.8-flash".to_string(),
            approval_mode: ApprovalMode::Ask,
            events: Vec::new(),
            cmd_tx,
            event_rx,
        })
    }

    async fn fork_session(&self, id: &SessionId, _turn: &TurnId) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("fork_session not implemented for {id}")
    }

    async fn delete_session(&self, _id: &SessionId) -> anyhow::Result<bool> {
        Ok(false)
    }
}

struct TestWorker {
    session_id: SessionId,
    cmd_rx: mpsc::UnboundedReceiver<AgentCommand>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

fn register_fake_session(
    registry: &SessionRegistry,
    id: &str,
    events: Vec<LoggedEvent>,
) -> TestWorker {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let session_id = SessionId::from_raw(id);
    registry.register_opened(OpenedSession {
        session_id: session_id.clone(),
        title: None,
        backend: Backend::Native,
        provider: "gemini".to_string(),
        model: "gemini-3.8-flash".to_string(),
        approval_mode: ApprovalMode::Ask,
        events,
        cmd_tx,
        event_rx,
    });
    TestWorker {
        session_id,
        cmd_rx,
        event_tx,
    }
}

fn basic_auth_header(user: &str, secret: &str) -> String {
    let encoded = STANDARD.encode(format!("{user}:{secret}"));
    format!("Basic {encoded}")
}

struct RawResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn send_raw_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> RawResponse {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut raw = format!("{method} {path} HTTP/1.1\r\n");
    let mut has_connection = false;
    let mut has_content_length = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("connection") {
            has_connection = true;
        }
        if k.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    if !has_content_length && !body.is_empty() {
        raw.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if !has_connection {
        raw.push_str("Connection: close\r\n");
    }
    raw.push_str("\r\n");
    let mut payload = raw.into_bytes();
    payload.extend_from_slice(body);
    stream.write_all(&payload).await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset && !buf.is_empty() => {
                break;
            }
            Err(err) => panic!("failed reading HTTP response: {err}"),
        }
    }
    parse_raw_response(&buf)
}

fn parse_raw_response(raw: &[u8]) -> RawResponse {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("valid HTTP response head");
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let body_raw = &raw[split + 4..];
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        decode_chunked(body_raw)
    } else {
        body_raw.to_vec()
    };
    RawResponse {
        status,
        headers,
        body,
    }
}

fn decode_chunked(mut input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(pos) = input.windows(2).position(|w| w == b"\r\n") {
        let len_str = std::str::from_utf8(&input[..pos]).unwrap().trim();
        let len = usize::from_str_radix(len_str, 16).unwrap();
        input = &input[pos + 2..];
        if len == 0 {
            break;
        }
        out.extend_from_slice(&input[..len]);
        input = &input[len + 2..];
    }
    out
}

struct SseClient {
    reader: BufReader<TcpStream>,
    chunked: bool,
    remaining_chunk: usize,
    pending: String,
}

#[derive(Debug, Clone)]
struct SseMessage {
    id: Option<u64>,
    event: ViewEvent,
}

impl SseClient {
    async fn connect(addr: SocketAddr, auth: &str, last_event_id: Option<u64>) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let host = format!("127.0.0.1:{}", addr.port());
        let mut req = format!(
            "GET /api/events HTTP/1.1\r\nHost: {host}\r\nAuthorization: {auth}\r\nAccept: text/event-stream\r\n"
        );
        if let Some(id) = last_event_id {
            req.push_str(&format!("Last-Event-ID: {id}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).await.unwrap();
        assert!(
            status_line.starts_with("HTTP/1.1 200"),
            "unexpected SSE status: {status_line}"
        );

        let mut chunked = false;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':')
                && k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.trim().eq_ignore_ascii_case("chunked")
            {
                chunked = true;
            }
        }

        Self {
            reader,
            chunked,
            remaining_chunk: 0,
            pending: String::new(),
        }
    }

    async fn next_event(&mut self) -> Option<SseMessage> {
        loop {
            while let Some(pos) = self.pending.find("\n\n") {
                let block = self.pending[..pos].to_string();
                self.pending.drain(..pos + 2);
                if let Some(msg) = parse_sse_block(&block) {
                    return Some(msg);
                }
            }

            let bytes = if self.chunked {
                self.read_next_chunk().await?
            } else {
                let mut buf = vec![0u8; 4096];
                let n = self.reader.read(&mut buf).await.ok()?;
                if n == 0 {
                    return None;
                }
                buf.truncate(n);
                buf
            };
            self.pending
                .push_str(std::str::from_utf8(&bytes).expect("SSE UTF-8"));
        }
    }

    async fn read_next_chunk(&mut self) -> Option<Vec<u8>> {
        if self.remaining_chunk == 0 {
            let mut size_line = String::new();
            let n = self.reader.read_line(&mut size_line).await.ok()?;
            if n == 0 {
                return None;
            }
            let len = usize::from_str_radix(size_line.trim(), 16).ok()?;
            if len == 0 {
                return None;
            }
            self.remaining_chunk = len;
        }

        let mut buf = vec![0u8; self.remaining_chunk];
        self.reader.read_exact(&mut buf).await.ok()?;
        let mut crlf = [0u8; 2];
        self.reader.read_exact(&mut crlf).await.ok()?;
        self.remaining_chunk = 0;
        Some(buf)
    }
}

fn parse_sse_block(block: &str) -> Option<SseMessage> {
    let mut id: Option<u64> = None;
    let mut data_lines = Vec::new();
    for line in block.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(val) = line.strip_prefix("id:") {
            id = val.trim().parse::<u64>().ok();
        } else if let Some(val) = line.strip_prefix("data:") {
            data_lines.push(val.trim_start());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    let event: ViewEvent = serde_json::from_str(&data).expect("valid ViewEvent JSON");
    Some(SseMessage { id, event })
}

async fn spawn_test_server(
    registry: Arc<SessionRegistry>,
    opener: Arc<FakeOpener>,
) -> meerai_web::RunningServer {
    let config = ServeConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        auth: ServeAuth::from_parts(
            Some("meerai".to_string()),
            Some(ServeSecret::validate(TEST_SECRET).unwrap()),
        ),
    };
    start_server(config, opener, registry).await.unwrap()
}

#[test]
fn serve_secret_validation_and_debug_redaction() {
    assert!(ServeSecret::validate("short").is_err());
    assert!(ServeSecret::validate("7-chars").is_err());

    let raw = "8-chars!";
    let secret = ServeSecret::validate(raw).unwrap();
    let dbg = format!("{secret:?}");
    assert_eq!(dbg, "ServeSecret([REDACTED])");
    assert!(!dbg.contains(raw));

    let generated = ServeSecret::generate().unwrap();
    assert!(generated.expose().chars().count() >= 16);
    assert!(!format!("{generated:?}").contains(generated.expose()));
}

#[tokio::test]
async fn refuses_non_loopback_bind_addr() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let config = ServeConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        auth: ServeAuth::None,
    };
    let err = start_server(config, opener, registry).await.err().unwrap();
    assert!(err.to_string().contains("non-loopback"));
}

#[tokio::test]
async fn optional_auth_allows_loopback_without_credentials_while_enforcing_host_and_csrf() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let _worker = register_fake_session(&registry, "ses-no-auth", Vec::new());
    let server = start_server(
        ServeConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            auth: ServeAuth::None,
        },
        opener,
        registry,
    )
    .await
    .unwrap();

    let host = format!("127.0.0.1:{}", server.addr.port());
    let snap = send_raw_request(server.addr, "GET", "/api/snapshot", &[("Host", &host)], b"").await;
    assert_eq!(snap.status, 200);

    let csrf_blocked = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-no-auth/prompt",
        &[("Host", &host), ("Content-Type", "application/json")],
        br#"{"prompt":"hi"}"#,
    )
    .await;
    assert_eq!(csrf_blocked.status, 403);

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn ac4_rejects_missing_wrong_user_and_wrong_secret_with_401() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let mut worker = register_fake_session(&registry, "ses-ac4", Vec::new());
    assert_eq!(worker.session_id.as_str(), "ses-ac4");
    let server = spawn_test_server(Arc::clone(&registry), opener).await;

    let host = format!("127.0.0.1:{}", server.addr.port());
    let origin = format!("http://{host}");

    let missing =
        send_raw_request(server.addr, "GET", "/api/snapshot", &[("Host", &host)], b"").await;
    assert_eq!(missing.status, 401);
    assert_eq!(
        missing.headers.get("www-authenticate").map(String::as_str),
        Some("Basic realm=\"meerai\", charset=\"UTF-8\"")
    );

    let wrong_user = basic_auth_header("wrong", TEST_SECRET);
    let r_user = send_raw_request(
        server.addr,
        "GET",
        "/api/snapshot",
        &[("Host", &host), ("Authorization", &wrong_user)],
        b"",
    )
    .await;
    assert_eq!(r_user.status, 401);
    assert_eq!(
        r_user.headers.get("www-authenticate").map(String::as_str),
        Some("Basic realm=\"meerai\", charset=\"UTF-8\"")
    );

    let wrong_secret = basic_auth_header("meerai", "wrong-secret-value-123");
    let prompt_body = serde_json::to_vec(&SubmitPromptRequest {
        prompt: "should fail".to_string(),
    })
    .unwrap();
    let r_secret = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-ac4/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &wrong_secret),
        ],
        &prompt_body,
    )
    .await;
    assert_eq!(r_secret.status, 401);
    assert_eq!(
        r_secret.headers.get("www-authenticate").map(String::as_str),
        Some("Basic realm=\"meerai\", charset=\"UTF-8\"")
    );
    assert!(worker.cmd_rx.try_recv().is_err());

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn ac5_rejects_bad_origin_content_type_host_and_options_preflight() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let mut worker = register_fake_session(&registry, "ses-ac5", Vec::new());
    let server = spawn_test_server(Arc::clone(&registry), opener).await;

    let host = format!("127.0.0.1:{}", server.addr.port());
    let origin = format!("http://{host}");
    let auth = basic_auth_header("meerai", TEST_SECRET);
    let prompt_body = serde_json::to_vec(&SubmitPromptRequest {
        prompt: "blocked".to_string(),
    })
    .unwrap();

    let missing_origin = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-ac5/prompt",
        &[
            ("Host", &host),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        &prompt_body,
    )
    .await;
    assert!((400..500).contains(&missing_origin.status));

    let foreign_origin = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-ac5/prompt",
        &[
            ("Host", &host),
            ("Origin", "https://evil.example"),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        &prompt_body,
    )
    .await;
    assert!((400..500).contains(&foreign_origin.status));

    let non_json = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-ac5/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "text/plain"),
            ("Authorization", &auth),
        ],
        &prompt_body,
    )
    .await;
    assert!((400..500).contains(&non_json.status));

    let bad_host = send_raw_request(
        server.addr,
        "GET",
        "/api/snapshot",
        &[
            ("Host", &format!("evil.example:{}", server.addr.port())),
            ("Authorization", &auth),
        ],
        b"",
    )
    .await;
    assert!((400..500).contains(&bad_host.status));

    let options = send_raw_request(
        server.addr,
        "OPTIONS",
        "/api/sessions/ses-ac5/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Authorization", &auth),
        ],
        b"",
    )
    .await;
    assert_eq!(options.status, 405);
    assert!(
        !options
            .headers
            .keys()
            .any(|k| k.starts_with("access-control-"))
    );

    assert!(worker.cmd_rx.try_recv().is_err());
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn ac3_rejects_prompt_while_turn_running() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let mut worker = register_fake_session(&registry, "ses-ac3", Vec::new());
    let server = spawn_test_server(Arc::clone(&registry), opener).await;

    let host = format!("127.0.0.1:{}", server.addr.port());
    let origin = format!("http://{host}");
    let auth = basic_auth_header("meerai", TEST_SECRET);

    let first_body = serde_json::to_vec(&SubmitPromptRequest {
        prompt: "first prompt".to_string(),
    })
    .unwrap();
    let first = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-ac3/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        &first_body,
    )
    .await;
    assert_eq!(first.status, 200);
    let ok_resp: SubmitPromptResponse = serde_json::from_slice(&first.body).unwrap();
    assert!(ok_resp.accepted);
    assert_eq!(ok_resp.session_id, "ses-ac3");

    let second_body = serde_json::to_vec(&SubmitPromptRequest {
        prompt: "second prompt".to_string(),
    })
    .unwrap();
    let second = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-ac3/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        &second_body,
    )
    .await;
    assert_eq!(second.status, 409);
    let api_err: ApiError = serde_json::from_slice(&second.body).unwrap();
    assert_eq!(api_err.code, ApiErrorCode::ConflictBusy);

    assert_eq!(
        worker.cmd_rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("first prompt".to_string())
    );
    assert!(worker.cmd_rx.try_recv().is_err());

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn ac1_end_to_end_stream_replay_resync_and_snapshot() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let mut worker = register_fake_session(&registry, "ses-e2e", Vec::new());
    let server = spawn_test_server(Arc::clone(&registry), Arc::clone(&opener)).await;

    let host = format!("127.0.0.1:{}", server.addr.port());
    let origin = format!("http://{host}");
    let auth = basic_auth_header("meerai", TEST_SECRET);

    let snap0 = send_raw_request(
        server.addr,
        "GET",
        "/api/snapshot",
        &[("Host", &host), ("Authorization", &auth)],
        b"",
    )
    .await;
    assert_eq!(snap0.status, 200);
    assert_eq!(
        snap0
            .headers
            .get("x-content-type-options")
            .map(String::as_str),
        Some("nosniff")
    );
    assert_eq!(
        snap0.headers.get("referrer-policy").map(String::as_str),
        Some("no-referrer")
    );
    assert_eq!(
        snap0.headers.get("x-frame-options").map(String::as_str),
        Some("DENY")
    );
    assert!(snap0.headers.contains_key("content-security-policy"));
    assert_eq!(
        snap0.headers.get("cache-control").map(String::as_str),
        Some("no-store")
    );
    let initial_snap: SnapshotResponse = serde_json::from_slice(&snap0.body).unwrap();
    let start_seq = initial_snap.event_id;

    let mut sse = SseClient::connect(server.addr, &auth, None).await;

    let prompt_body = serde_json::to_vec(&SubmitPromptRequest {
        prompt: "explain meerai".to_string(),
    })
    .unwrap();
    let post = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-e2e/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        &prompt_body,
    )
    .await;
    assert_eq!(post.status, 200);
    assert_eq!(
        worker.cmd_rx.recv().await,
        Some(AgentCommand::SubmitPrompt("explain meerai".to_string()))
    );

    let turn_id = TurnId::from_raw("turn-1");
    let item_id = ItemId::from_raw("item-1");
    worker
        .event_tx
        .send(AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            model: "gemini::gemini-3.8-flash".to_string(),
        })
        .unwrap();
    worker
        .event_tx
        .send(AgentEvent::ItemDelta {
            item_id: item_id.clone(),
            delta: ItemDelta::AgentText("hello ".to_string()),
        })
        .unwrap();
    worker
        .event_tx
        .send(AgentEvent::ItemDelta {
            item_id: item_id.clone(),
            delta: ItemDelta::AgentText("world".to_string()),
        })
        .unwrap();
    worker
        .event_tx
        .send(AgentEvent::ItemCompleted {
            item: SessionItem::AgentMessage {
                id: item_id.clone(),
                text: "hello world".to_string(),
            },
        })
        .unwrap();
    worker
        .event_tx
        .send(AgentEvent::TurnCompleted {
            turn_id: turn_id.clone(),
            reason: TurnEndReason::Completed,
        })
        .unwrap();

    let mut seen = Vec::new();
    let mut turn_started_seq = None;
    let mut last_seq = start_seq;
    for _ in 0..16 {
        let msg = tokio::time::timeout(Duration::from_secs(2), sse.next_event())
            .await
            .expect("SSE event timeout")
            .expect("SSE stream open");
        let seq = msg.id.expect("SSE event has id");
        assert!(seq > last_seq);
        last_seq = seq;
        if matches!(msg.event, ViewEvent::TurnStarted { .. }) {
            turn_started_seq = Some(seq);
        }
        let done = matches!(msg.event, ViewEvent::TurnCompleted { .. });
        seen.push(msg);
        if done {
            break;
        }
    }
    assert!(seen.iter().any(|m| matches!(
        &m.event,
        ViewEvent::TextDelta { delta, .. } if delta == "hello "
    )));
    assert!(seen.iter().any(|m| matches!(
        &m.event,
        ViewEvent::EntryCompleted {
            entry: TranscriptEntryDto::Assistant { text, .. },
            ..
        } if text == "hello world"
    )));
    assert!(seen.iter().any(|m| matches!(
        &m.event,
        ViewEvent::TurnCompleted { reason, .. } if reason == "completed"
    )));
    drop(sse);

    let replay_after = turn_started_seq.unwrap();
    let mut sse_replay = SseClient::connect(server.addr, &auth, Some(replay_after)).await;
    let mut replayed = Vec::new();
    for _ in 0..10 {
        let msg = tokio::time::timeout(Duration::from_secs(2), sse_replay.next_event())
            .await
            .expect("replay SSE timeout")
            .expect("replay SSE event");
        assert!(msg.id.unwrap() > replay_after);
        let done = matches!(msg.event, ViewEvent::TurnCompleted { .. });
        replayed.push(msg);
        if done {
            break;
        }
    }
    assert!(replayed.iter().any(|m| matches!(
        &m.event,
        ViewEvent::TextDelta { delta, .. } if delta == "hello "
    )));
    assert!(
        replayed
            .iter()
            .any(|m| matches!(&m.event, ViewEvent::TurnCompleted { .. }))
    );
    drop(sse_replay);

    let snap_after = send_raw_request(
        server.addr,
        "GET",
        "/api/snapshot",
        &[("Host", &host), ("Authorization", &auth)],
        b"",
    )
    .await;
    assert_eq!(snap_after.status, 200);
    let snap: SnapshotResponse = serde_json::from_slice(&snap_after.body).unwrap();
    let detail = snap.active_session.unwrap();
    assert_eq!(detail.summary.status, SessionStatus::Idle);
    assert_eq!(
        detail.entries,
        vec![
            TranscriptEntryDto::User {
                id: "entry-1".to_string(),
                turn_id: Some("turn-1".to_string()),
                text: "explain meerai".to_string(),
            },
            TranscriptEntryDto::Assistant {
                id: "item-1".to_string(),
                text: "hello world".to_string(),
                reasoning: String::new(),
            },
        ]
    );

    let overflow_item = ItemId::from_raw("item-overflow");
    for _ in 0..(CAPACITY + 32) {
        worker
            .event_tx
            .send(AgentEvent::ItemDelta {
                item_id: overflow_item.clone(),
                delta: ItemDelta::AgentText(".".to_string()),
            })
            .unwrap();
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let res = send_raw_request(
            server.addr,
            "GET",
            "/api/snapshot",
            &[("Host", &host), ("Authorization", &auth)],
            b"",
        )
        .await;
        let current: SnapshotResponse = serde_json::from_slice(&res.body).unwrap();
        if current.event_id > CAPACITY as u64 + 10 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "ring did not overflow in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let mut sse_stale = SseClient::connect(server.addr, &auth, Some(1)).await;
    let first_stale = tokio::time::timeout(Duration::from_secs(2), sse_stale.next_event())
        .await
        .expect("resync event timeout")
        .expect("resync event");
    assert_eq!(first_stale.event, ViewEvent::ResyncRequired);
    drop(sse_stale);

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn creates_sessions_and_handles_missing_and_static_routes() {
    let registry = SessionRegistry::new();
    let opener = Arc::new(FakeOpener::new());
    let server = spawn_test_server(Arc::clone(&registry), Arc::clone(&opener)).await;

    let host = format!("127.0.0.1:{}", server.addr.port());
    let origin = format!("http://{host}");
    let auth = basic_auth_header("meerai", TEST_SECRET);

    let created = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        b"{}",
    )
    .await;
    assert_eq!(created.status, 201);
    let created_dto: CreateSessionResponse = serde_json::from_slice(&created.body).unwrap();
    assert_eq!(created_dto.session.id, "ses-created-1");
    let created_worker = opener.take_last_worker().unwrap();
    assert_eq!(created_worker.session_id.as_str(), "ses-created-1");

    let hello_created = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-created-1/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        br#"{"prompt":"hello created"}"#,
    )
    .await;
    assert_eq!(hello_created.status, 200);
    assert_eq!(
        created_worker.cmd_rx.lock().await.recv().await,
        Some(AgentCommand::SubmitPrompt("hello created".to_string()))
    );
    created_worker
        .event_tx
        .send(AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("t-created"),
            reason: TurnEndReason::Completed,
        })
        .unwrap();

    let missing_session = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-does-not-exist/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        br#"{"prompt":"hi"}"#,
    )
    .await;
    assert_eq!(missing_session.status, 404);

    let empty_prompt = send_raw_request(
        server.addr,
        "POST",
        "/api/sessions/ses-created-1/prompt",
        &[
            ("Host", &host),
            ("Origin", &origin),
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
        ],
        br#"{"prompt":"   "}"#,
    )
    .await;
    assert_eq!(empty_prompt.status, 400);

    let root = send_raw_request(
        server.addr,
        "GET",
        "/",
        &[("Host", &host), ("Authorization", &auth)],
        b"",
    )
    .await;
    let has_bundle = std::env::var_os("MEERAI_WEB_DIR")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("web")
        })
        .join("app_bg.wasm")
        .is_file();
    if has_bundle {
        assert_eq!(root.status, 200);
        assert!(
            root.headers
                .get("content-type")
                .is_some_and(|v| v.contains("text/html"))
        );
        let wasm = send_raw_request(
            server.addr,
            "GET",
            "/app_bg.wasm",
            &[("Host", &host), ("Authorization", &auth)],
            b"",
        )
        .await;
        assert_eq!(wasm.status, 200);
        assert_eq!(
            wasm.headers.get("content-type").map(String::as_str),
            Some("application/wasm")
        );
    } else {
        assert_eq!(root.status, 503);
    }

    let traversal = send_raw_request(
        server.addr,
        "GET",
        "/../Cargo.toml",
        &[("Host", &host), ("Authorization", &auth)],
        b"",
    )
    .await;
    assert_eq!(traversal.status, 400);

    server.shutdown().await.unwrap();
}
