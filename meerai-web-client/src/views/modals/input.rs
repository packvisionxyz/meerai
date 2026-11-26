use super::*;

#[component]
fn ApprovalModal(store: StoreHandle) -> impl IntoView {
    let state_memo = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Approval {
                session_id,
                request,
                showing_reason,
                ..
            }) => Some((session_id.clone(), request.clone(), *showing_reason)),
            _ => None,
        })
    });
    let body = view! {
        {move || {
            let Some((_, request, showing_reason)) = state_memo.get() else {
                return view! { <span></span> }.into_any();
            };
            let title = request.title;
            let detail = request.detail;
            let effect = request
                .effect
                .map(|effect| format!("{effect:?}"))
                .unwrap_or_else(|| "unknown".to_owned());
            let initial_reason = store.with(|s| match &s.modal {
                Some(ActiveModal::Approval { reason, .. }) => reason.text.clone(),
                _ => String::new(),
            });
            view! {
                <div class="modal-form">
                    <div class="approval-kind">{format!("{:?}", request.kind)}</div>
                    <div class="approval-title">{title}</div>
                    <pre class="approval-detail">{detail}</pre>
                    <div class="kv"><span>"Effect"</span><strong>{effect}</strong></div>
                    <Show when=move || showing_reason>
                        <textarea
                            id="approval-reason"
                            class="input multiline"
                            rows="3"
                            placeholder="Reason for declining (Enter for a new line)"
                            prop:value=initial_reason.clone()
                            on:input=move |ev| {
                                let value = event_target_value(&ev);
                                store.update(|s| {
                                    if let Some(ActiveModal::Approval { reason, .. }) = &mut s.modal {
                                        reason.replace_text(value);
                                    }
                                });
                            }
                        ></textarea>
                    </Show>
                    <div class="hint">"Shortcuts: y approve once · t allow turn · a allow session · n decline · r reason · c cancel request"</div>
                </div>
            }
            .into_any()
        }}
    }
    .into_any();

    let actions = view! {
        {move || {
            let Some((session_id, request, showing_reason)) = state_memo.get() else {
                return view! { <span></span> }.into_any();
            };
            let request_id = request.request_id;
            let sid = session_id.clone();
            let rid = request_id.clone();
            let sid_turn = session_id.clone();
            let rid_turn = request_id.clone();
            let sid_session = session_id.clone();
            let rid_session = request_id.clone();
            let sid_decline = session_id.clone();
            let rid_decline = request_id.clone();
            let sid_reason = session_id.clone();
            let rid_reason = request_id.clone();
            let sid_cancel = session_id;
            let rid_cancel = request_id;
            view! {
                <button class="btn primary" on:click=move |_| decide(&store, &sid, &rid, ApprovalDecisionDto::Accept, None)>"Approve once (y)"</button>
                <button class="btn" on:click=move |_| decide(&store, &sid_turn, &rid_turn, ApprovalDecisionDto::Accept, Some(GrantScopeDto::Turn))>"Allow turn (t)"</button>
                <button class="btn" on:click=move |_| decide(&store, &sid_session, &rid_session, ApprovalDecisionDto::AcceptForSession, Some(GrantScopeDto::Session))>"Allow session (a)"</button>
                <button class="btn danger" on:click=move |_| decide(&store, &sid_decline, &rid_decline, ApprovalDecisionDto::Decline, None)>"Decline (n)"</button>
                {if showing_reason {
                    view! {
                        <button class="btn danger" on:click=move |_| {
                            let reason = store.with(|s| match &s.modal {
                                Some(ActiveModal::Approval { reason, .. }) => reason.text.clone(),
                                _ => String::new(),
                            });
                            let reason = if reason.trim().is_empty() {
                                "Declined by the browser client.".to_owned()
                            } else {
                                reason
                            };
                            decide(&store, &sid_reason, &rid_reason, ApprovalDecisionDto::DeclineWithReason { reason }, None);
                        }>"Send decline reason"</button>
                    }.into_any()
                } else {
                    view! {
                        <button class="btn" on:click=move |_| {
                            store.update_with_effects(|s, fx| {
                                if let Some(ActiveModal::Approval { showing_reason, .. }) = &mut s.modal {
                                    *showing_reason = true;
                                    fx.focus("approval-reason");
                                }
                            });
                        }>"Decline with reason (r)"</button>
                    }.into_any()
                }}
                <button class="btn" on:click=move |_| decide(&store, &sid_cancel, &rid_cancel, ApprovalDecisionDto::Cancel, None)>"Cancel request (c)"</button>
            }
            .into_any()
        }}
    }
    .into_any();
    shell("Approval required", store, body, actions)
}

#[component]
fn QuestionModal(store: StoreHandle) -> impl IntoView {
    let q_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Question {
                session_id,
                request,
                draft,
            }) => Some((
                session_id.clone(),
                request.clone(),
                draft.step,
                draft.answers.clone(),
                draft.focused_option,
            )),
            _ => None,
        })
    });
    let step_memo = Memo::new(move |_| q_state.get().map(|(_, _, step, _, _)| step).unwrap_or(0));

    let body = view! {
        <div class="modal-form">
            {move || {
                let Some((_, request, step, answers, focused_option)) = q_state.get() else {
                    return view! { <span></span> }.into_any();
                };
                let step = step.min(request.questions.len().saturating_sub(1));
                let Some(question) = request.questions.get(step) else {
                    return view! { <div class="modal-form">"No questions."</div> }.into_any();
                };
                let selected = answers.get(step).cloned().unwrap_or_default();
                let multiple = question.multiple;
                let options = question
                    .options
                    .iter()
                    .enumerate()
                    .map(|(idx, option)| {
                        let label = option.label.clone();
                        let label_click = label.clone();
                        let is_selected = selected.contains(&label);
                        let is_focused = idx == focused_option;
                        let marker = match (multiple, is_selected) {
                            (true, true) => "[x]",
                            (true, false) => "[ ]",
                            (false, true) => "(•)",
                            (false, false) => "( )",
                        };
                        view! {
                            <button
                                class="pick-row"
                                class:active=is_selected
                                class:focused=is_focused
                                on:click=move |_| {
                                    let label = label_click.clone();
                                    store.update(|s| {
                                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                                            let answers = &mut draft.answers[step];
                                            if multiple {
                                                if let Some(pos) = answers.iter().position(|value| value == &label) {
                                                    answers.remove(pos);
                                                } else {
                                                    answers.push(label.clone());
                                                }
                                            } else {
                                                *answers = vec![label.clone()];
                                            }
                                            draft.focused_option = idx;
                                        }
                                    });
                                }
                            >
                                <span class="pick-marker">{marker}</span>
                                <span class="pick-title">{label}</span>
                                {option.description.clone().map(|description| view! { <span class="pick-meta">{description}</span> }.into_any())}
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div class="modal-form">
                        <div class="hint">
                            {format!(
                                "Question {} of {} · {}",
                                step + 1,
                                request.questions.len(),
                                if multiple { "Select all that apply" } else { "Select one" }
                            )}
                        </div>
                        <div class="question-head">{question.header.clone()}</div>
                        <div class="question-text">{question.question.clone()}</div>
                        <div class="pick-list">{AnyView::from(Fragment::new(options))}</div>
                    </div>
                }
                .into_any()
            }}
            {move || {
                let step = step_memo.get();
                let custom = store.with(|s| match &s.modal {
                    Some(ActiveModal::Question { draft, .. }) => draft
                        .custom_inputs
                        .get(step)
                        .map(|input| input.text.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                });
                view! {
                    <textarea
                        class="input multiline"
                        rows="3"
                        placeholder="Other (type your own; Enter for a new line)"
                        prop:value=custom
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            store.update(|s| {
                                if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal
                                    && let Some(input) = draft.custom_inputs.get_mut(step)
                                {
                                    input.replace_text(value);
                                }
                            });
                        }
                    ></textarea>
                }
            }}
        </div>
    }
    .into_any();

    let actions = view! {
        {move || {
            let Some((session_id, request, step, _, _)) = q_state.get() else {
                return view! { <span></span> }.into_any();
            };
            let total = request.questions.len();
            let sid_submit = session_id.clone();
            let req_submit = request.clone();
            let sid_cancel = session_id;
            let rid_cancel = request.request_id;
            view! {
                <button class="btn" disabled={step == 0} on:click=move |_| {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            draft.step = draft.step.saturating_sub(1);
                            draft.focused_option = 0;
                        }
                    });
                }>"Back"</button>
                <button class="btn" disabled={step + 1 >= total} on:click=move |_| {
                    store.update(|s| {
                        if let Some(ActiveModal::Question { draft, .. }) = &mut s.modal {
                            let next = draft.step + 1;
                            if next < draft.answers.len() {
                                draft.step = next;
                                draft.focused_option = 0;
                            }
                        }
                    });
                }>"Next"</button>
                <button class="btn primary" on:click=move |_| {
                    let answers = store.with(|s| match &s.modal {
                        Some(ActiveModal::Question { draft, .. }) => build_question_answers(&req_submit, draft),
                        _ => Vec::new(),
                    });
                    if let Some(t) = crate::transport::current() {
                        t.answer_question(sid_submit.clone(), req_submit.request_id.clone(), answers);
                    }
                    store.update(|s| s.modal = None);
                }>"Submit answers"</button>
                <button class="btn danger" on:click=move |_| {
                    if let Some(t) = crate::transport::current() {
                        t.cancel_question(sid_cancel.clone(), rid_cancel.clone());
                    }
                    store.update(|s| s.modal = None);
                }>"Cancel"</button>
            }
            .into_any()
        }}
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Question { request, draft, .. }) => {
                let step = draft.step.min(request.questions.len().saturating_sub(1));
                request.questions.get(step).map_or_else(
                    || "Question".to_owned(),
                    |q| format!("Question — {}", q.header),
                )
            }
            _ => "Question".to_owned(),
        })
    };
    shell_reactive(title, store, body, actions)
}

#[component]
fn CommandPaletteModal(store: StoreHandle) -> impl IntoView {
    let list_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::CommandPalette { filter, selected }) => {
                (filter.text.clone(), *selected)
            }
            _ => (String::new(), 0),
        })
    });
    let initial_filter = store.with(|s| match &s.modal {
        Some(ActiveModal::CommandPalette { filter, .. }) => filter.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            <input
                class="input"
                placeholder="Search commands and actions"
                prop:value=initial_filter
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    store.update(|s| {
                        if let Some(ActiveModal::CommandPalette { filter, selected }) = &mut s.modal {
                            filter.replace_text(value);
                            *selected = 0;
                        }
                    });
                }
            />
            {move || {
                let (filter_text, selected) = list_state.get();
                let commands = filtered_commands(&filter_text);
                let actions = filtered_palette_actions(&filter_text);
                let cmd_len = commands.len();
                let rows = commands
                    .into_iter()
                    .enumerate()
                    .map(|(index, spec)| {
                        let cmd = spec.cmd.to_owned();
                        let takes_arg = spec.takes_arg;
                        let label = command_label(spec);
                        view! {
                            <button class="pick-row" class:active=index == selected on:click=move |_| {
                                if takes_arg {
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
                            }>
                                <span class="pick-title">{label}</span>
                                <span class="pick-meta">{spec.desc.to_owned()}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                let action_rows = actions
                    .into_iter()
                    .enumerate()
                    .map(|(idx, action)| {
                        let key = action.key.to_owned();
                        let is_selected = cmd_len + idx == selected;
                        view! {
                            <button class="pick-row" class:active=is_selected on:click=move |_| {
                                let key = key.clone();
                                store.update_with_effects(|s, fx| {
                                    s.modal = None;
                                    execute_palette_action(s, fx, &key);
                                });
                            }>
                                <span class="pick-title">{action.label.to_owned()}</span>
                                <span class="pick-meta">{action.hint.to_owned()}</span>
                            </button>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div class="modal-form">
                        <div class="palette-section">"Commands"</div>
                        <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                        <div class="palette-section">"Actions"</div>
                        <div class="pick-list">{AnyView::from(Fragment::new(action_rows))}</div>
                    </div>
                }
            }}
        </div>
    }
    .into_any();
    shell("Commands", store, body, empty_actions())
}
