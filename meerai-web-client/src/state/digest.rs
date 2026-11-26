use super::*;

pub fn activity_verb(tool_name: &str) -> &'static str {
    match tool_name {
        "code-read_file" => "Reading",
        "code-write_file" | "code-edit_file" => "Editing",
        "code-list_dir" => "Listing",
        "code-grep" => "Searching",
        "code-glob" => "Matching",
        "code-shell" => "Running",
        "sac-run_script" => "Running script",
        "retrieval-repo_map" => "Mapping",
        "question-ask" => "Asking",
        "subagent-run" => "Delegating subagent",
        "acp-run" => "Delegating ACP agent",
        name if name.starts_with("lsp-") => "Inspecting",
        _ => "Using",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityCategory {
    Command,
    Tool,
    Read,
    Edit,
    Agent,
    Thought,
}

pub fn activity_category(tool_name: &str) -> ActivityCategory {
    match tool_name {
        "code-shell" | "sac-run_script" => ActivityCategory::Command,
        "code-read_file" => ActivityCategory::Read,
        "code-write_file" | "code-edit_file" => ActivityCategory::Edit,
        "subagent-run" | "acp-run" => ActivityCategory::Agent,
        _ => ActivityCategory::Tool,
    }
}

pub fn activity_summary(blocks: &[(&str, bool)], thoughts: usize) -> String {
    fn part(count: usize, active: bool, gerund: &str, one: &str, many: &str) -> Option<String> {
        match count {
            0 => None,
            n if active => Some(format!("{gerund} {n}")),
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        }
    }
    let mut counts = [0usize; 6];
    let mut active = [false; 6];
    for (name, running) in blocks {
        let idx = activity_category(name) as usize;
        counts[idx] += 1;
        active[idx] |= *running;
    }
    counts[ActivityCategory::Thought as usize] = thoughts;
    [
        part(counts[0], active[0], "Running", "command", "commands"),
        part(counts[1], active[1], "Calling", "tool", "tools"),
        part(counts[2], active[2], "Reading", "read", "reads"),
        part(counts[3], active[3], "Editing", "edit", "edits"),
        part(counts[4], active[4], "Delegating", "agent", "agents"),
        part(counts[5], active[5], "Thinking", "thought", "thoughts"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

#[derive(Clone, Copy, Debug)]
pub enum DigestItem<'a> {
    Reasoning(&'a TranscriptEntryDto),
    ToolCall(&'a TranscriptEntryDto),
    Subagent(&'a TranscriptEntryDto),
}

#[derive(Clone, Debug)]
pub enum TranscriptBlock<'a> {
    User {
        turn_num: usize,
        id: &'a str,
        turn_id: Option<&'a str>,
        text: &'a str,
    },
    ActivityDigest {
        digest_id: String,
        items: Vec<DigestItem<'a>>,
    },
    Assistant(&'a TranscriptEntryDto),
    System(&'a TranscriptEntryDto),
    Error(&'a TranscriptEntryDto),
    Question(&'a TranscriptEntryDto),
}

pub fn group_transcript_blocks<'a>(entries: &'a [TranscriptEntryDto]) -> Vec<TranscriptBlock<'a>> {
    let mut blocks: Vec<TranscriptBlock<'a>> = Vec::new();
    let mut turn_num = 0usize;
    let mut folded: Vec<DigestItem<'a>> = Vec::new();

    let flush = |blocks: &mut Vec<TranscriptBlock<'a>>, folded: &mut Vec<DigestItem<'a>>| {
        if folded.is_empty() {
            return;
        }
        let items = std::mem::take(folded);
        let first_id = match items[0] {
            DigestItem::Reasoning(e) | DigestItem::ToolCall(e) | DigestItem::Subagent(e) => {
                entry_id_of(e)
            }
        };
        blocks.push(TranscriptBlock::ActivityDigest {
            digest_id: format!("digest-{first_id}"),
            items,
        });
    };

    for entry in entries {
        match entry {
            TranscriptEntryDto::ToolCall { name, .. } if name == crate::state::QUESTION_TOOL => {
                continue;
            }
            TranscriptEntryDto::Assistant {
                text, reasoning, ..
            } if text.trim().is_empty() && reasoning.trim().is_empty() => continue,
            TranscriptEntryDto::User { id, turn_id, text } => {
                flush(&mut blocks, &mut folded);
                turn_num += 1;
                blocks.push(TranscriptBlock::User {
                    turn_num,
                    id,
                    turn_id: turn_id.as_deref(),
                    text,
                });
            }
            TranscriptEntryDto::Assistant {
                text, reasoning, ..
            } if text.trim().is_empty() => {
                if !reasoning.trim().is_empty() {
                    folded.push(DigestItem::Reasoning(entry));
                }
            }
            TranscriptEntryDto::ToolCall { .. } => {
                folded.push(DigestItem::ToolCall(entry));
            }
            TranscriptEntryDto::Subagent { .. } => {
                folded.push(DigestItem::Subagent(entry));
            }
            TranscriptEntryDto::Assistant { reasoning, .. } => {
                if !reasoning.trim().is_empty() {
                    folded.push(DigestItem::Reasoning(entry));
                }
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::Assistant(entry));
            }
            TranscriptEntryDto::System { .. } => {
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::System(entry));
            }
            TranscriptEntryDto::Error { .. } => {
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::Error(entry));
            }
            TranscriptEntryDto::Question { .. } => {
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::Question(entry));
            }
        }
    }
    flush(&mut blocks, &mut folded);
    blocks
}

pub fn entry_id_of(entry: &TranscriptEntryDto) -> &str {
    match entry {
        TranscriptEntryDto::User { id, .. }
        | TranscriptEntryDto::Assistant { id, .. }
        | TranscriptEntryDto::System { id, .. }
        | TranscriptEntryDto::Error { id, .. }
        | TranscriptEntryDto::ToolCall { id, .. }
        | TranscriptEntryDto::Subagent { id, .. }
        | TranscriptEntryDto::Question { id, .. } => id,
    }
}
