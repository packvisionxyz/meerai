pub mod modals;
mod sessions;
pub(crate) use sessions::close as close_sessions;
pub mod settings;
pub mod transcript;

use std::sync::{Arc, Mutex};

use leptos::ev;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{
    ApprovalModeDto, BackendKind, ContextBreakdownDto, FileChangeDto, McpServerInfoDto,
    SessionStatus, TurnMetricsDto, UsageDto,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::keymap::open_command_palette;
use crate::state::{
    ActiveModal, CompletionKind, ConnectionState, MentionCategory, SlashCommandSpec,
    accept_completion, activate_and_switch_session, detach_tab_view, format_duration_ms,
    format_ratio, format_tokens, move_tab_to, open_diff_modal, open_diff_modal_with_status,
    open_model_input, open_provider_select, open_search_modal, open_settings, remove_mention_token,
    reopen_pending_request, resolved_mention_chips, set_approval_mode, set_status_notice,
    submit_or_queue_composer, toggle_diagnose, update_completion,
};
use crate::store::StoreHandle;
use crate::theme::{BrowserTheme, UiTheme};

#[component]
pub fn BrandMark() -> impl IntoView {
    view! {
        <svg class="brand-symbol" viewBox="0 0 10 8" width="24" height="20" aria-hidden="true">
            <path fill="currentColor" d="M0 0h2v8H0z M8 0h2v8H8z M2 2h2v2H2z M6 2h2v2H6z M4 4h2v2H4z" />
        </svg>
    }
}

#[component]
pub fn App(store: StoreHandle) -> impl IntoView {
    sessions::setup(store);
    let listener = window_event_listener(ev::keydown, move |ev| {
        crate::keymap::handle_global_key(store, &ev);
    });
    on_cleanup(move || listener.remove());

    let notice_timer: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let notice_timer_cleanup = notice_timer.clone();
    let notice_key = Memo::new(move |_| store.with(|s| (s.notice_seq, s.status_notice.is_some())));
    Effect::new(move |_| {
        let (seq, has_notice) = notice_key.get();
        if let Some(window) = web_sys::window() {
            if let Ok(mut guard) = notice_timer.lock()
                && let Some(prev) = guard.take()
            {
                window.clear_timeout_with_handle(prev);
            }
            if has_notice {
                let cb = Closure::once_into_js(move || {
                    store.update(|s| {
                        if s.notice_seq == seq {
                            s.status_notice = None;
                        }
                    });
                });
                if let Some(func) = cb.dyn_ref::<js_sys::Function>()
                    && let Ok(id) =
                        window.set_timeout_with_callback_and_timeout_and_arguments_0(func, 4200)
                    && let Ok(mut guard) = notice_timer.lock()
                {
                    *guard = Some(id);
                }
            }
        }
    });
    on_cleanup(move || {
        if let Ok(mut guard) = notice_timer_cleanup.lock()
            && let Some(id) = guard.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(id);
        }
    });

    let theme = RwSignal::new(BrowserTheme::load());
    provide_context(theme);
    let host_scheme =
        Memo::new(move |_| store.with(|s| s.host_info.settings.raw_theme.color_scheme.clone()));
    let prev_host_scheme = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let next = host_scheme.get();
        let prev = prev_host_scheme.get_untracked();
        if next != prev {
            prev_host_scheme.set(next.clone());
            if let Some(key) = next {
                let resolved = BrowserTheme::from_key(&key);
                theme.set(resolved);
                resolved.save();
            } else if prev.is_some() {
                let resolved = BrowserTheme::Host;
                theme.set(resolved);
                resolved.save();
            }
        }
    });
    let vars = Memo::new(move |_| {
        (theme.get() == BrowserTheme::Host)
            .then(|| store.with(|s| UiTheme::resolve(&s.host_info.settings.theme).vars))
    });
    let sidebar_open =
        Memo::new(move |_| store.with(|s| s.sidebar_open && !s.session_panel.narrow_open()));

    view! {
        <div class="app" data-theme=move || theme.get().key() style=move || vars.get()>
            <Header store=store />
            <div class="workspace">
                <sessions::SessionPanel store=store />
                <main class="workspace-main" aria-label="Conversation" inert=move || sessions::background_inert(store)>
                    <transcript::Transcript store=store />
                    <Composer store=store />
                </main>
                <Show when=move || sidebar_open.get()>
                    <div
                        class="sidebar-scrim"
                        on:click=move |_| store.update(|s| s.sidebar_open = false)
                    ></div>
                </Show>
                <Sidebar store=store />
            </div>
            <modals::ModalLayer store=store />
        </div>
    }
}

#[component]
fn Header(store: StoreHandle) -> impl IntoView {
    let header_info = Memo::new(move |_| {
        store.with(|s| {
            let session = s.active_session().cloned();
            let pending_count = s
                .active_tab()
                .map(|t| t.pending_approvals.len() + t.pending_questions.len())
                .unwrap_or(0);
            let modal_open = s.modal.is_some();
            (
                session,
                s.connection,
                s.file_changes.len(),
                s.host_info.update_available.clone(),
                s.host_info.install_hint.clone(),
                pending_count,
                modal_open,
                s.error_banner.clone(),
                s.status_notice.clone(),
                s.resume_command_hint(),
            )
        })
    });

    view! {
        <header class="header" inert=move || sessions::background_inert(store)>
            <div class="brand">
                <BrandMark />
                <span class="brand-mark">"meerai"</span>
            </div>
            {move || {
                let (
                    session,
                    connection,
                    change_count,
                    update_available,
                    install_hint,
                    pending_count,
                    modal_open,
                    banner,
                    notice,
                    resume_hint,
                ) = header_info.get();

                let title = session
                    .as_ref()
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "New session".to_owned());
                let status = session
                    .as_ref()
                    .map(|s| s.status)
                    .unwrap_or(SessionStatus::Idle);
                let mode = session
                    .as_ref()
                    .map(|s| s.approval_mode)
                    .unwrap_or_default();
                let is_acp = session
                    .as_ref()
                    .is_some_and(|s| matches!(s.backend, BackendKind::Acp { .. }));
                let acp_preset = session.as_ref().and_then(|s| match &s.backend {
                    BackendKind::Acp { preset, .. } => Some(preset.clone()),
                    BackendKind::Native => None,
                });
                let model_label = session
                    .as_ref()
                    .map(|s| format!("{}/{}", s.provider, s.model))
                    .unwrap_or_else(|| "no model".to_owned());
                let disconnected = connection == ConnectionState::Disconnected;
                let resume_cmd = resume_hint.unwrap_or_else(|| "meerai --resume <id>".to_owned());

                view! {
                    <>
                        <div class="header-main">
                            <div class="header-title-row">
                                <h1 class="session-title">{title}</h1>
                                {acp_preset.map(|preset| view! {
                                    <span
                                        class="badge acp-badge"
                                        title="External ACP backend keeps its own tools and policy."
                                    >
                                        {format!("ACP:{preset}")}
                                    </span>
                                }.into_any())}
                                {update_available.map(|ver| {
                                    let hint = install_hint.clone();
                                    view! {
                                        <button
                                            class="badge update-badge"
                                            title=format!("Update {ver} available. Click to copy install command.")
                                            on:click=move |_| {
                                                crate::store::copy_text(hint.clone());
                                                store.update(|s| set_status_notice(s, format!("Copied update command for v{ver}")));
                                            }
                                        >
                                            {format!("Update v{ver}")}
                                        </button>
                                    }
                                    .into_any()
                                })}
                            </div>
                            <div class="header-meta">
                                <span class=format!("conn {}", connection_class(connection))>
                                    {connection_label(connection)}
                                </span>
                                <span class=format!("state {}", status_class(status))>
                                    {status_label(status)}
                                </span>
                                <button
                                    class="pill-btn"
                                    disabled=is_acp
                                    title=if is_acp {
                                        "ACP sessions manage their own model"
                                    } else {
                                        "Switch model (Alt+L)"
                                    }
                                    on:click=move |_| store.update(open_model_input)
                                >
                                    {model_label}
                                </button>
                                <div class="mode-switch" role="group" aria-label="Approval mode">
                                    {[ApprovalModeDto::Plan, ApprovalModeDto::Ask, ApprovalModeDto::Auto]
                                        .into_iter()
                                        .map(|candidate| {
                                            let active = candidate == mode;
                                            let disabled = is_acp && candidate == ApprovalModeDto::Plan;
                                            let plan_cls = candidate == ApprovalModeDto::Plan && active;
                                            view! {
                                                <button
                                                    class="mode-seg"
                                                    class:active=active
                                                    class:plan=plan_cls
                                                    disabled=disabled
                                                    title=if disabled {
                                                        "Plan mode is unavailable on ACP sessions"
                                                    } else {
                                                        "Set approval mode (Alt+M)"
                                                    }
                                                    on:click=move |_| store.update(|s| set_approval_mode(s, candidate))
                                                >
                                                    {mode_label(candidate)}
                                                </button>
                                            }
                                            .into_any()
                                        })
                                        .collect::<Vec<_>>()}
                                </div>
                                <Show when=move || { pending_count > 0 && !modal_open }>
                                    <button
                                        class="pill-btn warn"
                                        title="Reopen pending approval or question"
                                        on:click=move |_| store.update(reopen_pending_request)
                                    >
                                        {format!("{pending_count} pending")}
                                    </button>
                                </Show>
                            </div>
                        </div>
                        <div class="header-actions">
                            <button
                                id="sessions-toggle"
                                class="btn subtle"
                                class:active=move || store.with(|s| s.session_panel.open)
                                aria-expanded=move || store.with(|s| s.session_panel.open).to_string()
                                title="Sessions (Alt+S)"
                                on:click=move |_| sessions::toggle(store)
                            >"Sessions"</button>
                            <button class="btn subtle" title="Workspace diff (Alt+Shift+D)" on:click=move |_| store.update(|s| open_diff_modal(s, None))>
                                "Diff"
                                <Show when=move || { change_count > 0 }>
                                    <span class="action-count">{change_count}</span>
                                </Show>
                            </button>
                            <button class="btn subtle" title="Workspace search (Alt+Shift+F)" on:click=move |_| store.update(|s| open_search_modal(s, meerai_web_protocol::SearchModeDto::Content, ""))>"Search"</button>
                            <button class="btn subtle" title="Command palette (Alt+K)" on:click=move |_| open_command_palette(store)>"Palette"</button>
                            <button class="btn subtle" title="Providers" on:click=move |_| store.update(open_provider_select)>"Connect"</button>
                            <button class="btn subtle" title="Settings" on:click=move |_| store.update(|s| open_settings(s, None))>"Settings"</button>
                            <span class="header-sep" aria-hidden="true"></span>
                            <button class="btn subtle icon-btn" title="Help (?)" aria-label="Help" on:click=move |_| store.update(|s| s.modal = Some(ActiveModal::Help))>"?"</button>
                            <button
                                class="btn subtle icon-btn"
                                class:active=move || store.with(|s| s.sidebar_open)
                                aria-expanded=move || store.with(|s| s.sidebar_open).to_string()
                                title="Toggle inspector (Ctrl+B)"
                                aria-label="Toggle inspector"
                                on:click=move |_| store.update(|s| s.sidebar_open = !s.sidebar_open)
                            >
                                <svg class="icon-panel" viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true">
                                    <rect x="1.75" y="2.25" width="12.5" height="11.5" rx="2" />
                                    <path d="M10.25 2.5v11" />
                                </svg>
                            </button>
                        </div>
                        <TabStrip store=store />
                        <StatusLine store=store />
                        {banner.map(|err| view! {
                            <div class="banner error banner-row" role="alert">
                                <span>{err}</span>
                                <button
                                    class="banner-close"
                                    title="Dismiss"
                                    on:click=move |_| store.update(|s| s.error_banner = None)
                                >"×"</button>
                            </div>
                        }.into_any())}
                        {notice.map(|msg| view! {
                            <div class="banner notice banner-row">
                                <span>{msg}</span>
                                <button
                                    class="banner-close"
                                    title="Dismiss"
                                    on:click=move |_| store.update(|s| s.status_notice = None)
                                >"×"</button>
                            </div>
                        }.into_any())}
                        <Show when=move || disconnected>
                            <div class="banner error banner-row" role="alert">
                                <span>
                                    "Host disconnected. The session log is safe. Restart with "
                                    <code>{resume_cmd.clone()}</code>
                                    " to continue."
                                </span>
                                <button
                                    class="btn tiny"
                                    on:click=move |_| {
                                        if let Some(t) = crate::transport::current() {
                                            t.resync();
                                        }
                                    }
                                >"Reconnect"</button>
                            </div>
                        </Show>
                    </>
                }
            }}
        </header>
    }
}

#[component]
fn TabStrip(store: StoreHandle) -> impl IntoView {
    let drag_source = RwSignal::new(None::<String>);
    let tabs_memo = Memo::new(move |_| {
        store.with(|s| {
            let active = s.active_session_id.clone();
            s.visible_sessions()
                .into_iter()
                .map(|session| {
                    let is_acp = matches!(session.backend, BackendKind::Acp { .. });
                    (
                        session.id.clone(),
                        session
                            .title
                            .clone()
                            .unwrap_or_else(|| "New session".to_owned()),
                        status_class(session.status),
                        s.session_tabs
                            .get(&session.id)
                            .is_some_and(|tab| tab.unread),
                        active.as_deref() == Some(session.id.as_str()),
                        is_acp,
                    )
                })
                .collect::<Vec<_>>()
        })
    });

    view! {
        <div class="tabs" role="tablist" aria-label="Sessions">
            {move || {
                let tabs = tabs_memo.get();
                AnyView::from(Fragment::new(
                    tabs.into_iter()
                        .map(|(id, title, status, unread, active, is_acp)| {
                            let click_id = id.clone();
                            let close_id = id.clone();
                            let drag_id = id.clone();
                            let drop_id = id.clone();
                            view! {
                                <div
                                    class="tab"
                                    class:active=active
                                    draggable="true"
                                    on:dragstart=move |ev| {
                                        drag_source.set(Some(drag_id.clone()));
                                        if let Some(dt) = ev.data_transfer() {
                                            let _ = dt.set_data("text/plain", &drag_id);
                                        }
                                    }
                                    on:dragover=move |ev| {
                                        ev.prevent_default();
                                    }
                                    on:drop=move |ev| {
                                        ev.prevent_default();
                                        let from = drag_source
                                            .try_update(|slot| slot.take())
                                            .flatten()
                                            .or_else(|| {
                                                ev.data_transfer()
                                                    .and_then(|dt| dt.get_data("text/plain").ok())
                                                    .filter(|s| !s.is_empty())
                                            });
                                        if let Some(from) = from {
                                            store.update(|s| move_tab_to(s, &from, &drop_id));
                                        }
                                    }
                                >
                                    <button
                                        class="tab-main"
                                        role="tab"
                                        aria-selected=active
                                        on:click=move |_| store.update(|s| activate_and_switch_session(s, &click_id))
                                    >
                                        <span class=format!("tab-dot {status}")></span>
                                        {is_acp.then(|| view! { <span class="tab-acp">"ACP"</span> }.into_any())}
                                        <span class="tab-title">{title}</span>
                                        <Show when=move || unread>
                                            <span class="tab-unread" title="Unread output"></span>
                                        </Show>
                                    </button>
                                    <button
                                        class="tab-close"
                                        title="Close view (Alt+W)"
                                        on:click=move |_| store.update(|s| detach_tab_view(s, &close_id))
                                    >
                                        "×"
                                    </button>
                                </div>
                            }
                            .into_any()
                        })
                        .collect(),
                ))
            }}
            <button
                class="tab-add"
                title="New session (/new)"
                aria-label="New session"
                on:click=move |_| {
                    store.update(|s| s.modal = None);
                    if let Some(t) = crate::transport::current() {
                        t.create_session(None);
                    }
                }
            >"+"</button>
        </div>
    }
}

#[component]
fn StatusLine(store: StoreHandle) -> impl IntoView {
    let now_ms = RwSignal::new(js_sys::Date::now() as u64);
    let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let interval_cleanup = interval_handle.clone();

    if let Some(window) = web_sys::window() {
        let cb = Closure::<dyn FnMut()>::new(move || {
            let running = store.with(|s| {
                s.active_session().is_some_and(|x| {
                    x.turn_started_at_ms.is_some() && x.status != SessionStatus::Idle
                })
            });
            if running {
                now_ms.set(js_sys::Date::now() as u64);
            }
        });
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

    let line_memo = Memo::new(move |_| {
        store.with(|s| {
            let root = if s.host_info.workspace_root.is_empty() {
                "workspace".to_owned()
            } else {
                s.host_info.workspace_root.clone()
            };
            let short_root = root
                .rsplit(['/', '\\'])
                .find(|part| !part.is_empty())
                .unwrap_or(&root)
                .to_owned();
            let branch = s.host_info.git_branch.clone();
            let session = s.active_session().cloned();
            let tab = s.active_tab();
            let (usage, metrics, queue_len) = match tab {
                Some(t) => (t.usage.clone(), t.metrics.clone(), t.pending_queue.len()),
                None => (Default::default(), Default::default(), 0),
            };
            let context = &s.host_info.settings.context;
            let effort = context
                .effort
                .unwrap_or(meerai_web_protocol::EffortDto::Medium);
            let temp = context
                .temperature
                .filter(|t| (0.0..=2.0).contains(t) && t.is_finite())
                .unwrap_or_else(|| crate::state::effort_default_temperature(effort));
            (
                root, short_root, branch, session, usage, metrics, queue_len, effort, temp,
            )
        })
    });

    view! {
        {move || {
            let (full_root, short_root, branch, session, usage, metrics, queue_len, effort, temp) =
                line_memo.get();
            let mut parts = vec![short_root];
            if let Some(branch) = branch {
                parts.push(branch);
            }

            parts.push(format!(
                "temp {temp:.2} · {}",
                crate::state::effort_label(effort)
            ));

            if let Some(cost) = usage.est_cost_usd
                && cost > 0.0
            {
                parts.push(format!("${cost:.4}"));
            }

            if let Some(tps) = metrics.tokens_per_sec
                && tps > 0.0
            {
                parts.push(format!("{tps:.1} tok/s"));
            }

            if let Some(sess) = &session {
                if sess.status != SessionStatus::Idle
                    && let Some(started) = sess.turn_started_at_ms
                {
                    let elapsed = now_ms.get().saturating_sub(started);
                    parts.push(format!("⏱ {}", format_duration_ms(elapsed)));
                } else if let Some(last_ms) = metrics.last_turn_ms {
                    parts.push(format!("last {}", format_duration_ms(last_ms)));
                }
                if let Some(tool) = &sess.active_tool {
                    parts.push(format!("{} {tool}", crate::state::activity_verb(tool)));
                }
            }

            if queue_len > 0 {
                parts.push(format!("{queue_len} queued"));
            }

            view! {
                <div class="statusline" title=full_root>{parts.join(" · ")}</div>
            }
        }}
    }
}

#[component]
fn Composer(store: StoreHandle) -> impl IntoView {
    Effect::new(move |_| {
        let text = store.with(|s| s.draft.text.clone());
        if let Some(window) = web_sys::window()
            && let Some(document) = window.document()
            && let Some(element) = document.get_element_by_id("composer")
            && let Ok(textarea) =
                wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlTextAreaElement>(element)
            && textarea.value() != text
        {
            textarea.set_value(&text);
        }
    });

    let composer_state = Memo::new(move |_| {
        store.with(|s| {
            let has_session = s.active_session_id.is_some();
            let queue = s
                .active_tab()
                .map(|tab| tab.pending_queue.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let pending_label = s.active_tab().and_then(|tab| {
                if let Some(req) = tab.pending_approvals.first() {
                    Some(format!("Approval needed: {}", req.title))
                } else if !tab.pending_questions.is_empty() {
                    Some("Question waiting for your answer".to_owned())
                } else {
                    None
                }
            });
            let chips = resolved_mention_chips(&s.draft.text, &s.host_info);
            let session_status = s.active_session().map(|x| x.status);
            let active_tool = s.active_session().and_then(|x| x.active_tool.clone());
            (
                has_session,
                queue,
                chips,
                s.completion.active,
                session_status,
                active_tool,
                pending_label,
                s.modal.is_some(),
            )
        })
    });

    let can_send = Memo::new(move |_| store.with(|s| s.can_submit_composer()));
    let busy = Memo::new(move |_| store.with(|s| s.is_busy()));

    view! {
        <div class="composer">
            <div class="composer-shell conversation-column">
                <Show when=move || !composer_state.get().1.is_empty()>
                    <div class="queue-strip">
                        {move || {
                            let items = composer_state.get().1;
                            AnyView::from(Fragment::new(
                                items
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, prompt)| {
                                        let steer_index = index;
                                        let drop_index = index;
                                        view! {
                                            <div class="queue-pill">
                                                <button
                                                    class="queue-item"
                                                    title="Steer this prompt now (Alt+G)"
                                                    on:click=move |_| store.update(|s| crate::state::steer_or_send_queued(s, steer_index))
                                                >
                                                    <span class="queue-steer">"Steer"</span>
                                                    <span>{truncate(&prompt, 72)}</span>
                                                </button>
                                                <button
                                                    class="queue-drop"
                                                    title="Remove from queue"
                                                    on:click=move |_| store.update(|s| {
                                                        if let Some(tab) = s.active_tab_mut() {
                                                            tab.pending_queue.remove(drop_index);
                                                        }
                                                    })
                                                >"×"</button>
                                            </div>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                    </div>
                </Show>
                {move || {
                    let (_, _, _, _, _, _, pending_label, modal_open) = composer_state.get();
                    if !modal_open && let Some(label) = pending_label {
                        view! {
                            <div class="pending-request-bar">
                                <span class="pending-request-text">{label}</span>
                                <button
                                    type="button"
                                    class="btn tiny warn-btn"
                                    on:click=move |_| store.update(reopen_pending_request)
                                >"Review"</button>
                            </div>
                        }
                        .into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }
                }}
                <Show when=move || composer_state.get().3>
                    <CompletionPopup store=store />
                </Show>
                <form
                    class="composer-card"
                    class:busy=move || busy.get()
                    aria-label="Message composer"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        store.update(submit_or_queue_composer);
                    }
                >
                    <Show when=move || !composer_state.get().2.is_empty()>
                        <div class="chips-strip">
                            {move || {
                                let chips = composer_state.get().2;
                                AnyView::from(Fragment::new(
                                    chips
                                        .into_iter()
                                        .map(|(cat, id, raw_token)| {
                                            let cls = match cat {
                                                MentionCategory::Agent => "def-chip agent",
                                                MentionCategory::Skill => "def-chip skill",
                                                MentionCategory::File => "def-chip",
                                            };
                                            let prefix = match cat {
                                                MentionCategory::Agent => "agent",
                                                MentionCategory::Skill => "skill",
                                                MentionCategory::File => "file",
                                            };
                                            view! {
                                                <span class=cls>
                                                    <span class="def-chip-kind">{prefix}</span>
                                                    <span>{id}</span>
                                                    <button
                                                        type="button"
                                                        class="def-chip-remove"
                                                        title="Remove mention"
                                                        on:click=move |_| store.update(|s| {
                                                            let next = remove_mention_token(&s.draft.text, &raw_token);
                                                            s.draft.replace_text(next.clone());
                                                            if let Some(tab) = s.active_tab_mut() {
                                                                tab.draft_text = next;
                                                            }
                                                            update_completion(s);
                                                        })
                                                    >"×"</button>
                                                </span>
                                            }
                                            .into_any()
                                        })
                                        .collect(),
                                ))
                            }}
                        </div>
                    </Show>
                    <div class="composer-input-row">
                        <textarea
                            id="composer"
                            class="composer-input"
                            rows="2"
                            aria-label="Message meerai"
                            placeholder=move || if !composer_state.get().0 {
                                "Open a session or use + to start one."
                            } else if busy.get() {
                                "Type a follow-up to queue, or press Stop…"
                            } else {
                                "Message meerai…"
                            }
                            on:input=move |ev| {
                                let value = event_target_value(&ev);
                                store.update(|s| {
                                    s.draft.replace_text(value);
                                    update_completion(s);
                                });
                            }
                            on:keydown=move |ev| {
                                crate::keymap::handle_composer_key(store, &ev);
                            }
                        ></textarea>
                        <div class="composer-action-slot">
                            {move || {
                                let is_busy = busy.get();
                                let ready = can_send.get();
                                let waiting = matches!(
                                    composer_state.get().4,
                                    Some(SessionStatus::WaitingApproval | SessionStatus::WaitingQuestion)
                                );
                                if is_busy && !ready {
                                    view! {
                                        <button
                                            type="button"
                                            class="composer-action stop"
                                            class:waiting=waiting
                                            title="Stop running turn (Alt+X)"
                                            aria-label="Stop running turn"
                                            on:click=move |_| {
                                                if let (Some(id), Some(t)) = (
                                                    store.with(|s| s.active_session_id.clone()),
                                                    crate::transport::current(),
                                                ) {
                                                    t.cancel_turn(id);
                                                }
                                            }
                                        >
                                            <span class="stop-ring" aria-hidden="true"></span>
                                            <span class="stop-glyph" aria-hidden="true"></span>
                                        </button>
                                    }
                                    .into_any()
                                } else {
                                    view! {
                                        <button
                                            type="submit"
                                            class="composer-action send"
                                            class:queue=is_busy
                                            disabled=!ready
                                            title=if is_busy { "Queue prompt (Enter)" } else { "Send message (Enter)" }
                                            aria-label=if is_busy { "Queue prompt" } else { "Send message" }
                                        >
                                            {is_busy.then(|| view! { <span class="action-label">"Queue"</span> }.into_any())}
                                            <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                                <path d="M8 12.5V3.5" />
                                                <path d="M4 7.5 8 3.5l4 4" />
                                            </svg>
                                        </button>
                                    }
                                    .into_any()
                                }
                            }}
                        </div>
                    </div>
                </form>
                <div class="composer-footer">
                    {move || {
                        let (_, _, _, _, session_status, active_tool, _, _) = composer_state.get();
                        if busy.get() {
                            let waiting = matches!(
                                session_status,
                                Some(SessionStatus::WaitingApproval | SessionStatus::WaitingQuestion)
                            );
                            let label = if let Some(tool) = active_tool {
                                format!("{} {tool}…", crate::state::activity_verb(&tool))
                            } else {
                                match session_status {
                                    Some(SessionStatus::WaitingApproval) => "Waiting for approval…".to_owned(),
                                    Some(SessionStatus::WaitingQuestion) => "Waiting for answer…".to_owned(),
                                    _ => "Working…".to_owned(),
                                }
                            };
                            view! {
                                <div class="working-status" role="status" aria-live="polite">
                                    <span class="working-dots" class:waiting=waiting aria-hidden="true">
                                        <span></span><span></span><span></span>
                                    </span>
                                    <span>{label}</span>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! {
                                <div class="composer-hint">
                                    <kbd>"/"</kbd>" commands"
                                    <span class="hint-sep">"·"</span>
                                    <kbd>"@"</kbd>" mentions"
                                    <span class="hint-sep">"·"</span>
                                    <kbd>"!"</kbd>" shell"
                                    <span class="hint-sep">"·"</span>
                                    <kbd>"Shift+Enter"</kbd>" newline"
                                </div>
                            }
                            .into_any()
                        }
                    }}
                    <div class="composer-footer-right">
                        {move || {
                            if busy.get() && can_send.get() {
                                view! {
                                    <button
                                        type="button"
                                        class="composer-stop-link"
                                        title="Stop running turn (Alt+X)"
                                        on:click=move |_| {
                                            if let (Some(id), Some(t)) = (
                                                store.with(|s| s.active_session_id.clone()),
                                                crate::transport::current(),
                                            ) {
                                                t.cancel_turn(id);
                                            }
                                        }
                                    >
                                        "Stop turn"
                                    </button>
                                }
                                .into_any()
                            } else if !busy.get() {
                                view! {
                                    <span class="composer-send-hint"><kbd>"Enter"</kbd>" to send"</span>
                                }
                                .into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn CompletionPopup(store: StoreHandle) -> impl IntoView {
    let items = Memo::new(move |_| {
        store.with(|s| {
            (
                s.completion.kind,
                s.completion.items.clone(),
                s.completion.selected,
            )
        })
    });
    view! {
        <div class="completion" role="listbox">
            {move || {
                let (kind, items, selected) = items.get();
                if items.is_empty() {
                    return view! { <div class="completion-empty">"No matches"</div> }.into_any();
                }
                let header = match kind {
                    Some(CompletionKind::Command) => "Commands",
                    Some(CompletionKind::Model) => "Models",
                    Some(CompletionKind::Mention) => "Mentions",
                    None => "Suggestions",
                };
                let rows = items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let insert_index = index;
                        let cat_badge = item.category.map(|c| match c {
                            MentionCategory::Agent => ("agent", "cat-badge agent"),
                            MentionCategory::Skill => ("skill", "cat-badge skill"),
                            MentionCategory::File => ("file", "cat-badge"),
                        });
                        view! {
                            <button
                                class="completion-row"
                                class:selected=index == selected
                                role="option"
                                aria-selected=index == selected
                                on:click=move |_| store.update(|s| { accept_completion(s, insert_index); })
                            >
                                {cat_badge.map(|(label, cls)| view! { <span class=cls>{label}</span> }.into_any())}
                                <span class="completion-label">{item.label}</span>
                                <span class="completion-detail">{item.detail}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div class="completion-head">{header}</div>
                    {AnyView::from(Fragment::new(rows))}
                }
                .into_any()
            }}
        </div>
    }
}

#[derive(Clone, PartialEq)]
struct SidebarSnapshot {
    open: bool,
    workspace_root: String,
    git_branch: Option<String>,
    agents_count: usize,
    skills_count: usize,
    hooks_count: usize,
    mcp_servers: Vec<McpServerInfoDto>,
    enabled_tools: Vec<String>,
    tools_expanded: bool,
    changes: Vec<FileChangeDto>,
    backend: Option<BackendKind>,
    metrics: TurnMetricsDto,
    usage: UsageDto,
    context: Option<ContextBreakdownDto>,
    diagnose_open: bool,
    subagents: Vec<(String, String, String, Option<String>)>,
}

#[component]
fn Sidebar(store: StoreHandle) -> impl IntoView {
    let sidebar_memo = Memo::new(move |_| {
        store.with(|s| {
            let metrics = s
                .active_tab()
                .map(|t| t.metrics.clone())
                .unwrap_or_default();
            let usage = s.active_tab().map(|t| t.usage.clone()).unwrap_or_default();
            let context = s.active_tab().and_then(|t| t.context.clone());
            let subagents = s
                .active_tab()
                .map(|tab| {
                    tab.entries
                        .iter()
                        .filter_map(|entry| match entry {
                            meerai_web_protocol::TranscriptEntryDto::Subagent {
                                id,
                                agent,
                                status,
                                child_session_id,
                                ..
                            } => Some((
                                id.clone(),
                                agent.clone(),
                                status.clone(),
                                child_session_id.clone(),
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            SidebarSnapshot {
                open: s.sidebar_open,
                workspace_root: s.host_info.workspace_root.clone(),
                git_branch: s.host_info.git_branch.clone(),
                agents_count: s.host_info.agents.len(),
                skills_count: s.host_info.skills.len(),
                hooks_count: s.host_info.hooks_count,
                mcp_servers: s.host_info.mcp_servers.clone(),
                enabled_tools: s.host_info.enabled_tools.clone(),
                tools_expanded: s.sidebar_tools_expanded,
                changes: s.file_changes.clone(),
                backend: s.active_session().map(|x| x.backend.clone()),
                metrics,
                usage,
                context,
                diagnose_open: s.sidebar_diagnose_open,
                subagents,
            }
        })
    });

    view! {
        {move || {
            let SidebarSnapshot {
                open,
                workspace_root,
                git_branch,
                agents_count,
                skills_count,
                hooks_count,
                mcp_servers,
                enabled_tools,
                tools_expanded,
                changes,
                backend,
                metrics,
                usage,
                context,
                diagnose_open,
                subagents,
            } = sidebar_memo.get();
            if !open {
                return view! { <span></span> }.into_any();
            }
            let branch_label = git_branch.unwrap_or_else(|| "no branch".to_owned());
            let backend_label = match backend {
                Some(BackendKind::Acp { preset, .. }) => format!("ACP ({preset})"),
                Some(BackendKind::Native) => "Native".to_owned(),
                None => "—".to_owned(),
            };
            let tool_count = enabled_tools.len();
            let diagnose_rows: Vec<(String, String)> = {
                let prompt = usage.prompt_tokens.unwrap_or(0).max(0) as u64;
                let hit = usage
                    .cached_tokens
                    .map_or_else(|| "n/a".to_owned(), |c| format_ratio(c.max(0) as u64, prompt));
                let write = usage
                    .cache_creation_tokens
                    .map_or_else(|| "n/a".to_owned(), |c| format_tokens(c.max(0) as u64));
                let mut rows = vec![
                    ("Cache hit (last)".to_owned(), hit),
                    ("Cache write".to_owned(), write),
                ];
                if let Some(c) = context.as_ref() {
                    rows.push((
                        "Context used".to_owned(),
                        format_ratio(c.estimated_total_tokens, c.context_window_tokens),
                    ));
                    rows.push((
                        "Memory".to_owned(),
                        format!("{} tokens", format_tokens(c.memory_tokens)),
                    ));
                    rows.push((
                        "Tool schemas".to_owned(),
                        format!("{} tokens", format_tokens(c.tool_schemas_tokens)),
                    ));
                }
                rows
            };

            view! {
                <aside class="sidebar open" aria-label="Workspace inspector" inert=move || sessions::background_inert(store)>
                    <div class="sidebar-heading">
                        <h2>"Inspector"</h2>
                        <button class="modal-close" aria-label="Close inspector" on:click=move |_| store.update(|s| s.sidebar_open = false)>"×"</button>
                    </div>
                    <section class="side-section">
                        <div class="side-head">
                            <h2>"Workspace"</h2>
                            <span class="badge">{backend_label}</span>
                        </div>
                        <div class="side-path">{workspace_root}</div>
                        <div class="side-meta">{format!("{branch_label} · {agents_count} agents · {skills_count} skills · {hooks_count} hooks")}</div>
                    </section>
                    <section class="side-section">
                        <h2>"Session metrics"</h2>
                        <div class="side-stats-grid">
                            <div class="side-stat">
                                <span class="side-stat-label">"Turns"</span>
                                <strong>{metrics.turn_count}</strong>
                            </div>
                            <div class="side-stat">
                                <span class="side-stat-label">"Tool calls"</span>
                                <strong>{metrics.tool_calls}</strong>
                            </div>
                            <div class="side-stat">
                                <span class="side-stat-label">"Session Σ"</span>
                                <strong>{format_tokens(metrics.session_total_tokens)}</strong>
                            </div>
                            <div class="side-stat">
                                <span class="side-stat-label">"Cached Σ"</span>
                                <strong>{format_tokens(metrics.session_cached_tokens)}</strong>
                            </div>
                        </div>
                        {usage.est_cost_usd.filter(|c| *c > 0.0).map(|cost| view! {
                            <div class="side-meta">{format!("Estimated cost: ${cost:.4}")}</div>
                        }.into_any())}
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h2>"Diagnose"</h2>
                            <button
                                class="btn tiny"
                                on:click=move |_| store.update(toggle_diagnose)
                            >
                                {if diagnose_open { "Hide" } else { "Show" }}
                            </button>
                        </div>
                        {if diagnose_open {
                            AnyView::from(Fragment::new(
                                diagnose_rows
                                    .into_iter()
                                    .map(|(label, value)| view! {
                                        <div class="side-kv">
                                            <span class="side-stat-label">{label}</span>
                                            <span class="side-row-text">{value}</span>
                                        </div>
                                    }.into_any())
                                    .collect::<Vec<_>>(),
                            ))
                        } else {
                            view! { <div class="side-meta">"Cache hit ratio, context use, and memory size"</div> }.into_any()
                        }}
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h2>"Changed files"</h2>
                            <div class="side-head-actions">
                                {(!changes.is_empty()).then(|| view! {
                                    <>
                                        <span class="badge">{changes.len()}</span>
                                        <button
                                            class="btn tiny"
                                            title="Open workspace diff (Alt+Shift+D)"
                                            on:click=move |_| store.update(|s| open_diff_modal(s, None))
                                        >"Diff"</button>
                                    </>
                                })}
                                <button
                                    class="btn tiny"
                                    title="Refresh git status"
                                    on:click=move |_| {
                                        if let Some(t) = crate::transport::current() {
                                            t.fetch_workspace_changes();
                                        }
                                    }
                                >"Refresh"</button>
                            </div>
                        </div>
                        {if changes.is_empty() {
                            view! { <div class="side-empty">"Working tree clean"</div> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(
                                changes
                                    .into_iter()
                                    .map(|change| {
                                        let path = change.path.clone();
                                        let status = change.status.clone();
                                        view! {
                                            <button
                                                class="side-row"
                                                title="Open diff"
                                                on:click=move |_| store.update(|s| open_diff_modal_with_status(s, Some(path.clone()), Some(status.clone())))
                                            >
                                                <span class=format!("diff-badge {}", change.status.to_ascii_lowercase())>
                                                    {change.status.clone()}
                                                </span>
                                                <span class="side-row-text">{change.path.clone()}</span>
                                            </button>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h2>"Tools & MCP"</h2>
                            <button
                                class="btn tiny"
                                on:click=move |_| store.update(|s| s.sidebar_tools_expanded = !s.sidebar_tools_expanded)
                            >
                                {if tools_expanded { "Hide" } else { "Show" }}
                            </button>
                        </div>
                        <div class="side-meta">{format!("{tool_count} built-in tools · {} MCP server(s)", mcp_servers.len())}</div>
                        {if mcp_servers.is_empty() {
                            view! { <span></span> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(
                                mcp_servers
                                    .into_iter()
                                    .map(|srv| {
                                        view! {
                                            <div class="side-kv">
                                                <span class="side-row-text">{format!("{} ({})", srv.name, srv.kind)}</span>
                                                <span class=format!("badge {}", if srv.enabled { "ok" } else { "muted" })>
                                                    {format!("{} tools", srv.tools.len())}
                                                </span>
                                            </div>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                        <Show when=move || tools_expanded>
                            <div class="side-tools-list">
                                {AnyView::from(Fragment::new(
                                    enabled_tools
                                        .iter()
                                        .map(|t| view! { <code class="tool-chip">{t.clone()}</code> }.into_any())
                                        .collect(),
                                ))}
                            </div>
                        </Show>
                    </section>
                    <section class="side-section">
                        <h2>"Subagents"</h2>
                        {if subagents.is_empty() {
                            view! { <div class="side-empty">"None"</div> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(
                                subagents
                                    .into_iter()
                                    .map(|(id, agent, status, child)| {
                                        let click_id = id.clone();
                                        view! {
                                            <button
                                                class="side-row"
                                                on:click=move |_| store.update(|s| {
                                                    s.modal = Some(ActiveModal::SubagentOverlay {
                                                        entry_id: click_id.clone(),
                                                        agent: agent.clone(),
                                                        status: status.clone(),
                                                        child_session_id: child.clone(),
                                                        entries: Vec::new(),
                                                        usage: None,
                                                        loading: child.is_some(),
                                                        error: None,
                                                    });
                                                    if let Some(child) = child.clone()
                                                        && let Some(t) = crate::transport::current()
                                                    {
                                                        t.fetch_session_events(child, true);
                                                    }
                                                })
                                            >
                                                <span class=format!("tab-dot {}", if status == "running" { "running" } else { "idle" })></span>
                                                <span class="side-row-text">{agent.clone()}</span>
                                                <span class="badge muted">{status.clone()}</span>
                                            </button>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                    </section>
                </aside>
            }
            .into_any()
        }}
    }
}

pub fn mode_label(mode: ApprovalModeDto) -> &'static str {
    match mode {
        ApprovalModeDto::Plan => "plan",
        ApprovalModeDto::Ask => "ask",
        ApprovalModeDto::Auto => "auto",
    }
}

pub fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::WaitingApproval => "approval",
        SessionStatus::WaitingQuestion => "question",
    }
}

pub fn status_class(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::WaitingApproval => "waiting",
        SessionStatus::WaitingQuestion => "waiting",
    }
}

pub fn connection_label(connection: ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Reconnecting => "reconnecting",
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Resyncing => "resyncing",
    }
}

pub fn connection_class(connection: ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Connecting | ConnectionState::Reconnecting => "waiting",
        ConnectionState::Connected => "ok",
        ConnectionState::Disconnected => "error",
        ConnectionState::Resyncing => "waiting",
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out = String::new();
    for _ in 0..max {
        let Some(ch) = chars.next() else {
            return out;
        };
        out.push(ch);
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

pub fn command_label(spec: &SlashCommandSpec) -> String {
    if spec.args.is_empty() {
        spec.cmd.to_owned()
    } else {
        format!("{} {}", spec.cmd, spec.args)
    }
}
