use meerai_agents::AgentCommand;
use meerai_common::definitions::{load_definitions_for, load_definitions_with_builtins};
use meerai_common::hooks::load_hooks_for_run;
use meerai_common::settings::{
    Temperature, find_project_root, load_project_settings, load_settings_from,
    merge_project_settings, open_store_for_config_dir,
};
use meerai_policy::rules_from_yaml;

use super::*;

impl AppState {
    pub(crate) async fn perform_reload(&mut self) {
        self.perform_reload_inner(true).await;
    }

    pub(crate) async fn perform_reload_quiet(&mut self) {
        self.perform_reload_inner(false).await;
    }

    async fn perform_reload_inner(&mut self, announce: bool) {
        let project_root = find_project_root(&self.workspace_root);
        let user = self
            .config_dir
            .as_deref()
            .and_then(|dir| load_settings_from(dir).ok())
            .unwrap_or_default();
        let mut merged = user.clone();
        let mut warnings = Vec::new();
        match load_project_settings(&project_root) {
            Ok(Some(project)) => {
                let (project_merged, project_warnings) =
                    merge_project_settings(user.clone(), project);
                merged = project_merged;
                warnings.extend(project_warnings);
            }
            Ok(None) => {}
            Err(err) => warnings.push(format!("ignoring project settings: {err:#}")),
        }

        let (palette, theme_warnings) =
            crate::theme::Palette::from_settings(&merged.theme.clone().unwrap_or_default());
        warnings.extend(theme_warnings);
        crate::theme::set(palette);

        let definitions = match open_store_for_config_dir(self.config_dir.as_deref()) {
            Ok(store) => load_definitions_for(&store, self.config_dir.as_deref(), &project_root),
            Err(_) => load_definitions_with_builtins(&project_root),
        };
        warnings.extend(definitions.warnings.clone());
        self.agents = definitions
            .agents
            .iter()
            .filter(|agent| agent.mode.selectable_primary())
            .cloned()
            .collect();
        self.skills = definitions.skills.clone();
        self.settings = user;
        self.context_temperature = merged
            .context
            .as_ref()
            .and_then(|context| context.temperature)
            .and_then(Temperature::effective);
        self.context_effort = merged.context.as_ref().and_then(|context| context.effort);
        self.explicit_context_window = merged
            .context
            .as_ref()
            .and_then(|context| context.context_window_tokens);

        let rules = merged
            .permissions
            .as_ref()
            .and_then(|value| rules_from_yaml(value).ok());
        let memory_bytes = merged
            .context
            .as_ref()
            .and_then(|context| context.memory_prompt_bytes)
            .unwrap_or(meerai_agents::DEFAULT_MEMORY_PROMPT_BYTES as u64)
            as usize;
        let memory_block = open_store_for_config_dir(self.config_dir.as_deref())
            .ok()
            .and_then(|store| {
                meerai_agents::load_memory_prompt(&store, &project_root, memory_bytes)
            });
        let base = meerai_agents::prompt::compose_system_prompt_with_memory(
            &self.launch_system,
            &self.workspace_root,
            memory_block.as_deref(),
        )
        .await;
        let system_prompt = format!("{base}{}", self.launch_definitions);
        for tab in &mut self.tabs {
            let _ = tab.cmd_tx.send(AgentCommand::Reload {
                rules: rules.clone(),
                context: merged.context.clone(),
                system_prompt: system_prompt.clone(),
            });
        }

        let (hook_entries, hook_warnings) = load_hooks_for_run(
            self.config_dir.as_deref(),
            &project_root,
            self.settings.hooks.as_ref(),
        );
        warnings.extend(
            hook_warnings
                .into_iter()
                .map(|warning| format!("hook config: {warning}")),
        );
        for tab in &mut self.tabs {
            let _ = tab
                .cmd_tx
                .send(AgentCommand::SetHooks(hook_entries.clone()));
        }

        if announce {
            let mut text = String::from(
                "Reloaded settings, theme, definitions, prompt, policy, and context budgets.",
            );
            if !warnings.is_empty() {
                text.push_str("\n\nWarnings:\n- ");
                text.push_str(&warnings.join("\n- "));
            }
            let tab = self.tab_mut();
            tab.auto_follow = true;
            tab.transcript.push(TranscriptEntry::System { text });
            self.status_notice = Some("Reloaded configuration".to_string());
        }
    }
}
