use meerai_agents::AgentCommand;
use meerai_common::definitions::{AgentDefinition, SkillDefinition, validate_agent_model};
use meerai_common::settings::default_data_dir;
use meerai_providers::providers::format_auth_list;
use meerai_providers::secrets::auth_file_path;

use super::*;
use crate::search::SearchMode;

impl AppState {
    pub(super) fn apply_mentions(&mut self, plan: &MentionPlan) {
        for index in &plan.skill_indices {
            let Some(skill) = self.skills.get(*index).cloned() else {
                continue;
            };
            self.send_skill(&skill);
        }
        for index in &plan.agent_indices {
            let Some(agent) = self.agents.get(*index).cloned() else {
                continue;
            };
            let model = self.agent_model_spec(&agent);
            self.send_agent(&agent, model);
        }
    }

    pub(super) fn acknowledge_mentions(&mut self, plan: &MentionPlan) {
        let mut messages = Vec::new();
        for index in &plan.skill_indices {
            if let Some(skill) = self.skills.get(*index) {
                messages.push(format!(
                    "Applied skill {} ({}).",
                    skill.id,
                    skill.source.label()
                ));
            }
        }
        for index in &plan.agent_indices {
            if let Some(agent) = self.agents.get(*index) {
                messages.push(format!(
                    "Applied agent {} ({}).",
                    agent.id,
                    agent.source.label()
                ));
            }
        }
        for message in messages {
            self.transcript
                .push(TranscriptEntry::System { text: message });
        }
    }

    pub(super) fn handle_slash_command(&mut self, command: &str) {
        let mut parts = command.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or_default();
        let arg = parts.next().map(str::trim).unwrap_or_default();

        match cmd {
            "/help" => {
                self.auto_follow = true;
                self.transcript.push(TranscriptEntry::System {
                    text: format_help_text(),
                });
            }
            "/auth" => {
                self.auto_follow = true;
                let data_dir = default_data_dir();
                let auth_path = data_dir.as_deref().map(auth_file_path);
                let runtime = self.runtime.read().unwrap();
                let text = format_auth_list(
                    &runtime.catalog,
                    self.secrets.as_ref(),
                    auth_path.as_deref(),
                    self.config_dir.as_deref(),
                );
                self.transcript.push(TranscriptEntry::System { text });
            }
            "/sidebar" => {
                self.toggle_sidebar();
            }
            "/search" | "/tv" | "/find" => {
                self.open_search_modal(SearchMode::Files, arg);
            }
            "/grep" => {
                self.open_search_modal(SearchMode::Content, arg);
            }
            "/diff" => {
                if self.file_changes.is_empty() {
                    self.status_notice = Some("Working tree is clean".to_string());
                } else {
                    let idx = if arg.is_empty() {
                        0
                    } else {
                        self.file_changes
                            .iter()
                            .position(|c| c.path.contains(arg))
                            .unwrap_or(0)
                    };
                    self.open_diff_modal(idx);
                }
            }
            "/connect" | "/provider" => {
                self.open_provider_select();
            }
            "/models" => {
                self.open_model_input();
            }
            "/agent" => {
                self.apply_agent(arg);
            }
            "/skill" => {
                self.apply_skill(arg);
            }
            "/model" => {
                self.auto_follow = true;
                if arg.is_empty() {
                    self.open_model_input();
                } else {
                    let (target_provider, target_model) = self.resolve_model_target(arg);
                    self.select_provider_and_model(&target_provider, &target_model);
                }
            }
            "/clear" => {
                self.transcript.clear();
                self.pending_assistant_idx = None;
                self.assistant_items.clear();
                self.tool_items.clear();
                self.subagent_items.clear();
                self.scroll_offset = 0;
                self.total_lines = 0;
                self.auto_follow = true;
            }
            "/reset" => {
                self.transcript.clear();
                self.pending_assistant_idx = None;
                self.assistant_items.clear();
                self.tool_items.clear();
                self.subagent_items.clear();
                self.turn_id = None;
                self.usage = UsageStats::default();
                self.metrics = TurnMetrics::default();
                self.scroll_offset = 0;
                self.total_lines = 0;
                self.auto_follow = true;
                self.is_running = false;
                self.active_tool = None;
                let _ = self.cmd_tx.send(AgentCommand::Reset);
                self.transcript.push(TranscriptEntry::System {
                    text: "Conversation reset.".to_string(),
                });
            }
            "/cancel" => {
                if self.is_running {
                    let _ = self.cmd_tx.send(AgentCommand::Cancel);
                    self.status_notice = Some("Cancelling turn...".to_string());
                } else {
                    self.transcript.push(TranscriptEntry::System {
                        text: "No active turn to cancel.".to_string(),
                    });
                }
            }
            "/compact" => {
                self.auto_follow = true;
                self.transcript.push(TranscriptEntry::System {
                    text: "Compaction requested.".to_string(),
                });
                let _ = self.cmd_tx.send(AgentCommand::Compact);
            }
            "/mode" => {
                self.auto_follow = true;
                if arg.is_empty() {
                    self.transcript.push(TranscriptEntry::System {
                        text: format!("Approval mode: {}", approval_mode_label(self.approval_mode)),
                    });
                } else {
                    match parse_approval_mode(arg) {
                        Some(mode) => {
                            self.approval_mode = mode;
                            let _ = self.cmd_tx.send(AgentCommand::SetPermissionMode(mode));
                            self.transcript.push(TranscriptEntry::System {
                                text: format!("Approval mode: {}", approval_mode_label(mode)),
                            });
                        }
                        None => {
                            self.transcript.push(TranscriptEntry::Error {
                                text: format!(
                                    "Invalid mode: {arg}. Expected: plan, ask, auto-edit, yolo"
                                ),
                                expanded: false,
                            });
                        }
                    }
                }
            }
            "/quit" | "/q" => {
                self.quit();
            }
            other => {
                self.auto_follow = true;
                self.transcript.push(TranscriptEntry::Error {
                    text: format!("Unknown command: {other}. Type /help for available commands."),
                    expanded: false,
                });
            }
        }
    }

    pub(super) fn available_agent_ids(&self) -> String {
        if self.agents.is_empty() {
            return "(none)".to_string();
        }
        self.agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn available_skill_ids(&self) -> String {
        if self.skills.is_empty() {
            return "(none)".to_string();
        }
        self.skills
            .iter()
            .map(|skill| skill.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn apply_agent(&mut self, id: &str) {
        self.auto_follow = true;
        let Some(agent) = self.agents.iter().find(|agent| agent.id == id).cloned() else {
            let available = self.available_agent_ids();
            let message = if id.is_empty() {
                format!("Usage: /agent <id>. Available agents: {available}")
            } else {
                format!("Unknown agent: {id}. Available agents: {available}")
            };
            self.transcript
                .push(TranscriptEntry::System { text: message });
            return;
        };
        let model = self.agent_model_spec(&agent);
        let source = agent.source.label();
        self.send_agent(&agent, model);
        self.transcript.push(TranscriptEntry::System {
            text: format!("Applied agent {} ({source}).", agent.id),
        });
    }

    pub(super) fn apply_skill(&mut self, id: &str) {
        self.auto_follow = true;
        let Some(skill) = self.skills.iter().find(|skill| skill.id == id).cloned() else {
            let available = self.available_skill_ids();
            let message = if id.is_empty() {
                format!("Usage: /skill <id>. Available skills: {available}")
            } else {
                format!("Unknown skill: {id}. Available skills: {available}")
            };
            self.transcript
                .push(TranscriptEntry::System { text: message });
            return;
        };
        let source = skill.source.label();
        self.send_skill(&skill);
        self.transcript.push(TranscriptEntry::System {
            text: format!("Applied skill {} ({source}).", skill.id),
        });
    }

    pub(super) fn send_agent(&mut self, agent: &AgentDefinition, model: Option<String>) {
        let _ = self.cmd_tx.send(AgentCommand::ApplyAgent {
            name: agent.id.clone(),
            instructions: agent.prompt.clone(),
            model,
        });
    }

    pub(super) fn send_skill(&mut self, skill: &SkillDefinition) {
        let _ = self.cmd_tx.send(AgentCommand::ApplySkill {
            name: skill.id.clone(),
            instructions: skill.body.clone(),
        });
    }

    pub(super) fn agent_model_spec(&mut self, agent: &AgentDefinition) -> Option<String> {
        let raw = match validate_agent_model(agent.model.as_deref()) {
            Ok(Some(raw)) => raw,
            Ok(None) => return None,
            Err(raw) => {
                self.transcript.push(TranscriptEntry::System {
                    text: format!(
                        "Agent {} model '{raw}' implies a provider change; it is not applied.",
                        agent.id
                    ),
                });
                return None;
            }
        };
        let spec = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.format_genai_spec(&self.active_provider, raw)
        };
        self.model = raw.to_string();
        Some(spec)
    }
}

fn format_help_text() -> String {
    let commands = crate::completion::SLASH_COMMANDS
        .iter()
        .map(|(cmd, _, _)| match *cmd {
            "/model" => "/model [name]".to_string(),
            "/agent" => "/agent <id> (or @agent/<id>)".to_string(),
            "/skill" => "/skill <id> (or @skill/<id>)".to_string(),
            "/mode" => "/mode [plan|ask|auto-edit|yolo]".to_string(),
            "/search" => "/search [query]".to_string(),
            "/grep" => "/grep [query]".to_string(),
            "/diff" => "/diff [path]".to_string(),
            other => (*other).to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Commands: {commands}\n\
         Shortcuts: Ctrl-f search, Ctrl-t toggle thinking, Ctrl-b toggle sidebar, click thinking or changed file\n\
         Insert: Enter send, Shift/Alt+Enter newline, @ file, / command, Esc Normal\n\
         Normal: j/k/Up/Down scroll, / search, t toggle thinking, Ctrl-d/Ctrl-u half-page, gg/G top/bottom, i/a Insert, q quit"
    )
}

const MENTION_PREFIX_LEN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MentionKind {
    Agent,
    Skill,
}

pub(crate) fn mention_is_known(
    kind: MentionKind,
    id: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> bool {
    match kind {
        MentionKind::Agent => agents.iter().any(|agent| agent.id == id),
        MentionKind::Skill => skills.iter().any(|skill| skill.id == id),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct MentionPlan {
    pub(super) agent_indices: Vec<usize>,
    pub(super) skill_indices: Vec<usize>,
    pub(super) text: Option<String>,
}

pub(super) fn parse_mentions(
    text: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> MentionPlan {
    let mut agent_indices: Vec<usize> = Vec::new();
    let mut skill_indices: Vec<usize> = Vec::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut changed = false;
    let mut index = 0;

    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        if ch == '@'
            && let Some((kind, id, end)) = parse_mention_token(text, index)
        {
            let found = match kind {
                MentionKind::Agent => agents.iter().position(|agent| agent.id == id).map(|pos| {
                    if !agent_indices.contains(&pos) {
                        agent_indices.push(pos);
                    }
                }),
                MentionKind::Skill => skills.iter().position(|skill| skill.id == id).map(|pos| {
                    if !skill_indices.contains(&pos) {
                        skill_indices.push(pos);
                    }
                }),
            };
            if found.is_some() {
                changed = true;
                index = end;
                continue;
            }
        }
        cleaned.push(ch);
        index += ch.len_utf8();
    }

    MentionPlan {
        agent_indices,
        skill_indices,
        text: changed.then(|| collapse_whitespace(&cleaned)),
    }
}

pub(crate) fn parse_mention_token(text: &str, start: usize) -> Option<(MentionKind, &str, usize)> {
    let rest = text.get(start + 1..)?;
    let (kind, rest) = if let Some(rest) = rest.strip_prefix("agent/") {
        (MentionKind::Agent, rest)
    } else {
        let rest = rest.strip_prefix("skill/")?;
        (MentionKind::Skill, rest)
    };
    let id_len = rest
        .char_indices()
        .find(|(_, ch)| !is_mention_id_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    if id_len == 0 {
        return None;
    }
    Some((
        kind,
        &rest[..id_len],
        start + 1 + MENTION_PREFIX_LEN + id_len,
    ))
}

fn is_mention_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

fn collapse_whitespace(text: &str) -> String {
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
