use std::sync::Arc;

use meerai_core::ToolObservation;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnStopAction {
    Finish,
    Continue { reason: String },
}

pub type SharedInterceptor = Arc<dyn Interceptor>;

pub trait Interceptor: Send + Sync {
    fn before_step(&self, _plan: &mut StepPlan) -> StepAction {
        StepAction::Continue
    }

    fn after_tool(&self, _outcome: &ToolOutcome) -> Option<String> {
        None
    }

    fn turn_stopping(&self, _turn_id: &TurnId, _step_index: usize) -> TurnStopAction {
        TurnStopAction::Finish
    }
}
