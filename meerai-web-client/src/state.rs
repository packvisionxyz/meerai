mod session_panel;

pub use session_panel::{HistoryStatus, SessionPanel, SessionPanelAction, SessionPanelRow};

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::text::{TextBuffer, UiEffects};
use meerai_web_protocol::{
    ApprovalModeDto, ApprovalRequestDto, BackendKind, ChatGptLoginStateDto, ContextBreakdownDto,
    ContextSettingsDto, DefinitionsDiagnosticsDto, EffortDto, FileChangeDto, FileDiffResponse,
    HostInfoDto, LoopDto, MemoryDto, ModelItemDto, PackDto, ProviderItemDto, ProvidersResponse,
    QuestionRequestDto, ReloadResponse, RewindScopeDto, RouteDto, SandboxSettingsDto,
    SessionDetailDto, SessionSettingsDto, SessionStatus, SessionSummaryDto, SnapshotResponse,
    ThemeOverrideDto, TranscriptEntryDto, TurnMetricsDto, UpdateSettingsRequest, UsageDto,
    ViewEvent, WorkspacePreviewResponse, WorkspaceSearchResponse,
};

mod apply;
mod commands;
mod completion;
mod digest;
mod format;
mod forms;
mod misc;
mod providers;
mod runs;
mod sessions;
mod settings;
mod types;
mod views;
pub use apply::*;
pub use commands::*;
pub use completion::*;
pub use digest::*;
pub use format::*;
pub use forms::*;
pub use misc::*;
pub use providers::*;
pub use runs::*;
pub use sessions::*;
pub use settings::*;
pub use types::*;
pub use views::*;

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_web_protocol::{ApprovalModeDto, BackendKind, SessionStatus, UsageDto};

    fn user(id: &str, turn: Option<&str>, text: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::User {
            id: id.to_owned(),
            turn_id: turn.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    fn tool(id: &str, name: &str, status: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            args: "{}".to_owned(),
            status: status.to_owned(),
            output: None,
            edit_diff: None,
        }
    }

    fn assistant(id: &str, text: &str, reasoning: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::Assistant {
            id: id.to_owned(),
            text: text.to_owned(),
            reasoning: reasoning.to_owned(),
        }
    }

    #[test]
    fn sidebar_blocks_start_open() {
        let mut state = AppState::default();
        assert!(state.sidebar_diagnose_open);
        assert!(state.sidebar_changes_open);

        toggle_diagnose(&mut state);
        assert!(!state.sidebar_diagnose_open);
        toggle_changes(&mut state);
        assert!(!state.sidebar_changes_open);
    }

    #[test]
    fn activity_summary_uses_tui_phrasing() {
        let running = activity_summary(&[("code-read_file", true), ("code-grep", false)], 0);
        assert_eq!(running, "1 tool · Reading 1");
        let done = activity_summary(
            &[
                ("code-shell", false),
                ("code-read_file", false),
                ("code-edit_file", false),
                ("subagent-run", false),
                ("lsp-get_diagnostics", false),
            ],
            2,
        );
        assert_eq!(
            done,
            "1 command · 1 tool · 1 read · 1 edit · 1 agent · 2 thoughts"
        );
    }

    #[test]
    fn digest_groups_contiguous_activity_before_visible_messages() {
        let entries = vec![
            user("u1", Some("t1"), "first"),
            tool("c1", "code-read_file", "succeeded"),
            tool("c2", "code-edit_file", "running"),
            assistant("a1", "", "thinking"),
            assistant("a2", "visible answer", "some reasoning"),
            user("u2", Some("t2"), "second"),
            tool("c3", "code-shell", "succeeded"),
        ];
        let blocks = group_transcript_blocks(&entries);
        let kinds: Vec<&str> = blocks
            .iter()
            .map(|b| match b {
                TranscriptBlock::User { .. } => "user",
                TranscriptBlock::ActivityDigest { .. } => "digest",
                TranscriptBlock::Assistant(_) => "assistant",
                TranscriptBlock::System(_) => "system",
                TranscriptBlock::Error(_) => "error",
                TranscriptBlock::Question(_) => "question",
            })
            .collect();
        assert_eq!(kinds, ["user", "digest", "assistant", "user", "digest"]);
        match &blocks[1] {
            TranscriptBlock::ActivityDigest { items, .. } => {
                assert_eq!(items.len(), 4);
                assert!(matches!(items[0], DigestItem::ToolCall(_)));
                assert!(matches!(items[1], DigestItem::ToolCall(_)));
                assert!(matches!(items[2], DigestItem::Reasoning(_)));
                assert!(matches!(items[3], DigestItem::Reasoning(_)));
            }
            other => panic!("expected digest, got {other:?}"),
        }
    }

    #[test]
    fn digest_splits_runs_between_assistant_messages_and_folds_subagents() {
        let entries = vec![
            user("u1", Some("t1"), "fix it"),
            assistant("a0", "I do ...", "t0"),
            tool("c1", "code-grep", "succeeded"),
            tool("c2", "code-grep", "succeeded"),
            assistant("a1", "", "t1"),
            assistant("a2", "", "t2"),
            assistant("a3", "", "t3"),
            assistant("a4", "", "t4"),
            assistant("a5", "....", "t5"),
            TranscriptEntryDto::Subagent {
                id: "sub1".to_owned(),
                name: None,
                agent: "explore".to_owned(),
                status: "succeeded".to_owned(),
                report: None,
                child_session_id: None,
                model: None,
                approval_mode: "ask".to_owned(),
                steps_used: 0,
                max_steps: 0,
                usage: UsageDto::default(),
                est_cost_usd: None,
                active_tool: None,
            },
        ];
        let blocks = group_transcript_blocks(&entries);
        let kinds: Vec<&str> = blocks
            .iter()
            .map(|b| match b {
                TranscriptBlock::User { .. } => "user",
                TranscriptBlock::ActivityDigest { .. } => "digest",
                TranscriptBlock::Assistant(_) => "assistant",
                TranscriptBlock::System(_) => "system",
                TranscriptBlock::Error(_) => "error",
                TranscriptBlock::Question(_) => "question",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user",
                "digest",
                "assistant",
                "digest",
                "assistant",
                "digest"
            ]
        );
        match (&blocks[1], &blocks[3], &blocks[5]) {
            (
                TranscriptBlock::ActivityDigest { items: first, .. },
                TranscriptBlock::ActivityDigest { items: second, .. },
                TranscriptBlock::ActivityDigest { items: third, .. },
            ) => {
                assert_eq!(first.len(), 1);
                assert!(matches!(first[0], DigestItem::Reasoning(_)));
                assert_eq!(second.len(), 7);
                assert_eq!(third.len(), 1);
                assert!(matches!(third[0], DigestItem::Subagent(_)));
            }
            _ => panic!("unexpected blocks"),
        }
    }

    fn state_with_tab() -> AppState {
        let mut state = AppState {
            active_session_id: Some("ses-1".to_owned()),
            ..AppState::default()
        };
        state.sessions.push(SessionSummaryDto {
            id: "ses-1".to_owned(),
            title: Some("demo".to_owned()),
            backend: BackendKind::Native,
            provider: "openai".to_owned(),
            model: "gpt".to_owned(),
            approval_mode: ApprovalModeDto::Ask,
            status: SessionStatus::Idle,
            active_turn_id: None,
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
            transient: false,
            shell_runs: Vec::new(),
        });
        state
            .session_tabs
            .entry("ses-1".to_owned())
            .or_default()
            .entries
            .push(user("u1", Some("t1"), "hello"));
        state
    }

    #[test]
    fn sidebar_drops_transient_sessions_but_tabs_keep_them() {
        let mut state = state_with_tab();
        let mut btw = state.sessions[0].clone();
        btw.id = "ses-btw".to_owned();
        btw.title = Some("btw".to_owned());
        btw.transient = true;
        state.sessions.push(btw);
        state.tab_order.push("ses-btw".to_owned());

        assert_eq!(state.sidebar_sessions().len(), 1);
        assert_eq!(state.visible_sessions().len(), 2);
    }

    #[test]
    fn transient_session_rejects_mode_changes() {
        let mut state = state_with_tab();
        state.sessions[0].transient = true;

        set_approval_mode(&mut state, ApprovalModeDto::Auto);

        assert_eq!(state.status_notice.as_deref(), Some("btw is read-only."));
    }

    #[test]
    fn clear_only_resets_the_local_view() {
        let mut state = state_with_tab();
        state.session_tabs.get_mut("ses-1").unwrap().usage = UsageDto::default();
        execute_slash_command(&mut state, "/clear");
        assert!(
            state
                .session_tabs
                .get("ses-1")
                .is_some_and(|t| t.entries.is_empty())
        );
        assert!(
            state
                .status_notice
                .as_deref()
                .is_some_and(|n| n.contains("Cleared this view"))
        );
        assert!(state.error_banner.is_none());
    }

    #[test]
    fn fork_and_rewind_refuse_while_busy() {
        let mut state = state_with_tab();
        state.sessions[0].status = SessionStatus::Running;
        execute_slash_command(&mut state, "/fork");
        assert!(state.error_banner.is_some());
        state.error_banner = None;
        execute_slash_command(&mut state, "/rewind");
        assert!(state.error_banner.is_some());
    }

    #[test]
    fn palette_actions_cover_non_command_actions() {
        let keys: Vec<&str> = PALETTE_ACTIONS.iter().map(|a| a.key).collect();
        assert!(keys.contains(&"copy-last"));
        assert!(keys.contains(&"next-tab"));
        assert!(keys.contains(&"cycle-mode"));
        let filtered = filtered_palette_actions("copy");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].key, "copy-last");
    }

    #[test]
    fn tool_summary_and_formatting_helpers_produce_concise_labels() {
        assert_eq!(
            summarize_tool_args("code-read_file", r#"{"path":"src/main.rs","limit":10}"#),
            "src/main.rs"
        );
        assert_eq!(
            summarize_tool_args(
                "code-shell",
                r#"{"command":"cargo test --workspace\necho ok"}"#
            ),
            "cargo test --workspace"
        );
        assert_eq!(
            summarize_tool_args("code-grep", r#"{"pattern":"AppState"}"#),
            "AppState"
        );
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_450), "12.4k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
        assert_eq!(format_duration_ms(450), "450ms");
        assert_eq!(format_duration_ms(4_250), "4.2s");
        assert_eq!(format_duration_ms(125_000), "2m 05s");
    }

    fn subagent(id: &str, agent: &str, status: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::Subagent {
            id: id.to_owned(),
            name: None,
            agent: agent.to_owned(),
            status: status.to_owned(),
            report: None,
            child_session_id: Some(format!("{id}-child")),
            model: None,
            approval_mode: "ask".to_owned(),
            steps_used: 2,
            max_steps: 32,
            usage: UsageDto::default(),
            est_cost_usd: None,
            active_tool: None,
        }
    }

    fn shell_run(id: &str, command: &str) -> meerai_web_protocol::ShellRunDto {
        meerai_web_protocol::ShellRunDto {
            id: id.to_owned(),
            command: command.to_owned(),
            started_at_ms: 1_000,
        }
    }

    #[test]
    fn running_counts_include_only_running_work() {
        let mut state = state_with_tab();
        let tab = state.session_tabs.get_mut("ses-1").unwrap();
        tab.entries.push(subagent("sa-1", "explore", "running"));
        tab.entries.push(subagent("sa-2", "plan", "proposed"));
        tab.entries.push(subagent("sa-3", "build", "completed"));
        state.sessions[0].shell_runs = vec![
            shell_run("run-1", "cargo test"),
            shell_run("run-2", "cargo clippy"),
        ];
        assert_eq!(running_subagent_count(&state), 2);
        assert_eq!(running_shell_count(&state), 2);
        assert_eq!(running_subagent_rows(&state).len(), 2);

        state.sessions[0].shell_runs.clear();
        assert_eq!(running_shell_count(&state), 0);
    }

    #[test]
    fn runs_navigation_opens_rows_and_returns_to_the_list() {
        let mut state = state_with_tab();
        state.sessions[0].shell_runs = vec![shell_run("run-1", "cargo test")];

        open_runs(&mut state, RunKindDto::Shells);
        assert!(matches!(
            &state.modal,
            Some(ActiveModal::Runs {
                kind: RunKindDto::Shells,
                ..
            })
        ));

        open_run_row(&mut state, RunKindDto::Shells, 0);
        match &state.modal {
            Some(ActiveModal::ShellRun {
                run_id,
                command,
                started_at_ms,
                from_runs,
            }) => {
                assert_eq!(run_id, "run-1");
                assert_eq!(command, "cargo test");
                assert_eq!(*started_at_ms, 1_000);
                assert_eq!(*from_runs, Some(RunKindDto::Shells));
            }
            other => panic!("unexpected modal: {other:?}"),
        }

        back_to_runs(&mut state, RunKindDto::Shells);
        assert!(matches!(
            &state.modal,
            Some(ActiveModal::Runs {
                kind: RunKindDto::Shells,
                ..
            })
        ));
    }

    #[test]
    fn agent_rows_open_the_subagent_overlay() {
        let mut state = state_with_tab();
        state
            .session_tabs
            .get_mut("ses-1")
            .unwrap()
            .entries
            .push(subagent("sa-1", "explore", "running"));

        open_run_row(&mut state, RunKindDto::Agents, 0);
        match &state.modal {
            Some(ActiveModal::SubagentOverlay {
                entry_id,
                agent,
                status,
                child_session_id,
                from_runs,
                ..
            }) => {
                assert_eq!(entry_id, "sa-1");
                assert_eq!(agent, "explore");
                assert_eq!(status, "running");
                assert_eq!(child_session_id.as_deref(), Some("sa-1-child"));
                assert_eq!(*from_runs, Some(RunKindDto::Agents));
            }
            other => panic!("unexpected modal: {other:?}"),
        }
    }

    #[test]
    fn apply_shell_output_records_result_by_run_id() {
        let mut state = state_with_tab();
        apply_shell_output(&mut state, "ses-1", "run-1", Ok("done".to_owned()));
        assert_eq!(
            state.shell_outputs.get("run-1"),
            Some(&Ok("done".to_owned()))
        );

        apply_shell_output(&mut state, "ses-1", "run-2", Err("boom".to_owned()));
        assert_eq!(
            state.shell_outputs.get("run-2"),
            Some(&Err("boom".to_owned()))
        );
    }

    #[test]
    fn composer_submits_prompts_while_a_shell_runs() {
        let mut state = state_with_tab();
        state.sessions[0].user_shell_running = true;
        state.draft.replace_text("next task");
        assert!(state.can_submit_composer());
        submit_or_queue_composer(&mut state);
        let tab = state.active_tab().unwrap();
        assert!(tab.pending_queue.is_empty());
        assert_eq!(tab.pending_submit.as_deref(), Some("next task"));
    }
}
