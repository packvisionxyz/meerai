use super::*;

#[component]
pub(super) fn DiffModal(store: StoreHandle) -> impl IntoView {
    let diff_state = Memo::new(move |_| {
        store.with(|s| {
            let files = s.file_changes.clone();
            match &s.modal {
                Some(ActiveModal::DiffView {
                    selected_path,
                    unified,
                    diff,
                    loading,
                    error,
                }) => (
                    files,
                    selected_path.clone(),
                    *unified,
                    diff.clone(),
                    *loading,
                    error.clone(),
                ),
                _ => (files, None, true, None, false, None),
            }
        })
    });
    let body = view! {
        {move || {
            let (files, selected_path, unified, diff, loading, error) = diff_state.get();
            diff_body(store, files, selected_path, unified, diff, loading, error)
        }}
    }
    .into_any();
    shell_wide("Diff", store, body, empty_actions())
}

#[component]
pub(super) fn SearchModal(store: StoreHandle) -> impl IntoView {
    let search_state = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Search {
                results,
                selected,
                preview,
                loading,
                ..
            }) => Some((results.clone(), *selected, preview.clone(), *loading)),
            _ => None,
        })
    });
    let mode = Memo::new(move |_| {
        store.with(|s| match &s.modal {
            Some(ActiveModal::Search { mode, .. }) => *mode,
            _ => SearchModeDto::Content,
        })
    });
    let has_query = Memo::new(move |_| {
        store.with(|s| {
            matches!(&s.modal,
                Some(ActiveModal::Search { query, .. }) if !query.text.trim().is_empty()
            )
        })
    });
    let initial_query = store.with(|s| match &s.modal {
        Some(ActiveModal::Search { query, .. }) => query.text.clone(),
        _ => String::new(),
    });
    let debounce_id: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    let debounce_cleanup = debounce_id.clone();
    on_cleanup(move || {
        if let Ok(mut guard) = debounce_cleanup.lock()
            && let Some(id) = guard.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(id);
        }
    });

    let mode_buttons = [
        (SearchModeDto::Content, "File contents"),
        (SearchModeDto::Files, "File names"),
    ]
    .into_iter()
    .map(|(candidate, label)| {
        let timer = debounce_id.clone();
        view! {
            <button type="button" class="search-mode"
                class:active=move || mode.get() == candidate
                aria-pressed=move || (mode.get() == candidate).to_string()
                on:click=move |_| if mode.get_untracked() != candidate {
                    clear_search_timer(&timer);
                    switch_search_mode(store, candidate);
                }
            >{label}</button>
        }
    })
    .collect::<Vec<_>>();
    let submit_timer = debounce_id.clone();
    let body = view! {
        <div class="search-form">
            <div class="search-modes" role="group" aria-label="Search scope">{mode_buttons}</div>
            <label class="field-label" for="workspace-query">"Search query"</label>
            <form class="search-bar" on:submit=move |ev| {
                ev.prevent_default();
                clear_search_timer(&submit_timer);
                switch_search_mode(store, mode.get_untracked());
            }>
                <input
                    class="input"
                    id="workspace-query"
                    autocomplete="off"
                    placeholder=move || if mode.get() == SearchModeDto::Files { "e.g. Cargo.toml" } else { "e.g. SessionStatus" }
                    prop:value=initial_query
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        let trimmed = value.trim().to_owned();
                        let mode = store.with(|s| match &s.modal {
                            Some(ActiveModal::Search { mode, .. }) => *mode,
                            _ => SearchModeDto::Content,
                        });
                        store.update(|s| {
                            if let Some(ActiveModal::Search { query, loading, results, preview, selected, .. }) = &mut s.modal {
                                query.replace_text(value);
                                *results = None;
                                *preview = None;
                                *selected = 0;
                                *loading = !trimmed.is_empty();
                            }
                        });
                        if let Some(window) = web_sys::window() {
                            clear_search_timer(&debounce_id);
                            if !trimmed.is_empty() {
                                let q = trimmed.clone();
                                let cb = Closure::once_into_js(move || {
                                    if let Some(t) = crate::transport::current() {
                                        t.search_workspace(mode, q);
                                    }
                                });
                                if let Some(func) = cb.dyn_ref::<js_sys::Function>()
                                    && let Ok(id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(func, 220)
                                    && let Ok(mut guard) = debounce_id.lock()
                                {
                                    *guard = Some(id);
                                }
                            }
                        }
                    }
                />
                <button class="btn primary" type="submit" disabled=move || !has_query.get()>"Search"</button>
            </form>
            {move || {
                let Some((results, selected, preview, loading)) = search_state.get() else {
                    return view! { <span></span> }.into_any();
                };
                let matches = results
                    .as_ref()
                    .map(|results| {
                        results
                            .matches
                            .iter()
                            .enumerate()
                            .map(|(index, result)| {
                                let path = result.path.clone();
                                let line = result.line;
                                let is_selected = index == selected;
                                let line_suffix = line.map(|l| format!(":{l}")).unwrap_or_default();
                                view! {
                                    <button class="pick-row search-result" class:active=is_selected aria-pressed=is_selected.to_string() on:click=move |_| {
                                        store.update(|s| {
                                            if let Some(ActiveModal::Search { selected, preview, .. }) = &mut s.modal {
                                                *selected = index;
                                                *preview = None;
                                            }
                                        });
                                        if let Some(t) = crate::transport::current() {
                                            t.preview_workspace(path.clone(), line);
                                        }
                                    }>
                                        <span class="pick-title">{format!("{}{line_suffix}", result.path)}</span>
                                        <span class="pick-meta">{result.snippet.clone()}</span>
                                    </button>
                                }
                                .into_any()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let preview_view = preview
                    .map(|preview| {
                        let lines = preview
                            .lines
                            .iter()
                            .map(|line| {
                                view! {
                                    <div class="preview-line" class:highlighted=line.highlighted>
                                        <span class="diff-no">{line.line_no}</span>
                                        <span>{line.text.clone()}</span>
                                    </div>
                                }
                                .into_any()
                            })
                            .collect::<Vec<_>>();
                        view! {
                            <div class="preview">
                                <div class="diff-title">{preview.title}</div>
                                {AnyView::from(Fragment::new(lines))}
                            </div>
                        }
                        .into_any()
                    })
                    .unwrap_or_else(|| {
                        view! { <div class="search-empty">"Select a result to see its file preview."</div> }.into_any()
                    });
                view! {
                    <div class="search-results">
                        <section class="split-side" aria-label="Search results">
                            <div class="search-results-head">"Results"</div>
                            {if matches.is_empty() {
                                let message = if loading {
                                    "Searching…"
                                } else if results.is_some() {
                                    "No matches found. Try a different query or scope."
                                } else {
                                    "Enter a query to search this workspace."
                                };
                                view! { <div class="search-empty" role="status">{message}</div> }.into_any()
                            } else {
                                AnyView::from(Fragment::new(matches))
                            }}
                        </section>
                        <section class="split-main" aria-label="File preview">
                            <div class="search-results-head">"Preview"</div>
                            {preview_view}
                        </section>
                    </div>
                }
                .into_any()
            }}
        </div>
    }
    .into_any();
    shell_wide("Search", store, body, empty_actions())
}

pub(super) fn stage_workspace_file(store: StoreHandle, path: String, stage: bool) {
    if let Some(t) = crate::transport::current() {
        t.stage_workspace_file(path, stage);
    }
    store.update(|s| {
        if let Some(ActiveModal::DiffView { loading, error, .. }) = &mut s.modal {
            *loading = true;
            *error = None;
        }
    });
}

pub(super) fn diff_body(
    store: StoreHandle,
    files: Vec<meerai_web_protocol::FileChangeDto>,
    selected_path: Option<String>,
    unified: bool,
    diff: Option<FileDiffResponse>,
    loading: bool,
    error: Option<String>,
) -> AnyView {
    let file_count = files.len();
    let file_rows = files
        .into_iter()
        .map(|change| {
            let path = change.path.clone();
            let status = change.status.clone();
            let badge_class = format!("diff-badge {}", status.to_ascii_lowercase());
            let active = selected_path.as_deref() == Some(change.path.as_str());
            let click_status = status.clone();
            let click_path = path.clone();
            let stage_path = path.clone();
            let unstage_path = path.clone();
            let can_stage = change.unstaged;
            let can_unstage = change.staged;
            view! {
                <div class="diff-file-row" class:active=active>
                    <button class="diff-file-select" on:click=move |_| {
                        if let Some(t) = crate::transport::current() {
                            t.fetch_workspace_diff(click_path.clone(), Some(click_status.clone()));
                        }
                        let path = click_path.clone();
                        store.update(move |s| {
                            if let Some(ActiveModal::DiffView { selected_path, loading, error, .. }) = &mut s.modal {
                                *selected_path = Some(path.clone());
                                *loading = true;
                                *error = None;
                            }
                        });
                    }>
                        <span class=badge_class>{status}</span>
                        <span class="diff-file-name">{change.path}</span>
                    </button>
                    <div class="diff-file-actions">
                        {can_stage.then(|| view! {
                            <button
                                class="btn tiny"
                                title="Stage file"
                                on:click=move |_| stage_workspace_file(store, stage_path.clone(), true)
                            >
                                "Stage"
                            </button>
                        })}
                        {can_unstage.then(|| view! {
                            <button
                                class="btn tiny"
                                title="Unstage file"
                                on:click=move |_| stage_workspace_file(store, unstage_path.clone(), false)
                            >
                                "Unstage"
                            </button>
                        })}
                    </div>
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>();

    let diff_view = if let Some(diff) = diff {
        let path = diff.path.clone();
        let status = diff.status.clone();
        let badge_class = format!("diff-badge {}", status.to_ascii_lowercase());
        let additions = diff.additions;
        let deletions = diff.deletions;
        let rows = if unified {
            render_unified_rows(&diff)
        } else {
            render_split_rows(&diff)
        };
        let scroll_class = if unified {
            "diff-scroll diff-unified"
        } else {
            "diff-scroll diff-split"
        };
        view! {
            <div class="diff-head">
                <div class="diff-title-group">
                    <span class="diff-path">{path}</span>
                    <span class=badge_class>{status}</span>
                </div>
                <div class="diff-stats">
                    <span class="diff-add">"+"{additions}</span>
                    <span class="diff-del">"-"{deletions}</span>
                </div>
            </div>
            <div class="diff-toolbar">
                <button class="btn tiny" class:active=unified on:click=move |_| set_diff_mode(store, true)>"Unified"</button>
                <button class="btn tiny" class:active=!unified on:click=move |_| set_diff_mode(store, false)>"Split"</button>
                <span class="diff-hint">"Click a file on the left to switch."</span>
            </div>
            <div class=scroll_class>{AnyView::from(Fragment::new(rows))}</div>
        }
        .into_any()
    } else if loading {
        view! { <div class="diff-empty">"Loading diff…"</div> }.into_any()
    } else if let Some(error) = error {
        view! { <div class="diff-empty banner error">{error}</div> }.into_any()
    } else {
        view! { <div class="diff-empty">"Select a changed file on the left."</div> }.into_any()
    };

    view! {
        <div class="diff-layout">
            <aside class="diff-files">
                <div class="diff-files-head">
                    <span>"Changed files"</span>
                    <div class="side-head-actions">
                        <span class="badge">{file_count}</span>
                        <button
                            class="btn tiny"
                            title="Refresh git diff"
                            on:click=move |_| {
                                if let Some(t) = crate::transport::current() {
                                    t.fetch_workspace_changes();
                                }
                            }
                        >"Refresh"</button>
                    </div>
                </div>
                <div class="diff-files-list">{AnyView::from(Fragment::new(file_rows))}</div>
            </aside>
            <section class="diff-main">{diff_view}</section>
        </div>
    }
    .into_any()
}

pub(super) fn set_diff_mode(store: StoreHandle, next: bool) {
    store.update(|s| {
        if let Some(ActiveModal::DiffView { unified, .. }) = &mut s.modal {
            *unified = next;
        }
    });
}

pub(super) fn render_unified_rows(diff: &FileDiffResponse) -> Vec<AnyView> {
    let mut rows = Vec::new();
    for row in &diff.rows {
        match &row.kind {
            meerai_web_protocol::DiffRowKindDto::HunkHeader { text } => {
                rows.push(diff_line("diff-hunk", "", "", text));
            }
            meerai_web_protocol::DiffRowKindDto::Context => {
                if let Some(cell) = row.right.as_ref().or(row.left.as_ref()) {
                    rows.push(diff_line(
                        "diff-context",
                        &line_no(cell.line_no),
                        " ",
                        &cell.text,
                    ));
                }
            }
            meerai_web_protocol::DiffRowKindDto::Added => {
                if let Some(cell) = row.right.as_ref() {
                    rows.push(diff_line(
                        "diff-added",
                        &line_no(cell.line_no),
                        "+",
                        &cell.text,
                    ));
                }
            }
            meerai_web_protocol::DiffRowKindDto::Removed => {
                if let Some(cell) = row.left.as_ref() {
                    rows.push(diff_line(
                        "diff-removed",
                        &line_no(cell.line_no),
                        "-",
                        &cell.text,
                    ));
                }
            }
            meerai_web_protocol::DiffRowKindDto::Modified => {
                if let Some(cell) = row.left.as_ref() {
                    rows.push(diff_line(
                        "diff-removed",
                        &line_no(cell.line_no),
                        "-",
                        &cell.text,
                    ));
                }
                if let Some(cell) = row.right.as_ref() {
                    rows.push(diff_line(
                        "diff-added",
                        &line_no(cell.line_no),
                        "+",
                        &cell.text,
                    ));
                }
            }
        }
    }
    rows
}

pub(super) fn render_split_rows(diff: &FileDiffResponse) -> Vec<AnyView> {
    let mut rows = Vec::new();
    for row in &diff.rows {
        match &row.kind {
            meerai_web_protocol::DiffRowKindDto::HunkHeader { text } => {
                rows.push(view! { <div class="diff-split-hunk">{text.clone()}</div> }.into_any());
            }
            kind => {
                let class = match kind {
                    meerai_web_protocol::DiffRowKindDto::Added => "diff-added",
                    meerai_web_protocol::DiffRowKindDto::Removed => "diff-removed",
                    meerai_web_protocol::DiffRowKindDto::Modified => "diff-modified",
                    _ => "diff-context",
                };
                let (left_no, left_text) = row
                    .left
                    .as_ref()
                    .map(|cell| (line_no(cell.line_no), cell.text.clone()))
                    .unwrap_or_default();
                let (right_no, right_text) = row
                    .right
                    .as_ref()
                    .map(|cell| (line_no(cell.line_no), cell.text.clone()))
                    .unwrap_or_default();
                rows.push(
                    view! {
                        <div class=format!("diff-row {class}")>
                            <span class="diff-no">{left_no}</span>
                            <span class="diff-cell">{left_text}</span>
                            <span class="diff-no">{right_no}</span>
                            <span class="diff-cell">{right_text}</span>
                        </div>
                    }
                    .into_any(),
                );
            }
        }
    }
    rows
}

pub(super) fn diff_line(class: &str, no: &str, sign: &str, text: &str) -> AnyView {
    view! {
        <div class=format!("udiff-row {class}")>
            <span class="udiff-no">{no.to_owned()}</span>
            <span class="udiff-sign">{sign.to_owned()}</span>
            <span class="udiff-code">{text.to_owned()}</span>
        </div>
    }
    .into_any()
}

pub(super) fn line_no(value: Option<usize>) -> String {
    value.map(|line| line.to_string()).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
struct PanelSnapshot {
    tab: crate::state::WorkspacePanelTab,
    context: Option<meerai_web_protocol::ContextBreakdownDto>,
    loop_state: Option<meerai_web_protocol::LoopDto>,
    memories: Vec<meerai_web_protocol::MemoryDto>,
    routes: Vec<meerai_web_protocol::RouteDto>,
    packs: Vec<meerai_web_protocol::PackDto>,
    definitions: meerai_web_protocol::DefinitionsDiagnosticsDto,
    selected: usize,
    loading: bool,
    error: Option<String>,
    subagent_name: String,
    agent: String,
    task: String,
    model: String,
    mode: Option<meerai_web_protocol::ApprovalModeDto>,
    route_input: String,
    pack_input: String,
    pack_pin_input: String,
    subagent_steps: String,
    subagent_tokens: String,
    subagent_cost: String,
    subagent_tools: String,
    context_turn: usize,
    context_focus: String,
    context_blob: Option<(String, String)>,
}

#[component]
pub(super) fn WorkspacePanelModal(store: StoreHandle) -> impl IntoView {
    let snapshot = Memo::new(move |_| {
        store.with(|state| match &state.modal {
            Some(ActiveModal::WorkspacePanel {
                tab,
                context,
                loop_state,
                memories,
                routes,
                packs,
                definitions,
                selected,
                loading,
                error,
                subagent_name,
                subagent_agent,
                subagent_task,
                subagent_model,
                subagent_mode,
                route_input,
                pack_input,
                context_turn,
                context_focus,
                context_blob,
                subagent_steps,
                subagent_tokens,
                subagent_cost,
                subagent_tools,
                pack_pin_input,
                ..
            }) => Some(PanelSnapshot {
                tab: *tab,
                context: context.as_deref().cloned(),
                loop_state: loop_state.as_deref().cloned(),
                memories: memories.clone(),
                routes: routes.clone(),
                packs: packs.clone(),
                definitions: (**definitions).clone(),
                selected: *selected,
                loading: *loading,
                error: error.clone(),
                subagent_name: subagent_name.text.clone(),
                agent: subagent_agent.clone(),
                task: subagent_task.text.clone(),
                model: subagent_model.text.clone(),
                mode: *subagent_mode,
                route_input: route_input.text.clone(),
                pack_input: pack_input.text.clone(),
                pack_pin_input: pack_pin_input.text.clone(),
                subagent_steps: subagent_steps.text.clone(),
                subagent_tokens: subagent_tokens.text.clone(),
                subagent_cost: subagent_cost.text.clone(),
                subagent_tools: subagent_tools.text.clone(),
                context_turn: *context_turn,
                context_focus: context_focus.text.clone(),
                context_blob: context_blob.clone(),
            }),
            _ => None,
        })
    });

    let reload = {
        let store = store;
        move |tab: crate::state::WorkspacePanelTab| {
            if tab == crate::state::WorkspacePanelTab::Loop {
                return;
            }
            if let Some(transport) = crate::transport::current() {
                match tab {
                    crate::state::WorkspacePanelTab::Context => {
                        transport.run_host_action("/context".to_owned())
                    }
                    crate::state::WorkspacePanelTab::Memory => transport.fetch_memories(),
                    crate::state::WorkspacePanelTab::Routing => transport.fetch_routes(),
                    crate::state::WorkspacePanelTab::Packs => {
                        transport.fetch_packs();
                        transport.fetch_definitions();
                    }
                    _ => {}
                }
            }
            store.update(|state| {
                if let Some(ActiveModal::WorkspacePanel { loading, error, .. }) =
                    state.modal.as_mut()
                {
                    *loading = true;
                    *error = None;
                }
            });
        }
    };

    let reload = std::sync::Arc::new(reload);
    let close = {
        let store = store;
        move |_| store.update(|state| state.modal = None)
    };

    view! {
        <div class="modal-backdrop">
            <div class="modal modal-wide">
                {move || {
                    let Some(PanelSnapshot {
                        tab,
                        context,
                        loop_state,
                        memories,
                        routes,
                        packs,
                        definitions,
                        selected,
                        loading,
                        error,
                        subagent_name,
                        agent,
                        task,
                        model,
                        mode,
                        route_input,
                        pack_input,
                        pack_pin_input,
                        context_turn,
                        context_focus,
                        context_blob,
                        subagent_steps,
                        subagent_tokens,
                        subagent_cost,
                        subagent_tools,
                    }) = snapshot.get() else {
                        return view! { <span></span> }.into_any();
                    };
                    let selected = if tab == crate::state::WorkspacePanelTab::Memory {
                        selected.min(memories.len().saturating_sub(1))
                    } else if tab == crate::state::WorkspacePanelTab::Routing {
                        selected.min(routes.len().saturating_sub(1))
                    } else if tab == crate::state::WorkspacePanelTab::Packs {
                        selected.min(packs.len().saturating_sub(1))
                    } else {
                        0
                    };
                    let reload = reload.clone();
                    let body = match tab {
                        crate::state::WorkspacePanelTab::Loop => {
                            let Some(g) = loop_state.as_ref() else {
                                return view! { <pre class="panel-text">"No active loop. Type @goal <instruction> in the composer to start one."</pre> }.into_any();
                            };
                            let mut text = String::new();
                            if let Some(objective) = g.objective.as_deref() {
                                text.push_str(&format!("Objective: {objective}\n"));
                            }
                            if !g.done_when.is_empty() {
                                text.push_str("Done when:\n");
                                for line in &g.done_when {
                                    text.push_str(&format!("- {line}\n"));
                                }
                            }
                            if !g.constraints.is_empty() {
                                text.push_str("Constraints:\n");
                                for line in &g.constraints {
                                    text.push_str(&format!("- {line}\n"));
                                }
                            }
                            if g.items.is_empty() {
                                text.push_str("\nItems: (none)\n");
                            } else {
                                text.push_str("\nItems:\n");
                                for item in &g.items {
                                    let mark = match item.status {
                                        meerai_web_protocol::LoopItemStatusDto::Done => "x",
                                        meerai_web_protocol::LoopItemStatusDto::Failed => "!",
                                        meerai_web_protocol::LoopItemStatusDto::Open => " ",
                                    };
                                    text.push_str(&format!("- [{mark}] {}. {}", item.id, item.text));
                                    if let Some(expect) = item.expect.as_deref() {
                                        text.push_str(&format!(" (observe: {expect})"));
                                    }
                                    if let Some(action) = item.action.as_deref() {
                                        text.push_str(&format!("\n  action: {action}"));
                                    }
                                    if let Some(observation) = item.observation.as_deref() {
                                        text.push_str(&format!("\n  observation: {observation}"));
                                    }
                                    text.push('\n');
                                }
                            }
                            let done = g
                                .items
                                .iter()
                                .filter(|item| {
                                    matches!(
                                        item.status,
                                        meerai_web_protocol::LoopItemStatusDto::Done
                                    )
                                })
                                .count();
                            text.push_str(&format!(
                                "\nProgress: {done}/{} done",
                                g.items.len()
                            ));
                            text.push_str(&format!("\nPhase: {}", match g.phase {
                                meerai_web_protocol::LoopPhaseDto::Plan => "plan",
                                meerai_web_protocol::LoopPhaseDto::Act => "act",
                                meerai_web_protocol::LoopPhaseDto::Observe => "observe",
                                meerai_web_protocol::LoopPhaseDto::Adjust => "adjust",
                            }));
                            if g.rounds > 0 {
                                text.push_str(&format!(" · round {}", g.rounds));
                            }
                            if g.turns > 0 {
                                text.push_str(&format!(" · turn {}", g.turns));
                            }
                            view! { <pre class="panel-text">{text}</pre> }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Context => {
                            let Some(c) = context.as_ref() else {
                                return view! { <pre class="panel-text">"No context report yet. Press Refresh."</pre> }.into_any();
                            };
                            let summary = format!(
                                "used {} / {} tokens ({:.1}%) · watermark {}\nsystem {} · tools {} · memory {} · user {} · assistant {}\ntool results {} · reasoning {} · summaries {}\npinned {} · cleared results {} · spilled {} · cached {}",
                                c.estimated_total_tokens, c.context_window_tokens,
                                if c.context_window_tokens == 0 { 0.0 } else { c.estimated_total_tokens as f64 * 100.0 / c.context_window_tokens as f64 },
                                c.high_watermark_tokens, c.system_prompt_tokens, c.tool_schemas_tokens,
                                c.memory_tokens, c.user_messages_tokens, c.assistant_tokens,
                                c.tool_results_tokens, c.reasoning_tokens, c.compaction_summary_tokens,
                                c.pinned_turn_ids.len(),
                                c.cleared_tool_results_count, c.spilled_blob_count,
                                c.last_prompt_cached_tokens.map_or_else(|| "n/a".to_owned(), |n| n.to_string())
                            );
                            let turns = c.turn_ids.clone();
                            let pinned_ids = c.pinned_turn_ids.clone();
                            let blobs = c.spilled_blobs.clone();
                            let verbatim = c.verbatim_window_turns;
                            let reasoning = c.reasoning_pruning;
                            let clear = c.clear_old_tool_results;
                            let selected_turn = context_turn;
                            let session_id = store.with(|s| s.active_session_id.clone());
                            let turn_rows = turns.iter().enumerate().map(|(idx, turn)| {
                                let pinned = pinned_ids.contains(turn);
                                let turn_for_pin = turn.clone();
                                let turn_for_sum = turn.clone();
                                let pin_actions = crate::transport::current();
                                let sum_actions = crate::transport::current();
                                view! {
                                    <div class=format!("panel-row {}", if idx == selected_turn { "selected" } else { "" })>
                                        <span class="panel-title">{turn.clone()}</span>
                                        <span class="panel-meta">{if pinned { "pinned" } else { "turn" }}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &pin_actions {
                                                t.run_host_action(format!("/pin-turn {turn_for_pin}{}", if pinned { " off" } else { "" }));
                                                t.run_host_action("/context".to_owned());
                                            }
                                        }>{if pinned { "Unpin" } else { "Pin" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &sum_actions { t.run_host_action(format!("/summarize --from {turn_for_sum}")); }
                                        }>"Summarize from here"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let next_window = match verbatim {
                                0..=3 => 4,
                                4..=7 => 8,
                                8..=15 => 16,
                                _ => 4,
                            };
                            let window_actions = crate::transport::current();
                            let reasoning_actions = crate::transport::current();
                            let clear_actions = crate::transport::current();
                            let policy = view! {
                                <div class="panel-form">
                                    <button class="btn tiny" on:click=move |_| {
                                        if let Some(t) = &window_actions {
                                            t.run_host_action(format!("/context-policy verbatim {next_window}"));
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>{format!("Verbatim {verbatim}")}</button>
                                    <button class="btn tiny" on:click=move |_| {
                                        if let Some(t) = &reasoning_actions {
                                            t.run_host_action(format!("/context-policy reasoning {}", if reasoning { "off" } else { "on" }));
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>{format!("Reasoning {}", if reasoning { "on" } else { "off" })}</button>
                                    <button class="btn tiny" on:click=move |_| {
                                        if let Some(t) = &clear_actions {
                                            t.run_host_action(format!("/context-policy clear {}", if clear { "off" } else { "on" }));
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>{format!("Clear old {}", if clear { "on" } else { "off" })}</button>
                                </div>
                            };
                            let focus_store = store;
                            let focus_click_store = store;
                            let focus_actions = crate::transport::current();
                            let focus = view! {
                                <div class="panel-form">
                                    <input class="field" placeholder="/summarize --from <turn> [focus]"
                                        prop:value=context_focus
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            focus_store.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { context_focus, .. }) = state.modal.as_mut() { context_focus.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let command = focus_click_store.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { context_focus, .. }) => context_focus.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &focus_actions {
                                            t.run_host_action(command);
                                            t.run_host_action("/context".to_owned());
                                        }
                                    }>"Run"</button>
                                </div>
                            };
                            let blob_rows = blobs.iter().map(|key| {
                                let key_for_fetch = key.clone();
                                let actions = crate::transport::current();
                                let session = session_id.clone();
                                view! {
                                    <div class="panel-row">
                                        <span class="panel-title">{key.clone()}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let (Some(t), Some(s)) = (&actions, session.clone()) {
                                                t.fetch_blob(s, key_for_fetch.clone());
                                            }
                                        }>"View"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let close_store = store;
                            let blob_view = context_blob.clone().map(|(key, content)| {
                                view! {
                                    <div class="panel-form column">
                                        <h4>{format!("Blob {key}")}</h4>
                                        <pre class="panel-text">{content}</pre>
                                        <button class="btn" on:click=move |_| {
                                            close_store.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { context_blob, .. }) = state.modal.as_mut() { *context_blob = None; }
                                            });
                                        }>"Close"</button>
                                    </div>
                                }.into_any()
                            }).unwrap_or_else(|| view! { <span></span> }.into_any());
                            view! {
                                <div>
                                    <pre class="panel-text">{summary}</pre>
                                    <h4>"Turns"</h4>
                                    <div class="panel-list">{turn_rows}</div>
                                    <h4>"Projection"</h4>
                                    {policy}
                                    {focus}
                                    <h4>"Spilled blobs"</h4>
                                    <div class="panel-list">{blob_rows}</div>
                                    {blob_view}
                                </div>
                            }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Memory => {
                            let rows = memories.iter().enumerate().map(|(idx, memory)| {
                                let id = memory.id.clone();
                                let pin_id = id.clone();
                                let delete_id = id.clone();
                                let actions = crate::transport::current();
                                let pin_actions = crate::transport::current();
                                let pinned = memory.pinned;
                                view! {
                                    <div class=format!("panel-row {}", if idx == selected { "selected" } else { "" })>
                                        <span class="panel-title">{format!("{} {}", if memory.pinned { "★" } else { " " }, memory.title)}</span>
                                        <span class="panel-meta">{format!("[{}] {}", memory.kind, memory.id)}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &pin_actions { t.run_host_action(format!("/pin {pin_id}{}", if pinned { " off" } else { "" })); t.fetch_memories(); }
                                        }>{if memory.pinned { "Unpin" } else { "Pin" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &actions { t.run_host_action(format!("/forget {delete_id}")); t.fetch_memories(); }
                                        }>"Delete"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            view! { <div class="panel-list">{rows}</div> }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Routing => {
                            let rows = routes.iter().map(|route| {
                                let task = route.task_type.clone();
                                let actions = crate::transport::current();
                                view! {
                                    <div class="panel-row">
                                        <span class="panel-title">{format!("{}", route.task_type)}</span>
                                        <span class="panel-meta">{format!("{}::{}", route.provider, route.model)}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &actions { t.run_host_action(format!("/routing clear {task}")); t.fetch_routes(); }
                                        }>"Delete"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let store_for_input = store;
                            let store_for_click = store;
                            let actions = crate::transport::current();
                            view! {
                                <div class="panel-list">{rows}</div>
                                <div class="panel-form">
                                    <input class="field" placeholder="/routing set build openai::o3 1.0 20000 2"
                                        prop:value=route_input
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            store_for_input.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { route_input, .. }) = state.modal.as_mut() { route_input.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let command = store_for_click.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { route_input, .. }) => route_input.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &actions { t.run_host_action(command); }
                                    }>"Set route"</button>
                                </div>
                            }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Packs => {
                            let rows = packs.iter().enumerate().map(|(idx, pack)| {
                                let enable_id = pack.id.clone();
                                let trust_id = pack.id.clone();
                                let remove_id = pack.id.clone();
                                let enabled = pack.enabled;
                                let trusted = pack.trusted;
                                let actions = crate::transport::current();
                                view! {
                                    <div class=format!("panel-row {}", if idx == selected { "selected" } else { "" })>
                                        <span class="panel-title">{format!("{} [{}] [{}]", pack.id, if pack.enabled { "enabled" } else { "disabled" }, if pack.trusted { "trusted" } else { "untrusted" })}</span>
                                        <span class="panel-meta">{format!("{} @ {}", pack.source_uri, pack.pinned_rev)}</span>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = &actions { t.run_host_action(format!("/pack {} {enable_id}", if enabled { "disable" } else { "enable" })); t.fetch_packs(); }
                                        }>{if pack.enabled { "Disable" } else { "Enable" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = crate::transport::current() { t.run_host_action(format!("/pack {} {trust_id}", if trusted { "untrust" } else { "trust" })); t.fetch_packs(); }
                                        }>{if pack.trusted { "Untrust" } else { "Trust" }}</button>
                                        <button class="btn tiny" on:click=move |_| {
                                            if let Some(t) = crate::transport::current() { t.run_host_action(format!("/pack remove {remove_id}")); t.fetch_packs(); }
                                        }>"Remove"</button>
                                    </div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let defs = definitions.agents.iter().chain(definitions.skills.iter()).map(|def| {
                                let invocation = match def.model_invocable {
                                    Some(true) => " · model",
                                    Some(false) => " · manual",
                                    None => "",
                                };
                                let mode = match def.mode.as_deref() {
                                    Some(mode) if mode != "all" => format!(" · {mode}"),
                                    _ => String::new(),
                                };
                                view! {
                                    <div class="panel-row"><span class="panel-title">{format!("{}/{}", def.kind, def.id)}</span><span class="panel-meta">{format!("{} · ~{} tokens{}{}", def.source, def.token_estimate, invocation, mode)}</span></div>
                                }.into_any()
                            }).collect::<Vec<_>>();
                            let store_for_input = store;
                            let store_for_click = store;
                            let actions = crate::transport::current();
                            let selected_pack = packs.get(selected).map(|pack| pack.id.clone()).unwrap_or_default();
                            let store_for_pin_input = store;
                            let store_for_pin_click = store;
                            let pin_actions = crate::transport::current();
                            let selected_pack_for_pin = selected_pack.clone();
                            view! {
                                <div class="panel-list">{rows}</div>
                                <div class="panel-form">
                                    <input class="field" placeholder="/pack add ./path or git URL"
                                        prop:value=pack_input
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            store_for_input.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { pack_input, .. }) = state.modal.as_mut() { pack_input.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let command = store_for_click.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { pack_input, .. }) => pack_input.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &actions { t.run_host_action(command); }
                                    }>"Install"</button>
                                </div>
                                <div class="panel-form">
                                    <input class="field" placeholder="revision for the selected pack"
                                        prop:value=pack_pin_input
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            store_for_pin_input.update(|state| {
                                                if let Some(ActiveModal::WorkspacePanel { pack_pin_input, .. }) = state.modal.as_mut() { pack_pin_input.replace_text(value); }
                                            });
                                        } />
                                    <button class="btn" on:click=move |_| {
                                        let revision = store_for_pin_click.with(|state| match &state.modal {
                                            Some(ActiveModal::WorkspacePanel { pack_pin_input, .. }) => pack_pin_input.text.clone(),
                                            _ => String::new(),
                                        });
                                        if let Some(t) = &pin_actions
                                            && !revision.trim().is_empty()
                                            && !selected_pack_for_pin.is_empty()
                                        {
                                            t.run_host_action(format!("/pack pin {} {}", selected_pack_for_pin, revision.trim()));
                                            t.fetch_packs();
                                        }
                                    }>"Pin selected"</button>
                                </div>
                                <h4>"Definitions"</h4>
                                <div class="panel-list">{defs}</div>
                            }.into_any()
                        }
                        crate::state::WorkspacePanelTab::Subagent => {
                            let store_name = store;
                            let store_agent = store;
                            let store_task = store;
                            let store_model = store;
                            let store_mode = store;
                            let store_steps = store;
                            let store_tokens = store;
                            let store_cost = store;
                            let store_tools = store;
                            let store_spawn = store;
                            let mode_label = mode.map(|m| match m { meerai_web_protocol::ApprovalModeDto::Plan => "plan", meerai_web_protocol::ApprovalModeDto::Ask => "ask", meerai_web_protocol::ApprovalModeDto::Auto => "auto" }).unwrap_or("parent");
                            view! {
                                <div class="panel-form column">
                                    <label>"Name" <input class="field" placeholder="optional, one word" prop:value=subagent_name.clone() on:input=move |ev| { let v = event_target_value(&ev); store_name.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_name, .. }) = s.modal.as_mut() { subagent_name.replace_text(v); }); } /></label>
                                    <label>"Agent" <input class="field" prop:value=agent.clone() on:input=move |ev| { let v = event_target_value(&ev); store_agent.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_agent, .. }) = s.modal.as_mut() { *subagent_agent = v; }); } /></label>
                                    <label>"Task" <input class="field" prop:value=task.clone() on:input=move |ev| { let v = event_target_value(&ev); store_task.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_task, .. }) = s.modal.as_mut() { subagent_task.replace_text(v); }); } /></label>
                                    <label>"Model" <input class="field" prop:value=model.clone() on:input=move |ev| { let v = event_target_value(&ev); store_model.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_model, .. }) = s.modal.as_mut() { subagent_model.replace_text(v); }); } /></label>
                                    <label>"Mode" <button class="btn tiny" on:click=move |_| {
                                        store_mode.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_mode, .. }) = s.modal.as_mut() {
                                            *subagent_mode = match *subagent_mode {
                                                None => Some(meerai_web_protocol::ApprovalModeDto::Plan),
                                                Some(meerai_web_protocol::ApprovalModeDto::Plan) => Some(meerai_web_protocol::ApprovalModeDto::Ask),
                                                Some(meerai_web_protocol::ApprovalModeDto::Ask) => Some(meerai_web_protocol::ApprovalModeDto::Auto),
                                                Some(meerai_web_protocol::ApprovalModeDto::Auto) => None,
                                            };
                                        });
                                    }>{mode_label}</button></label>
                                    <label>"Steps" <input class="field" placeholder="preset" prop:value=subagent_steps.clone() on:input=move |ev| { let v = event_target_value(&ev); store_steps.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_steps, .. }) = s.modal.as_mut() { subagent_steps.replace_text(v); }); } /></label>
                                    <label>"Tokens" <input class="field" placeholder="parent cap" prop:value=subagent_tokens.clone() on:input=move |ev| { let v = event_target_value(&ev); store_tokens.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_tokens, .. }) = s.modal.as_mut() { subagent_tokens.replace_text(v); }); } /></label>
                                    <label>"Cost USD" <input class="field" placeholder="parent cap" prop:value=subagent_cost.clone() on:input=move |ev| { let v = event_target_value(&ev); store_cost.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_cost, .. }) = s.modal.as_mut() { subagent_cost.replace_text(v); }); } /></label>
                                    <label>"Tools" <input class="field" placeholder="comma-separated subset" prop:value=subagent_tools.clone() on:input=move |ev| { let v = event_target_value(&ev); store_tools.update(|s| if let Some(ActiveModal::WorkspacePanel { subagent_tools, .. }) = s.modal.as_mut() { subagent_tools.replace_text(v); }); } /></label>
                                    <button class="btn" on:click=move |_| {
                                        let (name, agent, task, model, mode, steps, tokens, cost, tools) = store_spawn.with(|s| match &s.modal {
                                            Some(ActiveModal::WorkspacePanel { subagent_name, subagent_agent, subagent_task, subagent_model, subagent_mode, subagent_steps, subagent_tokens, subagent_cost, subagent_tools, .. }) => (
                                                subagent_name.text.clone(),
                                                subagent_agent.clone(),
                                                subagent_task.text.clone(),
                                                subagent_model.text.clone(),
                                                *subagent_mode,
                                                subagent_steps.text.clone(),
                                                subagent_tokens.text.clone(),
                                                subagent_cost.text.clone(),
                                                subagent_tools.text.clone(),
                                            ),
                                            _ => (String::new(), String::new(), String::new(), String::new(), None, String::new(), String::new(), String::new(), String::new()),
                                        });
                                        if let Some(t) = crate::transport::current() {
                                            let mut command = format!("/subagent {} ", agent);
                                            if !name.trim().is_empty() { command.push_str(&format!("--name {} ", name.trim())); }
                                            if !model.is_empty() { command.push_str(&format!("--model {model} ")); }
                                            if let Some(mode) = mode { command.push_str(&format!("--mode {} ", match mode { meerai_web_protocol::ApprovalModeDto::Plan => "plan", meerai_web_protocol::ApprovalModeDto::Ask => "ask", meerai_web_protocol::ApprovalModeDto::Auto => "auto" })); }
                                            if !steps.trim().is_empty() { command.push_str(&format!("--steps {} ", steps.trim())); }
                                            if !tokens.trim().is_empty() { command.push_str(&format!("--tokens {} ", tokens.trim())); }
                                            if !cost.trim().is_empty() { command.push_str(&format!("--cost {} ", cost.trim())); }
                                            if !tools.trim().is_empty() { command.push_str(&format!("--tools {} ", tools.trim())); }
                                            command.push_str(&task);
                                            t.run_host_action(command);
                                        }
                                    }>"Spawn subagent"</button>
                                </div>
                            }.into_any()
                        }
                    };
                    let tabs = [
                        (crate::state::WorkspacePanelTab::Context, "Context"),
                        (crate::state::WorkspacePanelTab::Loop, "Goal"),
                        (crate::state::WorkspacePanelTab::Memory, "Memory"),
                        (crate::state::WorkspacePanelTab::Subagent, "Subagent"),
                        (crate::state::WorkspacePanelTab::Routing, "Routing"),
                        (crate::state::WorkspacePanelTab::Packs, "Packs"),
                    ];
                    let tab_buttons = tabs.into_iter().map(|(value, label)| {
                        let store_tab = store;
                        view! {
                            <button class=format!("btn tiny {}", if value == tab { "active" } else { "" }) on:click=move |_| {
                                store_tab.update(|s| if let Some(ActiveModal::WorkspacePanel { tab, .. }) = s.modal.as_mut() { *tab = value; });
                            }>{label}</button>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <div class="modal-head">
                            <h2>"Control panel"</h2>
                            <div class="panel-tabs">{tab_buttons}</div>
                            <button class="btn tiny" on:click={
                                let reload = reload.clone();
                                move |_| reload(tab)
                            }>"Refresh"</button>
                            <button class="btn tiny" on:click=close.clone()>"Close"</button>
                        </div>
                        {if loading { view! { <div class="panel-status">"Loading..."</div> }.into_any() } else if let Some(error) = error { view! { <div class="panel-status error">{error}</div> }.into_any() } else { view! { <span></span> }.into_any() }}
                        {body}
                    }.into_any()
                }}
            </div>
        </div>
    }
}
