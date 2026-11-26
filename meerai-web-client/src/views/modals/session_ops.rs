use super::*;

#[component]
pub(super) fn TerminalModal(store: StoreHandle) -> impl IntoView {
    let status_memo = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Terminal {
                output,
                running,
                error,
                ..
            }) => (output.clone(), *running, error.clone()),
            _ => (None, false, None),
        })
    });
    let initial_cmd = store.with(|s| match &s.modal {
        Some(ActiveModal::Terminal { command, .. }) => command.text.clone(),
        _ => String::new(),
    });
    let body = view! {
        <div class="modal-form">
            <label class="field">
                <span class="field-label">"Command"</span>
                <input
                    class="input"
                    placeholder="Command, without the leading !"
                    prop:value=initial_cmd
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::Terminal { command, .. }) = &mut s.modal {
                                command.replace_text(value);
                            }
                        });
                    }
                />
            </label>
            {move || {
                let (output, running, error) = status_memo.get();
                view! {
                    <div class="modal-form">
                        {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                        {output.map(|output| view! {
                            <div class="field">
                                <span class="field-label">"Output"</span>
                                <pre class="terminal-output">{output}</pre>
                            </div>
                        }.into_any())}
                        <div class="hint">
                            {if running {
                                "Running. You can close this dialog; the result lands in the transcript."
                            } else {
                                "Press Enter to run. Uses the same sandbox, environment, and timeout guards as the TUI."
                            }}
                        </div>
                    </div>
                }
            }}
        </div>
    }
    .into_any();
    let actions = view! {
        <button
            class="btn primary"
            disabled=move || status_memo.get().1
            on:click=move |_| {
                let command = store.with(|s| match &s.modal {
                    Some(ActiveModal::Terminal { command, .. }) => command.text.clone(),
                    _ => String::new(),
                });
                if !command.trim().is_empty() {
                    store.update(|s| crate::state::execute_user_shell(s, command.trim()));
                }
            }
        >"Run"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
    }
    .into_any();
    shell("Terminal", store, body, actions)
}

#[component]
pub(super) fn QueuedEditModal(store: StoreHandle) -> impl IntoView {
    let (index, initial) = store.with(|s| match &s.modal {
        Some(ActiveModal::QueuedEdit { index, input, .. }) => (*index, input.text.clone()),
        _ => (0, String::new()),
    });
    let error = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::QueuedEdit { error, .. }) => error.clone(),
            _ => None,
        })
    });
    let body = view! {
        <div class="modal-form">
            <label class="field">
                <span class="field-label">{format!("Queued message {}", index + 1)}</span>
                <textarea
                    class="input"
                    rows="4"
                    placeholder="Queued prompt"
                    prop:value=initial
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        store.update(|s| {
                            if let Some(ActiveModal::QueuedEdit { input, error, .. }) = &mut s.modal {
                                input.replace_text(value);
                                *error = None;
                            }
                        });
                    }
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                            ev.prevent_default();
                            store.update(crate::state::commit_queued_edit);
                        }
                    }
                ></textarea>
            </label>
            {move || error.get().map(|msg| view! { <div class="banner error">{msg}</div> }.into_any())}
            <div class="hint">"Press Enter to save (Shift+Enter for newline), or Esc to discard."</div>
        </div>
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
        <button class="btn primary" on:click=move |_| {
            store.update(crate::state::commit_queued_edit);
        }>"Save"</button>
    }
    .into_any();
    shell("Edit queued message", store, body, actions)
}

#[component]
pub(super) fn RewindModal(store: StoreHandle) -> impl IntoView {
    let rewind_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::RewindMenu {
                turns,
                selected_turn,
                scope,
            }) => (turns.clone(), *selected_turn, *scope),
            _ => (Vec::new(), 0, RewindScopeDto::Both),
        })
    });
    let body = view! {
        {move || {
            let (turns, selected_turn, scope) = rewind_state.get();
            let rows = turns
                .iter()
                .enumerate()
                .map(|(index, (turn_id, label))| {
                    let turn = turn_id.clone();
                    let is_selected = index == selected_turn;
                    view! {
                        <button class="pick-row" class:active=is_selected on:click=move |_| {
                            let turn = turn.clone();
                            store.update(|s| {
                                if let Some(ActiveModal::RewindMenu { turns, selected_turn, .. }) = &mut s.modal
                                    && let Some(index) = turns.iter().position(|(id, _)| id == &turn)
                                {
                                    *selected_turn = index;
                                }
                            });
                        }>
                            <span class="pick-title">{label.clone()}</span>
                            <span class="pick-meta">{turn_id.clone()}</span>
                        </button>
                    }
                    .into_any()
                })
                .collect::<Vec<_>>();
            view! {
                <div class="modal-form">
                    <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                    <div class="choice-grid">
                        <button class="choice" class:active=scope == RewindScopeDto::Both on:click=move |_| set_scope(&store, RewindScopeDto::Both)>
                            <span class="choice-title">"Code + conversation"</span>
                        </button>
                        <button class="choice" class:active=scope == RewindScopeDto::Code on:click=move |_| set_scope(&store, RewindScopeDto::Code)>
                            <span class="choice-title">"Code only"</span>
                        </button>
                        <button class="choice" class:active=scope == RewindScopeDto::Conversation on:click=move |_| set_scope(&store, RewindScopeDto::Conversation)>
                            <span class="choice-title">"Conversation only"</span>
                        </button>
                    </div>
                </div>
            }
        }}
    }
    .into_any();
    let actions = view! {
        <button class="btn primary" on:click=move |_| {
            let target = store.with(|s| match &s.modal {
                Some(ActiveModal::RewindMenu { turns, selected_turn, scope }) => {
                    turns.get(*selected_turn).map(|(id, _)| (id.clone(), *scope))
                }
                _ => None,
            });
            if let Some((turn_id, scope)) = target
                && let (Some(session_id), Some(t)) = (store.with(|s| s.active_session_id.clone()), crate::transport::current())
            {
                t.rewind_session(session_id, turn_id, scope);
            }
            store.update(|s| s.modal = None);
        }>"Rewind"</button>
        <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Cancel"</button>
    }
    .into_any();
    shell("Rewind", store, body, actions)
}

#[component]
pub(super) fn SubagentModal(store: StoreHandle) -> impl IntoView {
    let sub_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::SubagentOverlay {
                entry_id,
                name,
                agent,
                status,
                child_session_id,
                entries,
                usage,
                loading,
                error,
                ..
            }) => {
                let live_status = s
                    .active_tab()
                    .and_then(|tab| {
                        tab.entries.iter().find_map(|e| match e {
                            meerai_web_protocol::TranscriptEntryDto::Subagent {
                                id,
                                status,
                                ..
                            } if id == entry_id => Some(status.clone()),
                            _ => None,
                        })
                    })
                    .unwrap_or_else(|| status.clone());
                Some((
                    name.clone(),
                    agent.clone(),
                    live_status,
                    child_session_id.clone(),
                    entries.clone(),
                    usage.clone(),
                    *loading,
                    error.clone(),
                ))
            }
            _ => None,
        })
    });

    let child_for_poll = store.with(|s| match &s.modal {
        Some(ActiveModal::SubagentOverlay {
            child_session_id, ..
        }) => child_session_id.clone(),
        _ => None,
    });

    if let Some(child_id) = child_for_poll {
        let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
        let cleanup_ref = interval_handle.clone();
        if let Some(window) = web_sys::window() {
            let cb = Closure::<dyn FnMut()>::new(move || {
                let should_poll = sub_state
                    .get_untracked()
                    .is_some_and(|(_, _, status, _, _, _, _, _)| status == "running");
                if should_poll && let Some(t) = crate::transport::current() {
                    t.fetch_session_events(child_id.clone(), true);
                }
            });
            if let Ok(id) = window.set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                1500,
            ) && let Ok(mut guard) = interval_handle.lock()
            {
                *guard = Some(id);
            }
            cb.forget();
        }
        on_cleanup(move || {
            if let Ok(mut guard) = cleanup_ref.lock()
                && let Some(id) = guard.take()
                && let Some(window) = web_sys::window()
            {
                window.clear_interval_with_handle(id);
            }
        });
    }

    let render_epoch = Memo::new(move |_| {
        store.with(|s| {
            s.active_tab()
                .map(|tab| {
                    (
                        tab.entries_version,
                        tab.expanded_tools.len(),
                        tab.expanded_reasoning.len(),
                        tab.expanded_subagents.len(),
                    )
                })
                .unwrap_or((0, 0, 0, 0))
        })
    });

    let body = view! {
        {move || {
            let _ = render_epoch.get();
            let Some((name, agent, status, child_session_id, entries, usage, loading, error)) = sub_state.get() else {
                return view! { <span></span> }.into_any();
            };
            let refresh_id = child_session_id.clone();
            view! {
                <div class="subagent-view">
                    <div class="subagent-meta">
                        <div class="kv"><span>"Agent"</span><strong>{name.clone().unwrap_or_else(|| agent.clone())}</strong></div>
                        {name.map(|_| view! { <div class="kv"><span>"Preset"</span><strong>{agent.clone()}</strong></div> })}
                        <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
                        {child_session_id.clone().map(|child| view! { <div class="kv"><span>"Child session"</span><code>{child}</code></div> }).into_any()}
                        {usage.map(|usage| view! {
                            <div class="kv"><span>"Usage"</span><strong>{format!(
                                "↑{} ↓{} · Σ{}",
                                format_tokens(usage.prompt_tokens.unwrap_or(0).max(0) as u64),
                                format_tokens(usage.completion_tokens.unwrap_or(0).max(0) as u64),
                                format_tokens(usage.total_tokens.unwrap_or(0).max(0) as u64),
                            )}</strong></div>
                        }.into_any())}
                        {refresh_id.map(|cid| view! {
                            <button class="btn tiny" on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.fetch_session_events(cid.clone(), true);
                                }
                            }>"Refresh"</button>
                        }.into_any())}
                    </div>
                    {error.map(|error| view! { <div class="banner error">{error}</div> }.into_any())}
                    <Show when=move || loading>
                        <div class="hint">"Loading child transcript…"</div>
                    </Show>
                    {if entries.is_empty() && !loading {
                        view! { <div class="hint">"This agent has no conversation yet."</div> }.into_any()
                    } else {
                        crate::views::transcript::render_child_transcript(store, &entries)
                    }}
                </div>
            }
            .into_any()
        }}
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::SubagentOverlay { name, agent, .. }) => match name {
                Some(name) => format!("Agent — {name} ({agent})"),
                None => format!("Agent — {agent}"),
            },
            _ => "Subagent".to_owned(),
        })
    };
    shell_reactive_full(
        title,
        store,
        body,
        view! { <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button> }
            .into_any(),
    )
}

#[component]
pub(super) fn RunsModal(store: StoreHandle) -> impl IntoView {
    let now_ms = RwSignal::new(js_sys::Date::now() as u64);
    let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let interval_cleanup = interval_handle.clone();
    if let Some(window) = web_sys::window() {
        let cb = Closure::<dyn FnMut()>::new(move || now_ms.set(js_sys::Date::now() as u64));
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
    let runs_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Runs { kind, selected }) => {
                let agents = running_subagent_rows(s);
                let shells = s
                    .active_session()
                    .map(|session| session.shell_runs.clone())
                    .unwrap_or_default();
                Some((*kind, *selected, agents, shells))
            }
            _ => None,
        })
    });
    let body = view! {
        {move || {
            let Some((kind, selected, agents, shells)) = runs_state.get() else {
                return view! { <span></span> }.into_any();
            };
            match kind {
                RunKindDto::Agents => {
                    if agents.is_empty() {
                        return view! { <div class="hint">"No running agents"</div> }.into_any();
                    }
                    let rows = agents
                        .into_iter()
                        .enumerate()
                        .map(|(index, row)| {
                            let label = row.name.clone().unwrap_or_else(|| row.agent.clone());
                            let preset = row
                                .name
                                .as_ref()
                                .map(|_| row.agent.clone());
                            view! {
                                <button
                                    class="pick-row"
                                    class:active=index == selected
                                    on:click=move |_| store.update(|s| open_run_row(s, RunKindDto::Agents, index))
                                >
                                    <span class="pick-title">{label}</span>
                                    {preset.map(|preset| view! { <span class="pick-meta">{preset}</span> })}
                                    <span class="pick-meta">{row.status}</span>
                                    <span class="badge muted">{format!("{}/{} steps", row.steps_used, row.max_steps)}</span>
                                </button>
                            }
                            .into_any()
                        })
                        .collect::<Vec<_>>();
                    view! {
                        <div class="modal-form">
                            <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                        </div>
                    }
                    .into_any()
                }
                RunKindDto::Shells => {
                    if shells.is_empty() {
                        return view! { <div class="hint">"No running shells"</div> }.into_any();
                    }
                    let now = now_ms.get();
                    let rows = shells
                        .into_iter()
                        .enumerate()
                        .map(|(index, run)| {
                            let elapsed = format_duration_ms(now.saturating_sub(run.started_at_ms));
                            view! {
                                <button
                                    class="pick-row"
                                    class:active=index == selected
                                    on:click=move |_| store.update(|s| open_run_row(s, RunKindDto::Shells, index))
                                >
                                    <span class="pick-title">{format!("!{}", run.command)}</span>
                                    <span class="pick-meta">{elapsed}</span>
                                </button>
                            }
                            .into_any()
                        })
                        .collect::<Vec<_>>();
                    view! {
                        <div class="modal-form">
                            <div class="pick-list">{AnyView::from(Fragment::new(rows))}</div>
                        </div>
                    }
                    .into_any()
                }
            }
        }}
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Runs { kind, .. }) => match kind {
                RunKindDto::Agents => {
                    format!("Agents ({})", crate::state::running_subagent_count(s))
                }
                RunKindDto::Shells => format!("Shells ({})", crate::state::running_shell_count(s)),
            },
            _ => "Running work".to_owned(),
        })
    };
    shell_reactive(
        title,
        store,
        body,
        view! {
            <button class="btn" on:click=move |_| store.update(|s| s.modal = None)>"Close"</button>
        }
        .into_any(),
    )
}

#[component]
pub(super) fn ShellRunModal(store: StoreHandle) -> impl IntoView {
    let now_ms = RwSignal::new(js_sys::Date::now() as u64);
    let interval_handle: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let interval_cleanup = interval_handle.clone();
    if let Some(window) = web_sys::window() {
        let cb = Closure::<dyn FnMut()>::new(move || now_ms.set(js_sys::Date::now() as u64));
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
    let run_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ShellRun {
                run_id,
                command,
                started_at_ms,
                ..
            }) => {
                let running = s
                    .active_session()
                    .is_some_and(|session| session.shell_runs.iter().any(|run| &run.id == run_id));
                let output = s.shell_outputs.get(run_id).cloned();
                Some((command.clone(), *started_at_ms, running, output))
            }
            _ => None,
        })
    });
    let back_label = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ShellRun {
                from_runs: Some(_), ..
            }) => "Back",
            _ => "Close",
        })
    });
    let body = view! {
        {move || {
            let Some((command, started_at_ms, running, output)) = run_state.get() else {
                return view! { <span></span> }.into_any();
            };
            let status = if running {
                let elapsed = now_ms.get();
                format!(
                    "running · {}",
                    format_duration_ms(elapsed.saturating_sub(started_at_ms))
                )
            } else if matches!(&output, Some(Err(_))) {
                "failed".to_owned()
            } else {
                "finished".to_owned()
            };
            let output_view = match output {
                Some(Ok(out)) => view! {
                    <div class="field">
                        <span class="field-label">"Output"</span>
                        <pre class="terminal-output">{out}</pre>
                    </div>
                }
                .into_any(),
                Some(Err(err)) => view! { <div class="banner error">{err}</div> }.into_any(),
                None if running => {
                    view! { <div class="hint">"waiting for output…"</div> }.into_any()
                }
                None => view! {
                    <div class="hint">"finished; output is in the transcript"</div>
                }
                .into_any(),
            };
            view! {
                <div class="modal-form">
                    <div class="kv"><span>"Command"</span><code>{format!("!{command}")}</code></div>
                    <div class="kv"><span>"Status"</span><strong>{status}</strong></div>
                    {output_view}
                </div>
            }
            .into_any()
        }}
    }
    .into_any();
    let actions = view! {
        <button class="btn" on:click=move |_| {
            store.update(|s| {
                let target = match &s.modal {
                    Some(ActiveModal::ShellRun { from_runs, .. }) => *from_runs,
                    _ => None,
                };
                if let Some(kind) = target {
                    back_to_runs(s, kind);
                } else {
                    s.modal = None;
                }
            });
        }>{move || back_label.get()}</button>
    }
    .into_any();
    let title = move || {
        store.with(|s| match &s.modal {
            Some(ActiveModal::ShellRun { command, .. }) => {
                format!("Shell — !{}", crate::views::truncate(command, 40))
            }
            _ => "Shell".to_owned(),
        })
    };
    shell_reactive(title, store, body, actions)
}
