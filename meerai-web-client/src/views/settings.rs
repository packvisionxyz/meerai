use std::collections::HashSet;

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{
    ApprovalModeDto, AuthKindDto, BackendKind, EffortDto, ProviderItemDto, ThemeOverrideDto,
    UpdateSettingsRequest,
};

use crate::state::{
    ActiveModal, ConnectionState, ProviderModalReturn, SettingsFormState, SettingsSection,
    activate_provider_from_modal, filtered_providers_sorted, open_model_input,
    open_provider_model_select, provider_for_model,
};
use crate::store::StoreHandle;
use crate::theme::{BrowserTheme, is_valid_color};
use crate::views::mode_label;

#[component]
pub fn SettingsModal(store: StoreHandle) -> impl IntoView {
    let section = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Settings(form)) => Some(form.section),
            _ => None,
        })
    });
    let body_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        section.track();
        if let Some(body) = body_ref.get() {
            body.set_scroll_top(0);
        }
    });
    let status_and_busy = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Settings(form)) => (form.status.clone(), form.busy),
            _ => (None, false),
        })
    });
    let writable = Memo::new(move |_| store.with(|s| s.host_info.settings.config_writable));

    let tabs = [
        (SettingsSection::Connections, "Providers & Models"),
        (SettingsSection::Session, "Current session"),
        (SettingsSection::Context, "Context & Limits"),
        (SettingsSection::Theme, "Theme"),
        (SettingsSection::Sessions, "Retention"),
        (SettingsSection::Sandbox, "Sandbox"),
        (SettingsSection::Permissions, "Permissions"),
    ];

    let tab_rows = tabs
        .into_iter()
        .map(|(tab, label)| {
            view! {
                <button
                    class="settings-tab"
                    class:active=move || section.get() == Some(tab)
                    aria-current=move || (section.get() == Some(tab)).then_some("true")
                    on:click=move |_| {
                        store.update(|s| {
                            if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                form.section = tab;
                            }
                        });
                    }
                >
                    {label}
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();

    let shows_save = move || {
        matches!(
            section.get(),
            Some(
                SettingsSection::Theme
                    | SettingsSection::Context
                    | SettingsSection::Sessions
                    | SettingsSection::Sandbox
            )
        )
    };

    view! {
        <div class="modal-backdrop" on:click=move |ev| {
            if ev.target() == ev.current_target() {
                store.update(|s| s.modal = None);
            }
        }>
            <div class="modal modal-wide settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-title" tabindex="-1">
                <div class="modal-head">
                    <h2 id="settings-title">"Settings"</h2>
                    <button class="modal-close" aria-label="Close settings" title="Close (Esc)" on:click=move |_| store.update(|s| s.modal = None)>"×"</button>
                </div>
                <div class="settings-layout">
                    <nav class="settings-tabs" aria-label="Settings sections">
                        {AnyView::from(Fragment::new(tab_rows))}
                    </nav>
                    <div class="settings-body" node_ref=body_ref>
                        {move || match section.get() {
                            Some(SettingsSection::Connections) => connections_body(store),
                            Some(SettingsSection::Session) => session_body(store),
                            Some(SettingsSection::Theme) => theme_body(store),
                            Some(SettingsSection::Context) => context_body(store),
                            Some(SettingsSection::Sessions) => sessions_body(store),
                            Some(SettingsSection::Sandbox) => sandbox_body(store),
                            Some(SettingsSection::Permissions) => permissions_body(store),
                            None => view! { <div></div> }.into_any(),
                        }}
                        {move || {
                            if shows_save() && !writable.get() {
                                view! {
                                    <div class="banner notice">
                                        "User settings file is read-only on this host."
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }
                        }}
                        {move || {
                            status_and_busy.get().0.map(|status| {
                                view! { <div class="banner notice settings-status" role="status">{status}</div> }.into_any()
                            })
                        }}
                    </div>
                </div>
                <div class="modal-actions">
                    <button
                        class="btn"
                        disabled=move || status_and_busy.get().1
                        on:click=move |_| {
                            if let Some(t) = crate::transport::current() {
                                t.reload_config();
                                t.fetch_providers();
                            }
                            store.update(|s| {
                                if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                    form.status = Some("Reloading configuration…".to_owned());
                                    form.busy = true;
                                }
                            });
                        }
                    >
                        "Reload config"
                    </button>
                    <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
                    <Show when=shows_save>
                        <button
                            class="btn primary"
                            disabled=move || status_and_busy.get().1 || !writable.get()
                            on:click=move |_| {
                                let result = store.with(|s| match &s.modal {
                                    Some(ActiveModal::Settings(form)) => form.to_request(),
                                    _ => Err("Settings are not open.".to_owned()),
                                });
                                match result {
                                    Ok(request) => {
                                        if let Some(t) = crate::transport::current() {
                                            t.update_settings(request);
                                        }
                                        store.update(|s| {
                                            if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                                form.busy = true;
                                                form.status = Some("Saving…".to_owned());
                                            }
                                        });
                                    }
                                    Err(error) => {
                                        store.update(|s| {
                                            if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                                form.status = Some(error);
                                            }
                                        });
                                    }
                                }
                            }
                        >
                            "Save changes"
                        </button>
                    </Show>
                </div>
            </div>
        </div>
    }
}

fn connections_body(store: StoreHandle) -> AnyView {
    let initial_filter = store.with(|s| match &s.modal {
        Some(ActiveModal::Settings(form)) => form.provider_filter.text.clone(),
        _ => String::new(),
    });
    let summary_memo = Memo::new(move |_| {
        store.with(|s| {
            let active_session = s.active_session().cloned();
            let active_badge = active_session.as_ref().and_then(|sess| {
                provider_for_model(s.providers.as_ref(), &sess.provider)
                    .map(|p| (p.status_badge.clone(), p.configured))
            });
            let default_provider = s
                .providers
                .as_ref()
                .and_then(|p| p.default_provider.clone());
            (active_session, active_badge, default_provider)
        })
    });
    let providers_memo = Memo::new(move |_| {
        store.with(|s| {
            let filter = match &s.modal {
                Some(ActiveModal::Settings(form)) => form.provider_filter.text.as_str(),
                _ => "",
            };
            let active_provider = s.active_session().map(|sess| sess.provider.clone());
            let loading = s.providers.is_none();
            let items =
                filtered_providers_sorted(s.providers.as_ref(), active_provider.as_deref(), filter);
            (loading, active_provider, items)
        })
    });

    view! {
        <div class="settings-section">
            <h3>"Active model"</h3>
            {move || {
                let (session, active_badge, default_provider) = summary_memo.get();
                match session {
                    Some(sess) => {
                        let is_acp = matches!(sess.backend, BackendKind::Acp { .. });
                        view! {
                            <div class="settings-summary-card">
                                <div class="settings-summary-main">
                                    <div class="settings-summary-title">
                                        <strong>
                                            {if is_acp {
                                                "ACP session".to_owned()
                                            } else {
                                                format!("{}/{}", sess.provider, sess.model)
                                            }}
                                        </strong>
                                        {(!is_acp).then(|| active_badge.map(|(badge, configured)| view! {
                                            <span class=format!("badge {}", if configured { "ok" } else { "warn" })>{badge}</span>
                                        })).flatten()}
                                    </div>
                                    <div class="hint">
                                        {if is_acp {
                                            "External ACP sessions manage their own model and provider."
                                        } else {
                                            "Active session model. Changes apply immediately."
                                        }}
                                        {default_provider.map(|dp| format!(" · Host default provider: {dp}")).unwrap_or_default()}
                                    </div>
                                </div>
                                {(!is_acp).then(|| view! {
                                    <button class="btn primary" on:click=move |_| store.update(open_model_input)>
                                        "Choose model…"
                                    </button>
                                })}
                            </div>
                        }
                        .into_any()
                    }
                    None => view! {
                        <div class="settings-summary-card">
                            <div class="hint">"No active session. Open or start a session to pick a model."</div>
                        </div>
                    }.into_any(),
                }
            }}

            <h3>"Providers"</h3>
            <p class="hint">"Connect API keys, ChatGPT plan sign-in, or custom Base URLs. Credentials stay on the host."</p>
            <input
                class="input settings-filter-input"
                placeholder="Filter providers (name, id, status)…"
                prop:value=initial_filter
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                            form.provider_filter.replace_text(value);
                        }
                    });
                }
            />
            {move || {
                let (loading, active_provider, providers) = providers_memo.get();
                if loading {
                    return view! { <div class="hint">"Loading providers…"</div> }.into_any();
                }
                if providers.is_empty() {
                    return view! { <div class="hint">"No providers match this filter."</div> }.into_any();
                }
                let can_pick_model = store.with(|s| {
                    s.active_session()
                        .is_some_and(|sess| !matches!(sess.backend, BackendKind::Acp { .. }))
                });
                let (configured_list, available_list): (Vec<_>, Vec<_>) =
                    providers.into_iter().partition(|p| p.configured);
                view! {
                    <>
                        {(!configured_list.is_empty()).then(|| {
                            let rows = configured_list
                                .into_iter()
                                .map(|p| settings_provider_row(store, p, active_provider.as_deref(), can_pick_model))
                                .collect::<Vec<_>>();
                            view! {
                                <div class="settings-group-title">"Configured"</div>
                                {AnyView::from(Fragment::new(rows))}
                            }
                        })}
                        {(!available_list.is_empty()).then(|| {
                            let rows = available_list
                                .into_iter()
                                .map(|p| settings_provider_row(store, p, active_provider.as_deref(), can_pick_model))
                                .collect::<Vec<_>>();
                            view! {
                                <div class="settings-group-title">"Available"</div>
                                {AnyView::from(Fragment::new(rows))}
                            }
                        })}
                    </>
                }
                .into_any()
            }}
        </div>
    }
    .into_any()
}

fn settings_provider_row(
    store: StoreHandle,
    provider: ProviderItemDto,
    active_provider: Option<&str>,
    can_pick_model: bool,
) -> AnyView {
    let is_active = active_provider == Some(provider.id.as_str());
    let model_id = provider.id.clone();
    let connect_id = provider.id.clone();
    let action_label = if !provider.configured {
        "Connect"
    } else if provider.auth_kind == AuthKindDto::None {
        "Base URL"
    } else {
        "Manage"
    };
    let mut meta_parts = vec![provider.id.clone(), provider.adapter.clone()];
    if let Some(url) = &provider.base_url
        && !url.is_empty()
    {
        meta_parts.push(url.clone());
    }
    if let Some(dm) = &provider.default_model
        && !dm.is_empty()
    {
        meta_parts.push(format!("default: {dm}"));
    }
    if !provider.configured && !provider.env_vars.is_empty() {
        meta_parts.push(format!("env: {}", provider.env_vars.join(" / ")));
    }
    view! {
        <div class="settings-row settings-provider-row">
            <div class="settings-provider-info">
                <div class="settings-provider-head">
                    <strong>{provider.display_name.clone()}</strong>
                    {is_active.then(|| view! { <span class="badge ok">"active"</span> }.into_any())}
                    {provider.is_custom.then(|| view! { <span class="badge">"custom"</span> }.into_any())}
                    <span class=format!("badge {}", if provider.configured { "ok" } else { "muted" })>
                        {provider.status_badge.clone()}
                    </span>
                </div>
                <div class="hint">{meta_parts.join(" · ")}</div>
            </div>
            <div class="settings-row-actions settings-provider-actions">
                <button
                    class="btn tiny"
                    disabled=!can_pick_model
                    title=if can_pick_model { "Browse or set models for this provider" } else { "Open a native session to pick a model" }
                    on:click=move |_| {
                        let id = model_id.clone();
                        store.update(|s| open_provider_model_select(s, id, ProviderModalReturn::SettingsConnections));
                    }
                >"Models"</button>
                <button class="btn tiny" on:click=move |_| {
                    let id = connect_id.clone();
                    store.update(|s| {
                        let provider = s.providers.as_ref().and_then(|p| {
                            p.providers.iter().find(|item| item.id == id).cloned()
                        });
                        if let Some(provider) = provider {
                            activate_provider_from_modal(s, provider, ProviderModalReturn::SettingsConnections);
                        }
                    });
                }>{action_label}</button>
            </div>
        </div>
    }
    .into_any()
}

fn session_body(store: StoreHandle) -> AnyView {
    let session_memo = Memo::new(move |_| store.with(|s| s.active_session().cloned()));
    let agents = store.with(|s| s.host_info.agents.clone());
    let skills = store.with(|s| s.host_info.skills.clone());

    view! {
        <div class="settings-section">
            {move || {
                let Some(session) = session_memo.get() else {
                    return view! { <div class="hint">"No active session."</div> }.into_any();
                };
                let mode = session.approval_mode;
                let (backend_label, acp) = match &session.backend {
                    BackendKind::Acp { preset, .. } => (format!("ACP ({preset})"), true),
                    BackendKind::Native => ("Native".to_owned(), false),
                };
                let sid = session.id.clone();
                let mode_row = [
                    (ApprovalModeDto::Plan, "Read-only exploration"),
                    (ApprovalModeDto::Ask, "Ask before writes & commands"),
                    (ApprovalModeDto::Auto, "Auto-approve allowed tools"),
                ]
                .into_iter()
                .map(|(candidate, desc)| {
                    let session_id = sid.clone();
                    let active = candidate == mode;
                    view! {
                        <button
                            class="choice"
                            class:active=active
                            disabled=acp && candidate == ApprovalModeDto::Plan
                            on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.set_mode(session_id.clone(), candidate);
                                }
                            }
                        >
                            <span class="choice-title">{mode_label(candidate)}</span>
                            <span class="choice-desc">{desc}</span>
                        </button>
                    }
                    .into_any()
                })
                .collect::<Vec<_>>();
                let agent_rows = agents
                    .iter()
                    .map(|agent| {
                        let id = agent.id.clone();
                        let session_id = sid.clone();
                        view! {
                            <button class="settings-card" on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.apply_agent(session_id.clone(), id.clone());
                                }
                                let applied = id.clone();
                                store.update(|s| {
                                    if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                        form.status = Some(format!("Applied agent @{applied} to session."));
                                    }
                                });
                            }>
                                <strong>{format!("@agent/{}", agent.id)}</strong>
                                <span class="hint">{agent.description.clone().unwrap_or_default()}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                let skill_rows = skills
                    .iter()
                    .map(|skill| {
                        let id = skill.id.clone();
                        let session_id = sid.clone();
                        view! {
                            <button class="settings-card" on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.apply_skill(session_id.clone(), id.clone());
                                }
                                let applied = id.clone();
                                store.update(|s| {
                                    if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                        form.status = Some(format!("Applied skill @{applied} to session."));
                                    }
                                });
                            }>
                                <strong>{format!("@skill/{}", skill.id)}</strong>
                                <span class="hint">{skill.description.clone().unwrap_or_default()}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                let no_agents = agent_rows.is_empty();
                let no_skills = skill_rows.is_empty();
                let model_provider = session.provider.clone();
                view! {
                    <div>
                        <h3>"Current session"</h3>
                        <p class="hint">"Applies immediately to the active session."</p>
                        <div class="settings-row"><span>"Session ID"</span><code>{session.id.clone()}</code></div>
                        <div class="settings-row"><span>"Backend"</span><strong>{backend_label}</strong></div>
                        <div class="settings-row">
                            <span>"Model"</span>
                            <div class="settings-row-actions">
                                <strong>{format!("{}/{}", session.provider, session.model)}</strong>
                                {(!acp).then(|| view! {
                                    <button class="btn tiny" on:click=move |_| {
                                        let p = model_provider.clone();
                                        store.update(|s| open_provider_model_select(s, p, ProviderModalReturn::SettingsConnections));
                                    }>"Change"</button>
                                })}
                            </div>
                        </div>
                        <div class="settings-row"><span>"Approval mode"</span></div>
                        <div class="choice-grid">{AnyView::from(Fragment::new(mode_row))}</div>
                        {if acp { view! { <div class="banner notice">"External ACP sessions keep their own tool policy. plan mode is not available."</div> }.into_any() } else { view! { <span></span> }.into_any() }}
                        <h3>"Agents"</h3>
                        <p class="hint">"Click an agent to inject its instructions into this session."</p>
                        {if no_agents {
                            view! { <div class="hint">"No agents discovered in this workspace."</div> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(agent_rows))
                        }}
                        <h3>"Skills"</h3>
                        <p class="hint">"Click a skill to inject its instructions into this session."</p>
                        {if no_skills {
                            view! { <div class="hint">"No skills discovered in this workspace."</div> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(skill_rows))
                        }}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
    .into_any()
}

fn theme_body(store: StoreHandle) -> AnyView {
    let appearance = expect_context::<RwSignal<BrowserTheme>>();
    let initial = store.with(|s| match &s.modal {
        Some(ActiveModal::Settings(form)) => (
            form.theme_fields.clone(),
            form.reset_theme,
            s.host_info.settings.theme.clone(),
        ),
        _ => (Default::default(), false, Default::default()),
    });
    let (fields, reset, effective) = initial;
    let rows = fields
        .into_iter()
        .map(|(key, value)| {
            let initial_val = value.text;
            let placeholder = theme_value(&effective, key);
            let swatch_color = move || {
                store.with(|s| match &s.modal {
                    Some(ActiveModal::Settings(form)) => {
                        let val = form
                            .theme_fields
                            .get(key)
                            .map(|f| f.text.trim())
                            .unwrap_or("");
                        if !val.is_empty() && is_valid_color(val) {
                            val.to_owned()
                        } else {
                            theme_value(&s.host_info.settings.theme, key)
                        }
                    }
                    _ => String::new(),
                })
            };
            view! {
                <label class="settings-row settings-field">
                    <span>{theme_label(key)}</span>
                    <div class="theme-input-group">
                        <span
                            class="color-swatch"
                            style=move || format!("background:{}", swatch_color())
                            title="Effective color"
                        ></span>
                        <input
                            class="input"
                            placeholder=placeholder
                            prop:value=initial_val
                            on:input=move |ev| {
                                let next = event_target_value(&ev);
                                store.update(|s| {
                                    if let Some(ActiveModal::Settings(form)) = &mut s.modal
                                        && let Some(field) = form.theme_fields.get_mut(key)
                                    {
                                        field.replace_text(next);
                                    }
                                });
                            }
                        />
                    </div>
                </label>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    view! {
        <div class="settings-section">
            <h3>"Browser appearance"</h3>
            <label class="settings-row settings-field">
                <span>"Color scheme"</span>
                <select class="input" aria-describedby="browser-theme-help" on:change=move |ev| {
                    let theme = BrowserTheme::from_key(&event_target_value(&ev));
                    appearance.set(theme);
                    theme.save();
                    let scheme_key = theme.key().to_owned();
                    let can_persist = store.with(|s| {
                        s.host_info.settings.config_writable
                            && s.connection == ConnectionState::Connected
                    });
                    store.update(|s| {
                        if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                            form.color_scheme = Some(scheme_key.clone());
                        }
                    });
                    if can_persist && let Some(t) = crate::transport::current() {
                        t.update_settings(UpdateSettingsRequest {
                            reset_theme: false,
                            theme: Some(ThemeOverrideDto {
                                color_scheme: Some(scheme_key),
                                ..ThemeOverrideDto::default()
                            }),
                            context: None,
                            session: None,
                            sandbox: None,
                            forget_model: None,
                        });
                    }
                }>
                    {BrowserTheme::OPTIONS.into_iter().map(|(theme, label)| view! {
                        <option value=theme.key() prop:selected=move || appearance.get() == theme>{label}</option>
                    }).collect::<Vec<_>>()}
                </select>
            </label>
            <p class="hint" id="browser-theme-help">"Applies immediately and saves to user settings when connected. System follows your device. Select Host theme to use the colors below."</p>
            <h3>"Host theme colors"</h3>
            <label class="settings-row settings-toggle">
                <span>"Reset to default theme"</span>
                <input
                    type="checkbox"
                    prop:checked=reset
                    on:change=move |ev| {
                        let checked = event_target_checked(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                form.reset_theme = checked;
                            }
                        });
                    }
                />
            </label>
            <div class="hint">"Values accept #rgb, #rrggbb, or a CSS color name. Leave blank to keep existing overrides, or check Reset above."</div>
            {AnyView::from(Fragment::new(rows))}
        </div>
    }
    .into_any()
}

fn context_body(store: StoreHandle) -> AnyView {
    let form = store.with(|s| match &s.modal {
        Some(ActiveModal::Settings(form)) => form.clone(),
        _ => SettingsFormState::from_host(&s.host_info),
    });
    let model_specs = store.with(|s| {
        let Some(resp) = s.providers.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for model in &resp.models {
            let adapter = resp
                .providers
                .iter()
                .find(|p| p.id == model.provider)
                .map(|p| p.adapter.as_str())
                .unwrap_or(model.provider.as_str());
            let spec = format!("{adapter}::{}", model.id);
            if seen.insert(spec.clone()) {
                out.push(spec);
            }
        }
        for (provider_id, recent) in &resp.recent_models {
            let adapter = resp
                .providers
                .iter()
                .find(|p| p.id == *provider_id)
                .map(|p| p.adapter.as_str())
                .unwrap_or(provider_id.as_str());
            for model_id in recent {
                let spec = format!("{adapter}::{model_id}");
                if seen.insert(spec.clone()) {
                    out.push(spec);
                }
            }
        }
        out
    });

    let render_input = |key: &'static str, datalist: Option<&'static str>| {
        let value = form.field_text(key).to_owned();
        let (label, help, placeholder) = context_field_meta(key);
        view! {
            <label class="settings-row settings-field">
                <div class="settings-field-label">
                    <span>{label}</span>
                    {(!help.is_empty()).then(|| view! { <span class="settings-field-help">{help}</span> })}
                </div>
                <input
                    class="input"
                    list=datalist
                    placeholder=placeholder
                    prop:value=value
                    on:input=move |ev| {
                        let next = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::Settings(form)) = &mut s.modal
                                && let Some(field) = form.inputs.get_mut(key)
                            {
                                field.replace_text(next);
                            }
                        });
                    }
                />
            </label>
        }
        .into_any()
    };

    let render_effort = |current: Option<EffortDto>| {
        let selected = current.unwrap_or(EffortDto::Medium);
        let option = |value: EffortDto, label: &'static str, temp: &'static str| {
            let is_selected = selected == value;
            view! {
                <option value=label prop:selected=is_selected>
                    {format!("{label} · temp {temp}")}
                </option>
            }
        };
        view! {
            <label class="settings-row settings-field">
                <div class="settings-field-label">
                    <span>"Reasoning effort"</span>
                    <span class="settings-field-help">
                        "Sets temperature per tier: low 0.00, medium 0.20, high 0.50."
                    </span>
                </div>
                <select
                    class="input"
                    on:change=move |ev| {
                        let next = match event_target_value(&ev).as_str() {
                            "low" => Some(EffortDto::Low),
                            "high" => Some(EffortDto::High),
                            _ => Some(EffortDto::Medium),
                        };
                        store.update(|s| {
                            if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                                form.effort = next;
                                if let Some(field) = form.inputs.get_mut("temperature") {
                                    field.clear();
                                }
                            }
                        });
                    }
                >
                    {option(EffortDto::Low, "low", "0.00")}
                    {option(EffortDto::Medium, "medium", "0.20")}
                    {option(EffortDto::High, "high", "0.50")}
                </select>
            </label>
        }
        .into_any()
    };

    let render_toggle = |key: &'static str, checked: bool| {
        let (label, help) = toggle_meta(key);
        view! {
            <label class="settings-row settings-toggle">
                <div class="settings-field-label">
                    <span>{label}</span>
                    {(!help.is_empty()).then(|| view! { <span class="settings-field-help">{help}</span> })}
                </div>
                <input type="checkbox" prop:checked=checked on:change=move |ev| {
                    let next = event_target_checked(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                            match key {
                                "reasoning_pruning" => form.reasoning_pruning = next,
                                "prompt_caching" => form.prompt_caching = next,
                                "rollback_on_verify_failure" => form.rollback_on_verify_failure = next,
                                "post_edit_diagnostics" => form.post_edit_diagnostics = next,
                                _ => {}
                            }
                        }
                    });
                } />
            </label>
        }
        .into_any()
    };

    view! {
        <div class="settings-section">
            <h3>"Context & Limits"</h3>
            <p class="hint">"Saved to the user settings database on the host. Leave numeric fields blank to use defaults."</p>
            <datalist id="context-model-specs">
                {model_specs.into_iter().map(|spec| view! { <option value=spec></option> }).collect::<Vec<_>>()}
            </datalist>

            <div class="settings-group-title">"Models & Sampling"</div>
            <div class="settings-list">
                {
                let effort = store.with(|s| match &s.modal {
                    Some(ActiveModal::Settings(form)) => form.effort,
                    _ => None,
                });
                render_effort(effort)
            }
                {render_input("temperature", None)}
                {render_input("recovery_model", Some("context-model-specs"))}
                {render_input("summarizer_model", Some("context-model-specs"))}
            </div>

            <div class="settings-group-title">"Context & Compaction"</div>
            <div class="settings-list">
                {render_input("context_window", None)}
                {render_input("high_watermark_pct", None)}
                {render_input("spill_max_inline", None)}
                {render_input("verbatim_window_turns", None)}
                {render_toggle("reasoning_pruning", form.reasoning_pruning)}
                {render_toggle("prompt_caching", form.prompt_caching)}
            </div>

            <div class="settings-group-title">"Execution Limits & Verification"</div>
            <div class="settings-list">
                {render_input("max_steps_per_turn", None)}
                {render_input("tool_timeout_ms", None)}
                {render_input("max_session_tokens", None)}
                {render_input("max_turn_secs", None)}
                {render_toggle("post_edit_diagnostics", form.post_edit_diagnostics)}
                {render_toggle("rollback_on_verify_failure", form.rollback_on_verify_failure)}
            </div>
        </div>
    }
    .into_any()
}

fn sessions_body(store: StoreHandle) -> AnyView {
    let (days, keep) = store.with(|s| match &s.modal {
        Some(ActiveModal::Settings(form)) => (
            form.field_text("auto_prune_days").to_owned(),
            form.field_text("session_keep").to_owned(),
        ),
        _ => (String::new(), String::new()),
    });
    view! {
        <div class="settings-section">
            <h3>"Session retention"</h3>
            <label class="settings-row settings-field">
                <div class="settings-field-label">
                    <span>"Auto prune days"</span>
                    <span class="settings-field-help">"Delete idle sessions older than this many days on host startup."</span>
                </div>
                <input class="input" placeholder="Disabled" prop:value=days on:input=move |ev| {
                    let next = event_target_value(&ev);
                    store.update(|s| if let Some(ActiveModal::Settings(form)) = &mut s.modal && let Some(field) = form.inputs.get_mut("auto_prune_days") { field.replace_text(next); });
                } />
            </label>
            <label class="settings-row settings-field">
                <div class="settings-field-label">
                    <span>"Keep newest"</span>
                    <span class="settings-field-help">"Always keep at least this many most recent sessions."</span>
                </div>
                <input class="input" placeholder="Keep all" prop:value=keep on:input=move |ev| {
                    let next = event_target_value(&ev);
                    store.update(|s| if let Some(ActiveModal::Settings(form)) = &mut s.modal && let Some(field) = form.inputs.get_mut("session_keep") { field.replace_text(next); });
                } />
            </label>
            <div class="hint">"Only the user settings file controls retention. Session files are never pruned by the browser."</div>
        </div>
    }
    .into_any()
}

fn sandbox_body(store: StoreHandle) -> AnyView {
    let (req_shell, req_scripts, strict) = store.with(|s| match &s.modal {
        Some(ActiveModal::Settings(form)) => (
            form.require_for_shell,
            form.require_for_scripts,
            form.strict_env,
        ),
        _ => (false, false, false),
    });
    let toggles = [
        ("require_for_shell", req_shell),
        ("require_for_scripts", req_scripts),
        ("strict_env", strict),
    ]
    .into_iter()
    .map(|(key, checked)| {
        let (label, help) = toggle_meta(key);
        view! {
            <label class="settings-row settings-toggle">
                <div class="settings-field-label">
                    <span>{label}</span>
                    {(!help.is_empty()).then(|| view! { <span class="settings-field-help">{help}</span> })}
                </div>
                <input type="checkbox" prop:checked=checked on:change=move |ev| {
                    let next = event_target_checked(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                            match key {
                                "require_for_shell" => form.require_for_shell = next,
                                "require_for_scripts" => form.require_for_scripts = next,
                                "strict_env" => form.strict_env = next,
                                _ => {}
                            }
                        }
                    });
                } />
            </label>
        }
        .into_any()
    })
    .collect::<Vec<_>>();
    view! {
        <div class="settings-section">
            <h3>"Sandbox"</h3>
            {AnyView::from(Fragment::new(toggles))}
            <div class="hint">"The host enforces these settings. The browser cannot lower them."</div>
        </div>
    }
    .into_any()
}

fn permissions_body(store: StoreHandle) -> AnyView {
    let summary_memo =
        Memo::new(move |_| store.with(|s| s.host_info.settings.permissions_summary.clone()));
    view! {
        <div class="settings-section">
            <h3>"Permissions"</h3>
            <div class="hint">"Project rules can tighten permissions only. Edit settings files on the host."</div>
            <div class="settings-list">
                {move || {
                    let summary = summary_memo.get();
                    if summary.is_empty() {
                        return view! { <div class="hint">"No explicit permission rules configured."</div> }.into_any();
                    }
                    let rows = summary
                        .into_iter()
                        .map(|row| view! { <div class="settings-row"><code>{row}</code></div> }.into_any())
                        .collect::<Vec<_>>();
                    AnyView::from(Fragment::new(rows))
                }}
            </div>
        </div>
    }
    .into_any()
}

fn theme_label(key: &str) -> &'static str {
    match key {
        "background" => "Background",
        "surface" => "Surface",
        "surface_high" => "Surface high",
        "text" => "Text",
        "muted" => "Muted text",
        "faint" => "Faint text",
        "accent" => "Accent",
        "interactive" => "Interactive",
        "border" => "Border",
        "error" => "Error",
        "warning" => "Warning",
        "success" => "Success",
        "info" => "Info",
        "diff_added" => "Diff added",
        "diff_removed" => "Diff removed",
        "diff_context" => "Diff context",
        "code_function" => "Code function",
        _ => "Theme value",
    }
}

fn context_field_meta(key: &str) -> (&'static str, &'static str, &'static str) {
    match key {
        "context_window" => (
            "Context window (tokens)",
            "Override detected model context limit.",
            "Model default",
        ),
        "high_watermark_pct" => (
            "Compaction threshold (%)",
            "Trigger compaction at this % of context window (1–100).",
            "80",
        ),
        "spill_max_inline" => (
            "Inline output limit (tokens)",
            "Spill larger tool outputs to disk instead of keeping inline.",
            "Default",
        ),
        "verbatim_window_turns" => (
            "Verbatim recent turns",
            "Number of latest turns kept uncompacted.",
            "Default",
        ),
        "max_steps_per_turn" => (
            "Max steps per turn",
            "Maximum model/tool round-trips in one turn.",
            "Default",
        ),
        "tool_timeout_ms" => (
            "Tool timeout (ms)",
            "Per-tool execution timeout in milliseconds.",
            "Default",
        ),
        "max_session_tokens" => (
            "Session token cap",
            "Total token budget across the session.",
            "Unlimited",
        ),
        "max_turn_secs" => (
            "Turn time cap (s)",
            "Maximum wall-clock seconds per turn.",
            "Unlimited",
        ),
        "recovery_model" => (
            "Recovery model",
            "Fallback model spec used when the primary model fails.",
            "Same as session (e.g. openai::gpt-5)",
        ),
        "summarizer_model" => (
            "Summarizer model",
            "Model spec used for context compaction summaries.",
            "Same as session (e.g. openai::gpt-5)",
        ),
        "temperature" => (
            "Temperature",
            "Optional sampling temperature override from 0.0 to 2.0.",
            "Tier default (0.00 / 0.20 / 0.50)",
        ),
        _ => ("Value", "", ""),
    }
}

fn toggle_meta(key: &str) -> (&'static str, &'static str) {
    match key {
        "reasoning_pruning" => (
            "Prune reasoning",
            "Drop older reasoning blocks when compacting context.",
        ),
        "prompt_caching" => (
            "Prompt caching",
            "Enable provider prompt cache breakpoints where supported.",
        ),
        "rollback_on_verify_failure" => (
            "Rollback on verify failure",
            "Restore edited files if post-turn verification fails.",
        ),
        "post_edit_diagnostics" => (
            "Post-edit diagnostics",
            "Attach LSP diagnostics after file edits.",
        ),
        "require_for_shell" => (
            "Require sandbox for shell",
            "Require OS sandbox for code-shell commands.",
        ),
        "require_for_scripts" => (
            "Require sandbox for scripts",
            "Require OS sandbox for script execution.",
        ),
        "strict_env" => (
            "Strict environment",
            "Strip non-allowlisted environment variables from child processes.",
        ),
        _ => ("Option", ""),
    }
}

fn theme_value(theme: &meerai_web_protocol::ThemeTokensDto, key: &str) -> String {
    match key {
        "background" => theme.background.clone(),
        "surface" => theme.surface.clone(),
        "surface_high" => theme.surface_high.clone(),
        "text" => theme.text.clone(),
        "muted" => theme.muted.clone(),
        "faint" => theme.faint.clone(),
        "accent" => theme.accent.clone(),
        "interactive" => theme.interactive.clone(),
        "border" => theme.border.clone(),
        "error" => theme.error.clone(),
        "warning" => theme.warning.clone(),
        "success" => theme.success.clone(),
        "info" => theme.info.clone(),
        "diff_added" => theme.diff_added.clone(),
        "diff_removed" => theme.diff_removed.clone(),
        "diff_context" => theme.diff_context.clone(),
        "code_function" => theme.code_function.clone(),
        _ => String::new(),
    }
}
