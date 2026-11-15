use meerai_agents::AgentCommand;
use meerai_common::definitions::load_definitions;
use meerai_common::settings::{
    find_project_root, load_project_settings, load_settings_from, merge_project_settings,
};
use meerai_policy::rules_from_yaml;

use super::*;

impl AppState {
    pub(crate) async fn perform_reload(&mut self) {
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

        let definitions = load_definitions(&project_root);
        warnings.extend(definitions.warnings.clone());
        self.agents = definitions.agents.clone();
        self.skills = definitions.skills.clone();
        self.settings = user;

        let rules = merged
            .permissions
            .as_ref()
            .and_then(|value| rules_from_yaml(value).ok());
        let base =
            meerai_agents::compose_system_prompt(&self.launch_system, &self.workspace_root).await;
        let system_prompt = format!("{base}{}", self.launch_definitions);
        for tab in &mut self.tabs {
            let _ = tab.cmd_tx.send(AgentCommand::Reload {
                rules: rules.clone(),
                context: merged.context.clone(),
                system_prompt: system_prompt.clone(),
            });
        }

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
