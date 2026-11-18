use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{
    ApprovalDecisionDto, ApprovalRequestDto, FileDiffResponse, GrantScopeDto, ModelItemDto,
    ProviderItemDto, QuestionAnswerDto, QuestionRequestDto, RewindScopeDto, SearchModeDto,
    StoredSessionDto,
};
use wasm_bindgen::JsCast;

use crate::state::{
    ActiveModal, QuestionDraft, activate_provider_from_modal, execute_palette_action,
    execute_slash_command, filtered_commands, filtered_history, filtered_models,
    filtered_palette_actions, filtered_providers, open_api_key_modal, open_rewind_modal,
    open_session_select, start_oauth_modal,
};
use crate::store::StoreHandle;
use crate::views::command_label;
use crate::views::settings::render_settings;

pub fn render_modal_layer(store: StoreHandle) -> AnyView {
    let Some(modal) = store.with(|s| s.modal.clone()) else {
        return view! { <div></div> }.into_any();
    };
    render_modal(store, modal)
}

#[component]
pub fn ModalLayer(store: StoreHandle) -> impl IntoView {
    let key = Memo::new(move |_| store.with(|s| s.modal.as_ref().map(modal_name)));
    Effect::new(move |_| {
        if key.get().is_some()
            && let Some(window) = web_sys::window()
            && let Some(document) = window.document()
            && let Ok(Some(element)) = document.query_selector(".modal")
            && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = element.focus();
        }
    });
    view! { {move || render_modal_layer(store)} }
}

fn modal_name(modal: &ActiveModal) -> &'static str {
    match modal {
        ActiveModal::ProviderSelect { .. } => "provider-select",
        ActiveModal::AuthMethodSelect { .. } => "auth-method",
        ActiveModal::ProviderConnection { .. } => "provider-connection",
        ActiveModal::ChatGptLogin { .. } => "chatgpt-login",
        ActiveModal::ApiKeyInput { .. } => "api-key",
        ActiveModal::ModelInput { .. } => "model",
        ActiveModal::DiffView { .. } => "diff",
        ActiveModal::Search { .. } => "search",
        ActiveModal::Approval { .. } => "approval",
        ActiveModal::Question { .. } => "question",
        ActiveModal::Terminal { .. } => "terminal",
        ActiveModal::RewindMenu { .. } => "rewind",
        ActiveModal::SessionSelect { .. } => "sessions",
        ActiveModal::CommandPalette { .. } => "palette",
        ActiveModal::UserMessageActions { .. } => "message-actions",
        ActiveModal::Settings(_) => "settings",
        ActiveModal::Help => "help",
        ActiveModal::SubagentOverlay { .. } => "subagent",
    }
}

fn render_modal(store: StoreHandle, modal: ActiveModal) -> AnyView {
    match modal {
        ActiveModal::ProviderSelect {
            filter,
            selected: _,
        } => {
            let filter_text = filter.text.clone();
            let providers = store.with(|s| filtered_providers(s.providers.as_ref(), &filter_text));
            let rows = providers
                .into_iter()
                .enumerate()
                .map(|(index, provider)| provider_row(store, provider, index))
                .collect::<Vec<_>>();
            let body = view! {
                <div class="modal-form">
                    <input
                        class="input"
                        placeholder="Filter providers"
                        prop:value=filter_text.clone()
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            store.update(|s| {
                                if let Some(ActiveModal::ProviderSelect { filter, .. }) = &mut s.modal {
                                    filter.replace_text(value);
                                }
                            });
                        }
                    />
                    <div class="pick-list" role="listbox">
                        {AnyView::from(Fragment::new(rows))}
                    </div>
                    <div class="hint">"Credentials stay on the host. The browser never receives a key."</div>
                </div>
            }
            .into_any();
            shell("Connect a provider", store, body, empty_actions())
        }
        ActiveModal::AuthMethodSelect {
            provider_id,
            provider_name,
            selected: _,
        } => {
            let provider = find_provider(&store, &provider_id);
            let api_provider = provider.clone();
            let oauth_provider = provider.clone();
            let body = view! {
                <div class="modal-form">
                    <p>{format!("How should meerai authenticate with {provider_name}?")}</p>
                    <div class="choice-grid">
                        <button class="choice" on:click=move |_| {
                            if let Some(provider) = api_provider.clone() {
                                store.update(|s| open_api_key_modal(s, &provider));
                            }
                        }>
                            <span class="choice-title">"API key"</span>
                            <span class="choice-desc">"Store a key on the host. The browser sends it once."</span>
                        </button>
                        <button class="choice" on:click=move |_| {
                            if let Some(provider) = oauth_provider.clone() {
                                store.update(|s| start_oauth_modal(s, &provider));
                            }
                        }>
                            <span class="choice-title">"ChatGPT sign-in"</span>
                            <span class="choice-desc">"Open the browser sign-in flow and paste the redirect on a remote host."</span>
                        </button>
                    </div>
                </div>
            }
            .into_any();
            shell("Authentication method", store, body, empty_actions())
        }
        ActiveModal::ProviderConnection {
            provider_id,
            provider_name,
            status_badge,
            configured,
            base_url,
            selected: _,
        } => {
            let body = view! {
                <div class="modal-form">
                    <div class="kv"><span>"Provider"</span><strong>{provider_name}</strong></div>
                    <div class="kv"><span>"Status"</span><strong>{status_badge}</strong></div>
                    <div class="kv"><span>"Configured"</span><strong>{if configured { "yes" } else { "no" }}</strong></div>
                    {base_url.map(|url| view! { <div class="kv"><span>"Base URL"</span><code>{url}</code></div> }.into_any())}
                </div>
            }
            .into_any();
            let id_continue = provider_id.clone();
            let id_update = provider_id.clone();
            let id_remove = provider_id.clone();
            let actions = view! {
                <button class="btn primary" on:click=move |_| {
                    let id = id_continue.clone();
                    store.update(|s| {
                        if let Some(provider) = find_provider_state(s, &id) {
                            let provider = provider.clone();
                            activate_provider_from_modal(s, provider);
                        }
                    });
                }>"Continue"</button>
                <button class="btn" on:click=move |_| {
                    let id = id_update.clone();
                    store.update(|s| {
                        if let Some(provider) = find_provider_state(s, &id) {
                            let provider = provider.clone();
                            open_api_key_modal(s, &provider);
                        }
                    });
                }>"Update credential"</button>
                <button class="btn danger" on:click=move |_| {
                    let id = id_remove.clone();
                    if let Some(t) = crate::transport::current() {
                        t.remove_credential(id.clone());
                    }
                    store.update(|s| {
                        s.modal = None;
                        s.status_notice = Some(format!("Removing credentials for {id}"));
                    });
                }>"Remove credential"</button>
                <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
            }
            .into_any();
            shell("Provider connection", store, body, actions)
        }
        ActiveModal::ChatGptLogin {
            provider_id,
            provider_name,
            attempt_id,
            url,
            paste,
            state,
            error,
            busy,
        } => {
            let status = state
                .as_ref()
                .map(|state| format!("{state:?}"))
                .unwrap_or_else(|| "waiting".to_owned());
            let body = view! {
                <div class="modal-form">
                    <div class="kv"><span>"Provider"</span><strong>{provider_name}</strong></div>
                    <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
                    {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                    {url.clone().map(|url| view! {
                        <div class="modal-form">
                            <div class="hint">"Open this URL to sign in. On a remote host, paste the final redirect URL below."</div>
                            <code class="url-box">{url.clone()}</code>
                            <button class="btn" on:click=move |_| crate::store::copy_text(url.clone())>"Copy URL"</button>
                        </div>
                    }.into_any())}
                    <input
                        class="input"
                        placeholder="Paste the redirect URL"
                        prop:value=paste.text.clone()
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
            let id_paste = provider_id.clone();
            let attempt_paste = attempt_id.clone();
            let id_cancel = provider_id.clone();
            let attempt_cancel = attempt_id.clone();
            let actions = view! {
                <button class="btn primary" disabled=busy on:click=move |_| {
                    let id = id_paste.clone();
                    let attempt = attempt_paste.clone();
                    let input = store.with(|s| match &s.modal {
                        Some(ActiveModal::ChatGptLogin { paste, .. }) => paste.text.clone(),
                        _ => String::new(),
                    });
                    if let (Some(attempt), Some(t)) = (attempt, crate::transport::current()) {
                        t.paste_chatgpt_login(id.clone(), attempt, input);
                    }
                }>"Submit redirect"</button>
                <button class="btn" on:click=move |_| {
                    if let (Some(attempt), Some(t)) = (attempt_cancel.clone(), crate::transport::current()) {
                        t.cancel_chatgpt_login(id_cancel.clone(), attempt);
                    }
                    store.update(|s| s.modal = None);
                }>"Cancel"</button>
            }
            .into_any();
            shell("ChatGPT sign-in", store, body, actions)
        }
        ActiveModal::ApiKeyInput {
            provider_id,
            provider_name,
            status_badge,
            api_key,
            base_url,
            error,
            busy,
        } => {
            let body = view! {
                <div class="modal-form">
                    <div class="kv"><span>"Provider"</span><strong>{provider_name}</strong></div>
                    <div class="kv"><span>"Status"</span><strong>{status_badge}</strong></div>
                    {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                    <input
                        class="input"
                        type="password"
                        placeholder="API key"
                        prop:value=api_key.text.clone()
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            store.update(|s| {
                                if let Some(ActiveModal::ApiKeyInput { api_key, .. }) = &mut s.modal {
                                    api_key.replace_text(value);
                                }
                            });
                        }
                    />
                    <input
                        class="input"
                        placeholder="Optional provider base URL"
                        prop:value=base_url.text.clone()
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            store.update(|s| {
                                if let Some(ActiveModal::ApiKeyInput { base_url, .. }) = &mut s.modal {
                                    base_url.replace_text(value);
                                }
                            });
                        }
                    />
                </div>
            }
            .into_any();
            let id_save = provider_id.clone();
            let id_remove = provider_id.clone();
            let actions = view! {
                <button class="btn primary" disabled=busy on:click=move |_| {
                    let id = id_save.clone();
                    let (key, base) = store.with(|s| match &s.modal {
                        Some(ActiveModal::ApiKeyInput { api_key, base_url, .. }) => (
                            api_key.text.clone(),
                            (!base_url.text.trim().is_empty()).then(|| base_url.text.trim().to_owned()),
                        ),
                        _ => (String::new(), None),
                    });
                    if key.trim().is_empty() {
                        store.update(|s| {
                            if let Some(ActiveModal::ApiKeyInput { error, .. }) = &mut s.modal {
                                *error = Some("Enter an API key.".to_owned());
                            }
                        });
                        return;
                    }
                    if let Some(t) = crate::transport::current() {
                        t.set_api_key(id.clone(), key, base);
                    }
                    store.update(|s| {
                        if let Some(ActiveModal::ApiKeyInput { busy, .. }) = &mut s.modal {
                            *busy = true;
                        }
                    });
                }>"Save"</button>
                <button class="btn danger" on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.remove_credential(id_remove.clone());
                    }
                }>"Remove saved key"</button>
                <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
            }
            .into_any();
            shell("API key", store, body, actions)
        }
        ActiveModal::ModelInput {
            filter,
            provider_id,
            selected: _,
        } => {
            let filter_text = filter.text.clone();
            let models = store.with(|s| {
                filtered_models(s.providers.as_ref(), provider_id.as_deref(), &filter_text)
            });
            let rows = models
                .into_iter()
                .map(|model| model_row(store, model))
                .collect::<Vec<_>>();
            let body = view! {
                <div class="modal-form">
                    <input
                        class="input"
                        placeholder="Type a model name or filter"
                        prop:value=filter_text.clone()
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            store.update(|s| {
                                if let Some(ActiveModal::ModelInput { filter, .. }) = &mut s.modal {
                                    filter.replace_text(value);
                                }
                            });
                        }
                    />
                    <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                </div>
            }
            .into_any();
            let actions = view! {
                <button class="btn primary" on:click=move |_| {
                    let raw = store.with(|s| match &s.modal {
                        Some(ActiveModal::ModelInput { filter, .. }) => filter.text.clone(),
                        _ => String::new(),
                    });
                    let raw = raw.trim().to_owned();
                    if raw.is_empty() {
                        return;
                    }
                    let provider = store.with(|s| {
                        s.active_session().map(|session| session.provider.clone()).unwrap_or_default()
                    });
                    let (provider, model) = crate::state::parse_provider_model_arg(&raw, &provider);
                    if let (Some(id), Some(t)) = (store.with(|s| s.active_session_id.clone()), crate::transport::current()) {
                        t.set_model(id, provider, model);
                    }
                    store.update(|s| s.modal = None);
                }>"Use model"</button>
                <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
            }
            .into_any();
            shell("Choose a model", store, body, actions)
        }
        ActiveModal::DiffView {
            selected_path,
            unified,
            diff,
            loading,
            error,
        } => {
            let body = diff_body(store, selected_path, unified, diff, loading, error);
            shell_wide("Workspace diff", store, body, empty_actions())
        }
        ActiveModal::Search {
            mode,
            query,
            results,
            selected,
            preview,
            loading,
        } => {
            let body = search_body(
                store,
                mode,
                &query.text,
                results,
                selected,
                preview,
                loading,
            );
            shell("Workspace search", store, body, empty_actions())
        }
        ActiveModal::Approval {
            session_id,
            request,
            reason,
            showing_reason,
        } => approval_modal(store, session_id, request, reason.text, showing_reason),
        ActiveModal::Question {
            session_id,
            request,
            draft,
        } => question_modal(store, session_id, request, draft),
        ActiveModal::Terminal {
            command,
            output,
            running,
            error,
        } => terminal_modal(store, command.text, output, running, error),
        ActiveModal::RewindMenu {
            turns,
            selected_turn,
            scope,
        } => rewind_modal(store, turns, selected_turn, scope),
        ActiveModal::SessionSelect {
            filter,
            history,
            selected,
            loading,
            confirm_delete,
        } => session_select_modal(
            store,
            filter.text,
            history,
            selected,
            loading,
            confirm_delete,
        ),
        ActiveModal::CommandPalette { filter, selected } => {
            palette_modal(store, filter.text, selected)
        }
        ActiveModal::UserMessageActions {
            entry_id,
            turn_id,
            text,
            selected,
        } => user_message_modal(store, entry_id, turn_id, text, selected),
        ActiveModal::Settings(form) => render_settings(store, form),
        ActiveModal::Help => help_modal(store),
        ActiveModal::SubagentOverlay {
            entry_id,
            agent,
            status,
            child_session_id,
            entries,
            usage,
            loading,
            error,
        } => subagent_modal(
            store,
            entry_id,
            agent,
            status,
            child_session_id,
            entries,
            usage,
            loading,
            error,
        ),
    }
}

fn shell(title: &str, store: StoreHandle, body: AnyView, actions: AnyView) -> AnyView {
    shell_with_width(title, store, body, actions, false)
}

fn shell_wide(title: &str, store: StoreHandle, body: AnyView, actions: AnyView) -> AnyView {
    shell_with_width(title, store, body, actions, true)
}

fn shell_with_width(
    title: &str,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
    wide: bool,
) -> AnyView {
    let close = store.clone();
    let class = if wide { "modal modal-wide" } else { "modal" };
    view! {
        <div class="modal-backdrop">
            <div class=class role="dialog" aria-modal="true" tabindex="-1">
                <div class="modal-head">
                    <h2>{title.to_owned()}</h2>
                    <button class="modal-close" title="Close" on:click=move |_| close.update(|s| s.modal = None)>"×"</button>
                </div>
                <div class="modal-body">{body}</div>
                <div class="modal-actions">{actions}</div>
            </div>
        </div>
    }
    .into_any()
}

fn empty_actions() -> AnyView {
    view! { <div></div> }.into_any()
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

fn provider_row(store: StoreHandle, provider: ProviderItemDto, _index: usize) -> AnyView {
    let click = provider.clone();
    let status = if provider.configured {
        "configured"
    } else {
        "not configured"
    };
    view! {
        <button class="pick-row" on:click=move |_| {
            let provider = click.clone();
            store.update(|s| activate_provider_from_modal(s, provider));
        }>
            <span class="pick-title">{provider.display_name.clone()}</span>
            <span class="pick-meta">{format!("{} · {}", provider.id, status)}</span>
            <span class=format!("badge {}", if provider.configured { "ok" } else { "muted" })>
                {provider.status_badge.clone()}
            </span>
        </button>
    }
    .into_any()
}

fn model_row(store: StoreHandle, model: ModelItemDto) -> AnyView {
    let click_model = model.clone();
    let current = store.with(|s| {
        s.active_session()
            .is_some_and(|session| session.model == model.id && session.provider == model.provider)
    });
    view! {
        <button class="pick-row" class:active=current on:click=move |_| {
            let model = click_model.clone();
            if let (Some(id), Some(t)) = (store.with(|s| s.active_session_id.clone()), crate::transport::current()) {
                t.set_model(id, model.provider.clone(), model.id.clone());
            }
            store.update(|s| s.modal = None);
        }>
            <span class="pick-title">{format!("{}/{}", model.provider, model.id)}</span>
            <span class="pick-meta">{model.display_name.clone()}</span>
        </button>
    }
    .into_any()
}

fn approval_modal(
    store: StoreHandle,
    session_id: String,
    request: ApprovalRequestDto,
    reason_text: String,
    showing_reason: bool,
) -> AnyView {
    let title = request.title.clone();
    let detail = request.detail.clone();
    let effect = request
        .effect
        .map(|effect| format!("{effect:?}"))
        .unwrap_or_else(|| "unknown".to_owned());
    let request_id = request.request_id.clone();
    let body = view! {
        <div class="modal-form">
            <div class="approval-kind">{format!("{:?}", request.kind)}</div>
            <div class="approval-title">{title}</div>
            <pre class="approval-detail">{detail}</pre>
            <div class="kv"><span>"Effect"</span><strong>{effect}</strong></div>
            <Show when=move || showing_reason>
                <input
                    class="input"
                    placeholder="Reason for declining"
                    prop:value=reason_text.clone()
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
            <div class="hint">"Questions are not approvals. This decision applies to the whole session."</div>
        </div>
    }
    .into_any();
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
    let sid_cancel = session_id.clone();
    let rid_cancel = request_id.clone();
    let actions = view! {
        <button class="btn primary" on:click=move |_| decide(&store, &sid, &rid, ApprovalDecisionDto::Accept, None)>"Approve once"</button>
        <button class="btn" on:click=move |_| decide(&store, &sid_turn, &rid_turn, ApprovalDecisionDto::Accept, Some(GrantScopeDto::Turn))>"Allow turn"</button>
        <button class="btn" on:click=move |_| decide(&store, &sid_session, &rid_session, ApprovalDecisionDto::AcceptForSession, Some(GrantScopeDto::Session))>"Allow session"</button>
        <button class="btn danger" on:click=move |_| decide(&store, &sid_decline, &rid_decline, ApprovalDecisionDto::Decline, None)>"Decline"</button>
        <button class="btn" on:click=move |_| {
            store.update(|s| {
                if let Some(ActiveModal::Approval { showing_reason, .. }) = &mut s.modal {
                    *showing_reason = true;
                }
            });
        }>"Reason"</button>
        <button class="btn danger" on:click=move |_| {
            let reason = store.with(|s| match &s.modal {
                Some(ActiveModal::Approval { reason, .. }) => reason.text.clone(),
                _ => String::new(),
            });
            let reason = if reason.trim().is_empty() { "Declined by the browser client.".to_owned() } else { reason };
            decide(&store, &sid_reason, &rid_reason, ApprovalDecisionDto::DeclineWithReason { reason }, None);
        }>"Decline with reason"</button>
        <button class="btn" on:click=move |_| decide(&store, &sid_cancel, &rid_cancel, ApprovalDecisionDto::Cancel, None)>"Cancel request"</button>
    }
    .into_any();
    shell("Approval required", store, body, actions)
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

fn question_modal(
    store: StoreHandle,
    session_id: String,
    request: QuestionRequestDto,
    draft: QuestionDraft,
) -> AnyView {
    let step = draft.step.min(request.questions.len().saturating_sub(1));
    let Some(question) = request.questions.get(step) else {
        return shell(
            "Question",
            store,
            view! { <div class="modal-form">"No questions."</div> }.into_any(),
            view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }.into_any(),
        );
    };
    let question = question.clone();
    let selected = draft.answers.get(step).cloned().unwrap_or_default();
    let custom = draft
        .custom_inputs
        .get(step)
        .map(|input| input.text.clone())
        .unwrap_or_default();
    let options = question
        .options
        .iter()
        .map(|option| {
            let label = option.label.clone();
            let label_click = label.clone();
            let is_selected = selected.contains(&label);
            let multiple = question.multiple;
            view! {
                <button
                    class="pick-row"
                    class:active=is_selected
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
                            }
                        });
                    }
                >
                    <span class="pick-title">{label}</span>
                    {option.description.clone().map(|description| view! { <span class="pick-meta">{description}</span> }.into_any())}
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let body = view! {
        <div class="modal-form">
            <div class="hint">{format!("Question {} of {}", step + 1, request.questions.len())}</div>
            <div class="question-head">{question.header.clone()}</div>
            <div class="question-text">{question.question.clone()}</div>
            <div class="pick-list">
                {AnyView::from(Fragment::new(options))}
            </div>
            <input
                class="input"
                placeholder="Other"
                prop:value=custom.clone()
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
        </div>
    }
    .into_any();
    let sid_cancel = session_id.clone();
    let rid_cancel = request.request_id.clone();
    let actions = view! {
        <button class="btn" on:click=move |_| {
            store.update(|s| {
                if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                    draft.step = draft.step.saturating_sub(1);
                }
            });
        }>"Back"</button>
        <button class="btn" on:click=move |_| {
            store.update(|s| {
                if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                    let next = draft.step + 1;
                    if next < draft.answers.len() {
                        draft.step = next;
                    }
                }
            });
        }>"Next"</button>
        <button class="btn primary" on:click=move |_| {
            let answers = store.with(|s| match &s.modal {
                Some(ActiveModal::Question { draft, .. }) => draft.answers.clone(),
                _ => Vec::new(),
            });
            let answers = request
                .questions
                .iter()
                .enumerate()
                .map(|(index, question)| {
                    let mut selected = answers.get(index).cloned().unwrap_or_default();
                    let custom = store.with(|s| match &s.modal {
                        Some(ActiveModal::Question { draft, .. }) => draft
                            .custom_inputs
                            .get(index)
                            .map(|input| input.text.trim().to_owned())
                            .unwrap_or_default(),
                        _ => String::new(),
                    });
                    if !custom.is_empty() {
                        selected.push(custom);
                    }
                    QuestionAnswerDto {
                        header: question.header.clone(),
                        selected,
                    }
                })
                .collect::<Vec<_>>();
            if let Some(t) = crate::transport::current() {
                t.answer_question(session_id.clone(), request.request_id.clone(), answers);
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
    .into_any();
    shell("Question", store, body, actions)
}

fn terminal_modal(
    store: StoreHandle,
    command_text: String,
    output: Option<String>,
    running: bool,
    error: Option<String>,
) -> AnyView {
    let body = view! {
        <div class="modal-form">
            <label class="field">
                <span class="field-label">"Command"</span>
                <input
                    class="input"
                    placeholder="Command, without the leading !"
                    prop:value=command_text.clone()
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
            {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
            {output.clone().map(|output| view! {
                <div class="field">
                    <span class="field-label">"Output"</span>
                    <pre class="terminal-output">{output}</pre>
                </div>
            }.into_any())}
            <div class="hint">{if running { "Running. You can close this dialog; the result lands in the transcript." } else { "The command runs with the same sandbox, environment, and timeout guards as the TUI." }}</div>
        </div>
    }
    .into_any();
    let actions = view! {
        <button class="btn primary" disabled=running on:click=move |_| {
            let command = store.with(|s| match &s.modal {
                Some(ActiveModal::Terminal { command, .. }) => command.text.clone(),
                _ => String::new(),
            });
            if !command.trim().is_empty() {
                store.update(|s| crate::state::execute_user_shell(s, command.trim()));
            }
        }>"Run"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Background"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
    }
    .into_any();
    shell("Shell", store, body, actions)
}

fn rewind_modal(
    store: StoreHandle,
    turns: Vec<(String, String)>,
    selected_turn: usize,
    scope: RewindScopeDto,
) -> AnyView {
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
    let body = view! {
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

fn set_scope(store: &StoreHandle, next: RewindScopeDto) {
    store.update(|s| {
        if let Some(ActiveModal::RewindMenu { scope, .. }) = &mut s.modal {
            *scope = next;
        }
    });
}

fn session_select_modal(
    store: StoreHandle,
    filter_text: String,
    history: Vec<StoredSessionDto>,
    _selected: usize,
    loading: bool,
    confirm_delete: Option<String>,
) -> AnyView {
    let rows = filtered_history(&history, &filter_text)
        .into_iter()
        .map(|session| {
            let open_id = session.id.clone();
            let delete_id = session.id.clone();
            let fork_id = session.id.clone();
            let confirming = confirm_delete.as_deref() == Some(session.id.as_str());
            let title = session.title.clone().unwrap_or_else(|| "Untitled session".to_owned());
            let model = session.model.clone().unwrap_or_default();
            view! {
                <div class="pick-row session-row">
                    <button class="pick-main" on:click=move |_| {
                        if let Some(t) = crate::transport::current() {
                            t.open_session(open_id.clone());
                        }
                        store.update(|s| s.modal = None);
                    }>
                        <span class="pick-title">{title}</span>
                        <span class="pick-meta">{format!("{} · {} · {} bytes", session.id, model, session.bytes)}</span>
                        <span class=format!("badge {}", if session.is_open { "ok" } else { "muted" })>
                            {if session.is_open { "live" } else { "stored" }}
                        </span>
                    </button>
                    <button class="btn tiny" on:click=move |_| {
                        if let Some(t) = crate::transport::current() {
                            t.fork_session(fork_id.clone(), None);
                        }
                        store.update(|s| s.modal = None);
                    }>"Fork"</button>
                    <button class="btn tiny danger" on:click=move |_| {
                        let id = delete_id.clone();
                        let already = store.with(|s| match &s.modal {
                            Some(ActiveModal::SessionSelect { confirm_delete, .. }) => confirm_delete.as_deref() == Some(id.as_str()),
                            _ => false,
                        });
                        if already {
                            if let Some(t) = crate::transport::current() {
                                t.delete_session(id.clone());
                            }
                            store.update(|s| s.modal = None);
                        } else {
                            store.update(|s| {
                                if let Some(ActiveModal::SessionSelect { confirm_delete, .. }) = &mut s.modal {
                                    *confirm_delete = Some(id.clone());
                                }
                            });
                        }
                    }>{if confirming { "Confirm delete" } else { "Delete" }}</button>
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let body = view! {
        <div class="modal-form">
            <input
                class="input"
                placeholder="Filter sessions"
                prop:value=filter_text.clone()
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::SessionSelect { filter, .. }) = &mut s.modal {
                            filter.replace_text(value);
                        }
                    });
                }
            />
            <Show when=move || loading>
                <div class="hint">"Loading sessions…"</div>
            </Show>
            <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
        </div>
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| store.update(|s| open_session_select(s, ""))>"Refresh"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
    }
    .into_any();
    shell("Sessions", store, body, actions)
}

fn palette_modal(store: StoreHandle, filter_text: String, selected: usize) -> AnyView {
    let commands = filtered_commands(&filter_text);
    let actions = filtered_palette_actions(&filter_text);
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
                        store.update(|s| {
                            s.draft.replace_text(format!("{cmd} "));
                            s.modal = None;
                            crate::state::update_completion(s);
                        });
                    } else {
                        store.update(|s| {
                            execute_slash_command(s, &cmd);
                            s.modal = None;
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
        .map(|action| {
            let key = action.key.to_owned();
            view! {
                <button class="pick-row" on:click=move |_| {
                    let key = key.clone();
                    store.update_with_effects(|s, fx| {
                        execute_palette_action(s, fx, &key);
                        s.modal = None;
                    });
                }>
                    <span class="pick-title">{action.label.to_owned()}</span>
                    <span class="pick-meta">{action.hint.to_owned()}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let body = view! {
        <div class="modal-form">
            <input
                class="input"
                placeholder="Search commands and actions"
                prop:value=filter_text.clone()
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::CommandPalette { filter, .. }) = &mut s.modal {
                            filter.replace_text(value);
                        }
                    });
                }
            />
            <div class="palette-section">"Commands"</div>
            <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
            <div class="palette-section">"Actions"</div>
            <div class="pick-list">{AnyView::from(Fragment::new(action_rows))}</div>
        </div>
    }
    .into_any();
    shell("Command palette", store, body, empty_actions())
}

fn user_message_modal(
    store: StoreHandle,
    _entry_id: String,
    turn_id: Option<String>,
    text: String,
    _selected: usize,
) -> AnyView {
    let body = view! {
        <div class="modal-form">
            <pre class="message-preview">{text.clone()}</pre>
        </div>
    }
    .into_any();
    let copy_text = text.clone();
    let rewind_turn = turn_id.clone();
    let fork_turn = turn_id.clone();
    let actions = view! {
        <button class="btn" on:click=move |_| crate::store::copy_text(copy_text.clone())>"Copy"</button>
        <button class="btn" on:click=move |_| {
            let hint = rewind_turn.clone();
            store.update(|s| open_rewind_modal(s, hint.as_deref()));
        }>"Rewind turn"</button>
        <button class="btn" on:click=move |_| {
            if let (Some(session_id), Some(turn)) = (store.with(|s| s.active_session_id.clone()), fork_turn.clone())
                && let Some(t) = crate::transport::current()
            {
                t.fork_session(session_id, Some(turn));
            }
            store.update(|s| s.modal = None);
        }>"Fork before"</button>
    }
    .into_any();
    shell("Message", store, body, actions)
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
            <h3>"Shortcuts"</h3>
            <div class="help-list">
                <div class="help-row"><code>"Alt+K"</code><span>"Command palette"</span></div>
                <div class="help-row"><code>"Alt+A"</code><span>"Focus composer"</span></div>
                <div class="help-row"><code>"Alt+[ / Alt+]"</code><span>"Previous / next tab"</span></div>
                <div class="help-row"><code>"Alt+1…9"</code><span>"Select tab"</span></div>
                <div class="help-row"><code>"Alt+M"</code><span>"Cycle approval mode"</span></div>
                <div class="help-row"><code>"Ctrl+B"</code><span>"Toggle sidebar"</span></div>
                <div class="help-row"><code>"Alt+G"</code><span>"Steer the oldest queued prompt"</span></div>
                <div class="help-row"><code>"Alt+X"</code><span>"Cancel the running turn"</span></div>
                <div class="help-row"><code>"Alt+C"</code><span>"Copy the latest answer"</span></div>
                <div class="help-row"><code>"?"</code><span>"This help"</span></div>
            </div>
            <div class="hint">"Alt is Option on macOS. Browser-reserved shortcuts are not used."</div>
        </div>
    }
    .into_any();
    shell("Help", store, body, view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }.into_any())
}

#[allow(clippy::too_many_arguments)]
fn subagent_modal(
    store: StoreHandle,
    _entry_id: String,
    agent: String,
    status: String,
    child_session_id: Option<String>,
    entries: Vec<meerai_web_protocol::TranscriptEntryDto>,
    usage: Option<meerai_web_protocol::UsageDto>,
    loading: bool,
    error: Option<String>,
) -> AnyView {
    let lines = entries
        .iter()
        .filter_map(|entry| match entry {
            meerai_web_protocol::TranscriptEntryDto::Assistant { text, .. }
                if !text.trim().is_empty() =>
            {
                Some(view! { <div class="child-line">{text.clone()}</div> }.into_any())
            }
            meerai_web_protocol::TranscriptEntryDto::ToolCall { name, status, .. } => Some(
                view! { <div class="child-line tool">{format!("{name} · {status}")}</div> }
                    .into_any(),
            ),
            meerai_web_protocol::TranscriptEntryDto::System { text, .. } => {
                Some(view! { <div class="child-line system">{text.clone()}</div> }.into_any())
            }
            meerai_web_protocol::TranscriptEntryDto::Error { message, .. } => {
                Some(view! { <div class="child-line error">{message.clone()}</div> }.into_any())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let body = view! {
        <div class="modal-form">
            <div class="kv"><span>"Agent"</span><strong>{agent}</strong></div>
            <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
            <div class="kv"><span>"Child session"</span><code>{child_session_id.clone().unwrap_or_else(|| "none".to_owned())}</code></div>
            {usage.map(|usage| view! {
                <div class="kv"><span>"Usage"</span><strong>{format!("{} prompt · {} completion", usage.prompt_tokens.unwrap_or(0), usage.completion_tokens.unwrap_or(0))}</strong></div>
            }.into_any())}
            {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
            <Show when=move || loading>
                <div class="hint">"Loading child transcript…"</div>
            </Show>
            <div class="child-transcript">{AnyView::from(Fragment::new(lines))}</div>
        </div>
    }
    .into_any();
    shell("Subagent", store, body, view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }.into_any())
}

fn diff_body(
    store: StoreHandle,
    selected_path: Option<String>,
    unified: bool,
    diff: Option<FileDiffResponse>,
    loading: bool,
    error: Option<String>,
) -> AnyView {
    let files = store.with(|s| s.file_changes.clone());
    let file_count = files.len();
    let file_rows = files
        .into_iter()
        .map(|change| {
            let path = change.path.clone();
            let active = selected_path.as_deref() == Some(change.path.as_str());
            let status = change.status.clone();
            view! {
                <button class="diff-file-row" class:active=active on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.fetch_workspace_diff(path.clone());
                    }
                    store.update(|s| {
                        if let Some(ActiveModal::DiffView { selected_path, loading, .. }) = &mut s.modal {
                            *selected_path = Some(path.clone());
                            *loading = true;
                        }
                    });
                }>
                    <span class=format!("diff-badge {}", status.to_ascii_lowercase())>{status.clone()}</span>
                    <span class="diff-file-name">{change.path.clone()}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();

    let diff_view = if let Some(diff) = diff {
        let path = diff.path.clone();
        let status = diff.status.clone();
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
                    <span class=format!("diff-badge {}", status.to_ascii_lowercase())>{status.clone()}</span>
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
                    <span class="badge">{file_count}</span>
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

fn search_body(
    store: StoreHandle,
    mode: SearchModeDto,
    query: &str,
    results: Option<meerai_web_protocol::WorkspaceSearchResponse>,
    _selected: usize,
    preview: Option<meerai_web_protocol::WorkspacePreviewResponse>,
    loading: bool,
) -> AnyView {
    let matches = results
        .as_ref()
        .map(|results| {
            results
                .matches
                .iter()
                .map(|result| {
                    let path = result.path.clone();
                    let line = result.line;
                    view! {
                        <button class="pick-row" on:click=move |_| {
                            if let Some(t) = crate::transport::current() {
                                t.preview_workspace(path.clone(), line);
                            }
                        }>
                            <span class="pick-title">{result.path.clone()}</span>
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
                        <div class="preview-line">
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
            view! { <div class="hint">"Select a result to preview."</div> }.into_any()
        });
    view! {
        <div class="modal-form">
            <div class="search-bar">
                <input
                    class="input"
                    placeholder="Search files or content"
                    prop:value=query.to_owned()
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::Search { query, .. }) = &mut s.modal {
                                query.replace_text(value);
                            }
                        });
                    }
                />
                <button class="btn" on:click=move |_| {
                    let query = store.with(|s| match &s.modal {
                        Some(ActiveModal::Search { query, .. }) => query.text.clone(),
                        _ => String::new(),
                    });
                    if !query.trim().is_empty() {
                        if let Some(t) = crate::transport::current() {
                            let mode = store.with(|s| match &s.modal {
                                Some(ActiveModal::Search { mode, .. }) => *mode,
                                _ => mode,
                            });
                            t.search_workspace(mode, query);
                        }
                    }
                }>"Search"</button>
            </div>
            <div class="choice-grid">
                <button class="choice" class:active=mode == SearchModeDto::Files on:click=move |_| set_search_mode(&store, SearchModeDto::Files)>
                    <span class="choice-title">"File names"</span>
                </button>
                <button class="choice" class:active=mode == SearchModeDto::Content on:click=move |_| set_search_mode(&store, SearchModeDto::Content)>
                    <span class="choice-title">"Content"</span>
                </button>
            </div>
            <Show when=move || loading>
                <div class="hint">"Searching…"</div>
            </Show>
            <div class="split">
                <div class="split-side">{AnyView::from(Fragment::new(matches))}</div>
                <div class="split-main">{preview_view}</div>
            </div>
        </div>
    }
    .into_any()
}

fn set_search_mode(store: &StoreHandle, next: SearchModeDto) {
    store.update(|s| {
        if let Some(ActiveModal::Search { mode, .. }) = &mut s.modal {
            *mode = next;
        }
    });
}
