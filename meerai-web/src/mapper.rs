use std::collections::BTreeMap;

use http::StatusCode;
use meerai_agents::{
    ApprovalRequest, QuestionAnswer, QuestionPrompt, QuestionRequest, RewindScope,
};
use meerai_common::settings::{
    ContextSettings, SandboxSettings, SessionSettings, Temperature, ThemeSettings,
};
use meerai_core::EffectClass;
use meerai_host::{
    Backend, ChatGptLoginStatus, DiffRowKind, FileChangeEntry, FileDiffView, HostAuthKind,
    HostError, HostInfoSnapshot, HostProvidersSnapshot, HostSettingsUpdate, LiveEntry,
    LiveSessionDetail, LiveSessionSummary, RegistryEventKind, RegistrySnapshot, ResolvedThemeHex,
    SearchMatch, SearchMode, SearchPreview, SessionStatus as HostSessionStatus,
    TurnMetricsSnapshot, UnifiedLine, UnifiedLineKind, UsageStatsSnapshot,
};
use meerai_policy::ApprovalMode;
use meerai_session::replay::{
    ReplayEntry, ToolCallStatus, replay_from_events, session_usage_from_events,
};
use meerai_session::{ApprovalDecision, ApprovalKind, GrantScope, LoggedEvent};
use meerai_web_protocol::{
    AcpPresetDto, AgentInfoDto, ApiError, ApiErrorCode, ApprovalDecisionDto, ApprovalKindDto,
    ApprovalModeDto, ApprovalRequestDto, AuthKindDto, BackendKind, ChatGptLoginStateDto,
    ContextSettingsDto, DiffCellDto, DiffRowDto, DiffRowKindDto, EffectClassDto, FileChangeDto,
    FileDiffResponse, GrantScopeDto, HostInfoDto, McpServerInfoDto, ModelItemDto, ProviderItemDto,
    ProvidersResponse, QuestionAnswerDto, QuestionOptionDto, QuestionPromptDto, QuestionRequestDto,
    RewindScopeDto, SandboxSettingsDto, SearchModeDto, SessionDetailDto, SessionSettingsDto,
    SessionStatus, SessionSummaryDto, SettingsSnapshotDto, SkillInfoDto, SnapshotResponse,
    ThemeOverrideDto, ThemeTokensDto, TranscriptEntryDto, TurnMetricsDto, UnifiedLineDto,
    UnifiedLineKindDto, UpdateSettingsRequest, UsageDto, ViewEvent, WorkspacePreviewLineDto,
    WorkspacePreviewResponse, WorkspaceSearchMatchDto, WorkspaceSearchResponse,
};
use serde::Serialize;

pub fn map_snapshot(
    snapshot: RegistrySnapshot,
    host_info: Option<HostInfoSnapshot>,
) -> SnapshotResponse {
    SnapshotResponse {
        event_id: snapshot.event_id,
        active_session_id: snapshot.active_session_id.map(|id| id.to_string()),
        sessions: snapshot.sessions.into_iter().map(map_summary).collect(),
        active_session: snapshot.active_session.map(map_detail),
        file_changes: snapshot
            .file_changes
            .into_iter()
            .map(map_file_change)
            .collect(),
        host_info: host_info.map(map_host_info),
    }
}

pub fn map_summary(summary: LiveSessionSummary) -> SessionSummaryDto {
    SessionSummaryDto {
        id: summary.session_id.to_string(),
        title: summary.title,
        backend: map_backend(summary.backend),
        provider: summary.provider,
        model: summary.model,
        approval_mode: map_approval_mode(summary.approval_mode),
        status: map_status(summary.status),
        active_turn_id: summary.active_turn_id.map(|id| id.to_string()),
        active_tool: summary.active_tool,
        turn_started_at_ms: summary.turn_started_at_ms,
        user_shell_running: summary.user_shell_running,
    }
}

pub fn map_detail(detail: LiveSessionDetail) -> SessionDetailDto {
    SessionDetailDto {
        summary: map_summary(detail.summary),
        entries: detail.entries.into_iter().map(map_entry).collect(),
        streaming_item_id: detail.streaming_item_id,
        streaming_text: detail.streaming_text,
        streaming_reasoning: detail.streaming_reasoning,
        pending_approvals: detail
            .pending_approvals
            .into_iter()
            .map(map_approval_request)
            .collect(),
        pending_questions: detail
            .pending_questions
            .into_iter()
            .map(map_question_request)
            .collect(),
        usage: map_usage(detail.usage),
        metrics: map_metrics(detail.metrics),
    }
}

pub fn map_entry(entry: LiveEntry) -> TranscriptEntryDto {
    match entry {
        LiveEntry::User { id, turn_id, text } => TranscriptEntryDto::User {
            id,
            turn_id: turn_id.map(|t| t.to_string()),
            text,
        },
        LiveEntry::Assistant {
            id,
            text,
            reasoning,
        } => TranscriptEntryDto::Assistant {
            id,
            text,
            reasoning,
        },
        LiveEntry::System { id, text } => TranscriptEntryDto::System { id, text },
        LiveEntry::Error { id, message } => TranscriptEntryDto::Error { id, message },
        LiveEntry::ToolCall {
            id,
            name,
            args,
            status,
            output,
            edit_diff,
        } => TranscriptEntryDto::ToolCall {
            id,
            name,
            args,
            status,
            output,
            edit_diff: edit_diff.map(|lines| lines.into_iter().map(map_unified_line).collect()),
        },
        LiveEntry::Subagent {
            id,
            agent,
            status,
            report,
            child_session_id,
        } => TranscriptEntryDto::Subagent {
            id,
            agent,
            status,
            report,
            child_session_id,
        },
        LiveEntry::Question {
            id,
            questions,
            answers,
            resolved,
        } => TranscriptEntryDto::Question {
            id,
            questions: questions.into_iter().map(map_question_prompt).collect(),
            answers: answers.map(|list| list.into_iter().map(map_question_answer).collect()),
            resolved,
        },
    }
}

pub fn map_stored_events(events: &[LoggedEvent]) -> (Vec<TranscriptEntryDto>, UsageDto) {
    let entries = replay_from_events(events)
        .into_iter()
        .enumerate()
        .map(|(idx, entry)| match entry {
            ReplayEntry::User { text } => TranscriptEntryDto::User {
                id: format!("stored-{idx}"),
                turn_id: None,
                text,
            },
            ReplayEntry::Assistant { text, reasoning } => TranscriptEntryDto::Assistant {
                id: format!("stored-{idx}"),
                text,
                reasoning,
            },
            ReplayEntry::ToolCall(tc) => TranscriptEntryDto::ToolCall {
                id: tc.call_id,
                edit_diff: meerai_host::diff::tool_edit_diff(
                    &tc.name,
                    &tc.args,
                    tc.output.as_deref(),
                )
                .map(|lines| lines.into_iter().map(map_unified_line).collect()),
                name: tc.name,
                args: tc.args,
                status: match tc.status {
                    ToolCallStatus::Proposed => "proposed",
                    ToolCallStatus::Running => "running",
                    ToolCallStatus::Succeeded => "succeeded",
                    ToolCallStatus::Failed => "failed",
                    ToolCallStatus::Stopped => "stopped",
                    ToolCallStatus::Cancelled => "cancelled",
                }
                .to_string(),
                output: tc.output,
            },
            ReplayEntry::Subagent(sub) => TranscriptEntryDto::Subagent {
                id: sub.id,
                agent: sub.agent,
                status: match sub.status {
                    ToolCallStatus::Proposed => "proposed",
                    ToolCallStatus::Running => "running",
                    ToolCallStatus::Succeeded => "succeeded",
                    ToolCallStatus::Failed => "failed",
                    ToolCallStatus::Stopped => "stopped",
                    ToolCallStatus::Cancelled => "cancelled",
                }
                .to_string(),
                report: sub.report,
                child_session_id: sub.child_session_id,
            },
            ReplayEntry::Question(q) => TranscriptEntryDto::Question {
                id: q.request_id,
                questions: q.questions.into_iter().map(map_question_prompt).collect(),
                answers: q
                    .answers
                    .map(|list| list.into_iter().map(map_question_answer).collect()),
                resolved: q.resolved,
            },
            ReplayEntry::System { text } => TranscriptEntryDto::System {
                id: format!("stored-{idx}"),
                text,
            },
        })
        .collect();
    let usage = match session_usage_from_events(events) {
        Some(u) => UsageDto {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
            cached_tokens: u.cached_tokens,
            est_cost_usd: None,
        },
        None => UsageDto::default(),
    };
    (entries, usage)
}

pub fn map_view_event(kind: RegistryEventKind) -> ViewEvent {
    match kind {
        RegistryEventKind::SessionOpened { session } => ViewEvent::SessionOpened {
            session: map_summary(session),
        },
        RegistryEventKind::SessionUpdated { session } => ViewEvent::SessionUpdated {
            session: map_summary(session),
        },
        RegistryEventKind::SessionActivated { session_id } => ViewEvent::SessionActivated {
            session_id: session_id.to_string(),
        },
        RegistryEventKind::SessionClosed {
            session_id,
            next_active_session_id,
        } => ViewEvent::SessionClosed {
            session_id: session_id.to_string(),
            next_active_session_id: next_active_session_id.map(|id| id.to_string()),
        },
        RegistryEventKind::SessionReset { session_id } => ViewEvent::SessionReset {
            session_id: session_id.to_string(),
        },
        RegistryEventKind::SessionRewound {
            session_id,
            turn_id,
            scope,
            restored,
            error,
            entries,
            restored_prompt,
        } => ViewEvent::SessionRewound {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            scope: map_rewind_scope(scope),
            restored,
            error,
            entries: entries.map(|list| list.into_iter().map(map_entry).collect()),
            restored_prompt,
        },
        RegistryEventKind::TurnStarted {
            session_id,
            turn_id,
            model,
        } => ViewEvent::TurnStarted {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            model,
        },
        RegistryEventKind::UserPromptSubmitted {
            session_id,
            entry_id,
            text,
        } => ViewEvent::UserPromptSubmitted {
            session_id: session_id.to_string(),
            entry_id,
            text,
        },
        RegistryEventKind::TextDelta {
            session_id,
            item_id,
            delta,
        } => ViewEvent::TextDelta {
            session_id: session_id.to_string(),
            item_id,
            delta,
        },
        RegistryEventKind::ReasoningDelta {
            session_id,
            item_id,
            delta,
        } => ViewEvent::ReasoningDelta {
            session_id: session_id.to_string(),
            item_id,
            delta,
        },
        RegistryEventKind::EntryCompleted { session_id, entry } => ViewEvent::EntryCompleted {
            session_id: session_id.to_string(),
            entry: map_entry(entry),
        },
        RegistryEventKind::EntryUpdated { session_id, entry } => ViewEvent::EntryUpdated {
            session_id: session_id.to_string(),
            entry: map_entry(entry),
        },
        RegistryEventKind::ApprovalRequested {
            session_id,
            request,
        } => ViewEvent::ApprovalRequested {
            session_id: session_id.to_string(),
            request: map_approval_request(request),
        },
        RegistryEventKind::ApprovalResolved {
            session_id,
            request_id,
        } => ViewEvent::ApprovalResolved {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
        },
        RegistryEventKind::QuestionAsked {
            session_id,
            request,
        } => ViewEvent::QuestionAsked {
            session_id: session_id.to_string(),
            request: map_question_request(request),
        },
        RegistryEventKind::QuestionResolved {
            session_id,
            request_id,
        } => ViewEvent::QuestionResolved {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
        },
        RegistryEventKind::UsageUpdated {
            session_id,
            usage,
            metrics,
        } => ViewEvent::UsageUpdated {
            session_id: session_id.to_string(),
            usage: map_usage(usage),
            metrics: map_metrics(metrics),
        },
        RegistryEventKind::TurnCompleted {
            session_id,
            turn_id,
            reason,
        } => ViewEvent::TurnCompleted {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            reason: format_turn_end_reason(&reason),
        },
        RegistryEventKind::Error {
            session_id,
            code,
            message,
        } => ViewEvent::Error {
            session_id: session_id.to_string(),
            code: format_error_code(code),
            message,
        },
        RegistryEventKind::FileChangesUpdated { changes } => ViewEvent::FileChangesUpdated {
            changes: changes.into_iter().map(map_file_change).collect(),
        },
        RegistryEventKind::Reloaded {
            host_info,
            warnings,
        } => ViewEvent::Reloaded {
            host_info: map_host_info(host_info),
            warnings,
        },
        RegistryEventKind::ProvidersUpdated => ViewEvent::ProvidersUpdated,
        RegistryEventKind::ChatGptLoginUpdated(status) => match status {
            ChatGptLoginStatus::Started {
                attempt_id,
                provider_id,
                url,
            } => ViewEvent::ChatGptLoginUpdated {
                attempt_id,
                provider_id,
                state: ChatGptLoginStateDto::Started,
                url: Some(url),
                account: None,
                message: None,
            },
            ChatGptLoginStatus::Completed {
                attempt_id,
                provider_id,
                account,
            } => ViewEvent::ChatGptLoginUpdated {
                attempt_id,
                provider_id,
                state: ChatGptLoginStateDto::Completed,
                url: None,
                account,
                message: None,
            },
            ChatGptLoginStatus::Failed {
                attempt_id,
                provider_id,
                message,
            } => ViewEvent::ChatGptLoginUpdated {
                attempt_id,
                provider_id,
                state: ChatGptLoginStateDto::Failed,
                url: None,
                account: None,
                message: Some(message),
            },
            ChatGptLoginStatus::Cancelled {
                attempt_id,
                provider_id,
            } => ViewEvent::ChatGptLoginUpdated {
                attempt_id,
                provider_id,
                state: ChatGptLoginStateDto::Cancelled,
                url: None,
                account: None,
                message: None,
            },
        },
        RegistryEventKind::HostDisconnected => ViewEvent::HostDisconnected,
    }
}

pub fn map_host_error(err: HostError) -> (StatusCode, ApiError) {
    let (status, code) = match &err {
        HostError::EmptyPrompt | HostError::InvalidInput(_) | HostError::NotRunning => {
            (StatusCode::BAD_REQUEST, ApiErrorCode::BadRequest)
        }
        HostError::SessionNotFound(_) => (StatusCode::NOT_FOUND, ApiErrorCode::NotFound),
        HostError::Busy => (StatusCode::CONFLICT, ApiErrorCode::ConflictBusy),
        HostError::StaleRequest(_) | HostError::WorkerClosed => {
            (StatusCode::CONFLICT, ApiErrorCode::ConflictStale)
        }
        HostError::UnsupportedBackend(_) => {
            (StatusCode::BAD_REQUEST, ApiErrorCode::UnsupportedBackend)
        }
    };
    (
        status,
        ApiError {
            code,
            message: err.to_string(),
        },
    )
}

pub fn map_host_info(info: HostInfoSnapshot) -> HostInfoDto {
    HostInfoDto {
        workspace_root: info.workspace_root,
        git_branch: info.git_branch,
        acp_presets: info
            .acp_presets
            .into_iter()
            .map(|p| AcpPresetDto {
                id: p.id,
                label: p.label,
                command: p.command,
            })
            .collect(),
        agents: info
            .agents
            .into_iter()
            .map(|a| AgentInfoDto {
                id: a.id,
                source: a.source,
                description: a.description,
                model: a.model,
            })
            .collect(),
        skills: info
            .skills
            .into_iter()
            .map(|s| SkillInfoDto {
                id: s.id,
                source: s.source,
                description: s.description,
            })
            .collect(),
        mcp_servers: info
            .mcp_servers
            .into_iter()
            .map(|m| McpServerInfoDto {
                name: m.name,
                kind: m.kind,
                enabled: m.enabled,
                tools: m.tools,
            })
            .collect(),
        enabled_tools: info.enabled_tools,
        hooks_count: info.hooks_count,
        update_available: info.update_available,
        install_hint: info.install_hint,
        settings: SettingsSnapshotDto {
            theme: map_theme_tokens(info.settings.theme),
            raw_theme: map_theme_override(&info.settings.raw_theme),
            context: ContextSettingsDto {
                context_window_tokens: info.settings.context.context_window_tokens,
                high_watermark_pct: info.settings.context.high_watermark_pct,
                spill_max_inline_tokens: info.settings.context.spill_max_inline_tokens,
                verbatim_window_turns: info.settings.context.verbatim_window_turns,
                reasoning_pruning: info.settings.context.reasoning_pruning,
                prompt_caching: info.settings.context.prompt_caching,
                rollback_on_verify_failure: info.settings.context.rollback_on_verify_failure,
                recovery_model: info.settings.context.recovery_model,
                max_steps_per_turn: info.settings.context.max_steps_per_turn,
                tool_timeout_ms: info.settings.context.tool_timeout_ms,
                max_session_tokens: info.settings.context.max_session_tokens,
                max_turn_secs: info.settings.context.max_turn_secs,
                summarizer_model: info.settings.context.summarizer_model,
                post_edit_diagnostics: info.settings.context.post_edit_diagnostics,
                temperature: info.settings.context.temperature.map(|t| t.value()),
            },
            session: SessionSettingsDto {
                auto_prune_days: info.settings.session.auto_prune_days,
                keep: info.settings.session.keep,
            },
            sandbox: SandboxSettingsDto {
                require_for_scripts: info.settings.sandbox.require_for_scripts,
                require_for_shell: info.settings.sandbox.require_for_shell,
                strict_env: info.settings.sandbox.strict_env,
            },
            permissions_summary: info.settings.permissions_summary,
            config_writable: info.settings.config_writable,
        },
    }
}

pub fn map_providers(snapshot: HostProvidersSnapshot) -> ProvidersResponse {
    ProvidersResponse {
        default_provider: snapshot.default_provider,
        providers: snapshot
            .providers
            .into_iter()
            .map(|p| ProviderItemDto {
                id: p.id,
                display_name: p.display_name,
                adapter: p.adapter,
                auth_kind: match p.auth_kind {
                    HostAuthKind::ApiKey => AuthKindDto::ApiKey,
                    HostAuthKind::OAuth => AuthKindDto::OAuth,
                    HostAuthKind::Either => AuthKindDto::Either,
                    HostAuthKind::None => AuthKindDto::None,
                },
                configured: p.configured,
                status_badge: p.status_badge,
                env_vars: p.env_vars,
                default_model: p.default_model,
                base_url: p.base_url,
                is_custom: p.is_custom,
            })
            .collect(),
        models: snapshot
            .models
            .into_iter()
            .map(|m| ModelItemDto {
                id: m.id,
                provider: m.provider,
                display_name: m.display_name,
                context_window_tokens: m.context_window_tokens,
                max_output_tokens: m.max_output_tokens,
                supports_reasoning: m.supports_reasoning,
                supports_vision: m.supports_vision,
                supports_tools: m.supports_tools,
                is_custom: m.is_custom,
            })
            .collect(),
        recent_models: snapshot
            .recent_models
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
    }
}

pub fn map_file_change(entry: FileChangeEntry) -> FileChangeDto {
    FileChangeDto {
        status: entry.status,
        path: entry.path,
    }
}

pub fn map_file_diff(view: FileDiffView) -> FileDiffResponse {
    FileDiffResponse {
        path: view.path,
        status: view.status,
        additions: view.additions,
        deletions: view.deletions,
        rows: view
            .rows
            .into_iter()
            .map(|row| DiffRowDto {
                kind: match row.kind {
                    DiffRowKind::HunkHeader(text) => DiffRowKindDto::HunkHeader { text },
                    DiffRowKind::Context => DiffRowKindDto::Context,
                    DiffRowKind::Modified => DiffRowKindDto::Modified,
                    DiffRowKind::Added => DiffRowKindDto::Added,
                    DiffRowKind::Removed => DiffRowKindDto::Removed,
                },
                left: row.left.map(|cell| DiffCellDto {
                    line_no: cell.line_no,
                    text: cell.text,
                }),
                right: row.right.map(|cell| DiffCellDto {
                    line_no: cell.line_no,
                    text: cell.text,
                }),
            })
            .collect(),
    }
}

pub fn map_search_results(
    mode: SearchMode,
    query: String,
    matches: Vec<SearchMatch>,
) -> WorkspaceSearchResponse {
    WorkspaceSearchResponse {
        mode: match mode {
            SearchMode::Files => SearchModeDto::Files,
            SearchMode::Content => SearchModeDto::Content,
        },
        query,
        matches: matches
            .into_iter()
            .map(|m| WorkspaceSearchMatchDto {
                path: m.path,
                line: m.line,
                column: m.column,
                snippet: m.snippet,
            })
            .collect(),
    }
}

pub fn map_search_preview(preview: SearchPreview) -> WorkspacePreviewResponse {
    WorkspacePreviewResponse {
        title: preview.title,
        lines: preview
            .lines
            .into_iter()
            .map(|(line_no, text, highlighted)| WorkspacePreviewLineDto {
                line_no,
                text,
                highlighted,
            })
            .collect(),
        scroll: preview.scroll,
    }
}

pub fn map_settings_update(req: UpdateSettingsRequest) -> HostSettingsUpdate {
    HostSettingsUpdate {
        reset_theme: req.reset_theme,
        theme: req.theme.map(|t| ThemeSettings {
            background: t.background,
            surface: t.surface,
            surface_high: t.surface_high,
            text: t.text,
            muted: t.muted,
            faint: t.faint,
            accent: t.accent,
            interactive: t.interactive,
            border: t.border,
            error: t.error,
            warning: t.warning,
            success: t.success,
            info: t.info,
            diff_added: t.diff_added,
            diff_removed: t.diff_removed,
            diff_context: t.diff_context,
            code_function: t.code_function,
        }),
        context: req.context.map(|c| ContextSettings {
            context_window_tokens: c.context_window_tokens,
            high_watermark_pct: c.high_watermark_pct,
            spill_max_inline_tokens: c.spill_max_inline_tokens,
            verbatim_window_turns: c.verbatim_window_turns,
            reasoning_pruning: c.reasoning_pruning,
            prompt_caching: c.prompt_caching,
            rollback_on_verify_failure: c.rollback_on_verify_failure,
            recovery_model: c.recovery_model,
            max_steps_per_turn: c.max_steps_per_turn,
            tool_timeout_ms: c.tool_timeout_ms,
            max_session_tokens: c.max_session_tokens,
            max_turn_secs: c.max_turn_secs,
            summarizer_model: c.summarizer_model,
            post_edit_diagnostics: c.post_edit_diagnostics,
            temperature: c.temperature.map(Temperature::new),
        }),
        session: req.session.map(|s| SessionSettings {
            auto_prune_days: s.auto_prune_days,
            keep: s.keep,
        }),
        sandbox: req.sandbox.map(|s| SandboxSettings {
            require_for_scripts: s.require_for_scripts,
            require_for_shell: s.require_for_shell,
            strict_env: s.strict_env,
        }),
    }
}

pub fn unmap_approval_decision(dto: ApprovalDecisionDto) -> ApprovalDecision {
    match dto {
        ApprovalDecisionDto::Accept => ApprovalDecision::Accept,
        ApprovalDecisionDto::AcceptForSession => ApprovalDecision::AcceptForSession,
        ApprovalDecisionDto::Decline => ApprovalDecision::Decline,
        ApprovalDecisionDto::DeclineWithReason { reason } => {
            ApprovalDecision::DeclineWithReason(reason)
        }
        ApprovalDecisionDto::Cancel => ApprovalDecision::Cancel,
    }
}

pub fn unmap_grant_scope(dto: GrantScopeDto) -> GrantScope {
    match dto {
        GrantScopeDto::Turn => GrantScope::Turn,
        GrantScopeDto::Session => GrantScope::Session,
    }
}

pub fn unmap_approval_mode(dto: ApprovalModeDto) -> ApprovalMode {
    match dto {
        ApprovalModeDto::Ask => ApprovalMode::Ask,
        ApprovalModeDto::Auto => ApprovalMode::Auto,
        ApprovalModeDto::Plan => ApprovalMode::Plan,
    }
}

pub fn unmap_rewind_scope(dto: RewindScopeDto) -> RewindScope {
    match dto {
        RewindScopeDto::Both => RewindScope::Both,
        RewindScopeDto::Code => RewindScope::Code,
        RewindScopeDto::Conversation => RewindScope::Conversation,
    }
}

pub fn unmap_question_answers(list: Vec<QuestionAnswerDto>) -> Vec<QuestionAnswer> {
    list.into_iter()
        .map(|a| QuestionAnswer {
            header: a.header,
            selected: a.selected,
        })
        .collect()
}

fn map_approval_mode(mode: ApprovalMode) -> ApprovalModeDto {
    match mode {
        ApprovalMode::Ask => ApprovalModeDto::Ask,
        ApprovalMode::Auto => ApprovalModeDto::Auto,
        ApprovalMode::Plan => ApprovalModeDto::Plan,
    }
}

fn map_rewind_scope(scope: RewindScope) -> RewindScopeDto {
    match scope {
        RewindScope::Both => RewindScopeDto::Both,
        RewindScope::Code => RewindScopeDto::Code,
        RewindScope::Conversation => RewindScopeDto::Conversation,
    }
}

fn map_approval_request(req: ApprovalRequest) -> ApprovalRequestDto {
    ApprovalRequestDto {
        request_id: req.request_id.to_string(),
        turn_id: req.turn_id.to_string(),
        kind: match req.kind {
            ApprovalKind::ToolCall => ApprovalKindDto::ToolCall,
            ApprovalKind::Spend => ApprovalKindDto::Spend,
            ApprovalKind::SandboxEscape => ApprovalKindDto::SandboxEscape,
            ApprovalKind::ExternalAgent => ApprovalKindDto::ExternalAgent,
        },
        title: req.preview.title,
        detail: req.preview.detail,
        effect: req.effect.map(|e| match e {
            EffectClass::Read => EffectClassDto::Read,
            EffectClass::Write => EffectClassDto::Write,
            EffectClass::Execute => EffectClassDto::Execute,
            EffectClass::External => EffectClassDto::External,
        }),
    }
}

fn map_question_request(req: QuestionRequest) -> QuestionRequestDto {
    QuestionRequestDto {
        request_id: req.request_id.to_string(),
        turn_id: req.turn_id.to_string(),
        questions: req.questions.into_iter().map(map_question_prompt).collect(),
    }
}

fn map_question_prompt(p: QuestionPrompt) -> QuestionPromptDto {
    QuestionPromptDto {
        header: p.header,
        question: p.question,
        options: p
            .options
            .into_iter()
            .map(|o| QuestionOptionDto {
                label: o.label,
                description: o.description,
            })
            .collect(),
        multiple: p.multiple,
    }
}

fn map_question_answer(a: QuestionAnswer) -> QuestionAnswerDto {
    QuestionAnswerDto {
        header: a.header,
        selected: a.selected,
    }
}

fn map_unified_line(line: UnifiedLine) -> UnifiedLineDto {
    UnifiedLineDto {
        kind: match line.kind {
            UnifiedLineKind::FileHeader => UnifiedLineKindDto::FileHeader,
            UnifiedLineKind::HunkHeader => UnifiedLineKindDto::HunkHeader,
            UnifiedLineKind::Context => UnifiedLineKindDto::Context,
            UnifiedLineKind::Added => UnifiedLineKindDto::Added,
            UnifiedLineKind::Removed => UnifiedLineKindDto::Removed,
        },
        text: line.text,
    }
}

fn map_usage(usage: UsageStatsSnapshot) -> UsageDto {
    UsageDto {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        cached_tokens: usage.cached_tokens,
        est_cost_usd: usage.est_cost_usd,
    }
}

fn map_metrics(metrics: TurnMetricsSnapshot) -> TurnMetricsDto {
    TurnMetricsDto {
        turn_count: metrics.turn_count,
        tool_calls: metrics.tool_calls,
        session_prompt_tokens: metrics.session_prompt_tokens,
        session_completion_tokens: metrics.session_completion_tokens,
        session_total_tokens: metrics.session_total_tokens,
        session_cached_tokens: metrics.session_cached_tokens,
        last_turn_ms: metrics.last_turn_ms,
        tokens_per_sec: metrics.tokens_per_sec,
    }
}

fn map_theme_tokens(tokens: ResolvedThemeHex) -> ThemeTokensDto {
    ThemeTokensDto {
        background: tokens.background,
        surface: tokens.surface,
        surface_high: tokens.surface_high,
        text: tokens.text,
        muted: tokens.muted,
        faint: tokens.faint,
        accent: tokens.accent,
        interactive: tokens.interactive,
        border: tokens.border,
        error: tokens.error,
        warning: tokens.warning,
        success: tokens.success,
        info: tokens.info,
        diff_added: tokens.diff_added,
        diff_removed: tokens.diff_removed,
        diff_context: tokens.diff_context,
        code_function: tokens.code_function,
    }
}

fn map_theme_override(settings: &ThemeSettings) -> ThemeOverrideDto {
    ThemeOverrideDto {
        background: settings.background.clone(),
        surface: settings.surface.clone(),
        surface_high: settings.surface_high.clone(),
        text: settings.text.clone(),
        muted: settings.muted.clone(),
        faint: settings.faint.clone(),
        accent: settings.accent.clone(),
        interactive: settings.interactive.clone(),
        border: settings.border.clone(),
        error: settings.error.clone(),
        warning: settings.warning.clone(),
        success: settings.success.clone(),
        info: settings.info.clone(),
        diff_added: settings.diff_added.clone(),
        diff_removed: settings.diff_removed.clone(),
        diff_context: settings.diff_context.clone(),
        code_function: settings.code_function.clone(),
    }
}

fn map_backend(backend: Backend) -> BackendKind {
    match backend {
        Backend::Native => BackendKind::Native,
        Backend::Acp {
            preset,
            remote_session_id,
        } => BackendKind::Acp {
            preset,
            remote_session_id,
        },
    }
}

fn map_status(status: HostSessionStatus) -> SessionStatus {
    match status {
        HostSessionStatus::Idle => SessionStatus::Idle,
        HostSessionStatus::Running => SessionStatus::Running,
        HostSessionStatus::WaitingApproval => SessionStatus::WaitingApproval,
        HostSessionStatus::WaitingQuestion => SessionStatus::WaitingQuestion,
    }
}

fn format_turn_end_reason(reason: impl Serialize) -> String {
    let Ok(value) = serde_json::to_value(reason) else {
        return "unknown".to_string();
    };
    match value.get("kind").and_then(|v| v.as_str()) {
        Some("completed") => "completed".to_string(),
        Some("aborted") => match value.get("cause").and_then(|v| v.as_str()) {
            Some(cause) => format!("aborted:{cause}"),
            None => "aborted".to_string(),
        },
        Some("failed") => match value.get("error").and_then(|v| v.as_str()) {
            Some(err) => format!("failed: {err}"),
            None => "failed".to_string(),
        },
        Some("max_tokens") => "maxTokens".to_string(),
        Some("interrupted") => "interrupted".to_string(),
        Some(other) => other.to_string(),
        None => value.to_string(),
    }
}

fn format_error_code(code: impl Serialize) -> String {
    let Ok(value) = serde_json::to_value(code) else {
        return "internal".to_string();
    };
    match value.as_str() {
        Some("context_overflow") => "contextOverflow".to_string(),
        Some("loop_detected") => "loopDetected".to_string(),
        Some(other) => other.to_string(),
        None => value.to_string(),
    }
}
