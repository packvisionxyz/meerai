use std::path::PathBuf;
use std::sync::Arc;

use meerai_common::definitions::{AgentDefinition, DefinitionSource, SkillDefinition};
use meerai_core::text::floor_char_boundary;

use crate::workspace::{WorkspaceQuery, local_workspace};

const MAX_VISIBLE_ITEMS: usize = 50;

pub const SLASH_COMMANDS: &[(&str, &str, bool)] = &[
    ("/help", "Show available commands and keybindings", false),
    (
        "/sessions",
        "Switch to another session (or /sessions <id>)",
        false,
    ),
    ("/new", "Open a new session tab", false),
    (
        "/fork",
        "Fork the session at the latest closed turn into a new tab",
        false,
    ),
    ("/acp", "Start an ACP session (agy)", false),
    ("/close", "Close the current session tab", false),
    ("/auth", "Show active credential sources", false),
    ("/connect", "Configure provider API key and model", false),
    ("/provider", "Switch or configure AI provider", false),
    ("/models", "Open model input modal", false),
    ("/model", "Switch active LLM model", true),
    (
        "/effort",
        "Show, set, or cycle reasoning effort (low|medium|high)",
        true,
    ),
    (
        "/agent",
        "Apply an agent by id (or mention @agent/<id>)",
        true,
    ),
    ("/agents", "Choose an agent from the Mentions popup", false),
    (
        "/skill",
        "Apply a skill by id (or mention @skill/<id>)",
        true,
    ),
    ("/skills", "Choose a skill from the Mentions popup", false),
    (
        "/search",
        "Search files by name or content (ripgrep)",
        false,
    ),
    ("/grep", "Search file contents with ripgrep", false),
    ("/diff", "View 2-pane diff of changed files", false),
    (
        "/sidebar",
        "Toggle right status and file changes sidebar",
        false,
    ),
    ("/clear", "Clear visible chat transcript", false),
    ("/reset", "Clear transcript and reset agent history", false),
    ("/cancel", "Cancel the active agent turn", false),
    ("/compact", "Compact older turns to reclaim context", false),
    ("/mode", "Show or set approval mode (plan|ask|auto)", true),
    (
        "/rewind",
        "Restore code and/or conversation for the last turn",
        false,
    ),
    (
        "/reload",
        "Reload settings, theme, definitions, and prompt",
        false,
    ),
    (
        "/serve",
        "Start or stop local browser UI server (/serve stop)",
        false,
    ),
    ("/quit", "Exit meerai", false),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    SlashCommand,
    Model,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionItemKind {
    Plain,
    File,
    Agent,
    Skill,
}

impl CompletionItemKind {
    pub fn badge(self) -> Option<&'static str> {
        match self {
            CompletionItemKind::Plain | CompletionItemKind::File => None,
            CompletionItemKind::Agent => Some("agent"),
            CompletionItemKind::Skill => Some("skill"),
        }
    }

    fn mention_keyword(self) -> Option<&'static str> {
        match self {
            CompletionItemKind::Agent => Some("agent"),
            CompletionItemKind::Skill => Some("skill"),
            CompletionItemKind::Plain | CompletionItemKind::File => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub description: String,
    pub insert_text: String,
    pub item_kind: CompletionItemKind,
    pub search: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveTrigger {
    kind: CompletionKind,
    replace_start: usize,
    replace_end: usize,
    query: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DismissedKey {
    kind: CompletionKind,
    replace_start: usize,
    query: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MentionEntry {
    id: String,
    description: String,
}

#[derive(Debug)]
pub struct CompletionEngine {
    workspace: Arc<dyn WorkspaceQuery>,
    cached_files: Option<Vec<String>>,
    provider_models: Vec<(String, String)>,
    agent_mentions: Vec<MentionEntry>,
    skill_mentions: Vec<MentionEntry>,
    active_kind: Option<CompletionKind>,
    active_query: String,
    replace_range: Option<(usize, usize)>,
    items: Vec<CompletionItem>,
    selected: usize,
    dismissed: Option<DismissedKey>,
}

impl CompletionEngine {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self::with_workspace(local_workspace(workspace_root))
    }

    pub fn with_workspace(workspace: Arc<dyn WorkspaceQuery>) -> Self {
        Self {
            workspace,
            cached_files: None,
            provider_models: Vec::new(),
            agent_mentions: Vec::new(),
            skill_mentions: Vec::new(),
            active_kind: None,
            active_query: String::new(),
            replace_range: None,
            items: Vec::new(),
            selected: 0,
            dismissed: None,
        }
    }

    pub fn sync_models(&mut self, models: &[String], active_provider: &str) {
        self.provider_models = models
            .iter()
            .map(|model_id| (model_id.clone(), active_provider.to_string()))
            .collect();
    }

    pub fn set_definitions(&mut self, agents: &[AgentDefinition], skills: &[SkillDefinition]) {
        self.agent_mentions = agents
            .iter()
            .map(|agent| MentionEntry {
                id: agent.id.clone(),
                description: mention_description(agent.source, agent.description.as_deref()),
            })
            .collect();
        self.skill_mentions = skills
            .iter()
            .map(|skill| MentionEntry {
                id: skill.id.clone(),
                description: mention_description(skill.source, skill.description.as_deref()),
            })
            .collect();
    }

    pub fn is_active(&self) -> bool {
        self.active_kind.is_some() && !self.items.is_empty()
    }

    pub fn kind(&self) -> Option<CompletionKind> {
        self.active_kind
    }

    pub fn items(&self) -> &[CompletionItem] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_is_noop(&self, input: &str) -> bool {
        let Some(item) = self.items.get(self.selected) else {
            return false;
        };
        let Some((start, end)) = self.replace_range else {
            return false;
        };
        let start = floor_char_boundary(input, start);
        let end = floor_char_boundary(input, end.max(start));
        input[start..end] == item.insert_text
    }

    pub fn refresh_files(&mut self) {
        self.cached_files = Some(self.workspace.list_files());
    }

    pub fn workspace(&self) -> Arc<dyn WorkspaceQuery> {
        self.workspace.clone()
    }

    pub fn set_workspace(&mut self, workspace: Arc<dyn WorkspaceQuery>) {
        self.workspace = workspace;
        self.cached_files = None;
    }

    pub fn dismiss(&mut self) {
        if let (Some(kind), Some((start, _))) = (self.active_kind, self.replace_range) {
            self.dismissed = Some(DismissedKey {
                kind,
                replace_start: start,
                query: self.active_query.clone(),
            });
        }
        self.clear_popup();
    }

    pub fn reset(&mut self) {
        self.dismissed = None;
        self.clear_popup();
    }

    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
    }

    pub fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.items.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn set_selected(&mut self, index: usize) {
        if !self.items.is_empty() {
            self.selected = index.min(self.items.len() - 1);
        }
    }

    pub fn update(&mut self, input: &str, cursor: usize) {
        let cursor = floor_char_boundary(input, cursor);
        let Some(trigger) = detect_trigger(input, cursor) else {
            self.dismissed = None;
            self.clear_popup();
            return;
        };

        let dismiss_key = DismissedKey {
            kind: trigger.kind,
            replace_start: trigger.replace_start,
            query: trigger.query.clone(),
        };
        if self.dismissed.as_ref() == Some(&dismiss_key) {
            self.clear_popup();
            return;
        }
        self.dismissed = None;

        let candidates = match trigger.kind {
            CompletionKind::SlashCommand => build_slash_candidates(&trigger.query),
            CompletionKind::Model => build_model_candidates(&self.provider_models, &trigger.query),
            CompletionKind::File => {
                if self.cached_files.is_none() || self.active_kind != Some(CompletionKind::File) {
                    self.refresh_files();
                }
                let files = self.cached_files.as_deref().unwrap_or(&[]);
                build_mention_candidates(
                    &self.agent_mentions,
                    &self.skill_mentions,
                    files,
                    &trigger.query,
                )
            }
        };

        if candidates.is_empty() || is_exact_sole_match(&trigger, &candidates) {
            self.clear_popup();
            return;
        }

        let prev_label = self.items.get(self.selected).map(|item| item.label.clone());
        self.active_kind = Some(trigger.kind);
        self.active_query = trigger.query;
        self.replace_range = Some((trigger.replace_start, trigger.replace_end));
        self.items = candidates;
        self.selected = prev_label
            .and_then(|label| self.items.iter().position(|item| item.label == label))
            .unwrap_or(0);
    }

    pub fn accept(&mut self, input: &str, cursor: usize) -> Option<(String, usize)> {
        if !self.is_active() {
            return None;
        }
        let item = self.items.get(self.selected)?.clone();
        let (start, end) = self.replace_range?;
        let start = floor_char_boundary(input, start);
        let end = floor_char_boundary(input, end.max(start));
        let _ = cursor;

        let mut new_input = String::with_capacity(input.len() + item.insert_text.len());
        new_input.push_str(&input[..start]);
        new_input.push_str(&item.insert_text);
        let rest = &input[end..];
        let new_cursor = if item.insert_text.ends_with(' ') && rest.starts_with(' ') {
            new_input.push_str(&rest[1..]);
            start + item.insert_text.len()
        } else {
            new_input.push_str(rest);
            start + item.insert_text.len()
        };

        self.dismissed = None;
        self.clear_popup();
        Some((new_input, new_cursor))
    }

    fn clear_popup(&mut self) {
        self.active_kind = None;
        self.active_query.clear();
        self.replace_range = None;
        self.items.clear();
        self.selected = 0;
    }
}

fn token_end(input: &str, cursor: usize) -> usize {
    let rest = &input[cursor..];
    match rest.find(char::is_whitespace) {
        Some(offset) => cursor + offset,
        None => input.len(),
    }
}

fn detect_trigger(input: &str, cursor: usize) -> Option<ActiveTrigger> {
    let before = &input[..cursor];

    if let Some(rest) = before.strip_prefix("/model ")
        && !rest.contains(char::is_whitespace)
    {
        let replace_start = "/model ".len();
        return Some(ActiveTrigger {
            kind: CompletionKind::Model,
            replace_start,
            replace_end: token_end(input, cursor),
            query: rest.to_string(),
        });
    }

    if before.starts_with('/') && !before.contains(char::is_whitespace) {
        return Some(ActiveTrigger {
            kind: CompletionKind::SlashCommand,
            replace_start: 0,
            replace_end: token_end(input, cursor),
            query: before[1..].to_string(),
        });
    }

    let token_start = before
        .rfind(char::is_whitespace)
        .and_then(|idx| before[idx..].chars().next().map(|ch| idx + ch.len_utf8()))
        .unwrap_or(0);
    let token = &before[token_start..];
    if let Some(query) = token.strip_prefix('@') {
        return Some(ActiveTrigger {
            kind: CompletionKind::File,
            replace_start: token_start,
            replace_end: token_end(input, cursor),
            query: query.to_string(),
        });
    }

    None
}

fn is_exact_sole_match(trigger: &ActiveTrigger, candidates: &[CompletionItem]) -> bool {
    if candidates.len() != 1 {
        return false;
    }
    let only = &candidates[0];
    match trigger.kind {
        CompletionKind::SlashCommand => {
            let takes_arg = SLASH_COMMANDS
                .iter()
                .find(|(name, _, _)| *name == only.label)
                .is_some_and(|(_, _, arg)| *arg);
            !takes_arg && only.label == format!("/{}", trigger.query)
        }
        CompletionKind::Model => only.label == trigger.query,
        CompletionKind::File => {
            only.item_kind == CompletionItemKind::File && only.label == trigger.query
        }
    }
}

fn build_slash_candidates(query: &str) -> Vec<CompletionItem> {
    let mut scored: Vec<(MatchRank, CompletionItem)> = SLASH_COMMANDS
        .iter()
        .filter_map(|&(name, desc, takes_arg)| {
            let target = name.trim_start_matches('/');
            let rank = score_candidate(target, query)?;
            let insert_text = if takes_arg {
                format!("{name} ")
            } else {
                name.to_string()
            };
            Some((
                rank,
                CompletionItem {
                    label: name.to_string(),
                    description: desc.to_string(),
                    insert_text,
                    item_kind: CompletionItemKind::Plain,
                    search: target.to_string(),
                },
            ))
        })
        .collect();

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored
        .into_iter()
        .take(MAX_VISIBLE_ITEMS)
        .map(|(_, item)| item)
        .collect()
}

fn build_model_candidates(models: &[(String, String)], query: &str) -> Vec<CompletionItem> {
    let mut scored: Vec<(u8, usize, usize, CompletionItem)> = models
        .iter()
        .enumerate()
        .filter_map(|(idx, (name, desc))| {
            let rank = score_candidate(name, query)?;
            Some((
                rank.tier,
                rank.pos,
                idx,
                CompletionItem {
                    label: name.clone(),
                    description: desc.clone(),
                    insert_text: name.clone(),
                    item_kind: CompletionItemKind::Plain,
                    search: name.clone(),
                },
            ))
        })
        .collect();

    scored.sort_by_key(|a| (a.0, a.1, a.2));
    scored
        .into_iter()
        .take(MAX_VISIBLE_ITEMS)
        .map(|(_, _, _, item)| item)
        .collect()
}

fn mention_description(source: DefinitionSource, description: Option<&str>) -> String {
    let source = match source {
        DefinitionSource::Agents => ".agents",
        DefinitionSource::Claude => ".claude",
    };
    match description.map(str::trim).filter(|text| !text.is_empty()) {
        Some(description) => format!("{source} — {description}"),
        None => source.to_string(),
    }
}

fn build_mention_candidates(
    agents: &[MentionEntry],
    skills: &[MentionEntry],
    files: &[String],
    query: &str,
) -> Vec<CompletionItem> {
    let mut candidates = build_definition_candidates(agents, query, CompletionItemKind::Agent);
    candidates.extend(build_definition_candidates(
        skills,
        query,
        CompletionItemKind::Skill,
    ));
    candidates.extend(build_file_candidates(files, query));
    candidates.truncate(MAX_VISIBLE_ITEMS);
    candidates
}

fn build_definition_candidates(
    entries: &[MentionEntry],
    query: &str,
    kind: CompletionItemKind,
) -> Vec<CompletionItem> {
    let Some(keyword) = kind.mention_keyword() else {
        return Vec::new();
    };
    let mut scored: Vec<(MatchRank, CompletionItem)> = entries
        .iter()
        .filter_map(|entry| {
            let search = format!("{keyword}/{} {} {keyword}", entry.id, entry.id);
            let rank = score_candidate(&search, query)?;
            Some((
                rank,
                CompletionItem {
                    label: entry.id.clone(),
                    description: entry.description.clone(),
                    insert_text: format!("@{keyword}/{} ", entry.id),
                    item_kind: kind,
                    search,
                },
            ))
        })
        .collect();

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().map(|(_, item)| item).collect()
}

fn build_file_candidates(files: &[String], query: &str) -> Vec<CompletionItem> {
    let mut scored: Vec<(MatchRank, CompletionItem)> = files
        .iter()
        .filter_map(|path| {
            let rank = score_candidate(path, query)?;
            Some((
                rank,
                CompletionItem {
                    label: path.clone(),
                    description: "workspace file".to_string(),
                    insert_text: format!("@{path} "),
                    item_kind: CompletionItemKind::File,
                    search: path.clone(),
                },
            ))
        })
        .collect();

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().map(|(_, item)| item).collect()
}

pub(crate) use meerai_host::search::{MatchRank, score_candidate};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "meerai_tui_test_{name}_{}_{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.root.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn slash_command_completion_and_acceptance() {
        let mut engine = CompletionEngine::new(PathBuf::from("."));

        engine.update("/", 1);
        assert!(engine.is_active());
        assert_eq!(engine.kind(), Some(CompletionKind::SlashCommand));
        assert_eq!(engine.items().len(), SLASH_COMMANDS.len());

        engine.update("/he", 3);
        assert_eq!(engine.items().len(), 1);
        assert_eq!(engine.items()[0].label, "/help");

        let (accepted, cursor) = engine.accept("/he", 3).unwrap();
        assert_eq!(accepted, "/help");
        assert_eq!(cursor, 5);
        assert!(!engine.is_active());

        engine.update("/model", 6);
        assert_eq!(engine.items()[0].label, "/model");
        let (accepted_model, cursor_model) = engine.accept("/model", 6).unwrap();
        assert_eq!(accepted_model, "/model ");
        assert_eq!(cursor_model, 7);

        engine.update(&accepted_model, cursor_model);
        assert!(!engine.is_active());

        engine.sync_models(&["gemini-3.8-flash".to_string()], "gemini");
        engine.update(&accepted_model, cursor_model);
        assert!(engine.is_active());
        assert_eq!(engine.kind(), Some(CompletionKind::Model));
    }

    fn agent_definition(
        id: &str,
        source: DefinitionSource,
        description: Option<&str>,
    ) -> AgentDefinition {
        AgentDefinition {
            id: id.to_string(),
            source,
            description: description.map(str::to_string),
            model: None,
            steps: None,
            mode: None,
            prompt: String::new(),
            path: PathBuf::from(format!(".agents/agents/{id}.md")),
        }
    }

    fn skill_definition(
        id: &str,
        source: DefinitionSource,
        description: Option<&str>,
    ) -> SkillDefinition {
        SkillDefinition {
            id: id.to_string(),
            source,
            description: description.map(str::to_string),
            body: String::new(),
            path: PathBuf::from(format!(".agents/skills/{id}/SKILL.md")),
        }
    }

    #[test]
    fn acp_command_is_listed_and_completes() {
        assert!(SLASH_COMMANDS.iter().any(|(name, hint, takes_arg)| {
            *name == "/acp" && *hint == "Start an ACP session (agy)" && !*takes_arg
        }));

        let mut engine = CompletionEngine::new(PathBuf::from("."));
        engine.update("/ac", 3);
        assert!(engine.is_active());
        assert_eq!(engine.items()[0].label, "/acp");
        let (accepted, cursor) = engine.accept("/ac", 3).unwrap();
        assert_eq!(accepted, "/acp");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn agent_and_skill_shortcuts_are_listed_and_take_arguments() {
        let mut engine = CompletionEngine::new(PathBuf::from("."));
        engine.update("/", 1);
        let labels: Vec<&str> = engine.items().iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"/agent"));
        assert!(labels.contains(&"/skill"));
        assert!(labels.contains(&"/agents"));
        assert!(labels.contains(&"/skills"));

        engine.update("/agent", 6);
        assert!(engine.is_active());
        let (accepted, cursor) = engine.accept("/agent", 6).unwrap();
        assert_eq!(accepted, "/agent ");
        assert_eq!(cursor, 7);

        engine.update("/skill", 6);
        assert!(engine.is_active());
        let (accepted_skill, cursor_skill) = engine.accept("/skill", 6).unwrap();
        assert_eq!(accepted_skill, "/skill ");
        assert_eq!(cursor_skill, 7);
    }

    #[test]
    fn mention_completion_lists_definitions_before_files_with_badges() {
        let ws = TempWorkspace::new("mentions");
        ws.write("agent/report.md", "notes");
        ws.write("src/app.rs", "fn main() {}");

        let mut engine = CompletionEngine::new(ws.root.clone());
        engine.set_definitions(
            &[agent_definition(
                "reviewer",
                DefinitionSource::Agents,
                Some("Reviews changes"),
            )],
            &[skill_definition(
                "orchestrate",
                DefinitionSource::Claude,
                Some("Plan work"),
            )],
        );

        engine.update("@", 1);
        assert!(engine.is_active());
        let items = engine.items();
        assert_eq!(items[0].label, "reviewer");
        assert_eq!(items[0].item_kind, CompletionItemKind::Agent);
        assert_eq!(items[0].item_kind.badge(), Some("agent"));
        assert!(items[0].description.contains(".agents"));
        assert!(items[0].description.contains("Reviews changes"));
        assert_eq!(items[0].insert_text, "@agent/reviewer ");
        assert!(!items[0].label.contains('@'));

        assert_eq!(items[1].label, "orchestrate");
        assert_eq!(items[1].item_kind, CompletionItemKind::Skill);
        assert_eq!(items[1].item_kind.badge(), Some("skill"));
        assert!(items[1].description.contains(".claude"));
        assert_eq!(items[1].insert_text, "@skill/orchestrate ");

        let file = items
            .iter()
            .find(|item| item.label == "agent/report.md")
            .expect("file candidate must be present");
        assert_eq!(file.item_kind, CompletionItemKind::File);
        assert_eq!(file.item_kind.badge(), None);
        assert_eq!(file.insert_text, "@agent/report.md ");
        assert_eq!(
            items
                .iter()
                .position(|item| item.item_kind == CompletionItemKind::Agent),
            Some(0)
        );
        assert!(
            items
                .iter()
                .position(|item| item.label == "agent/report.md")
                .unwrap()
                > 1
        );
    }

    #[test]
    fn mention_completion_filters_by_mention_prefix_and_bare_id() {
        let ws = TempWorkspace::new("mention_filter");
        ws.write("agent/report.md", "notes");

        let mut engine = CompletionEngine::new(ws.root.clone());
        engine.set_definitions(
            &[
                agent_definition("reviewer", DefinitionSource::Agents, None),
                agent_definition("writer", DefinitionSource::Claude, None),
            ],
            &[
                skill_definition("orchestrate", DefinitionSource::Agents, None),
                skill_definition("reviewer", DefinitionSource::Claude, None),
            ],
        );

        engine.update("@agent/re", 9);
        assert_eq!(engine.items()[0].label, "reviewer");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Agent);
        assert!(
            !engine
                .items()
                .iter()
                .any(|item| item.item_kind == CompletionItemKind::Skill)
        );

        engine.update("@skill/orch", 11);
        assert_eq!(engine.items()[0].label, "orchestrate");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Skill);

        engine.update("@skill/re", 9);
        assert_eq!(engine.items()[0].label, "reviewer");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Skill);
        assert!(
            engine
                .items()
                .iter()
                .all(|item| item.item_kind == CompletionItemKind::Skill)
        );

        engine.update("@reviewer", 9);
        assert_eq!(engine.items()[0].label, "reviewer");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Agent);

        engine.update("@write", 6);
        assert_eq!(engine.items()[0].label, "writer");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Agent);
    }

    #[test]
    fn mention_completion_keeps_agent_and_skill_with_the_same_id() {
        let ws = TempWorkspace::new("mention_shared_id");
        let mut engine = CompletionEngine::new(ws.root.clone());
        engine.set_definitions(
            &[agent_definition(
                "reviewer",
                DefinitionSource::Agents,
                Some("Reviews changes"),
            )],
            &[skill_definition(
                "reviewer",
                DefinitionSource::Claude,
                Some("Review skill"),
            )],
        );

        engine.update("@reviewer", 9);
        assert!(engine.is_active());
        assert_eq!(engine.items().len(), 2);
        assert_eq!(engine.items()[0].label, "reviewer");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Agent);
        assert_eq!(engine.items()[0].description, ".agents — Reviews changes");
        assert_eq!(engine.items()[1].label, "reviewer");
        assert_eq!(engine.items()[1].item_kind, CompletionItemKind::Skill);
        assert_eq!(engine.items()[1].description, ".claude — Review skill");

        engine.select_next();
        assert_eq!(engine.selected(), 1);
        let (accepted, cursor) = engine.accept("@reviewer", 9).unwrap();
        assert_eq!(accepted, "@skill/reviewer ");
        assert_eq!(cursor, "@skill/reviewer ".len());

        engine.update("@reviewer", 9);
        assert!(engine.is_active());
        let (accepted_agent, _) = engine.accept("@reviewer", 9).unwrap();
        assert_eq!(accepted_agent, "@agent/reviewer ");
    }

    #[test]
    fn mention_completion_keeps_exact_id_popup_open() {
        let ws = TempWorkspace::new("mention_exact");
        let mut engine = CompletionEngine::new(ws.root.clone());
        engine.set_definitions(
            &[agent_definition("reviewer", DefinitionSource::Agents, None)],
            &[],
        );

        engine.update("@reviewer", 9);
        assert!(engine.is_active());
        assert_eq!(engine.items().len(), 1);
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::Agent);
        let (accepted, cursor) = engine.accept("@reviewer", 9).unwrap();
        assert_eq!(accepted, "@agent/reviewer ");
        assert_eq!(cursor, "@agent/reviewer ".len());
    }

    #[test]
    fn mention_completion_inserts_and_falls_back_to_files_without_definitions() {
        let ws = TempWorkspace::new("mention_insert");
        ws.write("src/app.rs", "fn main() {}");

        let mut engine = CompletionEngine::new(ws.root.clone());
        let input = "@src/ap";
        engine.update(input, input.len());
        assert!(engine.is_active());
        assert_eq!(engine.items().len(), 1);
        assert_eq!(engine.items()[0].label, "src/app.rs");
        assert_eq!(engine.items()[0].item_kind, CompletionItemKind::File);
        assert_eq!(engine.items()[0].item_kind.badge(), None);
        let (accepted, cursor) = engine.accept(input, input.len()).unwrap();
        assert_eq!(accepted, "@src/app.rs ");
        assert_eq!(cursor, "@src/app.rs ".len());

        engine.set_definitions(
            &[agent_definition(
                "reviewer",
                DefinitionSource::Agents,
                Some("Reviews changes"),
            )],
            &[],
        );
        let input = "@agent/re";
        engine.update(input, input.len());
        assert_eq!(engine.items().len(), 1);
        let (accepted, _) = engine.accept(input, input.len()).unwrap();
        assert_eq!(accepted, "@agent/reviewer ");
    }

    #[test]
    fn model_completion_tracks_active_provider_recent_models() {
        let mut engine = CompletionEngine::new(PathBuf::from("."));
        engine.sync_models(
            &[
                "gemini-3.8-flash".to_string(),
                "gemini-3.1-pro-preview".to_string(),
            ],
            "gemini",
        );
        engine.update("/model gem", 10);
        assert!(engine.is_active());
        assert_eq!(engine.kind(), Some(CompletionKind::Model));

        let labels: Vec<&str> = engine.items().iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"gemini-3.8-flash"));
        assert!(labels.contains(&"gemini-3.1-pro-preview"));

        let first_label = labels[0].to_string();
        let (accepted, cursor) = engine.accept("/model gem", 10).unwrap();
        assert_eq!(accepted, format!("/model {first_label}"));
        assert_eq!(cursor, accepted.len());

        engine.sync_models(
            &["deepseek-v4.1-flash".to_string(), "glm-5.3".to_string()],
            "opencode-go",
        );
        engine.update("/model ", 7);
        let go_labels: Vec<&str> = engine.items().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(go_labels, vec!["deepseek-v4.1-flash", "glm-5.3"]);
    }

    #[test]
    fn file_completion_skips_ignored_dirs_and_replaces_token() {
        let ws = TempWorkspace::new("files");
        ws.write(".git/HEAD", "ref: refs/heads/main");
        ws.write("target/debug/meerai", "bin");
        ws.write("node_modules/pkg/index.js", "module.exports = {};");
        ws.write("src/main.rs", "fn main() {}");
        ws.write("src/app.rs", "pub struct AppState;");
        ws.write("README.md", "# meerai");

        let mut engine = CompletionEngine::new(ws.root.clone());
        let input = "review @ap please";
        engine.update(input, 10);
        assert!(engine.is_active());
        assert_eq!(engine.kind(), Some(CompletionKind::File));

        let labels: Vec<&str> = engine.items().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["src/app.rs"]);
        assert!(
            !labels
                .iter()
                .any(|p| p.contains(".git") || p.contains("target") || p.contains("node_modules"))
        );

        let (accepted, cursor) = engine.accept(input, 10).unwrap();
        assert_eq!(accepted, "review @src/app.rs please");
        assert_eq!(cursor, "review @src/app.rs ".len());
    }

    #[test]
    fn dismiss_keeps_popup_closed_until_query_changes() {
        let mut engine = CompletionEngine::new(PathBuf::from("."));
        engine.update("/he", 3);
        assert!(engine.is_active());

        engine.dismiss();
        assert!(!engine.is_active());

        engine.update("/he", 3);
        assert!(!engine.is_active());

        engine.update("/h", 2);
        assert!(engine.is_active());
    }
}
