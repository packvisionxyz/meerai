pub mod modals;
pub mod settings;
pub mod transcript;

use leptos::ev;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

use crate::state::{
    ActiveModal, CompletionKind, ConnectionState, SettingsFormState, SlashCommandSpec,
    accept_completion, cycle_approval_mode, detach_tab_view, open_diff_modal, open_model_input,
    open_provider_select, open_search_modal, open_session_select, submit_or_queue_composer,
    switch_to_session, update_completion,
};
use crate::store::StoreHandle;
use crate::theme::UiTheme;

#[component]
pub fn App(store: StoreHandle) -> impl IntoView {
    let key_store = store.clone();
    let listener = window_event_listener(ev::keydown, move |ev| {
        crate::keymap::handle_global_key(key_store, &ev);
    });
    on_cleanup(move || listener.remove());

    let vars =
        Memo::new(move |_| store.with(|s| UiTheme::resolve(&s.host_info.settings.theme).vars));

    view! {
        <div class="app" style=move || vars.get()>
            <Header store=store.clone() />
            <div class="workspace">
                <transcript::Transcript store=store.clone() />
                <Sidebar store=store.clone() />
            </div>
            <Composer store=store.clone() />
            <modals::ModalLayer store=store.clone() />
        </div>
    }
}

#[component]
fn Header(store: StoreHandle) -> impl IntoView {
    let title = move || {
        store.with(|s| {
            s.active_session()
                .and_then(|session| session.title.clone())
                .unwrap_or_else(|| "New session".to_owned())
        })
    };
    let connection = move || store.with(|s| s.connection);
    let status = move || {
        store.with(|s| {
            s.active_session()
                .map(|session| session.status)
                .unwrap_or(meerai_web_protocol::SessionStatus::Idle)
        })
    };
    let meta = move || {
        store.with(|s| {
            let session = s.active_session();
            let mode = s
                .active_session()
                .map(|session| session.approval_mode)
                .unwrap_or_default();
            let provider = session.map(|s| s.provider.clone()).unwrap_or_default();
            let model = session.map(|s| s.model.clone()).unwrap_or_default();
            format!("{provider}/{model} · {}", mode_label(mode))
        })
    };
    let banner = move || store.with(|s| s.error_banner.clone());
    let notice = move || store.with(|s| s.status_notice.clone());
    let disconnected = move || store.with(|s| s.connection == ConnectionState::Disconnected);

    view! {
        <header class="header">
            <div class="brand">
                <span class="brand-mark">"meerai"</span>
                <span class="brand-sub">"local workspace"</span>
            </div>
            <div class="header-main">
                <h1 class="session-title">{title}</h1>
                <div class="header-meta">
                    <span class=move || format!("conn {}", connection_class(connection()))>
                        {move || connection_label(connection())}
                    </span>
                    <span class=move || format!("state {}", status_class(status()))>
                        {move || status_label(status())}
                    </span>
                    <span class="meta-text">{meta}</span>
                </div>
            </div>
            <div class="header-actions">
                <button class="btn subtle" title="New session" on:click=move |_| {
                    store.update(|s| { s.modal = None; if let Some(t) = crate::transport::current() { t.create_session(None); } })
                }>"New"</button>
                <button class="btn subtle" title="Sessions" on:click=move |_| store.update(|s| open_session_select(s, ""))>"Sessions"</button>
                <button class="btn subtle" title="Models" on:click=move |_| store.update(open_model_input)>"Models"</button>
                <button class="btn subtle" title="Approval mode" on:click=move |_| store.update(cycle_approval_mode)>"Mode"</button>
                <button class="btn subtle" title="Workspace diff" on:click=move |_| store.update(|s| open_diff_modal(s, None))>"Diff"</button>
                <button class="btn subtle" title="Workspace search" on:click=move |_| store.update(|s| open_search_modal(s, meerai_web_protocol::SearchModeDto::Content, ""))>"Search"</button>
                <button class="btn subtle" title="Providers" on:click=move |_| store.update(open_provider_select)>"Connect"</button>
                <button
                    class="btn subtle icon-btn"
                    title="Toggle sidebar (Ctrl+B)"
                    aria-label="Toggle sidebar"
                    on:click=move |_| store.update(|s| s.sidebar_open = !s.sidebar_open)
                >"▤"</button>
                <button class="btn subtle" title="Settings" on:click=move |_| store.update(|s| {
                    s.modal = Some(ActiveModal::Settings(SettingsFormState::from_host(&s.host_info)));
                })>"Settings"</button>
                <button class="btn subtle" title="Help" on:click=move |_| store.update(|s| s.modal = Some(ActiveModal::Help))>"?"</button>
            </div>
            <TabStrip store=store.clone() />
            <StatusLine store=store.clone() />
            <Show when=move || banner().is_some()>
                <div class="banner error">{move || banner().unwrap_or_default()}</div>
            </Show>
            <Show when=move || notice().is_some()>
                <div class="banner notice">{move || notice().unwrap_or_default()}</div>
            </Show>
            <Show when=disconnected>
                <div class="banner error">
                    "Host disconnected. The session log is safe. Restart with "
                    <code>"meerai --resume &lt;id&gt;"</code>
                    " to continue."
                </div>
            </Show>
        </header>
    }
}

#[component]
fn TabStrip(store: StoreHandle) -> impl IntoView {
    view! {
        <div class="tabs" role="tablist" aria-label="Sessions">
            {move || {
                let tabs = store.with(|s| {
                    let active = s.active_session_id.clone();
                    s.visible_sessions()
                        .into_iter()
                        .map(|session| {
                            (
                                session.id.clone(),
                                session
                                    .title
                                    .clone()
                                    .unwrap_or_else(|| "New session".to_owned()),
                                status_class(session.status),
                                s.session_tabs.get(&session.id).is_some_and(|tab| tab.unread),
                                active.as_deref() == Some(session.id.as_str()),
                            )
                        })
                        .collect::<Vec<_>>()
                });
                AnyView::from(Fragment::new(
                    tabs.into_iter()
                        .map(|(id, title, status, unread, active)| {
                            let click_id = id.clone();
                            let close_id = id.clone();
                            view! {
                                <div class="tab" class:active=active>
                                    <button
                                        class="tab-main"
                                        role="tab"
                                        aria-selected=active
                                        on:click=move |_| store.update(|s| switch_to_session(s, &click_id))
                                    >
                                        <span class=format!("tab-dot {status}")></span>
                                        <span class="tab-title">{title}</span>
                                        <Show when=move || unread>
                                            <span class="tab-unread" title="Unread output"></span>
                                        </Show>
                                    </button>
                                    <button
                                        class="tab-close"
                                        title="Close view"
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
        </div>
    }
}

#[component]
fn StatusLine(store: StoreHandle) -> impl IntoView {
    let text = move || {
        store.with(|s| {
            let workspace = if s.host_info.workspace_root.is_empty() {
                "workspace".to_owned()
            } else {
                s.host_info.workspace_root.clone()
            };
            let branch = s
                .host_info
                .git_branch
                .clone()
                .map(|b| format!(" · {b}"))
                .unwrap_or_default();
            let usage = s
                .active_tab()
                .map(|tab| {
                    format!(
                        " · ↑{} ↓{} Σ{}",
                        tab.usage.prompt_tokens.unwrap_or(0),
                        tab.usage.completion_tokens.unwrap_or(0),
                        tab.usage.total_tokens.unwrap_or(0)
                    )
                })
                .unwrap_or_default();
            let active = s
                .active_tab()
                .map(|tab| tab.pending_queue.len())
                .unwrap_or(0);
            let queue = if active == 0 {
                String::new()
            } else {
                format!(" · {active} queued")
            };
            format!("{workspace}{branch}{usage}{queue}")
        })
    };
    view! { <div class="statusline">{text}</div> }
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
    let busy = move || store.with(|s| s.is_busy());
    let disconnected = move || store.with(|s| s.connection == ConnectionState::Disconnected);
    let queue = move || {
        store.with(|s| {
            s.active_tab()
                .map(|tab| tab.pending_queue.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    };
    let completion = move || {
        store.with(|s| {
            (
                s.completion.active,
                s.completion.items.clone(),
                s.completion.selected,
                s.completion.kind,
            )
        })
    };

    view! {
        <footer class="composer">
            <Show when=move || !queue().is_empty()>
                <div class="queue-strip">
                    {move || {
                        let items = queue();
                        AnyView::from(Fragment::new(
                            items
                                .into_iter()
                                .enumerate()
                                .map(|(index, prompt)| {
                                    let steer_index = index;
                                    view! {
                                        <button
                                            class="queue-item"
                                            title="Steer this prompt now"
                                            on:click=move |_| store.update(|s| crate::state::steer_or_send_queued(s, steer_index))
                                        >
                                            {truncate(&prompt, 80)}
                                        </button>
                                    }
                                    .into_any()
                                })
                                .collect(),
                        ))
                    }}
                </div>
            </Show>
            <Show when=move || completion().0>
                <CompletionPopup store=store.clone() />
            </Show>
            <div class="composer-card">
                <textarea
                    id="composer"
                    class="composer-input"
                    rows="2"
                    aria-label="Message meerai"
                    placeholder="Ask meerai to change, review, or explain this workspace…"
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
                <div class="composer-toolbar">
                    <div class="composer-hint">
                        <span class="hint-key">"/"</span>" commands"
                        <span class="hint-sep">"·"</span>
                        <span class="hint-key">"@"</span>" mentions"
                        <span class="hint-sep">"·"</span>
                        <span class="hint-key">"!"</span>" shell"
                    </div>
                    <div class="composer-buttons">
                        <Show when=busy>
                            <button
                                class="btn subtle"
                                title="Cancel the running turn"
                                on:click=move |_| store.update(|s| {
                                    if let Some(id) = s.active_session_id.clone()
                                        && let Some(t) = crate::transport::current()
                                    {
                                        t.cancel_turn(id);
                                    }
                                })
                            >"Cancel"</button>
                        </Show>
                        <button
                            class="btn subtle icon-btn"
                            title="Compact context"
                            aria-label="Compact context"
                            on:click=move |_| store.update(|s| {
                                if let Some(id) = s.active_session_id.clone()
                                    && let Some(t) = crate::transport::current()
                                {
                                    t.compact_session(id);
                                }
                            })
                        >"Compact"</button>
                        <button
                            class="btn subtle icon-btn"
                            title="Reset conversation context"
                            aria-label="Reset conversation context"
                            on:click=move |_| store.update(|s| {
                                if let Some(id) = s.active_session_id.clone()
                                    && let Some(t) = crate::transport::current()
                                {
                                    t.reset_session(id);
                                }
                            })
                        >"Reset"</button>
                        <button
                            class="btn primary send-btn"
                            disabled=move || disconnected()
                            on:click=move |_| store.update(submit_or_queue_composer)
                        >
                            {move || if busy() { "Queue" } else { "Send" }}
                            <span class="send-arrow">"↑"</span>
                        </button>
                    </div>
                </div>
            </div>
            <div class="composer-footer">
                <span>{move || store.with(|s| format!("INS · {}", mode_label(s.active_session().map(|x| x.approval_mode).unwrap_or_default())))}</span>
                <span>{move || store.with(|s| s.active_session().map(|x| format!("{} / {}", x.provider, x.model)).unwrap_or_default())}</span>
                <span>{move || store.with(|s| {
                    let temperature = s.host_info.settings.context.temperature;
                    temperature.map(|t| format!("temp {t:.2}")).unwrap_or_else(|| "temp default".to_owned())
                })}</span>
            </div>
        </footer>
    }
}

#[component]
fn CompletionPopup(store: StoreHandle) -> impl IntoView {
    let items = move || {
        store.with(|s| {
            (
                s.completion.kind,
                s.completion.items.clone(),
                s.completion.selected,
            )
        })
    };
    view! {
        <div class="completion" role="listbox">
            {move || {
                let (kind, items, selected) = items();
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
                        view! {
                            <button
                                class="completion-row"
                                class:selected=index == selected
                                role="option"
                                aria-selected=index == selected
                                on:click=move |_| store.update(|s| { accept_completion(s, insert_index); })
                            >
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

#[component]
fn Sidebar(store: StoreHandle) -> impl IntoView {
    let open = move || store.with(|s| s.sidebar_open);
    view! {
        <Show when=open>
            <aside class="sidebar">
                <section class="side-section">
                    <h2>"Workspace"</h2>
                    <div class="side-path">{move || store.with(|s| s.host_info.workspace_root.clone())}</div>
                    <div class="side-meta">{move || store.with(|s| {
                        let branch = s.host_info.git_branch.clone().unwrap_or_else(|| "no branch".to_owned());
                        let agents = s.host_info.agents.len();
                        let skills = s.host_info.skills.len();
                        let hooks = s.host_info.hooks_count;
                        format!("{branch} · {agents} agents · {skills} skills · {hooks} hooks")
                    })}</div>
                </section>
                <section class="side-section">
                    <h2>"Changed files"</h2>
                    {move || {
                        let changes = store.with(|s| s.file_changes.clone());
                        if changes.is_empty() {
                            return view! { <div class="side-empty">"Working tree clean"</div> }.into_any();
                        }
                        AnyView::from(Fragment::new(
                            changes
                                .into_iter()
                                .map(|change| {
                                    let path = change.path.clone();
                                    view! {
                                        <button
                                            class="side-row"
                                            title="Open diff"
                                            on:click=move |_| store.update(|s| open_diff_modal(s, Some(path.clone())))
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
                    <h2>"Sessions"</h2>
                    {move || {
                        let sessions = store.with(|s| {
                            s.visible_sessions()
                                .into_iter()
                                .map(|session| {
                                    (
                                        session.id.clone(),
                                        session
                                            .title
                                            .clone()
                                            .unwrap_or_else(|| "New session".to_owned()),
                                        status_class(session.status),
                                    )
                                })
                                .collect::<Vec<_>>()
                        });
                        AnyView::from(Fragment::new(
                            sessions
                                .into_iter()
                                .map(|(id, title, status)| {
                                    let click_id = id.clone();
                                    view! {
                                        <button
                                            class="side-row"
                                            on:click=move |_| store.update(|s| switch_to_session(s, &click_id))
                                        >
                                            <span class=format!("tab-dot {status}")></span>
                                            <span class="side-row-text">{title}</span>
                                        </button>
                                    }
                                    .into_any()
                                })
                                .collect(),
                        ))
                    }}
                </section>
                <section class="side-section">
                    <h2>"Running agents"</h2>
                    {move || {
                        let agents = store.with(|s| {
                            s.active_tab()
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
                                .unwrap_or_default()
                        });
                        if agents.is_empty() {
                            return view! { <div class="side-empty">"None"</div> }.into_any();
                        }
                        AnyView::from(Fragment::new(
                            agents
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
                                        </button>
                                    }
                                    .into_any()
                                })
                                .collect(),
                        ))
                    }}
                </section>
            </aside>
        </Show>
    }
}

pub fn mode_label(mode: meerai_web_protocol::ApprovalModeDto) -> &'static str {
    match mode {
        meerai_web_protocol::ApprovalModeDto::Plan => "plan",
        meerai_web_protocol::ApprovalModeDto::Ask => "ask",
        meerai_web_protocol::ApprovalModeDto::Auto => "auto",
    }
}

pub fn status_label(status: meerai_web_protocol::SessionStatus) -> &'static str {
    match status {
        meerai_web_protocol::SessionStatus::Idle => "idle",
        meerai_web_protocol::SessionStatus::Running => "running",
        meerai_web_protocol::SessionStatus::WaitingApproval => "approval",
        meerai_web_protocol::SessionStatus::WaitingQuestion => "question",
    }
}

pub fn status_class(status: meerai_web_protocol::SessionStatus) -> &'static str {
    match status {
        meerai_web_protocol::SessionStatus::Idle => "idle",
        meerai_web_protocol::SessionStatus::Running => "running",
        meerai_web_protocol::SessionStatus::WaitingApproval => "waiting",
        meerai_web_protocol::SessionStatus::WaitingQuestion => "waiting",
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
