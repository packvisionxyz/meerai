use super::*;
use meerai_core::env::now_millis;

impl AppState {
    pub(crate) fn running_subagents(&self) -> Vec<usize> {
        self.tab()
            .transcript
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                TranscriptEntry::Subagent(subagent)
                    if matches!(
                        subagent.status,
                        ToolCallStatus::Running | ToolCallStatus::Proposed
                    ) =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn running_shells(&self) -> Vec<u64> {
        self.tab()
            .shell_runs
            .iter()
            .filter(|run| run.status == ShellRunStatus::Running)
            .map(|run| run.id)
            .collect()
    }

    pub(crate) fn run_count(&self, kind: RunKind) -> usize {
        match kind {
            RunKind::Agents => self.running_subagents().len(),
            RunKind::Shells => self.running_shells().len(),
        }
    }

    pub(crate) fn open_runs(&mut self, kind: RunKind) {
        self.modal = Some(ActiveModal::Runs { kind, selected: 0 });
    }

    pub(crate) fn open_run_detail(&mut self, kind: RunKind, index: usize) {
        let target = match kind {
            RunKind::Agents => self
                .running_subagents()
                .get(index)
                .copied()
                .map(RunTarget::Subagent),
            RunKind::Shells => self
                .running_shells()
                .get(index)
                .copied()
                .map(RunTarget::Shell),
        };
        let Some(run) = target else {
            self.modal = Some(ActiveModal::Runs { kind, selected: 0 });
            return;
        };
        if let RunTarget::Subagent(entry_idx) = run {
            let item_id = match self.tab().transcript.get(entry_idx) {
                Some(TranscriptEntry::Subagent(subagent)) => subagent.id.clone(),
                _ => return,
            };
            self.request_child_session_load(&item_id, true);
        }
        self.modal = Some(ActiveModal::RunDetail {
            run,
            selected: index,
            scroll: 0,
        });
    }

    pub(crate) fn start_shell_run(&mut self, command: String) -> (u64, u64) {
        let tab = self.tab_mut();
        let id = tab.next_shell_run_id;
        tab.next_shell_run_id = tab.next_shell_run_id.wrapping_add(1);
        tab.shell_runs.push(ShellRun {
            id,
            command,
            status: ShellRunStatus::Running,
            output: None,
            started_ms: now_millis(),
        });
        if tab.shell_runs.len() > 32
            && let Some(index) = tab
                .shell_runs
                .iter()
                .position(|run| run.status != ShellRunStatus::Running)
        {
            tab.shell_runs.remove(index);
        }
        (tab.tab_id, id)
    }

    pub(crate) fn shell_run(&self, id: u64) -> Option<&ShellRun> {
        self.tab().shell_runs.iter().find(|run| run.id == id)
    }

    pub(crate) fn complete_shell_run(
        &mut self,
        tab_id: u64,
        run_id: u64,
        result: Result<String, String>,
    ) {
        let Some(index) = self.tab_index_by_id(tab_id) else {
            return;
        };
        let (command, text, failed) = {
            let tab = &mut self.tabs[index];
            let Some(run) = tab.shell_runs.iter_mut().find(|run| run.id == run_id) else {
                return;
            };
            match result {
                Ok(output) => {
                    run.status = ShellRunStatus::Succeeded;
                    run.output = Some(output.clone());
                    (run.command.clone(), output, false)
                }
                Err(error) => {
                    let text = format!("failed: {error}");
                    run.status = ShellRunStatus::Failed;
                    run.output = Some(text.clone());
                    (run.command.clone(), text, true)
                }
            }
        };
        if let Some(ActiveModal::Terminal {
            run_id: active,
            output,
            running,
            scroll,
            ..
        }) = &mut self.modal
            && *active == run_id
        {
            *output = Some(text.clone());
            *running = false;
            *scroll = 0;
        }
        let text = if failed {
            format!("$ {command}\n{text}")
        } else {
            text
        };
        let tab = &mut self.tabs[index];
        if failed {
            tab.transcript.push(TranscriptEntry::Error {
                text,
                expanded: false,
            });
        } else {
            tab.transcript.push(TranscriptEntry::System { text });
        }
        tab.transcript_version = tab.transcript_version.wrapping_add(1);
        tab.auto_follow = true;
    }
}
