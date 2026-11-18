use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::ApprovalModeDto;

use crate::state::{ActiveModal, SettingsFormState, SettingsSection};
use crate::store::StoreHandle;
use crate::views::mode_label;

pub fn render_settings(store: StoreHandle, form: SettingsFormState) -> AnyView {
    let section = form.section;
    let tabs = [
        (SettingsSection::Connections, "Connections"),
        (SettingsSection::Session, "Session"),
        (SettingsSection::Theme, "Theme"),
        (SettingsSection::Context, "Context"),
        (SettingsSection::Sessions, "Sessions"),
        (SettingsSection::Sandbox, "Sandbox"),
        (SettingsSection::Permissions, "Permissions"),
    ];
    let tab_rows = tabs
        .into_iter()
        .map(|(tab, label)| {
            let active = section == tab;
            view! {
                <button class="settings-tab" class:active=active on:click=move |_| {
                    store.update(|s| {
                        if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                            form.section = tab;
                        }
                    });
                }>{label}</button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();

    let body = match section {
        SettingsSection::Connections => connections_body(store, &form),
        SettingsSection::Session => session_body(store, &form),
        SettingsSection::Theme => theme_body(store, &form),
        SettingsSection::Context => context_body(store, &form),
        SettingsSection::Sessions => sessions_body(store, &form),
        SettingsSection::Sandbox => sandbox_body(store, &form),
        SettingsSection::Permissions => permissions_body(store),
    };

    let status = form.status.clone();
    let busy = form.busy;
    let actions = view! {
        <button class="btn primary" disabled=busy on:click=move |_| {
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
        }>"Save"</button>
        <button class="btn" on:click=move |_| {
            if let Some(t) = crate::transport::current() {
                t.reload_config();
            }
            store.update(|s| {
                if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                    form.status = Some("Reloading configuration…".to_owned());
                    form.busy = true;
                }
            });
        }>"Reload"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
    }
    .into_any();

    view! {
        <div class="modal-backdrop">
            <div class="modal modal-wide" role="dialog" aria-modal="true" tabindex="-1">
                <div class="modal-head">
                    <h2>"Settings"</h2>
                    <button class="modal-close" on:click=move |_| store.update(|s| s.modal = None)>"×"</button>
                </div>
                <div class="settings-layout">
                    <nav class="settings-tabs" aria-label="Settings sections">
                        {AnyView::from(Fragment::new(tab_rows))}
                    </nav>
                    <div class="settings-body">
                        {body}
                        {status.map(|status| view! { <div class="banner notice">{status}</div> }.into_any())}
                    </div>
                </div>
                <div class="modal-actions">{actions}</div>
            </div>
        </div>
    }
    .into_any()
}

fn connections_body(store: StoreHandle, form: &SettingsFormState) -> AnyView {
    let providers = store.with(|s| {
        s.providers
            .as_ref()
            .map(|p| p.providers.clone())
            .unwrap_or_default()
    });
    let rows = providers
        .into_iter()
        .map(|provider| {
            let connect_id = provider.id.clone();
            view! {
                <div class="settings-row">
                    <div>
                        <strong>{provider.display_name.clone()}</strong>
                        <div class="hint">{format!("{} · {}", provider.id, provider.status_badge)}</div>
                    </div>
                    <button class="btn tiny" on:click=move |_| {
                        let id = connect_id.clone();
                        store.update(|s| {
                            let provider = s.providers.as_ref().and_then(|p| {
                                p.providers.iter().find(|item| item.id == id).cloned()
                            });
                            if let Some(provider) = provider {
                                crate::state::activate_provider_from_modal(s, provider);
                            }
                        });
                    }>"Connect"</button>
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let _ = form;
    view! {
        <div class="settings-section">
            <h3>"Connections"</h3>
            <p class="hint">"Credential values stay on the host. The browser shows status only."</p>
            {AnyView::from(Fragment::new(rows))}
            <div class="settings-row">
                <span>"Model"</span>
                <button class="btn tiny" on:click=move |_| store.update(crate::state::open_model_input)>"Choose model"</button>
                <button class="btn tiny" on:click=move |_| store.update(crate::state::open_provider_select)>"Add provider"</button>
            </div>
        </div>
    }
    .into_any()
}

fn session_body(store: StoreHandle, form: &SettingsFormState) -> AnyView {
    let _ = form;
    let session = store.with(|s| s.active_session().cloned());
    let Some(session) = session else {
        return view! { <div class="hint">"No active session."</div> }.into_any();
    };
    let mode = session.approval_mode;
    let acp = matches!(
        session.backend,
        meerai_web_protocol::BackendKind::Acp { .. }
    );
    let sid = session.id.clone();
    let agents = store.with(|s| s.host_info.agents.clone());
    let skills = store.with(|s| s.host_info.skills.clone());
    let agent_rows = agents
        .into_iter()
        .map(|agent| {
            let id = agent.id.clone();
            let session_id = sid.clone();
            view! {
                <button class="settings-card" on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.apply_agent(session_id.clone(), id.clone());
                    }
                }>
                    <strong>{agent.id.clone()}</strong>
                    <span class="hint">{agent.description.clone().unwrap_or_default()}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let skill_rows = skills
        .into_iter()
        .map(|skill| {
            let id = skill.id.clone();
            let session_id = sid.clone();
            view! {
                <button class="settings-card" on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.apply_skill(session_id.clone(), id.clone());
                    }
                }>
                    <strong>{skill.id.clone()}</strong>
                    <span class="hint">{skill.description.clone().unwrap_or_default()}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let mode_row = [ApprovalModeDto::Plan, ApprovalModeDto::Ask, ApprovalModeDto::Auto]
        .into_iter()
        .map(|candidate| {
            let session_id = sid.clone();
            let active = candidate == mode;
            view! {
                <button class="choice" class:active=active disabled=acp && candidate == ApprovalModeDto::Plan on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.set_mode(session_id.clone(), candidate);
                    }
                }>
                    <span class="choice-title">{mode_label(candidate)}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let no_agents = agent_rows.is_empty();
    let no_skills = skill_rows.is_empty();
    view! {
        <div class="settings-section">
            <h3>"Session"</h3>
            <div class="settings-row"><span>"Session"</span><code>{session.id.clone()}</code></div>
            <div class="settings-row"><span>"Model"</span><strong>{format!("{}/{}", session.provider, session.model)}</strong></div>
            <div class="settings-row"><span>"Approval mode"</span></div>
            <div class="choice-grid">{AnyView::from(Fragment::new(mode_row))}</div>
            {if acp { view! { <div class="banner notice">"External ACP sessions keep their own tool policy. plan mode is not available."</div> }.into_any() } else { view! { <div></div> }.into_any() }}
            <h3>"Agents"</h3>
            {if no_agents {
                view! { <div class="hint">"No agents discovered in this workspace."</div> }.into_any()
            } else {
                AnyView::from(Fragment::new(agent_rows))
            }}
            <h3>"Skills"</h3>
            {if no_skills {
                view! { <div class="hint">"No skills discovered in this workspace."</div> }.into_any()
            } else {
                AnyView::from(Fragment::new(skill_rows))
            }}
        </div>
    }
    .into_any()
}

fn theme_body(store: StoreHandle, form: &SettingsFormState) -> AnyView {
    let fields = form.theme_fields.clone();
    let effective = store.with(|s| s.host_info.settings.theme.clone());
    let rows = fields
        .into_iter()
        .map(|(key, value)| {
            let value = value.text.clone();
            let placeholder = theme_value(&effective, key);
            view! {
                <div class="settings-row">
                    <span>{theme_label(key)}</span>
                    <input
                        class="input"
                        placeholder=placeholder
                        prop:value=value.clone()
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
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let reset = form.reset_theme;
    view! {
        <div class="settings-section">
            <h3>"Theme"</h3>
            <label class="settings-row">
                <span>"Reset to default theme"</span>
                <input type="checkbox" prop:checked=reset on:change=move |ev| {
                    let checked = event_target_checked(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::Settings(form)) = &mut s.modal {
                            form.reset_theme = checked;
                        }
                    });
                } />
            </label>
            <div class="hint">"Values accept #rgb, #rrggbb, or a CSS color name. Invalid values keep the default."</div>
            {AnyView::from(Fragment::new(rows))}
        </div>
    }
    .into_any()
}

fn context_body(store: StoreHandle, form: &SettingsFormState) -> AnyView {
    let inputs = form.inputs.clone();
    let rows = inputs
        .into_iter()
        .map(|(key, value)| {
            let value = value.text.clone();
            view! {
                <div class="settings-row">
                    <span>{input_label(key)}</span>
                    <input
                        class="input"
                        prop:value=value.clone()
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
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    let toggles = [
        ("reasoning_pruning", form.reasoning_pruning),
        ("prompt_caching", form.prompt_caching),
        ("rollback_on_verify_failure", form.rollback_on_verify_failure),
        ("post_edit_diagnostics", form.post_edit_diagnostics),
    ]
    .into_iter()
    .map(|(key, checked)| {
        view! {
            <label class="settings-row">
                <span>{toggle_label(key)}</span>
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
    })
    .collect::<Vec<_>>();
    view! {
        <div class="settings-section">
            <h3>"Context"</h3>
            {AnyView::from(Fragment::new(toggles))}
            <div class="settings-list">{AnyView::from(Fragment::new(rows))}</div>
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

fn input_label(key: &str) -> &'static str {
    match key {
        "context_window" => "Context window",
        "high_watermark_pct" => "High watermark %",
        "spill_max_inline" => "Spill threshold",
        "verbatim_window_turns" => "Verbatim turns",
        "max_steps_per_turn" => "Max steps per turn",
        "tool_timeout_ms" => "Tool timeout (ms)",
        "max_session_tokens" => "Session token cap",
        "max_turn_secs" => "Turn time cap (s)",
        "recovery_model" => "Recovery model",
        "summarizer_model" => "Summarizer model",
        "temperature" => "Temperature",
        "auto_prune_days" => "Auto prune days",
        "session_keep" => "Keep newest",
        _ => "Value",
    }
}

fn toggle_label(key: &str) -> &'static str {
    match key {
        "reasoning_pruning" => "Prune reasoning",
        "prompt_caching" => "Prompt caching",
        "rollback_on_verify_failure" => "Rollback on verify failure",
        "post_edit_diagnostics" => "Post-edit diagnostics",
        "require_for_shell" => "Require sandbox for shell",
        "require_for_scripts" => "Require sandbox for scripts",
        "strict_env" => "Strict environment",
        _ => "Option",
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

fn sessions_body(store: StoreHandle, form: &SettingsFormState) -> AnyView {
    let days = form.field_text("auto_prune_days").to_owned();
    let keep = form.field_text("session_keep").to_owned();
    view! {
        <div class="settings-section">
            <h3>"Session retention"</h3>
            <div class="settings-row"><span>"Auto prune days"</span><input class="input" prop:value=days.clone() on:input=move |ev| {
                let next = event_target_value(&ev);
                store.update(|s| if let Some(ActiveModal::Settings(form)) = &mut s.modal && let Some(field) = form.inputs.get_mut("auto_prune_days") { field.replace_text(next); });
            } /></div>
            <div class="settings-row"><span>"Keep newest"</span><input class="input" prop:value=keep.clone() on:input=move |ev| {
                let next = event_target_value(&ev);
                store.update(|s| if let Some(ActiveModal::Settings(form)) = &mut s.modal && let Some(field) = form.inputs.get_mut("session_keep") { field.replace_text(next); });
            } /></div>
            <div class="hint">"Only the user settings file controls retention. Session files are never pruned by the browser."</div>
        </div>
    }
    .into_any()
}

fn sandbox_body(store: StoreHandle, form: &SettingsFormState) -> AnyView {
    let toggles = [
        ("require_for_shell", form.require_for_shell),
        ("require_for_scripts", form.require_for_scripts),
        ("strict_env", form.strict_env),
    ]
    .into_iter()
    .map(|(key, checked)| {
        view! {
            <label class="settings-row">
                <span>{toggle_label(key)}</span>
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
    let summary = store.with(|s| s.host_info.settings.permissions_summary.clone());
    let rows = summary
        .into_iter()
        .map(|row| view! { <div class="settings-row"><code>{row}</code></div> }.into_any())
        .collect::<Vec<_>>();
    view! {
        <div class="settings-section">
            <h3>"Permissions"</h3>
            <div class="hint">"Project rules can tighten permissions only. Edit settings files on the host."</div>
            <div class="settings-list">{AnyView::from(Fragment::new(rows))}</div>
        </div>
    }
    .into_any()
}
