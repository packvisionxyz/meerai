use std::path::{Path, PathBuf};

use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolOutput, ToolPermissionSpec,
    Toolset, async_trait,
};
use meerai_session::SessionId;
use meerai_store::{MemoryRepo, NewProjectMemory, ProjectMemory, ProjectMemoryKind, Store};
use serde::Deserialize;

pub const DEFAULT_MEMORY_PROMPT_BYTES: usize = meerai_store::DEFAULT_MEMORY_PROMPT_BYTES;
pub const MEMORY_TOOLSET: &str = "memory";
pub const MEMORY_SEARCH: &str = "memory-search";
pub const MEMORY_SAVE: &str = "memory-save";
pub const MEMORY_DELETE: &str = "memory-delete";

const DEFAULT_SEARCH_LIMIT: usize = 10;
const MAX_BODY_BYTES: usize = 4 * 1024;
const MAX_TITLE_CHARS: usize = 120;

#[derive(Clone, Debug)]
pub struct MemoryToolset {
    repo: MemoryRepo,
    project_root: String,
    source_session_id: Option<String>,
}

impl MemoryToolset {
    pub fn new(store: Store, project_root: impl AsRef<Path>) -> Self {
        Self {
            repo: store.memories(),
            project_root: project_root.as_ref().display().to_string(),
            source_session_id: None,
        }
    }

    pub fn with_source_session(mut self, session_id: Option<SessionId>) -> Self {
        self.source_session_id = session_id.map(|id| id.to_string());
        self
    }

    fn search(&self, args: MemorySearchArgs) -> Result<ToolOutput, ToolError> {
        let query = args.query.trim();
        if query.is_empty() {
            return Ok(ToolOutput::Fail(
                "memory search query must not be empty".to_string(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, 50);
        let mut memories = self
            .repo
            .search(&self.project_root, query)
            .map_err(ToolError::Unknown)?;
        memories.truncate(limit);
        Ok(ToolOutput::Text(render_memories(&memories)))
    }

    fn save(&self, args: MemorySaveArgs) -> Result<ToolOutput, ToolError> {
        let kind = ProjectMemoryKind::parse(&args.kind).map_err(ToolError::Unknown)?;
        let body = truncate_bytes(args.body.trim(), MAX_BODY_BYTES);
        if body.is_empty() {
            return Ok(ToolOutput::Fail(
                "memory body must not be empty".to_string(),
            ));
        }
        let title = normalize_title(&args.title);
        if title.is_empty() {
            return Ok(ToolOutput::Fail(
                "memory title must not be empty".to_string(),
            ));
        }
        if let Some(id) = args
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let updated = self
                .repo
                .update(
                    &self.project_root,
                    id,
                    Some(kind),
                    Some(&title),
                    Some(&body),
                )
                .map_err(ToolError::Unknown)?;
            let Some(memory) = updated else {
                return Ok(ToolOutput::Fail(format!("memory not found: {id}")));
            };
            if let Some(pinned) = args.pinned {
                let _ = self.repo.set_pinned(&self.project_root, &memory.id, pinned);
            }
            return Ok(ToolOutput::Text(format!(
                "Updated memory {} ({})",
                memory.id, memory.title
            )));
        }
        let memory = self
            .repo
            .insert(&NewProjectMemory {
                project_root: self.project_root.clone(),
                kind,
                title,
                body,
                source_session_id: self.source_session_id.clone(),
                pinned: args.pinned.unwrap_or(false),
            })
            .map_err(ToolError::Unknown)?;
        Ok(ToolOutput::Text(format!(
            "Saved memory {} ({})",
            memory.id, memory.title
        )))
    }

    fn delete(&self, args: MemoryDeleteArgs) -> Result<ToolOutput, ToolError> {
        let id = args.id.trim();
        if id.is_empty() {
            return Ok(ToolOutput::Fail("memory id must not be empty".to_string()));
        }
        let removed = self
            .repo
            .delete(&self.project_root, id)
            .map_err(ToolError::Unknown)?;
        Ok(ToolOutput::Text(if removed {
            format!("Removed memory {id}")
        } else {
            format!("Memory not found: {id}")
        }))
    }
}

#[async_trait]
impl Toolset for MemoryToolset {
    fn name(&self) -> String {
        MEMORY_TOOLSET.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                r#type: "function".to_string(),
                name: MEMORY_SEARCH.to_string(),
                description: "Search this project's cross-session memories with full-text search. Use it to recover decisions, conventions, and progress from earlier sessions.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Full-text query"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum results (default 10)"}
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: MEMORY_SAVE.to_string(),
                description: "Save or update a durable project memory: an architectural decision, a convention, a fact, or progress.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string", "enum": ["decision", "progress", "fact", "convention"]},
                        "title": {"type": "string"},
                        "body": {"type": "string"},
                        "id": {"type": "string", "description": "Optional existing memory id to update"},
                        "pinned": {"type": "boolean"}
                    },
                    "required": ["kind", "title", "body"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: MEMORY_DELETE.to_string(),
                description: "Delete an outdated project memory by id.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }),
            },
        ]
    }

    fn contain(&self, fn_name: &str) -> bool {
        matches!(fn_name, MEMORY_SEARCH | MEMORY_SAVE | MEMORY_DELETE)
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        if fn_name == MEMORY_SEARCH {
            EffectClass::Read
        } else {
            EffectClass::Write
        }
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        match fn_name {
            MEMORY_SEARCH => Some(ToolPermissionSpec {
                action: PermissionAction::Read,
                resource_arg: None,
            }),
            MEMORY_SAVE | MEMORY_DELETE => Some(ToolPermissionSpec {
                action: PermissionAction::Edit,
                resource_arg: None,
            }),
            _ => None,
        }
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        match fn_name {
            MEMORY_SEARCH => {
                let args: MemorySearchArgs = serde_json::from_str(args)?;
                self.search(args)
            }
            MEMORY_SAVE => {
                let args: MemorySaveArgs = serde_json::from_str(args)?;
                self.save(args)
            }
            MEMORY_DELETE => {
                let args: MemoryDeleteArgs = serde_json::from_str(args)?;
                self.delete(args)
            }
            _ => Err(ToolError::InvalidFunctionName(fn_name.to_string())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MemorySearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemorySaveArgs {
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MemoryDeleteArgs {
    id: String,
}

#[derive(Clone, Debug)]
pub struct MemoryCapture {
    repo: MemoryRepo,
    project_root: String,
    source_session_id: Option<String>,
}

impl MemoryCapture {
    pub fn new(
        store: Store,
        project_root: impl AsRef<Path>,
        session_id: Option<SessionId>,
    ) -> Self {
        Self {
            repo: store.memories(),
            project_root: project_root.as_ref().display().to_string(),
            source_session_id: session_id.map(|id| id.to_string()),
        }
    }

    pub fn capture_summary(&self, summary: &str) {
        let sections = summary_sections(summary);
        let goal = sections.get("goal").cloned().unwrap_or_default();
        let progress = sections
            .get("completed progress")
            .cloned()
            .unwrap_or_default();
        let decisions = sections.get("key decisions").cloned().unwrap_or_default();
        let blockers = sections.get("open blockers").cloned().unwrap_or_default();
        let files = sections.get("modified files").cloned().unwrap_or_default();

        let mut body = String::new();
        push_section(&mut body, "Goal", &goal);
        push_section(&mut body, "Completed progress", &progress);
        push_section(&mut body, "Modified files", &files);
        push_section(&mut body, "Open blockers", &blockers);
        if !body.trim().is_empty() {
            let title = if goal.trim().is_empty() {
                "Compacted progress".to_string()
            } else {
                normalize_title(&goal)
            };
            let _ = self.repo.insert(&NewProjectMemory {
                project_root: self.project_root.clone(),
                kind: ProjectMemoryKind::Progress,
                title,
                body: truncate_bytes(body.trim(), MAX_BODY_BYTES),
                source_session_id: self.source_session_id.clone(),
                pinned: false,
            });
        }

        for bullet in bullets(&decisions).into_iter().take(8) {
            if bullet.is_empty() {
                continue;
            }
            let _ = self.repo.insert(&NewProjectMemory {
                project_root: self.project_root.clone(),
                kind: ProjectMemoryKind::Decision,
                title: normalize_title(&bullet),
                body: truncate_bytes(&bullet, MAX_BODY_BYTES),
                source_session_id: self.source_session_id.clone(),
                pinned: false,
            });
        }
    }
}

pub fn load_memory_prompt(store: &Store, project_root: &Path, max_bytes: usize) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    store
        .memories()
        .prompt_block(&project_root.display().to_string(), max_bytes)
        .ok()
        .flatten()
}

fn render_memories(memories: &[ProjectMemory]) -> String {
    if memories.is_empty() {
        return "No matching project memories.".to_string();
    }
    let mut out = format!("Memories ({}):", memories.len());
    for memory in memories {
        out.push_str(&format!(
            "\n- {} [{}] {}{}\n  {}",
            memory.id,
            kind_label(memory.kind),
            memory.title,
            if memory.pinned { " (pinned)" } else { "" },
            memory.body.replace('\n', " ")
        ));
    }
    out
}

fn kind_label(kind: ProjectMemoryKind) -> &'static str {
    match kind {
        ProjectMemoryKind::Decision => "decision",
        ProjectMemoryKind::Progress => "progress",
        ProjectMemoryKind::Fact => "fact",
        ProjectMemoryKind::Convention => "convention",
    }
}

fn normalize_title(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

fn truncate_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let end = meerai_core::text::floor_char_boundary(text, limit);
    text[..end].to_string()
}

fn summary_sections(summary: &str) -> std::collections::HashMap<String, String> {
    const HEADINGS: [&str; 5] = [
        "goal",
        "completed progress",
        "modified files",
        "key decisions",
        "open blockers",
    ];
    let mut sections = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut body = String::new();
    for line in summary.lines() {
        let trimmed = line.trim();
        let heading = if let Some(rest) = trimmed.strip_prefix("## ") {
            let lower = rest.trim().to_ascii_lowercase();
            HEADINGS
                .contains(&lower.as_str())
                .then_some((lower, String::new()))
        } else if HEADINGS.contains(&trimmed.to_ascii_lowercase().as_str()) {
            Some((trimmed.to_ascii_lowercase(), String::new()))
        } else {
            trimmed.split_once(':').and_then(|(name, rest)| {
                let lower = name.trim().to_ascii_lowercase();
                HEADINGS
                    .contains(&lower.as_str())
                    .then_some((lower, rest.trim().to_string()))
            })
        };
        if let Some((name, rest)) = heading {
            if let Some(key) = current.take() {
                sections.insert(key, body.trim().to_string());
            }
            body.clear();
            if !rest.is_empty() {
                body.push_str(&rest);
                body.push('\n');
            }
            current = Some(name);
            continue;
        }
        if current.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(key) = current {
        sections.insert(key, body.trim().to_string());
    }
    sections
}

fn bullets(section: &str) -> Vec<String> {
    section
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_start_matches(['-', '*', '•']).trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn push_section(body: &mut String, label: &str, content: &str) {
    if content.trim().is_empty() {
        return;
    }
    if !body.is_empty() {
        body.push('\n');
    }
    body.push_str(label);
    body.push_str(": ");
    body.push_str(content.trim());
}

pub fn memory_project_root(workspace_root: &Path) -> PathBuf {
    meerai_common::settings::find_project_root(workspace_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_core::Toolset;

    fn toolset() -> (tempfile::TempDir, MemoryToolset) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("memory.db")).unwrap();
        let toolset = MemoryToolset::new(store, dir.path());
        (dir, toolset)
    }

    #[tokio::test]
    async fn save_search_and_delete_round_trip() {
        let (_dir, toolset) = toolset();
        let saved = toolset
            .invoke(
                MEMORY_SAVE,
                r#"{"kind":"decision","title":"Use SQLite","body":"All state lives in meerai.db"}"#,
            )
            .await
            .unwrap();
        assert!(matches!(saved, ToolOutput::Text(text) if text.contains("Saved memory")));

        let found = toolset
            .invoke(MEMORY_SEARCH, r#"{"query":"SQLite"}"#)
            .await
            .unwrap();
        let ToolOutput::Text(text) = found else {
            panic!("expected text");
        };
        assert!(text.contains("Use SQLite"));

        let id = text
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("- ")
                    .and_then(|rest| rest.split_whitespace().next())
            })
            .unwrap()
            .to_string();
        let deleted = toolset
            .invoke(MEMORY_DELETE, &format!(r#"{{"id":"{id}"}}"#))
            .await
            .unwrap();
        assert!(matches!(deleted, ToolOutput::Text(text) if text.contains("Removed memory")));
    }

    #[test]
    fn summary_capture_extracts_progress_and_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("memory.db")).unwrap();
        let capture = MemoryCapture::new(store.clone(), dir.path(), None);
        capture.capture_summary(
            "## Goal\nShip the store\n\n## Completed progress\n- schema\n- vault\n\n\
             ## Modified files\n- meerai-store/src/lib.rs\n\n## Key decisions\n\
             - Use rusqlite bundled\n- Keep overlays file-based\n\n## Open blockers\n- none\n",
        );
        let memories = store
            .memories()
            .list(&dir.path().display().to_string())
            .unwrap();
        assert_eq!(memories.len(), 3, "one progress plus two decisions");
        assert!(
            memories
                .iter()
                .any(|memory| memory.kind == ProjectMemoryKind::Progress)
        );
        assert_eq!(
            memories
                .iter()
                .filter(|memory| memory.kind == ProjectMemoryKind::Decision)
                .count(),
            2
        );
    }

    #[test]
    fn effects_split_read_and_write() {
        let (_dir, toolset) = toolset();
        assert_eq!(toolset.effect_of(MEMORY_SEARCH), EffectClass::Read);
        assert_eq!(toolset.effect_of(MEMORY_SAVE), EffectClass::Write);
        assert_eq!(toolset.effect_of(MEMORY_DELETE), EffectClass::Write);
    }
}
