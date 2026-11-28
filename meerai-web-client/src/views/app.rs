use std::sync::{Arc, Mutex};

use leptos::ev;
use leptos::prelude::*;
use meerai_web_protocol::{ApprovalModeDto, SessionStatus};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::state::{ConnectionState, SlashCommandSpec};
use crate::store::StoreHandle;
use crate::theme::{BrowserTheme, UiTheme};

use super::{header, modals, sessions, sidebar, transcript};

#[component]
pub fn BrandMark() -> impl IntoView {
    view! {
        <svg class="brand-symbol" viewBox="0 0 10 8" width="20" height="16" aria-hidden="true">
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
            <header::TopBar store=store />
            <header::BannerStack store=store />
            <div class="workspace">
                <sessions::SessionPanel store=store />
                <main class="workspace-main" aria-label="Conversation" inert=move || sessions::background_inert(store)>
                    <transcript::Transcript store=store />
                    <super::composer::Composer store=store />
                </main>
                <Show when=move || sidebar_open.get()>
                    <div
                        class="sidebar-scrim"
                        on:click=move |_| store.update(|s| s.sidebar_open = false)
                    ></div>
                </Show>
                <sidebar::Sidebar store=store />
            </div>
            <header::StatusBar store=store />
            <modals::ModalLayer store=store />
        </div>
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
    spec.label()
}
