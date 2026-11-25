use crate::*;

fn sample_summary() -> SessionSummaryDto {
    SessionSummaryDto {
        id: "sess-1".to_owned(),
        title: Some("Fix protocol".to_owned()),
        backend: BackendKind::Native,
        provider: "anthropic".to_owned(),
        model: "claude-sonnet".to_owned(),
        approval_mode: ApprovalModeDto::Ask,
        status: SessionStatus::Running,
        active_turn_id: Some("turn-1".to_owned()),
        active_tool: None,
        turn_started_at_ms: None,
        user_shell_running: false,
        transient: false,
        shell_runs: Vec::new(),
    }
}

#[test]
fn api_error_round_trip() {
    let codes = [
        (ApiErrorCode::Unauthorized, "unauthorized"),
        (ApiErrorCode::Forbidden, "forbidden"),
        (ApiErrorCode::BadRequest, "badRequest"),
        (ApiErrorCode::NotFound, "notFound"),
        (ApiErrorCode::ConflictBusy, "conflictBusy"),
        (ApiErrorCode::ConflictStale, "conflictStale"),
        (ApiErrorCode::UnsupportedBackend, "unsupportedBackend"),
        (ApiErrorCode::Internal, "internal"),
    ];

    for (code, expected) in codes {
        let err = ApiError {
            code,
            message: "rejected".to_owned(),
        };
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["code"], expected);
        assert_eq!(value["message"], "rejected");
        let decoded: ApiError = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, err);
    }
}

#[test]
fn transcript_entry_round_trip() {
    let entries = [
        TranscriptEntryDto::User {
            id: "u1".to_owned(),
            turn_id: Some("turn-1".to_owned()),
            text: "hello".to_owned(),
        },
        TranscriptEntryDto::Assistant {
            id: "a1".to_owned(),
            text: "world".to_owned(),
            reasoning: "thought".to_owned(),
        },
        TranscriptEntryDto::System {
            id: "s1".to_owned(),
            text: "note".to_owned(),
        },
        TranscriptEntryDto::Error {
            id: "e1".to_owned(),
            message: "boom".to_owned(),
        },
        TranscriptEntryDto::ToolCall {
            id: "t1".to_owned(),
            name: "code-read_file".to_owned(),
            args: "{}".to_owned(),
            status: "succeeded".to_owned(),
            output: Some("ok".to_owned()),
            edit_diff: Some(vec![UnifiedLineDto {
                kind: UnifiedLineKindDto::Added,
                text: "let x = 1;".to_owned(),
            }]),
        },
        TranscriptEntryDto::Subagent {
            id: "sa-1".to_owned(),
            name: Some("scan".to_owned()),
            agent: "research".to_owned(),
            status: "running".to_owned(),
            report: None,
            child_session_id: Some("ses-child".to_owned()),
            model: None,
            approval_mode: "ask".to_owned(),
            steps_used: 0,
            max_steps: 0,
            usage: UsageDto::default(),
            est_cost_usd: None,
            active_tool: None,
        },
        TranscriptEntryDto::Question {
            id: "q-1".to_owned(),
            questions: vec![QuestionPromptDto {
                header: "Choice".to_owned(),
                question: "Pick one".to_owned(),
                options: vec![QuestionOptionDto {
                    label: "A".to_owned(),
                    description: None,
                }],
                multiple: false,
            }],
            answers: Some(vec![QuestionAnswerDto {
                header: "Choice".to_owned(),
                selected: vec!["A".to_owned()],
            }]),
            resolved: true,
        },
    ];

    for entry in entries {
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: TranscriptEntryDto = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, entry);
    }

    let tool_value = serde_json::to_value(&TranscriptEntryDto::ToolCall {
        id: "t1".to_owned(),
        name: "code-read_file".to_owned(),
        args: String::new(),
        status: "succeeded".to_owned(),
        output: None,
        edit_diff: None,
    })
    .unwrap();
    assert_eq!(tool_value["type"], "toolCall");
    assert_eq!(tool_value["data"]["name"], "code-read_file");
}

#[test]
fn snapshot_response_round_trip() {
    let mut acp_summary = sample_summary();
    acp_summary.id = "sess-2".to_owned();
    acp_summary.title = None;
    acp_summary.backend = BackendKind::Acp {
        preset: "agy".to_owned(),
        remote_session_id: Some("remote-9".to_owned()),
    };
    acp_summary.status = SessionStatus::WaitingApproval;
    acp_summary.active_turn_id = None;

    let snapshot = SnapshotResponse {
        event_id: 42,
        active_session_id: Some("sess-1".to_owned()),
        sessions: vec![sample_summary(), acp_summary],
        active_session: Some(SessionDetailDto {
            summary: sample_summary(),
            entries: vec![TranscriptEntryDto::User {
                id: "u1".to_owned(),
                turn_id: None,
                text: "ping".to_owned(),
            }],
            streaming_item_id: Some("item-2".to_owned()),
            streaming_text: "pong".to_owned(),
            streaming_reasoning: String::new(),
            pending_approvals: Vec::new(),
            pending_questions: Vec::new(),
            usage: UsageDto::default(),
            metrics: TurnMetricsDto::default(),
            context: None,
            loop_state: None,
        }),
        file_changes: vec![FileChangeDto {
            status: "M".to_owned(),
            path: "src/lib.rs".to_owned(),
            staged: false,
            unstaged: true,
        }],
        host_info: Some(HostInfoDto::default()),
    };

    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(value["eventId"], 42);
    assert_eq!(value["activeSessionId"], "sess-1");
    assert_eq!(value["sessions"][1]["backend"]["type"], "acp");
    assert_eq!(
        value["sessions"][1]["backend"]["data"]["remoteSessionId"],
        "remote-9"
    );
    let decoded: SnapshotResponse = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, snapshot);
}

#[test]
fn view_event_round_trip() {
    let events = [
        ViewEvent::SessionOpened {
            session: sample_summary(),
        },
        ViewEvent::SessionUpdated {
            session: sample_summary(),
        },
        ViewEvent::SessionActivated {
            session_id: "sess-1".to_owned(),
        },
        ViewEvent::SessionClosed {
            session_id: "sess-1".to_owned(),
            next_active_session_id: None,
        },
        ViewEvent::TurnStarted {
            session_id: "sess-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            model: "claude-sonnet".to_owned(),
        },
        ViewEvent::UserPromptSubmitted {
            session_id: "sess-1".to_owned(),
            entry_id: "u1".to_owned(),
            text: "hi".to_owned(),
        },
        ViewEvent::TextDelta {
            session_id: "sess-1".to_owned(),
            item_id: "a1".to_owned(),
            delta: "hel".to_owned(),
        },
        ViewEvent::ReasoningDelta {
            session_id: "sess-1".to_owned(),
            item_id: "a1".to_owned(),
            delta: "think".to_owned(),
        },
        ViewEvent::StreamDiscarded {
            session_id: "sess-1".to_owned(),
        },
        ViewEvent::EntryCompleted {
            session_id: "sess-1".to_owned(),
            entry: TranscriptEntryDto::Assistant {
                id: "a1".to_owned(),
                text: "hello".to_owned(),
                reasoning: "think".to_owned(),
            },
        },
        ViewEvent::ApprovalRequested {
            session_id: "sess-1".to_owned(),
            request: ApprovalRequestDto {
                request_id: "apr-1".to_owned(),
                turn_id: "turn-1".to_owned(),
                kind: ApprovalKindDto::ToolCall,
                title: "Edit file".to_owned(),
                detail: "src/main.rs".to_owned(),
                effect: Some(EffectClassDto::Write),
            },
        },
        ViewEvent::TurnCompleted {
            session_id: "sess-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            reason: "completed".to_owned(),
        },
        ViewEvent::ChatGptLoginUpdated {
            attempt_id: "att-1".to_owned(),
            provider_id: "openai-codex".to_owned(),
            state: ChatGptLoginStateDto::Started,
            url: Some("https://auth.example.com".to_owned()),
            account: None,
            message: None,
        },
        ViewEvent::Error {
            session_id: "sess-1".to_owned(),
            code: "providerError".to_owned(),
            message: "timeout".to_owned(),
        },
        ViewEvent::ProvidersUpdated,
        ViewEvent::ResyncRequired,
        ViewEvent::HostDisconnected,
    ];

    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        let decoded: ViewEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
    }

    let resync = serde_json::to_value(&ViewEvent::ResyncRequired).unwrap();
    assert_eq!(resync["type"], "resyncRequired");
    assert!(resync.get("data").is_none());

    let disconnected = serde_json::to_value(&ViewEvent::HostDisconnected).unwrap();
    assert_eq!(disconnected["type"], "hostDisconnected");
    assert!(disconnected.get("data").is_none());

    let turn_started = serde_json::to_value(&ViewEvent::TurnStarted {
        session_id: "sess-1".to_owned(),
        turn_id: "turn-1".to_owned(),
        model: "claude-sonnet".to_owned(),
    })
    .unwrap();
    assert_eq!(turn_started["type"], "turnStarted");
    assert_eq!(turn_started["data"]["sessionId"], "sess-1");
    assert_eq!(turn_started["data"]["turnId"], "turn-1");
    assert_eq!(turn_started["data"]["model"], "claude-sonnet");
}

#[test]
fn question_answer_request_supports_cancel() {
    let answered: QuestionAnswerRequest =
        serde_json::from_str(r#"{"answers":[{"header":"Choice","selected":["A"]}]}"#).unwrap();
    assert!(!answered.cancel);
    assert_eq!(answered.answers.len(), 1);

    let cancelled: QuestionAnswerRequest =
        serde_json::from_str(r#"{"answers":[],"cancel":true}"#).unwrap();
    assert!(cancelled.cancel);
    assert!(cancelled.answers.is_empty());
}

#[test]
fn request_and_response_round_trip() {
    let req = SubmitPromptRequest {
        prompt: "explain this".to_owned(),
        display: Some("@skill/verify explain this".to_owned()),
    };
    let req_json = serde_json::to_string(&req).unwrap();
    let decoded_req: SubmitPromptRequest = serde_json::from_str(&req_json).unwrap();
    assert_eq!(decoded_req, req);

    let legacy: SubmitPromptRequest = serde_json::from_str(r#"{"prompt":"hi"}"#).unwrap();
    assert_eq!(legacy.prompt, "hi");
    assert!(legacy.display.is_none());

    let resp = SubmitPromptResponse {
        accepted: true,
        session_id: "sess-1".to_owned(),
    };
    let resp_value = serde_json::to_value(&resp).unwrap();
    assert_eq!(resp_value["sessionId"], "sess-1");
    let decoded_resp: SubmitPromptResponse = serde_json::from_value(resp_value).unwrap();
    assert_eq!(decoded_resp, resp);

    let created = CreateSessionResponse {
        session: sample_summary(),
    };
    let created_json = serde_json::to_string(&created).unwrap();
    let decoded_created: CreateSessionResponse = serde_json::from_str(&created_json).unwrap();
    assert_eq!(decoded_created, created);
}
