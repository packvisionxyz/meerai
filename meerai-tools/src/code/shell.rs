use std::process::{Output, Stdio};
use std::time::Duration;

use meerai_core::{ObservationStatus, ToolDefinition, ToolObservation, ToolOutput, ToolRun};

use crate::code::{CodeTools, failure_run, truncate_middle};

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
    command: String,

    #[schemars(description = "Timeout in milliseconds. Defaults to 120000.")]
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
        let mut child = tokio::process::Command::new(program);
        child
            .args(argv)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        scrub_command_env(&mut child, self.sensitive_env_vars());

        let spawned = child.spawn();

        let child = match spawned {
            Ok(child) => child,
            Err(error) => {
                return failure_run(format!("cannot start {SHELL_PROGRAM} shell: {error}"));
            }
        };

        let waited =
            tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await;

        match waited {
            Err(_) => {
                let mut run = failure_run(format!(
                    "command timed out after {timeout_ms} ms and was killed: {}",
                    args.command
                ));
                run.observation.hint =
                    Some("Narrow the command, or raise timeout_ms and retry.".to_string());
                run
            }
            Ok(Err(error)) => failure_run(format!("command failed: {error}")),
            Ok(Ok(output)) => complete(output),
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
}
