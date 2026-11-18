use wasm_bindgen::JsCast;

use crate::state::{
    ActiveModal, accept_completion, copy_last_reply, cycle_approval_mode, cycle_tab,
    detach_tab_view, open_diff_modal, open_model_input, open_search_modal, open_session_select,
    reorder_active_tab, select_tab_index, steer_or_send_queued, submit_or_queue_composer,
    update_completion,
};
use crate::store::StoreHandle;

pub fn open_command_palette(store: StoreHandle) {
    store.update(|s| {
        s.modal = Some(ActiveModal::CommandPalette {
            filter: crate::text::TextBuffer::new(""),
            selected: 0,
        });
    });
}

pub fn handle_global_key(store: StoreHandle, ev: &web_sys::KeyboardEvent) {
    let key = ev.key();
    let lower = key.to_ascii_lowercase();
    let alt = ev.alt_key();
    let ctrl = ev.ctrl_key();
    let meta = ev.meta_key();
    let shift = ev.shift_key();
    let text_target = is_text_target(ev);

    if ev.key() == "Escape" {
        let closed = store.with(|s| s.modal.is_some());
        if closed {
            store.update(|s| s.modal = None);
            ev.prevent_default();
            return;
        }
    }

    if let Some(modal) = store.with(|s| s.modal.clone()) {
        if let ActiveModal::Approval {
            session_id,
            request,
            showing_reason,
            ..
        } = modal
            && !showing_reason
            && !text_target
        {
            let decision = match lower.as_str() {
                "y" => Some((meerai_web_protocol::ApprovalDecisionDto::Accept, None)),
                "t" => Some((
                    meerai_web_protocol::ApprovalDecisionDto::Accept,
                    Some(meerai_web_protocol::GrantScopeDto::Turn),
                )),
                "a" => Some((
                    meerai_web_protocol::ApprovalDecisionDto::AcceptForSession,
                    Some(meerai_web_protocol::GrantScopeDto::Session),
                )),
                "n" => Some((meerai_web_protocol::ApprovalDecisionDto::Decline, None)),
                "c" => Some((meerai_web_protocol::ApprovalDecisionDto::Cancel, None)),
                _ => None,
            };
            if let Some((decision, scope)) = decision {
                if let Some(t) = crate::transport::current() {
                    t.decide_approval(session_id, request.request_id, decision, scope);
                }
                store.update(|s| s.modal = None);
                ev.prevent_default();
                return;
            }
            if lower == "r" {
                store.update(|s| {
                    if let Some(ActiveModal::Approval { showing_reason, .. }) = &mut s.modal {
                        *showing_reason = true;
                    }
                });
                ev.prevent_default();
                return;
            }
        }
        if !text_target {
            return;
        }
    }

    if text_target {
        return;
    }

    if alt && !ctrl && !meta {
        if shift {
            match lower.as_str() {
                "[" => {
                    store.update(|s| reorder_active_tab(s, -1));
                    ev.prevent_default();
                    return;
                }
                "]" => {
                    store.update(|s| reorder_active_tab(s, 1));
                    ev.prevent_default();
                    return;
                }
                "d" => {
                    store.update(|s| open_diff_modal(s, None));
                    ev.prevent_default();
                    return;
                }
                "f" => {
                    store.update(|s| {
                        open_search_modal(s, meerai_web_protocol::SearchModeDto::Content, "")
                    });
                    ev.prevent_default();
                    return;
                }
                _ => {}
            }
        }
        match lower.as_str() {
            "k" => {
                open_command_palette(store);
                ev.prevent_default();
                return;
            }
            "a" => {
                store.effect(crate::text::UiEffect::Focus("composer".to_owned()));
                ev.prevent_default();
                return;
            }
            "[" => {
                store.update(|s| cycle_tab(s, -1));
                ev.prevent_default();
                return;
            }
            "]" => {
                store.update(|s| cycle_tab(s, 1));
                ev.prevent_default();
                return;
            }
            "s" => {
                store.update(|s| open_session_select(s, ""));
                ev.prevent_default();
                return;
            }
            "l" => {
                store.update(open_model_input);
                ev.prevent_default();
                return;
            }
            "m" => {
                store.update(cycle_approval_mode);
                ev.prevent_default();
                return;
            }
            "g" => {
                store.update(|s| steer_or_send_queued(s, 0));
                ev.prevent_default();
                return;
            }
            "x" => {
                store.update(|s| {
                    if let Some(id) = s.active_session_id.clone()
                        && let Some(t) = crate::transport::current()
                    {
                        t.cancel_turn(id);
                    }
                });
                ev.prevent_default();
                return;
            }
            "c" => {
                store.update_with_effects(|s, fx| {
                    if copy_last_reply(s, fx) {
                        s.status_notice = Some("Copied the latest reply".to_owned());
                    }
                });
                ev.prevent_default();
                return;
            }
            "w" => {
                store.update(|s| {
                    if let Some(id) = s.active_session_id.clone() {
                        detach_tab_view(s, &id);
                    }
                });
                ev.prevent_default();
                return;
            }
            _ => {}
        }
        if let Some(ch) = lower.chars().next()
            && ch.is_ascii_digit()
            && ch != '0'
        {
            let index = ch as usize - '1' as usize;
            store.update(|s| select_tab_index(s, index));
            ev.prevent_default();
            return;
        }
    }

    if ctrl && !alt && !meta {
        if lower == "b" {
            store.update(|s| s.sidebar_open = !s.sidebar_open);
            ev.prevent_default();
            return;
        }
    }

    if lower == "?" {
        store.update(|s| s.modal = Some(ActiveModal::Help));
        ev.prevent_default();
    }
}

pub fn handle_composer_key(store: StoreHandle, ev: &web_sys::KeyboardEvent) {
    let key = ev.key();
    let active = store.with(|s| s.completion.active);
    if active {
        match key.as_str() {
            "ArrowDown" => {
                let len = store.with(|s| s.completion.items.len());
                if len > 0 {
                    store.update(|s| s.completion.selected = (s.completion.selected + 1) % len);
                }
                ev.prevent_default();
                return;
            }
            "ArrowUp" => {
                let len = store.with(|s| s.completion.items.len());
                if len > 0 {
                    store.update(|s| {
                        s.completion.selected = (s.completion.selected + len - 1) % len;
                    });
                }
                ev.prevent_default();
                return;
            }
            "Tab" | "Enter" => {
                if !ev.shift_key() {
                    let index = store.with(|s| s.completion.selected);
                    store.update(|s| {
                        accept_completion(s, index);
                    });
                    ev.prevent_default();
                    return;
                }
            }
            "Escape" => {
                store.update(|s| s.completion.active = false);
                ev.prevent_default();
                return;
            }
            _ => {}
        }
    }

    if key == "Enter" && !ev.shift_key() && !ev.alt_key() && !ev.ctrl_key() && !ev.meta_key() {
        if ev.is_composing() {
            return;
        }
        ev.prevent_default();
        store.update(submit_or_queue_composer);
        return;
    }

    if key == "ArrowUp" && !ev.shift_key() {
        let at_start = textarea_at_boundary(ev, true);
        if at_start {
            store.update(|s| {
                if s.prompt_history.is_empty() {
                    return;
                }
                let next = match s.prompt_history_idx {
                    Some(index) => index.saturating_sub(1),
                    None => {
                        s.saved_draft_before_history = s.draft.text.clone();
                        s.prompt_history.len() - 1
                    }
                };
                s.prompt_history_idx = Some(next);
                if let Some(entry) = s.prompt_history.get(next).cloned() {
                    s.draft.replace_text(entry);
                }
            });
            ev.prevent_default();
            return;
        }
    }

    if key == "ArrowDown" && !ev.shift_key() {
        let at_end = textarea_at_boundary(ev, false);
        if at_end {
            store.update(|s| {
                let Some(index) = s.prompt_history_idx else {
                    return;
                };
                if index + 1 < s.prompt_history.len() {
                    let next = index + 1;
                    s.prompt_history_idx = Some(next);
                    if let Some(entry) = s.prompt_history.get(next).cloned() {
                        s.draft.replace_text(entry);
                    }
                } else {
                    s.prompt_history_idx = None;
                    let restored = s.saved_draft_before_history.clone();
                    s.draft.replace_text(restored);
                }
            });
            ev.prevent_default();
        }
    }

    if key == "Escape" {
        store.update(|s| {
            s.completion.active = false;
            update_completion(s);
        });
    }
}

fn is_text_target(ev: &web_sys::KeyboardEvent) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::HtmlElement>() else {
        return false;
    };
    matches!(
        element.tag_name().to_ascii_lowercase().as_str(),
        "input" | "textarea" | "select"
    )
}

fn textarea_at_boundary(ev: &web_sys::KeyboardEvent, start: bool) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let Ok(textarea) = target.dyn_into::<web_sys::HtmlTextAreaElement>() else {
        return false;
    };
    let position = textarea.selection_start().ok().flatten().unwrap_or(0) as usize;
    if start {
        position == 0
    } else {
        position >= textarea.value().len()
    }
}
