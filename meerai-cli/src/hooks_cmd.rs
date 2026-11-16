use std::path::Path;

use anyhow::Result;
use meerai_common::hooks::{
    HookEntry, HookEvent, MAX_HOOK_TIMEOUT_MS, add_trusted_project, append_user_hook,
    project_is_trusted, remove_trusted_project,
};
use meerai_common::settings::{AppSettings, HooksSettings, save_settings_to};

use crate::cli::HooksCommand;
use crate::env_flag;

pub(crate) fn run_hooks(
    command: HooksCommand,
    config_dir: Option<&Path>,
    project_root: &Path,
    user_settings: &mut AppSettings,
) -> Result<()> {
    match command {
        HooksCommand::Trust => {
            let dir = config_dir.ok_or_else(|| {
                anyhow::anyhow!("cannot save hook trust: no config directory available")
            })?;
            let hooks = user_settings
                .hooks
                .get_or_insert_with(HooksSettings::default);
            add_trusted_project(&mut hooks.trusted_projects, project_root);
            save_settings_to(dir, user_settings)?;
            println!("trusted project hooks: {}", project_root.display());
        }
        HooksCommand::Untrust => {
            let dir = config_dir.ok_or_else(|| {
                anyhow::anyhow!("cannot change hook trust: no config directory available")
            })?;
            if let Some(hooks) = user_settings.hooks.as_mut() {
                remove_trusted_project(&mut hooks.trusted_projects, project_root);
                save_settings_to(dir, user_settings)?;
            }
            println!("untrusted project hooks: {}", project_root.display());
        }
        HooksCommand::Status => {
            let hooks = user_settings.hooks.clone().unwrap_or_default();
            let enabled = hooks.enabled.unwrap_or(true);
            let trusted = hooks.trust_project == Some(true)
                || project_is_trusted(&hooks.trusted_projects, project_root);
            let env_override = env_flag("MEERAI_TRUST_PROJECT_HOOKS");
            println!("hooks: {}", if enabled { "enabled" } else { "disabled" });
            println!(
                "project hooks: {} ({})",
                project_root.display(),
                if trusted { "trusted" } else { "untrusted" }
            );
            println!("env override: {}", if env_override { "on" } else { "off" });
        }
        HooksCommand::Verify {
            command,
            reset_baseline,
        } => {
            let dir = config_dir.ok_or_else(|| {
                anyhow::anyhow!("cannot install verify hook: no config directory available")
            })?;
            let baseline = verify_baseline_path(dir, project_root);
            if reset_baseline {
                let _ = std::fs::remove_file(&baseline);
                println!("reset verify baseline: {}", baseline.display());
                return Ok(());
            }
            let command = command
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .or_else(|| detect_verify_command(project_root))
                .ok_or_else(|| anyhow::anyhow!("no verify command found; pass --command <CMD>"))?;
            let installed = append_user_hook(dir, verify_stop_hook(&command, baseline.clone()))?;
            if installed {
                println!("installed Stop verify hook: {command}");
            } else {
                println!("verify hook already installed: {command}");
            }
            println!("verify baseline: {}", baseline.display());
        }
    }
    Ok(())
}

fn verify_stop_hook(command: &str, baseline_file: std::path::PathBuf) -> HookEntry {
    HookEntry {
        event: HookEvent::Stop,
        matcher: None,
        if_pattern: None,
        command: command.to_string(),
        baseline_file: Some(baseline_file),
        timeout_ms: Some(MAX_HOOK_TIMEOUT_MS),
        fail_closed: Some(false),
    }
}

fn verify_baseline_path(config_dir: &Path, project_root: &Path) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    root.hash(&mut hasher);
    config_dir
        .join("verify-baselines")
        .join(format!("{:016x}.txt", hasher.finish()))
}

fn detect_verify_command(project_root: &Path) -> Option<String> {
    if project_root.join("Cargo.toml").exists() {
        Some("cargo test".to_string())
    } else if project_root.join("package.json").exists() {
        Some("npm test".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("meerai_hooks_cmd_{name}_{nanos}"))
    }

    #[test]
    fn verify_stop_hook_uses_the_command_and_baseline() {
        let baseline = PathBuf::from("/tmp/meerai-verify-baseline.txt");
        let hook = verify_stop_hook("cargo test", baseline.clone());
        assert_eq!(hook.event, HookEvent::Stop);
        assert_eq!(hook.command, "cargo test");
        assert_eq!(hook.baseline_file, Some(baseline));
        assert_eq!(hook.timeout_ms, Some(MAX_HOOK_TIMEOUT_MS));
        assert_eq!(hook.fail_closed, Some(false));
    }

    #[test]
    fn verify_baseline_path_is_stable_and_project_scoped() {
        let dir = temp_dir("baseline_path");
        fs::create_dir_all(&dir).unwrap();
        let first = verify_baseline_path(&dir, &dir);
        let second = verify_baseline_path(&dir, &dir);
        assert_eq!(first, second);
        assert!(first.starts_with(dir.join("verify-baselines")));
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_verify_command_prefers_cargo() {
        let dir = temp_dir("detect");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect_verify_command(&dir), None);
        fs::write(dir.join("package.json"), "{}").unwrap();
        assert_eq!(detect_verify_command(&dir).as_deref(), Some("npm test"));
        fs::write(dir.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_verify_command(&dir).as_deref(), Some("cargo test"));
        let _ = fs::remove_dir_all(&dir);
    }
}
