use anyhow::Result;
use meerai_core::env::now_millis;
use rusqlite::params;

use crate::Store;

#[derive(Clone, Debug, PartialEq)]
pub struct UsageEntry {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub turn_id: Option<String>,
    pub agent_id: Option<String>,
    pub task_type: Option<String>,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub est_cost_usd: Option<f64>,
    pub ts_ms: u64,
}

#[derive(Clone, Debug)]
pub struct UsageRepo {
    store: Store,
}

impl Store {
    pub fn usage(&self) -> UsageRepo {
        UsageRepo {
            store: self.clone(),
        }
    }
}

impl UsageRepo {
    pub fn record(&self, entry: &UsageEntry) -> Result<()> {
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO usage_ledger
                    (session_id, parent_session_id, turn_id, agent_id, task_type, provider, model,
                     prompt_tokens, completion_tokens, cached_tokens, est_cost_usd, ts_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    entry.session_id,
                    entry.parent_session_id,
                    entry.turn_id,
                    entry.agent_id,
                    entry.task_type,
                    entry.provider,
                    entry.model,
                    i64::try_from(entry.prompt_tokens)?,
                    i64::try_from(entry.completion_tokens)?,
                    i64::try_from(entry.cached_tokens)?,
                    entry.est_cost_usd,
                    i64::try_from(entry.ts_ms)?,
                ],
            )?;
            Ok(())
        })
    }

    pub fn list(&self, session_id: &str) -> Result<Vec<UsageEntry>> {
        self.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT session_id, parent_session_id, turn_id, agent_id, task_type, provider,
                        model, prompt_tokens, completion_tokens, cached_tokens, est_cost_usd, ts_ms
                 FROM usage_ledger WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([session_id], |row| {
                Ok(UsageEntry {
                    session_id: row.get(0)?,
                    parent_session_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    agent_id: row.get(3)?,
                    task_type: row.get(4)?,
                    provider: row.get(5)?,
                    model: row.get(6)?,
                    prompt_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                    completion_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                    cached_tokens: row.get::<_, i64>(9)?.max(0) as u64,
                    est_cost_usd: row.get(10)?,
                    ts_ms: row.get::<_, i64>(11)?.max(0) as u64,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn total_cost(&self, session_id: &str) -> Result<Option<f64>> {
        self.store.read(|conn| {
            conn.query_row(
                "SELECT SUM(est_cost_usd) FROM usage_ledger WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
    }

    pub fn now_ms() -> u64 {
        now_millis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> UsageRepo {
        let store = Store::open(":memory:").unwrap();
        store
            .immediate(|tx| {
                tx.execute(
                    "INSERT INTO sessions (id, project_root, cwd, created_at_ms, updated_at_ms)
                     VALUES ('s1', '/w', '/w', 0, 0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        store.usage()
    }

    #[test]
    fn records_and_lists_rows_in_order() {
        let repo = repo();
        for (prompt, cost) in [(10, Some(0.1)), (20, Some(0.2))] {
            repo.record(&UsageEntry {
                session_id: "s1".to_string(),
                parent_session_id: None,
                turn_id: Some("t1".to_string()),
                agent_id: None,
                task_type: Some("build".to_string()),
                provider: "openai".to_string(),
                model: "gpt".to_string(),
                prompt_tokens: prompt,
                completion_tokens: 5,
                cached_tokens: 1,
                est_cost_usd: cost,
                ts_ms: 1,
            })
            .unwrap();
        }
        let rows = repo.list("s1").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].prompt_tokens, 20);
        assert!((repo.total_cost("s1").unwrap().unwrap() - 0.3).abs() < 1e-9);
    }
}
