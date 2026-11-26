use super::completion::*;
use super::digest::*;
use super::forms::*;
use super::types::*;
use super::*;

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_session_id: None,
            sessions: Vec::new(),
            tab_order: Vec::new(),
            hidden_session_ids: HashSet::new(),
            pending_replace_session: None,
            session_tabs: HashMap::new(),
            session_panel: SessionPanel::default(),
            host_info: HostInfoDto::default(),
            file_changes: Vec::new(),
            providers: None,
            workspace_files: Vec::new(),
            last_event_id: 0,
            connection: ConnectionState::Connecting,
            draft: TextBuffer::new(""),
            prompt_history: Vec::new(),
            prompt_history_idx: None,
            saved_draft_before_history: String::new(),
            completion: CompletionState::default(),
            sidebar_open: true,
            sidebar_tools_expanded: false,
            sidebar_diagnose_open: true,
            sidebar_changes_open: true,
            modal: None,
            sending: false,
            reset_confirm_pending: false,
            error_banner: None,
            status_notice: None,
            notice_seq: 0,
            last_title_sent: String::new(),
            shell_outputs: HashMap::new(),
            next_shell_run_id: 0,
        }
    }
}

pub fn set_status_notice(state: &mut AppState, msg: impl Into<String>) {
    state.notice_seq = state.notice_seq.wrapping_add(1);
    state.status_notice = Some(msg.into());
}

pub fn reopen_pending_request(state: &mut AppState) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    let Some(tab) = state.session_tabs.get(&sid) else {
        return;
    };
    if let Some(req) = tab.pending_approvals.first().cloned() {
        state.modal = Some(ActiveModal::Approval {
            session_id: sid,
            request: req,
            reason: TextBuffer::new(""),
            showing_reason: false,
        });
    } else if let Some(req) = tab.pending_questions.first().cloned() {
        let draft = QuestionDraft::from_request(&req);
        state.modal = Some(ActiveModal::Question {
            session_id: sid,
            request: req,
            draft,
        });
    }
}

pub(crate) fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut pending_newline = false;
    for ch in text.chars() {
        if ch == '\n' {
            pending_newline = true;
            pending_space = false;
        } else if ch.is_whitespace() {
            pending_space = true;
        } else {
            if !result.is_empty() {
                if pending_newline {
                    result.push('\n');
                } else if pending_space {
                    result.push(' ');
                }
            }
            pending_newline = false;
            pending_space = false;
            result.push(ch);
        }
    }
    result
}

pub const QUESTION_TOOL: &str = "question-ask";

pub fn summarize_tool_args(name: &str, raw_args: &str) -> String {
    let trimmed = raw_args.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let pick_str = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| {
                map.get(*k)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
        };
        let summary = match name {
            "code-read_file" | "code-write_file" | "code-edit_file" | "code-list_dir" => {
                pick_str(&["path", "Path", "file_path", "dir"])
            }
            "code-shell" => pick_str(&["command", "CommandLine", "cmd"]),
            "code-grep" | "code-glob" => pick_str(&["pattern", "query", "glob", "path"]),
            "subagent-run" | "acp-run" => pick_str(&["agent", "preset", "role", "task", "prompt"]),
            _ => pick_str(&["path", "Path", "command", "query", "pattern", "name", "url"]),
        };
        if let Some(text) = summary {
            return text.lines().next().unwrap_or(&text).to_owned();
        }
    }
    trimmed.lines().next().unwrap_or(trimmed).to_owned()
}

pub(crate) fn push_local_system(tab: &mut SessionTabState, text: String) {
    let id = format!("local-sys-{}", tab.entries.len());
    tab.entries.push(TranscriptEntryDto::System { id, text });
    tab.entries_version = tab.entries_version.wrapping_add(1);
}

pub(crate) fn mark_unread_if_bg(state: &mut AppState, session_id: &str) {
    if state.active_session_id.as_deref() != Some(session_id)
        && let Some(tab) = state.session_tabs.get_mut(session_id)
    {
        tab.unread = true;
    }
}

pub(crate) fn upsert_entry(entries: &mut Vec<TranscriptEntryDto>, entry: TranscriptEntryDto) {
    let id = entry_id_of(&entry).to_owned();
    if let Some(existing) = entries.iter_mut().find(|item| entry_id_of(item) == id) {
        *existing = entry;
    } else {
        entries.push(entry);
    }
}
