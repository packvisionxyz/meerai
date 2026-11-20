use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{SessionStatus, TranscriptEntryDto, UnifiedLineKindDto};
use wasm_bindgen::JsCast;

use crate::markdown::render_markdown;
use crate::state::{
    DigestItem, MentionCategory, SessionTabState, TranscriptBlock, activity_summary, activity_verb,
    extract_mentions, group_transcript_blocks, mention_segments, open_rewind_modal,
    set_status_notice, summarize_tool_args, update_completion,
};
use crate::store::StoreHandle;
use crate::text::UiEffect;

#[component]
pub fn Transcript(store: StoreHandle) -> impl IntoView {
    let active_turn_num = RwSignal::new(1usize);
    let show_jump_latest = RwSignal::new(false);

    let committed_key = Memo::new(move |_| {
        store.with(|s| {
            (
                s.active_session_id.clone(),
                s.active_tab().map(|t| t.entries_version).unwrap_or(0),
            )
        })
    });

    let streaming_state = Memo::new(move |_| {
        store.with(|s| {
            let active_tool = s.active_session().and_then(|x| x.active_tool.clone());
            let is_running = s
                .active_session()
                .is_some_and(|x| x.status == SessionStatus::Running);
            s.active_tab().map(|t| {
                (
                    t.streaming_item_id.clone(),
                    t.streaming_text.clone(),
                    t.streaming_reasoning.clone(),
                    active_tool,
                    is_running,
                )
            })
        })
    });

    let pending_inputs = Memo::new(move |_| {
        store.with(|s| {
            s.active_tab()
                .map(|t| {
                    (
                        t.pending_submit.clone(),
                        t.pending_queue.iter().cloned().collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        })
    });

    let rail_turns = Memo::new(move |_| {
        let _ = committed_key.get();
        store.with(|s| {
            let Some(tab) = s.active_tab() else {
                return Vec::new();
            };
            let mut n = 0usize;
            tab.entries
                .iter()
                .filter_map(|entry| match entry {
                    TranscriptEntryDto::User { id, text, .. } => {
                        n += 1;
                        Some((n, id.clone(), truncate_inline(text, 48)))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    });

    let on_scroll = move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Some(container) = document.get_element_by_id("messages") else {
            return;
        };
        let dist = container.scroll_height() - container.scroll_top() - container.client_height();
        show_jump_latest.set(dist > 180);

        let turns = rail_turns.get_untracked();
        if turns.len() < 2 {
            return;
        }
        let container_top = container.get_bounding_client_rect().top();
        let mut current = turns[0].0;
        for (num, id, _) in &turns {
            if let Some(el) = document.get_element_by_id(&format!("turn-{id}")) {
                let top = el.get_bounding_client_rect().top() - container_top;
                if top <= 96.0 {
                    current = *num;
                } else {
                    break;
                }
            }
        }
        if active_turn_num.get_untracked() != current {
            active_turn_num.set(current);
        }
    };

    view! {
        <div class="transcript-shell">
            <section
                id="messages"
                class="transcript"
                role="log"
                aria-live="polite"
                on:scroll=on_scroll
            >
                <div class="transcript-inner conversation-column">
                    {move || {
                        let _ = committed_key.get();
                        render_committed_transcript(store)
                    }}
                    {move || match streaming_state.get() {
                        Some((_, text, reasoning, active_tool, is_running))
                            if !text.is_empty() || !reasoning.is_empty() || is_running =>
                        {
                            render_streaming(&text, &reasoning, active_tool.as_deref(), is_running)
                        }
                        _ => view! { <span></span> }.into_any(),
                    }}
                    {move || {
                        let (sending, queued) = pending_inputs.get();
                        render_pending_inputs(store, sending, queued)
                    }}
                </div>
            </section>
            <Show when=move || { rail_turns.get().len() >= 2 }>
                <nav class="turn-rail" aria-label="Turn navigation">
                    {move || {
                        let turns = rail_turns.get();
                        let current = active_turn_num.get();
                        AnyView::from(Fragment::new(
                            turns
                                .into_iter()
                                .map(|(num, id, preview)| {
                                    let anchor = format!("turn-{id}");
                                    let is_active = num == current;
                                    view! {
                                        <button
                                            class="turn-rail-btn"
                                            class:active=is_active
                                            title=format!("#{num}: {preview}")
                                            on:click=move |_| {
                                                active_turn_num.set(num);
                                                store.effect(UiEffect::ScrollIntoView(anchor.clone()));
                                            }
                                        >
                                            {num}
                                        </button>
                                    }
                                    .into_any()
                                })
                                .collect(),
                        ))
                    }}
                </nav>
            </Show>
            <Show when=move || show_jump_latest.get()>
                <button
                    class="jump-latest"
                    title="Jump to latest output"
                    on:click=move |_| {
                        show_jump_latest.set(false);
                        store.effect(UiEffect::ScrollToBottom("messages".to_owned()));
                    }
                >
                    "↓ Latest"
                </button>
            </Show>
        </div>
    }
}

fn render_committed_transcript(store: StoreHandle) -> AnyView {
    let tab = store.with(|s| s.active_tab().cloned());
    let Some(tab) = tab else {
        return render_empty_state(
            store,
            "No session open",
            "Start a new session or open one from Sessions.",
        );
    };

    let blocks = group_transcript_blocks(&tab.entries);
    if blocks.is_empty()
        && tab.streaming_text.is_empty()
        && tab.streaming_reasoning.is_empty()
        && tab.pending_submit.is_none()
        && tab.pending_queue.is_empty()
        && !store.with(|s| {
            s.active_session()
                .is_some_and(|x| x.status == SessionStatus::Running)
        })
    {
        return render_empty_state(
            store,
            "What should we build?",
            "meerai works in this workspace. Ask for a change, a review, or a fix.",
        );
    }

    let views = blocks
        .into_iter()
        .map(|block| render_block(store, &tab, block))
        .collect::<Vec<_>>();
    AnyView::from(Fragment::new(views))
}

fn render_block(store: StoreHandle, tab: &SessionTabState, block: TranscriptBlock<'_>) -> AnyView {
    match block {
        TranscriptBlock::User {
            turn_num,
            id,
            turn_id,
            text,
        } => {
            let anchor = format!("turn-{id}");
            let host = store.with(|s| s.host_info.clone());
            let copy_text = extract_mentions(text, &host).cleaned;
            let body: Vec<AnyView> = mention_segments(text, &host)
                .into_iter()
                .map(|segment| match segment.category {
                    Some(category) => {
                        let class = match category {
                            MentionCategory::Agent => "def-chip agent",
                            MentionCategory::Skill => "def-chip skill",
                            MentionCategory::File => "def-chip",
                        };
                        view! { <span class=class>{segment.text}</span> }.into_any()
                    }
                    None => view! { <span>{segment.text}</span> }.into_any(),
                })
                .collect();
            let has_turn = turn_id.is_some();
            let turn_rewind = turn_id.map(str::to_owned);
            let turn_fork = turn_id.map(str::to_owned);
            let session_id = store.with(|s| s.active_session_id.clone());
            view! {
                <article class="turn" id=anchor>
                    <div class="turn-head">
                        <span class="turn-num">{format!("#{turn_num}")}</span>
                        <div class="turn-actions">
                            <button class="btn tiny" title="Copy message" on:click=move |_| {
                                crate::store::copy_text(copy_text.clone());
                                store.update(|s| set_status_notice(s, "Copied message"));
                            }>"Copy"</button>
                            <button
                                class="btn tiny"
                                disabled=!has_turn
                                title=if has_turn { "Rewind this turn" } else { "Rewind is available for completed turns" }
                                on:click=move |_| {
                                    if let Some(hint) = turn_rewind.as_deref() {
                                        store.update(|s| open_rewind_modal(s, Some(hint)));
                                    }
                                }
                            >"Rewind"</button>
                            <button
                                class="btn tiny"
                                disabled=!has_turn
                                title=if has_turn { "Fork before this message" } else { "Fork is available for completed turns" }
                                on:click=move |_| {
                                    if let (Some(session), Some(turn)) = (session_id.clone(), turn_fork.clone())
                                        && let Some(t) = crate::transport::current()
                                    {
                                        t.fork_session(session, Some(turn));
                                    }
                                }
                            >"Fork"</button>
                        </div>
                    </div>
                    <div class="msg-label">"You"</div>
                    <div class="user-msg">
                        {AnyView::from(Fragment::new(body))}
                    </div>
                </article>
            }
            .into_any()
        }
        TranscriptBlock::ActivityDigest { digest_id, items } => {
            render_digest(store, tab, digest_id, items)
        }
        TranscriptBlock::Assistant(entry) => render_assistant(store, entry),
        TranscriptBlock::System(entry) => {
            let text = match entry {
                TranscriptEntryDto::System { text, .. } => text.clone(),
                _ => String::new(),
            };
            view! { <div class="system-line">{text}</div> }.into_any()
        }
        TranscriptBlock::Error(entry) => {
            let message = match entry {
                TranscriptEntryDto::Error { message, .. } => message.clone(),
                _ => String::new(),
            };
            view! {
                <div class="error-block" role="alert">
                    <span class="error-label">"Error"</span>
                    <span>{message}</span>
                </div>
            }
            .into_any()
        }
        TranscriptBlock::Question(entry) => render_question(entry),
    }
}

fn render_digest(
    store: StoreHandle,
    tab: &SessionTabState,
    digest_id: String,
    items: Vec<DigestItem<'_>>,
) -> AnyView {
    let expanded = tab.expanded_digests.contains(&digest_id);
    let toggle_id = digest_id.clone();
    let mut pairs: Vec<(&str, bool)> = Vec::new();
    let mut thoughts = 0usize;
    for item in &items {
        match item {
            DigestItem::ToolCall(TranscriptEntryDto::ToolCall { name, status, .. }) => {
                pairs.push((name.as_str(), status == "running"));
            }
            DigestItem::Subagent(TranscriptEntryDto::Subagent { status, .. }) => {
                pairs.push(("subagent-run", status == "running"));
            }
            DigestItem::Reasoning(_) => thoughts += 1,
            _ => {}
        }
    }
    let has_running = pairs.iter().any(|(_, running)| *running);
    let summary = activity_summary(&pairs, thoughts);
    let summary = if summary.is_empty() {
        "Activity".to_owned()
    } else {
        summary
    };
    let body = if expanded {
        let rows = items
            .iter()
            .map(|item| match item {
                DigestItem::ToolCall(entry) => render_tool_row(store, tab, entry),
                DigestItem::Subagent(entry) => render_subagent(store, tab, entry),
                DigestItem::Reasoning(entry) => render_reasoning(store, tab, entry, "Thought"),
            })
            .collect::<Vec<_>>();
        view! { <div class="digest-body">{AnyView::from(Fragment::new(rows))}</div> }.into_any()
    } else {
        view! { <div></div> }.into_any()
    };
    view! {
        <div class="digest" class:running=has_running>
            <button class="digest-head" aria-expanded=expanded on:click=move |_| {
                let key = toggle_id.clone();
                store.update(|s| {
                    if let Some(tab) = s.active_tab_mut() {
                        if tab.expanded_digests.contains(&key) {
                            tab.expanded_digests.remove(&key);
                        } else {
                            tab.expanded_digests.insert(key);
                        }
                        tab.entries_version = tab.entries_version.wrapping_add(1);
                    }
                });
            }>
                <span class="digest-caret">{if expanded { "▾" } else { "▸" }}</span>
                {has_running.then(|| view! {
                    <span class="working-dots" aria-hidden="true"><span></span><span></span><span></span></span>
                })}
                <span>{summary}</span>
            </button>
            {body}
        </div>
    }
    .into_any()
}

fn render_tool_row(
    store: StoreHandle,
    tab: &SessionTabState,
    entry: &TranscriptEntryDto,
) -> AnyView {
    let TranscriptEntryDto::ToolCall {
        id,
        name,
        args,
        status,
        output,
        edit_diff,
    } = entry
    else {
        return view! { <div></div> }.into_any();
    };
    let expanded = tab.expanded_tools.contains(id);
    let toggle_id = id.clone();
    let output_copy = output.clone();
    let output = output.clone();
    let edit_diff = edit_diff.clone();
    let summarized = summarize_tool_args(name, args);
    let args_text = if summarized.is_empty() {
        String::new()
    } else {
        truncate_inline(&summarized, 140)
    };
    let verb = activity_verb(name);
    let details = if expanded {
        let mut parts = Vec::new();
        if !args.is_empty() {
            parts.push(
                view! {
                    <div class="tool-block">
                        <div class="tool-label">"Arguments"</div>
                        <pre>{args.clone()}</pre>
                    </div>
                }
                .into_any(),
            );
        }
        if let Some(output) = output {
            let copy_val = output_copy.unwrap_or_default();
            parts.push(
                view! {
                    <div class="tool-block">
                        <div class="tool-block-head">
                            <span class="tool-label">"Output"</span>
                            <button class="btn tiny" on:click=move |_| {
                                crate::store::copy_text(copy_val.clone());
                                store.update(|s| set_status_notice(s, "Copied tool output"));
                            }>"Copy"</button>
                        </div>
                        <pre>{output}</pre>
                    </div>
                }
                .into_any(),
            );
        }
        if let Some(lines) = edit_diff {
            parts.push(render_edit_diff(&lines));
        }
        view! { <div class="tool-details">{AnyView::from(Fragment::new(parts))}</div> }.into_any()
    } else {
        view! { <div></div> }.into_any()
    };
    view! {
        <div class="tool-row">
            <button class="tool-head" aria-expanded=expanded on:click=move |_| {
                let key = toggle_id.clone();
                store.update(|s| {
                    if let Some(tab) = s.active_tab_mut() {
                        if tab.expanded_tools.contains(&key) {
                            tab.expanded_tools.remove(&key);
                        } else {
                            tab.expanded_tools.insert(key);
                        }
                        tab.entries_version = tab.entries_version.wrapping_add(1);
                    }
                });
            }>
                <span class=format!("tool-status {}", tool_status_class(status))></span>
                <span class="tool-verb">{verb}</span>
                <span class="tool-name">{name.clone()}</span>
                <span class="tool-args">{args_text}</span>
                <span class="tool-state-label">{status.clone()}</span>
            </button>
            {details}
        </div>
    }
    .into_any()
}

fn render_edit_diff(lines: &[meerai_web_protocol::UnifiedLineDto]) -> AnyView {
    let rows = lines
        .iter()
        .map(|line| {
            let class = match line.kind {
                UnifiedLineKindDto::FileHeader => "diff-file",
                UnifiedLineKindDto::HunkHeader => "diff-hunk",
                UnifiedLineKindDto::Context => "diff-context",
                UnifiedLineKindDto::Added => "diff-added",
                UnifiedLineKindDto::Removed => "diff-removed",
            };
            view! { <div class=format!("diff-line {class}")>{line.text.clone()}</div> }.into_any()
        })
        .collect::<Vec<_>>();
    view! {
        <div class="tool-block">
            <div class="tool-label">"Edit"</div>
            <div class="diff-inline">{AnyView::from(Fragment::new(rows))}</div>
        </div>
    }
    .into_any()
}

fn render_assistant(store: StoreHandle, entry: &TranscriptEntryDto) -> AnyView {
    let TranscriptEntryDto::Assistant { text, .. } = entry else {
        return view! { <div></div> }.into_any();
    };
    if text.trim().is_empty() {
        return view! { <div></div> }.into_any();
    }
    let copy_text = text.clone();
    let body = render_markdown(text);
    view! {
        <div class="assistant">
            <div class="assistant-head">
                <div class="msg-label assistant-label"><span class="assistant-dot"></span>"meerai"</div>
                <button
                    class="btn tiny assistant-copy"
                    title="Copy reply markdown"
                    on:click=move |_| {
                        crate::store::copy_text(copy_text.clone());
                        store.update(|s| set_status_notice(s, "Copied reply"));
                    }
                >"Copy"</button>
            </div>
            {body}
        </div>
    }
    .into_any()
}

fn render_reasoning(
    store: StoreHandle,
    tab: &SessionTabState,
    entry: &TranscriptEntryDto,
    label: &str,
) -> AnyView {
    let TranscriptEntryDto::Assistant { id, reasoning, .. } = entry else {
        return view! { <div></div> }.into_any();
    };
    if reasoning.trim().is_empty() {
        return view! { <div></div> }.into_any();
    }
    let is_open = tab.expanded_reasoning.contains(id);
    let key = id.clone();
    view! {
        <details
            class="reasoning"
            open=is_open
            on:toggle=move |ev| {
                let Some(target) = ev.target() else {
                    return;
                };
                if let Ok(details) = target.dyn_into::<web_sys::HtmlDetailsElement>() {
                    let open = details.open();
                    let id = key.clone();
                    store.update(|s| {
                        if let Some(tab) = s.active_tab_mut() {
                            if open {
                                tab.expanded_reasoning.insert(id);
                            } else {
                                tab.expanded_reasoning.remove(&id);
                            }
                        }
                    });
                }
            }
        >
            <summary>{label}</summary>
            <pre>{reasoning.clone()}</pre>
        </details>
    }
    .into_any()
}

fn render_streaming(
    streaming_text: &str,
    streaming_reasoning: &str,
    active_tool: Option<&str>,
    is_running: bool,
) -> AnyView {
    let reasoning = if streaming_reasoning.trim().is_empty() {
        view! { <div></div> }.into_any()
    } else {
        view! {
            <details class="reasoning" open=true>
                <summary>"Reasoning"</summary>
                <pre>{streaming_reasoning.to_owned()}</pre>
            </details>
        }
        .into_any()
    };
    let body = if streaming_text.is_empty() {
        let label = match active_tool {
            Some(tool) => format!("{} {tool}…", activity_verb(tool)),
            None if is_running => "Thinking…".to_owned(),
            None => "…".to_owned(),
        };
        view! {
            <div class="streaming-placeholder">
                <span class="working-dots" aria-hidden="true"><span></span><span></span><span></span></span>
                <span>{label}</span>
            </div>
        }
        .into_any()
    } else {
        render_markdown(streaming_text)
    };
    view! {
        <div class="assistant streaming">
            <div class="msg-label assistant-label">
                <span class="assistant-dot"></span>
                "meerai"
                <span class="streaming-tag">
                    <span class="live-dot" aria-hidden="true"></span>
                    "live"
                </span>
            </div>
            {reasoning}
            {body}
            <span class="cursor"></span>
        </div>
    }
    .into_any()
}

fn render_pending_inputs(
    store: StoreHandle,
    sending: Option<String>,
    queued: Vec<String>,
) -> AnyView {
    if sending.is_none() && queued.is_empty() {
        return view! { <span></span> }.into_any();
    }
    let mut cards = Vec::new();
    if let Some(text) = sending {
        cards.push(
            view! {
                <article class="turn pending">
                    <div class="turn-head">
                        <span class="pending-badge">
                            <span class="working-dots" aria-hidden="true"><span></span><span></span><span></span></span>
                            "Sending"
                        </span>
                    </div>
                    <div class="msg-label">"You"</div>
                    <div class="user-msg pending-msg">{text}</div>
                </article>
            }
            .into_any(),
        );
    }
    for (index, prompt) in queued.into_iter().enumerate() {
        let steer_index = index;
        let drop_index = index;
        let edit_index = index;
        cards.push(
            view! {
                <article class="turn pending">
                    <div class="turn-head">
                        <span class="pending-badge queued">"Pending · Queued"</span>
                        <div class="turn-pending-actions">
                            <button
                                class="btn tiny"
                                title="Steer this prompt now (Alt+G)"
                                on:click=move |_| store.update(|s| crate::state::steer_or_send_queued(s, steer_index))
                            >"Steer"</button>
                            <button
                                class="btn tiny"
                                title="Edit this queued prompt"
                                on:click=move |_| store.update(|s| crate::state::open_queued_edit(s, edit_index))
                            >"Edit"</button>
                            <button
                                class="btn tiny"
                                title="Remove from queue"
                                on:click=move |_| store.update(|s| crate::state::remove_queued(s, drop_index))
                            >"Remove"</button>
                        </div>
                    </div>
                    <div class="msg-label">"You"</div>
                    <div class="user-msg pending-msg">{prompt}</div>
                </article>
            }
            .into_any(),
        );
    }
    AnyView::from(Fragment::new(cards))
}

fn render_subagent(
    store: StoreHandle,
    tab: &SessionTabState,
    entry: &TranscriptEntryDto,
) -> AnyView {
    let TranscriptEntryDto::Subagent {
        id,
        agent,
        status,
        report,
        child_session_id,
        model,
        approval_mode,
        steps_used,
        max_steps,
        usage,
        est_cost_usd,
        active_tool,
    } = entry
    else {
        return view! { <div></div> }.into_any();
    };
    let cost = est_cost_usd.or(usage.est_cost_usd);
    let telemetry = format!(
        "{} · {} · {}/{} steps · ↑{} ↓{} · {} · {}",
        model.as_deref().unwrap_or("parent model"),
        approval_mode,
        steps_used,
        max_steps,
        usage.prompt_tokens.unwrap_or(0),
        usage.completion_tokens.unwrap_or(0),
        cost.map_or_else(|| "cost n/a".to_string(), |cost| format!("${cost:.4}")),
        active_tool.as_deref().unwrap_or("idle")
    );
    let child = child_session_id
        .as_ref()
        .and_then(|child| tab.child_sessions.get(child));
    let child_key = format!("child:{id}");
    let child_open = tab.expanded_subagents.contains(&child_key);
    let child_toggle_id = child_key.clone();
    let child_view = child
        .map(|entries| {
            let lines = entries
                .iter()
                .filter_map(|entry| match entry {
                    TranscriptEntryDto::Assistant { text, .. } if !text.trim().is_empty() => {
                        Some(view! { <div class="child-line">{text.clone()}</div> }.into_any())
                    }
                    TranscriptEntryDto::ToolCall { name, status, .. } => Some(
                        view! { <div class="child-line tool">{format!("{name} · {status}")}</div> }
                            .into_any(),
                    ),
                    TranscriptEntryDto::System { text, .. } => Some(
                        view! { <div class="child-line system">{text.clone()}</div> }.into_any(),
                    ),
                    TranscriptEntryDto::Error { message, .. } => Some(
                        view! { <div class="child-line error">{message.clone()}</div> }.into_any(),
                    ),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let count = lines.len();
            if count == 0 {
                return view! { <span></span> }.into_any();
            }
            view! {
                <details
                    class="subagent-messages"
                    open=child_open
                    on:toggle=move |ev| {
                        let Some(target) = ev.target() else {
                            return;
                        };
                        if let Ok(details) = target.dyn_into::<web_sys::HtmlDetailsElement>() {
                            let open = details.open();
                            let key = child_toggle_id.clone();
                            store.update(|s| {
                                if let Some(tab) = s.active_tab_mut() {
                                    if open {
                                        tab.expanded_subagents.insert(key);
                                    } else {
                                        tab.expanded_subagents.remove(&key);
                                    }
                                }
                            });
                        }
                    }
                >
                    <summary>{format!("Messages ({count})")}</summary>
                    <div class="child-transcript">{AnyView::from(Fragment::new(lines))}</div>
                </details>
            }
            .into_any()
        })
        .unwrap_or_else(|| view! { <span></span> }.into_any());
    let report_open = tab.expanded_subagents.contains(id);
    let report_id = id.clone();
    let report = report.clone().map(|report| {
        view! {
            <details
                class="subagent-report"
                open=report_open
                on:toggle=move |ev| {
                    let Some(target) = ev.target() else {
                        return;
                    };
                    if let Ok(details) = target.dyn_into::<web_sys::HtmlDetailsElement>() {
                        let open = details.open();
                        let id = report_id.clone();
                        store.update(|s| {
                            if let Some(tab) = s.active_tab_mut() {
                                if open {
                                    tab.expanded_subagents.insert(id);
                                } else {
                                    tab.expanded_subagents.remove(&id);
                                }
                            }
                        });
                    }
                }
            >
                <summary>"Report"</summary>
                <div class="subagent-report-body">{render_markdown(&report)}</div>
            </details>
        }
        .into_any()
    });
    let report = report.unwrap_or_else(|| view! { <span></span> }.into_any());
    let entry_id = id.clone();
    let agent_name = agent.clone();
    let status_text = status.clone();
    let child_id = child_session_id.clone();
    view! {
        <div class="subagent">
            <div class="subagent-head">
                <span class=format!("tab-dot {}", if status == "running" { "running" } else { "idle" })></span>
                <span class="subagent-name">{agent.clone()}</span>
                <span class="subagent-status">{status.clone()}</span>
                <span class="subagent-telemetry">{telemetry.clone()}</span>
                <button class="btn tiny" on:click=move |_| {
                    let child = child_id.clone();
                    store.update(|s| {
                        s.modal = Some(crate::state::ActiveModal::SubagentOverlay {
                            entry_id: entry_id.clone(),
                            agent: agent_name.clone(),
                            status: status_text.clone(),
                            child_session_id: child.clone(),
                            entries: Vec::new(),
                            usage: None,
                            loading: child.is_some(),
                            error: None,
                        });
                        if let Some(child) = child
                            && let Some(t) = crate::transport::current()
                        {
                            t.fetch_session_events(child, true);
                        }
                    });
                }>"Inspect"</button>
            </div>
            {report}
            {child_view}
        </div>
    }
    .into_any()
}

fn render_question(entry: &TranscriptEntryDto) -> AnyView {
    let TranscriptEntryDto::Question {
        questions,
        answers,
        resolved,
        ..
    } = entry
    else {
        return view! { <div></div> }.into_any();
    };
    let answer_text = answers.as_ref().map(|answers| {
        answers
            .iter()
            .map(|answer| format!("{}: {}", answer.header, answer.selected.join(", ")))
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let rows = questions
        .iter()
        .map(|question| {
            view! {
                <div class="question-line">
                    <div class="question-head">{question.header.clone()}</div>
                    <div class="question-text">{question.question.clone()}</div>
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    view! {
        <div class="question-card">
            <div class="question-title">
                {if *resolved { "Question" } else { "Waiting for an answer" }}
            </div>
            {AnyView::from(Fragment::new(rows))}
            {answer_text.map(|text| view! { <div class="question-answer">{text}</div> }.into_any())}
        </div>
    }
    .into_any()
}

fn render_empty_state(store: StoreHandle, title: &str, description: &str) -> AnyView {
    let suggestions = [
        (
            "Explain this repository",
            "Summarize the architecture, crates, and main flows.",
            "Explain this repository: the crates, the main flows, and where to start.",
        ),
        (
            "Fix a failing test",
            "Find the failing test, fix it, and verify the result.",
            "Find the failing test, fix it, and verify the result.",
        ),
        (
            "Review my changes",
            "Review the working-tree diff for bugs, risks, and missing tests.",
            "Review the working-tree diff for bugs, risks, and missing tests.",
        ),
    ];
    let rows = suggestions
        .into_iter()
        .map(|(title, description, prompt)| {
            let prompt = prompt.to_owned();
            view! {
                <button class="suggestion" on:click=move |_| {
                    let prompt = prompt.clone();
                    store.update(|s| {
                        s.draft.replace_text(prompt);
                        update_completion(s);
                    });
                    store.effect(UiEffect::Focus("composer".to_owned()));
                }>
                    <span class="suggestion-title">{title}</span>
                    <span class="suggestion-desc">{description}</span>
                </button>
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    view! {
        <div class="empty-state">
            <div class="empty-mark"><crate::views::BrandMark /></div>
            <h2>{title.to_owned()}</h2>
            <p>{description.to_owned()}</p>
            <div class="suggestions">{AnyView::from(Fragment::new(rows))}</div>
            <div class="empty-hint">
                <kbd>"Enter"</kbd>" to send · " <kbd>"Shift"</kbd>" + " <kbd>"Enter"</kbd>" for a new line · "
                <kbd>"Alt+K"</kbd>" command palette"
            </div>
        </div>
    }
    .into_any()
}

fn tool_status_class(status: &str) -> &'static str {
    match status {
        "running" => "running",
        "failed" | "error" => "error",
        "cancelled" => "cancelled",
        _ => "done",
    }
}

fn truncate_inline(text: &str, max: usize) -> String {
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

pub fn session_status_is_waiting(status: SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::WaitingApproval | SessionStatus::WaitingQuestion
    )
}
