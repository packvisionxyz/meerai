mod client;
mod uri;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, GotoDefinitionParams, GotoDefinitionResponse, Location,
    PartialResultParams, Position, ReferenceContext, ReferenceParams, TextDocumentPositionParams,
    WorkDoneProgressParams,
};
use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolOutput, ToolPermissionSpec,
    ToolRun, Toolset, async_trait,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::code::{CodeTools, definition_for, failure_run, parse_args, success_run};
use client::LspClient;
use uri::{path_to_uri, uri_to_path};

const TOOLSET_NAME: &str = "lsp";
const TOOL_GO_TO_DEFINITION: &str = "lsp-go_to_definition";
const TOOL_FIND_REFERENCES: &str = "lsp-find_references";
const TOOL_GET_DIAGNOSTICS: &str = "lsp-get_diagnostics";

#[derive(Clone, Debug)]
struct ServerSpec {
    command: String,
    args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ClientKey {
    workspace: PathBuf,
    server_key: String,
}

#[derive(Clone, Debug)]
pub struct LspToolset {
    workspace_root: PathBuf,
    readable_roots: Vec<PathBuf>,
    overrides: HashMap<String, ServerSpec>,
    clients: Arc<AsyncMutex<HashMap<ClientKey, Arc<LspClient>>>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GoToDefinitionArgs {
    #[schemars(
        description = "Path to the source file (workspace-relative or absolute inside workspace)."
    )]
    path: String,
    #[schemars(description = "1-based line number.")]
    #[serde(deserialize_with = "meerai_core::lenient::u32_value")]
    line: u32,
    #[schemars(description = "1-based character (column) offset.")]
    #[serde(deserialize_with = "meerai_core::lenient::u32_value")]
    character: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct FindReferencesArgs {
    #[schemars(
        description = "Path to the source file (workspace-relative or absolute inside workspace)."
    )]
    path: String,
    #[schemars(description = "1-based line number.")]
    #[serde(deserialize_with = "meerai_core::lenient::u32_value")]
    line: u32,
    #[schemars(description = "1-based character (column) offset.")]
    #[serde(deserialize_with = "meerai_core::lenient::u32_value")]
    character: u32,
    #[schemars(description = "Include the declaration itself in references (defaults to true).")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_bool_value")]
    include_declaration: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetDiagnosticsArgs {
    #[schemars(
        description = "Path to the source file (workspace-relative or absolute inside workspace)."
    )]
    path: String,
}

impl LspToolset {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            readable_roots: Vec::new(),
            overrides: HashMap::new(),
            clients: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    pub fn with_readable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.readable_roots = roots;
        self
    }

    pub fn with_server_override(
        mut self,
        ext_or_lang: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        let key = ext_or_lang
            .into()
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if !key.is_empty() {
            self.overrides.insert(
                key,
                ServerSpec {
                    command: command.into(),
                    args,
                },
            );
        }
        self
    }

    fn resolve_server(&self, path: &Path) -> Result<(String, String, ServerSpec), String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        let (language_id, default_group, default_cmd, default_args): (&str, &str, &str, &[&str]) =
            match ext.as_str() {
                "rs" => ("rust", "rust", "rust-analyzer", &[]),
                "ts" | "mts" | "cts" => (
                    "typescript",
                    "typescript",
                    "typescript-language-server",
                    &["--stdio"],
                ),
                "tsx" => (
                    "typescriptreact",
                    "typescript",
                    "typescript-language-server",
                    &["--stdio"],
                ),
                "js" | "mjs" | "cjs" => (
                    "javascript",
                    "typescript",
                    "typescript-language-server",
                    &["--stdio"],
                ),
                "jsx" => (
                    "javascriptreact",
                    "typescript",
                    "typescript-language-server",
                    &["--stdio"],
                ),
                "go" => ("go", "go", "gopls", &[]),
                "py" | "pyi" => ("python", "python", "pyright-langserver", &["--stdio"]),
                _ => {
                    if let Some(spec) = self.overrides.get(&ext)
                        && !ext.is_empty()
                    {
                        return Ok((ext.clone(), ext, spec.clone()));
                    }
                    return Err(format!(
                        "no language server configured for file: {}",
                        path.display()
                    ));
                }
            };

        if let Some(spec) = self.overrides.get(&ext) {
            return Ok((language_id.to_string(), ext, spec.clone()));
        }
        if let Some(spec) = self.overrides.get(language_id) {
            return Ok((
                language_id.to_string(),
                language_id.to_string(),
                spec.clone(),
            ));
        }
        if let Some(spec) = self.overrides.get(default_group) {
            return Ok((
                language_id.to_string(),
                default_group.to_string(),
                spec.clone(),
            ));
        }

        Ok((
            language_id.to_string(),
            default_group.to_string(),
            ServerSpec {
                command: default_cmd.to_string(),
                args: default_args.iter().map(|s| (*s).to_string()).collect(),
            },
        ))
    }

    async fn client_for(
        &self,
        workspace: &Path,
        server_key: &str,
        spec: &ServerSpec,
    ) -> Result<Arc<LspClient>, String> {
        let key = ClientKey {
            workspace: workspace.to_path_buf(),
            server_key: server_key.to_string(),
        };

        let mut guard = self.clients.lock().await;
        if let Some(existing) = guard.get(&key)
            && existing.is_alive()
        {
            return Ok(Arc::clone(existing));
        }
        guard.remove(&key);

        let client = LspClient::spawn_and_initialize(&spec.command, &spec.args, workspace).await?;
        guard.insert(key, Arc::clone(&client));
        Ok(client)
    }

    async fn prepare_document(
        &self,
        raw_path: &str,
    ) -> Result<(PathBuf, PathBuf, lsp_types::Uri, Arc<LspClient>, bool), Box<ToolRun>> {
        let guard =
            CodeTools::new(&self.workspace_root).with_readable_roots(self.readable_roots.clone());
        let resolved = guard
            .resolve_for_read(raw_path)
            .await
            .map_err(|issue| Box::new(issue.into_run()))?;

        let workspace = guard.canonical_workspace_root().await;
        let (language_id, server_key, spec) = match self.resolve_server(&resolved) {
            Ok(info) => info,
            Err(err) => return Err(Box::new(failure_run(err))),
        };

        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(text) => text,
            Err(err) => {
                return Err(Box::new(failure_run(format!(
                    "failed to read {}: {err}",
                    resolved.display()
                ))));
            }
        };

        let doc_uri = match path_to_uri(&resolved) {
            Ok(u) => u,
            Err(err) => return Err(Box::new(failure_run(err))),
        };

        let client = match self.client_for(&workspace, &server_key, &spec).await {
            Ok(client) => client,
            Err(err) => return Err(Box::new(failure_run(err))),
        };

        let changed = match client.sync_document(&doc_uri, &language_id, &content).await {
            Ok(changed) => changed,
            Err(err) => return Err(Box::new(failure_run(err))),
        };

        Ok((workspace, resolved, doc_uri, client, changed))
    }

    async fn go_to_definition(&self, args: GoToDefinitionArgs) -> ToolRun {
        let Some(position) = to_lsp_position(args.line, args.character) else {
            return failure_run("line and character are 1-based and must be >= 1".to_string());
        };

        let (workspace, _, doc_uri, client, _) = match self.prepare_document(&args.path).await {
            Ok(ok) => ok,
            Err(run) => return *run,
        };

        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: LspClient::text_document_identifier(&doc_uri),
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let response: Option<GotoDefinitionResponse> =
            match client.request("textDocument/definition", params).await {
                Ok(resp) => resp,
                Err(err) => return failure_run(err),
            };

        let locations: Vec<Location> = match response {
            None => Vec::new(),
            Some(GotoDefinitionResponse::Scalar(loc)) => vec![loc],
            Some(GotoDefinitionResponse::Array(locs)) => locs,
            Some(GotoDefinitionResponse::Link(links)) => links
                .into_iter()
                .map(|link| Location {
                    uri: link.target_uri,
                    range: link.target_selection_range,
                })
                .collect(),
        };

        if locations.is_empty() {
            return success_run("No definition found.".to_string());
        }

        let lines: Vec<String> = locations
            .iter()
            .map(|loc| format_location(&workspace, loc))
            .collect();
        success_run(lines.join("\n"))
    }

    async fn find_references(&self, args: FindReferencesArgs) -> ToolRun {
        let Some(position) = to_lsp_position(args.line, args.character) else {
            return failure_run("line and character are 1-based and must be >= 1".to_string());
        };

        let (workspace, _, doc_uri, client, _) = match self.prepare_document(&args.path).await {
            Ok(ok) => ok,
            Err(run) => return *run,
        };

        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: LspClient::text_document_identifier(&doc_uri),
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: args.include_declaration.unwrap_or(true),
            },
        };

        let response: Option<Vec<Location>> =
            match client.request("textDocument/references", params).await {
                Ok(resp) => resp,
                Err(err) => return failure_run(err),
            };

        let locations = response.unwrap_or_default();
        if locations.is_empty() {
            return success_run("No references found.".to_string());
        }

        let lines: Vec<String> = locations
            .iter()
            .map(|loc| format_location(&workspace, loc))
            .collect();
        success_run(lines.join("\n"))
    }

    async fn get_diagnostics(&self, args: GetDiagnosticsArgs) -> ToolRun {
        let (workspace, resolved, doc_uri, client, changed) =
            match self.prepare_document(&args.path).await {
                Ok(ok) => ok,
                Err(run) => return *run,
            };

        let mut diagnostics = client
            .published_diagnostics(&doc_uri, changed)
            .await
            .unwrap_or_default();

        if diagnostics.is_empty() && client.supports_pull_diagnostics() {
            let params = DocumentDiagnosticParams {
                text_document: LspClient::text_document_identifier(&doc_uri),
                identifier: None,
                previous_result_id: None,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            };
            if let Ok(DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
                report,
            ))) = client.request("textDocument/diagnostic", params).await
            {
                diagnostics = report.full_document_diagnostic_report.items;
            }
        }

        let shown_path = display_path(&workspace, &resolved);
        if diagnostics.is_empty() {
            return success_run(format!("No diagnostics for {shown_path}."));
        }

        let lines: Vec<String> = diagnostics
            .iter()
            .map(|diag| format_diagnostic(&shown_path, diag))
            .collect();
        success_run(lines.join("\n"))
    }
}

fn display_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_string_lossy().into_owned(),
        Ok(_) => ".".to_string(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

fn to_lsp_position(line: u32, character: u32) -> Option<Position> {
    if line == 0 || character == 0 {
        return None;
    }
    Some(Position {
        line: line - 1,
        character: character - 1,
    })
}

fn format_location(workspace: &Path, loc: &Location) -> String {
    let path_str = match uri_to_path(&loc.uri) {
        Some(path) => display_path(workspace, &path),
        None => loc.uri.as_str().to_string(),
    };
    let line = loc.range.start.line + 1;
    let col = loc.range.start.character + 1;
    format!("{path_str}:{line}:{col}")
}

fn format_diagnostic(path: &str, diag: &Diagnostic) -> String {
    let severity = match diag.severity {
        Some(DiagnosticSeverity::ERROR) => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "info",
        Some(DiagnosticSeverity::HINT) => "hint",
        _ => "diagnostic",
    };
    let line = diag.range.start.line + 1;
    let col = diag.range.start.character + 1;
    let source = diag
        .source
        .as_deref()
        .map(|s| format!(" [{s}]"))
        .unwrap_or_default();
    format!("{path}:{line}:{col}: {severity}{source}: {}", diag.message)
}

#[async_trait]
impl Toolset for LspToolset {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![
            definition_for::<GoToDefinitionArgs>(
                TOOL_GO_TO_DEFINITION,
                "Resolve the definition location(s) of a symbol at a 1-based line and character.",
            ),
            definition_for::<FindReferencesArgs>(
                TOOL_FIND_REFERENCES,
                "Find all reference locations of a symbol at a 1-based line and character.",
            ),
            definition_for::<GetDiagnosticsArgs>(
                TOOL_GET_DIAGNOSTICS,
                "Get language server diagnostics (errors/warnings) for a file.",
            ),
        ]
    }

    fn contain(&self, fn_name: &str) -> bool {
        matches!(
            fn_name,
            TOOL_GO_TO_DEFINITION | TOOL_FIND_REFERENCES | TOOL_GET_DIAGNOSTICS
        )
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        self.contain(fn_name).then_some(ToolPermissionSpec {
            action: PermissionAction::Read,
            resource_arg: Some("path"),
        })
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        true
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        match fn_name {
            TOOL_GO_TO_DEFINITION => Ok(self.go_to_definition(parse_args(args)?).await),
            TOOL_FIND_REFERENCES => Ok(self.find_references(parse_args(args)?).await),
            TOOL_GET_DIAGNOSTICS => Ok(self.get_diagnostics(parse_args(args)?).await),
            _ => Err(ToolError::InvalidFunctionName(fn_name.to_string())),
        }
    }
}

#[cfg(test)]
mod lenient_tests {
    use super::*;

    #[test]
    fn accepts_numeric_strings_for_positions() {
        let args: GoToDefinitionArgs =
            serde_json::from_str(r#"{"path":"a.rs","line":"12","character":"3"}"#).unwrap();
        assert_eq!(args.line, 12);
        assert_eq!(args.character, 3);

        let args: FindReferencesArgs = serde_json::from_str(
            r#"{"path":"a.rs","line":12,"character":3,"include_declaration":"false"}"#,
        )
        .unwrap();
        assert_eq!(args.include_declaration, Some(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_core::ObservationStatus;
    use std::fs;
    use tempfile::TempDir;

    fn python3_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[tokio::test]
    async fn refuses_path_escape() {
        let dir = TempDir::new().unwrap();
        let toolset = LspToolset::new(dir.path());
        let run = toolset
            .invoke_structured(
                TOOL_GO_TO_DEFINITION,
                r#"{"path": "../outside.rs", "line": 1, "character": 1}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Refused);
    }

    #[tokio::test]
    async fn missing_binary_returns_structured_failure() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let toolset = LspToolset::new(dir.path()).with_server_override(
            "rs",
            "nonexistent_lsp_server_binary_xyz_999",
            Vec::new(),
        );

        let run = toolset
            .invoke_structured(
                TOOL_GO_TO_DEFINITION,
                r#"{"path": "main.rs", "line": 1, "character": 4}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Error);
        assert!(
            run.observation.stderr.contains("not found"),
            "unexpected error: {}",
            run.observation.stderr
        );
    }

    #[tokio::test]
    async fn end_to_end_mock_lsp_server_protocol() {
        if !python3_available() {
            return;
        }

        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(
            root.join("lib.rs"),
            "pub fn target() {}\nfn caller() { target(); }\n",
        )
        .unwrap();

        let mock_script = root.join("mock_lsp.py");
        fs::write(
            &mock_script,
            r#"import json, sys

def read_msg():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        s = line.decode("utf-8").strip()
        if s == "":
            break
        if s.lower().startswith("content-length:"):
            content_length = int(s.split(":", 1)[1].strip())
    if content_length is None:
        return None
    body = sys.stdin.buffer.read(content_length)
    return json.loads(body.decode("utf-8"))

def send_msg(obj):
    data = json.dumps(obj).encode("utf-8")
    header = f"Content-Length: {len(data)}\r\n\r\n".encode("ascii")
    sys.stdout.buffer.write(header + data)
    sys.stdout.buffer.flush()

opened_uri = None

while True:
    msg = read_msg()
    if msg is None:
        break
    method = msg.get("method")
    msg_id = msg.get("id")
    params = msg.get("params") or {}

    if method == "initialize":
        send_msg({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "capabilities": {
                    "definitionProvider": True,
                    "referencesProvider": True,
                    "textDocumentSync": 1
                }
            }
        })
    elif method == "textDocument/didOpen":
        opened_uri = params["textDocument"]["uri"]
        send_msg({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": opened_uri,
                "diagnostics": [{
                    "range": {
                        "start": {"line": 1, "character": 14},
                        "end": {"line": 1, "character": 20}
                    },
                    "severity": 2,
                    "source": "mock-lsp",
                    "message": "unused return value"
                }]
            }
        })
    elif method == "textDocument/definition":
        pos = params["position"]
        assert pos["line"] == 1 and pos["character"] == 14
        send_msg({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "uri": params["textDocument"]["uri"],
                "range": {
                    "start": {"line": 0, "character": 7},
                    "end": {"line": 0, "character": 13}
                }
            }
        })
    elif method == "textDocument/references":
        include_decl = params["context"]["includeDeclaration"]
        items = [
            {
                "uri": params["textDocument"]["uri"],
                "range": {
                    "start": {"line": 1, "character": 14},
                    "end": {"line": 1, "character": 20}
                }
            }
        ]
        if include_decl:
            items.insert(0, {
                "uri": params["textDocument"]["uri"],
                "range": {
                    "start": {"line": 0, "character": 7},
                    "end": {"line": 0, "character": 13}
                }
            })
        send_msg({"jsonrpc": "2.0", "id": msg_id, "result": items})
    elif method == "shutdown":
        send_msg({"jsonrpc": "2.0", "id": msg_id, "result": None})
"#,
        )
        .unwrap();

        let toolset = LspToolset::new(root).with_server_override(
            "rs",
            "python3",
            vec![mock_script.to_string_lossy().into_owned()],
        );

        let def_run = toolset
            .invoke_structured(
                TOOL_GO_TO_DEFINITION,
                r#"{"path": "lib.rs", "line": 2, "character": 15}"#,
            )
            .await
            .unwrap();
        let ToolOutput::Text(def_out) = def_run.output else {
            panic!("expected definition text, got {:?}", def_run.output);
        };
        assert!(def_out.contains("lib.rs:1:8"), "got: {def_out}");

        let refs_run = toolset
            .invoke_structured(
                TOOL_FIND_REFERENCES,
                r#"{"path": "lib.rs", "line": 1, "character": 8, "include_declaration": true}"#,
            )
            .await
            .unwrap();
        let ToolOutput::Text(refs_out) = refs_run.output else {
            panic!("expected references text, got {:?}", refs_run.output);
        };
        assert!(refs_out.contains("lib.rs:1:8"), "got: {refs_out}");
        assert!(refs_out.contains("lib.rs:2:15"), "got: {refs_out}");

        let diag_run = toolset
            .invoke_structured(TOOL_GET_DIAGNOSTICS, r#"{"path": "lib.rs"}"#)
            .await
            .unwrap();
        let ToolOutput::Text(diag_out) = diag_run.output else {
            panic!("expected diagnostics text, got {:?}", diag_run.output);
        };
        assert!(
            diag_out.contains("lib.rs:2:15: warning [mock-lsp]: unused return value"),
            "got: {diag_out}"
        );
    }
}
