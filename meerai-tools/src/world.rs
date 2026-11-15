use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use meerai_core::async_trait;
use tokio::process::Command;

use crate::code::shell::scrub_command_env;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxPolicy {
    WorkspaceWrite {
        workspace_root: PathBuf,
        writable_roots: Vec<PathBuf>,
        readable_roots: Vec<PathBuf>,
        allow_network: bool,
    },
    ReadOnly {
        workspace_root: PathBuf,
        readable_roots: Vec<PathBuf>,
        allow_network: bool,
    },
    Unrestricted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub removed_env_vars: Vec<String>,
}

#[derive(Debug)]
pub enum WorldExecOutcome {
    Output(Output),
    TimedOut,
}

#[async_trait]
pub trait ExecutionWorld: Send + Sync + std::fmt::Debug {
    fn kind(&self) -> &'static str;
    fn is_sandboxed(&self) -> bool;
    fn warning(&self) -> Option<&str> {
        None
    }
    async fn spawn_command(
        &self,
        command: &WorldCommand,
        policy: &SandboxPolicy,
        timeout: Duration,
    ) -> std::io::Result<WorldExecOutcome>;
}

#[derive(Clone, Debug, Default)]
pub struct HostWorld {
    warning: Option<String>,
}

impl HostWorld {
    pub fn new() -> Self {
        Self { warning: None }
    }

    pub fn with_warning(warning: impl Into<String>) -> Self {
        Self {
            warning: Some(warning.into()),
        }
    }
}

#[async_trait]
impl ExecutionWorld for HostWorld {
    fn kind(&self) -> &'static str {
        "host"
    }

    fn is_sandboxed(&self) -> bool {
        false
    }

    fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    async fn spawn_command(
        &self,
        command: &WorldCommand,
        _policy: &SandboxPolicy,
        timeout: Duration,
    ) -> std::io::Result<WorldExecOutcome> {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args).current_dir(&command.cwd);
        run_command(cmd, command, timeout).await
    }
}

#[derive(Clone, Debug, Default)]
pub struct SeatbeltWorld;

impl SeatbeltWorld {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ExecutionWorld for SeatbeltWorld {
    fn kind(&self) -> &'static str {
        "seatbelt"
    }

    fn is_sandboxed(&self) -> bool {
        true
    }

    async fn spawn_command(
        &self,
        command: &WorldCommand,
        policy: &SandboxPolicy,
        timeout: Duration,
    ) -> std::io::Result<WorldExecOutcome> {
        #[cfg(target_os = "macos")]
        {
            if matches!(policy, SandboxPolicy::Unrestricted) {
                let mut cmd = Command::new(&command.program);
                cmd.args(&command.args).current_dir(&command.cwd);
                return run_command(cmd, command, timeout).await;
            }

            let profile = seatbelt_profile(policy);
            let mut cmd = Command::new("/usr/bin/sandbox-exec");
            cmd.arg("-p")
                .arg(&profile)
                .arg(&command.program)
                .args(&command.args)
                .current_dir(&command.cwd);
            run_command(cmd, command, timeout).await
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (command, policy, timeout);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Seatbelt sandbox is only supported on macOS",
            ))
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BubblewrapWorld;

impl BubblewrapWorld {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ExecutionWorld for BubblewrapWorld {
    fn kind(&self) -> &'static str {
        "bubblewrap"
    }

    fn is_sandboxed(&self) -> bool {
        true
    }

    async fn spawn_command(
        &self,
        command: &WorldCommand,
        policy: &SandboxPolicy,
        timeout: Duration,
    ) -> std::io::Result<WorldExecOutcome> {
        #[cfg(target_os = "linux")]
        {
            if matches!(policy, SandboxPolicy::Unrestricted) {
                let mut cmd = Command::new(&command.program);
                cmd.args(&command.args).current_dir(&command.cwd);
                return run_command(cmd, command, timeout).await;
            }

            let args = bwrap_args(policy, command);
            let mut cmd = Command::new("bwrap");
            cmd.args(args);
            run_command(cmd, command, timeout).await
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (command, policy, timeout);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "bubblewrap sandbox is only supported on Linux",
            ))
        }
    }
}

async fn run_command(
    mut cmd: Command,
    command: &WorldCommand,
    timeout: Duration,
) -> std::io::Result<WorldExecOutcome> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    scrub_command_env(&mut cmd, &command.removed_env_vars);
    for var in &command.removed_env_vars {
        cmd.env_remove(var);
    }

    let child = cmd.spawn()?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => Ok(WorldExecOutcome::TimedOut),
        Ok(Err(error)) => Err(error),
        Ok(Ok(output)) => Ok(WorldExecOutcome::Output(output)),
    }
}

fn escape_sbpl_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

fn normalize_sbpl_subpath(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw == "/" {
        return "/".to_string();
    }
    raw.trim_end_matches('/').to_string()
}

fn push_sbpl_subpath(rules: &mut Vec<String>, path: &Path) {
    let primary = normalize_sbpl_subpath(path);
    if !primary.is_empty() {
        let rule = format!("  (subpath \"{}\")", escape_sbpl_string(&primary));
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    if let Ok(canonical) = std::fs::canonicalize(path) {
        let canon = normalize_sbpl_subpath(&canonical);
        if !canon.is_empty() {
            let rule = format!("  (subpath \"{}\")", escape_sbpl_string(&canon));
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
}

pub fn seatbelt_profile(policy: &SandboxPolicy) -> String {
    match policy {
        SandboxPolicy::Unrestricted => "(version 1)\n(allow default)\n".to_string(),
        SandboxPolicy::ReadOnly { allow_network, .. } => {
            let network_rule = if *allow_network {
                "(allow network*)"
            } else {
                "(deny network*)"
            };
            format!(
                "(version 1)\n\
                 (deny default)\n\
                 (allow process*)\n\
                 (allow sysctl-read)\n\
                 (allow mach-lookup)\n\
                 (allow signal (target self))\n\
                 {network_rule}\n\
                 (allow file-read*)\n\
                 (allow file-write*\n  (literal \"/dev/null\")\n)\n"
            )
        }
        SandboxPolicy::WorkspaceWrite {
            workspace_root,
            writable_roots,
            allow_network,
            ..
        } => {
            let network_rule = if *allow_network {
                "(allow network*)"
            } else {
                "(deny network*)"
            };
            let mut write_rules = vec!["  (literal \"/dev/null\")".to_string()];
            push_sbpl_subpath(&mut write_rules, &std::env::temp_dir());
            push_sbpl_subpath(&mut write_rules, workspace_root);
            for root in writable_roots {
                push_sbpl_subpath(&mut write_rules, root);
            }
            let write_block = write_rules.join("\n");
            format!(
                "(version 1)\n\
                 (deny default)\n\
                 (allow process*)\n\
                 (allow sysctl-read)\n\
                 (allow mach-lookup)\n\
                 (allow signal (target self))\n\
                 {network_rule}\n\
                 (allow file-read*)\n\
                 (allow file-write*\n{write_block}\n)\n"
            )
        }
    }
}

pub fn bwrap_args(policy: &SandboxPolicy, command: &WorldCommand) -> Vec<String> {
    let mut args = Vec::new();
    match policy {
        SandboxPolicy::Unrestricted => {
            args.extend(["--bind", "/", "/"].into_iter().map(str::to_string));
            args.extend(["--dev", "/dev"].into_iter().map(str::to_string));
            args.extend(["--proc", "/proc"].into_iter().map(str::to_string));
        }
        SandboxPolicy::ReadOnly { allow_network, .. } => {
            args.extend(["--ro-bind", "/", "/"].into_iter().map(str::to_string));
            args.extend(["--dev", "/dev"].into_iter().map(str::to_string));
            args.extend(["--proc", "/proc"].into_iter().map(str::to_string));
            args.extend(["--tmpfs", "/tmp"].into_iter().map(str::to_string));
            if !allow_network {
                args.push("--unshare-net".to_string());
            }
        }
        SandboxPolicy::WorkspaceWrite {
            workspace_root,
            writable_roots,
            allow_network,
            ..
        } => {
            args.extend(["--ro-bind", "/", "/"].into_iter().map(str::to_string));
            args.extend(["--dev", "/dev"].into_iter().map(str::to_string));
            args.extend(["--proc", "/proc"].into_iter().map(str::to_string));
            args.extend(["--tmpfs", "/tmp"].into_iter().map(str::to_string));

            let mut seen = Vec::new();
            for root in std::iter::once(workspace_root).chain(writable_roots.iter()) {
                let root_str = root.to_string_lossy().into_owned();
                if !seen.contains(&root_str) {
                    args.push("--bind".to_string());
                    args.push(root_str.clone());
                    args.push(root_str.clone());
                    seen.push(root_str);
                }
            }

            if !allow_network {
                args.push("--unshare-net".to_string());
            }
        }
    }

    args.push("--die-with-parent".to_string());
    args.push("--chdir".to_string());
    args.push(command.cwd.to_string_lossy().into_owned());
    args.push("--".to_string());
    args.push(command.program.clone());
    args.extend(command.args.iter().cloned());
    args
}

fn sandbox_disabled_by_env() -> bool {
    std::env::var("MEERAI_DISABLE_SANDBOX")
        .ok()
        .is_some_and(|val| matches!(val.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

pub async fn detect_execution_world(enabled: bool) -> Arc<dyn ExecutionWorld> {
    if !enabled || sandbox_disabled_by_env() {
        return Arc::new(HostWorld::with_warning(
            "OS sandbox disabled; using Phase-1 in-process path guards",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let ok = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if ok {
            return Arc::new(SeatbeltWorld::new());
        }
        Arc::new(HostWorld::with_warning(
            "Seatbelt sandbox unavailable; falling back to Phase-1 in-process path guards",
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let ok = Command::new("bwrap")
            .args(["--ro-bind", "/", "/", "--", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if ok {
            return Arc::new(BubblewrapWorld::new());
        }
        Arc::new(HostWorld::with_warning(
            "bubblewrap sandbox unavailable; falling back to Phase-1 in-process path guards",
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Arc::new(HostWorld::with_warning(
            "no OS sandbox available on this platform; using Phase-1 in-process path guards",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

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

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
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

    #[test]
    fn seatbelt_profile_workspace_write_denies_network_and_scopes_writes() {
        let profile = seatbelt_profile(&SandboxPolicy::WorkspaceWrite {
            workspace_root: PathBuf::from("/tmp/my\"ws\\dir"),
            writable_roots: vec![PathBuf::from("/tmp/extra")],
            readable_roots: Vec::new(),
            allow_network: false,
        });

        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow process*)"));
        assert!(profile.contains("(allow sysctl-read)"));
        assert!(profile.contains("(allow mach-lookup)"));
        assert!(profile.contains("(allow signal (target self))"));
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("(allow network*)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(literal \"/dev/null\")"));
        assert!(profile.contains("(subpath \"/tmp/my\\\"ws\\\\dir\")"));
        assert!(profile.contains("(subpath \"/tmp/extra\")"));
    }

    #[test]
    fn seatbelt_profile_readonly_allows_only_dev_null_writes() {
        let profile = seatbelt_profile(&SandboxPolicy::ReadOnly {
            workspace_root: PathBuf::from("/tmp/ws"),
            readable_roots: Vec::new(),
            allow_network: false,
        });

        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(literal \"/dev/null\")"));
        assert!(!profile.contains("/tmp/ws"));

        let net_profile = seatbelt_profile(&SandboxPolicy::ReadOnly {
            workspace_root: PathBuf::from("/tmp/ws"),
            readable_roots: Vec::new(),
            allow_network: true,
        });
        assert!(net_profile.contains("(allow network*)"));
        assert!(!net_profile.contains("(deny network*)"));
    }

    #[test]
    fn bwrap_args_builds_workspace_write_and_readonly_invocations() {
        let cmd = WorldCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "echo ok".to_string()],
            cwd: PathBuf::from("/workspace"),
            removed_env_vars: Vec::new(),
        };

        let write_args = bwrap_args(
            &SandboxPolicy::WorkspaceWrite {
                workspace_root: PathBuf::from("/workspace"),
                writable_roots: vec![PathBuf::from("/workspace"), PathBuf::from("/cache")],
                readable_roots: Vec::new(),
                allow_network: false,
            },
            &cmd,
        );
        assert_eq!(
            write_args,
            vec![
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--tmpfs",
                "/tmp",
                "--bind",
                "/workspace",
                "/workspace",
                "--bind",
                "/cache",
                "/cache",
                "--unshare-net",
                "--die-with-parent",
                "--chdir",
                "/workspace",
                "--",
                "sh",
                "-c",
                "echo ok",
            ]
        );

        let ro_args = bwrap_args(
            &SandboxPolicy::ReadOnly {
                workspace_root: PathBuf::from("/workspace"),
                readable_roots: Vec::new(),
                allow_network: true,
            },
            &cmd,
        );
        assert!(!ro_args.contains(&"--bind".to_string()));
        assert!(!ro_args.contains(&"--unshare-net".to_string()));
        assert!(ro_args.contains(&"--tmpfs".to_string()));
        assert!(ro_args.contains(&"--die-with-parent".to_string()));
    }

    #[tokio::test]
    async fn host_world_executes_scrubs_env_and_enforces_timeout() {
        let _lock = ENV_LOCK.lock().await;
        let _guard = EnvGuard::set("OPENAI_API_KEY", "secret-val");
        let _custom = EnvGuard::set("MY_CUSTOM_VAR", "custom-val");

        let dir = tempfile::tempdir().unwrap();
        let world = HostWorld::new();
        assert_eq!(world.kind(), "host");
        assert!(!world.is_sandboxed());
        assert!(world.warning().is_none());

        #[cfg(unix)]
        {
            let outcome = world
                .spawn_command(
                    &WorldCommand {
                        program: "sh".to_string(),
                        args: vec![
                            "-c".to_string(),
                            "printf '%s:%s' \"${OPENAI_API_KEY:-NONE}\" \"${MY_CUSTOM_VAR:-NONE}\""
                                .to_string(),
                        ],
                        cwd: dir.path().to_path_buf(),
                        removed_env_vars: vec!["MY_CUSTOM_VAR".to_string()],
                    },
                    &SandboxPolicy::Unrestricted,
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
            match outcome {
                WorldExecOutcome::Output(out) => {
                    assert!(out.status.success());
                    assert_eq!(String::from_utf8_lossy(&out.stdout), "NONE:NONE");
                }
                WorldExecOutcome::TimedOut => panic!("unexpected timeout"),
            }

            let timed_out = world
                .spawn_command(
                    &WorldCommand {
                        program: "sh".to_string(),
                        args: vec!["-c".to_string(), "sleep 5".to_string()],
                        cwd: dir.path().to_path_buf(),
                        removed_env_vars: Vec::new(),
                    },
                    &SandboxPolicy::Unrestricted,
                    Duration::from_millis(80),
                )
                .await
                .unwrap();
            assert!(matches!(timed_out, WorldExecOutcome::TimedOut));
        }
    }

    #[tokio::test]
    async fn detect_execution_world_respects_disabled_flag_and_env() {
        let _lock = ENV_LOCK.lock().await;
        let _unset = EnvGuard::unset("MEERAI_DISABLE_SANDBOX");

        let disabled = detect_execution_world(false).await;
        assert_eq!(disabled.kind(), "host");
        assert!(!disabled.is_sandboxed());
        assert!(
            disabled
                .warning()
                .is_some_and(|w| w.contains("OS sandbox disabled"))
        );

        let _env = EnvGuard::set("MEERAI_DISABLE_SANDBOX", "true");
        let env_disabled = detect_execution_world(true).await;
        assert_eq!(env_disabled.kind(), "host");
        assert!(!env_disabled.is_sandboxed());
        assert!(
            env_disabled
                .warning()
                .is_some_and(|w| w.contains("OS sandbox disabled"))
        );
    }

    #[tokio::test]
    async fn detect_execution_world_enabled_returns_valid_world() {
        let _lock = ENV_LOCK.lock().await;
        let _unset = EnvGuard::unset("MEERAI_DISABLE_SANDBOX");

        let world = detect_execution_world(true).await;
        if world.is_sandboxed() {
            assert!(world.warning().is_none());
            assert!(matches!(world.kind(), "seatbelt" | "bubblewrap"));
        } else {
            assert_eq!(world.kind(), "host");
            assert!(world.warning().is_some());
        }
    }
}
