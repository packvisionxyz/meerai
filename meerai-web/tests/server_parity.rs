use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use meerai_agents::{
    AgentCommand, AgentEvent, ApprovalRequest, QuestionOption, QuestionPrompt, QuestionRequest,
    TurnEndReason, TurnId,
};
use meerai_common::hooks::HookEntry;
use meerai_common::settings::{ContextSettings, SandboxSettings, SessionSettings, ThemeSettings};
use meerai_host::{
    AcpPresetInfo, Backend, ChatGptLogin, ChatGptLoginOutcome, HostAgentInfo, HostAuthKind,
    HostInfoSnapshot, HostModelItem, HostPalette, HostProviderItem, HostProvidersSnapshot,
    HostReloadOutcome, HostReloadPayload, HostSettingsUpdate, HostSettingsView, HostSkillInfo,
    OpenedSession, ResolvedAgentApplication, ResolvedModelSelection, ResolvedSkillApplication,
    SessionOpener, SessionRegistry,
};
use meerai_policy::ApprovalMode;
use meerai_session::{
    ApprovalKind, ApprovalPreview, ContentBlock, GrantScope, InputSource, LoggedEvent, RequestId,
    Seq, SessionEvent, SessionId, SessionSummary,
};
use meerai_web::{RunningServer, ServeAuth, ServeConfig, ServeSecret, start_server};
use meerai_web_protocol::{
    ApiError, ApiErrorCode, ChatGptLoginStartResponse, CreateSessionResponse, FileDiffResponse,
    HostInfoDto, ProvidersResponse, ReloadResponse, RewindResponse, RunShellResponse,
    SessionEventsResponse, SessionHistoryResponse, SetModelResponse, WorkspaceFilesResponse,
    WorkspacePreviewResponse, WorkspaceSearchResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

const TEST_SECRET: &str = "test-parity-secret-16";

struct TempWs {
    root: PathBuf,
}

impl TempWs {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "meerai_web_parity_{name}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("main.rs"),
            "fn main() {\n    let target = 42;\n}\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "# Meerai\n").unwrap();
        Self { root }
    }
}

impl Drop for TempWs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ParityWorker {
    _session_id: SessionId,
    cmd_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<AgentCommand>>>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

struct ParityOpener {
    ws_root: PathBuf,
    workers: Mutex<HashMap<String, ParityWorker>>,
    stored: Mutex<Vec<SessionSummary>>,
    stored_events: Mutex<HashMap<String, Vec<LoggedEvent>>>,
    saved_api_key: Mutex<Option<(String, String)>>,
    saved_base_url: Mutex<Option<(String, Option<String>)>>,
    removed_cred: Mutex<Option<String>>,
    shell_commands: Mutex<Vec<String>>,
    deleted: Mutex<Vec<SessionId>>,
    next_seq: Mutex<u64>,
    oauth_paste_seen: Mutex<Option<String>>,
}

impl ParityOpener {
    fn new(ws_root: PathBuf) -> Self {
        Self {
            ws_root,
            workers: Mutex::new(HashMap::new()),
            stored: Mutex::new(vec![SessionSummary {
                id: SessionId::from_raw("ses-stored-1"),
                dir: PathBuf::from("/tmp/ses-stored-1"),
                title: Some("Stored Title".to_string()),
                model: Some("gemini-3.8-flash".to_string()),
                created_at_ms: 100,
                updated_at_ms: 200,
                bytes: 512,
                parent_session_id: None,
            }]),
            stored_events: Mutex::new(HashMap::from([(
                "ses-stored-1".to_string(),
                vec![
                    LoggedEvent {
                        seq: Seq(1),
                        event: SessionEvent::TurnStarted {
                            turn_id: TurnId::from_raw("t-stored"),
                            trigger: meerai_session::TurnTrigger::UserPrompt,
                        },
                    },
                    LoggedEvent {
                        seq: Seq(2),
                        event: SessionEvent::UserMessage {
                            source: InputSource::User,
                            content: vec![ContentBlock::Text("stored hello".to_string())],
                        },
                    },
                    LoggedEvent {
                        seq: Seq(3),
                        event: SessionEvent::TurnEnded {
                            turn_id: TurnId::from_raw("t-stored"),
                            reason: meerai_session::TurnEndReason::Completed,
                        },
                    },
                ],
            )])),
            saved_api_key: Mutex::new(None),
            saved_base_url: Mutex::new(None),
            removed_cred: Mutex::new(None),
            shell_commands: Mutex::new(Vec::new()),
            deleted: Mutex::new(Vec::new()),
            next_seq: Mutex::new(1),
            oauth_paste_seen: Mutex::new(None),
        }
    }

    fn make_opened(
        &self,
        session_id: SessionId,
        backend: Backend,
        events: Vec<LoggedEvent>,
    ) -> OpenedSession {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.workers.lock().unwrap().insert(
            session_id.to_string(),
            ParityWorker {
                _session_id: session_id.clone(),
                cmd_rx: Arc::new(tokio::sync::Mutex::new(cmd_rx)),
                event_tx,
            },
        );
        OpenedSession {
            session_id,
            title: Some("Parity".to_string()),
            backend,
            provider: "gemini".to_string(),
            model: "gemini-3.8-flash".to_string(),
            approval_mode: ApprovalMode::Ask,
            events,
            cmd_tx,
            event_rx,
        }
    }

    fn worker(
        &self,
        id: &str,
    ) -> (
        Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<AgentCommand>>>,
        mpsc::UnboundedSender<AgentEvent>,
    ) {
        let guard = self.workers.lock().unwrap();
        let w = guard.get(id).expect("worker registered");
        (Arc::clone(&w.cmd_rx), w.event_tx.clone())
    }
}

#[async_trait]
impl SessionOpener for ParityOpener {
    async fn list_sessions(&self) -> anyhow::Result<Vec<SessionSummary>> {
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn open_session(&self, id: &SessionId) -> anyhow::Result<OpenedSession> {
        let events = self
            .stored_events
            .lock()
            .unwrap()
            .get(id.as_str())
            .cloned()
            .unwrap_or_default();
        Ok(self.make_opened(id.clone(), Backend::Native, events))
    }

    async fn create_session(&self) -> anyhow::Result<OpenedSession> {
        let mut seq = self.next_seq.lock().unwrap();
        let n = *seq;
        *seq += 1;
        Ok(self.make_opened(
            SessionId::from_raw(format!("ses-new-{n}")),
            Backend::Native,
            Vec::new(),
        ))
    }

    async fn create_acp_session(&self, preset: &str) -> anyhow::Result<OpenedSession> {
        let mut seq = self.next_seq.lock().unwrap();
        let n = *seq;
        *seq += 1;
        Ok(self.make_opened(
            SessionId::from_raw(format!("ses-acp-{n}")),
            Backend::Acp {
                preset: preset.to_string(),
                remote_session_id: Some("remote-1".to_string()),
            },
            Vec::new(),
        ))
    }

    async fn fork_session(&self, _id: &SessionId, _turn: &TurnId) -> anyhow::Result<OpenedSession> {
        let mut seq = self.next_seq.lock().unwrap();
        let n = *seq;
        *seq += 1;
        Ok(self.make_opened(
            SessionId::from_raw(format!("ses-fork-{n}")),
            Backend::Native,
            Vec::new(),
        ))
    }

    async fn delete_session(&self, id: &SessionId) -> anyhow::Result<bool> {
        self.deleted.lock().unwrap().push(id.clone());
        Ok(true)
    }

    async fn read_session_events(&self, id: &SessionId) -> anyhow::Result<Vec<LoggedEvent>> {
        self.stored_events
            .lock()
            .unwrap()
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("not found"))
    }

    async fn run_user_shell(&self, command: &str) -> anyhow::Result<String> {
        self.shell_commands
            .lock()
            .unwrap()
            .push(command.to_string());
        Ok(format!("ran: {command}"))
    }

    fn workspace_root(&self) -> Option<PathBuf> {
        Some(self.ws_root.clone())
    }

    async fn host_info(&self) -> HostInfoSnapshot {
        HostInfoSnapshot {
            workspace_root: self.ws_root.display().to_string(),
            git_branch: Some("main".to_string()),
            acp_presets: vec![AcpPresetInfo {
                id: "agy".to_string(),
                label: "Antigravity".to_string(),
                command: "agy".to_string(),
            }],
            agents: vec![HostAgentInfo {
                id: "reviewer".to_string(),
                source: ".agents".to_string(),
                description: Some("Code reviewer".to_string()),
                model: None,
            }],
            skills: vec![HostSkillInfo {
                id: "lint".to_string(),
                source: ".agents".to_string(),
                description: Some("Lint helper".to_string()),
            }],
            mcp_servers: Vec::new(),
            enabled_tools: vec!["code-read_file".to_string()],
            hooks_count: 1,
            update_available: None,
            install_hint: "curl ...".to_string(),
            settings: HostSettingsView {
                theme: HostPalette::default().to_hex_tokens(),
                raw_theme: ThemeSettings::default(),
                context: ContextSettings::default(),
                effort_levels: Vec::new(),
                session: SessionSettings::default(),
                sandbox: SandboxSettings::default(),
                permissions_summary: vec!["allow: code-read_file".to_string()],
                config_writable: true,
            },
        }
    }

    async fn providers_snapshot(&self) -> anyhow::Result<HostProvidersSnapshot> {
        Ok(HostProvidersSnapshot {
            default_provider: Some("gemini".to_string()),
            providers: vec![HostProviderItem {
                id: "gemini".to_string(),
                display_name: "Gemini".to_string(),
                adapter: "google".to_string(),
                auth_kind: HostAuthKind::ApiKey,
                configured: true,
                status_badge: "saved".to_string(),
                env_vars: vec!["GEMINI_API_KEY".to_string()],
                default_model: Some("gemini-3.8-flash".to_string()),
                base_url: None,
                is_custom: false,
            }],
            models: vec![HostModelItem {
                id: "gemini-3.8-flash".to_string(),
                provider: "gemini".to_string(),
                display_name: "Gemini 3.8 Flash".to_string(),
                context_window_tokens: Some(1_000_000),
                max_output_tokens: Some(8192),
                supports_reasoning: true,
                supports_vision: true,
                supports_tools: true,
                is_custom: false,
            }],
            recent_models: HashMap::new(),
        })
    }

    async fn set_api_key(&self, provider_id: &str, api_key: &str) -> anyhow::Result<()> {
        *self.saved_api_key.lock().unwrap() = Some((provider_id.to_string(), api_key.to_string()));
        Ok(())
    }

    async fn remove_credential(&self, provider_id: &str) -> anyhow::Result<()> {
        *self.removed_cred.lock().unwrap() = Some(provider_id.to_string());
        Ok(())
    }

    async fn set_provider_base_url(
        &self,
        provider_id: &str,
        base_url: Option<String>,
    ) -> anyhow::Result<()> {
        *self.saved_base_url.lock().unwrap() = Some((provider_id.to_string(), base_url));
        Ok(())
    }

    async fn select_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> anyhow::Result<ResolvedModelSelection> {
        Ok(ResolvedModelSelection {
            provider: provider_id.to_string(),
            display_model: model_id.to_string(),
            command_model: format!("google::{model_id}"),
            context_window_tokens: Some(1_000_000),
            effort_levels: None,
        })
    }

    async fn resolve_agent_application(
        &self,
        name: &str,
        _current_provider: &str,
    ) -> anyhow::Result<ResolvedAgentApplication> {
        if name != "reviewer" {
            anyhow::bail!("unknown agent: {name}");
        }
        Ok(ResolvedAgentApplication {
            name: name.to_string(),
            instructions: "Review carefully".to_string(),
            model: None,
            context_window_tokens: None,
            effort_levels: None,
        })
    }

    async fn resolve_skill_application(
        &self,
        name: &str,
    ) -> anyhow::Result<ResolvedSkillApplication> {
        if name != "lint" {
            anyhow::bail!("unknown skill: {name}");
        }
        Ok(ResolvedSkillApplication {
            name: name.to_string(),
            instructions: "Run cargo clippy".to_string(),
        })
    }

    async fn update_settings(
        &self,
        _update: HostSettingsUpdate,
    ) -> anyhow::Result<HostReloadOutcome> {
        Ok(HostReloadOutcome {
            host_info: self.host_info().await,
            warnings: vec!["updated".to_string()],
            payload: HostReloadPayload {
                rules: None,
                context: None,
                system_prompt: "sys".to_string(),
                hooks: Vec::<HookEntry>::new(),
            },
        })
    }

    async fn reload_config(&self) -> anyhow::Result<HostReloadOutcome> {
        Ok(HostReloadOutcome {
            host_info: self.host_info().await,
            warnings: Vec::new(),
            payload: HostReloadPayload {
                rules: None,
                context: None,
                system_prompt: "reloaded".to_string(),
                hooks: Vec::<HookEntry>::new(),
            },
        })
    }

    async fn start_chatgpt_login(&self, provider_id: &str) -> anyhow::Result<ChatGptLogin> {
        let (paste_tx, mut paste_rx) = mpsc::unbounded_channel::<String>();
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let (result_tx, result_rx) = oneshot::channel();
        let pid = provider_id.to_string();
        let seen_ref = Arc::new(Mutex::new(String::new()));
        let seen_clone = Arc::clone(&seen_ref);
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel_rx => {}
                msg = paste_rx.recv() => {
                    if let Some(code) = msg {
                        *seen_clone.lock().unwrap() = code;
                        let _ = result_tx.send(Ok(ChatGptLoginOutcome {
                            provider_id: pid,
                            account: Some("user@example.com".to_string()),
                        }));
                    }
                }
            }
        });
        let _ = self.oauth_paste_seen.lock().unwrap().take();
        Ok(ChatGptLogin {
            url: "https://chatgpt.example/login".to_string(),
            paste_tx,
            cancel_tx,
            result_rx,
        })
    }
}

struct RawResponse {
    status: u16,
    body: Vec<u8>,
}

fn basic_auth(user: &str, secret: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{secret}")))
}

async fn req(
    addr: SocketAddr,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: Option<&[u8]>,
) -> RawResponse {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let host = format!("127.0.0.1:{}", addr.port());
    let origin = format!("http://{host}");
    let payload = body.unwrap_or(b"");
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(a) = auth {
        raw.push_str(&format!("Authorization: {a}\r\n"));
    }
    if matches!(method, "POST" | "PUT" | "DELETE") {
        raw.push_str(&format!(
            "Origin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        ));
    }
    raw.push_str("\r\n");
    let mut full = raw.into_bytes();
    full.extend_from_slice(payload);
    stream.write_all(&full).await.unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    while let Ok(n) = stream.read(&mut tmp).await {
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&buf[..split]).unwrap();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    RawResponse {
        status,
        body: buf[split + 4..].to_vec(),
    }
}

async fn spawn_server(
    auth: ServeAuth,
    opener: Arc<ParityOpener>,
    registry: Arc<SessionRegistry>,
) -> RunningServer {
    start_server(
        ServeConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            auth,
        },
        opener,
        registry,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn unauthenticated_serve_auth_none_blocks_privileged_parity_routes() {
    let ws = TempWs::new("noauth");
    let opener = Arc::new(ParityOpener::new(ws.root.clone()));
    let registry = SessionRegistry::new();
    let server = spawn_server(ServeAuth::None, opener, registry).await;

    assert_eq!(
        req(server.addr, "GET", "/api/snapshot", None, None)
            .await
            .status,
        200
    );
    for (method, path, body) in [
        ("GET", "/api/providers", None),
        ("GET", "/api/sessions/history", None),
        ("GET", "/api/workspace/files", None),
        ("POST", "/api/settings", Some(b"{}".as_slice())),
        ("POST", "/api/reload", Some(b"{}".as_slice())),
        (
            "POST",
            "/api/sessions",
            Some(br#"{"backend":{"type":"acp","data":{"preset":"agy"}}}"#.as_slice()),
        ),
        (
            "POST",
            "/api/sessions/ses-1/shell",
            Some(br#"{"command":"ls"}"#.as_slice()),
        ),
        ("DELETE", "/api/sessions/ses-1", Some(b"{}".as_slice())),
    ] {
        let res = req(server.addr, method, path, None, body).await;
        assert_eq!(res.status, 401, "expected 401 on {method} {path}");
    }

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn parity_routes_cover_session_turns_approvals_workspace_providers_oauth_and_settings() {
    let ws = TempWs::new("full");
    let opener = Arc::new(ParityOpener::new(ws.root.clone()));
    let registry = SessionRegistry::new();
    let auth = ServeAuth::from_parts(None, Some(ServeSecret::validate(TEST_SECRET).unwrap()));
    let server = spawn_server(auth, Arc::clone(&opener), Arc::clone(&registry)).await;
    let hdr = Some(basic_auth("meerai", TEST_SECRET));
    let auth_ref = hdr.as_deref();

    // History, open stored, read events
    let hist = req(server.addr, "GET", "/api/sessions/history", auth_ref, None).await;
    assert_eq!(hist.status, 200);
    let hist_dto: SessionHistoryResponse = serde_json::from_slice(&hist.body).unwrap();
    assert_eq!(hist_dto.sessions.len(), 1);
    assert!(!hist_dto.sessions[0].is_open);

    let opened = req(
        server.addr,
        "POST",
        "/api/sessions/ses-stored-1/open",
        auth_ref,
        Some(b"{}"),
    )
    .await;
    assert_eq!(opened.status, 200);

    let events = req(
        server.addr,
        "GET",
        "/api/sessions/ses-stored-1/events",
        auth_ref,
        None,
    )
    .await;
    assert_eq!(events.status, 200);
    let ev_dto: SessionEventsResponse = serde_json::from_slice(&events.body).unwrap();
    assert!(!ev_dto.entries.is_empty());

    // Fork stored
    let forked = req(
        server.addr,
        "POST",
        "/api/sessions/ses-stored-1/fork",
        auth_ref,
        Some(b"{}"),
    )
    .await;
    assert_eq!(forked.status, 201);
    let fork_dto: CreateSessionResponse = serde_json::from_slice(&forked.body).unwrap();
    assert_eq!(fork_dto.session.id, "ses-fork-1");

    // Set model, set mode, apply agent/skill, run shell + plan block
    let model_res = req(
        server.addr,
        "POST",
        "/api/sessions/ses-fork-1/model",
        auth_ref,
        Some(br#"{"provider":"gemini","model":"gemini-3.8-flash"}"#),
    )
    .await;
    assert_eq!(model_res.status, 200);
    let model_dto: SetModelResponse = serde_json::from_slice(&model_res.body).unwrap();
    assert_eq!(model_dto.session.model, "gemini-3.8-flash");

    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/apply-agent",
            auth_ref,
            Some(br#"{"name":"reviewer"}"#),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/apply-skill",
            auth_ref,
            Some(br#"{"name":"lint"}"#),
        )
        .await
        .status,
        200
    );

    let shell_ok = req(
        server.addr,
        "POST",
        "/api/sessions/ses-fork-1/shell",
        auth_ref,
        Some(br#"{"command":"echo ok"}"#),
    )
    .await;
    assert_eq!(shell_ok.status, 200);
    let shell_dto: RunShellResponse = serde_json::from_slice(&shell_ok.body).unwrap();
    assert_eq!(shell_dto.output, "ran: echo ok");

    // Switch to Plan -> shell blocked
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/mode",
            auth_ref,
            Some(br#"{"mode":"plan"}"#),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/shell",
            auth_ref,
            Some(br#"{"command":"echo blocked"}"#),
        )
        .await
        .status,
        400
    );

    // ACP session creation & unsupported ACP mode/compact refusal
    let acp = req(
        server.addr,
        "POST",
        "/api/sessions",
        auth_ref,
        Some(br#"{"backend":{"type":"acp","data":{"preset":"agy"}}}"#),
    )
    .await;
    assert_eq!(acp.status, 201);
    let acp_dto: CreateSessionResponse = serde_json::from_slice(&acp.body).unwrap();
    let mode_refused = req(
        server.addr,
        "POST",
        &format!("/api/sessions/{}/mode", acp_dto.session.id),
        auth_ref,
        Some(br#"{"mode":"auto"}"#),
    )
    .await;
    assert_eq!(mode_refused.status, 400);
    let mode_err: ApiError = serde_json::from_slice(&mode_refused.body).unwrap();
    assert_eq!(mode_err.code, ApiErrorCode::UnsupportedBackend);

    // Live approval + stale 409 + question answer
    let (cmd_rx, event_tx) = opener.worker("ses-fork-1");
    let turn = TurnId::from_raw("t-live");
    event_tx
        .send(AgentEvent::TurnStarted {
            turn_id: turn.clone(),
            model: "gemini-3.8-flash".to_string(),
        })
        .unwrap();
    event_tx
        .send(AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id: RequestId::from_raw("apr-1"),
                turn_id: turn.clone(),
                kind: ApprovalKind::ToolCall,
                preview: ApprovalPreview {
                    title: "Write".to_string(),
                    detail: "main.rs".to_string(),
                },
                effect: None,
            },
        })
        .unwrap();
    event_tx
        .send(AgentEvent::QuestionAsked {
            request: QuestionRequest {
                request_id: RequestId::from_raw("q-1"),
                turn_id: turn.clone(),
                questions: vec![QuestionPrompt {
                    header: "Env".to_string(),
                    question: "Target?".to_string(),
                    options: vec![QuestionOption {
                        label: "dev".to_string(),
                        description: None,
                    }],
                    multiple: false,
                }],
            },
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/approvals/apr-wrong",
            auth_ref,
            Some(br#"{"decision":{"type":"accept"},"scope":"turn"}"#),
        )
        .await
        .status,
        409
    );
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/approvals/apr-1",
            auth_ref,
            Some(br#"{"decision":{"type":"accept"},"scope":"turn"}"#),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/questions/q-1",
            auth_ref,
            Some(br#"{"answers":[{"header":"Env","selected":["dev"]}]}"#),
        )
        .await
        .status,
        200
    );

    // A cancelled question reaches the worker as AnswerQuestion { answers: None }
    event_tx
        .send(AgentEvent::QuestionAsked {
            request: QuestionRequest {
                request_id: RequestId::from_raw("q-2"),
                turn_id: turn.clone(),
                questions: vec![QuestionPrompt {
                    header: "Env".to_string(),
                    question: "Target?".to_string(),
                    options: vec![QuestionOption {
                        label: "dev".to_string(),
                        description: None,
                    }],
                    multiple: false,
                }],
            },
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/questions/q-2",
            auth_ref,
            Some(br#"{"answers":[],"cancel":true}"#),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/questions/q-2",
            auth_ref,
            Some(br#"{"answers":[],"cancel":true}"#),
        )
        .await
        .status,
        409
    );

    // Steer & Cancel while running
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/steer",
            auth_ref,
            Some(br#"{"prompt":"steer now"}"#),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/cancel",
            auth_ref,
            Some(b"{}")
        )
        .await
        .status,
        200
    );
    event_tx
        .send(AgentEvent::TurnCompleted {
            turn_id: turn,
            reason: TurnEndReason::Completed,
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Drain commands from cmd_rx to verify arrival including GrantScope::Turn
    let mut cmds = Vec::new();
    while let Ok(cmd) = cmd_rx.lock().await.try_recv() {
        cmds.push(cmd);
    }
    assert!(cmds.iter().any(|c| matches!(
        c,
        AgentCommand::Approve {
            scope: Some(GrantScope::Turn),
            ..
        }
    )));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, AgentCommand::Steer(s) if s == "steer now"))
    );
    assert!(cmds.iter().any(|c| matches!(c, AgentCommand::Cancel)));
    assert!(cmds.iter().any(|c| matches!(
        c,
        AgentCommand::AnswerQuestion {
            request_id,
            answers: None,
        } if request_id.to_string() == "q-2"
    )));

    // Compact, Rewind, Reset, Close, Delete
    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/compact",
            auth_ref,
            Some(b"{}")
        )
        .await
        .status,
        200
    );
    let rw = req(
        server.addr,
        "POST",
        "/api/sessions/ses-stored-1/rewind",
        auth_ref,
        Some(br#"{"turnId":"t-stored","scope":"both"}"#),
    )
    .await;
    assert_eq!(rw.status, 200);
    let rw_dto: RewindResponse = serde_json::from_slice(&rw.body).unwrap();
    assert_eq!(rw_dto.restored_prompt.as_deref(), Some("stored hello"));

    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/sessions/ses-fork-1/reset",
            auth_ref,
            Some(b"{}")
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "DELETE",
            "/api/sessions/ses-fork-1",
            auth_ref,
            Some(b"{}")
        )
        .await
        .status,
        200
    );
    assert!(
        opener
            .deleted
            .lock()
            .unwrap()
            .contains(&SessionId::from_raw("ses-fork-1"))
    );

    // Workspace files, search, preview, traversal block, diff
    let wf: WorkspaceFilesResponse = serde_json::from_slice(
        &req(server.addr, "GET", "/api/workspace/files", auth_ref, None)
            .await
            .body,
    )
    .unwrap();
    assert!(wf.files.contains(&"main.rs".to_string()));

    let ws_res: WorkspaceSearchResponse = serde_json::from_slice(
        &req(
            server.addr,
            "GET",
            "/api/workspace/search?mode=content&q=target",
            auth_ref,
            None,
        )
        .await
        .body,
    )
    .unwrap();
    assert_eq!(ws_res.matches[0].path, "main.rs");

    let pv: WorkspacePreviewResponse = serde_json::from_slice(
        &req(
            server.addr,
            "GET",
            "/api/workspace/preview?path=main.rs&line=2",
            auth_ref,
            None,
        )
        .await
        .body,
    )
    .unwrap();
    assert_eq!(pv.title, "main.rs:2");

    let diff_bad = req(
        server.addr,
        "GET",
        "/api/workspace/diff?path=../etc/passwd",
        auth_ref,
        None,
    )
    .await;
    assert_eq!(diff_bad.status, 400);

    let diff_ok: FileDiffResponse = serde_json::from_slice(
        &req(
            server.addr,
            "GET",
            "/api/workspace/diff?path=README.md&status=%3F%3F",
            auth_ref,
            None,
        )
        .await
        .body,
    )
    .unwrap();
    assert_eq!(diff_ok.additions, 1);

    // Providers, API key (never echoed), base-url, credential removal, OAuth start + cancel
    let prov: ProvidersResponse = serde_json::from_slice(
        &req(server.addr, "GET", "/api/providers", auth_ref, None)
            .await
            .body,
    )
    .unwrap();
    assert_eq!(prov.providers[0].id, "gemini");

    let key_res = req(
        server.addr,
        "POST",
        "/api/providers/gemini/api-key",
        auth_ref,
        Some(br#"{"apiKey":"sk-super-secret-value"}"#),
    )
    .await;
    assert_eq!(key_res.status, 200);
    assert!(!String::from_utf8_lossy(&key_res.body).contains("sk-super-secret-value"));
    assert_eq!(
        opener.saved_api_key.lock().unwrap().as_ref(),
        Some(&("gemini".to_string(), "sk-super-secret-value".to_string()))
    );

    assert_eq!(
        req(
            server.addr,
            "POST",
            "/api/providers/gemini/base-url",
            auth_ref,
            Some(br#"{"baseUrl":"https://proxy.example"}"#),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        req(
            server.addr,
            "DELETE",
            "/api/providers/gemini/credential",
            auth_ref,
            Some(b"{}")
        )
        .await
        .status,
        200
    );

    let oauth: ChatGptLoginStartResponse = serde_json::from_slice(
        &req(
            server.addr,
            "POST",
            "/api/providers/openai-codex/chatgpt/start",
            auth_ref,
            Some(b"{}"),
        )
        .await
        .body,
    )
    .unwrap();
    assert_eq!(
        req(
            server.addr,
            "POST",
            &format!(
                "/api/providers/openai-codex/chatgpt/{}/cancel",
                oauth.attempt_id
            ),
            auth_ref,
            Some(b"{}"),
        )
        .await
        .status,
        200
    );

    // Settings update + reload
    let settings_res: ReloadResponse = serde_json::from_slice(
        &req(
            server.addr,
            "POST",
            "/api/settings",
            auth_ref,
            Some(br#"{"resetTheme":false}"#),
        )
        .await
        .body,
    )
    .unwrap();
    assert_eq!(settings_res.warnings, vec!["updated"]);

    let reloaded: ReloadResponse = serde_json::from_slice(
        &req(server.addr, "POST", "/api/reload", auth_ref, Some(b"{}"))
            .await
            .body,
    )
    .unwrap();
    let _: HostInfoDto = reloaded.host_info;

    server.shutdown().await.unwrap();
}
