use std::process::Output;
use std::time::Duration;

use meerai_core::{ObservationStatus, ToolDefinition, ToolObservation, ToolOutput, ToolRun};

use crate::code::paths::canonicalize_or_keep;
use crate::code::{CodeTools, failure_run, truncate_middle};
use crate::world::{SandboxPolicy, WorldCommand, WorldExecOutcome};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

#[cfg(unix)]
const SHELL_PROGRAM: &str = "sh";
#[cfg(unix)]
const SHELL_ARGS: [&str; 1] = ["-c"];
#[cfg(unix)]
const SHELL_DESCRIPTION: &str = "(`sh -c`)";

#[cfg(windows)]
const SHELL_PROGRAM: &str = "cmd";
#[cfg(windows)]
const SHELL_ARGS: [&str; 1] = ["/C"];
#[cfg(windows)]
const SHELL_DESCRIPTION: &str = "(`cmd /C`)";

const SENSITIVE_ENV_NAMES: [&str; 11] = [
    "MEERAI_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "OPEN_ROUTER_API_KEY",
    "GEMINI_API_KEY",
    "ANTHROPIC_API_KEY",
    "DEEPSEEK_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "AZURE_OPENAI_API_KEY",
];

const SENSITIVE_ENV_SUFFIXES: [&str; 6] = [
    "_API_KEY",
    "_API_TOKEN",
    "_ACCESS_TOKEN",
    "_SECRET",
    "_SECRET_KEY",
    "_PRIVATE_KEY",
];

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ShellArgs {
    #[cfg_attr(
        unix,
        schemars(
            description = "Shell command to run with `sh -c`, using the workspace root as cwd."
        )
    )]
    #[cfg_attr(
        windows,
        schemars(
            description = "Shell command to run with `cmd /C`, using the workspace root as cwd."
        )
    )]
    #[serde(alias = "cmd")]
    command: String,

    #[schemars(description = "Timeout in milliseconds. Defaults to 120000.")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_u64_value")]
    timeout_ms: Option<u64>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<ShellArgs>(
        name,
        &format!(
            "Run a shell command {SHELL_DESCRIPTION} in the workspace root. Returns capped stdout/stderr and the exit code."
        ),
    )
}

fn shell_invocation(command: &str) -> (&'static str, Vec<String>) {
    let mut args: Vec<String> = SHELL_ARGS.iter().map(|arg| (*arg).to_string()).collect();
    args.push(command.to_string());
    (SHELL_PROGRAM, args)
}

fn is_sensitive_env_with(name: &str, extra: &[String]) -> bool {
    let upper = name.to_ascii_uppercase();
    SENSITIVE_ENV_NAMES.contains(&upper.as_str())
        || extra.iter().any(|item| item == &upper)
        || SENSITIVE_ENV_SUFFIXES
            .iter()
            .any(|suffix| upper.ends_with(suffix))
}

#[cfg(test)]
fn is_sensitive_env(name: &str) -> bool {
    is_sensitive_env_with(name, &[])
}

pub(crate) fn scrub_command_env(cmd: &mut tokio::process::Command, extra_sensitive: &[String]) {
    for (name, _) in std::env::vars() {
        if is_sensitive_env_with(&name, extra_sensitive) {
            cmd.env_remove(name);
        }
    }
}

impl CodeTools {
    pub(crate) async fn shell(&self, args: ShellArgs) -> ToolRun {
        let cwd = self.canonical_workspace_root().await;
        match tokio::fs::metadata(&cwd).await {
            Ok(metadata) if metadata.is_dir() => {}
            _ => {
                return failure_run(format!(
                    "workspace root is not a directory: {}",
                    cwd.display()
                ));
            }
        }

        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1);
        let (program, argv) = shell_invocation(&args.command);
        let command = WorldCommand {
            program: program.to_string(),
            args: argv,
            cwd: cwd.clone(),
            removed_env_vars: self.sensitive_env_vars().to_vec(),
        };

        let mut writable_roots = Vec::with_capacity(self.writable_roots().len() + 1);
        for root in self.writable_roots() {
            writable_roots.push(canonicalize_or_keep(root).await);
        }
        for root in self.extra_roots() {
            writable_roots.push(canonicalize_or_keep(&root).await);
        }
        let mut readable_roots = Vec::with_capacity(self.readable_roots().len() + 1);
        for root in self.readable_roots() {
            readable_roots.push(canonicalize_or_keep(root).await);
        }
        for root in self.extra_roots() {
            readable_roots.push(canonicalize_or_keep(&root).await);
        }
        let policy = SandboxPolicy::WorkspaceWrite {
            workspace_root: cwd,
            writable_roots,
            readable_roots,
            allow_network: false,
        };

        match self
            .execution_world()
            .spawn_command(&command, &policy, Duration::from_millis(timeout_ms))
            .await
        {
            Ok(WorldExecOutcome::TimedOut) => {
                let mut run = failure_run(format!(
                    "command timed out after {timeout_ms} ms and was killed: {}",
                    args.command
                ));
                run.observation.hint =
                    Some("Narrow the command, or raise timeout_ms and retry.".to_string());
                run
            }
            Ok(WorldExecOutcome::Output(output)) => complete(output),
            Err(error) => failure_run(format!("cannot start {SHELL_PROGRAM} shell: {error}")),
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
        let mut text = format!("command failed with exit code {status}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_env_names_match_known_provider_keys() {
        for name in [
            "MEERAI_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "OPEN_ROUTER_API_KEY",
            "GEMINI_API_KEY",
            "ANTHROPIC_API_KEY",
            "DEEPSEEK_API_KEY",
            "XAI_API_KEY",
            "MISTRAL_API_KEY",
            "GROQ_API_KEY",
            "AZURE_OPENAI_API_KEY",
        ] {
            assert!(is_sensitive_env(name), "{name}");
        }
    }

    #[test]
    fn sensitive_env_names_match_secret_suffixes() {
        for name in [
            "CUSTOM_API_KEY",
            "SERVICE_API_TOKEN",
            "GITHUB_ACCESS_TOKEN",
            "APP_SECRET",
            "APP_SECRET_KEY",
            "DEPLOY_PRIVATE_KEY",
        ] {
            assert!(is_sensitive_env(name), "{name}");
        }
    }

    #[test]
    fn sensitive_env_names_match_regardless_of_case() {
        for name in [
            "Openai_Api_Key",
            "openrouter_api_key",
            "my_secret",
            "App_Secret_Key",
            "deploy_private_key",
        ] {
            assert!(is_sensitive_env(name), "{name}");
        }
    }

    #[test]
    fn sensitive_env_names_reject_normal_variables() {
        for name in [
            "PATH",
            "HOME",
            "CARGO_HOME",
            "CARGO_PKG_NAME",
            "SECRETS_DIR",
            "API_KEY_PATH",
            "path",
            "cargo_home",
        ] {
            assert!(!is_sensitive_env(name), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn shell_invocation_uses_sh_dash_c() {
        let (program, argv) = shell_invocation("echo hi");

        assert_eq!(program, "sh");
        assert_eq!(argv, vec!["-c".to_string(), "echo hi".to_string()]);
    }

    #[tokio::test]
    async fn shell_delegates_to_configured_execution_world_with_workspace_write_policy() {
        use std::os::unix::process::ExitStatusExt;
        use std::sync::{Arc, Mutex};

        use meerai_core::async_trait;

        use crate::world::{ExecutionWorld, SandboxPolicy, WorldCommand, WorldExecOutcome};

        #[derive(Debug, Default)]
        struct RecordingWorld {
            recorded: Mutex<Vec<(WorldCommand, SandboxPolicy)>>,
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
                command: &WorldCommand,
                policy: &SandboxPolicy,
                _timeout: Duration,
            ) -> std::io::Result<WorldExecOutcome> {
                self.recorded
                    .lock()
                    .unwrap()
                    .push((command.clone(), policy.clone()));
                Ok(WorldExecOutcome::Output(Output {
                    status: std::process::ExitStatus::from_raw(0),
                    stdout: b"from-world".to_vec(),
                    stderr: Vec::new(),
                }))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let extra_write = tempfile::tempdir().unwrap();
        let world = Arc::new(RecordingWorld::default());
        let tools = CodeTools::new(dir.path())
            .with_writable_roots(vec![extra_write.path().to_path_buf()])
            .with_sensitive_env_vars(vec!["CUSTOM_SECRET".to_string()])
            .with_execution_world(world.clone());

        let run = tools
            .shell(ShellArgs {
                command: "echo test".to_string(),
                timeout_ms: Some(2500),
            })
            .await;
        assert_eq!(run.observation.status, ObservationStatus::Success);
        assert_eq!(run.observation.stdout, "from-world");

        let calls = world.recorded.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].0.removed_env_vars,
            vec!["CUSTOM_SECRET".to_string()]
        );
        match &calls[0].1 {
            SandboxPolicy::WorkspaceWrite {
                workspace_root,
                writable_roots,
                allow_network,
                ..
            } => {
                assert_eq!(workspace_root, &dir.path().canonicalize().unwrap());
                assert_eq!(
                    writable_roots,
                    &[extra_write.path().canonicalize().unwrap()]
                );
                assert!(!allow_network);
            }
            other => panic!("expected WorkspaceWrite policy, got {other:?}"),
        }
    }
}
