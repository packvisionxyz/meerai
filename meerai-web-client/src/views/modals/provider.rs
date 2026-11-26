use super::*;

#[component]
pub(super) fn ProviderSelectModal(store: StoreHandle) -> impl IntoView {
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
pub(super) fn AuthMethodModal(store: StoreHandle) -> impl IntoView {
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
pub(super) fn ProviderConnectionModal(store: StoreHandle) -> impl IntoView {
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
pub(super) fn ChatGptLoginModal(store: StoreHandle) -> impl IntoView {
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
pub(super) fn ApiKeyModal(store: StoreHandle) -> impl IntoView {
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
pub(super) fn ModelInputModal(store: StoreHandle) -> impl IntoView {
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
                            prop:value=move || store.with(|s| match &s.modal {
                                Some(ActiveModal::ModelInput { filter, .. }) => filter.text.clone(),
                                _ => String::new(),
                            })
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
                        <div class="hint">
                            "Type to filter. ↑↓ select, Tab completes the highlighted model, Enter uses it."
                        </div>
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

pub(super) fn find_provider(store: &StoreHandle, id: &str) -> Option<ProviderItemDto> {
    store.with(|s| {
        s.providers
            .as_ref()
            .and_then(|providers| providers.providers.iter().find(|p| p.id == id).cloned())
    })
}

pub(super) fn find_provider_state<'a>(
    state: &'a crate::state::AppState,
    id: &str,
) -> Option<&'a ProviderItemDto> {
    state
        .providers
        .as_ref()
        .and_then(|providers| providers.providers.iter().find(|p| p.id == id))
}

pub(super) fn provider_row(
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

pub(super) fn model_row(
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
