use super::forms::*;
use super::misc::*;
use super::types::*;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspacePanelTab {
    Context,
    Loop,
    Memory,
    Subagent,
    Routing,
    Packs,
}

pub fn sync_pending_modal_for_active(state: &mut AppState) {
    if state.modal.is_some() {
        return;
    }
    reopen_pending_request(state);
}

pub(crate) fn adjust_queued_edit_modal(state: &mut AppState, session_id: &str, removed_idx: usize) {
    if state.active_session_id.as_deref() != Some(session_id) {
        return;
    }
    if let Some(ActiveModal::QueuedEdit { index, .. }) = &mut state.modal {
        if *index == removed_idx {
            state.modal = None;
            set_status_notice(state, "Queued message was removed or sent");
        } else if *index > removed_idx {
            *index -= 1;
        }
    }
}

pub fn open_workspace_panel(state: &mut AppState, tab: WorkspacePanelTab) {
    let context = state.active_tab().and_then(|tab| tab.context.clone());
    let loop_state = state.active_tab().and_then(|tab| tab.loop_state.clone());
    state.modal = Some(ActiveModal::WorkspacePanel {
        tab,
        context: context.map(Box::new),
        loop_state: loop_state.map(Box::new),
        memories: Vec::new(),
        routes: Vec::new(),
        packs: Vec::new(),
        definitions: Box::default(),
        selected: 0,
        loading: !matches!(tab, WorkspacePanelTab::Loop),
        error: None,
        subagent_name: Box::default(),
        subagent_agent: "explore".to_owned(),
        subagent_task: Box::default(),
        subagent_model: Box::default(),
        subagent_mode: None,
        subagent_field: 1,
        subagent_steps: Box::default(),
        subagent_tokens: Box::default(),
        subagent_cost: Box::default(),
        subagent_tools: Box::default(),
        route_input: Box::default(),
        pack_input: Box::default(),
        pack_pin_input: Box::default(),
        context_turn: 0,
        context_focus: Box::default(),
        context_blob: None,
    });
}

pub fn apply_blob(state: &mut AppState, key: String, content: String) {
    if let Some(ActiveModal::WorkspacePanel {
        context_blob,
        error,
        ..
    }) = state.modal.as_mut()
    {
        *context_blob = Some((key, content));
        *error = None;
    }
}

pub fn apply_memories(state: &mut AppState, response: meerai_web_protocol::MemoriesResponse) {
    if let Some(ActiveModal::WorkspacePanel {
        memories,
        selected,
        loading,
        error,
        ..
    }) = state.modal.as_mut()
    {
        *memories = response.memories;
        *selected = (*selected).min(memories.len().saturating_sub(1));
        *loading = false;
        *error = None;
    }
}

pub fn apply_routes(state: &mut AppState, response: meerai_web_protocol::RoutesResponse) {
    if let Some(ActiveModal::WorkspacePanel {
        routes,
        selected,
        loading,
        error,
        ..
    }) = state.modal.as_mut()
    {
        *routes = response.routes;
        *selected = (*selected).min(routes.len().saturating_sub(1));
        *loading = false;
        *error = None;
    }
}

pub fn apply_packs(state: &mut AppState, response: meerai_web_protocol::PacksResponse) {
    if let Some(ActiveModal::WorkspacePanel {
        packs,
        selected,
        loading,
        error,
        ..
    }) = state.modal.as_mut()
    {
        *packs = response.packs;
        *selected = (*selected).min(packs.len().saturating_sub(1));
        *loading = false;
        *error = None;
    }
}

pub fn apply_definitions(state: &mut AppState, definitions: DefinitionsDiagnosticsDto) {
    if let Some(ActiveModal::WorkspacePanel {
        definitions: slot,
        loading,
        error,
        ..
    }) = state.modal.as_mut()
    {
        **slot = definitions;
        *loading = false;
        *error = None;
    }
}

pub fn apply_panel_error(state: &mut AppState, message: String) {
    if let Some(ActiveModal::WorkspacePanel { error, loading, .. }) = state.modal.as_mut() {
        *error = Some(message);
        *loading = false;
    }
}

pub fn open_diff_modal(state: &mut AppState, path: Option<String>) {
    open_diff_modal_with_status(state, path, None);
}

pub fn open_diff_modal_with_status(
    state: &mut AppState,
    path: Option<String>,
    status: Option<String>,
) {
    let target = path.or_else(|| state.file_changes.first().map(|c| c.path.clone()));
    let resolved_status = status.or_else(|| {
        target.as_ref().and_then(|p| {
            state
                .file_changes
                .iter()
                .find(|c| &c.path == p)
                .map(|c| c.status.clone())
        })
    });
    if let Some(t) = crate::transport::current() {
        t.fetch_workspace_changes();
        if let Some(p) = &target {
            t.fetch_workspace_diff(p.clone(), resolved_status);
        }
    }
    state.modal = Some(ActiveModal::DiffView {
        loading: target.is_some(),
        selected_path: target,
        unified: true,
        diff: None,
        error: None,
    });
}

pub fn open_search_modal(
    state: &mut AppState,
    mode: meerai_web_protocol::SearchModeDto,
    query: &str,
) {
    if let Some(t) = crate::transport::current()
        && !query.is_empty()
    {
        t.search_workspace(mode, query.to_owned());
    }
    state.modal = Some(ActiveModal::Search {
        mode,
        query: TextBuffer::new(query),
        results: None,
        selected: 0,
        preview: None,
        loading: !query.is_empty(),
    });
}

pub fn open_rewind_modal(state: &mut AppState, turn_hint: Option<&str>) {
    let turns = state.user_turns();
    if turns.is_empty() {
        set_status_notice(state, "No recorded turns to rewind to.");
        return;
    }
    let selected_turn = turn_hint
        .and_then(|h| turns.iter().position(|(id, _)| id == h))
        .unwrap_or_else(|| turns.len().saturating_sub(1));
    state.modal = Some(ActiveModal::RewindMenu {
        turns,
        selected_turn,
        scope: RewindScopeDto::Both,
    });
}

pub fn open_api_key_modal(
    state: &mut AppState,
    provider: &ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    state.modal = Some(ActiveModal::ApiKeyInput {
        provider_id: provider.id.clone(),
        provider_name: provider.display_name.clone(),
        auth_kind: provider.auth_kind,
        status_badge: provider.status_badge.clone(),
        configured: provider.configured,
        env_vars: provider.env_vars.clone(),
        api_key: TextBuffer::new(""),
        base_url: TextBuffer::new(provider.base_url.clone().unwrap_or_default()),
        error: None,
        busy: false,
        return_to,
    });
}

pub fn start_oauth_modal(
    state: &mut AppState,
    provider: &ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    if let Some(t) = crate::transport::current() {
        t.start_chatgpt_login(provider.id.clone());
    }
    state.modal = Some(ActiveModal::ChatGptLogin {
        provider_id: provider.id.clone(),
        provider_name: provider.display_name.clone(),
        attempt_id: None,
        url: None,
        paste: TextBuffer::new(""),
        state: None,
        error: None,
        busy: true,
        return_to,
    });
}
