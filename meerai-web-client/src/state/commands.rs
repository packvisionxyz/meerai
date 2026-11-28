use super::completion::*;
use super::forms::*;
use super::misc::*;
use super::providers::*;
use super::sessions::*;
use super::settings::*;
use super::types::*;
use super::views::*;
use super::*;

pub use meerai_commands::CommandSpec as SlashCommandSpec;

pub const SLASH_COMMANDS: &[SlashCommandSpec] = meerai_commands::COMMANDS;

pub fn slash_commands_for_web() -> Vec<&'static SlashCommandSpec> {
    SLASH_COMMANDS
        .iter()
        .filter(|spec| spec.clients.has_web())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteActionSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

pub const PALETTE_ACTIONS: &[PaletteActionSpec] = &[
    PaletteActionSpec {
        key: "copy-last",
        label: "Copy last reply",
        hint: "Alt+C",
    },
    PaletteActionSpec {
        key: "next-tab",
        label: "Next session tab",
        hint: "Alt+]",
    },
    PaletteActionSpec {
        key: "prev-tab",
        label: "Previous session tab",
        hint: "Alt+[",
    },
    PaletteActionSpec {
        key: "move-tab-left",
        label: "Move tab left",
        hint: "Alt+Shift+[",
    },
    PaletteActionSpec {
        key: "move-tab-right",
        label: "Move tab right",
        hint: "Alt+Shift+]",
    },
    PaletteActionSpec {
        key: "cycle-mode",
        label: "Cycle approval mode",
        hint: "Alt+M",
    },
    PaletteActionSpec {
        key: "cycle-effort",
        label: "Cycle reasoning effort",
        hint: "/effort",
    },
    PaletteActionSpec {
        key: "new-session",
        label: "New session tab",
        hint: "/new",
    },
    PaletteActionSpec {
        key: "toggle-sidebar",
        label: "Toggle workspace sidebar",
        hint: "Ctrl+B",
    },
    PaletteActionSpec {
        key: "toggle-diagnose",
        label: "Toggle diagnose block",
        hint: "/diagnose",
    },
    PaletteActionSpec {
        key: "toggle-changes",
        label: "Toggle changed files block",
        hint: "File Changes",
    },
    PaletteActionSpec {
        key: "open-diff",
        label: "Inspect workspace diff",
        hint: "Alt+Shift+D",
    },
    PaletteActionSpec {
        key: "open-search",
        label: "Search workspace",
        hint: "Alt+Shift+F",
    },
    PaletteActionSpec {
        key: "open-settings",
        label: "Open settings",
        hint: "/settings",
    },
];

pub fn filtered_palette_actions(filter: &str) -> Vec<&'static PaletteActionSpec> {
    let q = filter.trim().to_ascii_lowercase();
    PALETTE_ACTIONS
        .iter()
        .filter(|a| {
            q.is_empty()
                || a.label.to_ascii_lowercase().contains(&q)
                || a.hint.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

pub fn copy_last_reply(state: &AppState, tx: &mut UiEffects) -> bool {
    let Some(tab) = state.active_tab() else {
        return false;
    };
    if let Some(TranscriptEntryDto::Assistant { text, .. }) = tab
        .entries
        .iter()
        .rev()
        .find(|e| matches!(e, TranscriptEntryDto::Assistant { .. }))
    {
        if text.trim().is_empty() {
            return false;
        }
        tx.copy(text.clone());
        return true;
    }
    false
}

pub fn execute_palette_action(state: &mut AppState, tx: &mut UiEffects, key: &str) {
    match key {
        "copy-last" => {
            if copy_last_reply(state, tx) {
                set_status_notice(state, "Copied the latest reply");
            } else {
                set_status_notice(state, "No assistant reply to copy");
            }
        }
        "next-tab" => cycle_tab(state, 1),
        "prev-tab" => cycle_tab(state, -1),
        "move-tab-left" => reorder_active_tab(state, -1),
        "move-tab-right" => reorder_active_tab(state, 1),
        "cycle-mode" => cycle_approval_mode(state),
        "cycle-effort" => execute_slash_command(state, "/effort"),
        "new-session" => {
            if let Some(t) = crate::transport::current() {
                t.create_session(None);
            }
        }
        "toggle-sidebar" => state.sidebar_open = !state.sidebar_open,
        "toggle-diagnose" => toggle_diagnose(state),
        "toggle-changes" => toggle_changes(state),
        "open-diff" => open_diff_modal(state, None),
        "open-search" => open_search_modal(state, meerai_web_protocol::SearchModeDto::Content, ""),
        "open-settings" => open_settings(state, None),
        _ => {}
    }
}

pub fn execute_slash_command(state: &mut AppState, raw: &str) {
    let trimmed = raw.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().map(str::trim).unwrap_or("");

    match cmd {
        "/help" => {
            state.modal = Some(ActiveModal::Help);
        }
        "/sessions" => {
            if !arg.is_empty() && state.sessions.iter().any(|s| s.id == arg) {
                activate_and_switch_session(state, arg);
            } else {
                state.session_panel.reveal(Some(arg));
            }
        }
        "/new" => {
            if let Some(t) = crate::transport::current() {
                t.create_session(None);
            }
        }
        "/fork" => {
            if state.is_busy() {
                state.error_banner = Some("Cancel the running turn before forking.".to_owned());
            } else if let Some(sid) = state.active_session_id.clone() {
                match state.user_turns().last().map(|(id, _)| id.clone()) {
                    Some(turn) => {
                        if let Some(t) = crate::transport::current() {
                            t.fork_session(sid, Some(turn));
                        }
                    }
                    None => {
                        set_status_notice(state, "No closed turn to fork.");
                    }
                }
            }
        }
        "/btw" => {
            let Some(session) = state.active_session() else {
                set_status_notice(state, "btw needs an open session.");
                return;
            };
            if matches!(session.backend, BackendKind::Acp { .. }) {
                set_status_notice(
                    state,
                    "btw needs a native session (ACP keeps its own history).",
                );
                return;
            }
            let session_id = session.id.clone();
            if let Some(t) = crate::transport::current() {
                t.btw_session(session_id, arg.to_owned());
            }
        }
        "/acp" => {
            if state
                .active_session()
                .is_some_and(|s| s.approval_mode == ApprovalModeDto::Plan)
            {
                state.error_banner = Some("ACP sessions cannot run in plan mode.".to_owned());
                return;
            }
            let preset = if arg.is_empty() { "agy" } else { arg };
            if let Some(t) = crate::transport::current() {
                t.create_session(Some(BackendKind::Acp {
                    preset: preset.to_owned(),
                    remote_session_id: None,
                }));
            }
        }
        "/close" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.close_session(sid);
            }
        }
        "/auth" | "/connect" | "/provider" => {
            open_provider_select(state);
        }
        "/model" | "/models" => {
            if arg.is_empty() {
                open_model_input(state);
            } else if let Some(sid) = state.active_session_id.clone() {
                let (provider, model) = parse_provider_model_arg(
                    arg,
                    state
                        .active_session()
                        .map(|s| s.provider.as_str())
                        .unwrap_or("openai"),
                );
                if let Some(t) = crate::transport::current() {
                    t.set_model(sid, provider, model);
                }
            }
        }
        "/effort" => {
            if arg.is_empty() {
                open_settings(state, Some(SettingsSection::Context));
            } else {
                set_effort_from_arg(state, arg);
            }
        }
        "/sampling" | "/temperature" => {
            open_settings(state, Some(SettingsSection::Context));
        }
        "/agent" => {
            if arg.is_empty() {
                let msg = format!(
                    "Usage: /agent <id>. Available: {}",
                    state
                        .host_info
                        .agents
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                set_status_notice(state, msg);
            } else if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.apply_agent(sid, arg.to_owned());
            }
        }
        "/context" => {
            open_workspace_panel(state, WorkspacePanelTab::Context);
            if let Some(transport) = crate::transport::current() {
                transport.run_host_action("/context".to_owned());
            }
        }
        "/loop" => {
            open_workspace_panel(state, WorkspacePanelTab::Loop);
        }
        "/memory" => {
            open_workspace_panel(state, WorkspacePanelTab::Memory);
            if let Some(transport) = crate::transport::current() {
                transport.fetch_memories();
            }
        }
        "/routing" | "/routes" => {
            open_workspace_panel(state, WorkspacePanelTab::Routing);
            if let Some(transport) = crate::transport::current() {
                transport.fetch_routes();
            }
        }
        "/packs" | "/definitions" => {
            open_workspace_panel(state, WorkspacePanelTab::Packs);
            if let Some(transport) = crate::transport::current() {
                transport.fetch_packs();
                transport.fetch_definitions();
            }
        }
        "/subagent" | "/subagents" => {
            if arg.is_empty() {
                open_workspace_panel(state, WorkspacePanelTab::Subagent);
            } else if let Some(transport) = crate::transport::current() {
                transport.run_host_action(trimmed.to_string());
            } else {
                set_status_notice(state, "Host actions need a live server connection.");
            }
        }
        "/vault-reset" => {
            state.modal = Some(ActiveModal::VaultReset {
                input: TextBuffer::new(""),
                error: None,
            });
        }
        "/mcp" => {
            if arg.is_empty() || arg == "list" {
                state.modal = Some(ActiveModal::McpServers { selected: 0 });
            } else if let Some(transport) = crate::transport::current() {
                transport.run_host_action(trimmed.to_string());
            } else {
                set_status_notice(state, "Host actions need a live server connection.");
            }
        }
        "/remember" | "/forget" | "/pin" | "/pack" | "/open" => {
            if let Some(transport) = crate::transport::current() {
                transport.run_host_action(trimmed.to_string());
            } else {
                set_status_notice(state, "Host actions need a live server connection.");
            }
        }
        "/agents" => {
            state.draft.replace_text("@agent/");
            update_completion(state);
        }
        "/skill" => {
            if arg.is_empty() {
                let msg = format!(
                    "Usage: /skill <id>. Available: {}",
                    state
                        .host_info
                        .skills
                        .iter()
                        .map(|s| s.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                set_status_notice(state, msg);
            } else if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.apply_skill(sid, arg.to_owned());
            }
        }
        "/skills" => {
            open_workspace_panel(state, WorkspacePanelTab::Packs);
            if let Some(transport) = crate::transport::current() {
                transport.fetch_definitions();
                transport.fetch_packs();
            }
        }
        "/search" | "/tv" | "/find" => {
            open_search_modal(state, meerai_web_protocol::SearchModeDto::Files, arg);
        }
        "/grep" => {
            open_search_modal(state, meerai_web_protocol::SearchModeDto::Content, arg);
        }
        "/diff" => {
            open_diff_modal(state, (!arg.is_empty()).then(|| arg.to_owned()));
        }
        "/sidebar" => {
            state.sidebar_open = !state.sidebar_open;
        }
        "/diagnose" => {
            toggle_diagnose(state);
        }
        "/clear" => {
            if let Some(tab) = state.active_tab_mut() {
                tab.entries.clear();
                tab.entries_version = tab.entries_version.wrapping_add(1);
                tab.streaming_item_id = None;
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
                tab.expanded_digests.clear();
                tab.expanded_tools.clear();
                tab.expanded_reasoning.clear();
                tab.expanded_errors.clear();
                tab.expanded_subagents.clear();
                set_status_notice(state, "Cleared this view. The session log is unchanged.");
            }
        }
        "/reset" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.reset_session(sid);
            }
        }
        "/cancel" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.cancel_turn(sid);
            }
        }
        "/compact" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.compact_session(sid);
            }
        }
        "/mode" => {
            if arg.is_empty() {
                let label = state
                    .active_session()
                    .map(|session| match session.approval_mode {
                        ApprovalModeDto::Plan => "plan",
                        ApprovalModeDto::Ask => "ask",
                        ApprovalModeDto::Auto => "auto",
                    })
                    .unwrap_or("ask");
                set_status_notice(
                    state,
                    format!("Approval mode: {label}. Use /mode plan|ask|auto."),
                );
            } else {
                let mode = match arg.to_ascii_lowercase().as_str() {
                    "plan" => Some(ApprovalModeDto::Plan),
                    "ask" => Some(ApprovalModeDto::Ask),
                    "auto" => Some(ApprovalModeDto::Auto),
                    _ => None,
                };
                match mode {
                    Some(m) => set_approval_mode(state, m),
                    None => {
                        state.error_banner =
                            Some(format!("Unknown mode '{arg}'. Use: plan, ask, auto."));
                    }
                }
            }
        }
        "/rewind" => {
            if state.is_busy() {
                state.error_banner = Some("Cannot rewind while a turn is running.".to_owned());
            } else {
                open_rewind_modal(state, (!arg.is_empty()).then_some(arg));
            }
        }
        "/reload" => {
            if let Some(t) = crate::transport::current() {
                t.reload_config();
            }
        }
        "/settings" => open_settings(state, None),
        "/update" => {
            let msg = match &state.host_info.update_available {
                Some(v) => format!(
                    "Update available: {v}. Run: {}",
                    state.host_info.install_hint
                ),
                None => format!(
                    "meerai is up to date. Install/upgrade: {}",
                    state.host_info.install_hint
                ),
            };
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg);
            } else {
                set_status_notice(state, msg);
            }
        }
        "/quit" | "/q" => {
            let msg = "Browser companion cannot stop the host process. Close the browser tab or use /close to shut down this session.";
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.to_owned());
            } else {
                set_status_notice(state, msg);
            }
        }
        "/serve" => {
            let msg = "Host web server is already serving this browser UI.";
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.to_owned());
            } else {
                set_status_notice(state, msg);
            }
        }
        _ => {
            state.error_banner = Some(format!(
                "Unknown command: {cmd}. Type /help for available commands."
            ));
        }
    }
}

pub fn filtered_commands(filter: &str) -> Vec<&'static SlashCommandSpec> {
    let q = filter.trim().to_ascii_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|c| c.clients.has_web())
        .filter(|c| {
            q.is_empty()
                || c.name.to_ascii_lowercase().contains(&q)
                || c.desc.to_ascii_lowercase().contains(&q)
        })
        .collect()
}
