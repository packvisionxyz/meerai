use anyhow::{Context, Result, ensure};
use meerai_core::env::now_millis;
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Default,
    Explore,
    Plan,
    Build,
    Review,
    Compact,
    Recovery,
    Memory,
    Custom(String),
}

impl TaskType {
    pub const BUILTIN: [Self; 8] = [
        Self::Default,
        Self::Explore,
        Self::Plan,
        Self::Build,
        Self::Review,
        Self::Compact,
        Self::Recovery,
        Self::Memory,
    ];

    pub fn as_key(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Explore => "explore".to_string(),
            Self::Plan => "plan".to_string(),
            Self::Build => "build".to_string(),
            Self::Review => "review".to_string(),
            Self::Compact => "compact".to_string(),
            Self::Recovery => "recovery".to_string(),
            Self::Memory => "memory".to_string(),
            Self::Custom(name) => format!("custom:{name}"),
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        let trimmed = value.trim();
        let builtin = match trimmed.to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "explore" => Some(Self::Explore),
            "plan" => Some(Self::Plan),
            "build" => Some(Self::Build),
            "review" => Some(Self::Review),
            "compact" => Some(Self::Compact),
            "recovery" => Some(Self::Recovery),
            "memory" => Some(Self::Memory),
            _ => None,
        };
        if let Some(builtin) = builtin {
            return Ok(builtin);
        }
        let name = trimmed
            .strip_prefix("custom:")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .with_context(|| format!("unknown task type: {trimmed}"))?;
        Ok(Self::Custom(name.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRoute {
    pub task_type: TaskType,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
}

impl ModelRoute {
    pub fn model_spec(&self) -> String {
        format!("{}::{}", self.provider, self.model)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoutingSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ModelRoute>,
}

impl RoutingSettings {
    pub fn route(&self, task: &TaskType) -> Option<&ModelRoute> {
        self.routes
            .iter()
            .find(|route| &route.task_type == task)
            .or_else(|| {
                (*task != TaskType::Default)
                    .then(|| self.route(&TaskType::Default))
                    .flatten()
            })
    }

    pub fn upsert(&mut self, route: ModelRoute) {
        match self
            .routes
            .iter_mut()
            .find(|existing| existing.task_type == route.task_type)
        {
            Some(existing) => *existing = route,
            None => self.routes.push(route),
        }
    }

    pub fn remove(&mut self, task: &TaskType) -> bool {
        let before = self.routes.len();
        self.routes.retain(|route| &route.task_type != task);
        self.routes.len() != before
    }
}

impl Store {
    pub fn routes(&self) -> RoutesRepo {
        RoutesRepo {
            store: self.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RoutesRepo {
    store: Store,
}

impl RoutesRepo {
    pub fn load(&self) -> Result<RoutingSettings> {
        self.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT task_type, provider, model, effort, max_tokens_per_turn, max_cost_usd,
                        max_concurrency
                 FROM model_routes ORDER BY task_type",
            )?;
            let rows = stmt.query_map([], route_row)?;
            Ok(RoutingSettings {
                routes: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            })
        })
    }

    pub fn upsert(&self, route: &ModelRoute) -> Result<()> {
        let validated = route.clone();
        validate_route(&validated)?;
        let task_key = validated.task_type.as_key();
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO model_routes
                 (task_type, provider, model, effort, max_tokens_per_turn, max_cost_usd,
                  max_concurrency, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(task_type) DO UPDATE SET
                    provider = excluded.provider,
                    model = excluded.model,
                    effort = excluded.effort,
                    max_tokens_per_turn = excluded.max_tokens_per_turn,
                    max_cost_usd = excluded.max_cost_usd,
                    max_concurrency = excluded.max_concurrency,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    task_key,
                    validated.provider,
                    validated.model,
                    validated.effort,
                    validated
                        .max_tokens_per_turn
                        .map(i64::try_from)
                        .transpose()?,
                    validated.max_cost_usd,
                    validated.max_concurrency.map(i64::try_from).transpose()?,
                    i64::try_from(now_millis())?,
                ],
            )?;
            Ok(())
        })
    }

    pub fn replace_all(&self, settings: &RoutingSettings) -> Result<()> {
        for route in &settings.routes {
            validate_route(route)?;
        }
        let routes: Vec<(String, ModelRoute)> = settings
            .routes
            .iter()
            .map(|route| (route.task_type.as_key(), route.clone()))
            .collect();
        let now = i64::try_from(now_millis())?;
        self.store.immediate(|tx| {
            tx.execute("DELETE FROM model_routes", [])?;
            for (key, route) in &routes {
                tx.execute(
                    "INSERT INTO model_routes
                     (task_type, provider, model, effort, max_tokens_per_turn, max_cost_usd,
                      max_concurrency, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        key,
                        route.provider,
                        route.model,
                        route.effort,
                        route.max_tokens_per_turn.map(i64::try_from).transpose()?,
                        route.max_cost_usd,
                        route.max_concurrency.map(i64::try_from).transpose()?,
                        now,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn remove(&self, task: &TaskType) -> Result<bool> {
        let key = task.as_key();
        self.store.immediate(|tx| {
            let removed = tx.execute("DELETE FROM model_routes WHERE task_type = ?1", [key])?;
            Ok(removed == 1)
        })
    }
}

fn validate_route(route: &ModelRoute) -> Result<()> {
    ensure!(
        !route.provider.trim().is_empty(),
        "route provider must not be empty"
    );
    ensure!(
        !route.model.trim().is_empty(),
        "route model must not be empty"
    );
    ensure!(
        !route.model.contains("::"),
        "route model must not contain a provider prefix"
    );
    if let Some(cost) = route.max_cost_usd {
        ensure!(
            cost.is_finite() && cost > 0.0,
            "route max_cost_usd must be a positive number"
        );
    }
    if let TaskType::Custom(name) = &route.task_type {
        ensure!(!name.trim().is_empty(), "route custom task type is empty");
    }
    Ok(())
}

fn route_row(row: &Row<'_>) -> rusqlite::Result<ModelRoute> {
    let raw: String = row.get(0)?;
    let task_type = TaskType::parse(&raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown task type: {raw}"),
            )),
        )
    })?;
    let max_tokens: Option<i64> = row.get(4)?;
    let max_concurrency: Option<i64> = row.get(6)?;
    Ok(ModelRoute {
        task_type,
        provider: row.get(1)?,
        model: row.get(2)?,
        effort: row.get(3)?,
        max_tokens_per_turn: max_tokens.and_then(|value| u64::try_from(value).ok()),
        max_cost_usd: row.get(5)?,
        max_concurrency: max_concurrency.and_then(|value| usize::try_from(value).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, RoutesRepo) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("routes.db")).unwrap();
        let repo = store.routes();
        (dir, repo)
    }

    fn route(task: TaskType, model: &str) -> ModelRoute {
        ModelRoute {
            task_type: task,
            provider: "openai".to_string(),
            model: model.to_string(),
            effort: Some("high".to_string()),
            max_tokens_per_turn: Some(20_000),
            max_cost_usd: Some(1.5),
            max_concurrency: Some(2),
        }
    }

    #[test]
    fn task_type_keys_round_trip() {
        for task in TaskType::BUILTIN {
            assert_eq!(TaskType::parse(&task.as_key()).unwrap(), task);
        }
        assert_eq!(
            TaskType::parse("custom:reviewer").unwrap(),
            TaskType::Custom("reviewer".to_string())
        );
        assert!(TaskType::parse("nope").is_err());
        assert!(TaskType::parse("custom:").is_err());
    }

    #[test]
    fn upsert_load_and_remove() {
        let (_dir, repo) = repo();
        repo.upsert(&route(TaskType::Build, "o3")).unwrap();
        let loaded = repo.load().unwrap();
        assert_eq!(loaded.routes.len(), 1);
        assert_eq!(loaded.routes[0].model, "o3");
        assert_eq!(loaded.routes[0].max_concurrency, Some(2));

        repo.upsert(&route(TaskType::Build, "o4-mini")).unwrap();
        let loaded = repo.load().unwrap();
        assert_eq!(loaded.routes.len(), 1);
        assert_eq!(loaded.routes[0].model, "o4-mini");

        assert!(repo.remove(&TaskType::Build).unwrap());
        assert!(!repo.remove(&TaskType::Build).unwrap());
        assert!(repo.load().unwrap().routes.is_empty());
    }

    #[test]
    fn replace_all_clears_stale_rows() {
        let (_dir, repo) = repo();
        repo.upsert(&route(TaskType::Build, "a")).unwrap();
        repo.upsert(&route(TaskType::Explore, "b")).unwrap();
        let settings = RoutingSettings {
            routes: vec![route(TaskType::Memory, "c")],
        };
        repo.replace_all(&settings).unwrap();
        let loaded = repo.load().unwrap();
        assert_eq!(loaded.routes, settings.routes);
    }

    #[test]
    fn rejects_invalid_routes() {
        let (_dir, repo) = repo();
        let mut bad = route(TaskType::Build, "m");
        bad.provider = "  ".to_string();
        assert!(repo.upsert(&bad).is_err());

        let mut prefixed = route(TaskType::Build, "openai::m");
        prefixed.model = "openai::m".to_string();
        assert!(repo.upsert(&prefixed).is_err());

        let mut negative = route(TaskType::Build, "m");
        negative.max_cost_usd = Some(-1.0);
        assert!(repo.upsert(&negative).is_err());
    }

    #[test]
    fn lookup_falls_back_to_the_default_route() {
        let settings = RoutingSettings {
            routes: vec![route(TaskType::Default, "base")],
        };
        assert_eq!(
            settings.route(&TaskType::Explore).unwrap().model,
            "base".to_string()
        );
        assert_eq!(
            settings.route(&TaskType::Memory).unwrap().model_spec(),
            "openai::base"
        );
        assert!(RoutingSettings::default().route(&TaskType::Build).is_none());
    }
}
