use std::path::Path;

use anyhow::Result;
use meerai_common::hooks::{add_trusted_project, project_is_trusted, remove_trusted_project};
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
    }
    Ok(())
}
