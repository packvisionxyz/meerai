use super::*;

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
