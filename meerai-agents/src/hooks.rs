use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures::future::join_all;
use meerai_common::hooks::{HookEntry, HookEvent};
use meerai_core::text::floor_char_boundary;
use meerai_session::{HookDetail, SessionEvent};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};

use crate::events::TurnId;

pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 10_000;
pub use meerai_common::hooks::MAX_HOOK_TIMEOUT_MS;
pub const MAX_HOOK_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_HOOK_REASON_CHARS: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HookDecision {
    #[default]
    Pass,
    Allow,
    Ask,
    Deny,
    Block,
}

impl HookDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            HookDecision::Pass => "pass",
            HookDecision::Allow => "allow",
            HookDecision::Ask => "ask",
            HookDecision::Deny => "deny",
            HookDecision::Block => "block",
        }
    }

    fn priority(self) -> u8 {
        match self {
            HookDecision::Pass => 0,
            HookDecision::Allow => 1,
            HookDecision::Ask => 2,
            HookDecision::Block => 3,
            HookDecision::Deny => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookFiring {
    pub event: HookEvent,
    pub decision: HookDecision,
    pub tool: Option<String>,
    pub reason: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub fail_closed: Option<bool>,
}

impl HookFiring {
    pub fn into_session_event(self) -> SessionEvent {
        SessionEvent::HookFired {
            event: self.event.as_str().to_string(),
            decision: self.decision.as_str().to_string(),
            detail: Some(HookDetail {
                tool: self.tool,
                reason: self.reason,
                exit_code: self.exit_code,
                duration_ms: self.duration_ms,
                fail_closed: self.fail_closed,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookReport {
    pub decision: HookDecision,
    pub firings: Vec<HookFiring>,
}

impl HookReport {
    pub fn reason(&self) -> Option<&str> {
        self.firings
            .iter()
            .filter(|firing| firing.decision == self.decision)
            .find_map(|firing| firing.reason.as_deref())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HookContext<'a> {
    pub session_id: &'a str,
    pub turn_id: Option<&'a TurnId>,
    pub tool_name: Option<&'a str>,
    pub tool_input: Option<&'a str>,
    pub tool_output: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub cwd: &'a Path,
}

#[derive(Clone, Debug, Default)]
pub struct HookEngine {
    entries: Vec<HookEntry>,
}

impl HookEngine {
    pub fn new(entries: Vec<HookEntry>) -> Self {
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[HookEntry] {
        &self.entries
    }

    pub async fn run(&self, event: HookEvent, ctx: &HookContext<'_>) -> HookReport {
        let firings = join_all(
            self.entries
                .iter()
                .filter(|entry| entry.event == event && entry_matches(entry, ctx))
                .map(|entry| fire(entry, ctx)),
        )
        .await;
        let decision = firings.iter().fold(HookDecision::Pass, |combined, firing| {
            if firing.decision.priority() > combined.priority() {
                firing.decision
            } else {
                combined
            }
        });
        HookReport { decision, firings }
    }
}

#[derive(Serialize)]
struct HookPayload<'a> {
    event: &'static str,
    session_id: &'a str,
    turn_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_input: Option<&'a str>,
    tool_output: Option<&'a str>,
    prompt: Option<&'a str>,
    cwd: &'a str,
}

#[derive(Deserialize)]
struct PreToolUseDocument {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: Option<PreToolUseOutput>,
}

#[derive(Deserialize)]
struct PreToolUseOutput {
    #[serde(rename = "permissionDecision")]
    permission_decision: Option<String>,
    reason: Option<String>,
}

#[derive(Deserialize)]
struct StopDocument {
    decision: Option<String>,
    reason: Option<String>,
}

struct HookOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn entry_matches(entry: &HookEntry, ctx: &HookContext<'_>) -> bool {
    matcher_matches(entry.matcher.as_deref(), ctx.tool_name)
        && pattern_matches(entry.if_pattern.as_deref(), ctx.tool_input)
}

fn matcher_matches(matcher: Option<&str>, tool_name: Option<&str>) -> bool {
    match matcher {
        None => true,
        Some("*") => true,
        Some(pattern) => tool_name.is_some_and(|name| wildcard_match(pattern, name)),
    }
}

fn pattern_matches(pattern: Option<&str>, tool_input: Option<&str>) -> bool {
    match pattern {
        None => true,
        Some(pattern) => tool_input.is_some_and(|input| input.contains(pattern)),
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    let mut resume = 0;

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(star_index) = star {
            p = star_index + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

async fn fire(entry: &HookEntry, ctx: &HookContext<'_>) -> HookFiring {
    let started = Instant::now();
    let timeout_ms = entry
        .timeout_ms
        .unwrap_or(DEFAULT_HOOK_TIMEOUT_MS)
        .clamp(1, MAX_HOOK_TIMEOUT_MS);
    let outcome =
        tokio::time::timeout(Duration::from_millis(timeout_ms), execute(entry, ctx)).await;
    let duration_ms = started.elapsed().as_millis() as u64;

    let (decision, reason, exit_code) = match outcome {
        Ok(Ok(output)) => {
            let (decision, reason) = interpret(entry.event, &output);
            (decision, reason, output.exit_code)
        }
        Ok(Err(reason)) => (failure_decision(entry), Some(reason), None),
        Err(_) => (
            failure_decision(entry),
            Some(format!("hook timed out after {timeout_ms}ms")),
            None,
        ),
    };

    HookFiring {
        event: entry.event,
        decision,
        tool: ctx.tool_name.map(str::to_string),
        reason: reason.map(truncate_reason),
        exit_code,
        duration_ms: Some(duration_ms),
        fail_closed: entry.fail_closed,
    }
}

async fn execute(entry: &HookEntry, ctx: &HookContext<'_>) -> Result<HookOutput, String> {
    let payload = payload_bytes(entry.event, ctx)?;
    let mut child = shell_command(&entry.command, ctx.cwd)
        .spawn()
        .map_err(|error| format!("hook spawn failed: {error}"))?;
    let writer = tokio::spawn(write_payload(child.stdin.take(), payload));
    let (stdout, stderr, status) = tokio::join!(
        read_capped(child.stdout.take()),
        read_capped(child.stderr.take()),
        child.wait(),
    );
    let status = status.map_err(|error| format!("hook wait failed: {error}"))?;
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("hook stdin failed: {error}")),
        Err(error) => return Err(format!("hook stdin task failed: {error}")),
    }
    let stdout = stdout.map_err(|error| format!("hook stdout failed: {error}"))?;
    let stderr = stderr.map_err(|error| format!("hook stderr failed: {error}"))?;
    Ok(HookOutput {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

async fn read_capped<R>(reader: Option<R>) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return Ok(Vec::new());
    };
    let mut captured = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(captured);
        }
        let remaining = MAX_HOOK_OUTPUT_BYTES.saturating_sub(captured.len());
        captured.extend_from_slice(&chunk[..read.min(remaining)]);
    }
}

async fn write_payload(stdin: Option<ChildStdin>, payload: Vec<u8>) -> std::io::Result<()> {
    let Some(mut stdin) = stdin else {
        return Ok(());
    };
    if let Err(error) = stdin.write_all(&payload).await {
        if error.kind() == ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(error);
    }
    match stdin.shutdown().await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}

fn payload_bytes(event: HookEvent, ctx: &HookContext<'_>) -> Result<Vec<u8>, String> {
    let cwd = ctx.cwd.to_string_lossy();
    let payload = HookPayload {
        event: event.as_str(),
        session_id: ctx.session_id,
        turn_id: ctx.turn_id.map(TurnId::as_str),
        tool_name: ctx.tool_name,
        tool_input: ctx.tool_input,
        tool_output: ctx.tool_output,
        prompt: ctx.prompt,
        cwd: cwd.as_ref(),
    };
    serde_json::to_vec(&payload).map_err(|error| format!("hook payload failed: {error}"))
}

fn shell_command(command: &str, cwd: &Path) -> Command {
    #[cfg(windows)]
    let mut child = {
        let mut child = Command::new("cmd");
        child.arg("/C").arg(command);
        child
    };
    #[cfg(unix)]
    let mut child = {
        let mut child = Command::new("sh");
        child.arg("-c").arg(command);
        child
    };
    child
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child
}

fn interpret(event: HookEvent, output: &HookOutput) -> (HookDecision, Option<String>) {
    match output.exit_code {
        Some(0) => parse_stdout(event, &output.stdout),
        Some(2) => (HookDecision::Block, Some(block_reason(&output.stderr))),
        Some(code) => (HookDecision::Pass, Some(format!("exit code {code}"))),
        None => (
            HookDecision::Pass,
            Some("hook terminated without an exit code".to_string()),
        ),
    }
}

fn parse_stdout(event: HookEvent, stdout: &str) -> (HookDecision, Option<String>) {
    let stdout = &stdout[..floor_char_boundary(stdout, MAX_HOOK_OUTPUT_BYTES)];
    match event {
        HookEvent::PreToolUse => parse_permission(stdout),
        HookEvent::Stop => parse_stop(stdout),
        _ => (HookDecision::Pass, None),
    }
}

fn parse_permission(stdout: &str) -> (HookDecision, Option<String>) {
    let Ok(document) = serde_json::from_str::<PreToolUseDocument>(stdout) else {
        return (HookDecision::Pass, None);
    };
    let Some(output) = document.hook_specific_output else {
        return (HookDecision::Pass, None);
    };
    let decision = match output.permission_decision.as_deref() {
        Some("allow") => HookDecision::Allow,
        Some("ask") => HookDecision::Ask,
        Some("deny") => HookDecision::Deny,
        _ => HookDecision::Pass,
    };
    (decision, output.reason)
}

fn parse_stop(stdout: &str) -> (HookDecision, Option<String>) {
    let Ok(document) = serde_json::from_str::<StopDocument>(stdout) else {
        return (HookDecision::Pass, None);
    };
    if document.decision.as_deref() == Some("block") {
        (HookDecision::Block, document.reason)
    } else {
        (HookDecision::Pass, None)
    }
}

fn block_reason(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        "hook blocked".to_string()
    } else {
        trimmed.to_string()
    }
}

fn failure_decision(entry: &HookEntry) -> HookDecision {
    if entry.fail_closed == Some(true) {
        HookDecision::Deny
    } else {
        HookDecision::Pass
    }
}

fn truncate_reason(reason: String) -> String {
    if reason.chars().count() <= MAX_HOOK_REASON_CHARS {
        return reason;
    }
    reason.chars().take(MAX_HOOK_REASON_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "meerai_agents_hooks_{name}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    fn entry(event: HookEvent, command: &str) -> HookEntry {
        HookEntry {
            event,
            matcher: None,
            if_pattern: None,
            command: command.to_string(),
            timeout_ms: None,
            fail_closed: None,
        }
    }

    fn context<'a>(cwd: &'a Path, tool_name: Option<&'a str>) -> HookContext<'a> {
        HookContext {
            session_id: "ses-test",
            turn_id: None,
            tool_name,
            tool_input: None,
            tool_output: None,
            prompt: None,
            cwd,
        }
    }

    #[test]
    fn decision_names() {
        assert_eq!(HookDecision::default(), HookDecision::Pass);
        assert_eq!(HookDecision::Pass.as_str(), "pass");
        assert_eq!(HookDecision::Allow.as_str(), "allow");
        assert_eq!(HookDecision::Ask.as_str(), "ask");
        assert_eq!(HookDecision::Deny.as_str(), "deny");
        assert_eq!(HookDecision::Block.as_str(), "block");
    }

    #[test]
    fn firing_maps_to_hook_fired_event() {
        let firing = HookFiring {
            event: HookEvent::PreToolUse,
            decision: HookDecision::Deny,
            tool: Some("code-shell".to_string()),
            reason: Some("no shell".to_string()),
            exit_code: Some(2),
            duration_ms: Some(12),
            fail_closed: Some(true),
        };
        let event = firing.into_session_event();
        let SessionEvent::HookFired {
            event,
            decision,
            detail,
        } = event
        else {
            panic!("expected HookFired");
        };
        assert_eq!(event, "PreToolUse");
        assert_eq!(decision, "deny");
        let detail = detail.unwrap();
        assert_eq!(detail.tool.as_deref(), Some("code-shell"));
        assert_eq!(detail.reason.as_deref(), Some("no shell"));
        assert_eq!(detail.exit_code, Some(2));
        assert_eq!(detail.duration_ms, Some(12));
        assert_eq!(detail.fail_closed, Some(true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_zero_passes() {
        let dir = temp_dir("exit_zero");
        fs::create_dir_all(&dir).unwrap();
        let report = HookEngine::new(vec![entry(HookEvent::PreToolUse, "exit 0")])
            .run(
                HookEvent::PreToolUse,
                &context(&dir, Some("code-read_file")),
            )
            .await;
        assert_eq!(report.decision, HookDecision::Pass);
        assert_eq!(report.firings.len(), 1);
        let firing = &report.firings[0];
        assert_eq!(firing.event, HookEvent::PreToolUse);
        assert_eq!(firing.exit_code, Some(0));
        assert_eq!(firing.tool.as_deref(), Some("code-read_file"));
        assert_eq!(firing.fail_closed, None);
        assert!(firing.duration_ms.is_some());
        assert!(report.reason().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pre_tool_use_json_decisions() {
        let dir = temp_dir("pre_tool_json");
        fs::create_dir_all(&dir).unwrap();
        let cases = [
            (
                r#"printf '{"hookSpecificOutput":{"permissionDecision":"allow"}}'"#,
                HookDecision::Allow,
            ),
            (
                r#"printf '{"hookSpecificOutput":{"permissionDecision":"ask","reason":"check it"}}'"#,
                HookDecision::Ask,
            ),
            (
                r#"printf '{"hookSpecificOutput":{"permissionDecision":"deny","reason":"no writes"}}'"#,
                HookDecision::Deny,
            ),
        ];
        for (command, expected) in cases {
            let report = HookEngine::new(vec![entry(HookEvent::PreToolUse, command)])
                .run(
                    HookEvent::PreToolUse,
                    &context(&dir, Some("code-write_file")),
                )
                .await;
            assert_eq!(report.decision, expected, "{command}");
        }

        let report = HookEngine::new(vec![entry(
            HookEvent::PreToolUse,
            r#"printf '{"hookSpecificOutput":{"permissionDecision":"deny","reason":"no writes"}}'"#,
        )])
        .run(
            HookEvent::PreToolUse,
            &context(&dir, Some("code-write_file")),
        )
        .await;
        assert_eq!(report.reason(), Some("no writes"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_two_blocks_with_stderr() {
        let dir = temp_dir("exit_two");
        fs::create_dir_all(&dir).unwrap();
        let report = HookEngine::new(vec![entry(
            HookEvent::PreToolUse,
            "printf 'missing tests' >&2; exit 2",
        )])
        .run(HookEvent::PreToolUse, &context(&dir, Some("code-shell")))
        .await;
        assert_eq!(report.decision, HookDecision::Block);
        assert_eq!(report.firings[0].exit_code, Some(2));
        assert_eq!(report.reason(), Some("missing tests"));

        let report = HookEngine::new(vec![entry(HookEvent::PreToolUse, "exit 2")])
            .run(HookEvent::PreToolUse, &context(&dir, Some("code-shell")))
            .await;
        assert_eq!(report.decision, HookDecision::Block);
        assert_eq!(report.reason(), Some("hook blocked"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn large_stdout_is_capped_and_drained() {
        let dir = temp_dir("large_stdout");
        fs::create_dir_all(&dir).unwrap();
        let command = "yes x | head -c 200000";
        let report = HookEngine::new(vec![entry(HookEvent::SessionStart, command)])
            .run(HookEvent::SessionStart, &context(&dir, None))
            .await;
        assert_eq!(report.decision, HookDecision::Pass);
        assert_eq!(report.firings[0].exit_code, Some(0));
        assert!(report.reason().is_none());

        let output = execute(
            &entry(HookEvent::SessionStart, command),
            &context(&dir, None),
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, Some(0));
        assert!(!output.stdout.is_empty());
        assert!(output.stdout.len() <= MAX_HOOK_OUTPUT_BYTES);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn large_stderr_block_reason_is_capped_and_drained() {
        let dir = temp_dir("large_stderr");
        fs::create_dir_all(&dir).unwrap();
        let report = HookEngine::new(vec![entry(
            HookEvent::PreToolUse,
            "yes x | head -c 200000 >&2; exit 2",
        )])
        .run(HookEvent::PreToolUse, &context(&dir, Some("code-shell")))
        .await;
        assert_eq!(report.decision, HookDecision::Block);
        assert_eq!(report.firings[0].exit_code, Some(2));
        let reason = report.reason().unwrap();
        assert_eq!(reason.chars().count(), MAX_HOOK_REASON_CHARS);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_command_uses_platform_shell() {
        let command = shell_command("exit 0", Path::new("."));
        let command = command.as_std();
        #[cfg(unix)]
        {
            assert_eq!(command.get_program(), std::ffi::OsStr::new("sh"));
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec![std::ffi::OsStr::new("-c"), std::ffi::OsStr::new("exit 0")]
            );
        }
        #[cfg(windows)]
        {
            assert_eq!(command.get_program(), std::ffi::OsStr::new("cmd"));
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec![std::ffi::OsStr::new("/C"), std::ffi::OsStr::new("exit 0")]
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_fails_open() {
        let dir = temp_dir("timeout_open");
        fs::create_dir_all(&dir).unwrap();
        let mut hook = entry(HookEvent::PreToolUse, "sleep 5");
        hook.timeout_ms = Some(20);
        let report = HookEngine::new(vec![hook])
            .run(HookEvent::PreToolUse, &context(&dir, Some("code-shell")))
            .await;
        assert_eq!(report.decision, HookDecision::Pass);
        assert_eq!(report.firings[0].exit_code, None);
        assert!(report.reason().unwrap().contains("timed out"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fail_closed_timeout_denies() {
        let dir = temp_dir("timeout_closed");
        fs::create_dir_all(&dir).unwrap();
        let mut hook = entry(HookEvent::PreToolUse, "sleep 5");
        hook.timeout_ms = Some(20);
        hook.fail_closed = Some(true);
        let report = HookEngine::new(vec![hook])
            .run(HookEvent::PreToolUse, &context(&dir, Some("code-shell")))
            .await;
        assert_eq!(report.decision, HookDecision::Deny);
        assert_eq!(report.firings[0].fail_closed, Some(true));
        assert!(report.reason().unwrap().contains("timed out"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn matcher_filters_tool_names() {
        let dir = temp_dir("matcher");
        fs::create_dir_all(&dir).unwrap();
        let mut code_hook = entry(HookEvent::PreToolUse, "exit 0");
        code_hook.matcher = Some("code-*".to_string());
        let mut grep_hook = entry(HookEvent::PreToolUse, "exit 0");
        grep_hook.matcher = Some("grep*".to_string());
        let engine = HookEngine::new(vec![code_hook, grep_hook]);

        let report = engine
            .run(
                HookEvent::PreToolUse,
                &context(&dir, Some("code-read_file")),
            )
            .await;
        assert_eq!(report.firings.len(), 1);

        let mut single_hook = entry(HookEvent::PreToolUse, "exit 0");
        single_hook.matcher = Some("code-?ead_file".to_string());
        let single = HookEngine::new(vec![single_hook]);
        let report = single
            .run(
                HookEvent::PreToolUse,
                &context(&dir, Some("code-read_file")),
            )
            .await;
        assert_eq!(report.firings.len(), 1);

        let report = engine
            .run(HookEvent::PreToolUse, &context(&dir, None))
            .await;
        assert!(report.firings.is_empty());

        let mut star_hook = entry(HookEvent::PreToolUse, "exit 0");
        star_hook.matcher = Some("*".to_string());
        let star = HookEngine::new(vec![star_hook]);
        let report = star.run(HookEvent::PreToolUse, &context(&dir, None)).await;
        assert_eq!(report.firings.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn if_pattern_filters_tool_input() {
        let dir = temp_dir("if_pattern");
        fs::create_dir_all(&dir).unwrap();
        let mut hook = entry(HookEvent::PostToolUse, "exit 0");
        hook.if_pattern = Some("src/main.rs".to_string());
        let engine = HookEngine::new(vec![hook]);

        let matching = HookContext {
            tool_input: Some("path=src/main.rs"),
            ..context(&dir, Some("code-edit_file"))
        };
        let report = engine.run(HookEvent::PostToolUse, &matching).await;
        assert_eq!(report.firings.len(), 1);

        let other = HookContext {
            tool_input: Some("path=README.md"),
            ..context(&dir, Some("code-edit_file"))
        };
        let report = engine.run(HookEvent::PostToolUse, &other).await;
        assert!(report.firings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn payload_reaches_stdin() {
        let dir = temp_dir("payload");
        fs::create_dir_all(&dir).unwrap();
        let turn = TurnId::from_raw("turn-test");
        let ctx = HookContext {
            session_id: "ses-test",
            turn_id: Some(&turn),
            tool_name: Some("code-read_file"),
            tool_input: Some(r#"{"path":"a.rs"}"#),
            tool_output: Some("ok"),
            prompt: Some("fix it"),
            cwd: &dir,
        };
        let report = HookEngine::new(vec![entry(HookEvent::PostToolUse, "cat > payload.json")])
            .run(HookEvent::PostToolUse, &ctx)
            .await;
        assert_eq!(report.decision, HookDecision::Pass);

        let raw = fs::read_to_string(dir.join("payload.json")).unwrap();
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(payload["event"], "PostToolUse");
        assert_eq!(payload["session_id"], "ses-test");
        assert_eq!(payload["turn_id"], "turn-test");
        assert_eq!(payload["tool_name"], "code-read_file");
        assert_eq!(payload["tool_input"], r#"{"path":"a.rs"}"#);
        assert_eq!(payload["tool_output"], "ok");
        assert_eq!(payload["prompt"], "fix it");
        assert_eq!(payload["cwd"], dir.to_string_lossy().as_ref());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_block_decision() {
        let dir = temp_dir("stop_block");
        fs::create_dir_all(&dir).unwrap();
        let report = HookEngine::new(vec![entry(
            HookEvent::Stop,
            r#"printf '{"decision":"block","reason":"tests fail"}'"#,
        )])
        .run(HookEvent::Stop, &context(&dir, None))
        .await;
        assert_eq!(report.decision, HookDecision::Block);
        assert_eq!(report.reason(), Some("tests fail"));

        let report = HookEngine::new(vec![entry(
            HookEvent::SessionStart,
            r#"printf '{"hookSpecificOutput":{"permissionDecision":"deny"}}'"#,
        )])
        .run(HookEvent::SessionStart, &context(&dir, None))
        .await;
        assert_eq!(report.decision, HookDecision::Pass);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn deny_outranks_block() {
        let dir = temp_dir("combine");
        fs::create_dir_all(&dir).unwrap();
        let engine = HookEngine::new(vec![
            entry(
                HookEvent::PreToolUse,
                r#"printf '{"hookSpecificOutput":{"permissionDecision":"deny","reason":"nope"}}'"#,
            ),
            entry(HookEvent::PreToolUse, "printf 'blocked' >&2; exit 2"),
        ]);
        let report = engine
            .run(HookEvent::PreToolUse, &context(&dir, Some("code-shell")))
            .await;
        assert_eq!(report.decision, HookDecision::Deny);
        assert_eq!(report.firings.len(), 2);
        assert_eq!(report.firings[0].decision, HookDecision::Deny);
        assert_eq!(report.firings[1].decision, HookDecision::Block);
        assert_eq!(report.reason(), Some("nope"));
        let _ = fs::remove_dir_all(&dir);
    }
}
