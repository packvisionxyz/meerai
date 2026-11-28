use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{ApprovalModeDto, BackendKind, SessionStatus};

use crate::state::{
    CompletionKind, MentionCategory, RunKindDto, accept_completion, open_model_input, open_runs,
    remove_mention_token, reopen_pending_request, resolved_mention_chips, running_shell_count,
    running_subagent_count, set_approval_mode, submit_or_queue_composer, update_completion,
};
use crate::store::StoreHandle;

use super::app::mode_label;
use super::icons::{IconClose, IconSend, IconSparkles};

#[component]
pub fn Composer(store: StoreHandle) -> impl IntoView {
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
            let session = s.active_session().cloned();
            let has_session = session.is_some();
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
            let session_status = session.as_ref().map(|x| x.status);
            let active_tool = session.as_ref().and_then(|x| x.active_tool.clone());
            let mode = session
                .as_ref()
                .map(|x| x.approval_mode)
                .unwrap_or_default();
            let is_acp = session
                .as_ref()
                .is_some_and(|x| matches!(x.backend, BackendKind::Acp { .. }));
            let acp_preset = session.as_ref().and_then(|x| match &x.backend {
                BackendKind::Acp { preset, .. } => Some(preset.clone()),
                BackendKind::Native => None,
            });
            let model_label = session
                .as_ref()
                .map(|x| format!("{}/{}", x.provider, x.model))
                .unwrap_or_else(|| "No model".to_owned());
            (
                has_session,
                queue,
                chips,
                s.completion.active,
                session_status,
                active_tool,
                pending_label,
                s.modal.is_some(),
                mode,
                is_acp,
                acp_preset,
                model_label,
            )
        })
    });

    let can_send = Memo::new(move |_| store.with(|s| s.can_submit_composer()));
    let busy = Memo::new(move |_| store.with(|s| s.is_busy()));
    let run_counts =
        Memo::new(move |_| store.with(|s| (running_subagent_count(s), running_shell_count(s))));

    view! {
        <div class="composer">
            <div class="composer-shell conversation-column">
                <Show when=move || !composer_state.get().1.is_empty()>
                    <div class="queue-strip" role="list" aria-label="Queued prompts">
                        {move || {
                            let items = composer_state.get().1;
                            AnyView::from(Fragment::new(
                                items
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, prompt)| {
                                        let steer_index = index;
                                        let drop_index = index;
                                        let edit_index = index;
                                        view! {
                                            <div class="queue-row" role="listitem">
                                                <button
                                                    type="button"
                                                    class="queue-item"
                                                    title="Steer this prompt now (Alt+G)"
                                                    on:click=move |_| store.update(|s| crate::state::steer_or_send_queued(s, steer_index))
                                                >
                                                    <span class="queue-steer">"Steer"</span>
                                                    <span class="queue-text">{super::app::truncate(&prompt, 84)}</span>
                                                </button>
                                                <div class="queue-actions">
                                                    <button
                                                        type="button"
                                                        class="queue-drop"
                                                        title="Edit this queued prompt"
                                                        on:click=move |_| store.update(|s| crate::state::open_queued_edit(s, edit_index))
                                                    >"Edit"</button>
                                                    <button
                                                        type="button"
                                                        class="queue-drop"
                                                        title="Remove from queue"
                                                        aria-label="Remove from queue"
                                                        on:click=move |_| store.update(|s| crate::state::remove_queued(s, drop_index))
                                                    ><IconClose /></button>
                                                </div>
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
                    let (_, _, _, _, _, _, pending_label, modal_open, ..) = composer_state.get();
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
                                                        aria-label="Remove mention"
                                                        on:click=move |_| store.update(|s| {
                                                            let next = remove_mention_token(&s.draft.text, &raw_token);
                                                            s.draft.replace_text(next.clone());
                                                            if let Some(tab) = s.active_tab_mut() {
                                                                tab.draft_text = next;
                                                            }
                                                            update_completion(s);
                                                        })
                                                    ><IconClose /></button>
                                                </span>
                                            }
                                            .into_any()
                                        })
                                        .collect(),
                                ))
                            }}
                        </div>
                    </Show>
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
                            "Message meerai… (/ commands, @ mentions, ! shell)"
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
                    <div class="composer-toolbar">
                        <div class="composer-toolbar-left">
                            {move || {
                                let (_, _, _, _, _, _, _, _, mode, is_acp, acp_preset, model_label) =
                                    composer_state.get();
                                let display_model = acp_preset
                                    .as_ref()
                                    .map(|p| format!("ACP:{p}"))
                                    .unwrap_or(model_label);
                                view! {
                                    <>
                                        <button
                                            type="button"
                                            class="composer-pill"
                                            disabled=is_acp
                                            title=if is_acp {
                                                "ACP sessions manage their own model"
                                            } else {
                                                "Switch model (Alt+L)"
                                            }
                                            on:click=move |_| store.update(open_model_input)
                                        >
                                            <IconSparkles />
                                            <span class="composer-pill-text">{display_model}</span>
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
                                                            type="button"
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
                                    </>
                                }
                            }}
                            {move || {
                                let (agents, shells) = run_counts.get();
                                (agents > 0 || shells > 0).then(|| {
                                    view! {
                                        <div class="run-chips">
                                            {(agents > 0).then(|| view! {
                                                <button
                                                    type="button"
                                                    class="run-chip"
                                                    on:click=move |_| store.update(|s| open_runs(s, RunKindDto::Agents))
                                                >
                                                    {format!("{agents} {}", if agents == 1 { "agent" } else { "agents" })}
                                                </button>
                                            })}
                                            {(shells > 0).then(|| view! {
                                                <button
                                                    type="button"
                                                    class="run-chip"
                                                    on:click=move |_| store.update(|s| open_runs(s, RunKindDto::Shells))
                                                >
                                                    {format!("{shells} {}", if shells == 1 { "shell" } else { "shells" })}
                                                </button>
                                            })}
                                        </div>
                                    }
                                })
                            }}
                        </div>
                        <div class="composer-toolbar-right">
                            {move || {
                                let (_, _, _, _, session_status, active_tool, ..) = composer_state.get();
                                busy.get().then(|| {
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
                                })
                            }}
                            {move || {
                                (busy.get() && can_send.get()).then(|| view! {
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
                                        "Stop"
                                    </button>
                                })
                            }}
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
                                            <IconSend />
                                        </button>
                                    }
                                    .into_any()
                                }
                            }}
                        </div>
                    </div>
                </form>
                <div class="composer-footer">
                    <div class="composer-hint">
                        <kbd>"/"</kbd>" commands"
                        <span class="hint-sep">"·"</span>
                        <kbd>"@"</kbd>" mentions"
                        <span class="hint-sep">"·"</span>
                        <kbd>"!"</kbd>" shell"
                        <span class="hint-sep">"·"</span>
                        <kbd>"Shift+Enter"</kbd>" newline"
                    </div>
                    <div class="composer-footer-right">
                        <span class="composer-send-hint"><kbd>"Enter"</kbd>" to send"</span>
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
