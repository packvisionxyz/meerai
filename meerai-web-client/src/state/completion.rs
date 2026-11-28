use super::commands::*;
use super::misc::*;
use super::types::*;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Model,
    Mention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MentionCategory {
    Agent,
    Skill,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub insert: String,
    pub detail: String,
    pub category: Option<MentionCategory>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletionState {
    pub active: bool,
    pub kind: Option<CompletionKind>,
    pub replace_start: usize,
    pub replace_end: usize,
    pub query: String,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

pub(crate) fn push_applied_mentions_notice(
    tab: &mut SessionTabState,
    skills: &[String],
    agent: Option<&str>,
) {
    let mut applied = Vec::new();
    for s in skills {
        applied.push(format!("skill:{s}"));
    }
    if let Some(a) = agent {
        applied.push(format!("agent:{a}"));
    }
    if !applied.is_empty() {
        push_local_system(tab, format!("Applied {}", applied.join(", ")));
    }
}

pub fn update_completion(state: &mut AppState) {
    let text = state.draft.text.as_str();
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        state.completion = CompletionState::default();
        return;
    }

    if let Some(after_model) = text.strip_prefix("/model ") {
        let q = after_model.trim();
        let active_provider = state
            .active_session()
            .map(|s| s.provider.as_str())
            .unwrap_or("openai");
        let items = model_completion_items(state.providers.as_ref(), active_provider, q);
        state.completion = CompletionState {
            active: !items.is_empty(),
            kind: Some(CompletionKind::Model),
            replace_start: 7,
            replace_end: text.len(),
            query: q.to_owned(),
            items,
            selected: 0,
        };
        return;
    }

    if text.starts_with('/') && !text.contains(char::is_whitespace) {
        let q = text.trim().to_ascii_lowercase();
        let items = SLASH_COMMANDS
            .iter()
            .filter(|c| c.clients.has_web())
            .filter(|c| c.name.contains(&q) || c.desc.to_ascii_lowercase().contains(&q[1..]))
            .map(|c| CompletionItem {
                label: c.label(),
                insert: c.insert(),
                detail: c.desc.to_owned(),
                category: None,
            })
            .collect::<Vec<_>>();
        state.completion = CompletionState {
            active: !items.is_empty(),
            kind: Some(CompletionKind::Command),
            replace_start: 0,
            replace_end: text.len(),
            query: q,
            items,
            selected: 0,
        };
        return;
    }

    if let Some((start, end, q)) = trailing_at_token(text) {
        if state.workspace_files.is_empty()
            && let Some(t) = crate::transport::current()
        {
            t.fetch_workspace_files();
        }
        let items = mention_completion_items(q, &state.host_info, &state.workspace_files);
        state.completion = CompletionState {
            active: !items.is_empty(),
            kind: Some(CompletionKind::Mention),
            replace_start: start,
            replace_end: end,
            query: q.to_owned(),
            items,
            selected: 0,
        };
        return;
    }

    state.completion = CompletionState::default();
}

pub fn accept_completion(state: &mut AppState, index: usize) -> bool {
    let Some(item) = state.completion.items.get(index).cloned() else {
        return false;
    };
    let start = state.completion.replace_start.min(state.draft.text.len());
    let end = state.completion.replace_end.min(state.draft.text.len());
    let mut updated = String::with_capacity(state.draft.text.len() + item.insert.len());
    updated.push_str(&state.draft.text[..start]);
    updated.push_str(&item.insert);
    if matches!(state.completion.kind, Some(CompletionKind::Mention)) && !item.insert.ends_with(' ')
    {
        updated.push(' ');
    }
    updated.push_str(&state.draft.text[end..]);
    state.draft.replace_text(updated);
    let current_text = state.draft.text.clone();
    if let Some(tab) = state.active_tab_mut() {
        tab.draft_text = current_text;
    }
    state.completion = CompletionState::default();
    true
}

pub fn trailing_at_token(text: &str) -> Option<(usize, usize, &str)> {
    let last_token_start = text.rfind(char::is_whitespace).map_or(0, |idx| {
        idx + text[idx..].chars().next().map_or(1, char::len_utf8)
    });
    let token = &text[last_token_start..];
    token
        .strip_prefix('@')
        .map(|query| (last_token_start, text.len(), query))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedMentions {
    pub agent: Option<String>,
    pub skills: Vec<String>,
    pub mention_only: bool,
    pub cleaned: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionSegment {
    pub category: Option<MentionCategory>,
    pub text: String,
}

pub fn mention_segments(input: &str, host: &HostInfoDto) -> Vec<MentionSegment> {
    let agent_ids: HashSet<&str> = host.agents.iter().map(|a| a.id.as_str()).collect();
    let skill_ids: HashSet<&str> = host.skills.iter().map(|s| s.id.as_str()).collect();
    let mut segments = Vec::new();
    let mut plain_start = 0;
    let mut index = 0;

    while let Some(offset) = input[index..].find('@') {
        let at = index + offset;
        if let Some((category, id, end)) = scan_mention(input, at, &agent_ids, &skill_ids) {
            if at > plain_start {
                segments.push(MentionSegment {
                    category: None,
                    text: input[plain_start..at].to_owned(),
                });
            }
            segments.push(MentionSegment {
                category: Some(category),
                text: id.to_owned(),
            });
            index = end;
            plain_start = end;
        } else {
            index = at + 1;
        }
    }

    if plain_start < input.len() {
        segments.push(MentionSegment {
            category: None,
            text: input[plain_start..].to_owned(),
        });
    }
    segments
}

pub(crate) fn is_mention_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

pub(crate) fn scan_mention<'a>(
    input: &'a str,
    at: usize,
    agent_ids: &HashSet<&str>,
    skill_ids: &HashSet<&str>,
) -> Option<(MentionCategory, &'a str, usize)> {
    let rest = input.get(at + 1..)?;
    let (category, rest) = if let Some(rest) = rest.strip_prefix("agent/") {
        (MentionCategory::Agent, rest)
    } else if let Some(rest) = rest.strip_prefix("skill/") {
        (MentionCategory::Skill, rest)
    } else {
        let (id, after) = ["orchestrate", "goal"]
            .into_iter()
            .find_map(|id| rest.strip_prefix(id).map(|after| (id, after)))?;
        if after.chars().next().is_some_and(is_mention_id_char) {
            return None;
        }
        return skill_ids
            .contains(id)
            .then_some((MentionCategory::Skill, id, at + 1 + id.len()));
    };
    let id_len = rest
        .char_indices()
        .find(|(_, ch)| !is_mention_id_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    if id_len == 0 {
        return None;
    }
    let id = &rest[..id_len];
    let known = match category {
        MentionCategory::Agent => agent_ids.contains(id),
        MentionCategory::Skill => skill_ids.contains(id),
        MentionCategory::File => false,
    };
    known.then_some((category, id, at + 1 + 6 + id_len))
}

pub fn extract_mentions(input: &str, host: &HostInfoDto) -> ExtractedMentions {
    let mut agent = None;
    let mut skills = Vec::new();
    let mut seen_skills = HashSet::new();
    let mut cleaned = String::new();
    let mut has_non_definition_text = false;

    for segment in mention_segments(input, host) {
        match segment.category {
            Some(MentionCategory::Agent) => agent = Some(segment.text),
            Some(MentionCategory::Skill) => {
                if seen_skills.insert(segment.text.clone()) {
                    skills.push(segment.text);
                }
            }
            Some(MentionCategory::File) | None => {
                if !segment.text.trim().is_empty() {
                    has_non_definition_text = true;
                }
                cleaned.push_str(&segment.text);
            }
        }
    }

    ExtractedMentions {
        mention_only: !has_non_definition_text && (agent.is_some() || !skills.is_empty()),
        agent,
        skills,
        cleaned: collapse_whitespace(&cleaned),
    }
}

pub fn resolved_mention_chips(
    input: &str,
    host: &HostInfoDto,
) -> Vec<(MentionCategory, String, String)> {
    let agent_ids: HashSet<&str> = host.agents.iter().map(|a| a.id.as_str()).collect();
    let skill_ids: HashSet<&str> = host.skills.iter().map(|s| s.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for token in input.split_whitespace() {
        if let Some(name) = token.strip_prefix("@agent/")
            && agent_ids.contains(name)
            && seen.insert(token.to_owned())
        {
            out.push((MentionCategory::Agent, name.to_owned(), token.to_owned()));
        } else if let Some(name) = token.strip_prefix("@skill/")
            && skill_ids.contains(name)
            && seen.insert(token.to_owned())
        {
            out.push((MentionCategory::Skill, name.to_owned(), token.to_owned()));
        } else if let Some(id) = token.strip_prefix('@')
            && matches!(id, "goal" | "orchestrate")
            && skill_ids.contains(id)
            && seen.insert(token.to_owned())
        {
            out.push((MentionCategory::Skill, id.to_owned(), token.to_owned()));
        }
    }
    out
}

pub fn remove_mention_token(input: &str, token: &str) -> String {
    let kept: Vec<&str> = input.split_whitespace().filter(|t| *t != token).collect();
    kept.join(" ")
}

pub fn model_completion_items(
    providers: Option<&ProvidersResponse>,
    active_provider: &str,
    query: &str,
) -> Vec<CompletionItem> {
    let Some(providers) = providers else {
        return Vec::new();
    };
    let q = query.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(recent) = providers.recent_models.get(active_provider) {
        for m in recent {
            if (q.is_empty() || m.to_ascii_lowercase().contains(&q))
                && seen.insert((active_provider.to_owned(), m.clone()))
            {
                out.push(CompletionItem {
                    label: format!("{active_provider}/{m}"),
                    insert: format!("{active_provider}/{m}"),
                    detail: "recent".to_owned(),
                    category: None,
                });
            }
        }
    }

    for model in &providers.models {
        let full = format!("{}/{}", model.provider, model.id);
        if (q.is_empty()
            || model.id.to_ascii_lowercase().contains(&q)
            || full.to_ascii_lowercase().contains(&q))
            && seen.insert((model.provider.clone(), model.id.clone()))
        {
            let detail = model
                .context_window_tokens
                .map(|c| format!("{}k ctx", c / 1000))
                .unwrap_or_else(|| model.provider.clone());
            out.push(CompletionItem {
                label: full.clone(),
                insert: full,
                detail,
                category: None,
            });
        }
        if out.len() >= 18 {
            break;
        }
    }
    out
}

pub fn mention_completion_items(
    query: &str,
    host: &HostInfoDto,
    files: &[String],
) -> Vec<CompletionItem> {
    let q = query.to_ascii_lowercase();
    let (kind_filter, needle) = if let Some(rest) = q.strip_prefix("agent/") {
        (Some(MentionCategory::Agent), rest)
    } else if let Some(rest) = q.strip_prefix("skill/") {
        (Some(MentionCategory::Skill), rest)
    } else {
        (None, q.as_str())
    };

    let mut ranked: Vec<(u8, String, CompletionItem)> = Vec::new();

    if kind_filter.is_none() || kind_filter == Some(MentionCategory::Agent) {
        for agent in &host.agents {
            let label = format!("@agent/{}", agent.id);
            if needle.is_empty()
                || agent.id.to_ascii_lowercase().contains(needle)
                || agent
                    .description
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains(needle))
            {
                ranked.push((
                    score_rank(&agent.id, needle),
                    label.clone(),
                    CompletionItem {
                        label: label.clone(),
                        insert: label,
                        detail: agent
                            .description
                            .clone()
                            .unwrap_or_else(|| format!("agent ({})", agent.source)),
                        category: Some(MentionCategory::Agent),
                    },
                ));
            }
        }
    }

    if kind_filter.is_none() || kind_filter == Some(MentionCategory::Skill) {
        for skill in &host.skills {
            let label = if matches!(skill.id.as_str(), "goal" | "orchestrate") {
                format!("@{}", skill.id)
            } else {
                format!("@skill/{}", skill.id)
            };
            if needle.is_empty()
                || skill.id.to_ascii_lowercase().contains(needle)
                || skill
                    .description
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains(needle))
            {
                ranked.push((
                    score_rank(&skill.id, needle),
                    label.clone(),
                    CompletionItem {
                        label: label.clone(),
                        insert: label,
                        detail: skill
                            .description
                            .clone()
                            .unwrap_or_else(|| format!("skill ({})", skill.source)),
                        category: Some(MentionCategory::Skill),
                    },
                ));
            }
        }
    }

    if kind_filter.is_none() {
        for path in files {
            if needle.is_empty() || path.to_ascii_lowercase().contains(needle) {
                let base = path.rsplit('/').next().unwrap_or(path);
                ranked.push((
                    score_rank(base, needle),
                    path.clone(),
                    CompletionItem {
                        label: format!("@{path}"),
                        insert: format!("@{path}"),
                        detail: "file".to_owned(),
                        category: Some(MentionCategory::File),
                    },
                ));
            }
        }
    }

    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .take(16)
        .map(|(_, _, item)| item)
        .collect()
}

pub(crate) fn score_rank(candidate: &str, needle: &str) -> u8 {
    if needle.is_empty() {
        return 2;
    }
    let lower = candidate.to_ascii_lowercase();
    if lower == needle {
        0
    } else if lower.starts_with(needle) {
        1
    } else if lower.contains(needle) {
        2
    } else {
        3
    }
}
