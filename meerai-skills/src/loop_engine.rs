use std::sync::{Arc, Mutex, MutexGuard};

use meerai_session::loop_state::{LoopItemStatus, LoopPhase, LoopState, MAX_LOOP_ROUNDS};

pub const MAX_LOOP_TURNS: usize = 4;
const MAX_TEXT_CHARS: usize = 500;
const MAX_OBSERVATION_CHARS: usize = 400;
const MAX_RENDER_CHARS: usize = 6_000;

#[derive(Clone, Debug)]
pub struct LoopHandle(Arc<Mutex<LoopState>>);

impl LoopHandle {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(LoopState::default())))
    }

    pub fn lock(&self) -> MutexGuard<'_, LoopState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> LoopState {
        self.lock().clone()
    }
}

impl Default for LoopHandle {
    fn default() -> Self {
        Self::new()
    }
}

pub fn new_loop_handle() -> LoopHandle {
    LoopHandle::new()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopTurn {
    Finish,
    Continue { reason: String },
}

pub fn turn_stopping(state: &mut LoopState) -> LoopTurn {
    if !state.active() || state.all_done() {
        return LoopTurn::Finish;
    }
    if state.rounds >= MAX_LOOP_ROUNDS {
        state.pending_turn = true;
        return LoopTurn::Finish;
    }
    state.rounds += 1;
    LoopTurn::Continue {
        reason: continuation(state),
    }
}

pub fn follow_up_turn(
    state: &mut LoopState,
    budget_ended: bool,
    turn_ok: bool,
) -> Option<(usize, String)> {
    if !state.active() || state.all_done() || state.turns >= MAX_LOOP_TURNS {
        return None;
    }
    if !(budget_ended || (turn_ok && state.pending_turn)) {
        return None;
    }
    state.turns += 1;
    state.reset_rounds();
    let turn = state.turns;
    Some((turn, turn_prompt(state, turn, MAX_LOOP_TURNS)))
}

pub fn render(state: &LoopState) -> String {
    let mut out = String::new();
    match state.objective.as_deref().map(str::trim) {
        Some(objective) if !objective.is_empty() => {
            out.push_str("Objective: ");
            out.push_str(&cap_text(objective, MAX_TEXT_CHARS));
            out.push('\n');
        }
        _ => out.push_str("Objective: (none)\n"),
    }
    if !state.done_when.is_empty() {
        out.push_str("Done when:\n");
        for line in &state.done_when {
            out.push_str("- ");
            out.push_str(&cap_text(line, MAX_TEXT_CHARS));
            out.push('\n');
        }
    }
    if !state.constraints.is_empty() {
        out.push_str("Constraints:\n");
        for line in &state.constraints {
            out.push_str("- ");
            out.push_str(&cap_text(line, MAX_TEXT_CHARS));
            out.push('\n');
        }
    }
    out.push_str("Phase: ");
    out.push_str(state.phase.label());
    out.push('\n');
    if let Some(item) = state.current_item() {
        out.push_str(&format!(
            "Current item: {}. {}\n",
            item.id,
            cap_text(&item.text, MAX_TEXT_CHARS)
        ));
    }
    if state.items.is_empty() {
        out.push_str("Items: (none)\n");
    } else {
        out.push_str("Items:\n");
        for item in &state.items {
            out.push_str(&format!(
                "- [{}] {}. {}",
                item.status.mark(),
                item.id,
                cap_text(&item.text, MAX_TEXT_CHARS)
            ));
            if let Some(expect) = item.expect.as_deref() {
                out.push_str(" (observe: ");
                out.push_str(&cap_text(expect, MAX_TEXT_CHARS));
                out.push(')');
            }
            out.push('\n');
            if let Some(action) = item.action.as_deref() {
                out.push_str("  action: ");
                out.push_str(&cap_text(action, MAX_TEXT_CHARS));
                out.push('\n');
            }
            if let Some(observation) = item.observation.as_deref() {
                out.push_str("  observation: ");
                out.push_str(&cap_text(observation, MAX_OBSERVATION_CHARS));
                out.push('\n');
            }
        }
    }
    let done = state
        .items
        .iter()
        .filter(|item| item.status == LoopItemStatus::Done)
        .count();
    out.push_str(&format!("\nProgress: {done}/{} done", state.items.len()));
    if state.rounds > 0 {
        out.push_str(&format!(
            ", round {}/{}",
            state.rounds.min(MAX_LOOP_ROUNDS),
            MAX_LOOP_ROUNDS
        ));
    }
    out.push('\n');
    cap_text(&out, MAX_RENDER_CHARS)
}

pub fn continuation(state: &LoopState) -> String {
    let mut out = format!(
        "Loop round {}/{}: the objective is not complete.\n\n",
        state.rounds, MAX_LOOP_ROUNDS
    );
    out.push_str(&open_summary(state));
    out.push('\n');
    out.push_str(phase_instruction(state));
    out.push_str(
        "\nDo not stop while the loop is active. If a decision blocks you, call question-ask and \
         stop.",
    );
    out
}

pub fn turn_prompt(state: &LoopState, turn: usize, max_turns: usize) -> String {
    let mut out =
        format!("Loop turn {turn}/{max_turns}: the previous turn ended with open items.\n\n");
    out.push_str(&open_summary(state));
    out.push('\n');
    out.push_str(phase_instruction(state));
    out.push_str("\nThe loop stays active until every item is done or you must ask the user.");
    out
}

fn phase_instruction(state: &LoopState) -> &'static str {
    match state.phase {
        LoopPhase::Plan => {
            "Call loop-plan with the corrected objective, done condition, constraints, and items."
        }
        LoopPhase::Act => {
            "Do one action on the current item, then call loop-act with the action and the \
             expected observation."
        }
        LoopPhase::Observe => {
            "Collect the observation for the current item, then call loop-observe with the raw \
             result."
        }
        LoopPhase::Adjust => {
            "Judge the observation, then call loop-adjust with status done, failed, or open."
        }
    }
}

fn open_summary(state: &LoopState) -> String {
    let mut out = String::new();
    if let Some(objective) = state.objective.as_deref() {
        out.push_str("Objective: ");
        out.push_str(&cap_text(objective, MAX_TEXT_CHARS));
        out.push_str("\n\n");
    }
    out.push_str("Phase: ");
    out.push_str(state.phase.label());
    out.push('\n');
    if let Some(item) = state.current_item() {
        out.push_str(&format!(
            "Current item {}: {}\n",
            item.id,
            cap_text(&item.text, 300)
        ));
        if let Some(expect) = item.expect.as_deref() {
            out.push_str("Expected observation: ");
            out.push_str(&cap_text(expect, 300));
            out.push('\n');
        }
        if let Some(observation) = item.observation.as_deref() {
            out.push_str("Last observation: ");
            out.push_str(&cap_text(observation, 200));
            out.push('\n');
        }
        return out;
    }
    if state.items.is_empty() {
        out.push_str(
            "No items yet. Call loop-plan with 3-8 small, verifiable items before you act.\n",
        );
        return out;
    }
    out.push_str("Open items:\n");
    for item in state.open_items().take(10) {
        out.push_str("- ");
        out.push_str(&item.id.to_string());
        out.push_str(". ");
        out.push_str(&cap_text(&item.text, 300));
        out.push('\n');
    }
    out
}

pub fn cap_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use meerai_session::goal::{GoalItem, GoalItemStatus, GoalState};
    use meerai_session::loop_state::LoopPhase;

    use super::*;

    fn open_loop() -> LoopState {
        LoopState {
            objective: Some("Ship it".to_string()),
            items: vec![meerai_session::LoopItem {
                id: 1,
                text: "Fix it".to_string(),
                ..Default::default()
            }],
            current: Some(1),
            phase: LoopPhase::Act,
            ..LoopState::default()
        }
    }

    #[test]
    fn an_open_loop_continues_and_caps_rounds() {
        let mut state = open_loop();
        let LoopTurn::Continue { reason } = turn_stopping(&mut state) else {
            panic!("expected a continuation");
        };
        assert!(reason.contains("Loop round 1/"));
        assert!(reason.contains("Current item 1"));

        state.rounds = MAX_LOOP_ROUNDS;
        assert_eq!(turn_stopping(&mut state), LoopTurn::Finish);
        assert!(state.pending_turn);
    }

    #[test]
    fn a_done_or_empty_loop_finishes() {
        let mut empty = LoopState::default();
        assert_eq!(turn_stopping(&mut empty), LoopTurn::Finish);

        let mut done = open_loop();
        done.items[0].status = LoopItemStatus::Done;
        assert_eq!(turn_stopping(&mut done), LoopTurn::Finish);
    }

    #[test]
    fn follow_up_turns_need_a_pending_or_budget_end() {
        let mut state = open_loop();
        assert!(follow_up_turn(&mut state, false, true).is_none());

        state.pending_turn = true;
        let (turn, prompt) = follow_up_turn(&mut state, false, true).unwrap();
        assert_eq!(turn, 1);
        assert!(prompt.contains("Loop turn 1/"));

        state.turns = MAX_LOOP_TURNS;
        assert!(follow_up_turn(&mut state, true, false).is_none());
    }

    #[test]
    fn render_names_the_phase_and_the_item() {
        let state = open_loop();
        let text = render(&state);
        assert!(text.contains("Objective: Ship it"));
        assert!(text.contains("Phase: act"));
        assert!(text.contains("Current item: 1. Fix it"));
    }

    #[test]
    fn a_legacy_goal_maps_to_a_loop() {
        let goal = GoalState {
            goal: Some("Ship it".to_string()),
            done_when: vec!["tests pass".to_string()],
            items: vec![GoalItem {
                id: 1,
                text: "Fix it".to_string(),
                verify: Some("cargo test".to_string()),
                status: GoalItemStatus::Done,
                evidence: Some("ok".to_string()),
                ..GoalItem::default()
            }],
            ..GoalState::default()
        };
        let state = LoopState::from_goal(goal);
        assert_eq!(state.objective.as_deref(), Some("Ship it"));
        assert_eq!(state.items[0].expect.as_deref(), Some("cargo test"));
        assert_eq!(state.items[0].observation.as_deref(), Some("ok"));
        assert!(state.all_done());
        assert_eq!(state.phase, LoopPhase::Plan);
    }
}
