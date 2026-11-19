use wasm_bindgen::JsCast;

use crate::state::{
    ActiveModal, ProviderModalReturn, accept_completion, activate_provider_from_modal,
    copy_last_reply, custom_model_candidate, cycle_approval_mode, cycle_tab, detach_tab_view,
    execute_palette_action, execute_slash_command, filtered_commands, filtered_models_sorted,
    filtered_palette_actions, filtered_providers_sorted, open_diff_modal, open_model_input,
    open_search_modal, reorder_active_tab, select_tab_index, set_status_notice,
    steer_or_send_queued, submit_or_queue_composer, update_completion,
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
    let code = ev.code();
    let alt = ev.alt_key();
    let ctrl = ev.ctrl_key();
    let meta = ev.meta_key();
    let shift = ev.shift_key();
    let text_target = is_text_target(ev);

    if key == "Escape" {
        let handled = store.with(|s| match &s.modal {
            Some(ActiveModal::Approval {
                showing_reason: true,
                ..
            }) => 1u8,
            Some(_) => 2u8,
            None => 0u8,
        });
        if handled == 1 {
            store.update(|s| {
                if let Some(ActiveModal::Approval { showing_reason, .. }) = &mut s.modal {
                    *showing_reason = false;
                }
            });
            ev.prevent_default();
            return;
        }
        if handled == 2 {
            store.update(|s| s.modal = None);
            ev.prevent_default();
            return;
        }
    }

    if let Some(modal) = store.with(|s| s.modal.clone()) {
        if key == "Tab" && trap_modal_focus(shift) {
            ev.prevent_default();
            return;
        }
        if handle_modal_key(store, modal, ev, &key, &lower, text_target) {
            return;
        }
        if !text_target {
            return;
        }
    }

    if alt
        && !ctrl
        && !meta
        && !shift
        && code == "KeyS"
        && !ev.is_composing()
        && store.with(|s| s.modal.is_none())
    {
        store.update(|s| s.session_panel.reveal(None));
        ev.prevent_default();
        return;
    }

    if key == "Escape" && store.with(|s| s.session_panel.narrow_open()) {
        crate::views::close_sessions(store);
        ev.prevent_default();
        return;
    }

    if text_target {
        return;
    }

    if alt && !ctrl && !meta {
        if shift {
            match code.as_str() {
                "BracketLeft" => {
                    store.update(|s| reorder_active_tab(s, -1));
                    ev.prevent_default();
                    return;
                }
                "BracketRight" => {
                    store.update(|s| reorder_active_tab(s, 1));
                    ev.prevent_default();
                    return;
                }
                "KeyD" => {
                    store.update(|s| open_diff_modal(s, None));
                    ev.prevent_default();
                    return;
                }
                "KeyF" => {
                    store.update(|s| {
                        open_search_modal(s, meerai_web_protocol::SearchModeDto::Content, "")
                    });
                    ev.prevent_default();
                    return;
                }
                _ => {}
            }
        }
        match code.as_str() {
            "KeyK" => {
                open_command_palette(store);
                ev.prevent_default();
                return;
            }
            "KeyA" => {
                store.effect(crate::text::UiEffect::Focus("composer".to_owned()));
                ev.prevent_default();
                return;
            }
            "BracketLeft" => {
                store.update(|s| cycle_tab(s, -1));
                ev.prevent_default();
                return;
            }
            "BracketRight" => {
                store.update(|s| cycle_tab(s, 1));
                ev.prevent_default();
                return;
            }
            "KeyL" => {
                store.update(open_model_input);
                ev.prevent_default();
                return;
            }
            "KeyM" => {
                store.update(cycle_approval_mode);
                ev.prevent_default();
                return;
            }
            "KeyG" => {
                store.update(|s| steer_or_send_queued(s, 0));
                ev.prevent_default();
                return;
            }
            "KeyX" => {
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
            "KeyC" => {
                store.update_with_effects(|s, fx| {
                    if copy_last_reply(s, fx) {
                        set_status_notice(s, "Copied the latest reply");
                    }
                });
                ev.prevent_default();
                return;
            }
            "KeyW" => {
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
        if let Some(digit) = code.strip_prefix("Digit")
            && let Some(ch) = digit.chars().next()
            && ch.is_ascii_digit()
            && ch != '0'
        {
            let index = ch as usize - '1' as usize;
            store.update(|s| select_tab_index(s, index));
            ev.prevent_default();
            return;
        }
    }

    if ctrl && !alt && !meta && code == "KeyB" {
        store.update(|s| s.sidebar_open = !s.sidebar_open);
        ev.prevent_default();
        return;
    }

    if key == "?" {
        store.update(|s| s.modal = Some(ActiveModal::Help));
        ev.prevent_default();
    }
}

fn handle_modal_key(
    store: StoreHandle,
    modal: ActiveModal,
    ev: &web_sys::KeyboardEvent,
    key: &str,
    lower: &str,
    text_target: bool,
) -> bool {
    match modal {
        ActiveModal::Approval {
            session_id,
            request,
            showing_reason,
            ..
        } => {
            if !showing_reason && !text_target {
                let decision = match lower {
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
                    return true;
                }
                if lower == "r" {
                    store.update_with_effects(|s, fx| {
                        if let Some(ActiveModal::Approval { showing_reason, .. }) = &mut s.modal {
                            *showing_reason = true;
                            fx.focus("approval-reason");
                        }
                    });
                    ev.prevent_default();
                    return true;
                }
            }
            false
        }
        ActiveModal::CommandPalette { filter, selected } => {
            let cmds = filtered_commands(&filter.text);
            let actions = filtered_palette_actions(&filter.text);
            let total = cmds.len() + actions.len();
            match key {
                "ArrowDown" => {
                    if total > 0 {
                        store.update(|s| {
                            if let Some(ActiveModal::CommandPalette { selected, .. }) = &mut s.modal
                            {
                                *selected = (*selected + 1) % total;
                            }
                        });
                    }
                    ev.prevent_default();
                    true
                }
                "ArrowUp" => {
                    if total > 0 {
                        store.update(|s| {
                            if let Some(ActiveModal::CommandPalette { selected, .. }) = &mut s.modal
                            {
                                *selected = (*selected + total - 1) % total;
                            }
                        });
                    }
                    ev.prevent_default();
                    true
                }
                "Enter" if total > 0 => {
                    let idx = selected.min(total - 1);
                    if idx < cmds.len() {
                        let spec = cmds[idx];
                        let cmd = spec.cmd.to_owned();
                        if spec.takes_arg {
                            store.update_with_effects(|s, fx| {
                                s.draft.replace_text(format!("{cmd} "));
                                s.modal = None;
                                update_completion(s);
                                fx.focus("composer");
                            });
                        } else {
                            store.update(|s| {
                                s.modal = None;
                                execute_slash_command(s, &cmd);
                            });
                        }
                    } else {
                        let action_key = actions[idx - cmds.len()].key;
                        store.update_with_effects(|s, fx| {
                            s.modal = None;
                            execute_palette_action(s, fx, action_key);
                        });
                    }
                    ev.prevent_default();
                    true
                }
                _ => false,
            }
        }
        ActiveModal::ProviderSelect { filter, selected } => {
            let providers = store.with(|s| {
                let active_provider = s.active_session().map(|sess| sess.provider.as_str());
                filtered_providers_sorted(s.providers.as_ref(), active_provider, &filter.text)
            });
            let total = providers.len();
            match key {
                "ArrowDown" => {
                    if total > 0 {
                        store.update(|s| {
                            if let Some(ActiveModal::ProviderSelect { selected, .. }) = &mut s.modal
                            {
                                *selected = (*selected + 1) % total;
                            }
                        });
                    }
                    ev.prevent_default();
                    true
                }
                "ArrowUp" => {
                    if total > 0 {
                        store.update(|s| {
                            if let Some(ActiveModal::ProviderSelect { selected, .. }) = &mut s.modal
                            {
                                *selected = (*selected + total - 1) % total;
                            }
                        });
                    }
                    ev.prevent_default();
                    true
                }
                "Enter" if total > 0 => {
                    let provider = providers[selected.min(total - 1)].clone();
                    store.update(|s| {
                        activate_provider_from_modal(
                            s,
                            provider,
                            ProviderModalReturn::ProviderSelect,
                        );
                    });
                    ev.prevent_default();
                    true
                }
                _ => false,
            }
        }
        ActiveModal::ModelInput {
            filter,
            provider_id,
            selected,
            ..
        } => {
            if is_select_target(ev) {
                return false;
            }
            let (is_acp, models, custom) = store.with(|s| {
                let is_acp = s.active_session().is_some_and(|sess| {
                    matches!(sess.backend, meerai_web_protocol::BackendKind::Acp { .. })
                });
                let active_provider = s.active_session().map(|sess| sess.provider.as_str());
                let models = filtered_models_sorted(
                    s.providers.as_ref(),
                    provider_id.as_deref(),
                    active_provider,
                    &filter.text,
                );
                let custom =
                    custom_model_candidate(s, provider_id.as_deref(), &filter.text, &models);
                (is_acp, models, custom)
            });
            if is_acp {
                return false;
            }
            let total = models.len() + usize::from(custom.is_some());
            match key {
                "ArrowDown" => {
                    if total > 0 {
                        store.update(|s| {
                            if let Some(ActiveModal::ModelInput { selected, .. }) = &mut s.modal {
                                *selected = (*selected + 1) % total;
                            }
                        });
                    }
                    ev.prevent_default();
                    true
                }
                "ArrowUp" => {
                    if total > 0 {
                        store.update(|s| {
                            if let Some(ActiveModal::ModelInput { selected, .. }) = &mut s.modal {
                                *selected = (*selected + total - 1) % total;
                            }
                        });
                    }
                    ev.prevent_default();
                    true
                }
                "Enter" if total > 0 => {
                    let idx = selected.min(total - 1);
                    let target = if let Some(model) = models.get(idx) {
                        Some((model.provider.clone(), model.id.clone()))
                    } else {
                        custom
                    };
                    if let (Some((provider, model)), Some(id), Some(t)) = (
                        target,
                        store.with(|s| s.active_session_id.clone()),
                        crate::transport::current(),
                    ) {
                        t.set_model(id, provider, model);
                    }
                    store.update(|s| s.modal = None);
                    ev.prevent_default();
                    true
                }
                _ => false,
            }
        }
        ActiveModal::RewindMenu {
            turns,
            selected_turn,
            scope,
        } => {
            let total = turns.len();
            match key {
                "ArrowDown" if total > 0 => {
                    store.update(|s| {
                        if let Some(ActiveModal::RewindMenu { selected_turn, .. }) = &mut s.modal {
                            *selected_turn = (*selected_turn + 1) % total;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "ArrowUp" if total > 0 => {
                    store.update(|s| {
                        if let Some(ActiveModal::RewindMenu { selected_turn, .. }) = &mut s.modal {
                            *selected_turn = (*selected_turn + total - 1) % total;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "ArrowLeft" | "ArrowRight" => {
                    let next = match scope {
                        meerai_web_protocol::RewindScopeDto::Both => {
                            meerai_web_protocol::RewindScopeDto::Code
                        }
                        meerai_web_protocol::RewindScopeDto::Code => {
                            meerai_web_protocol::RewindScopeDto::Conversation
                        }
                        meerai_web_protocol::RewindScopeDto::Conversation => {
                            meerai_web_protocol::RewindScopeDto::Both
                        }
                    };
                    store.update(|s| {
                        if let Some(ActiveModal::RewindMenu { scope, .. }) = &mut s.modal {
                            *scope = next;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "Enter" if total > 0 => {
                    let turn_id = turns[selected_turn.min(total - 1)].0.clone();
                    if let (Some(session_id), Some(t)) = (
                        store.with(|s| s.active_session_id.clone()),
                        crate::transport::current(),
                    ) {
                        t.rewind_session(session_id, turn_id, scope);
                    }
                    store.update(|s| s.modal = None);
                    ev.prevent_default();
                    true
                }
                _ => false,
            }
        }
        ActiveModal::Search {
            results, selected, ..
        } => {
            let matches = results.map(|r| r.matches).unwrap_or_default();
            let total = matches.len();
            match key {
                "ArrowDown" if total > 0 => {
                    let next = (selected + 1) % total;
                    let item = &matches[next];
                    if let Some(t) = crate::transport::current() {
                        t.preview_workspace(item.path.clone(), item.line);
                    }
                    store.update(|s| {
                        if let Some(ActiveModal::Search { selected, .. }) = &mut s.modal {
                            *selected = next;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "ArrowUp" if total > 0 => {
                    let next = (selected + total - 1) % total;
                    let item = &matches[next];
                    if let Some(t) = crate::transport::current() {
                        t.preview_workspace(item.path.clone(), item.line);
                    }
                    store.update(|s| {
                        if let Some(ActiveModal::Search { selected, .. }) = &mut s.modal {
                            *selected = next;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                _ => false,
            }
        }
        ActiveModal::Terminal {
            command, running, ..
        } => {
            if key == "Enter" && !running && !command.text.trim().is_empty() {
                store.update(|s| crate::state::execute_user_shell(s, command.text.trim()));
                ev.prevent_default();
                return true;
            }
            false
        }
        ActiveModal::Question {
            session_id,
            request,
            draft,
        } if !text_target => {
            let step = draft.step.min(request.questions.len().saturating_sub(1));
            let Some(q) = request.questions.get(step) else {
                return false;
            };
            let opt_len = q.options.len();
            match key {
                "ArrowDown" if opt_len > 0 => {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            draft.focused_option = (draft.focused_option + 1) % opt_len;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "ArrowUp" if opt_len > 0 => {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            draft.focused_option = (draft.focused_option + opt_len - 1) % opt_len;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                " " if opt_len > 0 => {
                    let label = q.options[draft.focused_option.min(opt_len - 1)]
                        .label
                        .clone();
                    let multiple = q.multiple;
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            let answers = &mut draft.answers[step];
                            if multiple {
                                if let Some(pos) = answers.iter().position(|v| v == &label) {
                                    answers.remove(pos);
                                } else {
                                    answers.push(label);
                                }
                            } else {
                                *answers = vec![label];
                            }
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "ArrowLeft" => {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            draft.step = draft.step.saturating_sub(1);
                            draft.focused_option = 0;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "ArrowRight" => {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal
                            && draft.step + 1 < draft.answers.len()
                        {
                            draft.step += 1;
                            draft.focused_option = 0;
                        }
                    });
                    ev.prevent_default();
                    true
                }
                "Enter" => {
                    if step + 1 < request.questions.len() {
                        store.update(|s| {
                            if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                                draft.step += 1;
                                draft.focused_option = 0;
                            }
                        });
                    } else {
                        let answers = build_question_answers(&request, &draft);
                        if let Some(t) = crate::transport::current() {
                            t.answer_question(session_id, request.request_id, answers);
                        }
                        store.update(|s| s.modal = None);
                    }
                    ev.prevent_default();
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

pub fn build_question_answers(
    request: &meerai_web_protocol::QuestionRequestDto,
    draft: &crate::state::QuestionDraft,
) -> Vec<meerai_web_protocol::QuestionAnswerDto> {
    request
        .questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let mut selected = draft.answers.get(index).cloned().unwrap_or_default();
            let custom = draft
                .custom_inputs
                .get(index)
                .map(|input| input.text.trim().to_owned())
                .unwrap_or_default();
            if !custom.is_empty() {
                selected.push(custom);
            }
            meerai_web_protocol::QuestionAnswerDto {
                header: question.header.clone(),
                selected,
            }
        })
        .collect()
}

fn trap_modal_focus(shift: bool) -> bool {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return false;
    };
    let Ok(Some(modal)) = document.query_selector(".modal") else {
        return false;
    };
    let Ok(nodes) = modal.query_selector_all(
        "button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), a[href], [tabindex]:not([tabindex='-1'])",
    ) else {
        return false;
    };
    let mut focusables = Vec::new();
    for i in 0..nodes.length() {
        if let Some(node) = nodes.item(i)
            && let Ok(el) = node.dyn_into::<web_sys::HtmlElement>()
        {
            focusables.push(el);
        }
    }
    if focusables.is_empty() {
        if let Ok(el) = modal.dyn_into::<web_sys::HtmlElement>() {
            let _ = el.focus();
        }
        return true;
    }
    let active = document
        .active_element()
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
    let current_idx = active.as_ref().and_then(|cur| {
        focusables
            .iter()
            .position(|el| el.is_same_node(Some(cur.as_ref())))
    });
    let next_idx = match current_idx {
        Some(idx) if shift => {
            if idx == 0 {
                focusables.len() - 1
            } else {
                idx - 1
            }
        }
        Some(idx) => (idx + 1) % focusables.len(),
        None => {
            if shift {
                focusables.len() - 1
            } else {
                0
            }
        }
    };
    let _ = focusables[next_idx].focus();
    true
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

fn is_select_target(ev: &web_sys::KeyboardEvent) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::HtmlElement>() else {
        return false;
    };
    element.tag_name().eq_ignore_ascii_case("select")
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
        let utf16_len = textarea.value().encode_utf16().count();
        position >= utf16_len
    }
}
