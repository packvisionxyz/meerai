use anyhow::{Result, ensure};
use meerai_core::env::now_millis;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;

pub const DEFAULT_MEMORY_PROMPT_BYTES: usize = 8 * 1024;
const MAX_TITLE_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMemoryKind {
    Decision,
    Progress,
    Fact,
    Convention,
}

impl ProjectMemoryKind {
    pub const ALL: [Self; 4] = [Self::Decision, Self::Progress, Self::Fact, Self::Convention];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Progress => "progress",
            Self::Fact => "fact",
            Self::Convention => "convention",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "decision" => Ok(Self::Decision),
            "progress" => Ok(Self::Progress),
            "fact" => Ok(Self::Fact),
            "convention" => Ok(Self::Convention),
            other => anyhow::bail!("unknown memory kind: {other}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub id: String,
    pub project_root: String,
    pub kind: ProjectMemoryKind,
    pub title: String,
    pub body: String,
    pub source_session_id: Option<String>,
    pub pinned: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProjectMemory {
    pub project_root: String,
    pub kind: ProjectMemoryKind,
    pub title: String,
    pub body: String,
    pub source_session_id: Option<String>,
    pub pinned: bool,
}

impl Store {
    pub fn memories(&self) -> MemoryRepo {
        MemoryRepo {
            store: self.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MemoryRepo {
    store: Store,
}

impl MemoryRepo {
    pub fn insert(&self, draft: &NewProjectMemory) -> Result<ProjectMemory> {
        let project = normalize_project(&draft.project_root)?;
        let title = normalized_title(&draft.title)?;
        ensure!(!draft.body.trim().is_empty(), "memory body is empty");
        let now = now_millis();
        let memory = ProjectMemory {
            id: new_id()?,
            project_root: project,
            kind: draft.kind,
            title,
            body: draft.body.trim().to_string(),
            source_session_id: draft.source_session_id.clone(),
            pinned: draft.pinned,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let stored = memory.clone();
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO project_memories
                 (id, project_root, kind, title, body, source_session_id, pinned,
                  created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    stored.id,
                    stored.project_root,
                    stored.kind.as_str(),
                    stored.title,
                    stored.body,
                    stored.source_session_id,
                    stored.pinned,
                    i64::try_from(stored.created_at_ms)?,
                    i64::try_from(stored.updated_at_ms)?,
                ],
            )?;
            Ok(())
        })?;
        Ok(memory)
    }

    pub fn get(&self, project_root: &str, id: &str) -> Result<Option<ProjectMemory>> {
        let project = normalize_project(project_root)?;
        self.store.read(|conn| {
            Ok(conn
                .query_row(
                    &format!(
                        "SELECT {} FROM project_memories WHERE id = ?1 AND project_root = ?2",
                        COLUMNS
                    ),
                    params![id, project],
                    memory_row,
                )
                .optional()?)
        })
    }

    pub fn list(&self, project_root: &str) -> Result<Vec<ProjectMemory>> {
        let project = normalize_project(project_root)?;
        self.store.read(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM project_memories WHERE project_root = ?1
                 ORDER BY pinned DESC, updated_at_ms DESC, id"
            ))?;
            let rows = stmt.query_map([project], memory_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn search(&self, project_root: &str, query: &str) -> Result<Vec<ProjectMemory>> {
        let project = normalize_project(project_root)?;
        let Some(match_query) = fts_match_query(query) else {
            return Ok(Vec::new());
        };
        self.store.read(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM project_memories
                 WHERE project_root = ?1 AND id IN (
                    SELECT id FROM project_memories_fts
                    WHERE project_memories_fts MATCH ?2 AND project_root = ?1
                 )
                 ORDER BY pinned DESC, updated_at_ms DESC, id",
                prefixed_columns()
            ))?;
            let rows = stmt.query_map(params![project, match_query], memory_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(anyhow::Error::from)
        })
    }

    pub fn update(
        &self,
        project_root: &str,
        id: &str,
        kind: Option<ProjectMemoryKind>,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<Option<ProjectMemory>> {
        let Some(current) = self.get(project_root, id)? else {
            return Ok(None);
        };
        let title = match title {
            Some(title) => normalized_title(title)?,
            None => current.title.clone(),
        };
        let body = match body {
            Some(body) => {
                ensure!(!body.trim().is_empty(), "memory body is empty");
                body.trim().to_string()
            }
            None => current.body.clone(),
        };
        let kind = kind.unwrap_or(current.kind);
        let updated_at_ms = now_millis().max(current.updated_at_ms);
        self.store.immediate(|tx| {
            tx.execute(
                "UPDATE project_memories
                 SET kind = ?3, title = ?4, body = ?5, updated_at_ms = ?6
                 WHERE id = ?1 AND project_root = ?2",
                params![
                    id,
                    current.project_root,
                    kind.as_str(),
                    title,
                    body,
                    i64::try_from(updated_at_ms)?,
                ],
            )?;
            Ok(())
        })?;
        self.get(project_root, id)
    }

    pub fn set_pinned(
        &self,
        project_root: &str,
        id: &str,
        pinned: bool,
    ) -> Result<Option<ProjectMemory>> {
        let Some(current) = self.get(project_root, id)? else {
            return Ok(None);
        };
        self.store.immediate(|tx| {
            tx.execute(
                "UPDATE project_memories SET pinned = ?3 WHERE id = ?1 AND project_root = ?2",
                params![id, current.project_root, pinned],
            )?;
            Ok(())
        })?;
        self.get(project_root, id)
    }

    pub fn delete(&self, project_root: &str, id: &str) -> Result<bool> {
        let project = normalize_project(project_root)?;
        self.store.immediate(|tx| {
            let removed = tx.execute(
                "DELETE FROM project_memories WHERE id = ?1 AND project_root = ?2",
                params![id, project],
            )?;
            Ok(removed == 1)
        })
    }

    pub fn prompt_block(&self, project_root: &str, max_bytes: usize) -> Result<Option<String>> {
        if max_bytes == 0 {
            return Ok(None);
        }
        let memories = self.list(project_root)?;
        let mut block = String::new();
        for memory in memories {
            let entry = format!(
                "- [{}] {}: {}\n",
                memory.kind.as_str(),
                memory.title,
                memory.body
            );
            if block.len() + entry.len() > max_bytes {
                break;
            }
            block.push_str(&entry);
        }
        if block.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(block))
    }
}

const COLUMNS: &str = "id, project_root, kind, title, body, source_session_id, pinned, \
                       created_at_ms, updated_at_ms";

fn prefixed_columns() -> String {
    COLUMNS
        .split(", ")
        .map(|column| format!("project_memories.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn memory_row(row: &Row<'_>) -> rusqlite::Result<ProjectMemory> {
    let raw_kind: String = row.get(2)?;
    let kind = ProjectMemoryKind::parse(&raw_kind).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown memory kind: {raw_kind}"),
            )),
        )
    })?;
    Ok(ProjectMemory {
        id: row.get(0)?,
        project_root: row.get(1)?,
        kind,
        title: row.get(3)?,
        body: row.get(4)?,
        source_session_id: row.get(5)?,
        pinned: row.get(6)?,
        created_at_ms: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
        updated_at_ms: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
    })
}

pub fn normalize_project(project_root: &str) -> Result<String> {
    let trimmed = project_root.trim();
    ensure!(!trimmed.is_empty(), "memory project root is empty");
    Ok(trimmed.trim_end_matches('/').to_string())
}

fn normalized_title(title: &str) -> Result<String> {
    let trimmed = title.trim();
    ensure!(!trimmed.is_empty(), "memory title is empty");
    let title: String = trimmed.chars().take(MAX_TITLE_CHARS).collect();
    Ok(title)
}

fn fts_match_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn new_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("could not generate memory id"))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("memory.db")).unwrap();
        (dir, store)
    }

    fn draft(project: &str, kind: ProjectMemoryKind, title: &str, body: &str) -> NewProjectMemory {
        NewProjectMemory {
            project_root: project.to_string(),
            kind,
            title: title.to_string(),
            body: body.to_string(),
            source_session_id: Some("session-1".to_string()),
            pinned: false,
        }
    }

    #[test]
    fn insert_get_and_list_round_trip() {
        let (_dir, store) = repo();
        let repo = store.memories();
        let memory = repo
            .insert(&draft(
                "/work/repo",
                ProjectMemoryKind::Decision,
                "Use WAL",
                "Enable WAL for concurrent reads.",
            ))
            .unwrap();
        assert_eq!(memory.project_root, "/work/repo");
        assert_eq!(
            repo.get("/work/repo", &memory.id).unwrap(),
            Some(memory.clone())
        );
        assert_eq!(repo.list("/work/repo").unwrap(), vec![memory]);
    }

    #[test]
    fn project_isolation_blocks_cross_project_access() {
        let (_dir, store) = repo();
        let repo = store.memories();
        let memory = repo
            .insert(&draft(
                "/work/a",
                ProjectMemoryKind::Fact,
                "A",
                "only for a",
            ))
            .unwrap();
        assert_eq!(repo.get("/work/b", &memory.id).unwrap(), None);
        assert!(!repo.delete("/work/b", &memory.id).unwrap());
        assert!(
            repo.update("/work/b", &memory.id, None, Some("hijack"), None)
                .unwrap()
                .is_none()
        );
        assert!(
            repo.set_pinned("/work/b", &memory.id, true)
                .unwrap()
                .is_none()
        );
        assert_eq!(repo.list("/work/b").unwrap(), Vec::new());
        assert_eq!(repo.get("/work/a", &memory.id).unwrap(), Some(memory));
    }

    #[test]
    fn trailing_slash_does_not_split_a_project() {
        let (_dir, store) = repo();
        let repo = store.memories();
        let memory = repo
            .insert(&draft("/work/repo/", ProjectMemoryKind::Fact, "t", "b"))
            .unwrap();
        assert!(repo.get("/work/repo", &memory.id).unwrap().is_some());
    }

    #[test]
    fn search_finds_text_and_ignores_fts_operators() {
        let (_dir, store) = repo();
        let repo = store.memories();
        repo.insert(&draft(
            "/work/repo",
            ProjectMemoryKind::Convention,
            "Rust formatting",
            "Run cargo fmt before committing.",
        ))
        .unwrap();
        let hits = repo.search("/work/repo", "cargo fmt").unwrap();
        assert_eq!(hits.len(), 1);
        assert!(repo.search("/work/repo", "NEAR(").unwrap().is_empty());
        assert!(repo.search("/work/repo", "python").unwrap().is_empty());
        assert!(repo.search("/work/other", "cargo").unwrap().is_empty());
    }

    #[test]
    fn update_pin_and_delete_apply_to_the_owning_project() {
        let (_dir, store) = repo();
        let repo = store.memories();
        let memory = repo
            .insert(&draft(
                "/work/repo",
                ProjectMemoryKind::Progress,
                "old",
                "body",
            ))
            .unwrap();
        let updated = repo
            .update(
                "/work/repo",
                &memory.id,
                Some(ProjectMemoryKind::Decision),
                Some("new title"),
                Some("new body"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.kind, ProjectMemoryKind::Decision);
        assert_eq!(updated.title, "new title");
        assert_eq!(updated.body, "new body");
        assert_eq!(updated.created_at_ms, memory.created_at_ms);

        let pinned = repo
            .set_pinned("/work/repo", &memory.id, true)
            .unwrap()
            .unwrap();
        assert!(pinned.pinned);
        assert!(repo.delete("/work/repo", &memory.id).unwrap());
        assert!(repo.list("/work/repo").unwrap().is_empty());
        assert!(repo.search("/work/repo", "body").unwrap().is_empty());
    }

    #[test]
    fn prompt_block_is_bounded_and_prefers_pinned() {
        let (_dir, store) = repo();
        let repo = store.memories();
        let mut pinned = draft("/work/repo", ProjectMemoryKind::Convention, "pin", "keep");
        pinned.pinned = true;
        repo.insert(&pinned).unwrap();
        for index in 0..20 {
            repo.insert(&draft(
                "/work/repo",
                ProjectMemoryKind::Fact,
                &format!("fact {index}"),
                &"x".repeat(64),
            ))
            .unwrap();
        }
        let block = repo
            .prompt_block("/work/repo", DEFAULT_MEMORY_PROMPT_BYTES)
            .unwrap()
            .unwrap();
        assert!(block.starts_with("- [convention] pin: keep"));
        assert!(block.len() <= DEFAULT_MEMORY_PROMPT_BYTES);
        assert!(repo.prompt_block("/work/repo", 0).unwrap().is_none());
        assert!(repo.prompt_block("/work/none", 4096).unwrap().is_none());
    }
}
