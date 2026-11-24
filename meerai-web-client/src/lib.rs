pub mod state;
pub mod text;
pub mod theme;
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
pub mod pathlink;
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
        AgentInfoDto, ApprovalModeDto, BackendKind, FileChangeDto, HostInfoDto, ModelItemDto,
        ProvidersResponse, SessionDetailDto, SessionStatus, SessionSummaryDto, SettingsSnapshotDto,
        SkillInfoDto, SnapshotResponse, TranscriptEntryDto, TurnMetricsDto, UsageDto, ViewEvent,
    };

    use super::*;
    use crate::state::{
        accept_completion, apply_opened_session, apply_session_detail, cycle_approval_mode,
        detach_tab_view, dispatch_prompt_text, drain_next_queued_if_idle, execute_slash_command,
        extract_mentions, format_ratio, group_transcript_blocks, mention_segments, move_tab_to,
        open_diff_modal, remove_mention_token, reorder_active_tab, resolved_mention_chips,
        select_tab_index, submit_or_queue_composer, update_completion,
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
            shell_runs: Vec::new(),
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
                context: None,
                goal: None,
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
        execute_slash_command(&mut state, "/diagnose");
        assert!(state.sidebar_open);
        assert!(state.sidebar_diagnose_open);
        execute_slash_command(&mut state, "/diagnose");
        assert!(!state.sidebar_diagnose_open);
        assert_eq!(format_ratio(150, 200), "75.0% 150/200");
        assert_eq!(format_ratio(5, 0), "n/a");
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
        let mut settings = SettingsSnapshotDto::default();
        settings.raw_theme.color_scheme = Some("dark".to_owned());
        let info = HostInfoDto {
            settings,
            ..HostInfoDto::default()
        };
        let mut form = SettingsFormState::from_host(&info);
        assert_eq!(form.color_scheme.as_deref(), Some("dark"));
        assert_eq!(
            form.to_request()
                .unwrap()
                .theme
                .unwrap()
                .color_scheme
                .as_deref(),
            Some("dark")
        );

        form.theme_fields
            .get_mut("accent")
            .unwrap()
            .replace_text("not-a-color");
        assert!(form.to_request().is_err());

        form.theme_fields
            .get_mut("accent")
            .unwrap()
            .replace_text("rebeccapurple");
        assert!(form.to_request().is_ok());

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

    #[test]
    fn opened_session_hydrates_detail_and_clears_on_reset() {
        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Idle));
        assert_eq!(state.active_session_id.as_deref(), Some("s-1"));
        assert!(!state.session_tabs.get("s-1").unwrap().hydrated);

        let detail = SessionDetailDto {
            summary: summary("s-1", SessionStatus::Idle),
            entries: vec![TranscriptEntryDto::User {
                id: "u1".to_owned(),
                turn_id: Some("t1".to_owned()),
                text: "loaded".to_owned(),
            }],
            streaming_item_id: None,
            streaming_text: String::new(),
            streaming_reasoning: String::new(),
            usage: UsageDto {
                prompt_tokens: Some(120),
                completion_tokens: Some(45),
                total_tokens: Some(165),
                cached_tokens: Some(50),
                cache_creation_tokens: None,
                est_cost_usd: Some(0.002),
            },
            metrics: TurnMetricsDto {
                last_turn_ms: Some(1200),
                tokens_per_sec: Some(37.5),
                ..TurnMetricsDto::default()
            },
            pending_approvals: Vec::new(),
            pending_questions: Vec::new(),
            context: None,
            goal: None,
        };
        apply_session_detail(&mut state, detail);
        let tab = state.session_tabs.get("s-1").unwrap();
        assert!(tab.hydrated);
        assert_eq!(tab.entries.len(), 1);
        assert_eq!(tab.usage.total_tokens, Some(165));
        assert_eq!(tab.metrics.last_turn_ms, Some(1200));

        apply_view_event(
            &mut state,
            ViewEvent::SessionReset {
                session_id: "s-1".to_owned(),
            },
        );
        let tab = state.session_tabs.get("s-1").unwrap();
        assert!(tab.entries.is_empty());
        assert_eq!(tab.usage.total_tokens, None);
        assert_eq!(tab.metrics.last_turn_ms, None);
    }

    #[test]
    fn move_tab_to_reorders_and_background_queue_drains_for_target_session() {
        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Idle));
        apply_opened_session(&mut state, summary("s-2", SessionStatus::Idle));
        apply_opened_session(&mut state, summary("s-3", SessionStatus::Idle));
        assert_eq!(state.tab_order, vec!["s-1", "s-2", "s-3"]);

        move_tab_to(&mut state, "s-1", "s-3");
        assert_eq!(state.tab_order, vec!["s-2", "s-3", "s-1"]);

        state
            .session_tabs
            .get_mut("s-1")
            .unwrap()
            .pending_queue
            .push_back("active prompt".to_owned());
        state
            .session_tabs
            .get_mut("s-2")
            .unwrap()
            .pending_queue
            .push_back("bg prompt".to_owned());
        state.active_session_id = Some("s-1".to_owned());
        drain_next_queued_if_idle(&mut state, "s-2");
        assert!(state.sending);
        assert!(
            state
                .session_tabs
                .get("s-2")
                .unwrap()
                .pending_queue
                .is_empty()
        );
        assert_eq!(
            state.session_tabs.get("s-1").unwrap().pending_queue.len(),
            1
        );
    }

    #[test]
    fn resolved_mention_chips_resolve_and_remove_tokens() {
        let mut host = HostInfoDto::default();
        host.agents.push(AgentInfoDto {
            id: "reviewer".to_owned(),
            source: ".agents".to_owned(),
            description: Some("Reviews code diffs".to_owned()),
            model: None,
        });
        host.skills.push(SkillInfoDto {
            id: "rust".to_owned(),
            source: ".agents".to_owned(),
            description: Some("Rust conventions".to_owned()),
        });
        let chips = resolved_mention_chips("check @agent/reviewer and @skill/rust @unknown", &host);
        assert_eq!(
            chips,
            vec![
                (
                    MentionCategory::Agent,
                    "reviewer".to_owned(),
                    "@agent/reviewer".to_owned(),
                ),
                (
                    MentionCategory::Skill,
                    "rust".to_owned(),
                    "@skill/rust".to_owned(),
                ),
            ]
        );
        let next = remove_mention_token("check @agent/reviewer and @skill/rust", "@agent/reviewer");
        assert_eq!(next, "check and @skill/rust");
    }

    #[test]
    fn mention_extraction_cleans_known_tokens_and_keeps_unknown_ones() {
        let mut host = HostInfoDto::default();
        host.skills.push(SkillInfoDto {
            id: "orchestrate".to_owned(),
            source: ".agents".to_owned(),
            description: None,
        });

        let mentions = extract_mentions("@skill/orchestrate\nreview   this", &host);
        assert_eq!(mentions.skills, vec!["orchestrate".to_owned()]);
        assert_eq!(mentions.cleaned, "review this");
        assert!(!mentions.mention_only);

        let unknown = extract_mentions("@skill/missing keep", &host);
        assert!(unknown.skills.is_empty());
        assert_eq!(unknown.cleaned, "@skill/missing keep");
    }

    #[test]
    fn the_goal_skill_accepts_a_bare_at_alias() {
        let mut host = HostInfoDto::default();
        host.skills.push(SkillInfoDto {
            id: "goal".to_owned(),
            source: "built-in".to_owned(),
            description: Some("Run a goal loop".to_owned()),
        });

        let mentions = extract_mentions("@goal ship the fix", &host);
        assert_eq!(mentions.skills, vec!["goal".to_owned()]);
        assert_eq!(mentions.cleaned, "ship the fix");
        assert!(!mentions.mention_only);

        let chips = resolved_mention_chips("@goal ship the fix", &host);
        assert_eq!(
            chips,
            vec![(
                MentionCategory::Skill,
                "goal".to_owned(),
                "@goal".to_owned(),
            )]
        );
        let segments = mention_segments("@goal ship", &host);
        assert_eq!(segments[0].category, Some(MentionCategory::Skill));
        assert_eq!(segments[0].text, "goal");

        let unknown = extract_mentions("@goalkeeper keep", &host);
        assert!(unknown.skills.is_empty());
        assert_eq!(unknown.cleaned, "@goalkeeper keep");

        let items = crate::state::mention_completion_items("goal", &host, &[]);
        assert!(items.iter().any(|item| item.insert == "@goal"));
    }

    #[test]
    fn mention_segments_split_known_tokens_and_preserve_plain_text() {
        let mut host = HostInfoDto::default();
        host.agents.push(AgentInfoDto {
            id: "reviewer".to_owned(),
            source: ".agents".to_owned(),
            description: None,
            model: None,
        });

        let segments = mention_segments("go @agent/reviewer now", &host);
        assert_eq!(
            segments,
            vec![
                crate::state::MentionSegment {
                    category: None,
                    text: "go ".to_owned(),
                },
                crate::state::MentionSegment {
                    category: Some(MentionCategory::Agent),
                    text: "reviewer".to_owned(),
                },
                crate::state::MentionSegment {
                    category: None,
                    text: " now".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn dispatch_prompt_cleans_known_mentions_from_the_model_prompt() {
        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        state.host_info.skills.push(SkillInfoDto {
            id: "orchestrate".to_owned(),
            source: ".agents".to_owned(),
            description: None,
        });
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Idle));

        dispatch_prompt_text(&mut state, "@skill/orchestrate review this".to_owned());
        assert_eq!(
            state.active_tab().unwrap().pending_submit.as_deref(),
            Some("review this")
        );

        let mut running = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        running.host_info = state.host_info.clone();
        apply_opened_session(&mut running, summary("s-2", SessionStatus::Running));
        dispatch_prompt_text(&mut running, "@skill/orchestrate review this".to_owned());
        let queued: Vec<String> = running
            .active_tab()
            .unwrap()
            .pending_queue
            .iter()
            .cloned()
            .collect();
        assert_eq!(queued, vec!["review this".to_owned()]);
    }

    #[test]
    fn steer_draft_when_busy_and_rewind_syncs_tab_draft() {
        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Running));
        state.draft.replace_text("steer this draft");
        crate::state::steer_or_send_queued(&mut state, 0);
        assert!(state.draft.text.is_empty());
        assert_eq!(
            state.prompt_history.last().map(String::as_str),
            Some("steer this draft")
        );

        apply_view_event(
            &mut state,
            ViewEvent::SessionRewound {
                session_id: "s-1".to_owned(),
                turn_id: "t-1".to_owned(),
                scope: meerai_web_protocol::RewindScopeDto::Both,
                restored: Vec::new(),
                error: None,
                entries: Some(Vec::new()),
                restored_prompt: Some("restored draft".to_owned()),
            },
        );
        assert_eq!(state.draft.text, "restored draft");
        assert_eq!(state.active_tab().unwrap().draft_text, "restored draft");

        let clear_desc = crate::state::SLASH_COMMANDS
            .iter()
            .find(|c| c.cmd == "/clear")
            .unwrap()
            .desc;
        let reset_desc = crate::state::SLASH_COMMANDS
            .iter()
            .find(|c| c.cmd == "/reset")
            .unwrap()
            .desc;
        assert_ne!(clear_desc, reset_desc);

        let mut send_state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        apply_opened_session(&mut send_state, summary("s-send", SessionStatus::Idle));
        send_state.draft.replace_text("pending hello");
        submit_or_queue_composer(&mut send_state);
        assert_eq!(
            send_state.active_tab().unwrap().pending_submit.as_deref(),
            Some("pending hello")
        );
        apply_view_event(
            &mut send_state,
            ViewEvent::UserPromptSubmitted {
                session_id: "s-send".to_owned(),
                entry_id: "u-send".to_owned(),
                text: "pending hello".to_owned(),
            },
        );
        assert!(send_state.active_tab().unwrap().pending_submit.is_none());
    }

    fn sample_providers() -> ProvidersResponse {
        use meerai_web_protocol::{AuthKindDto, ProviderItemDto};
        let mut recent = BTreeMap::new();
        recent.insert("deepseek".to_owned(), vec!["deepseek-chat".to_owned()]);
        recent.insert(
            "anthropic".to_owned(),
            vec!["claude-opus-4-6".to_owned(), "claude-sonnet-4-5".to_owned()],
        );
        ProvidersResponse {
            default_provider: Some("openai".to_owned()),
            providers: vec![
                ProviderItemDto {
                    id: "deepseek".to_owned(),
                    display_name: "DeepSeek".to_owned(),
                    adapter: "deepseek".to_owned(),
                    auth_kind: AuthKindDto::ApiKey,
                    configured: false,
                    status_badge: "not set".to_owned(),
                    env_vars: vec!["DEEPSEEK_API_KEY".to_owned()],
                    default_model: None,
                    base_url: None,
                    is_custom: false,
                },
                ProviderItemDto {
                    id: "anthropic".to_owned(),
                    display_name: "Anthropic".to_owned(),
                    adapter: "anthropic".to_owned(),
                    auth_kind: AuthKindDto::ApiKey,
                    configured: true,
                    status_badge: "saved".to_owned(),
                    env_vars: vec!["ANTHROPIC_API_KEY".to_owned()],
                    default_model: None,
                    base_url: None,
                    is_custom: false,
                },
                ProviderItemDto {
                    id: "openai".to_owned(),
                    display_name: "OpenAI".to_owned(),
                    adapter: "openai_resp".to_owned(),
                    auth_kind: AuthKindDto::Either,
                    configured: true,
                    status_badge: "plan".to_owned(),
                    env_vars: vec!["OPENAI_API_KEY".to_owned()],
                    default_model: Some("gpt-5".to_owned()),
                    base_url: None,
                    is_custom: false,
                },
                ProviderItemDto {
                    id: "ollama".to_owned(),
                    display_name: "Ollama (Local)".to_owned(),
                    adapter: "ollama".to_owned(),
                    auth_kind: AuthKindDto::None,
                    configured: true,
                    status_badge: "local".to_owned(),
                    env_vars: Vec::new(),
                    default_model: None,
                    base_url: Some("http://localhost:11434/v1".to_owned()),
                    is_custom: false,
                },
            ],
            models: vec![
                ModelItemDto {
                    id: "claude-sonnet-4-5".to_owned(),
                    provider: "anthropic".to_owned(),
                    display_name: "claude-sonnet-4-5".to_owned(),
                    context_window_tokens: Some(200_000),
                    max_output_tokens: None,
                    supports_reasoning: true,
                    supports_vision: false,
                    supports_tools: true,
                    is_custom: false,
                },
                ModelItemDto {
                    id: "gpt-5".to_owned(),
                    provider: "openai".to_owned(),
                    display_name: "gpt-5".to_owned(),
                    context_window_tokens: Some(400_000),
                    max_output_tokens: None,
                    supports_reasoning: true,
                    supports_vision: false,
                    supports_tools: true,
                    is_custom: false,
                },
            ],
            recent_models: recent,
        }
    }

    #[test]
    fn model_picker_filters_ranks_and_completes() {
        use crate::state::{
            ActiveModal, ProviderModalReturn, complete_model_filter, filtered_models_sorted,
            open_model_input_for,
        };
        use meerai_web_protocol::ModelItemDto;

        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Idle));
        state.providers = Some(sample_providers());
        state.providers.as_mut().unwrap().models.push(ModelItemDto {
            id: "opus-mini".to_owned(),
            provider: "ollama".to_owned(),
            display_name: "opus-mini".to_owned(),
            context_window_tokens: None,
            max_output_tokens: None,
            supports_reasoning: false,
            supports_vision: false,
            supports_tools: true,
            is_custom: false,
        });

        let filtered = filtered_models_sorted(state.providers.as_ref(), None, None, "opus");
        let ids: Vec<&str> = filtered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["opus-mini", "claude-opus-4-6"]);

        assert!(filtered_models_sorted(state.providers.as_ref(), None, None, "zzz").is_empty());

        open_model_input_for(
            &mut state,
            Some("anthropic".to_owned()),
            ProviderModalReturn::Close,
        );
        complete_model_filter(&mut state, "claude-opus-4-6");
        match &state.modal {
            Some(ActiveModal::ModelInput {
                filter, selected, ..
            }) => {
                assert_eq!(filter.text, "claude-opus-4-6");
                assert_eq!(*selected, 0);
            }
            _ => panic!("model dialog closed"),
        }
    }

    #[test]
    fn provider_and_model_pickers_sort_include_recent_and_avoid_connection_loop() {
        use crate::state::{
            ProviderModalReturn, activate_provider_from_modal, custom_model_candidate,
            filtered_models_sorted, filtered_providers_sorted, is_removable_credential_badge,
            open_provider_credential_update, open_provider_model_select, open_settings,
            resolve_model_input_target, return_from_provider_modal,
        };

        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Idle));
        state.providers = Some(sample_providers());

        let sorted_providers =
            filtered_providers_sorted(state.providers.as_ref(), Some("anthropic"), "");
        let provider_ids: Vec<&str> = sorted_providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            provider_ids,
            vec!["anthropic", "openai", "ollama", "deepseek"]
        );

        let all_models =
            filtered_models_sorted(state.providers.as_ref(), None, Some("anthropic"), "");
        let pairs: Vec<(&str, &str)> = all_models
            .iter()
            .map(|m| (m.provider.as_str(), m.id.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("anthropic", "claude-opus-4-6"),
                ("anthropic", "claude-sonnet-4-5"),
                ("openai", "gpt-5"),
                ("deepseek", "deepseek-chat"),
            ]
        );

        assert_eq!(
            resolve_model_input_target(&state, Some("deepseek"), "deepseek-reasoner"),
            Some(("deepseek".to_owned(), "deepseek-reasoner".to_owned()))
        );
        assert_eq!(
            custom_model_candidate(&state, Some("anthropic"), "claude-sonnet-4-5", &all_models),
            None
        );
        assert_eq!(
            custom_model_candidate(&state, Some("deepseek"), "deepseek-reasoner", &all_models),
            Some(("deepseek".to_owned(), "deepseek-reasoner".to_owned()))
        );

        let openai = state
            .providers
            .as_ref()
            .unwrap()
            .providers
            .iter()
            .find(|p| p.id == "openai")
            .unwrap()
            .clone();
        activate_provider_from_modal(
            &mut state,
            openai.clone(),
            ProviderModalReturn::SettingsConnections,
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::ProviderConnection { ref provider_id, .. }) if provider_id == "openai"
        ));

        open_provider_model_select(
            &mut state,
            "openai".to_owned(),
            ProviderModalReturn::SettingsConnections,
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::ModelInput {
                provider_id: Some(ref pid),
                return_to: ProviderModalReturn::SettingsConnections,
                ..
            }) if pid == "openai"
        ));

        open_provider_credential_update(
            &mut state,
            &openai,
            ProviderModalReturn::SettingsConnections,
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::AuthMethodSelect {
                ref provider_id,
                return_to: ProviderModalReturn::SettingsConnections,
                ..
            }) if provider_id == "openai"
        ));

        return_from_provider_modal(&mut state, ProviderModalReturn::SettingsConnections);
        assert!(matches!(
            state.modal,
            Some(ActiveModal::Settings(ref form)) if form.section == SettingsSection::Connections
        ));

        open_settings(&mut state, None);
        assert!(matches!(
            state.modal,
            Some(ActiveModal::Settings(ref form)) if form.section == SettingsSection::Connections
        ));

        assert!(is_removable_credential_badge("saved"));
        assert!(is_removable_credential_badge("plan"));
        assert!(!is_removable_credential_badge("env:OPENAI_API_KEY"));
        assert!(!is_removable_credential_badge("config"));
        assert!(!is_removable_credential_badge("local"));
    }

    #[test]
    fn file_changes_update_syncs_open_diff_modal() {
        let mut state = AppState::default();
        open_diff_modal(&mut state, None);
        assert!(matches!(
            state.modal,
            Some(ActiveModal::DiffView {
                selected_path: None,
                loading: false,
                ..
            })
        ));

        apply_view_event(
            &mut state,
            ViewEvent::FileChangesUpdated {
                changes: vec![
                    FileChangeDto {
                        status: "M".to_owned(),
                        path: "src/a.rs".to_owned(),
                    },
                    FileChangeDto {
                        status: "A".to_owned(),
                        path: "src/b.rs".to_owned(),
                    },
                ],
            },
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::DiffView {
                selected_path: Some(ref path),
                loading: true,
                ..
            }) if path == "src/a.rs"
        ));

        apply_view_event(
            &mut state,
            ViewEvent::FileChangesUpdated {
                changes: vec![FileChangeDto {
                    status: "A".to_owned(),
                    path: "src/b.rs".to_owned(),
                }],
            },
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::DiffView {
                selected_path: Some(ref path),
                loading: true,
                ..
            }) if path == "src/b.rs"
        ));

        apply_view_event(
            &mut state,
            ViewEvent::FileChangesUpdated {
                changes: Vec::new(),
            },
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::DiffView {
                selected_path: None,
                loading: false,
                ..
            })
        ));
    }

    #[test]
    fn effort_command_follows_the_model_effort_levels() {
        use meerai_web_protocol::EffortDto;

        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        state.host_info.settings.config_writable = true;

        state.host_info.settings.context.effort_levels =
            vec![EffortDto::Low, EffortDto::Medium, EffortDto::High];
        crate::state::set_effort_from_arg(&mut state, "xhigh");
        assert_eq!(
            state.host_info.settings.context.effort,
            Some(EffortDto::High),
            "a three-tier model clamps extra high down to high"
        );

        state.host_info.settings.context.effort_levels = EffortDto::ALL.to_vec();
        crate::state::set_effort_from_arg(&mut state, "xhigh");
        assert_eq!(
            state.host_info.settings.context.effort,
            Some(EffortDto::ExtraHigh)
        );
        assert_eq!(
            crate::state::effort_default_temperature(EffortDto::ExtraHigh),
            0.7
        );
        crate::state::set_effort_from_arg(&mut state, "max");
        assert_eq!(
            state.host_info.settings.context.effort,
            Some(EffortDto::Max)
        );
        assert_eq!(
            crate::state::effort_default_temperature(EffortDto::Max),
            0.9
        );

        state.host_info.settings.context.effort_levels = vec![EffortDto::Low, EffortDto::ExtraHigh];
        crate::state::set_effort_from_arg(&mut state, "medium");
        assert_eq!(
            state.host_info.settings.context.effort,
            Some(EffortDto::Low),
            "an unsupported tier clamps down inside the set"
        );
    }

    #[test]
    fn effort_command_queued_edit_mention_punctuation_and_last_tab_detach() {
        let mut state = AppState {
            connection: ConnectionState::Connected,
            ..AppState::default()
        };
        state.host_info.settings.config_writable = true;
        state.host_info.skills.push(SkillInfoDto {
            id: "orchestrate".to_owned(),
            source: ".agents".to_owned(),
            description: None,
        });
        apply_opened_session(&mut state, summary("s-1", SessionStatus::Running));

        execute_slash_command(&mut state, "/sampling");
        assert!(matches!(
            &state.modal,
            Some(ActiveModal::Settings(form))
                if form.section == crate::state::SettingsSection::Context
        ));
        state.modal = None;
        state.host_info.settings.context.effort = Some(meerai_web_protocol::EffortDto::High);
        execute_slash_command(&mut state, "/effort low");
        assert_eq!(
            state.host_info.settings.context.effort,
            Some(meerai_web_protocol::EffortDto::Low)
        );

        let mentions = extract_mentions("@skill/orchestrate, review this", &state.host_info);
        assert_eq!(mentions.skills, vec!["orchestrate".to_owned()]);
        assert_eq!(mentions.cleaned, ", review this");

        {
            let tab = state.active_tab_mut().unwrap();
            tab.pending_queue.push_back("first".to_owned());
            tab.pending_queue.push_back("second".to_owned());
        }
        crate::state::open_queued_edit(&mut state, 1);
        assert!(matches!(
            state.modal,
            Some(ActiveModal::QueuedEdit { index: 1, .. })
        ));
        if let Some(ActiveModal::QueuedEdit { input, .. }) = &mut state.modal {
            input.replace_text("   ");
        }
        crate::state::commit_queued_edit(&mut state);
        assert!(matches!(
            state.modal,
            Some(ActiveModal::QueuedEdit { error: Some(_), .. })
        ));

        crate::state::remove_queued(&mut state, 0);
        assert!(matches!(
            state.modal,
            Some(ActiveModal::QueuedEdit { index: 0, .. })
        ));
        if let Some(ActiveModal::QueuedEdit { input, .. }) = &mut state.modal {
            input.replace_text("updated second");
        }
        crate::state::commit_queued_edit(&mut state);
        assert!(state.modal.is_none());
        assert_eq!(
            state
                .active_tab()
                .unwrap()
                .pending_queue
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["updated second".to_owned()]
        );

        detach_tab_view(&mut state, "s-1");
        assert_eq!(state.visible_sessions().len(), 1);
        assert_eq!(state.pending_replace_session.as_deref(), Some("s-1"));
        crate::state::apply_created_session(&mut state, summary("s-2", SessionStatus::Idle));
        assert!(state.pending_replace_session.is_none());
        assert_eq!(
            state
                .visible_sessions()
                .into_iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["s-2"]
        );
    }

    #[test]
    fn host_action_results_land_in_the_active_transcript() {
        let mut state = AppState::default();
        crate::state::apply_created_session(&mut state, summary("s-1", SessionStatus::Idle));
        state.active_session_id = Some("s-1".to_owned());

        crate::state::apply_host_action_result(&mut state, "Memories (0):".to_owned());

        let tab = state.session_tabs.get("s-1").unwrap();
        assert!(matches!(
            tab.entries.last(),
            Some(TranscriptEntryDto::System { text, .. }) if text == "Memories (0):"
        ));
    }

    #[test]
    fn host_commands_open_the_workspace_panel() {
        let mut state = AppState::default();
        execute_slash_command(&mut state, "/memory");
        assert!(matches!(
            state.modal,
            Some(ActiveModal::WorkspacePanel {
                tab: crate::state::WorkspacePanelTab::Memory,
                ..
            })
        ));
        state.modal = None;
        execute_slash_command(&mut state, "/context");
        assert!(matches!(
            state.modal,
            Some(ActiveModal::WorkspacePanel {
                tab: crate::state::WorkspacePanelTab::Context,
                ..
            })
        ));
        state.modal = None;
        execute_slash_command(&mut state, "/goal");
        assert!(matches!(
            state.modal,
            Some(ActiveModal::WorkspacePanel {
                tab: crate::state::WorkspacePanelTab::Goal,
                ..
            })
        ));
        state.modal = None;
        execute_slash_command(&mut state, "/subagent");
        assert!(matches!(
            state.modal,
            Some(ActiveModal::WorkspacePanel {
                tab: crate::state::WorkspacePanelTab::Subagent,
                ..
            })
        ));
    }

    #[test]
    fn stream_discarded_clears_the_streaming_tab() {
        let mut state = AppState::default();
        apply_view_event(
            &mut state,
            ViewEvent::SessionOpened {
                session: summary("s1", SessionStatus::Idle),
            },
        );
        apply_view_event(
            &mut state,
            ViewEvent::TextDelta {
                session_id: "s1".to_owned(),
                item_id: "a1".to_owned(),
                delta: "partial".to_owned(),
            },
        );
        assert_eq!(
            state.session_tabs.get("s1").unwrap().streaming_text,
            "partial"
        );

        apply_view_event(
            &mut state,
            ViewEvent::StreamDiscarded {
                session_id: "s1".to_owned(),
            },
        );
        let tab = state.session_tabs.get("s1").unwrap();
        assert!(tab.streaming_item_id.is_none());
        assert!(tab.streaming_text.is_empty());
        assert!(tab.streaming_reasoning.is_empty());
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    #[test]
    fn goal_updates_reach_the_tab_and_the_open_panel() {
        let mut state = AppState {
            active_session_id: Some("s1".to_owned()),
            ..AppState::default()
        };
        let goal = meerai_web_protocol::GoalDto {
            goal: Some("Ship the change".to_owned()),
            items: vec![meerai_web_protocol::GoalItemDto {
                id: 1,
                text: "Fix".to_owned(),
                status: meerai_web_protocol::GoalItemStatusDto::Done,
                ..Default::default()
            }],
            ..Default::default()
        };
        crate::state::open_workspace_panel(&mut state, crate::state::WorkspacePanelTab::Goal);
        apply_view_event(
            &mut state,
            meerai_web_protocol::ViewEvent::GoalUpdated {
                session_id: "s1".to_owned(),
                goal: goal.clone(),
            },
        );
        assert_eq!(
            state
                .session_tabs
                .get("s1")
                .and_then(|tab| tab.goal.clone()),
            Some(goal.clone())
        );
        assert!(matches!(
            state.modal,
            Some(ActiveModal::WorkspacePanel {
                tab: crate::state::WorkspacePanelTab::Goal,
                goal: Some(ref panel),
                ..
            }) if **panel == goal
        ));
    }

    #[test]
    fn panel_data_land_in_the_open_panel() {
        let mut state = AppState::default();
        crate::state::open_workspace_panel(&mut state, crate::state::WorkspacePanelTab::Memory);
        crate::state::apply_memories(
            &mut state,
            meerai_web_protocol::MemoriesResponse {
                memories: vec![meerai_web_protocol::MemoryDto {
                    id: "mem-1".to_owned(),
                    project_root: "/repo".to_owned(),
                    kind: "fact".to_owned(),
                    title: "Use SQLite".to_owned(),
                    body: "state".to_owned(),
                    source_session_id: None,
                    pinned: false,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                }],
            },
        );
        assert!(matches!(
            &state.modal,
            Some(ActiveModal::WorkspacePanel {
                memories,
                loading: false,
                ..
            }) if memories.len() == 1
        ));

        crate::state::open_workspace_panel(&mut state, crate::state::WorkspacePanelTab::Routing);
        crate::state::apply_routes(
            &mut state,
            meerai_web_protocol::RoutesResponse {
                routes: vec![meerai_web_protocol::RouteDto {
                    task_type: "build".to_owned(),
                    provider: "openai".to_owned(),
                    model: "o3".to_owned(),
                    ..Default::default()
                }],
            },
        );
        assert!(matches!(
            &state.modal,
            Some(ActiveModal::WorkspacePanel { routes, .. }) if routes.len() == 1
        ));
    }
}

#[cfg(test)]
mod vault_command_tests {
    use super::*;

    #[test]
    fn vault_reset_opens_a_confirmation_panel() {
        let mut state = AppState::default();
        crate::state::execute_slash_command(&mut state, "/vault-reset");
        assert!(matches!(state.modal, Some(ActiveModal::VaultReset { .. })));
    }
}
