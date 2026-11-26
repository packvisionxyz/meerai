use super::types::*;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKindDto {
    Agents,
    Shells,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningSubagent {
    pub entry_id: String,
    pub name: Option<String>,
    pub agent: String,
    pub status: String,
    pub child_session_id: Option<String>,
    pub steps_used: usize,
    pub max_steps: usize,
}

pub fn running_subagent_rows(state: &AppState) -> Vec<RunningSubagent> {
    let Some(tab) = state.active_tab() else {
        return Vec::new();
    };
    tab.entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntryDto::Subagent {
                id,
                name,
                agent,
                status,
                child_session_id,
                steps_used,
                max_steps,
                ..
            } if status == "running" || status == "proposed" => Some(RunningSubagent {
                entry_id: id.clone(),
                name: name.clone(),
                agent: agent.clone(),
                status: status.clone(),
                child_session_id: child_session_id.clone(),
                steps_used: *steps_used,
                max_steps: *max_steps,
            }),
            _ => None,
        })
        .collect()
}

pub fn running_subagent_count(state: &AppState) -> usize {
    running_subagent_rows(state).len()
}

pub fn running_shell_count(state: &AppState) -> usize {
    state.active_session().map_or(0, |s| s.shell_runs.len())
}

pub fn open_runs(state: &mut AppState, kind: RunKindDto) {
    state.modal = Some(ActiveModal::Runs { kind, selected: 0 });
}

pub fn open_run_row(state: &mut AppState, kind: RunKindDto, index: usize) {
    match kind {
        RunKindDto::Agents => {
            let Some(row) = running_subagent_rows(state).into_iter().nth(index) else {
                return;
            };
            let child = row.child_session_id.clone();
            state.modal = Some(ActiveModal::SubagentOverlay {
                entry_id: row.entry_id,
                name: row.name,
                agent: row.agent,
                status: row.status,
                child_session_id: child.clone(),
                entries: Vec::new(),
                usage: None,
                loading: child.is_some(),
                error: None,
                from_runs: Some(RunKindDto::Agents),
            });
            if let Some(child) = child
                && let Some(t) = crate::transport::current()
            {
                t.fetch_session_events(child, true);
            }
        }
        RunKindDto::Shells => {
            let Some(run) = state
                .active_session()
                .and_then(|s| s.shell_runs.get(index))
                .cloned()
            else {
                return;
            };
            state.modal = Some(ActiveModal::ShellRun {
                run_id: run.id,
                command: run.command,
                started_at_ms: run.started_at_ms,
                from_runs: Some(RunKindDto::Shells),
            });
        }
    }
}

pub fn back_to_runs(state: &mut AppState, kind: RunKindDto) {
    state.modal = Some(ActiveModal::Runs { kind, selected: 0 });
}

pub(crate) fn next_shell_run(state: &mut AppState) -> String {
    let run_id = format!(
        "web-{}-{}",
        shell_run_timestamp_ms(),
        state.next_shell_run_id
    );
    state.next_shell_run_id = state.next_shell_run_id.wrapping_add(1);
    run_id
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn shell_run_timestamp_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn shell_run_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub fn apply_shell_output(
    state: &mut AppState,
    _session_id: &str,
    run_id: &str,
    result: Result<String, String>,
) {
    if let Err(err) = &result {
        state.error_banner = Some(err.clone());
    }
    state
        .shell_outputs
        .insert(run_id.to_owned(), result.clone());
    while state.shell_outputs.len() > 32 {
        let Some(key) = state
            .shell_outputs
            .keys()
            .find(|key| key.as_str() != run_id)
            .cloned()
        else {
            break;
        };
        state.shell_outputs.remove(&key);
    }
    if let Some(ActiveModal::Terminal {
        run_id: modal_run,
        output,
        running,
        error,
        ..
    }) = &mut state.modal
        && modal_run.as_deref() == Some(run_id)
    {
        *running = false;
        match result {
            Ok(out) => {
                *output = Some(out);
                *error = None;
            }
            Err(err) => {
                *error = Some(err);
            }
        }
    }
}

pub fn execute_user_shell(state: &mut AppState, cmd: &str) {
    if cmd.is_empty() {
        state.modal = Some(ActiveModal::Terminal {
            command: TextBuffer::new(""),
            output: None,
            running: false,
            error: None,
            run_id: None,
        });
        return;
    }
    if state
        .active_session()
        .is_some_and(|s| s.approval_mode == ApprovalModeDto::Plan)
    {
        state.error_banner = Some("Shell commands (!) are disabled in plan mode.".to_owned());
        return;
    }
    let Some(sid) = state.active_session_id.clone() else {
        state.error_banner = Some("No active session".to_owned());
        return;
    };
    if let Some(t) = crate::transport::current() {
        let run_id = next_shell_run(state);
        state.modal = Some(ActiveModal::Terminal {
            command: TextBuffer::new(cmd),
            output: None,
            running: true,
            error: None,
            run_id: Some(run_id.clone()),
        });
        t.run_shell(sid, cmd.to_owned(), run_id);
    }
}
