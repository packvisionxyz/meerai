use std::path::Path;
use std::sync::Arc;

use meerai_core::{EffectClass, ToolObservation, async_trait};
use meerai_session::{SessionEvent, SnapshotStore};

use crate::events::{ItemStatus, TurnId};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepPlan {
    pub turn_id: TurnId,
    pub step_index: usize,
    pub model: String,
    pub final_step: bool,
    pub tool_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepAction {
    Continue,
    Stop { reason: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    pub turn_id: TurnId,
    pub step_index: usize,
    pub tool_name: String,
    pub call_id: String,
    pub status: ItemStatus,
    pub observation: ToolObservation,
}

pub struct ToolPlan<'a> {
    pub turn_id: &'a TurnId,
    pub step_index: usize,
    pub tool_name: &'a str,
    pub call_id: &'a str,
    pub args: &'a str,
    pub effect: EffectClass,
}

pub struct ToolContext<'a> {
    pub workspace: &'a Path,
    pub snapshots: Option<&'a SnapshotStore>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnStopAction {
    Finish,
    Continue { reason: String },
}

pub type SharedInterceptor = Arc<dyn Interceptor>;

#[async_trait]
pub trait Interceptor: Send + Sync {
    fn before_step(&self, _plan: &mut StepPlan) -> StepAction {
        StepAction::Continue
    }

    async fn before_tool(&self, _plan: &ToolPlan<'_>, _ctx: &ToolContext<'_>) -> Vec<SessionEvent> {
        Vec::new()
    }

    fn after_tool(&self, _outcome: &ToolOutcome) -> Option<String> {
        None
    }

    fn turn_stopping(&self, _turn_id: &TurnId, _step_index: usize) -> TurnStopAction {
        TurnStopAction::Finish
    }
}
