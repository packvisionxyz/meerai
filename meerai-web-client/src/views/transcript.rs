use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{SessionStatus, TranscriptEntryDto, UnifiedLineKindDto};

use crate::markdown::render_markdown;
use crate::state::{
    SessionTabState, TranscriptBlock, activity_summary, group_transcript_blocks, open_rewind_modal,
    update_completion,
};
use crate::store::StoreHandle;
use crate::text::UiEffect;

#[component]
pub fn Transcript(store: StoreHandle) -> impl IntoView {
    view! {
        <section id="messages" class="transcript" role="log" aria-live="polite">
            <div class="transcript-inner">{move || render_transcript(store)}</div>
        </section>
    }
}

pub fn render_transcript(store: StoreHandle) -> AnyView {
    let tab = store.with(|s| s.active_tab().cloned());
    let Some(tab) = tab else {
        return render_empty_state(
            store,
            "No session open",
            "Start a new session or open one from Sessions.",
        );
    };

    let blocks = group_transcript_blocks(&tab.entries);
    let mut views = Vec::new();
    for block in blocks {
        views.push(render_block(store.clone(), &tab, block));
    }

    if !tab.streaming_text.is_empty() || !tab.streaming_reasoning.is_empty() {
        views.push(render_streaming(&tab));
    }

    if views.is_empty() {
        views.push(render_empty_state(
            store,
            "What should we build?",
            "meerai works in this workspace. Ask for a change, a review, or a fix.",
        ));
    }

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
            let copy_text = text.to_owned();
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
                            }>"Copy"</button>
                            <button class="btn tiny" title="Rewind this turn" on:click=move |_| {
                                let hint = turn_rewind.clone();
                                store.update(|s| open_rewind_modal(s, hint.as_deref()));
                            }>"Rewind"</button>
                            <button class="btn tiny" title="Fork before this message" on:click=move |_| {
                                if let (Some(session), Some(turn)) = (session_id.clone(), turn_fork.clone())
                                    && let Some(t) = crate::transport::current()
                                {
                                    t.fork_session(session, Some(turn));
                                }
                            }>"Fork"</button>
                        </div>
                    </div>
                    <div class="msg-label">"You"</div>
                    <div class="user-msg">{text.to_owned()}</div>
                </article>
            }
            .into_any()
        }
        TranscriptBlock::ActivityDigest {
            digest_id,
            tools,
            reasoning,
        } => render_digest(store, tab, digest_id, tools, reasoning),
        TranscriptBlock::Assistant(entry) => render_assistant(entry),
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
        TranscriptBlock::Subagent(entry) => render_subagent(store, tab, entry),
        TranscriptBlock::Question(entry) => render_question(entry),
    }
}

fn render_digest(
    store: StoreHandle,
    tab: &SessionTabState,
    digest_id: String,
    tools: Vec<&TranscriptEntryDto>,
    reasoning: Vec<&TranscriptEntryDto>,
) -> AnyView {
    let expanded = tab.expanded_digests.contains(&digest_id);
    let toggle_id = digest_id.clone();
    let pairs: Vec<(&str, bool)> = tools
        .iter()
        .map(|entry| match entry {
            TranscriptEntryDto::ToolCall { name, status, .. } => {
                (name.as_str(), status == "running")
            }
            _ => ("tool", false),
        })
        .collect();
    let summary = activity_summary(&pairs, reasoning.len());
    let summary = if summary.is_empty() {
        "Activity".to_owned()
    } else {
        summary
    };
    let body = if expanded {
        let mut rows = Vec::new();
        for entry in &tools {
            rows.push(render_tool_row(store.clone(), tab, entry));
        }
        for entry in &reasoning {
            rows.push(render_reasoning(entry, "Thought"));
        }
        view! { <div class="digest-body">{AnyView::from(Fragment::new(rows))}</div> }.into_any()
    } else {
        view! { <div></div> }.into_any()
    };
    view! {
        <div class="digest">
            <button class="digest-head" aria-expanded=expanded on:click=move |_| {
                let key = toggle_id.clone();
                store.update(|s| {
                    if let Some(tab) = s.active_tab_mut() {
                        if tab.expanded_digests.contains(&key) {
                            tab.expanded_digests.remove(&key);
                        } else {
                            tab.expanded_digests.insert(key);
                        }
                    }
                });
            }>
                <span class="digest-caret">{if expanded { "▾" } else { "▸" }}</span>
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
    let output = output.clone();
    let edit_diff = edit_diff.clone();
    let args_text = if args.is_empty() {
        String::new()
    } else {
        truncate_inline(args, 180)
    };
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
            parts.push(
                view! {
                    <div class="tool-block">
                        <div class="tool-label">"Output"</div>
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
                    }
                });
            }>
                <span class=format!("tool-status {}", tool_status_class(status))></span>
                <span class="tool-name">{name.clone()}</span>
                <span class="tool-args">{args_text}</span>
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

fn render_assistant(entry: &TranscriptEntryDto) -> AnyView {
    let TranscriptEntryDto::Assistant {
        text, reasoning, ..
    } = entry
    else {
        return view! { <div></div> }.into_any();
    };
    if text.trim().is_empty() && reasoning.trim().is_empty() {
        return view! { <div></div> }.into_any();
    }
    let body = if text.trim().is_empty() {
        view! { <div></div> }.into_any()
    } else {
        render_markdown(text)
    };
    let reasoning = if reasoning.trim().is_empty() {
        view! { <div></div> }.into_any()
    } else {
        view! {
            <details class="reasoning">
                <summary>"Reasoning"</summary>
                <pre>{reasoning.clone()}</pre>
            </details>
        }
        .into_any()
    };
    view! {
        <div class="assistant">
            <div class="msg-label assistant-label"><span class="assistant-dot"></span>"meerai"</div>
            {reasoning}
            {body}
        </div>
    }
    .into_any()
}

fn render_reasoning(entry: &TranscriptEntryDto, label: &str) -> AnyView {
    let TranscriptEntryDto::Assistant { reasoning, .. } = entry else {
        return view! { <div></div> }.into_any();
    };
    if reasoning.trim().is_empty() {
        return view! { <div></div> }.into_any();
    }
    view! {
        <details class="reasoning">
            <summary>{label}</summary>
            <pre>{reasoning.clone()}</pre>
        </details>
    }
    .into_any()
}

fn render_streaming(tab: &SessionTabState) -> AnyView {
    let reasoning = if tab.streaming_reasoning.trim().is_empty() {
        view! { <div></div> }.into_any()
    } else {
        view! {
            <details class="reasoning" open=true>
                <summary>"Reasoning"</summary>
                <pre>{tab.streaming_reasoning.clone()}</pre>
            </details>
        }
        .into_any()
    };
    let body = if tab.streaming_text.is_empty() {
        view! { <div class="streaming-placeholder">"…"</div> }.into_any()
    } else {
        render_markdown(&tab.streaming_text)
    };
    view! {
        <div class="assistant streaming">
            <div class="msg-label assistant-label"><span class="assistant-dot"></span>"meerai"</div>
            {reasoning}
            {body}
            <span class="cursor"></span>
        </div>
    }
    .into_any()
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
    } = entry
    else {
        return view! { <div></div> }.into_any();
    };
    let child = child_session_id
        .as_ref()
        .and_then(|child| tab.child_sessions.get(child));
    let child_view = child.map(|entries| {
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
                TranscriptEntryDto::System { text, .. } => {
                    Some(view! { <div class="child-line system">{text.clone()}</div> }.into_any())
                }
                TranscriptEntryDto::Error { message, .. } => {
                    Some(view! { <div class="child-line error">{message.clone()}</div> }.into_any())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        view! { <div class="child-transcript">{AnyView::from(Fragment::new(lines))}</div> }
            .into_any()
    });
    let child_view = child_view.unwrap_or_else(|| view! { <div></div> }.into_any());
    let report = report.clone().map(|report| {
        view! {
            <details class="subagent-report">
                <summary>"Report"</summary>
                <pre>{report}</pre>
            </details>
        }
        .into_any()
    });
    let report = report.unwrap_or_else(|| view! { <div></div> }.into_any());
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
                }>"Open"</button>
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
            <div class="empty-mark">"✦"</div>
            <h2>{title.to_owned()}</h2>
            <p>{description.to_owned()}</p>
            <div class="suggestions">{AnyView::from(Fragment::new(rows))}</div>
            <div class="empty-hint">
                <kbd>"Enter"</kbd>" to send · " <kbd>"Shift"</kbd>" + " <kbd>"Enter"</kbd>" for a new line"
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
