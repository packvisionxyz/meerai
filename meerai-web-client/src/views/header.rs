use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{BackendKind, EffortDto, SessionStatus};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::keymap::open_command_palette;
use crate::state::{
    ActiveModal, ConnectionState, SettingsSection, activate_and_switch_session, detach_tab_view,
    format_duration_ms, move_tab_to, open_diff_modal, open_provider_select, open_search_modal,
    open_settings, reopen_pending_request, set_status_notice,
};
use crate::store::StoreHandle;

use super::app::{BrandMark, connection_class, connection_label, status_class, status_label};
use super::icons::{
    IconClose, IconCommand, IconDiff, IconGitBranch, IconHelp, IconPlug, IconPlus, IconSearch,
    IconServer, IconSettings, IconSidebarLeft, IconSidebarRight,
};
use super::sessions;

#[component]
pub fn TopBar(store: StoreHandle) -> impl IntoView {
    let topbar_info = Memo::new(move |_| {
        store.with(|s| {
            let session = s.active_session().cloned();
            let pending_count = s
                .active_tab()
                .map(|t| t.pending_approvals.len() + t.pending_questions.len())
                .unwrap_or(0);
            let modal_open = s.modal.is_some();
            (
                session,
                s.file_changes.len(),
                s.host_info.update_available.clone(),
                s.host_info.install_hint.clone(),
                pending_count,
                modal_open,
            )
        })
    });

    view! {
        <header class="topbar" inert=move || sessions::background_inert(store)>
            <div class="topbar-left">
                <button
                    id="sessions-toggle"
                    class="icon-action"
                    class:active=move || store.with(|s| s.session_panel.open)
                    aria-expanded=move || store.with(|s| s.session_panel.open).to_string()
                    title="Sessions (Alt+S)"
                    aria-label="Toggle sessions"
                    on:click=move |_| sessions::toggle(store)
                >
                    <IconSidebarLeft />
                </button>
                <div class="brand">
                    <BrandMark />
                    <span class="brand-mark">"meerai"</span>
                </div>
            </div>
            <div class="topbar-center">
                {move || {
                    let title = topbar_info
                        .get()
                        .0
                        .and_then(|s| s.title)
                        .unwrap_or_else(|| "New session".to_owned());
                    view! { <h1 class="sr-only">{title}</h1> }
                }}
                <TabStrip store=store />
            </div>
            <div class="topbar-actions">
                {move || {
                    let (session, change_count, update_available, install_hint, pending_count, modal_open) =
                        topbar_info.get();
                    let acp_preset = session.as_ref().and_then(|s| match &s.backend {
                        BackendKind::Acp { preset, .. } => Some(preset.clone()),
                        BackendKind::Native => None,
                    });
                    view! {
                        <>
                            {acp_preset.map(|preset| view! {
                                <span
                                    class="topbar-pill info"
                                    title="External ACP backend keeps its own tools and policy."
                                >
                                    {format!("ACP:{preset}")}
                                </span>
                            }.into_any())}
                            {(pending_count > 0 && !modal_open).then(|| view! {
                                <button
                                    class="topbar-pill warn"
                                    title="Reopen pending approval or question"
                                    on:click=move |_| store.update(reopen_pending_request)
                                >
                                    {format!("{pending_count} pending")}
                                </button>
                            })}
                            {update_available.map(|ver| {
                                let hint = install_hint.clone();
                                view! {
                                    <button
                                        class="topbar-pill accent"
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
                            <button
                                class="topbar-btn"
                                title="Workspace diff (Alt+Shift+D)"
                                on:click=move |_| store.update(|s| open_diff_modal(s, None))
                            >
                                <IconDiff />
                                <span class="topbar-btn-label">"Diff"</span>
                                {(change_count > 0).then(|| view! {
                                    <span class="action-count">{change_count}</span>
                                })}
                            </button>
                            <button
                                class="topbar-btn"
                                title="Workspace search (Alt+Shift+F)"
                                on:click=move |_| store.update(|s| open_search_modal(s, meerai_web_protocol::SearchModeDto::Content, ""))
                            >
                                <IconSearch />
                                <span class="topbar-btn-label">"Search"</span>
                            </button>
                            <button
                                class="topbar-btn"
                                title="Command palette (Alt+K)"
                                on:click=move |_| open_command_palette(store)
                            >
                                <IconCommand />
                                <span class="topbar-btn-label">"Palette"</span>
                            </button>
                            <button
                                class="topbar-btn"
                                title="Providers"
                                on:click=move |_| store.update(open_provider_select)
                            >
                                <IconPlug />
                                <span class="topbar-btn-label">"Connect"</span>
                            </button>
                            <button
                                class="topbar-btn"
                                title="Settings"
                                on:click=move |_| store.update(|s| open_settings(s, None))
                            >
                                <IconSettings />
                                <span class="topbar-btn-label">"Settings"</span>
                            </button>
                            <span class="header-sep" aria-hidden="true"></span>
                            <button
                                class="icon-action"
                                title="Help (?)"
                                aria-label="Help"
                                on:click=move |_| store.update(|s| s.modal = Some(ActiveModal::Help))
                            >
                                <IconHelp />
                            </button>
                            <button
                                class="icon-action"
                                class:active=move || store.with(|s| s.sidebar_open)
                                aria-expanded=move || store.with(|s| s.sidebar_open).to_string()
                                title="Toggle inspector (Ctrl+B)"
                                aria-label="Toggle inspector"
                                on:click=move |_| store.update(|s| s.sidebar_open = !s.sidebar_open)
                            >
                                <IconSidebarRight />
                            </button>
                        </>
                    }
                }}
            </div>
        </header>
    }
}

#[component]
pub fn BannerStack(store: StoreHandle) -> impl IntoView {
    let banner_info = Memo::new(move |_| {
        store.with(|s| {
            (
                s.connection == ConnectionState::Disconnected,
                s.error_banner.clone(),
                s.status_notice.clone(),
                s.resume_command_hint(),
            )
        })
    });

    view! {
        {move || {
            let (disconnected, banner, notice, resume_hint) = banner_info.get();
            if !disconnected && banner.is_none() && notice.is_none() {
                return view! { <span></span> }.into_any();
            }
            let resume_cmd = resume_hint.unwrap_or_else(|| "meerai --resume <id>".to_owned());
            view! {
                <div class="banner-stack" inert=move || sessions::background_inert(store)>
                    {banner.map(|err| view! {
                        <div class="banner error banner-row" role="alert">
                            <span>{err}</span>
                            <button
                                class="banner-close"
                                title="Dismiss"
                                aria-label="Dismiss error"
                                on:click=move |_| store.update(|s| s.error_banner = None)
                            ><IconClose /></button>
                        </div>
                    }.into_any())}
                    {notice.map(|msg| view! {
                        <div class="banner notice banner-row">
                            <span>{msg}</span>
                            <button
                                class="banner-close"
                                title="Dismiss"
                                aria-label="Dismiss notice"
                                on:click=move |_| store.update(|s| s.status_notice = None)
                            ><IconClose /></button>
                        </div>
                    }.into_any())}
                    {disconnected.then(|| view! {
                        <div class="banner error banner-row" role="alert">
                            <span>
                                "Host disconnected. The session log is safe. Restart with "
                                <code>{resume_cmd}</code>
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
                    })}
                </div>
            }
            .into_any()
        }}
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
                            let tab_tooltip = title.clone();
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
                                        title=tab_tooltip
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
                                        aria-label="Close tab"
                                        on:click=move |_| store.update(|s| detach_tab_view(s, &close_id))
                                    >
                                        <IconClose />
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
            >
                <IconPlus />
            </button>
        </div>
    }
}

#[derive(Clone, PartialEq)]
struct StatusBarSnapshot {
    full_root: String,
    short_root: String,
    branch: Option<String>,
    connection: ConnectionState,
    session: Option<meerai_web_protocol::SessionSummaryDto>,
    usage: meerai_web_protocol::UsageDto,
    metrics: meerai_web_protocol::TurnMetricsDto,
    queue_len: usize,
    effort: EffortDto,
    temp: f32,
    mcp_connected: usize,
    mcp_failed: usize,
    mcp_needs_trust: usize,
    mcp_total: usize,
}

#[component]
pub fn StatusBar(store: StoreHandle) -> impl IntoView {
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
            let effort = context.effort.unwrap_or(EffortDto::Medium);
            let temp = context
                .temperature
                .filter(|t| (0.0..=2.0).contains(t) && t.is_finite())
                .unwrap_or_else(|| crate::state::effort_default_temperature(effort));
            let servers = &s.host_info.mcp_servers;
            let mcp_failed = servers
                .iter()
                .filter(|server| matches!(server.status.as_str(), "failed" | "unsupported"))
                .count();
            let mcp_needs_trust = servers
                .iter()
                .filter(|server| matches!(server.status.as_str(), "needs trust" | "untrusted"))
                .count();
            let mcp_connected = servers
                .iter()
                .filter(|server| matches!(server.status.as_str(), "connected" | "configured"))
                .count();
            let mcp_total = servers.len();
            StatusBarSnapshot {
                full_root: root,
                short_root,
                branch,
                connection: s.connection,
                session,
                usage,
                metrics,
                queue_len,
                effort,
                temp,
                mcp_connected,
                mcp_failed,
                mcp_needs_trust,
                mcp_total,
            }
        })
    });

    view! {
        {move || {
            let StatusBarSnapshot {
                full_root,
                short_root,
                branch,
                connection,
                session,
                usage,
                metrics,
                queue_len,
                effort,
                temp,
                mcp_connected,
                mcp_failed,
                mcp_needs_trust,
                mcp_total,
            } = line_memo.get();

            let activity = session.as_ref().and_then(|sess| {
                if let Some(tool) = &sess.active_tool {
                    Some(format!("{} {tool}", crate::state::activity_verb(tool)))
                } else if sess.status != SessionStatus::Idle {
                    Some(status_label(sess.status).to_owned())
                } else {
                    None
                }
            });

            let timing = session.as_ref().and_then(|sess| {
                if sess.status != SessionStatus::Idle
                    && let Some(started) = sess.turn_started_at_ms
                {
                    let elapsed = now_ms.get().saturating_sub(started);
                    Some(format_duration_ms(elapsed))
                } else {
                    metrics.last_turn_ms.map(format_duration_ms)
                }
            });

            let mcp_warn = mcp_failed > 0 || mcp_needs_trust > 0;
            let mut mcp_text = format!("mcp {mcp_connected}/{mcp_total}");
            if mcp_failed > 0 {
                mcp_text.push_str(&format!(" · {mcp_failed} failed"));
            }
            if mcp_needs_trust > 0 {
                mcp_text.push_str(&format!(" · {mcp_needs_trust} needs trust"));
            }

            view! {
                <footer class="statusbar" inert=move || sessions::background_inert(store)>
                    <div class="statusbar-left">
                        <span class=format!("status-conn {}", connection_class(connection))>
                            <span class="status-dot" aria-hidden="true"></span>
                            <span>{connection_label(connection)}</span>
                        </span>
                        <span class="status-sep" aria-hidden="true">"·"</span>
                        <span class="status-workspace" title=full_root>{short_root}</span>
                        {branch.map(|b| view! {
                            <span class="status-branch">
                                <IconGitBranch />
                                <span>{b}</span>
                            </span>
                        })}
                        {activity.map(|label| view! {
                            <span class="status-activity">{label}</span>
                        })}
                        {timing.map(|t| view! {
                            <span class="status-metric">{t}</span>
                        })}
                        {(queue_len > 0).then(|| view! {
                            <span class="status-metric">{format!("{queue_len} queued")}</span>
                        })}
                    </div>
                    <div class="statusbar-right">
                        <button
                            type="button"
                            class="status-btn"
                            title="Sampling & effort settings"
                            on:click=move |_| store.update(|s| open_settings(s, Some(SettingsSection::Context)))
                        >
                            {format!("temp {temp:.2} · {}", crate::state::effort_label(effort))}
                        </button>
                        {usage.est_cost_usd.filter(|c| *c > 0.0).map(|cost| view! {
                            <span class="status-metric">{format!("${cost:.4}")}</span>
                        })}
                        {metrics.tokens_per_sec.filter(|t| *t > 0.0).map(|tps| view! {
                            <span class="status-metric">{format!("{tps:.1} tok/s")}</span>
                        })}
                        {(mcp_total > 0).then(|| view! {
                            <button
                                type="button"
                                class=format!("status-chip {}", if mcp_warn { "warn" } else { "ok" })
                                title="Show MCP servers (/mcp)"
                                on:click=move |_| store.update(|s| s.modal = Some(ActiveModal::McpServers { selected: 0 }))
                            >
                                <IconServer />
                                <span>{mcp_text}</span>
                            </button>
                        })}
                    </div>
                </footer>
            }
        }}
    }
}
