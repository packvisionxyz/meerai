use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEvent {
    #[serde(rename = "SessionStart")]
    SessionStart,
    #[serde(rename = "UserPromptSubmit")]
    UserPromptSubmit,
    #[serde(rename = "PreToolUse")]
    PreToolUse,
    #[serde(rename = "PermissionRequest")]
    PermissionRequest,
    #[serde(rename = "PostToolUse")]
    PostToolUse,
    #[serde(rename = "PostToolUseFailure")]
    PostToolUseFailure,
    #[serde(rename = "Stop")]
    Stop,
    #[serde(rename = "PreCompact")]
    PreCompact,
    #[serde(rename = "PostCompact")]
    PostCompact,
    #[serde(rename = "SubagentStart")]
    SubagentStart,
    #[serde(rename = "SubagentStop")]
    SubagentStop,
    #[serde(rename = "SessionEnd")]
    SessionEnd,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEntry {
    pub event: HookEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_pattern: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_closed: Option<bool>,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PermissionRequest => "PermissionRequest",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::Stop => "Stop",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::PostCompact => "PostCompact",
            HookEvent::SubagentStart => "SubagentStart",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::SessionEnd => "SessionEnd",
        }
    }
}

fn project_trust_key(project_root: &Path) -> String {
    std::fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string()
}

pub fn project_is_trusted(trusted: &[String], project_root: &Path) -> bool {
    let key = project_trust_key(project_root);
    trusted.iter().any(|entry| entry == &key)
}

pub fn add_trusted_project(trusted: &mut Vec<String>, project_root: &Path) -> bool {
    let key = project_trust_key(project_root);
    if trusted.iter().any(|entry| entry == &key) {
        return false;
    }
    trusted.push(key);
    true
}

pub fn remove_trusted_project(trusted: &mut Vec<String>, project_root: &Path) -> bool {
    let key = project_trust_key(project_root);
    let before = trusted.len();
    trusted.retain(|entry| entry != &key);
    trusted.len() != before
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HookDocument {
    List(Vec<HookEntry>),
    Wrapped { hooks: Vec<HookEntry> },
}

pub const MAX_HOOK_TIMEOUT_MS: u64 = 60_000;

fn env_trusts_project_hooks() -> bool {
    std::env::var("MEERAI_TRUST_PROJECT_HOOKS")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|flag| matches!(flag.as_str(), "1" | "true" | "yes"))
}

pub fn load_hooks_for_run(
    config_dir: Option<&Path>,
    project_root: &Path,
    settings: Option<&crate::settings::HooksSettings>,
) -> (Vec<HookEntry>, Vec<String>) {
    if settings.and_then(|hooks| hooks.enabled) == Some(false) {
        return (Vec::new(), Vec::new());
    }
    let trusted = env_trusts_project_hooks()
        || settings.and_then(|hooks| hooks.trust_project) == Some(true)
        || settings.is_some_and(|hooks| project_is_trusted(&hooks.trusted_projects, project_root));
    let project_path = project_root.join(".meerai").join("hooks.json");
    let mut warnings = Vec::new();
    let entries = match config_dir {
        Some(dir) => load_hooks(
            &dir.join("hooks.json"),
            trusted.then_some(project_path.as_path()),
            &mut warnings,
        ),
        None if trusted => load_hooks(&project_path, None, &mut warnings),
        None => Vec::new(),
    };
    (entries, warnings)
}

pub fn load_hooks(
    user_path: &Path,
    project_path: Option<&Path>,
    warnings: &mut Vec<String>,
) -> Vec<HookEntry> {
    let mut entries = read_entries(user_path, warnings);
    if let Some(path) = project_path {
        entries.extend(read_entries(path, warnings));
    }
    entries
}

fn read_entries(path: &Path, warnings: &mut Vec<String>) -> Vec<HookEntry> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(format!("ignoring hooks at {}: {error}", path.display()));
            return Vec::new();
        }
    };
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let entries = match serde_yaml::from_str::<HookDocument>(&raw) {
        Ok(HookDocument::List(entries)) | Ok(HookDocument::Wrapped { hooks: entries }) => entries,
        Err(error) => {
            warnings.push(format!("ignoring hooks at {}: {error}", path.display()));
            return Vec::new();
        }
    };

    let mut kept = Vec::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        if entry.command.trim().is_empty() {
            warnings.push(format!(
                "ignoring hooks at {}: entry {} has a blank command",
                path.display(),
                index + 1
            ));
            continue;
        }
        if let Some(timeout) = entry.timeout_ms
            && (timeout == 0 || timeout > MAX_HOOK_TIMEOUT_MS)
        {
            warnings.push(format!(
                "hooks at {}: entry {} timeout_ms {timeout} is outside 1..={MAX_HOOK_TIMEOUT_MS}; using the default",
                path.display(),
                index + 1
            ));
            kept.push(HookEntry {
                timeout_ms: None,
                ..entry
            });
            continue;
        }
        kept.push(entry);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::HooksSettings;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(value: &str) -> Self {
            let prev = std::env::var("MEERAI_TRUST_PROJECT_HOOKS").ok();
            unsafe {
                std::env::set_var("MEERAI_TRUST_PROJECT_HOOKS", value);
            }
            Self { prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(value) => std::env::set_var("MEERAI_TRUST_PROJECT_HOOKS", value),
                    None => std::env::remove_var("MEERAI_TRUST_PROJECT_HOOKS"),
                }
            }
        }
    }

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "meerai_common_hooks_{name}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    fn write_hooks(dir: &Path, name: &str, raw: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, raw).unwrap();
        path
    }

    #[test]
    fn parses_list_and_wrapped_documents() {
        let dir = temp_dir("list");
        let list = write_hooks(
            &dir,
            "hooks.json",
            r#"[
                {
                    "event": "PreToolUse",
                    "matcher": "code-*",
                    "if": "src/**",
                    "command": "check.sh",
                    "timeout_ms": 5000,
                    "fail_closed": true
                },
                { "event": "SessionEnd", "command": "notify.sh" }
            ]"#,
        );
        let mut warnings = Vec::new();
        let entries = load_hooks(&list, None, &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].event, HookEvent::PreToolUse);
        assert_eq!(entries[0].event.as_str(), "PreToolUse");
        assert_eq!(entries[0].matcher.as_deref(), Some("code-*"));
        assert_eq!(entries[0].if_pattern.as_deref(), Some("src/**"));
        assert_eq!(entries[0].command, "check.sh");
        assert_eq!(entries[0].timeout_ms, Some(5000));
        assert_eq!(entries[0].fail_closed, Some(true));
        assert_eq!(entries[1].event, HookEvent::SessionEnd);
        assert_eq!(entries[1].if_pattern, None);

        let wrapped = write_hooks(
            &dir,
            "wrapped.json",
            r#"{ "hooks": [ { "event": "PostToolUse", "command": "fmt.sh" } ] }"#,
        );
        let mut warnings = Vec::new();
        let entries = load_hooks(&wrapped, None, &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].event, HookEvent::PostToolUse);
        assert_eq!(entries[0].command, "fmt.sh");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_json_warns_and_returns_no_entries() {
        let dir = temp_dir("invalid");
        let path = write_hooks(&dir, "hooks.json", r#"{ "hooks": [ }"#);
        let mut warnings = Vec::new();
        let entries = load_hooks(&path, None, &mut warnings);
        assert!(entries.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("ignoring hooks at "));
        assert!(warnings[0].contains(&path.display().to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_empty_without_warning() {
        let dir = temp_dir("missing");
        let path = dir.join("hooks.json");
        let mut warnings = Vec::new();
        let entries = load_hooks(&path, Some(&dir.join("project.json")), &mut warnings);
        assert!(entries.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn blank_command_is_skipped_with_warning() {
        let dir = temp_dir("blank");
        let path = write_hooks(
            &dir,
            "hooks.json",
            r#"[ { "event": "Stop", "command": "   " }, { "event": "Stop", "command": "ok.sh" } ]"#,
        );
        let mut warnings = Vec::new();
        let entries = load_hooks(&path, None, &mut warnings);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "ok.sh");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains(&path.display().to_string()));
        assert!(warnings[0].contains("blank command"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_entries_precede_project_entries() {
        let dir = temp_dir("order");
        let user = write_hooks(
            &dir,
            "user.json",
            r#"[ { "event": "SessionStart", "command": "user.sh" } ]"#,
        );
        let project = write_hooks(
            &dir,
            "project.json",
            r#"[ { "event": "SessionStart", "command": "project.sh" } ]"#,
        );
        let mut warnings = Vec::new();
        let entries = load_hooks(&user, Some(&project), &mut warnings);
        assert!(warnings.is_empty());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command, "user.sh");
        assert_eq!(entries[1].command, "project.sh");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trusted_project_keys_are_canonical() {
        let dir = temp_dir("trusted_canonical");
        fs::create_dir_all(&dir).unwrap();
        let canonical = fs::canonicalize(&dir).unwrap().display().to_string();

        let mut trusted = Vec::new();
        assert!(!project_is_trusted(&trusted, &dir));
        assert!(add_trusted_project(&mut trusted, &dir));
        assert_eq!(trusted, vec![canonical]);
        assert!(project_is_trusted(&trusted, &dir));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trusting_a_project_twice_is_idempotent() {
        let dir = temp_dir("trusted_duplicate");
        fs::create_dir_all(&dir).unwrap();

        let mut trusted = Vec::new();
        assert!(add_trusted_project(&mut trusted, &dir));
        assert!(!add_trusted_project(&mut trusted, &dir));
        assert_eq!(trusted.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_trust_reports_whether_it_changed() {
        let dir = temp_dir("trusted_remove");
        fs::create_dir_all(&dir).unwrap();

        let mut trusted = Vec::new();
        assert!(!remove_trusted_project(&mut trusted, &dir));
        assert!(add_trusted_project(&mut trusted, &dir));
        assert!(remove_trusted_project(&mut trusted, &dir));
        assert!(trusted.is_empty());
        assert!(!remove_trusted_project(&mut trusted, &dir));
        assert!(!project_is_trusted(&trusted, &dir));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_hooks_setting_loads_nothing() {
        let _lock = env_lock();
        let dir = temp_dir("disabled");
        write_hooks(
            &dir,
            "hooks.json",
            r#"[ { "event": "Stop", "command": "stop.sh" } ]"#,
        );
        let settings = HooksSettings {
            enabled: Some(false),
            ..HooksSettings::default()
        };

        let (entries, warnings) = load_hooks_for_run(Some(&dir), &dir, Some(&settings));

        assert!(entries.is_empty());
        assert!(warnings.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_flag_trusts_project_hooks() {
        let _lock = env_lock();
        let dir = temp_dir("env_trust");
        write_hooks(
            &dir.join(".meerai"),
            "hooks.json",
            r#"[ { "event": "Stop", "command": "project.sh" } ]"#,
        );
        let _guard = EnvGuard::set("1");

        let (entries, warnings) = load_hooks_for_run(None, &dir, None);

        assert!(warnings.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "project.sh");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trusted_projects_list_loads_project_hooks() {
        let _lock = env_lock();
        let dir = temp_dir("trusted_list");
        write_hooks(
            &dir.join(".meerai"),
            "hooks.json",
            r#"[ { "event": "Stop", "command": "project.sh" } ]"#,
        );
        let mut trusted = Vec::new();
        assert!(add_trusted_project(&mut trusted, &dir));
        let settings = HooksSettings {
            trusted_projects: trusted,
            ..HooksSettings::default()
        };

        let (entries, warnings) = load_hooks_for_run(None, &dir, Some(&settings));

        assert!(warnings.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "project.sh");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_timeouts_are_reset_with_warnings() {
        let dir = temp_dir("timeout");
        let path = write_hooks(
            &dir,
            "hooks.json",
            r#"[
                { "event": "Stop", "command": "zero.sh", "timeout_ms": 0 },
                { "event": "Stop", "command": "large.sh", "timeout_ms": 60001 },
                { "event": "Stop", "command": "limit.sh", "timeout_ms": 60000 },
                { "event": "Stop", "command": "default.sh" }
            ]"#,
        );
        let mut warnings = Vec::new();

        let entries = load_hooks(&path, None, &mut warnings);

        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].timeout_ms, None);
        assert_eq!(entries[1].timeout_ms, None);
        assert_eq!(entries[2].timeout_ms, Some(60_000));
        assert_eq!(entries[3].timeout_ms, None);
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .all(|warning| warning.contains("timeout_ms"))
        );
        assert!(warnings.iter().all(|warning| warning.contains("60000")));

        let _ = fs::remove_dir_all(&dir);
    }
}
