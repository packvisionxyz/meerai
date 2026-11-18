pub mod state;
pub mod text;
pub mod transport;

pub mod store;

pub use state::{
    ActiveModal, AppState, CompletionKind, ConnectionState, EventEffect, MentionCategory,
    SessionTabState, SettingsFormState, SettingsSection, TranscriptBlock, apply_created_session,
    apply_snapshot, apply_view_event,
};

#[cfg(target_arch = "wasm32")]
pub mod highlight;
#[cfg(target_arch = "wasm32")]
pub mod keymap;
#[cfg(target_arch = "wasm32")]
pub mod markdown;
#[cfg(target_arch = "wasm32")]
pub mod theme;
#[cfg(target_arch = "wasm32")]
pub mod views;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    use leptos::prelude::*;

    use wasm_bindgen::JsCast;

    let store = store::StoreHandle::new(AppState::default());
    let transport = transport::TransportClient::connect(store);
    transport::set_current(Some(transport));
    let mount = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("app"))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok());
    if let Some(mount) = mount {
        leptos::mount::mount_to(mount, move || view! { <views::App store=store /> }).forget();
    } else {
        leptos::mount::mount_to_body(move || view! { <views::App store=store /> });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use meerai_web_protocol::{
        AgentInfoDto, ApprovalModeDto, BackendKind, HostInfoDto, ModelItemDto, ProvidersResponse,
        SessionDetailDto, SessionStatus, SessionSummaryDto, SettingsSnapshotDto, SkillInfoDto,
        SnapshotResponse, TranscriptEntryDto, TurnMetricsDto, UsageDto, ViewEvent,
    };

    use super::*;
    use crate::state::{
        accept_completion, cycle_approval_mode, detach_tab_view, execute_slash_command,
        extract_mentions, group_transcript_blocks, reorder_active_tab, select_tab_index,
        submit_or_queue_composer, update_completion,
    };

    fn summary(id: &str, status: SessionStatus) -> SessionSummaryDto {
        SessionSummaryDto {
            id: id.to_owned(),
            title: Some("Test session".to_owned()),
            backend: BackendKind::Native,
            provider: "anthropic".to_owned(),
            model: "claude-sonnet".to_owned(),
            approval_mode: ApprovalModeDto::Ask,
            status,
            active_turn_id: None,
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
        }
    }

    #[test]
    fn apply_snapshot_populates_active_session_streaming_and_host() {
        let mut state = AppState {
            error_banner: Some("stale".to_owned()),
            ..AppState::default()
        };
        let snapshot = SnapshotResponse {
            event_id: 7,
            active_session_id: Some("s1".to_owned()),
            sessions: vec![
                summary("s1", SessionStatus::Running),
                summary("s2", SessionStatus::Idle),
            ],
            active_session: Some(SessionDetailDto {
                summary: summary("s1", SessionStatus::Running),
                entries: vec![TranscriptEntryDto::User {
                    id: "u1".to_owned(),
                    turn_id: Some("turn-1".to_owned()),
                    text: "hi".to_owned(),
                }],
                streaming_item_id: Some("a1".to_owned()),
                streaming_text: "hel".to_owned(),
                streaming_reasoning: "think".to_owned(),
                pending_approvals: Vec::new(),
                pending_questions: Vec::new(),
                usage: UsageDto::default(),
                metrics: TurnMetricsDto::default(),
            }),
            file_changes: Vec::new(),
            host_info: Some(HostInfoDto {
                workspace_root: "/repo".to_owned(),
                git_branch: Some("main".to_owned()),
                ..HostInfoDto::default()
            }),
        };

        apply_snapshot(&mut state, snapshot);

        assert_eq!(state.connection, ConnectionState::Connected);
        assert_eq!(state.last_event_id, 7);
        assert_eq!(state.active_session_id.as_deref(), Some("s1"));
        assert_eq!(state.sessions.len(), 2);
        assert_eq!(state.host_info.git_branch.as_deref(), Some("main"));
        let tab = state.active_tab().unwrap();
        assert_eq!(tab.entries.len(), 1);
        assert_eq!(tab.streaming_item_id.as_deref(), Some("a1"));
        assert_eq!(tab.streaming_text, "hel");
        assert_eq!(tab.streaming_reasoning, "think");
        assert!(state.error_banner.is_none());
        assert!(state.is_busy());
    }

    #[test]
    fn turn_lifecycle_queues_and_streams_without_clobbering_digest_state() {
        let mut state = AppState::default();
        apply_view_event(
            &mut state,
            ViewEvent::SessionOpened {
                session: summary("s1", SessionStatus::Idle),
            },
        );
        apply_view_event(
            &mut state,
            ViewEvent::TurnStarted {
                session_id: "s1".to_owned(),
                turn_id: "t1".to_owned(),
                model: "claude-opus".to_owned(),
            },
        );
        assert!(state.is_busy());

        state.draft.replace_text("queued follow-up");
        submit_or_queue_composer(&mut state);
        assert_eq!(state.active_tab().unwrap().pending_queue.len(), 1);

        apply_view_event(
            &mut state,
            ViewEvent::EntryCompleted {
                session_id: "s1".to_owned(),
                entry: TranscriptEntryDto::ToolCall {
                    id: "tc1".to_owned(),
                    name: "code-read_file".to_owned(),
                    args: "README.md".to_owned(),
                    status: "completed".to_owned(),
                    output: Some("ok".to_owned()),
                    edit_diff: None,
                },
            },
        );
        let digest_id = {
            let tab = state.active_tab().unwrap();
            let blocks = group_transcript_blocks(&tab.entries);
            assert!(matches!(blocks[0], TranscriptBlock::ActivityDigest { .. }));
            match &blocks[0] {
                TranscriptBlock::ActivityDigest { digest_id, .. } => digest_id.clone(),
                _ => unreachable!(),
            }
        };
        state
            .active_tab_mut()
            .unwrap()
            .expanded_digests
            .insert(digest_id);

        apply_view_event(
            &mut state,
            ViewEvent::TextDelta {
                session_id: "s1".to_owned(),
                item_id: "a1".to_owned(),
                delta: "answer".to_owned(),
            },
        );
        assert!(!state.active_tab().unwrap().expanded_digests.is_empty());

        apply_view_event(
            &mut state,
            ViewEvent::TurnCompleted {
                session_id: "s1".to_owned(),
                turn_id: "t1".to_owned(),
                reason: "completed".to_owned(),
            },
        );
        assert!(state.active_tab().unwrap().pending_queue.is_empty());
        assert!(state.sending);
    }

    #[test]
    fn tabs_reorder_detach_unread_and_title_update() {
        let mut state = AppState::default();
        apply_view_event(
            &mut state,
            ViewEvent::SessionOpened {
                session: summary("s1", SessionStatus::Idle),
            },
        );
        apply_view_event(
            &mut state,
            ViewEvent::SessionOpened {
                session: summary("s2", SessionStatus::WaitingApproval),
            },
        );
        assert_eq!(state.visible_sessions().len(), 2);
        apply_view_event(
            &mut state,
            ViewEvent::TurnStarted {
                session_id: "s2".to_owned(),
                turn_id: "t2".to_owned(),
                model: "m".to_owned(),
            },
        );
        assert!(state.session_tabs.get("s2").unwrap().unread);

        reorder_active_tab(&mut state, 1);
        let order: Vec<&str> = state
            .visible_sessions()
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(order, vec!["s2", "s1"]);

        select_tab_index(&mut state, 0);
        assert_eq!(state.active_session_id.as_deref(), Some("s2"));
        assert!(!state.session_tabs.get("s2").unwrap().unread);
        assert!(state.window_title().contains("M | Test session"));

        detach_tab_view(&mut state, "s2");
        assert_eq!(state.visible_sessions().len(), 1);
        assert_eq!(state.active_session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn slash_commands_completions_and_mentions_work() {
        let mut state = AppState::default();
        state.host_info.agents = vec![AgentInfoDto {
            id: "reviewer".to_owned(),
            source: ".agents".to_owned(),
            description: Some("Review code".to_owned()),
            model: None,
        }];
        state.host_info.skills = vec![SkillInfoDto {
            id: "verify".to_owned(),
            source: ".agents".to_owned(),
            description: Some("Run checks".to_owned()),
        }];
        state.workspace_files = vec!["src/lib.rs".to_owned()];
        state.providers = Some(ProvidersResponse {
            default_provider: Some("anthropic".to_owned()),
            providers: Vec::new(),
            models: vec![ModelItemDto {
                id: "claude-sonnet".to_owned(),
                provider: "anthropic".to_owned(),
                display_name: "Claude Sonnet".to_owned(),
                context_window_tokens: Some(200_000),
                max_output_tokens: None,
                supports_reasoning: true,
                supports_vision: false,
                supports_tools: true,
                is_custom: false,
            }],
            recent_models: BTreeMap::new(),
        });

        state.draft.replace_text("/sea");
        update_completion(&mut state);
        assert_eq!(state.completion.kind, Some(CompletionKind::Command));
        assert!(
            state
                .completion
                .items
                .iter()
                .any(|i| i.insert.starts_with("/search"))
        );
        assert!(accept_completion(&mut state, 0));

        state.draft.replace_text("/model claude");
        update_completion(&mut state);
        assert_eq!(state.completion.kind, Some(CompletionKind::Model));
        assert_eq!(state.completion.items[0].label, "anthropic/claude-sonnet");

        state.draft.replace_text("check @rev");
        update_completion(&mut state);
        assert_eq!(state.completion.kind, Some(CompletionKind::Mention));
        assert_eq!(state.completion.items[0].insert, "@agent/reviewer");
        assert!(accept_completion(&mut state, 0));
        assert_eq!(state.draft.text, "check @agent/reviewer ");

        let m = extract_mentions(
            "@skill/verify @skill/verify @agent/reviewer",
            &state.host_info,
        );
        assert_eq!(m.skills, vec!["verify".to_owned()]);
        assert_eq!(m.agent.as_deref(), Some("reviewer"));
        assert!(m.mention_only);

        execute_slash_command(&mut state, "/sidebar");
        assert!(!state.sidebar_open);
        execute_slash_command(&mut state, "/bogus");
        assert!(
            state
                .error_banner
                .as_deref()
                .unwrap()
                .contains("Unknown command")
        );
        execute_slash_command(&mut state, "/settings");
        assert!(matches!(state.modal, Some(ActiveModal::Settings(_))));
    }

    #[test]
    fn settings_form_validates_hex_and_numeric_bounds() {
        let info = HostInfoDto {
            settings: SettingsSnapshotDto::default(),
            ..HostInfoDto::default()
        };
        let mut form = SettingsFormState::from_host(&info);
        assert!(form.to_request().is_ok());

        form.theme_fields
            .get_mut("accent")
            .unwrap()
            .replace_text("not-a-color");
        assert!(form.to_request().is_err());

        form.theme_fields
            .get_mut("accent")
            .unwrap()
            .replace_text("#2dd4bf");
        form.inputs
            .get_mut("temperature")
            .unwrap()
            .replace_text("3.5");
        assert!(form.to_request().is_err());

        form.inputs
            .get_mut("temperature")
            .unwrap()
            .replace_text("0.7");
        let req = form.to_request().unwrap();
        assert_eq!(req.context.unwrap().temperature, Some(0.7));
    }

    #[test]
    fn cycle_approval_mode_skips_plan_on_acp() {
        let mut state = AppState::default();
        let mut acp = summary("s-acp", SessionStatus::Idle);
        acp.backend = BackendKind::Acp {
            preset: "agy".to_owned(),
            remote_session_id: None,
        };
        apply_view_event(&mut state, ViewEvent::SessionOpened { session: acp });
        cycle_approval_mode(&mut state);
        assert!(state.error_banner.is_none());
    }
}
