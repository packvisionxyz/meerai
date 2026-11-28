use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{
    BackendKind, ContextBreakdownDto, FileChangeDto, McpServerInfoDto, TurnMetricsDto, UsageDto,
};

use crate::state::{
    ActiveModal, format_ratio, format_tokens, open_diff_modal, open_diff_modal_with_status,
    toggle_diagnose,
};
use crate::store::StoreHandle;

use super::icons::{IconClose, IconRefresh};
use super::sessions;

#[derive(Clone, PartialEq)]
struct SidebarSnapshot {
    open: bool,
    workspace_root: String,
    git_branch: Option<String>,
    agents_count: usize,
    skills_count: usize,
    hooks_count: usize,
    mcp_servers: Vec<McpServerInfoDto>,
    enabled_tools: Vec<String>,
    tools_expanded: bool,
    changes: Vec<FileChangeDto>,
    backend: Option<BackendKind>,
    metrics: TurnMetricsDto,
    usage: UsageDto,
    context: Option<ContextBreakdownDto>,
    diagnose_open: bool,
    changes_open: bool,
    subagents: Vec<SidebarSubagent>,
}

#[derive(Clone, PartialEq)]
struct SidebarSubagent {
    id: String,
    name: Option<String>,
    agent: String,
    status: String,
    child_session_id: Option<String>,
}

#[component]
pub fn Sidebar(store: StoreHandle) -> impl IntoView {
    let sidebar_memo = Memo::new(move |_| {
        store.with(|s| {
            let metrics = s
                .active_tab()
                .map(|t| t.metrics.clone())
                .unwrap_or_default();
            let usage = s.active_tab().map(|t| t.usage.clone()).unwrap_or_default();
            let context = s.active_tab().and_then(|t| t.context.clone());
            let subagents = s
                .active_tab()
                .map(|tab| {
                    tab.entries
                        .iter()
                        .filter_map(|entry| match entry {
                            meerai_web_protocol::TranscriptEntryDto::Subagent {
                                id,
                                name,
                                agent,
                                status,
                                child_session_id,
                                ..
                            } => Some(SidebarSubagent {
                                id: id.clone(),
                                name: name.clone(),
                                agent: agent.clone(),
                                status: status.clone(),
                                child_session_id: child_session_id.clone(),
                            }),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            SidebarSnapshot {
                open: s.sidebar_open,
                workspace_root: s.host_info.workspace_root.clone(),
                git_branch: s.host_info.git_branch.clone(),
                agents_count: s.host_info.agents.len(),
                skills_count: s.host_info.skills.len(),
                hooks_count: s.host_info.hooks_count,
                mcp_servers: s.host_info.mcp_servers.clone(),
                enabled_tools: s.host_info.enabled_tools.clone(),
                tools_expanded: s.sidebar_tools_expanded,
                changes: s.file_changes.clone(),
                backend: s.active_session().map(|x| x.backend.clone()),
                metrics,
                usage,
                context,
                diagnose_open: s.sidebar_diagnose_open,
                changes_open: s.sidebar_changes_open,
                subagents,
            }
        })
    });

    view! {
        {move || {
            let SidebarSnapshot {
                open,
                workspace_root,
                git_branch,
                agents_count,
                skills_count,
                hooks_count,
                mcp_servers,
                enabled_tools,
                tools_expanded,
                changes,
                backend,
                metrics,
                usage,
                context,
                diagnose_open,
                changes_open,
                subagents,
            } = sidebar_memo.get();
            if !open {
                return view! { <span></span> }.into_any();
            }
            let branch_label = git_branch.unwrap_or_else(|| "no branch".to_owned());
            let backend_label = match backend {
                Some(BackendKind::Acp { preset, .. }) => format!("ACP ({preset})"),
                Some(BackendKind::Native) => "Native".to_owned(),
                None => "—".to_owned(),
            };
            let tool_count = enabled_tools.len();
            let ctx_pct = context.as_ref().map(|c| {
                if c.context_window_tokens == 0 {
                    0.0
                } else {
                    (c.estimated_total_tokens as f64 * 100.0 / c.context_window_tokens as f64)
                        .clamp(0.0, 100.0)
                }
            });
            let diagnose_rows: Vec<(String, String)> = {
                let prompt = usage.prompt_tokens.unwrap_or(0).max(0) as u64;
                let hit = usage
                    .cached_tokens
                    .map_or_else(|| "n/a".to_owned(), |c| format_ratio(c.max(0) as u64, prompt));
                let write = usage
                    .cache_creation_tokens
                    .map_or_else(|| "n/a".to_owned(), |c| format_tokens(c.max(0) as u64));
                let mut rows = vec![
                    ("Cache hit (last)".to_owned(), hit),
                    ("Cache write".to_owned(), write),
                ];
                if let Some(c) = context.as_ref() {
                    rows.push((
                        "Context used".to_owned(),
                        format_ratio(c.estimated_total_tokens, c.context_window_tokens),
                    ));
                    rows.push((
                        "Memory".to_owned(),
                        format!("{} tokens", format_tokens(c.memory_tokens)),
                    ));
                    rows.push((
                        "Tool schemas".to_owned(),
                        format!("{} tokens", format_tokens(c.tool_schemas_tokens)),
                    ));
                }
                rows
            };

            view! {
                <aside class="sidebar open" aria-label="Workspace inspector" inert=move || sessions::background_inert(store)>
                    <div class="sidebar-heading">
                        <h2>"Inspector"</h2>
                        <button class="modal-close" aria-label="Close inspector" on:click=move |_| store.update(|s| s.sidebar_open = false)>
                            <IconClose />
                        </button>
                    </div>
                    <section class="side-section">
                        <div class="side-head">
                            <h3>"Workspace"</h3>
                            <span class="badge">{backend_label}</span>
                        </div>
                        <div class="side-path">{workspace_root}</div>
                        <div class="side-meta">{format!("{branch_label} · {agents_count} agents · {skills_count} skills · {hooks_count} hooks")}</div>
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h3>"Session metrics"</h3>
                            {usage.est_cost_usd.filter(|c| *c > 0.0).map(|cost| view! {
                                <span class="side-cost">{format!("${cost:.4}")}</span>
                            })}
                        </div>
                        <div class="side-stats-grid">
                            <div class="side-stat">
                                <span class="side-stat-label">"Turns"</span>
                                <strong>{metrics.turn_count}</strong>
                            </div>
                            <div class="side-stat">
                                <span class="side-stat-label">"Tool calls"</span>
                                <strong>{metrics.tool_calls}</strong>
                            </div>
                            <div class="side-stat">
                                <span class="side-stat-label">"Session Σ"</span>
                                <strong>{format_tokens(metrics.session_total_tokens)}</strong>
                            </div>
                            <div class="side-stat">
                                <span class="side-stat-label">"Cached Σ"</span>
                                <strong>{format_tokens(metrics.session_cached_tokens)}</strong>
                            </div>
                        </div>
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h3>"Diagnose"</h3>
                            <button
                                class="btn tiny subtle"
                                on:click=move |_| store.update(toggle_diagnose)
                            >
                                {if diagnose_open { "Hide" } else { "Show" }}
                            </button>
                        </div>
                        {if diagnose_open {
                            view! {
                                <>
                                    {ctx_pct.map(|pct| {
                                        let tone = if pct >= 90.0 {
                                            "error"
                                        } else if pct >= 75.0 {
                                            "warn"
                                        } else {
                                            "ok"
                                        };
                                        view! {
                                            <div class="ctx-meter" title=format!("{pct:.1}% context used")>
                                                <div class=format!("ctx-meter-fill {tone}") style=format!("width:{pct:.1}%")></div>
                                            </div>
                                        }
                                    })}
                                    {AnyView::from(Fragment::new(
                                        diagnose_rows
                                            .into_iter()
                                            .map(|(label, value)| view! {
                                                <div class="side-kv">
                                                    <span class="side-stat-label">{label}</span>
                                                    <span class="side-kv-val">{value}</span>
                                                </div>
                                            }.into_any())
                                            .collect::<Vec<_>>(),
                                    ))}
                                </>
                            }
                            .into_any()
                        } else {
                            view! { <div class="side-meta">"Cache hit ratio, context use, and memory size"</div> }.into_any()
                        }}
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h3>"Changed files"</h3>
                            <div class="side-head-actions">
                                {(!changes.is_empty()).then(|| view! {
                                    <span class="badge">{changes.len()}</span>
                                })}
                                {(!changes.is_empty()).then(|| view! {
                                    <button
                                        class="btn tiny subtle"
                                        title="Open workspace diff (Alt+Shift+D)"
                                        on:click=move |_| store.update(|s| open_diff_modal(s, None))
                                    >"Diff"</button>
                                })}
                                <button
                                    class="btn tiny subtle"
                                    title="Refresh git status"
                                    aria-label="Refresh git status"
                                    on:click=move |_| {
                                        if let Some(t) = crate::transport::current() {
                                            t.fetch_workspace_changes();
                                        }
                                    }
                                ><IconRefresh /></button>
                                <button
                                    class="btn tiny subtle"
                                    on:click=move |_| store.update(|s| s.sidebar_changes_open = !s.sidebar_changes_open)
                                >
                                    {if changes_open { "Hide" } else { "Show" }}
                                </button>
                            </div>
                        </div>
                        {if !changes_open {
                            let summary = if changes.is_empty() {
                                "Working tree clean".to_owned()
                            } else {
                                format!("{} changed file(s)", changes.len())
                            };
                            view! { <div class="side-meta">{summary}</div> }.into_any()
                        } else if changes.is_empty() {
                            view! { <div class="side-empty">"Working tree clean"</div> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(
                                changes
                                    .into_iter()
                                    .map(|change| {
                                        let path = change.path.clone();
                                        let status = change.status.clone();
                                        view! {
                                            <button
                                                class="side-row"
                                                title="Open diff"
                                                on:click=move |_| store.update(|s| open_diff_modal_with_status(s, Some(path.clone()), Some(status.clone())))
                                            >
                                                <span class=format!("diff-badge {}", change.status.to_ascii_lowercase())>
                                                    {change.status.clone()}
                                                </span>
                                                <span class="side-row-text">{change.path.clone()}</span>
                                            </button>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h3>"Tools & MCP"</h3>
                            <div class="side-head-actions">
                                <button
                                    class="btn tiny subtle"
                                    title="Manage MCP servers (/mcp)"
                                    on:click=move |_| store.update(|s| s.modal = Some(ActiveModal::McpServers { selected: 0 }))
                                >"Manage"</button>
                                <button
                                    class="btn tiny subtle"
                                    on:click=move |_| store.update(|s| s.sidebar_tools_expanded = !s.sidebar_tools_expanded)
                                >
                                    {if tools_expanded { "Hide" } else { "Show" }}
                                </button>
                            </div>
                        </div>
                        <div class="side-meta">{format!("{tool_count} built-in tools · {} MCP server(s)", mcp_servers.len())}</div>
                        {if mcp_servers.is_empty() {
                            view! { <span></span> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(
                                mcp_servers
                                    .into_iter()
                                    .map(|srv| {
                                        let status_class = match srv.status.as_str() {
                                            "connected" | "configured" => "ok",
                                            "failed" | "unsupported" => "warn",
                                            "needs trust" | "untrusted" => "warn",
                                            _ => "muted",
                                        };
                                        let error = srv.detail.clone();
                                        view! {
                                            <div class="side-mcp-item">
                                                <div class="side-kv">
                                                    <span class="side-row-text">{format!("{} ({})", srv.name, srv.kind)}</span>
                                                    <span class=format!("badge {status_class}")>
                                                        {if srv.status.is_empty() {
                                                            format!("{} tools", srv.tools.len())
                                                        } else {
                                                            format!("{} · {} tools", srv.status, srv.tools.len())
                                                        }}
                                                    </span>
                                                </div>
                                                {error
                                                    .map(|detail| {
                                                        view! {
                                                            <div class="side-meta side-error">
                                                                {format!("↳ {detail}")}
                                                            </div>
                                                        }
                                                        .into_any()
                                                    })
                                                    .unwrap_or_else(|| view! { <span></span> }.into_any())}
                                            </div>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                        <Show when=move || tools_expanded>
                            <div class="side-tools-list">
                                {AnyView::from(Fragment::new(
                                    enabled_tools
                                        .iter()
                                        .map(|t| view! { <code class="tool-chip">{t.clone()}</code> }.into_any())
                                        .collect(),
                                ))}
                            </div>
                        </Show>
                    </section>
                    <section class="side-section">
                        <div class="side-head">
                            <h3>"Subagents"</h3>
                        </div>
                        {if subagents.is_empty() {
                            view! { <div class="side-empty">"None"</div> }.into_any()
                        } else {
                            AnyView::from(Fragment::new(
                                subagents
                                    .into_iter()
                                    .map(|subagent| {
                                        let SidebarSubagent {
                                            id,
                                            name,
                                            agent,
                                            status,
                                            child_session_id: child,
                                        } = subagent;
                                        let click_id = id.clone();
                                        let label = name.clone().unwrap_or_else(|| agent.clone());
                                        view! {
                                            <button
                                                class="side-row"
                                                on:click=move |_| store.update(|s| {
                                                    s.modal = Some(ActiveModal::SubagentOverlay {
                                                        entry_id: click_id.clone(),
                                                        name: name.clone(),
                                                        agent: agent.clone(),
                                                        status: status.clone(),
                                                        child_session_id: child.clone(),
                                                        entries: Vec::new(),
                                                        usage: None,
                                                        loading: child.is_some(),
                                                        error: None,
                                                        from_runs: None,
                                                    });
                                                    if let Some(child) = child.clone()
                                                        && let Some(t) = crate::transport::current()
                                                    {
                                                        t.fetch_session_events(child, true);
                                                    }
                                                })
                                            >
                                                <span class=format!("tab-dot {}", if status == "running" { "running" } else { "idle" })></span>
                                                <span class="side-row-text">{label}</span>
                                                <span class="badge muted">{status.clone()}</span>
                                            </button>
                                        }
                                        .into_any()
                                    })
                                    .collect(),
                            ))
                        }}
                    </section>
                </aside>
            }
            .into_any()
        }}
    }
}
