use meerai_skills::{LoopHandle, LoopTurn, loop_engine};

use crate::events::TurnId;
use crate::intercept::{Interceptor, TurnStopAction};

pub struct LoopInterceptor {
    state: LoopHandle,
}

impl LoopInterceptor {
    pub fn new(state: LoopHandle) -> Self {
        Self { state }
    }
}

impl Interceptor for LoopInterceptor {
    fn turn_stopping(&self, _turn_id: &TurnId, _step_index: usize) -> TurnStopAction {
        match loop_engine::turn_stopping(&mut self.state.lock()) {
            LoopTurn::Finish => TurnStopAction::Finish,
            LoopTurn::Continue { reason } => TurnStopAction::Continue { reason },
        }
    }
}
