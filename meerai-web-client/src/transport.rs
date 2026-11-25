use crate::state::{ProviderModalReturn, SessionPanelAction};

#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(target_arch = "wasm32")]
use crate::store::StoreHandle;
use meerai_web_protocol::{
    ApprovalDecisionDto, ApprovalModeDto, BackendKind, GrantScopeDto, RewindScopeDto,
    SearchModeDto, UpdateSettingsRequest,
};

#[derive(Clone)]
pub struct TransportClient {
    #[cfg(target_arch = "wasm32")]
    inner: wasm::WasmTransport,
    #[cfg(not(target_arch = "wasm32"))]
    _phantom: (),
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static CURRENT: std::cell::RefCell<Option<TransportClient>> = const { std::cell::RefCell::new(None) };
}

pub fn current() -> Option<TransportClient> {
    #[cfg(target_arch = "wasm32")]
    {
        CURRENT.with(|current| current.borrow().clone())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

pub fn set_current(client: Option<TransportClient>) {
    #[cfg(target_arch = "wasm32")]
    CURRENT.with(|current| {
        *current.borrow_mut() = client;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = client;
}

impl TransportClient {
    #[cfg(target_arch = "wasm32")]
    pub fn connect(store: StoreHandle) -> Self {
        Self {
            inner: wasm::WasmTransport::connect(store),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect() -> Self {
        Self { _phantom: () }
    }

    pub fn run_host_action(&self, command: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.run_host_action(command);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = command;
    }

    pub fn fetch_memories(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_memories();
    }

    pub fn fetch_routes(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_routes();
    }

    pub fn fetch_packs(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_packs();
    }

    pub fn fetch_definitions(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_definitions();
    }

    pub fn fetch_blob(&self, session_id: String, key: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_blob(session_id, key);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, key);
    }

    pub fn resync(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.resync();
    }

    pub fn submit_prompt(&self, session_id: String, prompt: String, display: Option<String>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.submit_prompt(session_id, prompt, display);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, prompt, display);
    }

    pub fn steer_prompt(&self, session_id: String, prompt: String, display: Option<String>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.steer_prompt(session_id, prompt, display);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, prompt, display);
    }

    pub fn cancel_turn(&self, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .post_empty(format!("/api/sessions/{session_id}/cancel"));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = session_id;
    }

    pub fn compact_session(&self, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .post_empty(format!("/api/sessions/{session_id}/compact"));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = session_id;
    }

    pub fn reset_session(&self, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .post_empty(format!("/api/sessions/{session_id}/reset"));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = session_id;
    }

    pub fn create_session(&self, backend: Option<BackendKind>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.create_session(backend);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = backend;
    }

    pub fn fetch_session_detail(&self, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_session_detail(session_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = session_id;
    }

    pub fn activate_session(&self, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .post_empty(format!("/api/sessions/{session_id}/activate"));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = session_id;
    }

    pub fn close_session(&self, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .post_empty(format!("/api/sessions/{session_id}/close"));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = session_id;
    }

    pub fn session_panel_action(&self, action: SessionPanelAction, session_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.session_panel_action(action, session_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (action, session_id);
    }

    pub fn fork_session(&self, session_id: String, turn_id: Option<String>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fork_session(session_id, turn_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, turn_id);
    }

    pub fn btw_session(&self, session_id: String, text: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.btw_session(session_id, text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, text);
    }

    pub fn fetch_history(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_history();
    }

    pub fn fetch_session_events(&self, session_id: String, for_overlay: bool) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_session_events(session_id, for_overlay);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, for_overlay);
    }

    pub fn rewind_session(&self, session_id: String, turn_id: String, scope: RewindScopeDto) {
        #[cfg(target_arch = "wasm32")]
        self.inner.rewind_session(session_id, turn_id, scope);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, turn_id, scope);
    }

    pub fn set_model(&self, session_id: String, provider: String, model: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.set_model(session_id, provider, model);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, provider, model);
    }

    pub fn set_mode(&self, session_id: String, mode: ApprovalModeDto) {
        #[cfg(target_arch = "wasm32")]
        self.inner.set_mode(session_id, mode);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, mode);
    }

    pub fn apply_agent(&self, session_id: String, name: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.apply_agent(session_id, name);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, name);
    }

    pub fn apply_skill(&self, session_id: String, name: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.apply_skill(session_id, name);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, name);
    }

    pub fn decide_approval(
        &self,
        session_id: String,
        request_id: String,
        decision: ApprovalDecisionDto,
        scope: Option<GrantScopeDto>,
    ) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .decide_approval(session_id, request_id, decision, scope);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, request_id, decision, scope);
    }

    pub fn answer_question(
        &self,
        session_id: String,
        request_id: String,
        answers: Vec<meerai_web_protocol::QuestionAnswerDto>,
    ) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .answer_question(session_id, request_id, answers, false);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, request_id, answers);
    }

    pub fn cancel_question(&self, session_id: String, request_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .answer_question(session_id, request_id, Vec::new(), true);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, request_id);
    }

    pub fn run_shell(&self, session_id: String, command: String, run_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.run_shell(session_id, command, run_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (session_id, command, run_id);
    }

    pub fn fetch_workspace_files(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_workspace_files();
    }

    pub fn search_workspace(&self, mode: SearchModeDto, query: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.search_workspace(mode, query);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (mode, query);
    }

    pub fn preview_workspace(&self, path: String, line: Option<usize>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.preview_workspace(path, line);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (path, line);
    }

    pub fn fetch_workspace_changes(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_workspace_changes();
    }

    pub fn fetch_workspace_diff(&self, path: String, status: Option<String>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_workspace_diff(path, status);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (path, status);
    }

    pub fn stage_workspace_file(&self, path: String, stage: bool) {
        #[cfg(target_arch = "wasm32")]
        self.inner.stage_workspace_file(path, stage);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (path, stage);
    }

    pub fn fetch_providers(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.fetch_providers();
    }

    pub fn set_api_key(
        &self,
        provider_id: String,
        api_key: String,
        base_url: Option<String>,
        return_to: ProviderModalReturn,
    ) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .set_api_key(provider_id, api_key, base_url, return_to);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (provider_id, api_key, base_url, return_to);
    }

    pub fn set_provider_base_url(&self, provider_id: String, base_url: Option<String>) {
        #[cfg(target_arch = "wasm32")]
        self.inner.set_provider_base_url(provider_id, base_url);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (provider_id, base_url);
    }

    pub fn remove_credential(&self, provider_id: String, return_to: ProviderModalReturn) {
        #[cfg(target_arch = "wasm32")]
        self.inner.remove_credential(provider_id, return_to);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (provider_id, return_to);
    }

    pub fn start_chatgpt_login(&self, provider_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.start_chatgpt_login(provider_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = provider_id;
    }

    pub fn paste_chatgpt_login(&self, provider_id: String, attempt_id: String, input: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner
            .paste_chatgpt_login(provider_id, attempt_id, input);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (provider_id, attempt_id, input);
    }

    pub fn cancel_chatgpt_login(&self, provider_id: String, attempt_id: String) {
        #[cfg(target_arch = "wasm32")]
        self.inner.cancel_chatgpt_login(provider_id, attempt_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (provider_id, attempt_id);
    }

    pub fn update_settings(&self, req: UpdateSettingsRequest) {
        #[cfg(target_arch = "wasm32")]
        self.inner.update_settings(req);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = req;
    }

    pub fn reload_config(&self) {
        #[cfg(target_arch = "wasm32")]
        self.inner.reload_config();
    }
}
