use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use meerai_core::{
    EffectClass, ObservationStatus, PermissionAction, ToolDefinition, ToolError, ToolObservation,
    ToolOutput, ToolPermissionSpec, ToolRun, Toolset, async_trait,
};

use crate::code::paths::PathIssue;
use crate::code::{CodeTools, definition_for, failure_run, parse_args, truncate_middle};
use crate::world::{ExecutionWorld, HostWorld, SandboxPolicy, WorldCommand, WorldExecOutcome};

const TOOLSET_NAME: &str = "sac";
const RUN_SCRIPT_TOOL: &str = "sac-run_script";
const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const MAX_TIMEOUT_MS: u64 = 60_000;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    Python,
    Javascript,
}

impl ScriptLanguage {
    pub fn runtime(self) -> ScriptRuntime {
        match self {
            ScriptLanguage::Python => ScriptRuntime::Python,
            ScriptLanguage::Javascript => ScriptRuntime::Javascript,
        }
    }

    pub fn program(self) -> &'static str {
        self.runtime().program()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptRuntime {
    Python,
    Javascript,
}

impl ScriptRuntime {
    pub fn language(self) -> ScriptLanguage {
        match self {
            ScriptRuntime::Python => ScriptLanguage::Python,
            ScriptRuntime::Javascript => ScriptLanguage::Javascript,
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            ScriptRuntime::Python => "python3",
            ScriptRuntime::Javascript => "node",
        }
    }

    pub fn detect() -> Option<Self> {
        let probe = |program: &str| {
            std::process::Command::new(program)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        if probe("python3") {
            Some(ScriptRuntime::Python)
        } else if probe("node") {
            Some(ScriptRuntime::Javascript)
        } else {
            None
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunScriptArgs {
    #[schemars(
        description = "Script runtime language: `python` (`python3 -I -c`) or `javascript` (`node -e`)."
    )]
    #[serde(alias = "lang")]
    language: ScriptLanguage,
    #[schemars(description = "Inline script source. Provide exactly one of `script` or `path`.")]
    #[serde(alias = "code", default)]
    script: Option<String>,
    #[schemars(
        description = "Path to an existing script file, relative to the workspace root or absolute. Provide exactly one of `script` or `path`."
    )]
    #[serde(alias = "file", alias = "script_path", default)]
    path: Option<String>,
    #[schemars(description = "Timeout in milliseconds (default 15000, max 60000).")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_u64_value")]
    timeout_ms: Option<u64>,
}

enum ScriptSource {
    Inline(String),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct SearchAsCodeToolset {
    workspace_root: PathBuf,
    sensitive_env_vars: Vec<String>,
    execution_world: Arc<dyn ExecutionWorld>,
    runtime: Option<ScriptRuntime>,
}

impl SearchAsCodeToolset {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            sensitive_env_vars: Vec::new(),
            execution_world: Arc::new(HostWorld::new()),
            runtime: None,
        }
    }

    pub fn with_runtime(mut self, runtime: ScriptRuntime) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn runtime(&self) -> Option<ScriptRuntime> {
        self.runtime
    }

    pub fn with_sensitive_env_vars(mut self, vars: Vec<String>) -> Self {
        self.sensitive_env_vars = vars
            .into_iter()
            .map(|v| v.trim().to_ascii_uppercase())
            .filter(|v| !v.is_empty())
            .collect();
        self
    }

    pub fn with_execution_world(mut self, world: Arc<dyn ExecutionWorld>) -> Self {
        self.execution_world = world;
        self
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn execution_world(&self) -> &Arc<dyn ExecutionWorld> {
        &self.execution_world
    }

    async fn run_script(&self, args: RunScriptArgs) -> ToolRun {
        let cwd = CodeTools::new(&self.workspace_root)
            .canonical_workspace_root()
            .await;
        match tokio::fs::metadata(&cwd).await {
            Ok(meta) if meta.is_dir() => {}
            _ => {
                return failure_run(format!(
                    "workspace root is not a directory: {}",
                    cwd.display()
                ));
            }
        }

        let timeout_ms = args
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS);

        if let Some(runtime) = self.runtime
            && args.language != runtime.language()
        {
            return failure_run(format!(
                "{} is not available at startup; use language=\"{}\" instead",
                args.language.program(),
                match runtime {
                    ScriptRuntime::Python => "python",
                    ScriptRuntime::Javascript => "javascript",
                }
            ));
        }

        let source = match (args.script, args.path) {
            (Some(script), None) if !script.trim().is_empty() => ScriptSource::Inline(script),
            (None, Some(path)) if !path.trim().is_empty() => {
                let trimmed = path.trim();
                let candidate = match CodeTools::new(&self.workspace_root)
                    .resolve_for_read(trimmed)
                    .await
                {
                    Ok(resolved) => resolved,
                    Err(PathIssue::Missing(_)) => {
                        return failure_run(format!("script file not found: {trimmed}"));
                    }
                    Err(issue) => return issue.into_run(),
                };
                match tokio::fs::metadata(&candidate).await {
                    Ok(meta) if meta.is_file() => ScriptSource::File(candidate),
                    _ => {
                        return failure_run(format!(
                            "script file not found: {}",
                            candidate.display()
                        ));
                    }
                }
            }
            (Some(_), Some(_)) => {
                return failure_run("provide exactly one of `script` or `path`".to_string());
            }
            _ => return failure_run("provide `script` or `path`".to_string()),
        };

        let (program, argv): (&str, Vec<String>) = match (args.language, &source) {
            (ScriptLanguage::Python, ScriptSource::Inline(script)) => (
                "python3",
                vec!["-I".to_string(), "-c".to_string(), script.clone()],
            ),
            (ScriptLanguage::Python, ScriptSource::File(path)) => (
                "python3",
                vec!["-I".to_string(), path.to_string_lossy().into_owned()],
            ),
            (ScriptLanguage::Javascript, ScriptSource::Inline(script)) => {
                ("node", vec!["-e".to_string(), script.clone()])
            }
            (ScriptLanguage::Javascript, ScriptSource::File(path)) => {
                ("node", vec![path.to_string_lossy().into_owned()])
            }
        };

        let command = WorldCommand {
            program: program.to_string(),
            args: argv,
            cwd: cwd.clone(),
            removed_env_vars: self.sensitive_env_vars.clone(),
        };
        let policy = SandboxPolicy::ReadOnly {
            workspace_root: cwd.clone(),
            readable_roots: vec![cwd],
            allow_network: false,
        };

        match self
            .execution_world
            .spawn_command(&command, &policy, Duration::from_millis(timeout_ms))
            .await
        {
            Ok(WorldExecOutcome::TimedOut) => {
                let mut run = failure_run(format!(
                    "script timed out after {timeout_ms} ms and was killed"
                ));
                run.observation.hint =
                    Some("Narrow the script scope, or raise timeout_ms (up to 60000).".to_string());
                run
            }
            Ok(WorldExecOutcome::Output(output)) => complete(output),
            Err(error) => failure_run(format!("cannot start {program}: {error}")),
        }
    }
}

fn complete(output: Output) -> ToolRun {
    let exit_code = output.status.code();
    let stdout = truncate_middle(&String::from_utf8_lossy(&output.stdout), MAX_OUTPUT_BYTES);
    let stderr = truncate_middle(&String::from_utf8_lossy(&output.stderr), MAX_OUTPUT_BYTES);
    let combined = truncate_middle(&combine(&stdout, &stderr), MAX_OUTPUT_BYTES);
    let succeeded = output.status.success();

    let observation = ToolObservation {
        status: if succeeded {
            ObservationStatus::Success
        } else {
            ObservationStatus::Error
        },
        exit_code,
        stdout,
        stderr,
        hint: None,
    };

    if succeeded {
        let text = if combined.is_empty() {
            "(no output)".to_string()
        } else {
            combined
        };
        ToolRun {
            output: ToolOutput::Text(text),
            observation,
            file_changes: Vec::new(),
        }
    } else {
        let status = exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let mut text = format!("script failed with exit code {status}");
        if !combined.is_empty() {
            text.push('\n');
            text.push_str(&combined);
        }
        ToolRun {
            output: ToolOutput::Fail(text),
            observation,
            file_changes: Vec::new(),
        }
    }
}

fn combine(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

#[async_trait]
impl Toolset for SearchAsCodeToolset {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        let description = match self.runtime {
            Some(ScriptRuntime::Python) => {
                "Run a short Python analysis script (python3 -I) inline or from a file in the workspace root with sensitive environment variables scrubbed. Use `script` for inline code or `path` for an existing file. Prefer one script over many small lookups when a task needs multi-step scanning or aggregation."
            }
            Some(ScriptRuntime::Javascript) => {
                "Run a short JavaScript analysis script (node) inline or from a file in the workspace root with sensitive environment variables scrubbed. Use `script` for inline code or `path` for an existing file. Prefer one script over many small lookups when a task needs multi-step scanning or aggregation."
            }
            None => {
                "Run a short Python or JavaScript analysis script inline or from a file in the workspace root with sensitive environment variables scrubbed."
            }
        };
        vec![definition_for::<RunScriptArgs>(
            RUN_SCRIPT_TOOL,
            description,
        )]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == RUN_SCRIPT_TOOL
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Execute
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        (fn_name == RUN_SCRIPT_TOOL).then_some(ToolPermissionSpec {
            action: PermissionAction::Shell,
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
        if fn_name != RUN_SCRIPT_TOOL {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }
        Ok(self.run_script(parse_args(args)?).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    fn command_available(program: &str) -> bool {
        std::process::Command::new(program)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, val);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(val) => std::env::set_var(self.key, val),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[tokio::test]
    async fn executes_python_script_in_workspace() {
        if !command_available("python3") {
            return;
        }
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello sac").unwrap();

        let toolset = SearchAsCodeToolset::new(dir.path());
        let run = toolset
            .invoke_structured(
                RUN_SCRIPT_TOOL,
                r#"{"language": "python", "script": "print(open('sample.txt').read())"}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Success);
        assert_eq!(run.observation.exit_code, Some(0));
        assert_eq!(run.observation.stdout.trim(), "hello sac");
    }

    #[tokio::test]
    async fn executes_python_script_file_in_workspace() {
        if !command_available("python3") {
            return;
        }
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello file").unwrap();
        fs::write(
            dir.path().join("scan.py"),
            "print(open('sample.txt').read())",
        )
        .unwrap();

        let toolset = SearchAsCodeToolset::new(dir.path());
        let run = toolset
            .invoke_structured(
                RUN_SCRIPT_TOOL,
                r#"{"language": "python", "path": "scan.py"}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Success);
        assert_eq!(run.observation.exit_code, Some(0));
        assert_eq!(run.observation.stdout.trim(), "hello file");
    }

    #[tokio::test]
    async fn requires_exactly_one_script_source() {
        let dir = TempDir::new().unwrap();
        let toolset = SearchAsCodeToolset::new(dir.path());
        for args in [
            r#"{"language": "python"}"#,
            r#"{"language": "python", "script": "print(1)", "path": "scan.py"}"#,
            r#"{"language": "python", "path": "missing.py"}"#,
        ] {
            let run = toolset
                .invoke_structured(RUN_SCRIPT_TOOL, args)
                .await
                .unwrap();
            assert_eq!(run.observation.status, ObservationStatus::Error);
            let text = match &run.output {
                ToolOutput::Fail(text) | ToolOutput::Text(text) => text.clone(),
                ToolOutput::Stop(reason) => format!("{reason:?}"),
            };
            assert!(
                text.contains("exactly one")
                    || text.contains("provide `script`")
                    || text.contains("not found"),
                "{text}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_language_outside_detected_runtime() {
        let dir = TempDir::new().unwrap();
        let toolset = SearchAsCodeToolset::new(dir.path()).with_runtime(ScriptRuntime::Python);
        let run = toolset
            .invoke_structured(
                RUN_SCRIPT_TOOL,
                r#"{"language": "javascript", "script": "console.log(1)"}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Error);
        let output = match &run.output {
            ToolOutput::Fail(text) | ToolOutput::Text(text) => text.clone(),
            ToolOutput::Stop(reason) => format!("{reason:?}"),
        };
        assert!(
            output.contains("use language=\"python\" instead"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn executes_javascript_script_in_workspace() {
        if !command_available("node") {
            return;
        }
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("data.json"), r#"{"ok":true}"#).unwrap();

        let toolset = SearchAsCodeToolset::new(dir.path());
        let run = toolset
            .invoke_structured(
                RUN_SCRIPT_TOOL,
                r#"{"language": "javascript", "script": "const fs = require('fs'); console.log(JSON.parse(fs.readFileSync('data.json', 'utf8')).ok);"}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Success);
        assert_eq!(run.observation.exit_code, Some(0));
        assert_eq!(run.observation.stdout.trim(), "true");
    }

    #[tokio::test]
    async fn enforces_timeout_and_kills_process() {
        if !command_available("python3") {
            return;
        }
        let dir = TempDir::new().unwrap();
        let toolset = SearchAsCodeToolset::new(dir.path());
        let run = toolset
            .invoke_structured(
                RUN_SCRIPT_TOOL,
                r#"{"language": "python", "script": "import time; time.sleep(5)", "timeout_ms": 100}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Error);
        assert!(
            run.observation.stderr.contains("timed out"),
            "unexpected stderr: {}",
            run.observation.stderr
        );
    }

    #[tokio::test]
    async fn scrubs_sensitive_env_vars_from_child() {
        if !command_available("python3") {
            return;
        }
        let _lock = ENV_LOCK.lock().await;
        let _guard = EnvGuard::set("OPENAI_API_KEY", "sk-secret-value-123");
        let _extra = EnvGuard::set("CUSTOM_TOOL_TOKEN", "tok-secret-456");

        let dir = TempDir::new().unwrap();
        let toolset = SearchAsCodeToolset::new(dir.path())
            .with_sensitive_env_vars(vec!["CUSTOM_TOOL_TOKEN".to_string()]);
        let run = toolset
            .invoke_structured(
                RUN_SCRIPT_TOOL,
                r#"{"language": "python", "script": "import os; print(os.environ.get('OPENAI_API_KEY', 'MISSING'), os.environ.get('CUSTOM_TOOL_TOKEN', 'MISSING'))"}"#,
            )
            .await
            .unwrap();

        assert_eq!(run.observation.status, ObservationStatus::Success);
        assert_eq!(run.observation.stdout.trim(), "MISSING MISSING");
    }

    #[tokio::test]
    async fn uses_readonly_network_free_policy_in_execution_world() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            use std::sync::Mutex as StdMutex;

            #[derive(Debug, Default)]
            struct RecordingWorld {
                policies: StdMutex<Vec<SandboxPolicy>>,
            }

            #[async_trait]
            impl ExecutionWorld for RecordingWorld {
                fn kind(&self) -> &'static str {
                    "recording"
                }

                fn is_sandboxed(&self) -> bool {
                    true
                }

                async fn spawn_command(
                    &self,
                    _command: &WorldCommand,
                    policy: &SandboxPolicy,
                    _timeout: Duration,
                ) -> std::io::Result<WorldExecOutcome> {
                    self.policies.lock().unwrap().push(policy.clone());
                    Ok(WorldExecOutcome::Output(Output {
                        status: std::process::ExitStatus::from_raw(0),
                        stdout: b"ok".to_vec(),
                        stderr: Vec::new(),
                    }))
                }
            }

            let dir = TempDir::new().unwrap();
            let world = Arc::new(RecordingWorld::default());
            let toolset = SearchAsCodeToolset::new(dir.path()).with_execution_world(world.clone());

            let run = toolset
                .invoke_structured(
                    RUN_SCRIPT_TOOL,
                    r#"{"language": "python", "script": "print('ok')"}"#,
                )
                .await
                .unwrap();
            assert_eq!(run.observation.status, ObservationStatus::Success);

            let recorded = world.policies.lock().unwrap();
            assert_eq!(recorded.len(), 1);
            match &recorded[0] {
                SandboxPolicy::ReadOnly {
                    workspace_root,
                    readable_roots,
                    allow_network,
                } => {
                    let canon = dir.path().canonicalize().unwrap();
                    assert_eq!(workspace_root, &canon);
                    assert_eq!(readable_roots, &[canon]);
                    assert!(!allow_network);
                }
                other => panic!("expected ReadOnly policy, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn rejects_script_path_escaping_workspace_or_sensitive() {
        let outside = TempDir::new().unwrap();
        let outside_script = outside.path().join("outside.py");
        fs::write(&outside_script, "print('escape')").unwrap();

        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
        let toolset = SearchAsCodeToolset::new(dir.path());

        let escape_args = serde_json::json!({
            "language": "python",
            "path": outside_script.to_string_lossy(),
        })
        .to_string();
        let escape_run = toolset
            .invoke_structured(RUN_SCRIPT_TOOL, &escape_args)
            .await
            .unwrap();
        assert_eq!(escape_run.observation.status, ObservationStatus::Refused);

        let sensitive_run = toolset
            .invoke_structured(RUN_SCRIPT_TOOL, r#"{"language": "python", "path": ".env"}"#)
            .await
            .unwrap();
        assert_eq!(sensitive_run.observation.status, ObservationStatus::Refused);
    }
}
