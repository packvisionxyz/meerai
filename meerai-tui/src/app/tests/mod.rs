use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use meerai_agents::events::{
    ApprovalDecision, ApprovalKind, ApprovalPreview, ApprovalRequest, GrantScope, RequestId,
};
use meerai_agents::{
    AbortCause, AgentEvent, ErrorCode, ItemDelta, ItemStatus, SessionItem, TurnEndReason, Usage,
};
use meerai_client::providers::ProviderCatalog;
use meerai_client::secrets::{Credential, MemorySecretStore, OAuthCredential};
use meerai_common::definitions::DefinitionSource;
use meerai_core::EffectClass;

use super::*;
use crate::completion::CompletionItemKind;

fn test_app() -> (
    AppState,
    mpsc::UnboundedReceiver<AgentCommand>,
    Arc<MemorySecretStore>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_key("gemini", "test-gemini-key").unwrap();
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let app = AppState::new(
        "gemini",
        "gemini-3.8-flash",
        PathBuf::from("."),
        tx,
        runtime,
        secrets.clone(),
        settings,
        None,
    );
    (app, rx, secrets)
}

fn oauth_credential(needs_relogin: bool) -> Credential {
    Credential::OAuth(OAuthCredential {
        access: "plan-access".to_string(),
        refresh: "plan-refresh".to_string(),
        expires: u64::MAX,
        client_id: "plan-client".to_string(),
        scopes: vec!["chatgpt.tokens.use.direct".to_string()],
        account: None,
        needs_relogin,
    })
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn type_text(app: &mut AppState, text: &str) {
    for ch in text.chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
}

fn sample_agent(
    id: &str,
    source: DefinitionSource,
    description: Option<&str>,
    model: Option<&str>,
    prompt: &str,
) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        source,
        description: description.map(str::to_string),
        model: model.map(str::to_string),
        steps: None,
        mode: None,
        prompt: prompt.to_string(),
        path: PathBuf::from(format!(".agents/agents/{id}.md")),
        token_estimate: 0,
        warnings: Vec::new(),
        supporting_files: Vec::new(),
    }
}

fn sample_skill(
    id: &str,
    source: DefinitionSource,
    description: Option<&str>,
    body: &str,
) -> SkillDefinition {
    SkillDefinition {
        id: id.to_string(),
        source,
        description: description.map(str::to_string),
        model_invocable: description.is_some(),
        body: body.to_string(),
        path: PathBuf::from(format!(".agents/skills/{id}/SKILL.md")),
        directory: PathBuf::from(format!(".agents/skills/{id}")),
        token_estimate: 0,
        warnings: Vec::new(),
        supporting_files: Vec::new(),
    }
}

fn submit(app: &mut AppState, command: &str) {
    type_text(app, command);
    if app.completion.is_active() {
        app.handle_key_event(key(KeyCode::Esc));
    }
    app.handle_key_event(key(KeyCode::Enter));
}

fn approval_request(id: &str, path: &str) -> ApprovalRequest {
    ApprovalRequest {
        request_id: RequestId::from_raw(id),
        turn_id: TurnId::from_raw("turn-approval"),
        kind: ApprovalKind::ToolCall,
        preview: ApprovalPreview {
            title: format!("edit: {path}"),
            detail: format!("tool: code-write_file\neffect: write\nresource: {path}"),
        },
        effect: Some(EffectClass::Write),
    }
}

struct DummySessionOpener;

#[meerai_core::async_trait]
impl SessionOpener for DummySessionOpener {
    async fn list_sessions(&self) -> anyhow::Result<Vec<meerai_session::SessionSummary>> {
        Ok(Vec::new())
    }

    async fn open_session(&self, id: &SessionId) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("unexpected open_session call for {id}")
    }

    async fn create_session(&self) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("unexpected create_session call")
    }

    async fn fork_session(
        &self,
        id: &SessionId,
        turn: &meerai_agents::TurnId,
    ) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("unexpected fork_session call for {id} at {turn}")
    }

    async fn delete_session(&self, id: &SessionId) -> anyhow::Result<bool> {
        anyhow::bail!("unexpected delete_session call for {id}")
    }
}

mod commands;
mod events;
mod mentions;
mod modals;
mod navigation;
mod sessions;
mod settings;

#[test]
fn bang_command_needs_a_shell_opener() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "!echo hi".to_string();
    app.tab_mut().cursor = app.tab().input.len();
    app.submit_input();
    assert!(app.tab().input.is_empty());
    assert!(
        app.status_notice
            .as_deref()
            .unwrap_or("")
            .contains("Shell commands are not available")
    );
}
