use leptos::ev;
use leptos::prelude::*;

use crate::state::{ConnectionState, HistoryStatus, SessionPanelAction};
use crate::store::StoreHandle;
use crate::text::UiEffect;

pub(super) fn setup(store: StoreHandle) {
    let sync_size = move || {
        let narrow = web_sys::window()
            .and_then(|window| window.match_media("(max-width: 900px)").ok().flatten())
            .is_some_and(|query| query.matches());
        if store.with(|s| s.session_panel.narrow != narrow) {
            store.update(|s| s.session_panel.set_narrow(narrow));
        }
    };
    sync_size();
    let resize = window_event_listener(ev::resize, move |_| sync_size());
    on_cleanup(move || resize.remove());

    let history_key = Memo::new(move |_| {
        store.with(|s| {
            (s.session_panel.open && s.connection == ConnectionState::Connected).then(|| {
                let mut ids = s
                    .sessions
                    .iter()
                    .map(|session| session.id.clone())
                    .collect::<Vec<_>>();
                ids.sort_unstable();
                ids
            })
        })
    });
    Effect::new(move |_| {
        if history_key.get().is_some() {
            refresh();
        }
    });

    let focus_request = Memo::new(move |_| store.with(|s| s.session_panel.focus_request));
    Effect::new(move |_| {
        if focus_request.get() != 0 {
            store.effect(UiEffect::Focus("sessions-filter".to_owned()));
        }
    });
}

pub(super) fn background_inert(store: StoreHandle) -> bool {
    store.with(|s| s.session_panel.narrow_open())
}

pub(super) fn toggle(store: StoreHandle) {
    if store.with(|s| s.session_panel.open) {
        close(store);
    } else {
        store.update(|s| s.session_panel.reveal(None));
    }
}

pub(crate) fn close(store: StoreHandle) {
    store.update_with_effects(|s, tx| {
        s.session_panel.close();
        tx.focus("sessions-toggle");
    });
}

fn refresh() {
    if let Some(transport) = crate::transport::current() {
        transport.fetch_history();
    }
}

fn act(action: SessionPanelAction, id: String) {
    if let Some(transport) = crate::transport::current() {
        transport.session_panel_action(action, id);
    }
}

#[component]
pub(super) fn SessionPanel(store: StoreHandle) -> impl IntoView {
    let open = Memo::new(move |_| store.with(|s| s.session_panel.open));
    let narrow = Memo::new(move |_| store.with(|s| s.session_panel.narrow));
    let filter = Memo::new(move |_| store.with(|s| s.session_panel.filter.clone()));
    let rows = Memo::new(move |_| store.with(|s| s.session_panel.rows(&s.sidebar_sessions())));
    let selected = Memo::new(move |_| {
        let count = rows.with(Vec::len);
        store.with(|s| s.session_panel.selected.min(count.saturating_sub(1)))
    });
    let pending = Memo::new(move |_| store.with(|s| s.session_panel.action_pending));
    let history = Memo::new(move |_| store.with(|s| s.session_panel.history_status.clone()));
    let action_error = Memo::new(move |_| store.with(|s| s.session_panel.action_error.clone()));

    let on_filter_key = move |ev: ev::KeyboardEvent| {
        if ev.is_composing() || ev.alt_key() || ev.ctrl_key() || ev.meta_key() || ev.shift_key() {
            return;
        }
        let count = rows.with(Vec::len);
        if count == 0 {
            return;
        }
        let index = selected.get_untracked();
        let next = match ev.key().as_str() {
            "ArrowDown" => (index + 1) % count,
            "ArrowUp" => (index + count - 1) % count,
            "Enter" => {
                ev.prevent_default();
                if !pending.get_untracked() {
                    let id = rows.with(|rows| rows[index].id.clone());
                    act(SessionPanelAction::Open, id);
                }
                return;
            }
            _ => return,
        };
        ev.prevent_default();
        store.update(|s| s.session_panel.selected = next);
        store.effect(UiEffect::ScrollIntoView(format!(
            "session-panel-row-{next}"
        )));
    };

    view! {
        <Show when=move || open.get()>
            <Show when=move || narrow.get()>
                <div class="sessions-scrim" aria-hidden="true" on:click=move |_| close(store)></div>
            </Show>
            <aside
                id="sessions-panel"
                class="sessions-panel"
                role=move || if narrow.get() { "dialog" } else { "complementary" }
                aria-modal=move || narrow.get().then_some("true")
                aria-labelledby="sessions-heading"
                inert=move || store.with(|s| s.modal.is_some())
            >
                <div class="sessions-heading">
                    <h2 id="sessions-heading">"Sessions"</h2>
                    <button
                        class="icon-action"
                        title="New session (/new)"
                        aria-label="New session"
                        on:click=move |_| {
                            if let Some(transport) = crate::transport::current() {
                                transport.create_session(None);
                            }
                        }
                    >
                        <super::icons::IconPlus />
                    </button>
                    <button
                        class="icon-action"
                        title="Refresh sessions"
                        aria-label="Refresh sessions"
                        on:click=move |_| refresh()
                    >
                        <super::icons::IconRefresh />
                    </button>
                    <button class="modal-close" aria-label="Close Sessions" on:click=move |_| close(store)>
                        <super::icons::IconClose />
                    </button>
                </div>
                <div class="search-input-wrap">
                    <span class="search-input-icon"><super::icons::IconSearch /></span>
                    <input
                        id="sessions-filter"
                        class="input"
                        aria-label="Filter sessions by title, model, or ID"
                        placeholder="Filter sessions"
                        prop:value=move || filter.get()
                        on:input=move |ev| store.update(|s| s.session_panel.set_filter(event_target_value(&ev)))
                        on:keydown=on_filter_key
                    />
                </div>
                <div class="sessions-status" aria-live="polite">
                    {move || match history.get() {
                        HistoryStatus::Loading => view! { <span class="hint">"Loading sessions…"</span> }.into_any(),
                        HistoryStatus::Failed(error) => view! { <span class="sessions-error">{format!("Cannot load sessions: {error}")}</span> }.into_any(),
                        HistoryStatus::Idle if rows.with(Vec::is_empty) => view! { <span class="hint">"No sessions match this filter."</span> }.into_any(),
                        HistoryStatus::Idle => ().into_any(),
                    }}
                    {move || action_error.get().map(|error| view! { <div class="sessions-error">{error}</div> })}
                </div>
                <ul class="sessions-list" aria-label="Session history">
                    {move || rows.get().into_iter().enumerate().map(|(index, row)| {
                        let id = row.id.clone();
                        let open_id = id.clone();
                        let fork_id = id.clone();
                        let delete_id = id.clone();
                        let confirm_id = id.clone();
                        let current_id = id.clone();
                        let confirming = Memo::new(move |_| store.with(|s| s.session_panel.confirm_delete.as_deref() == Some(confirm_id.as_str())));
                        let title = row.title.unwrap_or_else(|| "Untitled session".to_owned());
                        let details = match row.model {
                            Some(model) => format!("{model} · {id}"),
                            None => id,
                        };
                        view! {
                            <li
                                class="sessions-row"
                                id=format!("session-panel-row-{index}")
                                class:active=move || index == selected.get()
                                class:confirming=move || confirming.get()
                            >
                                <button
                                    class="sessions-open"
                                    aria-current=move || store.with(|s| (s.active_session_id.as_deref() == Some(current_id.as_str())).then_some("true"))
                                    aria-disabled=move || pending.get().to_string()
                                    on:click=move |_| act(SessionPanelAction::Open, open_id.clone())
                                >
                                    <span class="sessions-title-row">
                                        <span
                                            class=format!("session-dot {}", if row.live { "live" } else { "stored" })
                                            title=if row.live { "Live session" } else { "Stored session" }
                                            aria-hidden="true"
                                        ></span>
                                        <span class="sessions-title">{title}</span>
                                        <span class="sr-only">{if row.live { " (live)" } else { " (stored)" }}</span>
                                    </span>
                                    <span class="sessions-detail" title=details.clone()>{details.clone()}</span>
                                </button>
                                <div class="sessions-row-actions">
                                    <button class="btn tiny" aria-disabled=move || pending.get().to_string() on:click=move |_| act(SessionPanelAction::Fork, fork_id.clone())>"Fork"</button>
                                    <button class="btn tiny danger" aria-disabled=move || pending.get().to_string() on:click=move |_| {
                                        if pending.get_untracked() {
                                            return;
                                        }
                                        if confirming.get_untracked() {
                                            act(SessionPanelAction::Delete, delete_id.clone());
                                        } else {
                                            store.update(|s| s.session_panel.confirm_delete = Some(delete_id.clone()));
                                        }
                                    }>{move || if confirming.get() { "Confirm delete" } else { "Delete" }}</button>
                                    <Show when=move || confirming.get()>
                                        <button class="btn tiny" on:click=move |_| store.update(|s| s.session_panel.confirm_delete = None)>"Cancel"</button>
                                    </Show>
                                </div>
                            </li>
                        }
                    }).collect::<Vec<_>>()}
                </ul>
            </aside>
        </Show>
    }
}
