use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use meerai_common::settings::{ModelRoute, RoutingSettings, TaskType};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_GLOBAL_CONCURRENCY: usize = 4;

const GLOBAL_PROVIDER: &str = "provider";

#[derive(Debug)]
pub struct RoutingPermit {
    _global: OwnedSemaphorePermit,
    _provider: Option<OwnedSemaphorePermit>,
}

#[derive(Debug, Default)]
struct ProviderPool {
    permits: Mutex<HashMap<String, Arc<Semaphore>>>,
}

#[derive(Debug)]
pub struct RoutingCoordinator {
    global: Arc<Semaphore>,
    global_limit: usize,
    providers: ProviderPool,
    spend: Mutex<SpendState>,
}

#[derive(Debug, Default)]
struct SpendState {
    spent_usd: f64,
    cap_usd: Option<f64>,
}

impl RoutingCoordinator {
    pub fn new(global_limit: usize) -> Self {
        let global_limit = global_limit.max(1);
        Self {
            global: Arc::new(Semaphore::new(global_limit)),
            global_limit,
            providers: ProviderPool::default(),
            spend: Mutex::new(SpendState::default()),
        }
    }

    pub fn from_settings(settings: &RoutingSettings, global_limit: usize) -> Self {
        let coordinator = Self::new(global_limit);
        coordinator.set_provider_limits(settings);
        if let Some(route) = settings.route(&TaskType::Default) {
            coordinator.set_cap(route.max_cost_usd);
        }
        coordinator
    }

    pub fn set_provider_limits(&self, settings: &RoutingSettings) {
        let mut permits = self
            .providers
            .permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for route in &settings.routes {
            let Some(limit) = route.max_concurrency else {
                continue;
            };
            permits
                .entry(route.provider.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(limit.max(1))));
        }
    }

    pub fn global_limit(&self) -> usize {
        self.global_limit
    }

    pub fn provider_limit(&self, provider: &str) -> Option<usize> {
        self.providers
            .permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(provider)
            .map(|semaphore| semaphore.available_permits())
    }

    pub async fn acquire(&self, provider: &str) -> Option<RoutingPermit> {
        let provider_semaphore = self
            .providers
            .permits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(provider)
            .cloned();
        let global = self.global.clone().acquire_owned().await.ok()?;
        let provider_permit = match provider_semaphore {
            Some(semaphore) => {
                let permit = semaphore.acquire_owned().await.ok();
                permit.as_ref()?;
                permit
            }
            None => None,
        };
        Some(RoutingPermit {
            _global: global,
            _provider: provider_permit,
        })
    }

    pub fn available(&self) -> usize {
        self.global.available_permits()
    }

    pub fn note_cost(&self, cost: f64) -> f64 {
        if !cost.is_finite() || cost <= 0.0 {
            return self.spent_usd();
        }
        let mut spend = self
            .spend
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        spend.spent_usd += cost;
        spend.spent_usd
    }

    pub fn spent_usd(&self) -> f64 {
        self.spend
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spent_usd
    }

    pub fn set_cap(&self, cap: Option<f64>) {
        let mut spend = self
            .spend
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        spend.cap_usd = cap.filter(|cap| cap.is_finite() && *cap > 0.0);
    }

    pub fn cap(&self) -> Option<f64> {
        self.spend
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cap_usd
    }

    pub fn over_cap(&self) -> bool {
        let spend = self
            .spend
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        spend.cap_usd.is_some_and(|cap| spend.spent_usd >= cap)
    }
}

pub fn resolve_route<'a>(settings: &'a RoutingSettings, task: &TaskType) -> Option<&'a ModelRoute> {
    settings.route(task)
}

pub fn route_model(settings: &RoutingSettings, task: &TaskType, fallback: &str) -> String {
    resolve_route(settings, task).map_or_else(|| fallback.to_string(), ModelRoute::model_spec)
}

pub fn parse_effort(raw: &str) -> Option<meerai_common::settings::EffortLevel> {
    use meerai_common::settings::EffortLevel;
    match raw.trim().to_ascii_lowercase().as_str() {
        "low" => Some(EffortLevel::Low),
        "medium" => Some(EffortLevel::Medium),
        "high" => Some(EffortLevel::High),
        _ => None,
    }
}

pub fn provider_of(model: &str) -> String {
    let trimmed = model.trim();
    let split = trimmed.split_once("::").or_else(|| trimmed.split_once(':'));
    match split {
        Some((provider, rest)) if !provider.is_empty() && !rest.is_empty() => provider.to_string(),
        _ => GLOBAL_PROVIDER.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn route(
        task: TaskType,
        provider: &str,
        model: &str,
        concurrency: Option<usize>,
    ) -> ModelRoute {
        ModelRoute {
            task_type: task,
            provider: provider.to_string(),
            model: model.to_string(),
            effort: None,
            max_tokens_per_turn: None,
            max_cost_usd: None,
            max_concurrency: concurrency,
        }
    }

    #[test]
    fn coordinator_takes_the_default_route_cost_cap() {
        let mut route = route(TaskType::Default, "openai", "base", None);
        route.max_cost_usd = Some(2.5);
        let settings = RoutingSettings {
            routes: vec![route],
        };
        let coordinator = RoutingCoordinator::from_settings(&settings, 4);
        assert_eq!(coordinator.cap(), Some(2.5));
    }

    #[tokio::test]
    async fn provider_limit_serializes_parallel_calls() {
        let settings = RoutingSettings {
            routes: vec![route(TaskType::Build, "openai", "o3", Some(1))],
        };
        let coordinator = Arc::new(RoutingCoordinator::from_settings(&settings, 4));
        let first = coordinator.acquire("openai").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), coordinator.acquire("openai"))
                .await
                .is_err(),
            "a second call must wait for the single provider permit"
        );
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), coordinator.acquire("openai"))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn unconfigured_provider_only_needs_the_global_permit() {
        let coordinator = RoutingCoordinator::new(2);
        let _first = coordinator.acquire("anthropic").await.unwrap();
        let _second = coordinator.acquire("anthropic").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), coordinator.acquire("anthropic"))
                .await
                .is_err()
        );
        assert_eq!(coordinator.global_limit(), 2);
    }

    #[test]
    fn spend_ledger_tracks_cap_crossing() {
        let coordinator = RoutingCoordinator::new(1);
        assert!(!coordinator.over_cap());
        coordinator.set_cap(Some(0.5));
        assert_eq!(coordinator.note_cost(0.25), 0.25);
        assert!(!coordinator.over_cap());
        assert_eq!(coordinator.note_cost(0.30), 0.55);
        assert!(coordinator.over_cap());
        coordinator.set_cap(None);
        assert!(!coordinator.over_cap());
        assert_eq!(coordinator.spent_usd(), 0.55);
        assert_eq!(coordinator.note_cost(f64::NAN), 0.55);
        assert_eq!(coordinator.note_cost(-1.0), 0.55);
    }

    #[test]
    fn routes_resolve_and_fall_back_to_default() {
        let settings = RoutingSettings {
            routes: vec![
                route(TaskType::Default, "openai", "base", None),
                route(TaskType::Explore, "anthropic", "fast", None),
            ],
        };
        assert_eq!(
            route_model(&settings, &TaskType::Explore, "openai::fallback"),
            "anthropic::fast"
        );
        assert_eq!(
            route_model(&settings, &TaskType::Memory, "openai::fallback"),
            "openai::base"
        );
        assert_eq!(
            route_model(&RoutingSettings::default(), &TaskType::Build, "openai::x"),
            "openai::x"
        );
        assert_eq!(provider_of("openai::gpt"), "openai");
        assert_eq!(provider_of("openai:gpt"), "openai");
        assert_eq!(provider_of("bare"), GLOBAL_PROVIDER);
        assert_eq!(provider_of("::gpt"), GLOBAL_PROVIDER);
        assert_eq!(provider_of("openai::"), GLOBAL_PROVIDER);
    }
}
