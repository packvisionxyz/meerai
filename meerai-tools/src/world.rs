use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ignore::WalkBuilder;
use meerai_core::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::code::paths::is_sensitive_path;
use crate::code::shell::scrub_command_env;

const MAX_CAPTURED_STREAM_BYTES: usize = 1024 * 1024;
const MAX_WORKSPACE_SENSITIVE_FILES: usize = 256;
const WORKSPACE_SENSITIVE_SCAN_DEPTH: usize = 5;

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
    pub env_allowlist: Option<Vec<String>>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SensitivePaths {
    pub(crate) dirs: Vec<PathBuf>,
    pub(crate) files: Vec<PathBuf>,
}

fn push_unique_path(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

fn collect_host_sensitive_paths(workspace_root: &Path, out: &mut SensitivePaths) {
    if let Some(home) = meerai_core::env::home_dir() {
        for rel in [".ssh", ".aws", ".gnupg", ".kube", ".docker"] {
            let dir = home.join(rel);
            if !workspace_root.starts_with(&dir) {
                push_unique_path(&mut out.dirs, dir);
            }
        }
        for rel in [".netrc", "_netrc", ".npmrc", ".pypirc", ".git-credentials"] {
            push_unique_path(&mut out.files, home.join(rel));
        }
    }

    for dir in [meerai_core::env::config_dir(), meerai_core::env::data_dir()]
        .into_iter()
        .flatten()
    {
        if !workspace_root.starts_with(&dir) {
            push_unique_path(&mut out.dirs, dir);
        }
    }
}

fn collect_workspace_sensitive_files(workspace_root: &Path, out: &mut SensitivePaths) {
    if !workspace_root.is_dir() {
        return;
    }
    let walker = WalkBuilder::new(workspace_root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .follow_links(false)
        .max_depth(Some(WORKSPACE_SENSITIVE_SCAN_DEPTH))
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(".git" | "target" | "node_modules" | ".venv" | "venv")
            )
        })
        .build();

    for entry in walker.flatten() {
        if out.files.len() >= MAX_WORKSPACE_SENSITIVE_FILES {
            break;
        }
        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_file()) && is_sensitive_path(path) {
            push_unique_path(&mut out.files, path.to_path_buf());
        }
    }
}

fn collect_sensitive_paths(policy: &SandboxPolicy, existing_only: bool) -> SensitivePaths {
    let mut out = SensitivePaths::default();
    let workspace_root = match policy {
        SandboxPolicy::Unrestricted => return out,
        SandboxPolicy::ReadOnly { workspace_root, .. }
        | SandboxPolicy::WorkspaceWrite { workspace_root, .. } => workspace_root.as_path(),
    };

    collect_host_sensitive_paths(workspace_root, &mut out);
    collect_workspace_sensitive_files(workspace_root, &mut out);

    if existing_only {
        out.dirs
            .retain(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()));
        out.files
            .retain(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()));
    }
    out
}

async fn read_capped_stream<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if buf.len() < MAX_CAPTURED_STREAM_BYTES {
            let remaining = MAX_CAPTURED_STREAM_BYTES - buf.len();
            let take = n.min(remaining);
            buf.extend_from_slice(&chunk[..take]);
            if take < n {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    if truncated {
        buf.extend_from_slice(b"\n[stream capped at 1 MiB]");
    }
    Ok(buf)
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
    if let Some(allow) = &command.env_allowlist {
        cmd.env_clear();
        for key in allow {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
    }
    scrub_command_env(&mut cmd, &command.removed_env_vars);
    for var in &command.removed_env_vars {
        cmd.env_remove(var);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().pre_exec(|| {
            let file_limit = libc::rlimit {
                rlim_cur: 1024 * 1024 * 1024,
                rlim_max: 1024 * 1024 * 1024,
            };
            libc::setrlimit(libc::RLIMIT_FSIZE, &file_limit);
            let core_limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::setrlimit(libc::RLIMIT_CORE, &core_limit);
            Ok(())
        });
    }

    let mut child = cmd.spawn()?;
    let stdout_handle = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(read_capped_stream(pipe)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_capped_stream(pipe)));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Err(_) => {
            let _ = child.start_kill();
            if let Some(handle) = stdout_handle {
                handle.abort();
            }
            if let Some(handle) = stderr_handle {
                handle.abort();
            }
            return Ok(WorldExecOutcome::TimedOut);
        }
        Ok(Err(error)) => {
            if let Some(handle) = stdout_handle {
                handle.abort();
            }
            if let Some(handle) = stderr_handle {
                handle.abort();
            }
            return Err(error);
        }
        Ok(Ok(status)) => status,
    };

    let stdout = match stdout_handle {
        Some(handle) => handle.await.unwrap_or(Ok(Vec::new()))?,
        None => Vec::new(),
    };
    let stderr = match stderr_handle {
        Some(handle) => handle.await.unwrap_or(Ok(Vec::new()))?,
        None => Vec::new(),
    };

    Ok(WorldExecOutcome::Output(Output {
        status,
        stdout,
        stderr,
    }))
}

pub fn strict_env_allowlist() -> Vec<String> {
    let mut keys: Vec<String> = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TERM",
        "TMPDIR",
        "PWD",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LC_MESSAGES",
    ]
    .iter()
    .map(|key| key.to_string())
    .collect();
    for (key, _) in std::env::vars() {
        if key.starts_with("LC_") && !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
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

fn push_sbpl_filter(rules: &mut Vec<String>, kind: &str, path: &Path) {
    let primary = normalize_sbpl_subpath(path);
    if !primary.is_empty() {
        let rule = format!("  ({kind} \"{}\")", escape_sbpl_string(&primary));
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    if let Ok(canonical) = std::fs::canonicalize(path) {
        let canon = normalize_sbpl_subpath(&canonical);
        if !canon.is_empty() {
            let rule = format!("  ({kind} \"{}\")", escape_sbpl_string(&canon));
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
}

fn push_sbpl_subpath(rules: &mut Vec<String>, path: &Path) {
    push_sbpl_filter(rules, "subpath", path);
}

fn push_sbpl_literal(rules: &mut Vec<String>, path: &Path) {
    push_sbpl_filter(rules, "literal", path);
}

fn sbpl_sensitive_read_rules(sensitive: &SensitivePaths) -> Vec<String> {
    let mut deny_rules = Vec::new();
    for dir in &sensitive.dirs {
        push_sbpl_subpath(&mut deny_rules, dir);
    }
    for file in &sensitive.files {
        push_sbpl_literal(&mut deny_rules, file);
    }
    deny_rules
}

pub fn seatbelt_profile(policy: &SandboxPolicy) -> String {
    let sensitive = collect_sensitive_paths(policy, false);
    seatbelt_profile_with_sensitive(policy, &sensitive)
}

pub(crate) fn seatbelt_profile_with_sensitive(
    policy: &SandboxPolicy,
    sensitive: &SensitivePaths,
) -> String {
    match policy {
        SandboxPolicy::Unrestricted => "(version 1)\n(allow default)\n".to_string(),
        SandboxPolicy::ReadOnly { allow_network, .. } => {
            let network_rule = if *allow_network {
                "(allow network*)"
            } else {
                "(deny network*)"
            };
            let deny_reads = sbpl_sensitive_read_rules(sensitive);
            let deny_read_block = if deny_reads.is_empty() {
                String::new()
            } else {
                format!("(deny file-read*\n{}\n)\n", deny_reads.join("\n"))
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
                 {deny_read_block}\
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
            let deny_reads = sbpl_sensitive_read_rules(sensitive);
            let deny_read_block = if deny_reads.is_empty() {
                String::new()
            } else {
                format!("(deny file-read*\n{}\n)\n", deny_reads.join("\n"))
            };
            let mut write_rules = vec!["  (literal \"/dev/null\")".to_string()];
            push_sbpl_subpath(&mut write_rules, &std::env::temp_dir());
            push_sbpl_subpath(&mut write_rules, workspace_root);
            for root in writable_roots {
                push_sbpl_subpath(&mut write_rules, root);
            }
            let write_block = write_rules.join("\n");
            let mut deny_writes = Vec::new();
            for file in &sensitive.files {
                push_sbpl_literal(&mut deny_writes, file);
            }
            let deny_write_block = if deny_writes.is_empty() {
                String::new()
            } else {
                format!("(deny file-write*\n{}\n)\n", deny_writes.join("\n"))
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
                 {deny_read_block}\
                 (allow file-write*\n{write_block}\n)\n\
                 {deny_write_block}"
            )
        }
    }
}

pub fn bwrap_args(policy: &SandboxPolicy, command: &WorldCommand) -> Vec<String> {
    let sensitive = collect_sensitive_paths(policy, true);
    bwrap_args_with_sensitive(policy, command, &sensitive)
}

pub(crate) fn bwrap_args_with_sensitive(
    policy: &SandboxPolicy,
    command: &WorldCommand,
    sensitive: &SensitivePaths,
) -> Vec<String> {
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
            push_bwrap_sensitive_masks(&mut args, sensitive);
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

            push_bwrap_sensitive_masks(&mut args, sensitive);

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

fn push_bwrap_sensitive_masks(args: &mut Vec<String>, sensitive: &SensitivePaths) {
    for dir in &sensitive.dirs {
        args.push("--tmpfs".to_string());
        args.push(dir.to_string_lossy().into_owned());
    }
    for file in &sensitive.files {
        args.push("--ro-bind".to_string());
        args.push("/dev/null".to_string());
        args.push(file.to_string_lossy().into_owned());
    }
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
            env_allowlist: None,
        };

        let write_args = bwrap_args_with_sensitive(
            &SandboxPolicy::WorkspaceWrite {
                workspace_root: PathBuf::from("/workspace"),
                writable_roots: vec![PathBuf::from("/workspace"), PathBuf::from("/cache")],
                readable_roots: Vec::new(),
                allow_network: false,
            },
            &cmd,
            &SensitivePaths::default(),
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

        let ro_args = bwrap_args_with_sensitive(
            &SandboxPolicy::ReadOnly {
                workspace_root: PathBuf::from("/workspace"),
                readable_roots: Vec::new(),
                allow_network: true,
            },
            &cmd,
            &SensitivePaths::default(),
        );
        assert!(!ro_args.contains(&"--bind".to_string()));
        assert!(!ro_args.contains(&"--unshare-net".to_string()));
        assert!(ro_args.contains(&"--tmpfs".to_string()));
        assert!(ro_args.contains(&"--die-with-parent".to_string()));
    }

    #[tokio::test]
    async fn sensitive_paths_are_masked_in_seatbelt_and_bwrap() {
        let _lock = ENV_LOCK.lock().await;
        let fake_home = tempfile::tempdir().unwrap();
        let ssh_dir = fake_home.path().join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        let netrc = fake_home.path().join(".netrc");
        std::fs::write(&netrc, "machine example").unwrap();

        let _home = EnvGuard::set("HOME", fake_home.path().to_str().unwrap());
        let _xdg_cfg = EnvGuard::set(
            "XDG_CONFIG_HOME",
            fake_home.path().join(".config").to_str().unwrap(),
        );
        let _xdg_data = EnvGuard::set(
            "XDG_DATA_HOME",
            fake_home.path().join(".local/share").to_str().unwrap(),
        );

        let ws = tempfile::tempdir().unwrap();
        let env_file = ws.path().join(".env");
        std::fs::write(&env_file, "SECRET=1").unwrap();
        let env_example = ws.path().join(".env.example");
        std::fs::write(&env_example, "SECRET=placeholder").unwrap();

        let ro_policy = SandboxPolicy::ReadOnly {
            workspace_root: ws.path().to_path_buf(),
            readable_roots: Vec::new(),
            allow_network: false,
        };
        let profile = seatbelt_profile(&ro_policy);
        assert!(profile.contains("(deny file-read*"));
        assert!(profile.contains(&ssh_dir.to_string_lossy().to_string()));
        assert!(profile.contains(&netrc.to_string_lossy().to_string()));
        assert!(profile.contains(&env_file.to_string_lossy().to_string()));
        assert!(!profile.contains(&env_example.to_string_lossy().to_string()));

        let write_policy = SandboxPolicy::WorkspaceWrite {
            workspace_root: ws.path().to_path_buf(),
            writable_roots: Vec::new(),
            readable_roots: Vec::new(),
            allow_network: false,
        };
        let cmd = WorldCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "echo ok".to_string()],
            cwd: ws.path().to_path_buf(),
            removed_env_vars: Vec::new(),
            env_allowlist: None,
        };
        let args = bwrap_args(&write_policy, &cmd);
        let bind_idx = args.iter().position(|a| a == "--bind").unwrap();
        let tmpfs_ssh_idx = args
            .windows(2)
            .position(|w| w[0] == "--tmpfs" && w[1] == ssh_dir.to_string_lossy())
            .expect("expected --tmpfs for .ssh");
        let ro_env_idx = args
            .windows(3)
            .position(|w| {
                w[0] == "--ro-bind" && w[1] == "/dev/null" && w[2] == env_file.to_string_lossy()
            })
            .expect("expected --ro-bind /dev/null for .env");
        assert!(tmpfs_ssh_idx > bind_idx);
        assert!(ro_env_idx > bind_idx);
        assert!(!args.iter().any(|a| a == &*env_example.to_string_lossy()));
    }

    #[tokio::test]
    async fn read_capped_stream_truncates_large_output_cleanly() {
        let big = vec![b'x'; MAX_CAPTURED_STREAM_BYTES + 4096];
        let captured = read_capped_stream(std::io::Cursor::new(big)).await.unwrap();
        let text = String::from_utf8_lossy(&captured);
        assert!(text.ends_with("[stream capped at 1 MiB]"));
        assert!(captured.len() < MAX_CAPTURED_STREAM_BYTES + 64);
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
                        env_allowlist: None,
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
                        env_allowlist: None,
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

#[cfg(test)]
mod strict_env_tests {
    use super::*;

    #[test]
    fn strict_env_allowlist_keeps_path_and_drops_secrets() {
        let allow = strict_env_allowlist();
        assert!(allow.iter().any(|key| key == "PATH"));
        assert!(!allow.iter().any(|key| key == "SUPER_SECRET"));
    }
}
