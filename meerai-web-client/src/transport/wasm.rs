use std::cell::RefCell;
use std::rc::Rc;

use crate::store::StoreHandle;
use meerai_web_protocol::{
    ApiError, ApplyDefinitionRequest, ApprovalDecisionDto, ApprovalDecisionRequest,
    ApprovalModeDto, BackendKind, ChatGptLoginPasteRequest, ChatGptLoginStartResponse,
    CreateSessionRequest, CreateSessionResponse, FileChangeDto, FileDiffResponse,
    ForkSessionRequest, GrantScopeDto, ProvidersResponse, QuestionAnswerDto, QuestionAnswerRequest,
    ReloadResponse, RewindRequest, RewindResponse, RewindScopeDto, RunShellRequest,
    RunShellResponse, SearchModeDto, SessionDetailDto, SessionEventsResponse,
    SessionHistoryResponse, SetApiKeyRequest, SetModeRequest, SetModelRequest,
    SetProviderBaseUrlRequest, SnapshotResponse, SubmitPromptRequest, UpdateSettingsRequest,
    ViewEvent, WorkspaceFilesResponse, WorkspacePreviewResponse, WorkspaceSearchResponse,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::JsValue;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    ErrorEvent, EventSource, Headers, MessageEvent, Request, RequestCredentials, RequestInit,
    Response,
};

use crate::state::{
    ActiveModal, AppState, ConnectionState, apply_active_detail, apply_created_session,
    apply_history_response, apply_providers_response, apply_reload_response, apply_shell_output,
    apply_snapshot, apply_view_event,
};

struct EventSourceSlot {
    source: Option<EventSource>,
    _on_message: Option<Closure<dyn FnMut(MessageEvent)>>,
    _on_error: Option<Closure<dyn FnMut(ErrorEvent)>>,
}

#[derive(Clone)]
pub struct WasmTransport {
    handle: StoreHandle,
    slot: Rc<RefCell<EventSourceSlot>>,
}

impl WasmTransport {
    pub fn connect(handle: StoreHandle) -> Self {
        let slot = Rc::new(RefCell::new(EventSourceSlot {
            source: None,
            _on_message: None,
            _on_error: None,
        }));
        let transport = Self {
            handle: handle.clone(),
            slot: slot.clone(),
        };
        spawn_bootstrap(handle, slot, true);
        transport
    }

    pub fn resync(&self) {
        spawn_bootstrap(self.handle.clone(), self.slot.clone(), true);
    }

    pub fn post_empty(&self, path: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            match fetch_text("POST", &path, Some("{}")).await {
                Ok((200..=299, _)) => {}
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn submit_prompt(&self, session_id: String, prompt: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/prompt");
            let payload = match serde_json::to_string(&SubmitPromptRequest { prompt }) {
                Ok(json) => json,
                Err(err) => {
                    handle.update(move |state| {
                        state.sending = false;
                        state.error_banner = Some(format!("Failed to encode prompt: {err}"));
                    });
                    return;
                }
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {
                    handle.update_with_effects(|state, tx| {
                        state.sending = false;
                        tx.scroll_to_bottom("messages");
                    });
                }
                Ok((status, body)) => {
                    let message = format_api_error(status, &body);
                    handle.update(move |state| {
                        state.sending = false;
                        state.error_banner = Some(message);
                    });
                }
                Err(err) => {
                    handle.update(move |state| {
                        state.sending = false;
                        state.error_banner = Some(err);
                    });
                }
            }
        });
    }

    pub fn steer_prompt(&self, session_id: String, prompt: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/steer");
            let Ok(payload) = serde_json::to_string(&SubmitPromptRequest { prompt }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {
                    handle.effect(crate::text::UiEffect::ScrollToBottom("messages".to_owned()));
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn create_session(&self, backend: Option<BackendKind>) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let payload = serde_json::to_string(&CreateSessionRequest { backend })
                .unwrap_or_else(|_| "{}".to_owned());
            match fetch_text("POST", "/api/sessions", Some(&payload)).await {
                Ok((200..=299, body)) => match serde_json::from_str::<CreateSessionResponse>(&body)
                {
                    Ok(created) => {
                        handle.update_with_effects(move |state, tx| {
                            apply_created_session(state, created.session);
                            sync_title(state, tx);
                        });
                    }
                    Err(err) => {
                        set_error(&handle, format!("Invalid create-session response: {err}"))
                    }
                },
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn open_session(&self, session_id: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/open");
            match fetch_text("POST", &path, Some("{}")).await {
                Ok((200..=299, body)) => match serde_json::from_str::<SessionDetailDto>(&body) {
                    Ok(detail) => {
                        let id = detail.summary.id.clone();
                        handle.update_with_effects(move |state, tx| {
                            state.hidden_session_ids.remove(&id);
                            apply_active_detail(state, detail);
                            state.active_session_id = Some(id);
                            state.modal = None;
                            sync_title(state, tx);
                            tx.scroll_to_bottom("messages");
                        });
                    }
                    Err(err) => set_error(&handle, format!("Invalid open-session response: {err}")),
                },
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn delete_session(&self, session_id: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}");
            match fetch_text("DELETE", &path, Some("{}")).await {
                Ok((200..=299, _)) => {
                    let deleted = session_id.clone();
                    handle.update(move |state| {
                        if let Some(ActiveModal::SessionSelect {
                            history,
                            confirm_delete,
                            ..
                        }) = &mut state.modal
                        {
                            history.retain(|s| s.id != deleted);
                            *confirm_delete = None;
                        }
                        state.status_notice = Some(format!("Deleted session {deleted}"));
                    });
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn fork_session(&self, session_id: String, turn_id: Option<String>) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/fork");
            let payload = serde_json::to_string(&ForkSessionRequest { turn_id })
                .unwrap_or_else(|_| "{}".to_owned());
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, body)) => match serde_json::from_str::<SessionDetailDto>(&body) {
                    Ok(detail) => {
                        let id = detail.summary.id.clone();
                        handle.update_with_effects(move |state, tx| {
                            state.hidden_session_ids.remove(&id);
                            apply_active_detail(state, detail);
                            state.active_session_id = Some(id);
                            state.modal = None;
                            sync_title(state, tx);
                            tx.scroll_to_bottom("messages");
                        });
                    }
                    Err(err) => set_error(&handle, format!("Invalid fork response: {err}")),
                },
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn fetch_history(&self) {
        let handle = self.handle.clone();
        spawn_local(async move {
            match fetch_text("GET", "/api/sessions/history", None).await {
                Ok((200..=299, body)) => {
                    if let Ok(history) = serde_json::from_str::<SessionHistoryResponse>(&body) {
                        handle.update(move |state| apply_history_response(state, history));
                    }
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn fetch_session_events(&self, session_id: String, for_overlay: bool) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/events");
            match fetch_text("GET", &path, None).await {
                Ok((200..=299, body)) => match serde_json::from_str::<SessionEventsResponse>(&body)
                {
                    Ok(resp) => {
                        handle.update(move |state| {
                            if let Some(tab) = state.active_tab_mut() {
                                tab.child_loading.remove(&session_id);
                                tab.child_sessions
                                    .insert(session_id.clone(), resp.entries.clone());
                            }
                            if for_overlay
                                && let Some(ActiveModal::SubagentOverlay {
                                    child_session_id,
                                    entries,
                                    usage,
                                    loading,
                                    error,
                                    ..
                                }) = &mut state.modal
                                && child_session_id.as_deref() == Some(session_id.as_str())
                            {
                                *entries = resp.entries;
                                *usage = Some(resp.usage);
                                *loading = false;
                                *error = None;
                            }
                        });
                    }
                    Err(err) => set_error(&handle, format!("Invalid events response: {err}")),
                },
                Ok((status, body)) => {
                    let msg = format_api_error(status, &body);
                    handle.update(move |state| {
                        if let Some(tab) = state.active_tab_mut() {
                            tab.child_loading.remove(&session_id);
                        }
                        if let Some(ActiveModal::SubagentOverlay { loading, error, .. }) =
                            &mut state.modal
                        {
                            *loading = false;
                            *error = Some(msg.clone());
                        } else {
                            state.error_banner = Some(msg);
                        }
                    });
                }
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn rewind_session(&self, session_id: String, turn_id: String, scope: RewindScopeDto) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/rewind");
            let Ok(payload) = serde_json::to_string(&RewindRequest { turn_id, scope }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, body)) => {
                    if let Ok(resp) = serde_json::from_str::<RewindResponse>(&body) {
                        handle.update(move |state| {
                            state.modal = None;
                            if let Some(prompt) = resp.restored_prompt
                                && state.draft.text.trim().is_empty()
                            {
                                state.draft.replace_text(prompt);
                            }
                        });
                    }
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn set_model(&self, session_id: String, provider: String, model: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/model");
            let Ok(payload) = serde_json::to_string(&SetModelRequest { provider, model }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {
                    handle.update_with_effects(|state, tx| {
                        state.modal = None;
                        sync_title(state, tx);
                    });
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn set_mode(&self, session_id: String, mode: ApprovalModeDto) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/mode");
            let Ok(payload) = serde_json::to_string(&SetModeRequest { mode }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {}
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn apply_agent(&self, session_id: String, name: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/apply-agent");
            let Ok(payload) = serde_json::to_string(&ApplyDefinitionRequest { name }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {}
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn apply_skill(&self, session_id: String, name: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/apply-skill");
            let Ok(payload) = serde_json::to_string(&ApplyDefinitionRequest { name }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {}
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn decide_approval(
        &self,
        session_id: String,
        request_id: String,
        decision: ApprovalDecisionDto,
        scope: Option<GrantScopeDto>,
    ) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/approvals/{request_id}");
            let Ok(payload) = serde_json::to_string(&ApprovalDecisionRequest { decision, scope })
            else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {
                    handle.update_with_effects(|state, tx| sync_title(state, tx));
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn answer_question(
        &self,
        session_id: String,
        request_id: String,
        answers: Vec<QuestionAnswerDto>,
        cancel: bool,
    ) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/questions/{request_id}");
            let Ok(payload) = serde_json::to_string(&QuestionAnswerRequest { answers, cancel })
            else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {
                    handle.update_with_effects(|state, tx| sync_title(state, tx));
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn run_shell(&self, session_id: String, command: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/sessions/{session_id}/shell");
            let Ok(payload) = serde_json::to_string(&RunShellRequest { command }) else {
                return;
            };
            let res = match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, body)) => match serde_json::from_str::<RunShellResponse>(&body) {
                    Ok(r) => Ok(r.output),
                    Err(err) => Err(format!("Invalid shell response: {err}")),
                },
                Ok((status, body)) => Err(format_api_error(status, &body)),
                Err(err) => Err(err),
            };
            handle.update(move |state| apply_shell_output(state, &session_id, res));
        });
    }

    pub fn fetch_workspace_files(&self) {
        let handle = self.handle.clone();
        spawn_local(async move {
            if let Ok((200..=299, body)) = fetch_text("GET", "/api/workspace/files", None).await
                && let Ok(resp) = serde_json::from_str::<WorkspaceFilesResponse>(&body)
            {
                handle.update(move |state| {
                    state.workspace_files = resp.files;
                    crate::state::update_completion(state);
                });
            }
        });
    }

    pub fn search_workspace(&self, mode: SearchModeDto, query: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let mode_str = match mode {
                SearchModeDto::Files => "files",
                SearchModeDto::Content => "content",
            };
            let url = format!(
                "/api/workspace/search?mode={mode_str}&q={}",
                encode_query_component(&query)
            );
            match fetch_text("GET", &url, None).await {
                Ok((200..=299, body)) => {
                    if let Ok(resp) = serde_json::from_str::<WorkspaceSearchResponse>(&body) {
                        handle.update(move |state| {
                            if let Some(ActiveModal::Search {
                                mode,
                                query,
                                results,
                                selected,
                                loading,
                                ..
                            }) = &mut state.modal
                                && resp.query == query.text.trim()
                                && resp.mode == *mode
                            {
                                *results = Some(resp);
                                *selected = 0;
                                *loading = false;
                            }
                        });
                    }
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn preview_workspace(&self, path: String, line: Option<usize>) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let mut url = format!(
                "/api/workspace/preview?path={}",
                encode_query_component(&path)
            );
            if let Some(l) = line {
                url.push_str(&format!("&line={l}"));
            }
            if let Ok((200..=299, body)) = fetch_text("GET", &url, None).await
                && let Ok(resp) = serde_json::from_str::<WorkspacePreviewResponse>(&body)
            {
                handle.update(move |state| {
                    if let Some(ActiveModal::Search {
                        preview,
                        results,
                        selected,
                        ..
                    }) = &mut state.modal
                    {
                        let expected = results
                            .as_ref()
                            .and_then(|r| r.matches.get(*selected))
                            .map(|m| m.path.clone());
                        let matches_selection = expected.is_none_or(|path| {
                            resp.title == path || resp.title.starts_with(&format!("{path}:"))
                        });
                        if matches_selection {
                            *preview = Some(resp);
                        }
                    }
                });
            }
        });
    }

    pub fn fetch_workspace_changes(&self) {
        let handle = self.handle.clone();
        spawn_local(async move {
            if let Ok((200..=299, body)) = fetch_text("GET", "/api/workspace/changes", None).await
                && let Ok(changes) = serde_json::from_str::<Vec<FileChangeDto>>(&body)
            {
                handle.update(move |state| {
                    state.file_changes = changes;
                });
            }
        });
    }

    pub fn fetch_workspace_diff(&self, path: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let url = format!("/api/workspace/diff?path={}", encode_query_component(&path));
            match fetch_text("GET", &url, None).await {
                Ok((200..=299, body)) => match serde_json::from_str::<FileDiffResponse>(&body) {
                    Ok(resp) => {
                        handle.update(move |state| {
                            if let Some(ActiveModal::DiffView {
                                diff,
                                loading,
                                error,
                                ..
                            }) = &mut state.modal
                            {
                                *diff = Some(resp);
                                *loading = false;
                                *error = None;
                            }
                        });
                    }
                    Err(err) => set_error(&handle, format!("Invalid diff response: {err}")),
                },
                Ok((status, body)) => {
                    let msg = format_api_error(status, &body);
                    handle.update(move |state| {
                        if let Some(ActiveModal::DiffView { loading, error, .. }) = &mut state.modal
                        {
                            *loading = false;
                            *error = Some(msg.clone());
                        } else {
                            state.error_banner = Some(msg);
                        }
                    });
                }
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn fetch_providers(&self) {
        let handle = self.handle.clone();
        spawn_local(async move {
            match fetch_text("GET", "/api/providers", None).await {
                Ok((200..=299, body)) => {
                    if let Ok(resp) = serde_json::from_str::<ProvidersResponse>(&body) {
                        handle.update(move |state| apply_providers_response(state, resp));
                    }
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn set_api_key(&self, provider_id: String, api_key: String, base_url: Option<String>) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let trimmed_key = api_key.trim().to_owned();
            if !trimmed_key.is_empty() {
                let path = format!("/api/providers/{provider_id}/api-key");
                let Ok(payload) = serde_json::to_string(&SetApiKeyRequest {
                    api_key: trimmed_key,
                }) else {
                    return;
                };
                match fetch_text("POST", &path, Some(&payload)).await {
                    Ok((200..=299, _)) => {}
                    Ok((status, body)) => {
                        let msg = format_api_error(status, &body);
                        handle.update(move |state| {
                            if let Some(ActiveModal::ApiKeyInput { error, busy, .. }) =
                                &mut state.modal
                            {
                                *busy = false;
                                *error = Some(msg.clone());
                            } else {
                                state.error_banner = Some(msg);
                            }
                        });
                        return;
                    }
                    Err(err) => {
                        set_error(&handle, err);
                        return;
                    }
                }
            }
            if let Some(url) = base_url {
                let clean = url.trim().to_owned();
                let path = format!("/api/providers/{provider_id}/base-url");
                let Ok(payload) = serde_json::to_string(&SetProviderBaseUrlRequest {
                    base_url: (!clean.is_empty()).then_some(clean),
                }) else {
                    return;
                };
                if let Ok((status, body)) = fetch_text("POST", &path, Some(&payload)).await
                    && !(200..=299).contains(&status)
                {
                    let msg = format_api_error(status, &body);
                    handle.update(move |state| {
                        if let Some(ActiveModal::ApiKeyInput { error, busy, .. }) = &mut state.modal
                        {
                            *busy = false;
                            *error = Some(msg.clone());
                        } else {
                            state.error_banner = Some(msg);
                        }
                    });
                    return;
                }
            }
            handle.update(move |state| {
                state.modal = Some(ActiveModal::ModelInput {
                    filter: crate::text::TextBuffer::new(""),
                    provider_id: Some(provider_id.clone()),
                    selected: 0,
                });
                state.status_notice = Some(format!("Updated {provider_id} settings"));
                if let Some(t) = crate::transport::current() {
                    t.fetch_providers();
                }
            });
        });
    }

    pub fn set_provider_base_url(&self, provider_id: String, base_url: Option<String>) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/providers/{provider_id}/base-url");
            let Ok(payload) = serde_json::to_string(&SetProviderBaseUrlRequest { base_url }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {
                    handle.update(|_| {
                        if let Some(t) = crate::transport::current() {
                            t.fetch_providers();
                        }
                    });
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn remove_credential(&self, provider_id: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/providers/{provider_id}/credential");
            match fetch_text("DELETE", &path, Some("{}")).await {
                Ok((200..=299, _)) => {
                    handle.update(move |state| {
                        state.modal = None;
                        state.status_notice =
                            Some(format!("Removed credentials for {provider_id}"));
                        if let Some(t) = crate::transport::current() {
                            t.fetch_providers();
                        }
                    });
                }
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn start_chatgpt_login(&self, provider_id: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/providers/{provider_id}/chatgpt/start");
            match fetch_text("POST", &path, Some("{}")).await {
                Ok((200..=299, body)) => {
                    match serde_json::from_str::<ChatGptLoginStartResponse>(&body) {
                        Ok(resp) => {
                            handle.update(move |state| {
                                if let Some(ActiveModal::ChatGptLogin {
                                    attempt_id,
                                    url,
                                    busy,
                                    error,
                                    ..
                                }) = &mut state.modal
                                {
                                    *attempt_id = Some(resp.attempt_id);
                                    *url = Some(resp.url);
                                    *busy = false;
                                    *error = None;
                                }
                            });
                        }
                        Err(err) => {
                            set_error(&handle, format!("Invalid OAuth start response: {err}"))
                        }
                    }
                }
                Ok((status, body)) => {
                    let msg = format_api_error(status, &body);
                    handle.update(move |state| {
                        if let Some(ActiveModal::ChatGptLogin { busy, error, .. }) =
                            &mut state.modal
                        {
                            *busy = false;
                            *error = Some(msg.clone());
                        } else {
                            state.error_banner = Some(msg);
                        }
                    });
                }
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn paste_chatgpt_login(&self, provider_id: String, attempt_id: String, input: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/providers/{provider_id}/chatgpt/{attempt_id}/paste");
            let Ok(payload) = serde_json::to_string(&ChatGptLoginPasteRequest { input }) else {
                return;
            };
            match fetch_text("POST", &path, Some(&payload)).await {
                Ok((200..=299, _)) => {}
                Ok((status, body)) => {
                    let msg = format_api_error(status, &body);
                    handle.update(move |state| {
                        if let Some(ActiveModal::ChatGptLogin { error, .. }) = &mut state.modal {
                            *error = Some(msg.clone());
                        } else {
                            state.error_banner = Some(msg);
                        }
                    });
                }
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn cancel_chatgpt_login(&self, provider_id: String, attempt_id: String) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let path = format!("/api/providers/{provider_id}/chatgpt/{attempt_id}/cancel");
            let _ = fetch_text("POST", &path, Some("{}")).await;
            handle.update(|state| {
                if matches!(&state.modal, Some(ActiveModal::ChatGptLogin { .. })) {
                    state.modal = None;
                }
            });
        });
    }

    pub fn update_settings(&self, req: UpdateSettingsRequest) {
        let handle = self.handle.clone();
        spawn_local(async move {
            let Ok(payload) = serde_json::to_string(&req) else {
                return;
            };
            match fetch_text("POST", "/api/settings", Some(&payload)).await {
                Ok((200..=299, body)) => match serde_json::from_str::<ReloadResponse>(&body) {
                    Ok(resp) => {
                        handle.update(move |state| {
                            apply_reload_response(state, resp);
                            if let Some(ActiveModal::Settings(form)) = &mut state.modal {
                                form.busy = false;
                                form.status = Some("Saved and reloaded settings.".to_owned());
                            }
                        });
                    }
                    Err(err) => set_error(&handle, format!("Invalid settings response: {err}")),
                },
                Ok((status, body)) => {
                    let msg = format_api_error(status, &body);
                    handle.update(move |state| {
                        if let Some(ActiveModal::Settings(form)) = &mut state.modal {
                            form.busy = false;
                            form.status = Some(msg.clone());
                        } else {
                            state.error_banner = Some(msg);
                        }
                    });
                }
                Err(err) => set_error(&handle, err),
            }
        });
    }

    pub fn reload_config(&self) {
        let handle = self.handle.clone();
        spawn_local(async move {
            match fetch_text("POST", "/api/reload", Some("{}")).await {
                Ok((200..=299, body)) => match serde_json::from_str::<ReloadResponse>(&body) {
                    Ok(resp) => {
                        handle.update(move |state| apply_reload_response(state, resp));
                    }
                    Err(err) => set_error(&handle, format!("Invalid reload response: {err}")),
                },
                Ok((status, body)) => set_error(&handle, format_api_error(status, &body)),
                Err(err) => set_error(&handle, err),
            }
        });
    }
}

fn spawn_bootstrap(handle: StoreHandle, slot: Rc<RefCell<EventSourceSlot>>, reopen_stream: bool) {
    spawn_local(async move {
        let mut cursor = None;
        match fetch_text("GET", "/api/snapshot", None).await {
            Ok((200..=299, body)) => match serde_json::from_str::<SnapshotResponse>(&body) {
                Ok(snapshot) => {
                    cursor = Some(snapshot.event_id);
                    handle.update_with_effects(move |state, tx| {
                        apply_snapshot(state, snapshot);
                        sync_title(state, tx);
                        tx.scroll_to_bottom("messages");
                    });
                }
                Err(err) => {
                    handle.update_with_effects(move |state, tx| {
                        state.connection = ConnectionState::Reconnecting;
                        state.error_banner = Some(format!("Invalid snapshot response: {err}"));
                        sync_title(state, tx);
                    });
                }
            },
            Ok((status, body)) => {
                let message = format_api_error(status, &body);
                handle.update_with_effects(move |state, tx| {
                    state.connection = ConnectionState::Disconnected;
                    state.error_banner = Some(message);
                    sync_title(state, tx);
                });
            }
            Err(err) => {
                handle.update_with_effects(move |state, tx| {
                    state.connection = ConnectionState::Reconnecting;
                    state.error_banner = Some(err);
                    sync_title(state, tx);
                });
            }
        }
        if reopen_stream {
            open_event_stream(handle, slot, cursor);
        }
    });
}

fn open_event_stream(
    handle: StoreHandle,
    slot: Rc<RefCell<EventSourceSlot>>,
    after_event_id: Option<u64>,
) {
    if let Some(prev) = slot.borrow_mut().source.take() {
        prev.close();
    }
    let url = match after_event_id {
        Some(id) if id > 0 => format!("/api/events?after={id}"),
        _ => "/api/events".to_owned(),
    };
    let source = match EventSource::new(&url) {
        Ok(source) => source,
        Err(err) => {
            let msg = format!("Failed to open event stream: {}", js_error(err));
            handle.update_with_effects(move |state, tx| {
                state.connection = ConnectionState::Disconnected;
                state.error_banner = Some(msg);
                sync_title(state, tx);
            });
            return;
        }
    };

    let msg_handle = handle.clone();
    let msg_slot = slot.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(raw) = event.data().as_string() else {
            return;
        };
        let Ok(view_event) = serde_json::from_str::<ViewEvent>(&raw) else {
            return;
        };
        let seq = event.last_event_id().parse::<u64>().ok();
        let is_disconnect = matches!(view_event, ViewEvent::HostDisconnected);
        let is_resync = matches!(view_event, ViewEvent::ResyncRequired);
        let should_scroll = matches!(
            view_event,
            ViewEvent::UserPromptSubmitted { .. }
                | ViewEvent::TextDelta { .. }
                | ViewEvent::EntryCompleted { .. }
        );
        if is_disconnect && let Some(es) = msg_slot.borrow_mut().source.take() {
            es.close();
        }
        let resync_handle = msg_handle.clone();
        let resync_slot = msg_slot.clone();
        msg_handle.update_with_effects(move |state, tx| {
            if let Some(id) = seq {
                state.last_event_id = id;
            }
            if !is_disconnect && !is_resync {
                state.connection = ConnectionState::Connected;
            }
            let _ = apply_view_event(state, view_event);
            sync_title(state, tx);
            if should_scroll {
                tx.scroll_to_bottom("messages");
            }
        });
        if is_resync {
            spawn_bootstrap(resync_handle, resync_slot, true);
        }
    });

    let err_handle = handle.clone();
    let err_slot = slot.clone();
    let on_error = Closure::<dyn FnMut(ErrorEvent)>::new(move |_: ErrorEvent| {
        let closed = err_slot
            .borrow()
            .source
            .as_ref()
            .is_some_and(|es| es.ready_state() == EventSource::CLOSED);
        err_handle.update_with_effects(move |state, tx| {
            if state.connection != ConnectionState::Disconnected {
                state.connection = if closed {
                    ConnectionState::Disconnected
                } else {
                    ConnectionState::Reconnecting
                };
            }
            sync_title(state, tx);
        });
    });

    source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    source.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    let mut guard = slot.borrow_mut();
    guard.source = Some(source);
    guard._on_message = Some(on_message);
    guard._on_error = Some(on_error);
}

fn sync_title(state: &mut AppState, tx: &mut crate::text::UiEffects) {
    let next = state.window_title();
    if state.last_title_sent != next {
        state.last_title_sent = next.clone();
        tx.set_title(next);
    }
}

fn set_error(handle: &StoreHandle, message: String) {
    handle.update(move |state| {
        state.error_banner = Some(message);
    });
}

async fn fetch_text(
    method: &str,
    url: &str,
    json_body: Option<&str>,
) -> Result<(u16, String), String> {
    let window = web_sys::window().ok_or_else(|| "Missing browser window".to_owned())?;
    let init = RequestInit::new();
    init.set_method(method);
    init.set_credentials(RequestCredentials::SameOrigin);

    if let Some(body) = json_body {
        let headers = Headers::new().map_err(js_error)?;
        headers
            .set("Content-Type", "application/json")
            .map_err(js_error)?;
        init.set_headers(&headers);
        init.set_body(&JsValue::from_str(body));
    }

    let request = Request::new_with_str_and_init(url, &init).map_err(js_error)?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(js_error)?;
    let resp: Response = resp_value.dyn_into().map_err(js_error)?;
    let status = resp.status();
    let text_promise = resp.text().map_err(js_error)?;
    let text_value = JsFuture::from(text_promise).await.map_err(js_error)?;
    Ok((status, text_value.as_string().unwrap_or_default()))
}

fn format_api_error(status: u16, body: &str) -> String {
    if let Ok(api_err) = serde_json::from_str::<ApiError>(body) {
        return api_err.message;
    }
    if body.trim().is_empty() {
        format!("Request failed with status {status}")
    } else {
        format!("Request failed ({status}): {body}")
    }
}

fn encode_query_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                let hi = char::from_digit((b >> 4) as u32, 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase();
                let lo = char::from_digit((b & 0x0f) as u32, 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase();
                out.push(hi);
                out.push(lo);
            }
        }
    }
    out
}

fn js_error(err: JsValue) -> String {
    err.as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&err)
                .ok()
                .and_then(|s| s.as_string())
        })
        .unwrap_or_else(|| "Browser request failed".to_owned())
}
