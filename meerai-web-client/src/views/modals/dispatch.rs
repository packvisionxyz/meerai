use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModalIdentity {
    ProviderSelect,
    AuthMethodSelect(String),
    ProviderConnection(String),
    ChatGptLogin(String),
    ApiKeyInput(String),
    ModelInput,
    DiffView,
    Search,
    Approval(String, String),
    Question(String, String),
    Terminal,
    RewindMenu,
    CommandPalette,
    Settings,
    QueuedEdit,
    VaultReset,
    Help,
    McpServers,
    SubagentOverlay(String),
    Runs,
    ShellRun(String),
    WorkspacePanel,
}

fn modal_identity(modal: &ActiveModal) -> ModalIdentity {
    match modal {
        ActiveModal::ProviderSelect { .. } => ModalIdentity::ProviderSelect,
        ActiveModal::AuthMethodSelect { provider_id, .. } => {
            ModalIdentity::AuthMethodSelect(provider_id.clone())
        }
        ActiveModal::ProviderConnection { provider_id, .. } => {
            ModalIdentity::ProviderConnection(provider_id.clone())
        }
        ActiveModal::ChatGptLogin { provider_id, .. } => {
            ModalIdentity::ChatGptLogin(provider_id.clone())
        }
        ActiveModal::ApiKeyInput { provider_id, .. } => {
            ModalIdentity::ApiKeyInput(provider_id.clone())
        }
        ActiveModal::ModelInput { .. } => ModalIdentity::ModelInput,
        ActiveModal::DiffView { .. } => ModalIdentity::DiffView,
        ActiveModal::Search { .. } => ModalIdentity::Search,
        ActiveModal::Approval {
            session_id,
            request,
            ..
        } => ModalIdentity::Approval(session_id.clone(), request.request_id.clone()),
        ActiveModal::Question {
            session_id,
            request,
            ..
        } => ModalIdentity::Question(session_id.clone(), request.request_id.clone()),
        ActiveModal::Terminal { .. } => ModalIdentity::Terminal,
        ActiveModal::RewindMenu { .. } => ModalIdentity::RewindMenu,
        ActiveModal::CommandPalette { .. } => ModalIdentity::CommandPalette,
        ActiveModal::Settings(_) => ModalIdentity::Settings,
        ActiveModal::QueuedEdit { .. } => ModalIdentity::QueuedEdit,
        ActiveModal::VaultReset { .. } => ModalIdentity::VaultReset,
        ActiveModal::Help => ModalIdentity::Help,
        ActiveModal::McpServers { .. } => ModalIdentity::McpServers,
        ActiveModal::WorkspacePanel { .. } => ModalIdentity::WorkspacePanel,
        ActiveModal::SubagentOverlay { entry_id, .. } => {
            ModalIdentity::SubagentOverlay(entry_id.clone())
        }
        ActiveModal::Runs { .. } => ModalIdentity::Runs,
        ActiveModal::ShellRun { run_id, .. } => ModalIdentity::ShellRun(run_id.clone()),
    }
}

#[component]
pub fn ModalLayer(store: StoreHandle) -> impl IntoView {
    let key = Memo::new(move |_| store.with(|s| s.modal.as_ref().map(modal_identity)));
    let opener: Rc<RefCell<Option<web_sys::HtmlElement>>> = Rc::new(RefCell::new(None));
    let opener_effect = opener.clone();
    let prev_open = Rc::new(RefCell::new(false));
    let prev_open_effect = prev_open.clone();

    Effect::new(move |_| {
        let current = key.get();
        let is_open = current.is_some();
        let was_open = *prev_open_effect.borrow();
        *prev_open_effect.borrow_mut() = is_open;

        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };

        if is_open && !was_open {
            let active = document
                .active_element()
                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
            *opener_effect.borrow_mut() = active;
        }

        if is_open {
            let cb = Closure::once_into_js(move || {
                let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                    return;
                };
                if let Ok(Some(target)) = doc
                    .query_selector(".modal input:not([disabled]), .modal textarea:not([disabled])")
                    && let Ok(el) = target.dyn_into::<web_sys::HtmlElement>()
                {
                    let _ = el.focus();
                    return;
                }
                if let Ok(Some(modal)) = doc.query_selector(".modal")
                    && let Ok(el) = modal.dyn_into::<web_sys::HtmlElement>()
                {
                    let _ = el.focus();
                }
            });
            if let Some(func) = cb.dyn_ref::<js_sys::Function>() {
                let _ = window.request_animation_frame(func);
            }
        } else if was_open
            && let Some(el) = opener_effect.borrow_mut().take()
            && el.is_connected()
        {
            let _ = el.focus();
        }
    });

    view! {
        {move || match key.get() {
            None => view! { <span></span> }.into_any(),
            Some(ModalIdentity::ProviderSelect) => view! { <ProviderSelectModal store=store /> }.into_any(),
            Some(ModalIdentity::AuthMethodSelect(_)) => view! { <AuthMethodModal store=store /> }.into_any(),
            Some(ModalIdentity::ProviderConnection(_)) => view! { <ProviderConnectionModal store=store /> }.into_any(),
            Some(ModalIdentity::ChatGptLogin(_)) => view! { <ChatGptLoginModal store=store /> }.into_any(),
            Some(ModalIdentity::ApiKeyInput(_)) => view! { <ApiKeyModal store=store /> }.into_any(),
            Some(ModalIdentity::ModelInput) => view! { <ModelInputModal store=store /> }.into_any(),
            Some(ModalIdentity::DiffView) => view! { <DiffModal store=store /> }.into_any(),
            Some(ModalIdentity::Search) => view! { <SearchModal store=store /> }.into_any(),
            Some(ModalIdentity::Approval(_, _)) => view! { <ApprovalModal store=store /> }.into_any(),
            Some(ModalIdentity::Question(_, _)) => view! { <QuestionModal store=store /> }.into_any(),
            Some(ModalIdentity::Terminal) => view! { <TerminalModal store=store /> }.into_any(),
            Some(ModalIdentity::RewindMenu) => view! { <RewindModal store=store /> }.into_any(),
            Some(ModalIdentity::CommandPalette) => view! { <CommandPaletteModal store=store /> }.into_any(),
            Some(ModalIdentity::Settings) => view! { <SettingsModal store=store /> }.into_any(),
            Some(ModalIdentity::QueuedEdit) => view! { <QueuedEditModal store=store /> }.into_any(),
            Some(ModalIdentity::VaultReset) => view! { <VaultResetModal store=store /> }.into_any(),
            Some(ModalIdentity::Help) => help_modal(store),
            Some(ModalIdentity::McpServers) => view! { <McpServersModal store=store /> }.into_any(),
            Some(ModalIdentity::SubagentOverlay(_)) => view! { <SubagentModal store=store /> }.into_any(),
            Some(ModalIdentity::Runs) => view! { <RunsModal store=store /> }.into_any(),
            Some(ModalIdentity::ShellRun(_)) => view! { <ShellRunModal store=store /> }.into_any(),
            Some(ModalIdentity::WorkspacePanel) => view! { <WorkspacePanelModal store=store /> }.into_any(),
        }}
    }
}
