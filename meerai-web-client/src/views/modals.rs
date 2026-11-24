use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{
    ApprovalDecisionDto, AuthKindDto, BackendKind, FileDiffResponse, GrantScopeDto, ModelItemDto,
    ProviderItemDto, RewindScopeDto, SearchModeDto,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::keymap::build_question_answers;
use crate::state::{
    ActiveModal, ProviderModalReturn, RunKindDto, activate_provider_from_modal, back_to_runs,
    custom_model_candidate, execute_palette_action, execute_slash_command, filtered_commands,
    filtered_models_sorted, filtered_palette_actions, filtered_providers_sorted,
    format_duration_ms, format_tokens, is_recent_model, is_removable_credential_badge,
    open_api_key_modal, open_provider_credential_update, open_provider_model_select,
    open_provider_select, open_run_row, return_from_provider_modal, running_subagent_rows,
    set_status_notice, start_oauth_modal, update_completion,
};
use crate::store::StoreHandle;
use crate::views::command_label;
use crate::views::settings::SettingsModal;

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
            Some(ModalIdentity::SubagentOverlay(_)) => view! { <SubagentModal store=store /> }.into_any(),
            Some(ModalIdentity::Runs) => view! { <RunsModal store=store /> }.into_any(),
            Some(ModalIdentity::ShellRun(_)) => view! { <ShellRunModal store=store /> }.into_any(),
            Some(ModalIdentity::WorkspacePanel) => view! { <WorkspacePanelModal store=store /> }.into_any(),
        }}
    }
}

#[component]
fn ProviderSelectModal(store: StoreHandle) -> impl IntoView {
    let list_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ProviderSelect { filter, selected }) => {
                let active_provider = s.active_session().map(|sess| sess.provider.clone());
                let loading = s.providers.is_none();
                (
                    loading,
                    active_provider.clone(),
                    filtered_providers_sorted(
                        s.providers.as_ref(),
                        active_provider.as_deref(),
                        &filter.text,
                    ),
                    *selected,
                )
            }
            _ => (false, None, Vec::new(), 0),
        })
    });
    let initial_filter = store.with(|s| match &s.modal {
        Some(ActiveModal::ProviderSelect { filter, .. }) => filter.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            <div class="hint">"Select a provider to configure credentials, set a custom Base URL, or choose a model."</div>
            <input
                class="input"
                placeholder="Filter providers (name, id, status)…"
                prop:value=initial_filter
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::ProviderSelect { filter, selected }) = &mut s.modal {
                            filter.replace_text(value);
                            *selected = 0;
                        }
                    });
                }
            />
            <div class="pick-list" role="listbox">
                {move || {
                    let (loading, active_provider, providers, selected) = list_state.get();
                    if loading {
                        return view! { <div class="hint">"Loading providers…"</div> }.into_any();
                    }
                    if providers.is_empty() {
                        return view! { <div class="hint">"No providers match this filter."</div> }.into_any();
                    }
                    let rows = providers
                        .into_iter()
                        .enumerate()
                        .map(|(index, provider)| {
                            let is_active = active_provider.as_deref() == Some(provider.id.as_str());
                            provider_row(store, provider, index == selected, is_active)
                        })
                        .collect::<Vec<_>>();
                    AnyView::from(Fragment::new(rows))
                }}
            </div>
            <div class="hint">"Credentials stay on the host. The browser never receives a secret key."</div>
        </div>
    }
    .into_any();
    shell(
        "Connect provider",
        store,
        body,
        view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }
            .into_any(),
    )
}

#[component]
fn AuthMethodModal(store: StoreHandle) -> impl IntoView {
    let info = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::AuthMethodSelect {
                provider_id,
                provider_name,
                return_to,
                ..
            }) => Some((provider_id.clone(), provider_name.clone(), *return_to)),
            _ => None,
        })
    });
    let title = info
        .get_untracked()
        .map(|(_, name, _)| format!("Connect {name}"))
        .unwrap_or_else(|| "Authentication method".to_owned());
    let body = view! {
        {move || {
            let Some((provider_id, provider_name, return_to)) = info.get() else {
                return view! { <span></span> }.into_any();
            };
            let provider = find_provider(&store, &provider_id);
            let api_provider = provider.clone();
            let oauth_provider = provider;
            view! {
                <div class="modal-form">
                    <p>{format!("Choose how to authenticate with {provider_name}:")}</p>
                    <div class="choice-grid">
                        <button class="choice" on:click=move |_| {
                            if let Some(provider) = oauth_provider.clone() {
                                store.update(|s| start_oauth_modal(s, &provider, return_to));
                            }
                        }>
                            <span class="choice-title">"ChatGPT sign-in"</span>
                            <span class="choice-desc">"Use your ChatGPT Plus/Pro plan via browser sign-in."</span>
                        </button>
                        <button class="choice" on:click=move |_| {
                            if let Some(provider) = api_provider.clone() {
                                store.update(|s| open_api_key_modal(s, &provider, return_to));
                            }
                        }>
                            <span class="choice-title">"API key / Base URL"</span>
                            <span class="choice-desc">"Store an API key or custom endpoint URL on the host."</span>
                        </button>
                    </div>
                </div>
            }
            .into_any()
        }}
    }
    .into_any();
    let actions = view! {
        {move || {
            let return_to = info.get().map(|(_, _, r)| r).unwrap_or(ProviderModalReturn::Close);
            view! {
                <>
                    {(return_to != ProviderModalReturn::Close).then(|| view! {
                        <button class="btn" on:click=move |_| store.update(|s| return_from_provider_modal(s, return_to))>"Back"</button>
                    })}
                    <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
                </>
            }
        }}
    }
    .into_any();
    shell(&title, store, body, actions)
}

#[component]
fn ProviderConnectionModal(store: StoreHandle) -> impl IntoView {
    let info = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ProviderConnection {
                provider_id,
                provider_name,
                auth_kind,
                status_badge,
                configured,
                base_url,
                env_vars,
                default_model,
                return_to,
                ..
            }) => {
                let live = find_provider_state(s, provider_id);
                Some((
                    provider_id.clone(),
                    live.map(|p| p.display_name.clone())
                        .unwrap_or_else(|| provider_name.clone()),
                    live.map(|p| p.auth_kind).unwrap_or(*auth_kind),
                    live.map(|p| p.status_badge.clone())
                        .unwrap_or_else(|| status_badge.clone()),
                    live.map(|p| p.configured).unwrap_or(*configured),
                    live.and_then(|p| p.base_url.clone())
                        .or_else(|| base_url.clone()),
                    live.map(|p| p.env_vars.clone())
                        .unwrap_or_else(|| env_vars.clone()),
                    live.and_then(|p| p.default_model.clone())
                        .or_else(|| default_model.clone()),
                    *return_to,
                    s.active_session_id.is_some()
                        && !s
                            .active_session()
                            .is_some_and(|sess| matches!(sess.backend, BackendKind::Acp { .. })),
                ))
            }
            _ => None,
        })
    });
    let title = info
        .get_untracked()
        .map(|(_, name, ..)| name)
        .unwrap_or_else(|| "Provider".to_owned());
    let body = view! {
        {move || {
            let Some((
                provider_id,
                provider_name,
                _,
                status_badge,
                configured,
                base_url,
                env_vars,
                default_model,
                _,
                _,
            )) = info.get() else {
                return view! { <span></span> }.into_any();
            };
            let source_note = if status_badge.starts_with("env:") {
                Some(format!("Credentials come from environment variable ({status_badge})."))
            } else if status_badge == "config" {
                Some("Credentials are configured in settings.yml on the host.".to_owned())
            } else {
                None
            };
            view! {
                <div class="modal-form">
                    <div class="kv"><span>"Provider"</span><strong>{format!("{provider_name} ({provider_id})")}</strong></div>
                    <div class="kv">
                        <span>"Status"</span>
                        <span class=format!("badge {}", if configured { "ok" } else { "muted" })>{status_badge}</span>
                    </div>
                    <div class="kv">
                        <span>"Base URL"</span>
                        <code>{base_url.unwrap_or_else(|| "Provider default".to_owned())}</code>
                    </div>
                    {default_model.map(|m| view! { <div class="kv"><span>"Default model"</span><code>{m}</code></div> }.into_any())}
                    {(!env_vars.is_empty()).then(|| view! {
                        <div class="kv"><span>"Env variables"</span><code>{env_vars.join(", ")}</code></div>
                    }.into_any())}
                    {source_note.map(|note| view! { <div class="hint">{note}</div> }.into_any())}
                </div>
            }
            .into_any()
        }}
    }
    .into_any();
    let actions = view! {
        {move || {
            let Some((
                provider_id,
                _,
                auth_kind,
                status_badge,
                _,
                _,
                _,
                _,
                return_to,
                can_pick_model,
            )) = info.get() else {
                return view! { <span></span> }.into_any();
            };
            let id_model = provider_id.clone();
            let id_update = provider_id.clone();
            let id_remove = provider_id;
            let cred_label = match auth_kind {
                AuthKindDto::None => "Configure Base URL",
                AuthKindDto::ApiKey => "Update API key & URL",
                AuthKindDto::OAuth => "Re-authenticate ChatGPT",
                AuthKindDto::Either => "Update auth / Base URL",
            };
            let can_remove = is_removable_credential_badge(&status_badge);
            view! {
                <>
                    {(return_to != ProviderModalReturn::Close).then(|| view! {
                        <button class="btn" on:click=move |_| store.update(|s| return_from_provider_modal(s, return_to))>"Back"</button>
                    })}
                    {can_remove.then(|| {
                        let id = id_remove.clone();
                        view! {
                            <button class="btn danger" on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.remove_credential(id.clone(), return_to);
                                }
                            }>"Remove saved credential"</button>
                        }
                    })}
                    <button class="btn" on:click=move |_| {
                        let id = id_update.clone();
                        store.update(|s| {
                            if let Some(provider) = find_provider_state(s, &id).cloned() {
                                open_provider_credential_update(s, &provider, return_to);
                            }
                        });
                    }>{cred_label}</button>
                    <button
                        class="btn primary"
                        disabled=!can_pick_model
                        title=if can_pick_model { "Pick a model for this provider" } else { "Open a native session to choose a model" }
                        on:click=move |_| {
                            let id = id_model.clone();
                            store.update(|s| open_provider_model_select(s, id, return_to));
                        }
                    >"Choose model"</button>
                </>
            }
            .into_any()
        }}
    }
    .into_any();
    shell(&title, store, body, actions)
}

#[component]
fn ChatGptLoginModal(store: StoreHandle) -> impl IntoView {
    let snapshot = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ChatGptLogin {
                provider_id,
                provider_name,
                attempt_id,
                url,
                state,
                error,
                busy,
                return_to,
                ..
            }) => Some((
                provider_id.clone(),
                provider_name.clone(),
                attempt_id.clone(),
                url.clone(),
                *state,
                error.clone(),
                *busy,
                *return_to,
            )),
            _ => None,
        })
    });
    let initial_paste = store.with(|s| match &s.modal {
        Some(ActiveModal::ChatGptLogin { paste, .. }) => paste.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            {move || {
                let Some((_, provider_name, _, url, state, error, _, _)) = snapshot.get() else {
                    return view! { <span></span> }.into_any();
                };
                let status = state
                    .as_ref()
                    .map(|state| format!("{state:?}"))
                    .unwrap_or_else(|| "waiting".to_owned());
                view! {
                    <div class="modal-form">
                        <div class="kv"><span>"Provider"</span><strong>{provider_name}</strong></div>
                        <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
                        {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                        {url.map(|url| {
                            let copy_url = url.clone();
                            view! {
                                <div class="modal-form">
                                    <div class="hint">"Open the sign-in page. On a remote host, paste the final redirect URL below."</div>
                                    <code class="url-box">{url.clone()}</code>
                                    <div class="oauth-links">
                                        <a class="btn primary" href=url target="_blank" rel="noopener noreferrer" referrerpolicy="no-referrer">"Open sign-in page"</a>
                                        <button class="btn" on:click=move |_| {
                                            crate::store::copy_text(copy_url.clone());
                                            store.update(|s| set_status_notice(s, "Copied sign-in URL"));
                                        }>"Copy URL"</button>
                                    </div>
                                </div>
                            }
                            .into_any()
                        })}
                    </div>
                }
                .into_any()
            }}
            <input
                class="input"
                placeholder="Paste the redirect URL"
                prop:value=initial_paste
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::ChatGptLogin { paste, .. }) = &mut s.modal {
                            paste.replace_text(value);
                        }
                    });
                }
            />
        </div>
    }
    .into_any();
    let actions = view! {
        <button
            class="btn primary"
            disabled=move || snapshot.get().is_none_or(|(_, _, att, _, _, _, busy, _)| busy || att.is_none())
            on:click=move |_| {
                let (id, attempt, input) = store.with(|s| match &s.modal {
                    Some(ActiveModal::ChatGptLogin {
                        provider_id,
                        attempt_id,
                        paste,
                        ..
                    }) => (
                        provider_id.clone(),
                        attempt_id.clone(),
                        paste.text.clone(),
                    ),
                    _ => (String::new(), None, String::new()),
                });
                if input.trim().is_empty() {
                    store.update(|s| {
                        if let Some(ActiveModal::ChatGptLogin { error, .. }) = &mut s.modal {
                            *error = Some("Paste the redirect URL or code first.".to_owned());
                        }
                    });
                    return;
                }
                if let (Some(attempt), Some(t)) = (attempt, crate::transport::current()) {
                    t.paste_chatgpt_login(id, attempt, input);
                }
            }
        >"Submit redirect"</button>
        <button class="btn" on:click=move |_| {
            let (id, attempt, return_to) = store.with(|s| match &s.modal {
                Some(ActiveModal::ChatGptLogin {
                    provider_id,
                    attempt_id,
                    return_to,
                    ..
                }) => (provider_id.clone(), attempt_id.clone(), *return_to),
                _ => (String::new(), None, ProviderModalReturn::Close),
            });
            if let (Some(attempt), Some(t)) = (attempt, crate::transport::current()) {
                t.cancel_chatgpt_login(id, attempt);
            }
            store.update(|s| return_from_provider_modal(s, return_to));
        }>"Cancel"</button>
    }
    .into_any();
    shell("ChatGPT sign-in", store, body, actions)
}

#[component]
fn ApiKeyModal(store: StoreHandle) -> impl IntoView {
    let status_memo = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ApiKeyInput {
                provider_id,
                provider_name,
                auth_kind,
                status_badge,
                configured,
                env_vars,
                error,
                busy,
                return_to,
                ..
            }) => {
                let live = find_provider_state(s, provider_id);
                Some((
                    provider_id.clone(),
                    live.map(|p| p.display_name.clone())
                        .unwrap_or_else(|| provider_name.clone()),
                    live.map(|p| p.auth_kind).unwrap_or(*auth_kind),
                    live.map(|p| p.status_badge.clone())
                        .unwrap_or_else(|| status_badge.clone()),
                    live.map(|p| p.configured).unwrap_or(*configured),
                    live.map(|p| p.env_vars.clone())
                        .unwrap_or_else(|| env_vars.clone()),
                    error.clone(),
                    *busy,
                    *return_to,
                ))
            }
            _ => None,
        })
    });
    let (init_key, init_base) = store.with(|s| match &s.modal {
        Some(ActiveModal::ApiKeyInput {
            api_key, base_url, ..
        }) => (api_key.text.clone(), base_url.text.clone()),
        _ => (String::new(), String::new()),
    });
    let title = status_memo
        .get_untracked()
        .map(|(_, name, _, _, configured, ..)| {
            if configured {
                format!("Configure {name}")
            } else {
                format!("Connect {name}")
            }
        })
        .unwrap_or_else(|| "Provider configuration".to_owned());
    let body = view! {
        <div class="modal-form">
            {move || {
                let Some((provider_id, provider_name, auth_kind, status_badge, configured, env_vars, error, _, _)) = status_memo.get() else {
                    return view! { <span></span> }.into_any();
                };
                let needs_key = auth_kind != AuthKindDto::None;
                let key_placeholder = if configured {
                    "Leave blank to keep existing credential"
                } else {
                    "sk-…"
                };
                let env_hint = (!env_vars.is_empty()).then(|| {
                    format!("Stored in the encrypted vault on the host. Alternatively set {} in the host environment.", env_vars.join(" or "))
                });
                view! {
                    <div class="modal-form">
                        <div class="kv"><span>"Provider"</span><strong>{format!("{provider_name} ({provider_id})")}</strong></div>
                        <div class="kv">
                            <span>"Status"</span>
                            <span class=format!("badge {}", if configured { "ok" } else { "muted" })>{status_badge}</span>
                        </div>
                        {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                        {needs_key.then(|| view! {
                            <label class="field">
                                <span class="field-label">"API key"</span>
                                <input
                                    class="input"
                                    type="password"
                                    placeholder=key_placeholder
                                    prop:value=init_key.clone()
                                    on:input=move |ev| {
                                        let value = event_target_value(&ev);
                                        store.update(|s| {
                                            if let Some(ActiveModal::ApiKeyInput { api_key, .. }) = &mut s.modal {
                                                api_key.replace_text(value);
                                            }
                                        });
                                    }
                                />
                                {env_hint.map(|h| view! { <span class="hint">{h}</span> }.into_any())}
                            </label>
                        }.into_any())}
                    </div>
                }
                .into_any()
            }}
            <label class="field">
                <span class="field-label">"Base URL (optional)"</span>
                <input
                    class="input"
                    placeholder="Provider default (e.g. https://api.example.com/v1)"
                    prop:value=init_base
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::ApiKeyInput { base_url, .. }) = &mut s.modal {
                                base_url.replace_text(value);
                            }
                        });
                    }
                />
                <span class="hint">"Leave blank to use the provider's default endpoint."</span>
            </label>
        </div>
    }
    .into_any();
    let actions = view! {
        {move || {
            let Some((id, _, _, status_badge, _, _, _, busy, return_to)) = status_memo.get() else {
                return view! { <span></span> }.into_any();
            };
            let id_remove = id.clone();
            let show_remove = status_badge == "saved";
            view! {
                <>
                    {(return_to != ProviderModalReturn::Close).then(|| view! {
                        <button class="btn" disabled=busy on:click=move |_| store.update(|s| return_from_provider_modal(s, return_to))>"Back"</button>
                    })}
                    {show_remove.then(|| view! {
                        <button class="btn danger" disabled=busy on:click=move |_| {
                            if let Some(t) = crate::transport::current() {
                                t.remove_credential(id_remove.clone(), return_to);
                            }
                        }>"Remove saved key"</button>
                    })}
                    <button class="btn" disabled=busy on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
                    <button
                        class="btn primary"
                        disabled=busy
                        on:click=move |_| {
                            let (id, key, base, configured, auth_kind, return_to) = store.with(|s| match &s.modal {
                                Some(ActiveModal::ApiKeyInput {
                                    provider_id,
                                    auth_kind,
                                    configured,
                                    api_key,
                                    base_url,
                                    return_to,
                                    ..
                                }) => {
                                    let live = find_provider_state(s, provider_id);
                                    (
                                        provider_id.clone(),
                                        api_key.text.clone(),
                                        base_url.text.trim().to_owned(),
                                        live.map(|p| p.configured).unwrap_or(*configured),
                                        live.map(|p| p.auth_kind).unwrap_or(*auth_kind),
                                        *return_to,
                                    )
                                }
                                _ => (String::new(), String::new(), String::new(), false, AuthKindDto::ApiKey, ProviderModalReturn::Close),
                            });
                            if !configured && auth_kind != AuthKindDto::None && key.trim().is_empty() {
                                store.update(|s| {
                                    if let Some(ActiveModal::ApiKeyInput { error, .. }) = &mut s.modal {
                                        *error = Some("Enter an API key.".to_owned());
                                    }
                                });
                                return;
                            }
                            if let Some(t) = crate::transport::current() {
                                t.set_api_key(id, key, Some(base), return_to);
                            }
                            store.update(|s| {
                                if let Some(ActiveModal::ApiKeyInput { busy, error, .. }) = &mut s.modal {
                                    *busy = true;
                                    *error = None;
                                }
                            });
                        }
                    >"Save"</button>
                </>
            }
            .into_any()
        }}
    }
    .into_any();
    shell(&title, store, body, actions)
}

#[component]
fn ModelInputModal(store: StoreHandle) -> impl IntoView {
    let is_acp_memo = Memo::new(move |_| {
        store.with(|s| {
            s.active_session()
                .is_some_and(|sess| matches!(sess.backend, BackendKind::Acp { .. }))
        })
    });
    let scope_memo = Memo::new(move |_| {
        store.with(|s| {
            let providers = s
                .providers
                .as_ref()
                .map(|p| p.providers.clone())
                .unwrap_or_default();
            let (provider_id, return_to) = match &s.modal {
                Some(ActiveModal::ModelInput {
                    provider_id,
                    return_to,
                    ..
                }) => (provider_id.clone(), *return_to),
                _ => (None, ProviderModalReturn::Close),
            };
            let active_pair = s
                .active_session()
                .map(|session| (session.provider.clone(), session.model.clone()));
            (provider_id, providers, active_pair, return_to)
        })
    });
    let list_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ModelInput {
                filter,
                provider_id,
                selected,
                return_to,
            }) => {
                let active_provider = s.active_session().map(|sess| sess.provider.clone());
                let models = filtered_models_sorted(
                    s.providers.as_ref(),
                    provider_id.as_deref(),
                    active_provider.as_deref(),
                    &filter.text,
                );
                let custom =
                    custom_model_candidate(s, provider_id.as_deref(), &filter.text, &models);
                let providers = s
                    .providers
                    .as_ref()
                    .map(|p| p.providers.clone())
                    .unwrap_or_default();
                let active_pair = s
                    .active_session()
                    .map(|session| (session.provider.clone(), session.model.clone()));
                (
                    s.providers.is_none(),
                    models,
                    custom,
                    *selected,
                    provider_id.clone(),
                    providers,
                    active_pair,
                    *return_to,
                )
            }
            _ => (
                false,
                Vec::new(),
                None,
                0,
                None,
                Vec::new(),
                None,
                ProviderModalReturn::Close,
            ),
        })
    });
    let initial_filter = store.with(|s| match &s.modal {
        Some(ActiveModal::ModelInput { filter, .. }) => filter.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            {move || {
                if is_acp_memo.get() {
                    return view! {
                        <div class="banner notice">"External ACP sessions manage their own model. Switch to a native session to change models."</div>
                    }.into_any();
                }
                view! {
                    <>
                        {move || {
                            let (provider_scope, providers, active_pair, return_to) = scope_memo.get();
                            let scope_val = provider_scope.clone().unwrap_or_default();
                            let scope_provider = provider_scope
                                .as_deref()
                                .and_then(|id| providers.iter().find(|p| p.id == id).cloned());
                            let config_btn_label = match &scope_provider {
                                Some(p) if p.configured => "Manage provider",
                                Some(_) => "Connect provider",
                                None => "Providers…",
                            };
                            view! {
                                <>
                                    {active_pair.map(|(p, m)| view! {
                                        <div class="hint">{format!("Current session model: {p}/{m}")}</div>
                                    })}
                                    <div class="model-scope-bar">
                                        <select
                                            class="input model-scope-select"
                                            aria-label="Filter by provider"
                                            on:change=move |ev| {
                                                let val = event_target_value(&ev);
                                                let next = (!val.is_empty()).then_some(val);
                                                store.update(|s| {
                                                    if let Some(ActiveModal::ModelInput { provider_id, selected, .. }) = &mut s.modal {
                                                        *provider_id = next;
                                                        *selected = 0;
                                                    }
                                                });
                                            }
                                        >
                                            <option value="" prop:selected=scope_val.is_empty()>"All providers"</option>
                                            {providers.iter().map(|p| {
                                                let pid = p.id.clone();
                                                let label = format!("{} ({})", p.display_name, p.status_badge);
                                                let is_selected = scope_val == pid;
                                                view! {
                                                    <option value=pid prop:selected=is_selected>{label}</option>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </select>
                                        <button
                                            class="btn"
                                            on:click=move |_| {
                                                if let Some(provider) = scope_provider.clone() {
                                                    let next_return = if return_to == ProviderModalReturn::SettingsConnections {
                                                        ProviderModalReturn::SettingsConnections
                                                    } else {
                                                        ProviderModalReturn::ProviderSelect
                                                    };
                                                    store.update(|s| activate_provider_from_modal(s, provider, next_return));
                                                } else {
                                                    store.update(open_provider_select);
                                                }
                                            }
                                        >{config_btn_label}</button>
                                    </div>
                                </>
                            }
                        }}
                        <input
                            class="input"
                            placeholder="Filter models or type custom model ID (e.g. anthropic/claude-sonnet-4-5)"
                            prop:value=initial_filter.clone()
                            on:input=move |ev| {
                                let value = event_target_value(&ev);
                                store.update(|s| {
                                    if let Some(ActiveModal::ModelInput { filter, selected, .. }) = &mut s.modal {
                                        filter.replace_text(value);
                                        *selected = 0;
                                    }
                                });
                            }
                        />
                        <div class="pick-list" role="listbox">
                            {move || {
                                let (loading, models, custom, selected, provider_scope, providers, active_pair, _) = list_state.get();
                                let model_len = models.len();
                                let total_len = model_len + usize::from(custom.is_some());
                                let clamped_sel = if total_len == 0 { 0 } else { selected.min(total_len - 1) };
                                if loading {
                                    view! { <div class="hint">"Loading models…"</div> }.into_any()
                                } else if total_len == 0 {
                                    view! {
                                        <div class="hint">
                                            {match provider_scope {
                                                Some(p) => format!("No saved or catalog models for {p} yet. Type a model ID above and press Enter."),
                                                None => "No models match this filter. Type provider/model above and press Enter.".to_owned(),
                                            }}
                                        </div>
                                    }.into_any()
                                } else {
                                    let mut rows = models
                                        .into_iter()
                                        .enumerate()
                                        .map(|(idx, model)| {
                                            let is_current = active_pair
                                                .as_ref()
                                                .is_some_and(|(p, m)| p == &model.provider && m == &model.id);
                                            let prov = providers.iter().find(|p| p.id == model.provider);
                                            let prov_badge = prov.map(|p| (p.status_badge.clone(), p.configured));
                                            let recent = store.with(|s| is_recent_model(s.providers.as_ref(), &model.provider, &model.id));
                                            model_row(store, model, idx == clamped_sel, is_current, recent, prov_badge)
                                        })
                                        .collect::<Vec<_>>();
                                    if let Some((cp, cm)) = custom {
                                        let cp_click = cp.clone();
                                        let cm_click = cm.clone();
                                        let is_sel = clamped_sel == model_len;
                                        rows.push(view! {
                                            <button class="pick-row" class:active=is_sel on:click=move |_| {
                                                if let (Some(id), Some(t)) = (
                                                    store.with(|s| s.active_session_id.clone()),
                                                    crate::transport::current(),
                                                ) {
                                                    t.set_model(id, cp_click.clone(), cm_click.clone());
                                                }
                                                store.update(|s| s.modal = None);
                                            }>
                                                <span class="pick-title">{format!("{cp}/{cm}")}</span>
                                                <span class="pick-meta">"Custom model ID"</span>
                                                <span class="badge">"custom"</span>
                                            </button>
                                        }.into_any());
                                    }
                                    AnyView::from(Fragment::new(rows))
                                }
                            }}
                        </div>
                    </>
                }.into_any()
            }}
        </div>
    }
    .into_any();
    let actions = view! {
        {move || {
            let is_acp = is_acp_memo.get();
            let (_, models, custom, selected, _, _, _, return_to) = list_state.get();
            let total = models.len() + usize::from(custom.is_some());
            let can_use = !is_acp && total > 0;
            view! {
                <>
                    {(return_to != ProviderModalReturn::Close).then(|| view! {
                        <button class="btn" on:click=move |_| store.update(|s| return_from_provider_modal(s, return_to))>"Back"</button>
                    })}
                    <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
                    {(!is_acp).then(|| view! {
                        <button class="btn primary" disabled=!can_use on:click=move |_| {
                            let idx = selected.min(total.saturating_sub(1));
                            let target = if let Some(model) = models.get(idx) {
                                Some((model.provider.clone(), model.id.clone()))
                            } else {
                                custom.clone()
                            };
                            if let (Some((provider, model)), Some(id), Some(t)) = (
                                target,
                                store.with(|s| s.active_session_id.clone()),
                                crate::transport::current(),
                            ) {
                                t.set_model(id, provider, model);
                            }
                            store.update(|s| s.modal = None);
                        }>"Use model"</button>
                    })}
                </>
            }
        }}
    }
    .into_any();
    shell("Set model", store, body, actions)
}

#[component]
fn ApprovalModal(store: StoreHandle) -> impl IntoView {
    let state_memo = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Approval {
                session_id,
                request,
                showing_reason,
                ..
            }) => Some((session_id.clone(), request.clone(), *showing_reason)),
            _ => None,
        })
    });
    let body = view! {
        {move || {
            let Some((_, request, showing_reason)) = state_memo.get() else {
                return view! { <span></span> }.into_any();
            };
            let title = request.title;
            let detail = request.detail;
            let effect = request
                .effect
                .map(|effect| format!("{effect:?}"))
                .unwrap_or_else(|| "unknown".to_owned());
            let initial_reason = store.with(|s| match &s.modal {
                Some(ActiveModal::Approval { reason, .. }) => reason.text.clone(),
                _ => String::new(),
            });
            view! {
                <div class="modal-form">
                    <div class="approval-kind">{format!("{:?}", request.kind)}</div>
                    <div class="approval-title">{title}</div>
                    <pre class="approval-detail">{detail}</pre>
                    <div class="kv"><span>"Effect"</span><strong>{effect}</strong></div>
                    <Show when=move || showing_reason>
                        <input
                            id="approval-reason"
                            class="input"
                            placeholder="Reason for declining"
                            prop:value=initial_reason.clone()
                            on:input=move |ev| {
                                let value = event_target_value(&ev);
                                store.update(|s| {
                                    if let Some(ActiveModal::Approval { reason, .. }) = &mut s.modal {
                                        reason.replace_text(value);
                                    }
                                });
                            }
                        />
                    </Show>
                    <div class="hint">"Shortcuts: y approve once · t allow turn · a allow session · n decline · r reason · c cancel request"</div>
                </div>
            }
            .into_any()
        }}
    }
    .into_any();

    let actions = view! {
        {move || {
            let Some((session_id, request, showing_reason)) = state_memo.get() else {
                return view! { <span></span> }.into_any();
            };
            let request_id = request.request_id;
            let sid = session_id.clone();
            let rid = request_id.clone();
            let sid_turn = session_id.clone();
            let rid_turn = request_id.clone();
            let sid_session = session_id.clone();
            let rid_session = request_id.clone();
            let sid_decline = session_id.clone();
            let rid_decline = request_id.clone();
            let sid_reason = session_id.clone();
            let rid_reason = request_id.clone();
            let sid_cancel = session_id;
            let rid_cancel = request_id;
            view! {
                <button class="btn primary" on:click=move |_| decide(&store, &sid, &rid, ApprovalDecisionDto::Accept, None)>"Approve once (y)"</button>
                <button class="btn" on:click=move |_| decide(&store, &sid_turn, &rid_turn, ApprovalDecisionDto::Accept, Some(GrantScopeDto::Turn))>"Allow turn (t)"</button>
                <button class="btn" on:click=move |_| decide(&store, &sid_session, &rid_session, ApprovalDecisionDto::AcceptForSession, Some(GrantScopeDto::Session))>"Allow session (a)"</button>
                <button class="btn danger" on:click=move |_| decide(&store, &sid_decline, &rid_decline, ApprovalDecisionDto::Decline, None)>"Decline (n)"</button>
                {if showing_reason {
                    view! {
                        <button class="btn danger" on:click=move |_| {
                            let reason = store.with(|s| match &s.modal {
                                Some(ActiveModal::Approval { reason, .. }) => reason.text.clone(),
                                _ => String::new(),
                            });
                            let reason = if reason.trim().is_empty() {
                                "Declined by the browser client.".to_owned()
                            } else {
                                reason
                            };
                            decide(&store, &sid_reason, &rid_reason, ApprovalDecisionDto::DeclineWithReason { reason }, None);
                        }>"Send decline reason"</button>
                    }.into_any()
                } else {
                    view! {
                        <button class="btn" on:click=move |_| {
                            store.update_with_effects(|s, fx| {
                                if let Some(ActiveModal::Approval { showing_reason, .. }) = &mut s.modal {
                                    *showing_reason = true;
                                    fx.focus("approval-reason");
                                }
                            });
                        }>"Decline with reason (r)"</button>
                    }.into_any()
                }}
                <button class="btn" on:click=move |_| decide(&store, &sid_cancel, &rid_cancel, ApprovalDecisionDto::Cancel, None)>"Cancel request (c)"</button>
            }
            .into_any()
        }}
    }
    .into_any();
    shell("Approval required", store, body, actions)
}

#[component]
fn QuestionModal(store: StoreHandle) -> impl IntoView {
    let q_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Question {
                session_id,
                request,
                draft,
            }) => Some((
                session_id.clone(),
                request.clone(),
                draft.step,
                draft.answers.clone(),
                draft.focused_option,
            )),
            _ => None,
        })
    });
    let step_memo = Memo::new(move |_| q_state.get().map(|(_, _, step, _, _)| step).unwrap_or(0));

    let body = view! {
        <div class="modal-form">
            {move || {
                let Some((_, request, step, answers, focused_option)) = q_state.get() else {
                    return view! { <span></span> }.into_any();
                };
                let step = step.min(request.questions.len().saturating_sub(1));
                let Some(question) = request.questions.get(step) else {
                    return view! { <div class="modal-form">"No questions."</div> }.into_any();
                };
                let selected = answers.get(step).cloned().unwrap_or_default();
                let multiple = question.multiple;
                let options = question
                    .options
                    .iter()
                    .enumerate()
                    .map(|(idx, option)| {
                        let label = option.label.clone();
                        let label_click = label.clone();
                        let is_selected = selected.contains(&label);
                        let is_focused = idx == focused_option;
                        let marker = match (multiple, is_selected) {
                            (true, true) => "[x]",
                            (true, false) => "[ ]",
                            (false, true) => "(•)",
                            (false, false) => "( )",
                        };
                        view! {
                            <button
                                class="pick-row"
                                class:active=is_selected
                                class:focused=is_focused
                                on:click=move |_| {
                                    let label = label_click.clone();
                                    store.update(|s| {
                                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                                            let answers = &mut draft.answers[step];
                                            if multiple {
                                                if let Some(pos) = answers.iter().position(|value| value == &label) {
                                                    answers.remove(pos);
                                                } else {
                                                    answers.push(label.clone());
                                                }
                                            } else {
                                                *answers = vec![label.clone()];
                                            }
                                            draft.focused_option = idx;
                                        }
                                    });
                                }
                            >
                                <span class="pick-marker">{marker}</span>
                                <span class="pick-title">{label}</span>
                                {option.description.clone().map(|description| view! { <span class="pick-meta">{description}</span> }.into_any())}
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div class="modal-form">
                        <div class="hint">
                            {format!(
                                "Question {} of {} · {}",
                                step + 1,
                                request.questions.len(),
                                if multiple { "Select all that apply" } else { "Select one" }
                            )}
                        </div>
                        <div class="question-head">{question.header.clone()}</div>
                        <div class="question-text">{question.question.clone()}</div>
                        <div class="pick-list">{AnyView::from(Fragment::new(options))}</div>
                    </div>
                }
                .into_any()
            }}
            {move || {
                let step = step_memo.get();
                let custom = store.with(|s| match &s.modal {
                    Some(ActiveModal::Question { draft, .. }) => draft
                        .custom_inputs
                        .get(step)
                        .map(|input| input.text.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                });
                view! {
                    <input
                        class="input"
                        placeholder="Other (type your own)"
                        prop:value=custom
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            store.update(|s| {
                                if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal
                                    && let Some(input) = draft.custom_inputs.get_mut(step)
                                {
                                    input.replace_text(value);
                                }
                            });
                        }
                    />
                }
            }}
        </div>
    }
    .into_any();

    let actions = view! {
        {move || {
            let Some((session_id, request, step, _, _)) = q_state.get() else {
                return view! { <span></span> }.into_any();
            };
            let total = request.questions.len();
            let sid_submit = session_id.clone();
            let req_submit = request.clone();
            let sid_cancel = session_id;
            let rid_cancel = request.request_id;
            view! {
                <button class="btn" disabled={step == 0} on:click=move |_| {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            draft.step = draft.step.saturating_sub(1);
                            draft.focused_option = 0;
                        }
                    });
                }>"Back"</button>
                <button class="btn" disabled={step + 1 >= total} on:click=move |_| {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            let next = draft.step + 1;
                            if next < draft.answers.len() {
                                draft.step = next;
                                draft.focused_option = 0;
                            }
                        }
                    });
                }>"Next"</button>
                <button class="btn primary" on:click=move |_| {
                    let answers = store.with(|s| match &s.modal {
                        Some(ActiveModal::Question { draft, .. }) => build_question_answers(&req_submit, draft),
                        _ => Vec::new(),
                    });
                    if let Some(t) = crate::transport::current() {
                        t.answer_question(sid_submit.clone(), req_submit.request_id.clone(), answers);
                    }
                    store.update(|s| s.modal = None);
                }>"Submit answers"</button>
                <button class="btn danger" on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.cancel_question(sid_cancel.clone(), rid_cancel.clone());
                    }
                    store.update(|s| s.modal = None);
                }>"Cancel"</button>
            }
            .into_any()
        }}
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Question { request, draft, .. }) => {
                let step = draft.step.min(request.questions.len().saturating_sub(1));
                request.questions.get(step).map_or_else(
                    || "Question".to_owned(),
                    |q| format!("Question — {}", q.header),
                )
            }
            _ => "Question".to_owned(),
        })
    };
    shell_reactive(title, store, body, actions)
}

#[component]
fn TerminalModal(store: StoreHandle) -> impl IntoView {
    let status_memo = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Terminal {
                output,
                running,
                error,
                ..
            }) => (output.clone(), *running, error.clone()),
            _ => (None, false, None),
        })
    });
    let initial_cmd = store.with(|s| match &s.modal {
        Some(ActiveModal::Terminal { command, .. }) => command.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            <label class="field">
                <span class="field-label">"Command"</span>
                <input
                    class="input"
                    placeholder="Command, without the leading !"
                    prop:value=initial_cmd
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::Terminal { command, .. }) = &mut s.modal {
                                command.replace_text(value);
                            }
                        });
                    }
                />
            </label>
            {move || {
                let (output, running, error) = status_memo.get();
                view! {
                    <div class="modal-form">
                        {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                        {output.map(|output| view! {
                            <div class="field">
                                <span class="field-label">"Output"</span>
                                <pre class="terminal-output">{output}</pre>
                            </div>
                        }.into_any())}
                        <div class="hint">
                            {if running {
                                "Running. You can close this dialog; the result lands in the transcript."
                            } else {
                                "Press Enter to run. Uses the same sandbox, environment, and timeout guards as the TUI."
                            }}
                        </div>
                    </div>
                }
            }}
        </div>
    }
    .into_any();
    let actions = view! {
        <button
            class="btn primary"
            disabled=move || status_memo.get().1
            on:click=move |_| {
                let command = store.with(|s| match &s.modal {
                    Some(ActiveModal::Terminal { command, .. }) => command.text.clone(),
                    _ => String::new(),
                });
                if !command.trim().is_empty() {
                    store.update(|s| crate::state::execute_user_shell(s, command.trim()));
                }
            }
        >"Run"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
    }
    .into_any();
    shell("Terminal", store, body, actions)
}

#[component]
fn QueuedEditModal(store: StoreHandle) -> impl IntoView {
    let (index, initial) = store.with(|s| match &s.modal {
        Some(ActiveModal::QueuedEdit { index, input, .. }) => (*index, input.text.clone()),
        _ => (0, String::new()),
    });
    let error = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::QueuedEdit { error, .. }) => error.clone(),
            _ => None,
        })
    });
    let body = view! {
        <div class="modal-form">
            <label class="field">
                <span class="field-label">{format!("Queued message {}", index + 1)}</span>
                <textarea
                    class="input"
                    rows="4"
                    placeholder="Queued prompt"
                    prop:value=initial
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::QueuedEdit { input, error, .. }) = &mut s.modal {
                                input.replace_text(value);
                                *error = None;
                            }
                        });
                    }
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                            ev.prevent_default();
                            store.update(crate::state::commit_queued_edit);
                        }
                    }
                ></textarea>
            </label>
            {move || error.get().map(|msg| view! { <div class="banner error">{msg}</div> }.into_any())}
            <div class="hint">"Press Enter to save (Shift+Enter for newline), or Esc to discard."</div>
        </div>
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
        <button class="btn primary" on:click=move |_| {
            store.update(crate::state::commit_queued_edit);
        }>"Save"</button>
    }
    .into_any();
    shell("Edit queued message", store, body, actions)
}

#[component]
fn RewindModal(store: StoreHandle) -> impl IntoView {
    let rewind_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::RewindMenu {
                turns,
                selected_turn,
                scope,
            }) => (turns.clone(), *selected_turn, *scope),
            _ => (Vec::new(), 0, RewindScopeDto::Both),
        })
    });
    let body = view! {
        {move || {
            let (turns, selected_turn, scope) = rewind_state.get();
            let rows = turns
                .iter()
                .enumerate()
                .map(|(index, (turn_id, label))| {
                    let turn = turn_id.clone();
                    let is_selected = index == selected_turn;
                    view! {
                        <button class="pick-row" class:active=is_selected on:click=move |_| {
                            let turn = turn.clone();
                            store.update(|s| {
                                if let Some(ActiveModal::RewindMenu { turns, selected_turn, .. }) = &mut s.modal
                                    && let Some(index) = turns.iter().position(|(id, _)| id == &turn)
                                {
                                    *selected_turn = index;
                                }
                            });
                        }>
                            <span class="pick-title">{label.clone()}</span>
                            <span class="pick-meta">{turn_id.clone()}</span>
                        </button>
                    }
                    .into_any()
                })
                .collect::<Vec<_>>();
            view! {
                <div class="modal-form">
                    <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                    <div class="choice-grid">
                        <button class="choice" class:active=scope == RewindScopeDto::Both on:click=move |_| set_scope(&store, RewindScopeDto::Both)>
                            <span class="choice-title">"Code + conversation"</span>
                        </button>
                        <button class="choice" class:active=scope == RewindScopeDto::Code on:click=move |_| set_scope(&store, RewindScopeDto::Code)>
                            <span class="choice-title">"Code only"</span>
                        </button>
                        <button class="choice" class:active=scope == RewindScopeDto::Conversation on:click=move |_| set_scope(&store, RewindScopeDto::Conversation)>
                            <span class="choice-title">"Conversation only"</span>
                        </button>
                    </div>
                </div>
            }
        }}
    }
    .into_any();
    let actions = view! {
        <button class="btn primary" on:click=move |_| {
            let target = store.with(|s| match &s.modal {
                Some(ActiveModal::RewindMenu { turns, selected_turn, scope }) => {
                    turns.get(*selected_turn).map(|(id, _)| (id.clone(), *scope))
                }
                _ => None,
            });
            if let Some((turn_id, scope)) = target
                && let (Some(session_id), Some(t)) = (store.with(|s| s.active_session_id.clone()), crate::transport::current())
            {
                t.rewind_session(session_id, turn_id, scope);
            }
            store.update(|s| s.modal = None);
        }>"Rewind"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
    }
    .into_any();
    shell("Rewind", store, body, actions)
}

#[component]
fn CommandPaletteModal(store: StoreHandle) -> impl IntoView {
    let list_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::CommandPalette { filter, selected }) => {
                (filter.text.clone(), *selected)
            }
            _ => (String::new(), 0),
        })
    });
    let initial_filter = store.with(|s| match &s.modal {
        Some(ActiveModal::CommandPalette { filter, .. }) => filter.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            <input
                class="input"
                placeholder="Search commands and actions"
                prop:value=initial_filter
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::CommandPalette { filter, selected }) = &mut s.modal {
                            filter.replace_text(value);
                            *selected = 0;
                        }
                    });
                }
            />
            {move || {
                let (filter_text, selected) = list_state.get();
                let commands = filtered_commands(&filter_text);
                let actions = filtered_palette_actions(&filter_text);
                let cmd_len = commands.len();
                let rows = commands
                    .into_iter()
                    .enumerate()
                    .map(|(index, spec)| {
                        let cmd = spec.cmd.to_owned();
                        let takes_arg = spec.takes_arg;
                        let label = command_label(spec);
                        view! {
                            <button class="pick-row" class:active=index == selected on:click=move |_| {
                                if takes_arg {
                                    store.update_with_effects(|s, fx| {
                                        s.draft.replace_text(format!("{cmd} "));
                                        s.modal = None;
                                        update_completion(s);
                                        fx.focus("composer");
                                    });
                                } else {
                                    store.update(|s| {
                                        s.modal = None;
                                        execute_slash_command(s, &cmd);
                                    });
                                }
                            }>
                                <span class="pick-title">{label}</span>
                                <span class="pick-meta">{spec.desc.to_owned()}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                let action_rows = actions
                    .into_iter()
                    .enumerate()
                    .map(|(idx, action)| {
                        let key = action.key.to_owned();
                        let is_selected = cmd_len + idx == selected;
                        view! {
                            <button class="pick-row" class:active=is_selected on:click=move |_| {
                                let key = key.clone();
                                store.update_with_effects(|s, fx| {
                                    s.modal = None;
                                    execute_palette_action(s, fx, &key);
                                });
                            }>
                                <span class="pick-title">{action.label.to_owned()}</span>
                                <span class="pick-meta">{action.hint.to_owned()}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div class="modal-form">
                        <div class="palette-section">"Commands"</div>
                        <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                        <div class="palette-section">"Actions"</div>
                        <div class="pick-list">{AnyView::from(Fragment::new(action_rows))}</div>
                    </div>
                }
            }}
        </div>
    }
    .into_any();
    shell("Commands", store, body, empty_actions())
}

#[component]
fn DiffModal(store: StoreHandle) -> impl IntoView {
    let diff_state = Memo::new(move |_| {
        store.with(|s| {
            let files = s.file_changes.clone();
            match &s.modal {
                Some(ActiveModal::DiffView {
                    selected_path,
                    unified,
                    diff,
                    loading,
                    error,
                }) => (
                    files,
                    selected_path.clone(),
                    *unified,
                    diff.clone(),
                    *loading,
                    error.clone(),
                ),
                _ => (files, None, true, None, false, None),
            }
        })
    });
    let body = view! {
        {move || {
            let (files, selected_path, unified, diff, loading, error) = diff_state.get();
            diff_body(store, files, selected_path, unified, diff, loading, error)
        }}
    }
    .into_any();
    shell_wide("Diff", store, body, empty_actions())
}

#[component]
fn SearchModal(store: StoreHandle) -> impl IntoView {
    let search_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Search {
                results,
                selected,
                preview,
                loading,
                ..
            }) => Some((results.clone(), *selected, preview.clone(), *loading)),
            _ => None,
        })
    });
    let mode = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Search { mode, .. }) => *mode,
            _ => SearchModeDto::Content,
        })
    });
    let has_query = Memo::new(move |_| {
        store.with(|s| {
            matches!(&s.modal,
                Some(ActiveModal::Search { query, .. }) if !query.text.trim().is_empty()
            )
        })
    });
    let initial_query = store.with(|s| match &s.modal {
        Some(ActiveModal::Search { query, .. }) => query.text.clone(),
        _ => String::new(),
    });
    let debounce_id: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let debounce_cleanup = debounce_id.clone();
    on_cleanup(move || {
        if let Ok(mut guard) = debounce_cleanup.lock()
            && let Some(id) = guard.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(id);
        }
    });

    let mode_buttons = [
        (SearchModeDto::Content, "File contents"),
        (SearchModeDto::Files, "File names"),
    ]
    .into_iter()
    .map(|(candidate, label)| {
        let timer = debounce_id.clone();
        view! {
            <button type="button" class="search-mode"
                class:active=move || mode.get() == candidate
                aria-pressed=move || (mode.get() == candidate).to_string()
                on:click=move |_| if mode.get_untracked() != candidate {
                    clear_search_timer(&timer);
                    switch_search_mode(store, candidate);
                }
            >{label}</button>
        }
    })
    .collect::<Vec<_>>();
    let submit_timer = debounce_id.clone();
    let body = view! {
        <div class="search-form">
            <div class="search-modes" role="group" aria-label="Search scope">{mode_buttons}</div>
            <label class="field-label" for="workspace-query">"Search query"</label>
            <form class="search-bar" on:submit=move |ev| {
                ev.prevent_default();
                clear_search_timer(&submit_timer);
                switch_search_mode(store, mode.get_untracked());
            }>
                <input
                    class="input"
                    id="workspace-query"
                    autocomplete="off"
                    placeholder=move || if mode.get() == SearchModeDto::Files { "e.g. Cargo.toml" } else { "e.g. SessionStatus" }
                    prop:value=initial_query
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        let trimmed = value.trim().to_owned();
                        let mode = store.with(|s| match &s.modal {
                            Some(ActiveModal::Search { mode, .. }) => *mode,
                            _ => SearchModeDto::Content,
                        });
                        store.update(|s| {
                            if let Some(ActiveModal::Search { query, loading, results, preview, selected, .. }) = &mut s.modal {
                                query.replace_text(value);
                                *results = None;
                                *preview = None;
                                *selected = 0;
                                *loading = !trimmed.is_empty();
                            }
                        });
                        if let Some(window) = web_sys::window() {
                            clear_search_timer(&debounce_id);
                            if !trimmed.is_empty() {
                                let q = trimmed.clone();
                                let cb = Closure::once_into_js(move || {
                                    if let Some(t) = crate::transport::current() {
                                        t.search_workspace(mode, q);
                                    }
                                });
                                if let Some(func) = cb.dyn_ref::<js_sys::Function>()
                                    && let Ok(id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(func, 220)
                                    && let Ok(mut guard) = debounce_id.lock()
                                {
                                    *guard = Some(id);
                                }
                            }
                        }
                    }
                />
                <button class="btn primary" type="submit" disabled=move || !has_query.get()>"Search"</button>
            </form>
            {move || {
                let Some((results, selected, preview, loading)) = search_state.get() else {
                    return view! { <span></span> }.into_any();
                };
                let matches = results
                    .as_ref()
                    .map(|results| {
                        results
                            .matches
                            .iter()
                            .enumerate()
                            .map(|(index, result)| {
                                let path = result.path.clone();
                                let line = result.line;
                                let is_selected = index == selected;
                                let line_suffix = line.map(|l| format!(":{l}")).unwrap_or_default();
                                view! {
                                    <button class="pick-row search-result" class:active=is_selected aria-pressed=is_selected.to_string() on:click=move |_| {
                                        store.update(|s| {
                                            if let Some(ActiveModal::Search { selected, preview, .. }) = &mut s.modal {
                                                *selected = index;
                                                *preview = None;
                                            }
                                        });
                                        if let Some(t) = crate::transport::current() {
                                            t.preview_workspace(path.clone(), line);
                                        }
                                    }>
                                        <span class="pick-title">{format!("{}{line_suffix}", result.path)}</span>
                                        <span class="pick-meta">{result.snippet.clone()}</span>
                                    </button>
                                }
                                .into_any()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let preview_view = preview
                    .map(|preview| {
                        let lines = preview
                            .lines
                            .iter()
                            .map(|line| {
                                view! {
                                    <div class="preview-line" class:highlighted=line.highlighted>
                                        <span class="diff-no">{line.line_no}</span>
                                        <span>{line.text.clone()}</span>
                                    </div>
                                }
                                .into_any()
                            })
                            .collect::<Vec<_>>();
                        view! {
                            <div class="preview">
                                <div class="diff-title">{preview.title}</div>
                                {AnyView::from(Fragment::new(lines))}
                            </div>
                        }
                        .into_any()
                    })
                    .unwrap_or_else(|| {
                        view! { <div class="search-empty">"Select a result to see its file preview."</div> }.into_any()
                    });
                view! {
                    <div class="search-results">
                        <section class="split-side" aria-label="Search results">
                            <div class="search-results-head">"Results"</div>
                            {if matches.is_empty() {
                                let message = if loading {
                                    "Searching…"
                                } else if results.is_some() {
                                    "No matches found. Try a different query or scope."
                                } else {
                                    "Enter a query to search this workspace."
                                };
                                view! { <div class="search-empty" role="status">{message}</div> }.into_any()
                            } else {
                                AnyView::from(Fragment::new(matches))
                            }}
                        </section>
                        <section class="split-main" aria-label="File preview">
                            <div class="search-results-head">"Preview"</div>
                            {preview_view}
                        </section>
                    </div>
                }
                .into_any()
            }}
        </div>
    }
    .into_any();
    shell_wide("Search", store, body, empty_actions())
}

fn clear_search_timer(timer: &Mutex<Option<i32>>) {
    if let Ok(mut guard) = timer.lock()
        && let Some(id) = guard.take()
        && let Some(window) = web_sys::window()
    {
        window.clear_timeout_with_handle(id);
    }
}

fn switch_search_mode(store: StoreHandle, next: SearchModeDto) {
    let query = store.with(|s| match &s.modal {
        Some(ActiveModal::Search { query, .. }) => query.text.trim().to_owned(),
        _ => String::new(),
    });
    store.update(|s| {
        if let Some(ActiveModal::Search {
            mode,
            loading,
            results,
            preview,
            selected,
            ..
        }) = &mut s.modal
        {
            if *mode != next {
                *results = None;
                *preview = None;
                *selected = 0;
            }
            *mode = next;
            *loading = !query.is_empty();
        }
    });
    if !query.is_empty()
        && let Some(t) = crate::transport::current()
    {
        t.search_workspace(next, query);
    }
}

#[component]
fn SubagentModal(store: StoreHandle) -> impl IntoView {
    let sub_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::SubagentOverlay {
                entry_id,
                name,
                agent,
                status,
                child_session_id,
                entries,
                usage,
                loading,
                error,
                ..
            }) => {
                let live_status = s
                    .active_tab()
                    .and_then(|tab| {
                        tab.entries.iter().find_map(|e| match e {
                            meerai_web_protocol::TranscriptEntryDto::Subagent {
                                id,
                                status,
                                ..
                            } if id == entry_id => Some(status.clone()),
                            _ => None,
                        })
                    })
                    .unwrap_or_else(|| status.clone());
                Some((
                    name.clone(),
                    agent.clone(),
                    live_status,
                    child_session_id.clone(),
                    entries.clone(),
                    usage.clone(),
                    *loading,
                    error.clone(),
                ))
            }
            _ => None,
        })
    });

    let child_for_poll = store.with(|s| match &s.modal {
        Some(ActiveModal::SubagentOverlay {
            child_session_id, ..
        }) => child_session_id.clone(),
        _ => None,
    });

    if let Some(child_id) = child_for_poll {
        let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
        let cleanup_ref = interval_handle.clone();
        if let Some(window) = web_sys::window() {
            let cb = Closure::<dyn FnMut()>::new(move || {
                let should_poll = sub_state
                    .get_untracked()
                    .is_some_and(|(_, _, status, _, _, _, _, _)| status == "running");
                if should_poll && let Some(t) = crate::transport::current() {
                    t.fetch_session_events(child_id.clone(), true);
                }
            });
            if let Ok(id) = window.set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                1500,
            ) && let Ok(mut guard) = interval_handle.lock()
            {
                *guard = Some(id);
            }
            cb.forget();
        }
        on_cleanup(move || {
            if let Ok(mut guard) = cleanup_ref.lock()
                && let Some(id) = guard.take()
                && let Some(window) = web_sys::window()
            {
                window.clear_interval_with_handle(id);
            }
        });
    }

    let render_epoch = Memo::new(move |_| {
        store.with(|s| {
            s.active_tab()
                .map(|tab| {
                    (
                        tab.entries_version,
                        tab.expanded_tools.len(),
                        tab.expanded_reasoning.len(),
                        tab.expanded_subagents.len(),
                    )
                })
                .unwrap_or((0, 0, 0, 0))
        })
    });

    let body = view! {
        {move || {
            let _ = render_epoch.get();
            let Some((name, agent, status, child_session_id, entries, usage, loading, error)) = sub_state.get() else {
                return view! { <span></span> }.into_any();
            };
            let refresh_id = child_session_id.clone();
            view! {
                <div class="subagent-view">
                    <div class="subagent-meta">
                        <div class="kv"><span>"Agent"</span><strong>{name.clone().unwrap_or_else(|| agent.clone())}</strong></div>
                        {name.map(|_| view! { <div class="kv"><span>"Preset"</span><strong>{agent.clone()}</strong></div> })}
                        <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
                        {child_session_id.clone().map(|child| view! { <div class="kv"><span>"Child session"</span><code>{child}</code></div> }).into_any()}
                        {usage.map(|usage| view! {
                            <div class="kv"><span>"Usage"</span><strong>{format!(
                                "↑{} ↓{} · Σ{}",
                                format_tokens(usage.prompt_tokens.unwrap_or(0).max(0) as u64),
                                format_tokens(usage.completion_tokens.unwrap_or(0).max(0) as u64),
                                format_tokens(usage.total_tokens.unwrap_or(0).max(0) as u64),
                            )}</strong></div>
                        }.into_any())}
                        {refresh_id.map(|cid| view! {
                            <button class="btn tiny" on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.fetch_session_events(cid.clone(), true);
                                }
                            }>"Refresh"</button>
                        }.into_any())}
                    </div>
                    {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                    <Show when=move || loading>
                        <div class="hint">"Loading child transcript…"</div>
                    </Show>
                    {if entries.is_empty() && !loading {
                        view! { <div class="hint">"This agent has no conversation yet."</div> }.into_any()
                    } else {
                        crate::views::transcript::render_child_transcript(store, &entries)
                    }}
                </div>
            }
            .into_any()
        }}
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::SubagentOverlay { name, agent, .. }) => match name {
                Some(name) => format!("Agent — {name} ({agent})"),
                None => format!("Agent — {agent}"),
            },
            _ => "Subagent".to_owned(),
        })
    };
    shell_reactive_full(
        title,
        store,
        body,
        view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }
            .into_any(),
    )
}

#[component]
fn RunsModal(store: StoreHandle) -> impl IntoView {
    let now_ms = RwSignal::new(js_sys::Date::now() as u64);
    let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let interval_cleanup = interval_handle.clone();
    if let Some(window) = web_sys::window() {
        let cb = Closure::<dyn FnMut()>::new(move || now_ms.set(js_sys::Date::now() as u64));
        if let Ok(id) = window.set_interval_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            1000,
        ) && let Ok(mut guard) = interval_handle.lock()
        {
            *guard = Some(id);
        }
        cb.forget();
    }
    on_cleanup(move || {
        if let Ok(mut guard) = interval_cleanup.lock()
            && let Some(id) = guard.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_interval_with_handle(id);
        }
    });
    let runs_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Runs { kind, selected }) => {
                let agents = running_subagent_rows(s);
                let shells = s
                    .active_session()
                    .map(|session| session.shell_runs.clone())
                    .unwrap_or_default();
                Some((*kind, *selected, agents, shells))
            }
            _ => None,
        })
    });
    let body = view! {
        {move || {
            let Some((kind, selected, agents, shells)) = runs_state.get() else {
                return view! { <span></span> }.into_any();
            };
            match kind {
                RunKindDto::Agents => {
                    if agents.is_empty() {
                        return view! { <div class="hint">"No running agents"</div> }.into_any();
                    }
                    let rows = agents
                        .into_iter()
                        .enumerate()
                        .map(|(index, row)| {
                            let label = row.name.clone().unwrap_or_else(|| row.agent.clone());
                            let preset = row
                                .name
                                .as_ref()
                                .map(|_| row.agent.clone());
                            view! {
                                <button
                                    class="pick-row"
                                    class:active=index == selected
                                    on:click=move |_| store.update(|s| open_run_row(s, RunKindDto::Agents, index))
                                >
                                    <span class="pick-title">{label}</span>
                                    {preset.map(|preset| view! { <span class="pick-meta">{preset}</span> })}
                                    <span class="pick-meta">{row.status}</span>
                                    <span class="badge muted">{format!("{}/{} steps", row.steps_used, row.max_steps)}</span>
                                </button>
                            }
                            .into_any()
                        })
                        .collect::<Vec<_>>();
                    view! {
                        <div class="modal-form">
                            <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                        </div>
                    }
                    .into_any()
                }
                RunKindDto::Shells => {
                    if shells.is_empty() {
                        return view! { <div class="hint">"No running shells"</div> }.into_any();
                    }
                    let now = now_ms.get();
                    let rows = shells
                        .into_iter()
                        .enumerate()
                        .map(|(index, run)| {
                            let elapsed = format_duration_ms(now.saturating_sub(run.started_at_ms));
                            view! {
                                <button
                                    class="pick-row"
                                    class:active=index == selected
                                    on:click=move |_| store.update(|s| open_run_row(s, RunKindDto::Shells, index))
                                >
                                    <span class="pick-title">{format!("!{}", run.command)}</span>
                                    <span class="pick-meta">{elapsed}</span>
                                </button>
                            }
                            .into_any()
                        })
                        .collect::<Vec<_>>();
                    view! {
                        <div class="modal-form">
                            <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                        </div>
                    }
                    .into_any()
                }
            }
        }}
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Runs { kind, .. }) => match kind {
                RunKindDto::Agents => {
                    format!("Agents ({})", crate::state::running_subagent_count(s))
                }
                RunKindDto::Shells => format!("Shells ({})", crate::state::running_shell_count(s)),
            },
            _ => "Running work".to_owned(),
        })
    };
    shell_reactive(
        title,
        store,
        body,
        view! {
            <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
        }
        .into_any(),
    )
}

#[component]
fn ShellRunModal(store: StoreHandle) -> impl IntoView {
    let now_ms = RwSignal::new(js_sys::Date::now() as u64);
    let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let interval_cleanup = interval_handle.clone();
    if let Some(window) = web_sys::window() {
        let cb = Closure::<dyn FnMut()>::new(move || now_ms.set(js_sys::Date::now() as u64));
        if let Ok(id) = window.set_interval_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            1000,
        ) && let Ok(mut guard) = interval_handle.lock()
        {
            *guard = Some(id);
        }
        cb.forget();
    }
    on_cleanup(move || {
        if let Ok(mut guard) = interval_cleanup.lock()
            && let Some(id) = guard.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_interval_with_handle(id);
        }
    });
    let run_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ShellRun {
                run_id,
                command,
                started_at_ms,
                ..
            }) => {
                let running = s
                    .active_session()
                    .is_some_and(|session| session.shell_runs.iter().any(|run| &run.id == run_id));
                let output = s.shell_outputs.get(run_id).cloned();
                Some((command.clone(), *started_at_ms, running, output))
            }
            _ => None,
        })
    });
    let back_label = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ShellRun {
                from_runs: Some(_), ..
            }) => "Back",
            _ => "Close",
        })
    });
    let body = view! {
        {move || {
            let Some((command, started_at_ms, running, output)) = run_state.get() else {
                return view! { <span></span> }.into_any();
            };
            let status = if running {
                let elapsed = now_ms.get();
                format!(
                    "running · {}",
                    format_duration_ms(elapsed.saturating_sub(started_at_ms))
                )
            } else if matches!(&output, Some(Err(_))) {
                "failed".to_owned()
            } else {
                "finished".to_owned()
            };
            let output_view = match output {
                Some(Ok(out)) => view! {
                    <div class="field">
                        <span class="field-label">"Output"</span>
                        <pre class="terminal-output">{out}</pre>
                    </div>
                }
                .into_any(),
                Some(Err(err)) => view! { <div class="banner error">{err}</div> }.into_any(),
                None if running => {
                    view! { <div class="hint">"waiting for output…"</div> }.into_any()
                }
                None => view! {
                    <div class="hint">"finished; output is in the transcript"</div>
                }
                .into_any(),
            };
            view! {
                <div class="modal-form">
                    <div class="kv"><span>"Command"</span><code>{format!("!{command}")}</code></div>
                    <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
                    {output_view}
                </div>
            }
            .into_any()
        }}
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| {
            store.update(|s| {
                let target = match &s.modal {
                    Some(ActiveModal::ShellRun { from_runs, .. }) => *from_runs,
                    _ => None,
                };
                if let Some(kind) = target {
                    back_to_runs(s, kind);
                } else {
                    s.modal = None;
                }
            });
        }>{move || back_label.get()}</button>
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ShellRun { command, .. }) => {
                format!("Shell — !{}", super::truncate(command, 40))
            }
            _ => "Shell".to_owned(),
        })
    };
    shell_reactive(title, store, body, actions)
}

fn shell(title: &str, store: StoreHandle, body: AnyView, actions: AnyView) -> AnyView {
    shell_with_width(title, store, body, actions, false)
}

fn shell_wide(title: &str, store: StoreHandle, body: AnyView, actions: AnyView) -> AnyView {
    shell_with_width(title, store, body, actions, true)
}

fn shell_reactive(
    title: impl Fn() -> String + Send + 'static,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
) -> AnyView {
    shell_reactive_class(title, store, body, actions, "modal")
}

fn shell_reactive_full(
    title: impl Fn() -> String + Send + 'static,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
) -> AnyView {
    shell_reactive_class(title, store, body, actions, "modal modal-full")
}

fn shell_reactive_class(
    title: impl Fn() -> String + Send + 'static,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
    class: &'static str,
) -> AnyView {
    view! {
        <div
            class="modal-backdrop"
            on:click=move |ev| {
                if ev.target() == ev.current_target() {
                    store.update(|s| s.modal = None);
                }
            }
        >
            <div class=class role="dialog" aria-modal="true" aria-labelledby="dialog-title" tabindex="-1">
                <div class="modal-head">
                    <h2 id="dialog-title">{move || title()}</h2>
                    <button class="modal-close" aria-label="Close dialog" title="Close (Esc)" on:click=move |_| store.update(|s| s.modal = None)>"×"</button>
                </div>
                <div class="modal-body">{body}</div>
                <div class="modal-actions">{actions}</div>
            </div>
        </div>
    }
    .into_any()
}

fn shell_with_width(
    title: &str,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
    wide: bool,
) -> AnyView {
    let class = if wide { "modal modal-wide" } else { "modal" };
    view! {
        <div
            class="modal-backdrop"
            on:click=move |ev| {
                if ev.target() == ev.current_target() {
                    store.update(|s| s.modal = None);
                }
            }
        >
            <div class=class role="dialog" aria-modal="true" aria-labelledby="dialog-title" tabindex="-1">
                <div class="modal-head">
                    <h2 id="dialog-title">{title.to_owned()}</h2>
                    <button class="modal-close" aria-label="Close dialog" title="Close (Esc)" on:click=move |_| store.update(|s| s.modal = None)>"×"</button>
                </div>
                <div class="modal-body">{body}</div>
                <div class="modal-actions">{actions}</div>
            </div>
        </div>
    }
    .into_any()
}

fn empty_actions() -> AnyView {
    view! { <span></span> }.into_any()
}

fn find_provider(store: &StoreHandle, id: &str) -> Option<ProviderItemDto> {
    store.with(|s| {
        s.providers
            .as_ref()
            .and_then(|providers| providers.providers.iter().find(|p| p.id == id).cloned())
    })
}

fn find_provider_state<'a>(
    state: &'a crate::state::AppState,
    id: &str,
) -> Option<&'a ProviderItemDto> {
    state
        .providers
        .as_ref()
        .and_then(|providers| providers.providers.iter().find(|p| p.id == id))
}

fn provider_row(
    store: StoreHandle,
    provider: ProviderItemDto,
    selected: bool,
    is_active: bool,
) -> AnyView {
    let click = provider.clone();
    let mut meta_parts = vec![provider.id.clone(), provider.adapter.clone()];
    if let Some(url) = &provider.base_url
        && !url.is_empty()
    {
        meta_parts.push(url.clone());
    } else if !provider.configured && !provider.env_vars.is_empty() {
        meta_parts.push(format!("env: {}", provider.env_vars.join(" / ")));
    }
    view! {
        <button class="pick-row" class:active=selected on:click=move |_| {
            let provider = click.clone();
            store.update(|s| {
                activate_provider_from_modal(s, provider, ProviderModalReturn::ProviderSelect);
            });
        }>
            <span class="pick-title">{provider.display_name.clone()}</span>
            <span class="pick-meta">{meta_parts.join(" · ")}</span>
            {is_active.then(|| view! { <span class="badge ok">"active"</span> }.into_any())}
            {provider.is_custom.then(|| view! { <span class="badge">"custom"</span> }.into_any())}
            <span class=format!("badge {}", if provider.configured { "ok" } else { "muted" })>
                {provider.status_badge.clone()}
            </span>
        </button>
    }
    .into_any()
}

fn model_row(
    store: StoreHandle,
    model: ModelItemDto,
    selected: bool,
    current: bool,
    recent: bool,
    provider_status: Option<(String, bool)>,
) -> AnyView {
    let click_model = model.clone();
    let forget_model = model.clone();
    let ctx_label = model
        .context_window_tokens
        .map(|c| format!("{}k ctx", c / 1000))
        .unwrap_or_default();
    let caps = [
        model.supports_reasoning.then_some("reasoning"),
        model.supports_vision.then_some("vision"),
        model.supports_tools.then_some("tools"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut parts = Vec::new();
    if model.display_name != model.id {
        parts.push(model.display_name.clone());
    }
    if !ctx_label.is_empty() {
        parts.push(ctx_label);
    }
    if !caps.is_empty() {
        parts.push(caps);
    }
    let meta = parts.join(" · ");
    let row_content = view! {
        <span class="pick-title">{format!("{}/{}", model.provider, model.id)}</span>
        <span class="pick-meta">{meta}</span>
        {recent.then(|| view! { <span class="badge">"recent"</span> }.into_any())}
        {current.then(|| view! { <span class="badge ok">"active"</span> }.into_any())}
        {provider_status.map(|(badge, configured)| view! {
            <span class=format!("badge {}", if configured { "ok" } else { "warn" })>{badge}</span>
        }.into_any())}
    };
    let select_model = move || {
        let model = click_model.clone();
        if let (Some(id), Some(t)) = (
            store.with(|s| s.active_session_id.clone()),
            crate::transport::current(),
        ) {
            t.set_model(id, model.provider.clone(), model.id.clone());
        }
        store.update(|s| s.modal = None);
    };
    if recent {
        view! {
            <div class="pick-row" class:active=selected || current>
                <button class="pick-main" on:click=move |_| select_model()>
                    {row_content}
                </button>
                <button
                    class="badge warn pick-remove"
                    title="Remove this model from saved history"
                    on:click=move |_| {
                        let target = forget_model.clone();
                        store.update(move |s| {
                            crate::state::forget_model(s, &target.provider, &target.id)
                        });
                    }
                >"Forget"</button>
            </div>
        }
        .into_any()
    } else {
        view! {
            <button class="pick-row" class:active=selected || current on:click=move |_| select_model()>
                {row_content}
            </button>
        }
        .into_any()
    }
}

fn decide(
    store: &StoreHandle,
    session_id: &str,
    request_id: &str,
    decision: ApprovalDecisionDto,
    scope: Option<GrantScopeDto>,
) {
    if let Some(t) = crate::transport::current() {
        t.decide_approval(
            session_id.to_owned(),
            request_id.to_owned(),
            decision,
            scope,
        );
    }
    store.update(|s| s.modal = None);
}

fn set_scope(store: &StoreHandle, next: RewindScopeDto) {
    store.update(|s| {
        if let Some(ActiveModal::RewindMenu { scope, .. }) = &mut s.modal {
            *scope = next;
        }
    });
}

fn help_modal(store: StoreHandle) -> AnyView {
    let commands = filtered_commands("")
        .into_iter()
        .map(|spec| {
            view! {
                <div class="help-row">
                    <code>{command_label(spec)}</code>
                    <span>{spec.desc.to_owned()}</span>
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let body = view! {
        <div class="modal-form">
            <h3>"Commands"</h3>
            <div class="help-list">{AnyView::from(Fragment::new(commands))}</div>
            <h3>"Browser Shortcuts"</h3>
            <div class="help-list">
                <div class="help-row"><code>"Alt+K"</code><span>"Command palette"</span></div>
                <div class="help-row"><code>"Alt+A"</code><span>"Focus composer"</span></div>
                <div class="help-row"><code>"Alt+[ / Alt+]"</code><span>"Previous / next tab"</span></div>
                <div class="help-row"><code>"Alt+Shift+[ / ]"</code><span>"Move tab left / right"</span></div>
                <div class="help-row"><code>"Alt+1…9"</code><span>"Select tab"</span></div>
                <div class="help-row"><code>"Alt+W"</code><span>"Close tab view"</span></div>
                <div class="help-row"><code>"Ctrl+B"</code><span>"Toggle sidebar"</span></div>
                <div class="help-row"><code>"Alt+M"</code><span>"Cycle approval mode"</span></div>
                <div class="help-row"><code>"Alt+S"</code><span>"Reveal and focus the Sessions sidebar"</span></div>
                <div class="help-row"><code>"Alt+L"</code><span>"Model picker"</span></div>
                <div class="help-row"><code>"Alt+Shift+D"</code><span>"Workspace diff"</span></div>
                <div class="help-row"><code>"Alt+Shift+F"</code><span>"Workspace search"</span></div>
                <div class="help-row"><code>"Alt+G"</code><span>"Steer oldest queued prompt or current draft"</span></div>
                <div class="help-row"><code>"Alt+X"</code><span>"Cancel the running turn"</span></div>
                <div class="help-row"><code>"Alt+C"</code><span>"Copy the latest answer"</span></div>
                <div class="help-row"><code>"y / t / a / n / r / c"</code><span>"Approval decisions (in approval dialog)"</span></div>
                <div class="help-row"><code>"Esc"</code><span>"Leave reason edit or close dialog"</span></div>
                <div class="help-row"><code>"?"</code><span>"This help"</span></div>
            </div>
            <div class="hint">"Alt is Option on macOS. Shortcuts use physical keys so Option+<letter> works reliably."</div>
        </div>
    }
    .into_any();
    shell(
        "Help",
        store,
        body,
        view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }
            .into_any(),
    )
}

fn diff_body(
    store: StoreHandle,
    files: Vec<meerai_web_protocol::FileChangeDto>,
    selected_path: Option<String>,
    unified: bool,
    diff: Option<FileDiffResponse>,
    loading: bool,
    error: Option<String>,
) -> AnyView {
    let file_count = files.len();
    let file_rows = files
        .into_iter()
        .map(|change| {
            let path = change.path.clone();
            let status = change.status.clone();
            let badge_class = format!("diff-badge {}", status.to_ascii_lowercase());
            let active = selected_path.as_deref() == Some(change.path.as_str());
            let click_status = status.clone();
            view! {
                <button class="diff-file-row" class:active=active on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.fetch_workspace_diff(path.clone(), Some(click_status.clone()));
                    }
                    store.update(|s| {
                        if let Some(ActiveModal::DiffView { selected_path, loading, error, .. }) = &mut s.modal {
                            *selected_path = Some(path.clone());
                            *loading = true;
                            *error = None;
                        }
                    });
                }>
                    <span class=badge_class>{status}</span>
                    <span class="diff-file-name">{change.path}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();

    let diff_view = if let Some(diff) = diff {
        let path = diff.path.clone();
        let status = diff.status.clone();
        let badge_class = format!("diff-badge {}", status.to_ascii_lowercase());
        let additions = diff.additions;
        let deletions = diff.deletions;
        let rows = if unified {
            render_unified_rows(&diff)
        } else {
            render_split_rows(&diff)
        };
        let scroll_class = if unified {
            "diff-scroll diff-unified"
        } else {
            "diff-scroll diff-split"
        };
        view! {
            <div class="diff-head">
                <div class="diff-title-group">
                    <span class="diff-path">{path}</span>
                    <span class=badge_class>{status}</span>
                </div>
                <div class="diff-stats">
                    <span class="diff-add">"+"{additions}</span>
                    <span class="diff-del">"-"{deletions}</span>
                </div>
            </div>
            <div class="diff-toolbar">
                <button class="btn tiny" class:active=unified on:click=move |_| set_diff_mode(store, true)>"Unified"</button>
                <button class="btn tiny" class:active=!unified on:click=move |_| set_diff_mode(store, false)>"Split"</button>
                <span class="diff-hint">"Click a file on the left to switch."</span>
            </div>
            <div class=scroll_class>{AnyView::from(Fragment::new(rows))}</div>
        }
        .into_any()
    } else if loading {
        view! { <div class="diff-empty">"Loading diff…"</div> }.into_any()
    } else if let Some(error) = error {
        view! { <div class="diff-empty banner error">{error}</div> }.into_any()
    } else {
        view! { <div class="diff-empty">"Select a changed file on the left."</div> }.into_any()
    };

    view! {
        <div class="diff-layout">
            <aside class="diff-files">
                <div class="diff-files-head">
                    <span>"Changed files"</span>
                    <div class="side-head-actions">
                        <span class="badge">{file_count}</span>
                        <button
                            class="btn tiny"
                            title="Refresh git diff"
                            on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.fetch_workspace_changes();
                                }
                            }
                        >"Refresh"</button>
                    </div>
                </div>
                <div class="diff-files-list">{AnyView::from(Fragment::new(file_rows))}</div>
            </aside>
            <section class="diff-main">{diff_view}</section>
        </div>
    }
    .into_any()
}

fn set_diff_mode(store: StoreHandle, next: bool) {
    store.update(|s| {
        if let Some(ActiveModal::DiffView { unified, .. }) = &mut s.modal {
            *unified = next;
        }
    });
}

fn render_unified_rows(diff: &FileDiffResponse) -> Vec<AnyView> {
    let mut rows = Vec::new();
    for row in &diff.rows {
        match &row.kind {
            meerai_web_protocol::DiffRowKindDto::HunkHeader { text } => {
                rows.push(diff_line("diff-hunk", "", "", text));
            }
            meerai_web_protocol::DiffRowKindDto::Context => {
                if let Some(cell) = row.right.as_ref().or(row.left.as_ref()) {
                    rows.push(diff_line(
                        "diff-context",
                        &line_no(cell.line_no),
                        " ",
                        &cell.text,
                    ));
                }
            }
            meerai_web_protocol::DiffRowKindDto::Added => {
                if let Some(cell) = row.right.as_ref() {
                    rows.push(diff_line(
                        "diff-added",
                        &line_no(cell.line_no),
                        "+",
                        &cell.text,
                    ));
                }
            }
            meerai_web_protocol::DiffRowKindDto::Removed => {
                if let Some(cell) = row.left.as_ref() {
                    rows.push(diff_line(
                        "diff-removed",
                        &line_no(cell.line_no),
                        "-",
                        &cell.text,
                    ));
                }
            }
            meerai_web_protocol::DiffRowKindDto::Modified => {
                if let Some(cell) = row.left.as_ref() {
                    rows.push(diff_line(
                        "diff-removed",
                        &line_no(cell.line_no),
                        "-",
                        &cell.text,
                    ));
                }
                if let Some(cell) = row.right.as_ref() {
                    rows.push(diff_line(
                        "diff-added",
                        &line_no(cell.line_no),
                        "+",
                        &cell.text,
                    ));
                }
            }
        }
    }
    rows
}

fn render_split_rows(diff: &FileDiffResponse) -> Vec<AnyView> {
    let mut rows = Vec::new();
    for row in &diff.rows {
        match &row.kind {
            meerai_web_protocol::DiffRowKindDto::HunkHeader { text } => {
                rows.push(view! { <div class="diff-split-hunk">{text.clone()}</div> }.into_any());
            }
            kind => {
                let class = match kind {
                    meerai_web_protocol::DiffRowKindDto::Added => "diff-added",
                    meerai_web_protocol::DiffRowKindDto::Removed => "diff-removed",
                    meerai_web_protocol::DiffRowKindDto::Modified => "diff-modified",
                    _ => "diff-context",
                };
                let (left_no, left_text) = row
                    .left
                    .as_ref()
                    .map(|cell| (line_no(cell.line_no), cell.text.clone()))
                    .unwrap_or_default();
                let (right_no, right_text) = row
                    .right
                    .as_ref()
                    .map(|cell| (line_no(cell.line_no), cell.text.clone()))
                    .unwrap_or_default();
                rows.push(
                    view! {
                        <div class=format!("diff-row {class}")>
                            <span class="diff-no">{left_no}</span>
                            <span class="diff-cell">{left_text}</span>
                            <span class="diff-no">{right_no}</span>
                            <span class="diff-cell">{right_text}</span>
                        </div>
                    }
                    .into_any(),
                );
            }
        }
    }
    rows
}

fn diff_line(class: &str, no: &str, sign: &str, text: &str) -> AnyView {
    view! {
        <div class=format!("udiff-row {class}")>
            <span class="udiff-no">{no.to_owned()}</span>
            <span class="udiff-sign">{sign.to_owned()}</span>
            <span class="udiff-code">{text.to_owned()}</span>
        </div>
    }
    .into_any()
}

fn line_no(value: Option<usize>) -> String {
    value.map(|line| line.to_string()).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
struct PanelSnapshot {
    tab: crate::state::WorkspacePanelTab,
    context: Option<meerai_web_protocol::ContextBreakdownDto>,
    goal: Option<meerai_web_protocol::GoalDto>,
    memories: Vec<meerai_web_protocol::MemoryDto>,
    routes: Vec<meerai_web_protocol::RouteDto>,
    packs: Vec<meerai_web_protocol::PackDto>,
    definitions: meerai_web_protocol::DefinitionsDiagnosticsDto,
    selected: usize,
    loading: bool,
    error: Option<String>,
    subagent_name: String,
    agent: String,
    task: String,
    model: String,
    mode: Option<meerai_web_protocol::ApprovalModeDto>,
    route_input: String,
    pack_input: String,
    pack_pin_input: String,
    subagent_steps: String,
    subagent_tokens: String,
    subagent_cost: String,
    subagent_tools: String,
    context_turn: usize,
    context_focus: String,
    context_blob: Option<(String, String)>,
}

#[component]
fn WorkspacePanelModal(store: StoreHandle) -> impl IntoView {
    let snapshot = Memo::new(move |_| {
        store.with(|state| match &state.modal {
            Some(ActiveModal::WorkspacePanel {
                tab,
                context,
                goal,
                memories,
                routes,
                packs,
                definitions,
                selected,
                loading,
                error,
                subagent_name,
                subagent_agent,
                subagent_task,
                subagent_model,
                subagent_mode,
                route_input,
                pack_input,
                context_turn,
                context_focus,
                context_blob,
                subagent_steps,
                subagent_tokens,
                subagent_cost,
                subagent_tools,
                pack_pin_input,
                ..
            }) => Some(PanelSnapshot {
                tab: *tab,
                context: context.as_deref().cloned(),
                goal: goal.as_deref().cloned(),
                memories: memories.clone(),
                routes: routes.clone(),
                packs: packs.clone(),
                definitions: (**definitions).clone(),
                selected: *selected,
                loading: *loading,
                error: error.clone(),
                subagent_name: subagent_name.text.clone(),
                agent: subagent_agent.clone(),
                task: subagent_task.text.clone(),
                model: subagent_model.text.clone(),
                mode: *subagent_mode,
                route_input: route_input.text.clone(),
                pack_input: pack_input.text.clone(),
                pack_pin_input: pack_pin_input.text.clone(),
                subagent_steps: subagent_steps.text.clone(),
                subagent_tokens: subagent_tokens.text.clone(),
                subagent_cost: subagent_cost.text.clone(),
                subagent_tools: subagent_tools.text.clone(),
                context_turn: *context_turn,
                context_focus: context_focus.text.clone(),
                context_blob: context_blob.clone(),
            }),
            _ => None,
        })
    });

    let reload = {
        let store = store;
        move |tab: crate::state::WorkspacePanelTab| {
            if tab == crate::state::WorkspacePanelTab::Goal {
                return;
            }
            if let Some(transport) = crate::transport::current() {
                match tab {
                    crate::state::WorkspacePanelTab::Context => {
                        transport.run_host_action("/context".to_owned())
                    }
                    crate::state::WorkspacePanelTab::Memory => transport.fetch_memories(),
                    crate::state::WorkspacePanelTab::Routing => transport.fetch_routes(),
                    crate::state::WorkspacePanelTab::Packs => {
                        transport.fetch_packs();
                        transport.fetch_definitions();
                    }
                    _ => {}
                }
            }
            store.update(|state| {
                if let Some(ActiveModal::WorkspacePanel { loading, error, .. }) =
                    state.modal.as_mut()
                {
                    *loading = true;
                    *error = None;
                }
            });
        }
    };

    let reload = std::sync::Arc::new(reload);
    let close = {
        let store = store;
        move |_| store.update(|state| state.modal = None)
    };

    view! {
        <div class="modal-backdrop">
            <div class="modal modal-wide">
                {move || {
                    let Some(PanelSnapshot {
                        tab,
                        context,
                        goal,
                        memories,
                        routes,
                        packs,
                        definitions,
                        selected,
                        loading,
                        error,
                        subagent_name,
                        agent,
                        task,
                        model,
                        mode,
                        route_input,
                        pack_input,
                        pack_pin_input,
                        context_turn,
                        context_focus,
                        context_blob,
                        subagent_steps,
                        subagent_tokens,
                        subagent_cost,
                        subagent_tools,
                    }) = snapshot.get() else {
                        return view! { <span></span> }.into_any();
                    };
                    let selected = if tab == crate::state::WorkspacePanelTab::Memory {
                        selected.min(memories.len().saturating_sub(1))
                    } else if tab == crate::state::WorkspacePanelTab::Routing {
                        selected.min(routes.len().saturating_sub(1))
                    } else if tab == crate::state::WorkspacePanelTab::Packs {
                        selected.min(packs.len().saturating_sub(1))
                    } else {
                        0
                    };
                    let reload = reload.clone();
                    let body = match tab {
                        crate::state::WorkspacePanelTab::Goal => {
                            let Some(g) = goal.as_ref() else {
                                return view! { <pre class="panel-text">"No active goal. Type @goal <instruction> in the composer to start one."</pre> }.into_any();
                            };
                            let mut text = String::new();
                            if let Some(goal) = g.goal.as_deref() {
                                text.push_str(&format!("Goal: {goal}\n"));
                            }
                            if !g.done_when.is_empty() {
                                text.push_str("Done when:\n");
                                for line in &g.done_when {
                                    text.push_str(&format!("- {line}\n"));
                                }
                            }
                            if !g.constraints.is_empty() {
                                text.push_str("Constraints:\n");
                                for line in &g.constraints {
                                    text.push_str(&format!("- {line}\n"));
                                }
                            }
                            if g.items.is_empty() {
                                text.push_str("\nItems: (none)\n");
                            } else {
                                text.push_str("\nItems:\n");
                                for item in &g.items {
                                    let mark = match item.status {
                                        meerai_web_protocol::GoalItemStatusDto::Done => "x",
                                        meerai_web_protocol::GoalItemStatusDto::Failed => "!",
                                        meerai_web_protocol::GoalItemStatusDto::Open => " ",
                                    };
                                    text.push_str(&format!("- [{mark}] {}. {}", item.id, item.text));
                                    if let Some(verify) = item.verify.as_deref() {
                                        text.push_str(&format!(" (verify: {verify})"));
                                    }
                                    if let Some(evidence) = item.evidence.as_deref() {
                                        text.push_str(&format!("\n  evidence: {evidence}"));
                                    }
                                    text.push('\n');
                                }
                            }
                            let done = g
                                .items
                                .iter()
                                .filter(|item| {
                                    matches!(
                                        item.status,
                                        meerai_web_protocol::GoalItemStatusDto::Done
                                    )
                                })
                                .count();
                            text.push_str(&format!(
                                "\nProgress: {done}/{} done",
                                g.items.len()
                            ));
                            if g.rounds > 0 {
                                text.push_str(&format!(" · round {}", g.rounds));
                            }
                            if g.turns > 0 {
                                text.push_str(&format!(" · turn {}", g.turns));
                            }
                            view! { <pre class="panel-text">{text}</pre> }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Context => {
                            let Some(c) = context.as_ref() else {
                                return view! { <pre class="panel-text">"No context report yet. Press Refresh."</pre> }.into_any();
                            };
                            let summary = format!(
                                "used {} / {} tokens ({:.1}%) · watermark {}\nsystem {} · tools {} · memory {} · user {} · assistant {}\ntool results {} · reasoning {} · summaries {}\npinned {} · cleared results {} · spilled {} · cached {}",
                                c.estimated_total_tokens, c.context_window_tokens,
                                if c.context_window_tokens == 0 { 0.0 } else { c.estimated_total_tokens as f64 * 100.0 / c.context_window_tokens as f64 },
                                c.high_watermark_tokens, c.system_prompt_tokens, c.tool_schemas_tokens,
                                c.memory_tokens, c.user_messages_tokens, c.assistant_tokens,
                                c.tool_results_tokens, c.reasoning_tokens, c.compaction_summary_tokens,
                                c.pinned_turn_ids.len(),
                                c.cleared_tool_results_count, c.spilled_blob_count,
                                c.last_prompt_cached_tokens.map_or_else(|| "n/a".to_owned(), |n| n.to_string())
                            );
                            let turns = c.turn_ids.clone();
                            let pinned_ids = c.pinned_turn_ids.clone();
                            let blobs = c.spilled_blobs.clone();
                            let verbatim = c.verbatim_window_turns;
                            let reasoning = c.reasoning_pruning;
                            let clear = c.clear_old_tool_results;
                            let selected_turn = context_turn;
                            let session_id = store.with(|s| s.active_session_id.clone());
                            let turn_rows = turns.iter().enumerate().map(|(idx, turn)| {
                                let pinned = pinned_ids.contains(turn);
                                let turn_for_pin = turn.clone();
                                let turn_for_sum = turn.clone();
                                let pin_actions = crate::transport::current();
                                let sum_actions = crate::transport::current();
                                view! {
                                    <div class=format!("panel-row {}", if idx == selected_turn { "selected" } else { "" })>
                                        <span class="panel-title">{turn.clone()}</span>
                                        <span class="panel-meta">{if pinned { "pinned" } else { "turn" }}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &pin_actions {
                                                t.run_host_action(format!("/pin-turn {turn_for_pin}{}", if pinned { " off" } else { "" }));
                                                t.run_host_action("/context".to_owned());
                                            }
                                        }>{if pinned { "Unpin" } else { "Pin" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &sum_actions { t.run_host_action(format!("/summarize --from {turn_for_sum}")); }
                                        }>"Summarize from here"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let next_window = match verbatim {
                                0..=3 => 4,
                                4..=7 => 8,
                                8..=15 => 16,
                                _ => 4,
                            };
                            let window_actions = crate::transport::current();
                            let reasoning_actions = crate::transport::current();
                            let clear_actions = crate::transport::current();
                            let policy = view! {
                                <div class="panel-form">
                                    <button class="btn tiny" on:click=move |_| {
                                        if let Some(t) = &window_actions {
                                            t.run_host_action(format!("/context-policy verbatim {next_window}"));
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>{format!("Verbatim {verbatim}")}</button>
                                    <button class="btn tiny" on:click=move |_| {
                                        if let Some(t) = &reasoning_actions {
                                            t.run_host_action(format!("/context-policy reasoning {}", if reasoning { "off" } else { "on" }));
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>{format!("Reasoning {}", if reasoning { "on" } else { "off" })}</button>
                                    <button class="btn tiny" on:click=move |_| {
                                        if let Some(t) = &clear_actions {
                                            t.run_host_action(format!("/context-policy clear {}", if clear { "off" } else { "on" }));
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>{format!("Clear old {}", if clear { "on" } else { "off" })}</button>
                                </div>
                            };
                            let focus_store = store;
                            let focus_click_store = store;
                            let focus_actions = crate::transport::current();
                            let focus = view! {
                                <div class="panel-form">
                                    <input class="field" placeholder="/summarize --from <turn> [focus]"
                                        prop:value=context_focus
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            focus_store.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { context_focus, .. }) = state.modal.as_mut() { context_focus.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let command = focus_click_store.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { context_focus, .. }) => context_focus.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &focus_actions {
                                            t.run_host_action(command);
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>"Run"</button>
                                </div>
                            };
                            let blob_rows = blobs.iter().map(|key| {
                                let key_for_fetch = key.clone();
                                let actions = crate::transport::current();
                                let session = session_id.clone();
                                view! {
                                    <div class="panel-row">
                                        <span class="panel-title">{key.clone()}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let (Some(t), Some(s)) = (&actions, session.clone()) {
                                                t.fetch_blob(s, key_for_fetch.clone());
                                            }
                                        }>"View"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let close_store = store;
                            let blob_view = context_blob.clone().map(|(key, content)| {
                                view! {
                                    <div class="panel-form column">
                                        <h4>{format!("Blob {key}")}</h4>
                                        <pre class="panel-text">{content}</pre>
                                        <button class="btn" on:click=move |_| {
                                            close_store.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { context_blob, .. }) = state.modal.as_mut() { *context_blob = None; }
                                            });
                                        }>"Close"</button>
                                    </div>
                                }.into_any()
                            }).unwrap_or_else(|| view! { <span></span> }.into_any());
                            view! {
                                <div>
                                    <pre class="panel-text">{summary}</pre>
                                    <h4>"Turns"</h4>
                                    <div class="panel-list">{turn_rows}</div>
                                    <h4>"Projection"</h4>
                                    {policy}
                                    {focus}
                                    <h4>"Spilled blobs"</h4>
                                    <div class="panel-list">{blob_rows}</div>
                                    {blob_view}
                                </div>
                            }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Memory => {
                            let rows = memories.iter().enumerate().map(|(idx, memory)| {
                                let id = memory.id.clone();
                                let pin_id = id.clone();
                                let delete_id = id.clone();
                                let actions = crate::transport::current();
                                let pin_actions = crate::transport::current();
                                let pinned = memory.pinned;
                                view! {
                                    <div class=format!("panel-row {}", if idx == selected { "selected" } else { "" })>
                                        <span class="panel-title">{format!("{} {}", if memory.pinned { "★" } else { " " }, memory.title)}</span>
                                        <span class="panel-meta">{format!("[{}] {}", memory.kind, memory.id)}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &pin_actions { t.run_host_action(format!("/pin {pin_id}{}", if pinned { " off" } else { "" })); t.fetch_memories(); }
                                        }>{if memory.pinned { "Unpin" } else { "Pin" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &actions { t.run_host_action(format!("/forget {delete_id}")); t.fetch_memories(); }
                                        }>"Delete"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            view! { <div class="panel-list">{rows}</div> }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Routing => {
                            let rows = routes.iter().map(|route| {
                                let task = route.task_type.clone();
                                let actions = crate::transport::current();
                                view! {
                                    <div class="panel-row">
                                        <span class="panel-title">{format!("{}", route.task_type)}</span>
                                        <span class="panel-meta">{format!("{}::{}", route.provider, route.model)}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &actions { t.run_host_action(format!("/routing clear {task}")); t.fetch_routes(); }
                                        }>"Delete"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let store_for_input = store;
                            let store_for_click = store;
                            let actions = crate::transport::current();
                            view! {
                                <div class="panel-list">{rows}</div>
                                <div class="panel-form">
                                    <input class="field" placeholder="/routing set build openai::o3 1.0 20000 2"
                                        prop:value=route_input
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            store_for_input.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { route_input, .. }) = state.modal.as_mut() { route_input.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let command = store_for_click.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { route_input, .. }) => route_input.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &actions { t.run_host_action(command); }
                                    }>"Set route"</button>
                                </div>
                            }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Packs => {
                            let rows = packs.iter().enumerate().map(|(idx, pack)| {
                                let enable_id = pack.id.clone();
                                let trust_id = pack.id.clone();
                                let remove_id = pack.id.clone();
                                let enabled = pack.enabled;
                                let trusted = pack.trusted;
                                let actions = crate::transport::current();
                                view! {
                                    <div class=format!("panel-row {}", if idx == selected { "selected" } else { "" })>
                                        <span class="panel-title">{format!("{} [{}] [{}]", pack.id, if pack.enabled { "enabled" } else { "disabled" }, if pack.trusted { "trusted" } else { "untrusted" })}</span>
                                        <span class="panel-meta">{format!("{} @ {}", pack.source_uri, pack.pinned_rev)}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &actions { t.run_host_action(format!("/pack {} {enable_id}", if enabled { "disable" } else { "enable" })); t.fetch_packs(); }
                                        }>{if pack.enabled { "Disable" } else { "Enable" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = crate::transport::current() { t.run_host_action(format!("/pack {} {trust_id}", if trusted { "untrust" } else { "trust" })); t.fetch_packs(); }
                                        }>{if pack.trusted { "Untrust" } else { "Trust" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = crate::transport::current() { t.run_host_action(format!("/pack remove {remove_id}")); t.fetch_packs(); }
                                        }>"Remove"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let defs = definitions.agents.iter().chain(definitions.skills.iter()).map(|def| {
                                let invocation = match def.model_invocable {
                                    Some(true) => " · model",
                                    Some(false) => " · manual",
                                    None => "",
                                };
                                let mode = match def.mode.as_deref() {
                                    Some(mode) if mode != "all" => format!(" · {mode}"),
                                    _ => String::new(),
                                };
                                view! {
                                    <div class="panel-row"><span class="panel-title">{format!("{}/{}", def.kind, def.id)}</span><span class="panel-meta">{format!("{} · ~{} tokens{}{}", def.source, def.token_estimate, invocation, mode)}</span></div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let store_for_input = store;
                            let store_for_click = store;
                            let actions = crate::transport::current();
                            let selected_pack = packs.get(selected).map(|pack| pack.id.clone()).unwrap_or_default();
                            let store_for_pin_input = store;
                            let store_for_pin_click = store;
                            let pin_actions = crate::transport::current();
                            let selected_pack_for_pin = selected_pack.clone();
                            view! {
                                <div class="panel-list">{rows}</div>
                                <div class="panel-form">
                                    <input class="field" placeholder="/pack add ./path or git URL"
                                        prop:value=pack_input
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            store_for_input.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { pack_input, .. }) = state.modal.as_mut() { pack_input.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let command = store_for_click.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { pack_input, .. }) => pack_input.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &actions { t.run_host_action(command); }
                                    }>"Install"</button>
                                </div>
                                <div class="panel-form">
                                    <input class="field" placeholder="revision for the selected pack"
                                        prop:value=pack_pin_input
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            store_for_pin_input.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { pack_pin_input, .. }) = state.modal.as_mut() { pack_pin_input.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let revision = store_for_pin_click.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { pack_pin_input, .. }) => pack_pin_input.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &pin_actions
                                            && !revision.trim().is_empty()
                                            && !selected_pack_for_pin.is_empty()
                                        {
                                            t.run_host_action(format!("/pack pin {} {}", selected_pack_for_pin, revision.trim()));
                                            t.fetch_packs();
                                        }
                                    }>"Pin selected"</button>
                                </div>
                                <h4>"Definitions"</h4>
                                <div class="panel-list">{defs}</div>
                            }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Subagent => {
                            let store_name = store;
                            let store_agent = store;
                            let store_task = store;
                            let store_model = store;
                            let store_mode = store;
                            let store_steps = store;
                            let store_tokens = store;
                            let store_cost = store;
                            let store_tools = store;
                            let store_spawn = store;
                            let mode_label = mode.map(|m| match m { meerai_web_protocol::ApprovalModeDto::Plan => "plan", meerai_web_protocol::ApprovalModeDto::Ask => "ask", meerai_web_protocol::ApprovalModeDto::Auto => "auto" }).unwrap_or("parent");
                            view! {
                                <div class="panel-form column">
                                    <label>"Name" <input class="field" placeholder="optional, one word" prop:value=subagent_name.clone() on:input=move |ev| { let v = event_target_value(&ev); store_name.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_name, .. }) = s.modal.as_mut() { subagent_name.replace_text(v); }); } /></label>
                                    <label>"Agent" <input class="field" prop:value=agent.clone() on:input=move |ev| { let v = event_target_value(&ev); store_agent.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_agent, .. }) = s.modal.as_mut() { *subagent_agent = v; }); } /></label>
                                    <label>"Task" <input class="field" prop:value=task.clone() on:input=move |ev| { let v = event_target_value(&ev); store_task.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_task, .. }) = s.modal.as_mut() { subagent_task.replace_text(v); }); } /></label>
                                    <label>"Model" <input class="field" prop:value=model.clone() on:input=move |ev| { let v = event_target_value(&ev); store_model.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_model, .. }) = s.modal.as_mut() { subagent_model.replace_text(v); }); } /></label>
                                    <label>"Mode" <button class="btn tiny" on:click=move |_| {
                                        store_mode.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_mode, .. }) = s.modal.as_mut() {
                                            *subagent_mode = match *subagent_mode {
                                                None => Some(meerai_web_protocol::ApprovalModeDto::Plan),
                                                Some(meerai_web_protocol::ApprovalModeDto::Plan) => Some(meerai_web_protocol::ApprovalModeDto::Ask),
                                                Some(meerai_web_protocol::ApprovalModeDto::Ask) => Some(meerai_web_protocol::ApprovalModeDto::Auto),
                                                Some(meerai_web_protocol::ApprovalModeDto::Auto) => None,
                                            };
                                        });
                                    }>{mode_label}</button></label>
                                    <label>"Steps" <input class="field" placeholder="preset" prop:value=subagent_steps.clone() on:input=move |ev| { let v = event_target_value(&ev); store_steps.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_steps, .. }) = s.modal.as_mut() { subagent_steps.replace_text(v); }); } /></label>
                                    <label>"Tokens" <input class="field" placeholder="parent cap" prop:value=subagent_tokens.clone() on:input=move |ev| { let v = event_target_value(&ev); store_tokens.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_tokens, .. }) = s.modal.as_mut() { subagent_tokens.replace_text(v); }); } /></label>
                                    <label>"Cost USD" <input class="field" placeholder="parent cap" prop:value=subagent_cost.clone() on:input=move |ev| { let v = event_target_value(&ev); store_cost.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_cost, .. }) = s.modal.as_mut() { subagent_cost.replace_text(v); }); } /></label>
                                    <label>"Tools" <input class="field" placeholder="comma-separated subset" prop:value=subagent_tools.clone() on:input=move |ev| { let v = event_target_value(&ev); store_tools.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_tools, .. }) = s.modal.as_mut() { subagent_tools.replace_text(v); }); } /></label>
                                    <button class="btn" on:click=move |_| {
                                        let (name, agent, task, model, mode, steps, tokens, cost, tools) = store_spawn.with(|s| match &s.modal {
                                            Some(ActiveModal::WorkspacePanel { subagent_name, subagent_agent, subagent_task, subagent_model, subagent_mode, subagent_steps, subagent_tokens, subagent_cost, subagent_tools, .. }) => (
                                                subagent_name.text.clone(),
                                                subagent_agent.clone(),
                                                subagent_task.text.clone(),
                                                subagent_model.text.clone(),
                                                *subagent_mode,
                                                subagent_steps.text.clone(),
                                                subagent_tokens.text.clone(),
                                                subagent_cost.text.clone(),
                                                subagent_tools.text.clone(),
                                            ),
                                            _ => (String::new(), String::new(), String::new(), String::new(), None, String::new(), String::new(), String::new(), String::new()),
                                        });
                                        if let Some(t) = crate::transport::current() {
                                            let mut command = format!("/subagent {} ", agent);
                                            if !name.trim().is_empty() { command.push_str(&format!("--name {} ", name.trim())); }
                                            if !model.is_empty() { command.push_str(&format!("--model {model} ")); }
                                            if let Some(mode) = mode { command.push_str(&format!("--mode {} ", match mode { meerai_web_protocol::ApprovalModeDto::Plan => "plan", meerai_web_protocol::ApprovalModeDto::Ask => "ask", meerai_web_protocol::ApprovalModeDto::Auto => "auto" })); }
                                            if !steps.trim().is_empty() { command.push_str(&format!("--steps {} ", steps.trim())); }
                                            if !tokens.trim().is_empty() { command.push_str(&format!("--tokens {} ", tokens.trim())); }
                                            if !cost.trim().is_empty() { command.push_str(&format!("--cost {} ", cost.trim())); }
                                            if !tools.trim().is_empty() { command.push_str(&format!("--tools {} ", tools.trim())); }
                                            command.push_str(&task);
                                            t.run_host_action(command);
                                        }
                                    }>"Spawn subagent"</button>
                                </div>
                            }.into_any()
                        }
                    };
                    let tabs = [
                        (crate::state::WorkspacePanelTab::Context, "Context"),
                        (crate::state::WorkspacePanelTab::Goal, "Goal"),
                        (crate::state::WorkspacePanelTab::Memory, "Memory"),
                        (crate::state::WorkspacePanelTab::Subagent, "Subagent"),
                        (crate::state::WorkspacePanelTab::Routing, "Routing"),
                        (crate::state::WorkspacePanelTab::Packs, "Packs"),
                    ];
                    let tab_buttons = tabs.into_iter().map(|(value, label)| {
                        let store_tab = store;
                        view! {
                            <button class=format!("btn tiny {}", if value == tab { "active" } else { "" }) on:click=move |_| {
                                store_tab.update(|s| if let Some(ActiveModal::WorkspacePanel { tab, .. }) = s.modal.as_mut() { *tab = value; });
                            }>{label}</button>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <div class="modal-head">
                            <h2>"Control panel"</h2>
                            <div class="panel-tabs">{tab_buttons}</div>
                            <button class="btn tiny" on:click={
                                let reload = reload.clone();
                                move |_| reload(tab)
                            }>"Refresh"</button>
                            <button class="btn tiny" on:click=close.clone()>"Close"</button>
                        </div>
                        {if loading { view! { <div class="panel-status">"Loading..."</div> }.into_any() } else if let Some(error) = error { view! { <div class="panel-status error">{error}</div> }.into_any() } else { view! { <span></span> }.into_any() }}
                        {body}
                    }.into_any()
                }}
            </div>
        </div>
    }
}

#[component]
fn VaultResetModal(store: StoreHandle) -> impl IntoView {
    let value = Memo::new(move |_| {
        store.with(|state| match &state.modal {
            Some(ActiveModal::VaultReset { input, .. }) => input.text.clone(),
            _ => String::new(),
        })
    });
    let error = Memo::new(move |_| {
        store.with(|state| match &state.modal {
            Some(ActiveModal::VaultReset { error, .. }) => error.clone(),
            _ => None,
        })
    });
    let store_input = store;
    let store_cancel = store;
    let store_reset = store;
    view! {
        <div class="modal-backdrop">
            <div class="modal">
                <div class="modal-head">
                    <h2>"Reset credential vault"</h2>
                    <button class="btn tiny" on:click=move |_| {
                        store_cancel.update(|state| state.modal = None);
                    }>"Close"</button>
                </div>
                <div class="modal-form column">
                    <p class="panel-status error">
                        "This deletes every saved API key and OAuth credential and creates a new vault key. The old data cannot be recovered."
                    </p>
                    <label class="field">
                        <span class="field-label">"Type reset to confirm"</span>
                        <input
                            class="input"
                            prop:value=value
                            on:input=move |ev| {
                                let value = event_target_value(&ev);
                                store_input.update(|state| {
                                    if let Some(ActiveModal::VaultReset { input, error }) = state.modal.as_mut() {
                                        input.replace_text(value);
                                        *error = None;
                                    }
                                });
                            }
                        />
                    </label>
                    {move || error.get().map(|message| view! {
                        <p class="panel-status error">{message}</p>
                    })}
                    <button class="btn danger" on:click=move |_| {
                        let value = store_reset.with(|state| match &state.modal {
                            Some(ActiveModal::VaultReset { input, .. }) => input.text.clone(),
                            _ => String::new(),
                        });
                        if value.trim() != "reset" {
                            store_reset.update(|state| {
                                if let Some(ActiveModal::VaultReset { error, .. }) = state.modal.as_mut() {
                                    *error = Some("Type reset exactly to confirm.".to_owned());
                                }
                            });
                            return;
                        }
                        if let Some(transport) = crate::transport::current() {
                            transport.run_host_action("/vault-reset reset".to_owned());
                            store_reset.update(|state| state.modal = None);
                        } else {
                            store_reset.update(|state| {
                                if let Some(ActiveModal::VaultReset { error, .. }) = state.modal.as_mut() {
                                    *error = Some("Reset needs a live server connection.".to_owned());
                                }
                            });
                        }
                    }>"Reset vault"</button>
                </div>
            </div>
        </div>
    }
}
