use serde::{Deserialize, Serialize};

pub const MAX_GOAL_ROUNDS: usize = 24;
pub const MAX_GOAL_ITEMS: usize = 40;
const MAX_TEXT_CHARS: usize = 500;
const MAX_EVIDENCE_CHARS: usize = 400;
const MAX_RENDER_CHARS: usize = 6_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalItemStatus {
    #[default]
    Open,
    Done,
    Failed,
}

impl GoalItemStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" | "todo" | "pending" => Some(Self::Open),
            "done" | "complete" | "completed" | "passed" => Some(Self::Done),
            "failed" | "blocked" | "error" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn mark(self) -> &'static str {
        match self {
            Self::Open => " ",
            Self::Done => "x",
            Self::Failed => "!",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalItem {
    pub id: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    pub status: GoalItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_when: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<GoalItem>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rounds: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turns: usize,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending_turn: bool,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl GoalState {
    pub fn active(&self) -> bool {
        self.goal
            .as_deref()
            .is_some_and(|goal| !goal.trim().is_empty())
    }

    pub fn all_done(&self) -> bool {
        !self.items.is_empty()
            && self
                .items
                .iter()
                .all(|item| item.status == GoalItemStatus::Done)
    }

    pub fn open_items(&self) -> impl Iterator<Item = &GoalItem> {
        self.items
            .iter()
            .filter(|item| item.status != GoalItemStatus::Done)
    }

    pub fn reset_rounds(&mut self) {
        self.rounds = 0;
        self.pending_turn = false;
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        match self.goal.as_deref().map(str::trim) {
            Some(goal) if !goal.is_empty() => {
                out.push_str("Goal: ");
                out.push_str(&cap_text(goal, MAX_TEXT_CHARS));
                out.push('\n');
            }
            _ => out.push_str("Goal: (none)\n"),
        }
        if !self.done_when.is_empty() {
            out.push_str("Done when:\n");
            for line in &self.done_when {
                out.push_str("- ");
                out.push_str(&cap_text(line, MAX_TEXT_CHARS));
                out.push('\n');
            }
        }
        if !self.constraints.is_empty() {
            out.push_str("Constraints:\n");
            for line in &self.constraints {
                out.push_str("- ");
                out.push_str(&cap_text(line, MAX_TEXT_CHARS));
                out.push('\n');
            }
        }
        if self.items.is_empty() {
            out.push_str("Items: (none)\n");
        } else {
            out.push_str("Items:\n");
            for item in &self.items {
                out.push_str(&format!(
                    "- [{}] {}. {}",
                    item.status.mark(),
                    item.id,
                    cap_text(&item.text, MAX_TEXT_CHARS)
                ));
                if let Some(verify) = item.verify.as_deref() {
                    out.push_str(" (verify: ");
                    out.push_str(&cap_text(verify, MAX_TEXT_CHARS));
                    out.push(')');
                }
                if let Some(evidence) = item.evidence.as_deref() {
                    out.push_str("\n  evidence: ");
                    out.push_str(&cap_text(evidence, MAX_EVIDENCE_CHARS));
                }
                out.push('\n');
            }
        }
        let done = self
            .items
            .iter()
            .filter(|item| item.status == GoalItemStatus::Done)
            .count();
        out.push_str(&format!("\nProgress: {done}/{} done", self.items.len()));
        if self.rounds > 0 {
            out.push_str(&format!(
                ", round {}/{}",
                self.rounds.min(MAX_GOAL_ROUNDS),
                MAX_GOAL_ROUNDS
            ));
        }
        out.push('\n');
        cap_text(&out, MAX_RENDER_CHARS)
    }

    pub fn continuation(&self) -> String {
        let mut out = format!(
            "Goal round {}/{}: the goal is not complete.\n\n",
            self.rounds, MAX_GOAL_ROUNDS
        );
        out.push_str(&self.open_summary());
        out.push_str(
            "\nAct on one open item now. Run its verification, then call goal-check with \
             status done and the evidence. Do not stop while an item is open. If a decision \
             blocks you, call question-ask and stop.",
        );
        out
    }

    pub fn turn_prompt(&self, turn: usize, max_turns: usize) -> String {
        let mut out =
            format!("Goal turn {turn}/{max_turns}: the previous turn ended with open items.\n\n");
        out.push_str(&self.open_summary());
        out.push_str(
            "\nContinue with the first open item. Run its verification, then call goal-check \
             with status done and the evidence. The goal stays active until every item is \
             done or you must ask the user.",
        );
        out
    }

    fn open_summary(&self) -> String {
        let mut out = String::new();
        if let Some(goal) = self.goal.as_deref() {
            out.push_str("Goal: ");
            out.push_str(&cap_text(goal, MAX_TEXT_CHARS));
            out.push_str("\n\n");
        }
        if self.items.is_empty() {
            out.push_str(
                "No checklist yet. Call goal-set with 3-8 small, verifiable items before you act.\n",
            );
            return out;
        }
        out.push_str("Open items:\n");
        for item in self.open_items().take(10) {
            out.push_str("- ");
            out.push_str(&item.id.to_string());
            out.push_str(". ");
            out.push_str(&cap_text(&item.text, 300));
            if let Some(verify) = item.verify.as_deref() {
                out.push_str(" (verify: ");
                out.push_str(&cap_text(verify, 300));
                out.push(')');
            }
            if let Some(evidence) = item.evidence.as_deref() {
                out.push_str(" [last evidence: ");
                out.push_str(&cap_text(evidence, 200));
                out.push(']');
            }
            out.push('\n');
        }
        out
    }
}

pub fn cap_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn last_goal(events: &[crate::LoggedEvent]) -> Option<GoalState> {
    events.iter().rev().find_map(|logged| match &logged.event {
        crate::SessionEvent::GoalUpdated { state } => Some(state.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal() -> GoalState {
        GoalState {
            goal: Some("Ship the change".to_string()),
            done_when: vec!["cargo test passes".to_string()],
            items: vec![
                GoalItem {
                    id: 1,
                    text: "Fix it".to_string(),
                    verify: Some("cargo test".to_string()),
                    status: GoalItemStatus::Done,
                    evidence: Some("passed".to_string()),
                },
                GoalItem {
                    id: 2,
                    text: "Review it".to_string(),
                    ..GoalItem::default()
                },
            ],
            ..GoalState::default()
        }
    }

    #[test]
    fn snapshots_roundtrip_through_json() {
        let state = goal();
        let json = serde_json::to_string(&state).unwrap();
        let restored: GoalState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, state);
        assert!(json.contains("\"status\":\"done\""));
        assert!(!json.contains("rounds"));
    }

    #[test]
    fn predicates_and_render_track_open_items() {
        let state = goal();
        assert!(state.active());
        assert!(!state.all_done());
        assert_eq!(state.open_items().count(), 1);
        let rendered = state.render();
        assert!(rendered.contains("- [x] 1. Fix it"));
        assert!(rendered.contains("- [ ] 2. Review it"));
        assert!(rendered.contains("Progress: 1/2 done"));
    }

    #[test]
    fn a_turn_prompt_names_the_open_items() {
        let mut state = goal();
        state.turns = 2;
        state.reset_rounds();
        let prompt = state.turn_prompt(3, 4);
        assert!(prompt.contains("Goal turn 3/4"));
        assert!(prompt.contains("2. Review it"));
        assert!(!prompt.contains("1. Fix it\n"));
    }

    #[test]
    fn an_empty_goal_is_inactive() {
        let state = GoalState::default();
        assert!(!state.active());
        assert!(!state.all_done());
    }

    #[test]
    fn last_goal_reads_the_newest_event() {
        let first = GoalState {
            goal: Some("old".to_string()),
            ..GoalState::default()
        };
        let second = goal();
        let events = vec![
            crate::LoggedEvent {
                seq: crate::Seq(1),
                event: crate::SessionEvent::GoalUpdated { state: first },
            },
            crate::LoggedEvent {
                seq: crate::Seq(2),
                event: crate::SessionEvent::GoalUpdated {
                    state: second.clone(),
                },
            },
        ];
        assert_eq!(last_goal(&events), Some(second));
        assert_eq!(last_goal(&[]), None);
    }
}
