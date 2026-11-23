use std::sync::{Arc, Mutex, MutexGuard};

use crate::events::TurnId;
use crate::intercept::{Interceptor, TurnStopAction};

pub use meerai_session::goal::{
    GoalItem, GoalItemStatus, GoalState, MAX_GOAL_ITEMS, MAX_GOAL_ROUNDS, cap_text,
};

pub const MAX_GOAL_TURNS: usize = 4;

#[derive(Clone, Debug)]
pub struct GoalHandle(Arc<Mutex<GoalState>>);

impl GoalHandle {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(GoalState::default())))
    }

    pub fn lock(&self) -> MutexGuard<'_, GoalState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> GoalState {
        self.lock().clone()
    }
}

impl Default for GoalHandle {
    fn default() -> Self {
        Self::new()
    }
}

pub fn new_goal_handle() -> GoalHandle {
    GoalHandle::new()
}

pub struct GoalInterceptor {
    state: GoalHandle,
}

impl GoalInterceptor {
    pub fn new(state: GoalHandle) -> Self {
        Self { state }
    }
}

impl Interceptor for GoalInterceptor {
    fn turn_stopping(&self, _turn_id: &TurnId, _step_index: usize) -> TurnStopAction {
        let mut state = self.state.lock();
        if !state.active() || state.all_done() {
            return TurnStopAction::Finish;
        }
        if state.rounds >= MAX_GOAL_ROUNDS {
            state.pending_turn = true;
            return TurnStopAction::Finish;
        }
        state.rounds += 1;
        TurnStopAction::Continue {
            reason: state.continuation(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal_with(items: Vec<GoalItem>) -> GoalHandle {
        let handle = new_goal_handle();
        {
            let mut state = handle.lock();
            state.goal = Some("Ship the change".to_string());
            state.items = items;
        }
        handle
    }

    fn item(id: u32, status: GoalItemStatus) -> GoalItem {
        GoalItem {
            id,
            text: format!("item {id}"),
            status,
            ..GoalItem::default()
        }
    }

    #[test]
    fn an_empty_or_complete_goal_finishes_the_turn() {
        let empty = new_goal_handle();
        assert_eq!(
            GoalInterceptor::new(empty).turn_stopping(&TurnId::new(), 0),
            TurnStopAction::Finish
        );

        let done = goal_with(vec![
            item(1, GoalItemStatus::Done),
            item(2, GoalItemStatus::Done),
        ]);
        assert_eq!(
            GoalInterceptor::new(done).turn_stopping(&TurnId::new(), 0),
            TurnStopAction::Finish
        );
    }

    #[test]
    fn an_open_goal_continues_with_the_next_item() {
        let handle = goal_with(vec![
            item(1, GoalItemStatus::Done),
            item(2, GoalItemStatus::Open),
        ]);
        let interceptor = GoalInterceptor::new(handle.clone());
        let TurnStopAction::Continue { reason } = interceptor.turn_stopping(&TurnId::new(), 3)
        else {
            panic!("expected a continuation");
        };
        assert!(reason.contains("Goal round 1/"));
        assert!(reason.contains("2. item 2"));
        assert!(!reason.contains("1. item 1"));
        assert_eq!(handle.lock().rounds, 1);
    }

    #[test]
    fn the_round_cap_stops_the_loop() {
        let handle = goal_with(vec![item(1, GoalItemStatus::Open)]);
        handle.lock().rounds = MAX_GOAL_ROUNDS;
        assert_eq!(
            GoalInterceptor::new(handle.clone()).turn_stopping(&TurnId::new(), 0),
            TurnStopAction::Finish
        );
        assert!(handle.lock().pending_turn);
    }

    #[test]
    fn a_handle_clone_shares_the_same_state() {
        let handle = new_goal_handle();
        let clone = handle.clone();
        clone.lock().goal = Some("shared".to_string());
        assert_eq!(handle.snapshot().goal.as_deref(), Some("shared"));
    }
}
