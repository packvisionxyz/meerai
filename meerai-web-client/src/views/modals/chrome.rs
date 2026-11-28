use super::*;

pub(super) fn clear_search_timer(timer: &Mutex<Option<i32>>) {
    if let Ok(mut guard) = timer.lock()
        && let Some(id) = guard.take()
        && let Some(window) = web_sys::window()
    {
        window.clear_timeout_with_handle(id);
    }
}

pub(super) fn switch_search_mode(store: StoreHandle, next: SearchModeDto) {
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

pub(super) fn shell(title: &str, store: StoreHandle, body: AnyView, actions: AnyView) -> AnyView {
    shell_with_width(title, store, body, actions, false)
}

pub(super) fn shell_wide(
    title: &str,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
) -> AnyView {
    shell_with_width(title, store, body, actions, true)
}

pub(super) fn shell_reactive(
    title: impl Fn() -> String + Send + 'static,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
) -> AnyView {
    shell_reactive_class(title, store, body, actions, "modal")
}

pub(super) fn shell_reactive_full(
    title: impl Fn() -> String + Send + 'static,
    store: StoreHandle,
    body: AnyView,
    actions: AnyView,
) -> AnyView {
    shell_reactive_class(title, store, body, actions, "modal modal-full")
}

pub(super) fn shell_reactive_class(
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
                    <button class="modal-close" aria-label="Close dialog" title="Close (Esc)" on:click=move |_| store.update(|s| s.modal = None)>
                        <crate::views::icons::IconClose />
                    </button>
                </div>
                <div class="modal-body">{body}</div>
                <div class="modal-actions">{actions}</div>
            </div>
        </div>
    }
    .into_any()
}

pub(super) fn shell_with_width(
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
                    <button class="modal-close" aria-label="Close dialog" title="Close (Esc)" on:click=move |_| store.update(|s| s.modal = None)>
                        <crate::views::icons::IconClose />
                    </button>
                </div>
                <div class="modal-body">{body}</div>
                <div class="modal-actions">{actions}</div>
            </div>
        </div>
    }
    .into_any()
}

pub(super) fn empty_actions() -> AnyView {
    view! { <span></span> }.into_any()
}

pub(super) fn decide(
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

pub(super) fn set_scope(store: &StoreHandle, next: RewindScopeDto) {
    store.update(|s| {
        if let Some(ActiveModal::RewindMenu { scope, .. }) = &mut s.modal {
            *scope = next;
        }
    });
}

pub(super) fn help_modal(store: StoreHandle) -> AnyView {
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

#[component]
pub(super) fn McpServersModal(store: StoreHandle) -> impl IntoView {
    let servers = Memo::new(move |_| {
        let mut rows = store.with(|s| s.host_info.mcp_servers.clone());
        rows.sort_by_key(|server| match server.status.as_str() {
            "failed" | "unsupported" => 0,
            "needs trust" | "untrusted" => 1,
            "connected" | "configured" => 2,
            _ => 3,
        });
        rows
    });
    let selected = Memo::new(move |_| {
        let count = servers.with(Vec::len);
        store.with(|s| match &s.modal {
            Some(ActiveModal::McpServers { selected }) => (*selected).min(count.saturating_sub(1)),
            _ => 0,
        })
    });
    let title = move || {
        let rows = servers.get();
        let total = rows.len();
        let failed = rows
            .iter()
            .filter(|server| matches!(server.status.as_str(), "failed" | "unsupported"))
            .count();
        let connected = rows
            .iter()
            .filter(|server| matches!(server.status.as_str(), "connected" | "configured"))
            .count();
        format!("MCP servers ({connected} connected, {failed} failed, {total} total)")
    };
    let run_action = move |command: &'static str| {
        if let Some(t) = crate::transport::current() {
            t.run_host_action(command.to_owned());
        }
    };
    let body = view! {
        {move || {
            let rows = servers.get();
            if rows.is_empty() {
                return view! {
                    <div class="modal-form">
                        <div class="hint">"No MCP servers are configured. Add one to .mcp.json at the project root, then reconnect."</div>
                    </div>
                }.into_any();
            }
            let active = selected.get();
            let items = rows.into_iter().enumerate().map(|(index, server)| {
                let status_class = match server.status.as_str() {
                    "connected" | "configured" => "ok",
                    "failed" | "unsupported" => "warn",
                    "needs trust" | "untrusted" => "warn",
                    _ => "muted",
                };
                let needs_trust = matches!(server.status.as_str(), "needs trust" | "untrusted");
                let detail = server.detail.clone();
                let tool_count = server.tools.len();
                view! {
                    <div class=format!("mcp-row {}", if index == active { "selected" } else { "" })>
                        <div class="mcp-row-main">
                            <span class="panel-title">{server.name.clone()}</span>
                            <span class="panel-meta">{format!("({})", server.kind)}</span>
                            <span class=format!("badge {status_class}")>{
                                if server.status.is_empty() { format!("{tool_count} tools") } else { format!("{} · {tool_count} tools", server.status) }
                            }</span>
                            {if needs_trust {
                                view! {
                                    <button class="btn tiny" on:click=move |_| run_action("/mcp trust")>"Trust"</button>
                                }.into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }}
                        </div>
                        {detail.map(|detail| view! {
                            <div class="mcp-row-error">{format!("↳ {detail}")}</div>
                        }.into_any()).unwrap_or_else(|| view! { <span></span> }.into_any())}
                    </div>
                }.into_any()
            }).collect::<Vec<_>>();
            view! {
                <div class="modal-form">
                    <div class="hint">"Failed servers sort first. A server that failed to connect is never shown as connected."</div>
                    <div class="panel-list">{AnyView::from(Fragment::new(items))}</div>
                </div>
            }.into_any()
        }}
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| run_action("/mcp reload")>"Reload (r)"</button>
        <button class="btn" on:click=move |_| run_action("/mcp trust")>"Trust project (t)"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
    }
    .into_any();
    shell_reactive(title, store, body, actions)
}

#[component]
pub(super) fn VaultResetModal(store: StoreHandle) -> impl IntoView {
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
    let body = view! {
        <div class="modal-form">
            <div class="banner error">
                "This deletes every saved API key and OAuth credential and creates a new vault key. The old data cannot be recovered."
            </div>
            <label class="field">
                <span class="field-label">"Type reset to confirm"</span>
                <input
                    class="input"
                    placeholder="reset"
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
                <div class="banner error">{message}</div>
            })}
        </div>
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| store_cancel.update(|state| state.modal = None)>"Cancel"</button>
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
    }
    .into_any();
    shell("Reset credential vault", store, body, actions)
}
