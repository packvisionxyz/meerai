use super::*;

#[derive(Default)]
pub(super) struct TurnDigest {
    pub(super) commands: usize,
    pub(super) commands_active: bool,
    pub(super) tools: usize,
    pub(super) tools_active: bool,
    pub(super) reads: usize,
    pub(super) reads_active: bool,
    pub(super) edits: usize,
    pub(super) edits_active: bool,
    pub(super) agents: usize,
    pub(super) agents_active: bool,
    pub(super) thoughts: usize,
    pub(super) thoughts_active: bool,
}

impl TurnDigest {
    fn record_tool(&mut self, name: &str, status: &ToolCallStatus) {
        if name == meerai_agents::QUESTION_TOOL {
            return;
        }
        let active = matches!(status, ToolCallStatus::Proposed | ToolCallStatus::Running);
        match name {
            "code-shell" | "sac-run_script" => {
                self.commands += 1;
                self.commands_active |= active;
            }
            "code-read_file" => {
                self.reads += 1;
                self.reads_active |= active;
            }
            "code-write_file" | "code-edit_file" => {
                self.edits += 1;
                self.edits_active |= active;
            }
            "subagent-run" | "acp-run" => {
                self.agents += 1;
                self.agents_active |= active;
            }
            _ => {
                self.tools += 1;
                self.tools_active |= active;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.commands + self.tools + self.reads + self.edits + self.agents + self.thoughts == 0
    }

    fn summary(&self) -> String {
        fn part(
            count: usize,
            active: bool,
            gerund: &str,
            singular: &str,
            plural: &str,
        ) -> Option<String> {
            match count {
                0 => None,
                n if active => Some(format!("{gerund} {n}")),
                1 => Some(format!("1 {singular}")),
                n => Some(format!("{n} {plural}")),
            }
        }
        [
            part(
                self.commands,
                self.commands_active,
                "Running",
                "command",
                "commands",
            ),
            part(self.tools, self.tools_active, "Calling", "tool", "tools"),
            part(self.reads, self.reads_active, "Reading", "read", "reads"),
            part(self.edits, self.edits_active, "Editing", "edit", "edits"),
            part(
                self.agents,
                self.agents_active,
                "Delegating",
                "agent",
                "agents",
            ),
            part(
                self.thoughts,
                self.thoughts_active,
                "Thinking",
                "thought",
                "thoughts",
            ),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

#[derive(Clone, Copy)]
pub(super) enum DigestItem {
    Reasoning(usize),
    ToolCall(usize),
    Subagent(usize),
}

pub(super) struct ActivityGroup {
    pub(super) start_idx: usize,
    pub(super) digest: TurnDigest,
    pub(super) items: Vec<DigestItem>,
}

pub(super) enum TranscriptSegment {
    Digest(ActivityGroup),
    Entry(usize),
}

pub(super) fn segment_transcript(transcript: &[TranscriptEntry]) -> Vec<TranscriptSegment> {
    let mut segments = Vec::new();
    let mut current: Option<ActivityGroup> = None;

    let flush = |segments: &mut Vec<TranscriptSegment>, current: &mut Option<ActivityGroup>| {
        if let Some(group) = current.take()
            && !group.digest.is_empty()
        {
            segments.push(TranscriptSegment::Digest(group));
        }
    };

    for (idx, entry) in transcript.iter().enumerate() {
        match entry {
            TranscriptEntry::ToolCall(tc) if tc.name == meerai_agents::QUESTION_TOOL => continue,
            TranscriptEntry::Assistant {
                text, reasoning, ..
            } if text.is_empty() && reasoning.is_empty() => continue,
            TranscriptEntry::Assistant {
                text,
                reasoning: _,
                streaming,
                ..
            } if text.is_empty() => {
                let group = current.get_or_insert_with(|| ActivityGroup {
                    start_idx: idx,
                    digest: TurnDigest::default(),
                    items: Vec::new(),
                });
                group.digest.thoughts += 1;
                group.digest.thoughts_active |= *streaming;
                group.items.push(DigestItem::Reasoning(idx));
            }
            TranscriptEntry::ToolCall(tc) => {
                let group = current.get_or_insert_with(|| ActivityGroup {
                    start_idx: idx,
                    digest: TurnDigest::default(),
                    items: Vec::new(),
                });
                group.digest.record_tool(&tc.name, &tc.status);
                group.items.push(DigestItem::ToolCall(idx));
            }
            TranscriptEntry::Subagent(sub) => {
                let group = current.get_or_insert_with(|| ActivityGroup {
                    start_idx: idx,
                    digest: TurnDigest::default(),
                    items: Vec::new(),
                });
                group.digest.agents += 1;
                group.digest.agents_active |= matches!(
                    sub.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                );
                group.items.push(DigestItem::Subagent(idx));
            }
            TranscriptEntry::Assistant { reasoning, .. } => {
                if !reasoning.is_empty() {
                    let group = current.get_or_insert_with(|| ActivityGroup {
                        start_idx: idx,
                        digest: TurnDigest::default(),
                        items: Vec::new(),
                    });
                    group.digest.thoughts += 1;
                    group.items.push(DigestItem::Reasoning(idx));
                }
                flush(&mut segments, &mut current);
                segments.push(TranscriptSegment::Entry(idx));
            }
            TranscriptEntry::User { .. }
            | TranscriptEntry::Question { .. }
            | TranscriptEntry::System { .. }
            | TranscriptEntry::Error { .. } => {
                flush(&mut segments, &mut current);
                segments.push(TranscriptSegment::Entry(idx));
            }
        }
    }

    flush(&mut segments, &mut current);
    segments
}

pub(crate) fn is_activity_group_start(transcript: &[TranscriptEntry], idx: usize) -> bool {
    segment_transcript(transcript)
        .iter()
        .any(|seg| matches!(seg, TranscriptSegment::Digest(group) if group.start_idx == idx))
}

pub(super) fn digest_line(digest: &TurnDigest, expanded: bool) -> Option<Line<'static>> {
    if digest.is_empty() {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(
            if expanded { "- " } else { "+ " },
            Style::default().fg(theme::p().faint),
        ),
        Span::styled(digest.summary(), Style::default().fg(theme::p().faint)),
    ]))
}

pub(super) fn activity_verbs(name: &str) -> (&'static str, &'static str) {
    match name {
        "code-read_file" => ("Reading", "Read"),
        "code-write_file" | "code-edit_file" => ("Editing", "Edited"),
        "code-shell" => ("Running", "Ran"),
        "sac-run_script" => ("Running script", "Ran script"),
        "code-grep" => ("Searching", "Searched"),
        "code-glob" => ("Matching", "Matched"),
        "code-list_dir" => ("Listing", "Listed"),
        "retrieval-repo_map" => ("Mapping", "Mapped"),
        "question-ask" => ("Asking", "Asked"),
        name if name.starts_with("lsp-") => ("Inspecting", "Inspected"),
        _ => ("Using", "Used"),
    }
}
