use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use meerai_agents::AgentCommand;
use meerai_agents::events::{ApprovalDecision, GrantScope, QuestionAnswer, RewindScope};
use meerai_client::providers::canonical_provider_id;
use meerai_common::settings::save_settings_to;

use super::text_input::{TextAction, handle_text_key};
use super::*;
use crate::diff::{build_file_diff, list_git_changes, stage_file, unstage_file};
use crate::search::{
    SearchMatch, SearchMode, SearchPreview, build_search_preview, search_content, search_files,
};

mod dispatch;
mod keys_chat;
mod keys_insight;
mod keys_misc;
mod keys_panels;
mod keys_provider;
mod palette;
mod provider;
mod search_diff;
mod session;

pub(crate) struct SearchSources<'a> {
    pub(super) workspace: &'a dyn crate::workspace::WorkspaceQuery,
    pub(super) workspace_root: &'a Path,
    pub(super) mode: SearchMode,
    pub(super) workspace_files: &'a [String],
    pub(super) query: &'a str,
}

pub(crate) struct SearchOutput<'a> {
    pub(super) results: &'a mut Vec<SearchMatch>,
    pub(super) selected: &'a mut usize,
    pub(super) preview: &'a mut Option<SearchPreview>,
}

fn collect_question_answers(
    request: &QuestionRequest,
    answers: &[Vec<usize>],
    custom: &[TextInput],
) -> Vec<QuestionAnswer> {
    request
        .questions
        .iter()
        .enumerate()
        .map(|(index, prompt)| {
            let mut selected: Vec<String> = answers[index]
                .iter()
                .filter_map(|i| prompt.options.get(*i))
                .map(|option| option.label.clone())
                .collect();
            let text = custom[index].trim();
            if !text.is_empty() {
                selected.push(text.to_string());
            }
            QuestionAnswer {
                header: prompt.header.clone(),
                selected,
            }
        })
        .collect()
}

fn queue_modal_host_action(app: &mut AppState, action: meerai_host::HostAction) {
    let tab_id = app.tab().tab_id;
    app.pending_session_actions
        .push(PendingSessionAction::HostAction { tab_id, action });
}

fn subagent_spec_from_panel(panel: &SubagentSpawnPanel) -> Option<meerai_agents::SubagentSpec> {
    let max_steps = parse_optional(&panel.steps)?;
    let max_tokens = parse_optional(&panel.tokens)?;
    let max_cost_usd = if panel.cost.trim().is_empty() {
        None
    } else {
        Some(panel.cost.trim().parse::<f64>().ok()?)
    };
    let allowed_tools = if panel.tools.trim().is_empty() {
        None
    } else {
        let tools: Vec<String> = panel
            .tools
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_string)
            .collect();
        (!tools.is_empty()).then_some(tools)
    };
    Some(meerai_agents::SubagentSpec {
        name: (!panel.name.trim().is_empty()).then(|| panel.name.trim().to_string()),
        agent: panel.agent.trim().to_string(),
        task: panel.task.trim().to_string(),
        model: (!panel.model.trim().is_empty()).then(|| panel.model.trim().to_string()),
        approval_mode: panel.mode,
        allowed_tools,
        max_steps,
        max_tokens,
        max_cost_usd,
    })
}

fn parse_optional<T: std::str::FromStr>(raw: &str) -> Option<Option<T>> {
    let raw = raw.trim();
    if raw.is_empty() {
        Some(None)
    } else {
        raw.parse().ok().map(Some)
    }
}

fn typing_effort_keyword(temperature: &str) -> bool {
    temperature
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
}
