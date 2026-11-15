use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lsp_types::{
    ClientCapabilities, Diagnostic, DiagnosticClientCapabilities, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, InitializeParams, InitializeResult, InitializedParams,
    PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams, ServerCapabilities,
    TextDocumentClientCapabilities, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentSyncClientCapabilities, Uri, VersionedTextDocumentIdentifier,
    WorkspaceFolder,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};

use super::uri::path_to_uri;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DIAG_WAIT_TIMEOUT: Duration = Duration::from_millis(500);

type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;
type DiagnosticsMap = Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>;

#[derive(Debug)]
pub(crate) struct LspClient {
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicI64,
    diagnostics: DiagnosticsMap,
    diag_notify: Arc<Notify>,
    opened_docs: AsyncMutex<HashMap<String, (i32, String)>>,
    capabilities: ServerCapabilities,
    alive: Arc<AtomicBool>,
    _child: AsyncMutex<Child>,
}

impl LspClient {
    pub(crate) async fn spawn_and_initialize(
        command: &str,
        args: &[String],
        workspace_root: &Path,
    ) -> Result<Arc<Self>, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        crate::code::shell::scrub_command_env(&mut cmd, &[]);

        let mut child = cmd.spawn().map_err(|err| {
            if err.kind() == ErrorKind::NotFound {
                format!("language server '{command}' not found in PATH")
            } else {
                format!("failed to start language server '{command}': {err}")
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("language server '{command}' missing stdin pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("language server '{command}' missing stdout pipe"))?;

        let stdin = Arc::new(AsyncMutex::new(stdin));
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let diagnostics: DiagnosticsMap = Arc::new(Mutex::new(HashMap::new()));
        let diag_notify = Arc::new(Notify::new());
        let alive = Arc::new(AtomicBool::new(true));

        tokio::spawn(read_loop(
            stdout,
            Arc::clone(&stdin),
            Arc::clone(&pending),
            Arc::clone(&diagnostics),
            Arc::clone(&diag_notify),
            Arc::clone(&alive),
        ));

        let root_uri = path_to_uri(workspace_root)?;
        let folder_name = workspace_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string();

        #[allow(deprecated)]
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: None,
            root_uri: Some(root_uri.clone()),
            initialization_options: None,
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        dynamic_registration: Some(false),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        did_save: Some(false),
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities::default()),
                    diagnostic: Some(DiagnosticClientCapabilities::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            trace: None,
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: folder_name,
            }]),
            client_info: None,
            locale: None,
            work_done_progress_params: Default::default(),
        };

        let mut client = Self {
            stdin,
            pending,
            next_id: AtomicI64::new(1),
            diagnostics,
            diag_notify,
            opened_docs: AsyncMutex::new(HashMap::new()),
            capabilities: ServerCapabilities::default(),
            alive,
            _child: AsyncMutex::new(child),
        };

        let init_res: InitializeResult = client
            .request("initialize", init_params)
            .await
            .map_err(|err| format!("language server '{command}' failed to initialize: {err}"))?;
        client.capabilities = init_res.capabilities;

        client.notify("initialized", InitializedParams {}).await?;

        Ok(Arc::new(client))
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub(crate) fn supports_pull_diagnostics(&self) -> bool {
        self.capabilities.diagnostic_provider.is_some()
    }

    pub(crate) async fn sync_document(
        &self,
        uri: &Uri,
        language_id: &str,
        content: &str,
    ) -> Result<bool, String> {
        let key = uri.as_str().to_string();
        let mut docs = self.opened_docs.lock().await;
        match docs.get_mut(&key) {
            None => {
                docs.insert(key, (1, content.to_string()));
                drop(docs);
                let params = DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: uri.clone(),
                        language_id: language_id.to_string(),
                        version: 1,
                        text: content.to_string(),
                    },
                };
                self.notify("textDocument/didOpen", params).await?;
                Ok(true)
            }
            Some((version, previous)) if previous != content => {
                *version += 1;
                let next_version = *version;
                *previous = content.to_string();
                drop(docs);
                if let Ok(mut diags) = self.diagnostics.lock() {
                    diags.remove(&key);
                }
                let params = DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version: next_version,
                    },
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: content.to_string(),
                    }],
                };
                self.notify("textDocument/didChange", params).await?;
                Ok(true)
            }
            Some(_) => Ok(false),
        }
    }

    pub(crate) async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, String> {
        if !self.is_alive() {
            return Err("language server process exited".to_string());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self
                .pending
                .lock()
                .map_err(|_| "pending request lock poisoned".to_string())?;
            guard.insert(id, tx);
        }

        if let Err(err) = write_frame(&self.stdin, &payload).await {
            if let Ok(mut guard) = self.pending.lock() {
                guard.remove(&id);
            }
            return Err(err);
        }

        let response = tokio::time::timeout(REQUEST_TIMEOUT, rx)
            .await
            .map_err(|_| {
                if let Ok(mut guard) = self.pending.lock() {
                    guard.remove(&id);
                }
                format!("LSP request '{method}' timed out")
            })?
            .map_err(|_| "language server closed response channel".to_string())??;

        serde_json::from_value(response)
            .map_err(|err| format!("invalid LSP response for '{method}': {err}"))
    }

    pub(crate) async fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), String> {
        if !self.is_alive() {
            return Err("language server process exited".to_string());
        }
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        write_frame(&self.stdin, &payload).await
    }

    pub(crate) async fn published_diagnostics(
        &self,
        uri: &Uri,
        wait_if_missing: bool,
    ) -> Option<Vec<Diagnostic>> {
        let key = uri.as_str();
        if let Ok(guard) = self.diagnostics.lock()
            && let Some(diags) = guard.get(key)
        {
            return Some(diags.clone());
        }

        if !wait_if_missing {
            return None;
        }

        let deadline = tokio::time::Instant::now() + DIAG_WAIT_TIMEOUT;
        loop {
            let notified = self.diag_notify.notified();
            if let Ok(guard) = self.diagnostics.lock()
                && let Some(diags) = guard.get(key)
            {
                return Some(diags.clone());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, notified).await.is_err() {
                return self
                    .diagnostics
                    .lock()
                    .ok()
                    .and_then(|guard| guard.get(key).cloned());
            }
        }
    }

    pub(crate) fn text_document_identifier(uri: &Uri) -> TextDocumentIdentifier {
        TextDocumentIdentifier { uri: uri.clone() }
    }
}

async fn write_frame(stdin: &AsyncMutex<ChildStdin>, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value)
        .map_err(|err| format!("failed to serialize LSP message: {err}"))?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut guard = stdin.lock().await;
    guard
        .write_all(header.as_bytes())
        .await
        .map_err(|err| format!("failed writing LSP header: {err}"))?;
    guard
        .write_all(&body)
        .await
        .map_err(|err| format!("failed writing LSP body: {err}"))?;
    guard
        .flush()
        .await
        .map_err(|err| format!("failed flushing LSP stdin: {err}"))?;
    Ok(())
}

async fn read_loop(
    stdout: ChildStdout,
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: PendingMap,
    diagnostics: DiagnosticsMap,
    diag_notify: Arc<Notify>,
    alive: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stdout);
    while let Ok(Some(msg)) = read_frame(&mut reader).await {
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").and_then(Value::as_i64);

        match (id, method) {
            (Some(req_id), Some(_req_method)) => {
                let reply = json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": Value::Null,
                });
                let _ = write_frame(&stdin, &reply).await;
            }
            (Some(resp_id), None) => {
                let sender = pending.lock().ok().and_then(|mut map| map.remove(&resp_id));
                if let Some(tx) = sender {
                    if let Some(err) = msg.get("error") {
                        let message = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("LSP error")
                            .to_string();
                        let _ = tx.send(Err(message));
                    } else {
                        let result = msg.get("result").cloned().unwrap_or(Value::Null);
                        let _ = tx.send(Ok(result));
                    }
                }
            }
            (None, Some("textDocument/publishDiagnostics")) => {
                if let Some(params) = msg.get("params").cloned()
                    && let Ok(parsed) = serde_json::from_value::<PublishDiagnosticsParams>(params)
                {
                    if let Ok(mut guard) = diagnostics.lock() {
                        guard.insert(parsed.uri.as_str().to_string(), parsed.diagnostics);
                    }
                    diag_notify.notify_waiters();
                }
            }
            _ => {}
        }
    }

    alive.store(false, Ordering::SeqCst);
    if let Ok(mut guard) = pending.lock() {
        for (_, tx) in guard.drain() {
            let _ = tx.send(Err("language server exited unexpectedly".to_string()));
        }
    }
    diag_notify.notify_waiters();
}

async fn read_frame(reader: &mut BufReader<ChildStdout>) -> Result<Option<Value>, ()> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await.map_err(|_| ())?;
        if bytes_read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }

    let len = content_length.ok_or(())?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.map_err(|_| ())?;
    let value = serde_json::from_slice(&buf).map_err(|_| ())?;
    Ok(Some(value))
}
