use serde::{Deserialize, Serialize};

pub const MAX_LOOP_ROUNDS: usize = 24;
pub const MAX_LOOP_ITEMS: usize = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhase {
    #[default]
    Plan,
    Act,
    Observe,
    Adjust,
}

impl LoopPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Act => "act",
            Self::Observe => "observe",
            Self::Adjust => "adjust",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopItemStatus {
    #[default]
    Open,
    Done,
    Failed,
}

impl LoopItemStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" | "todo" | "pending" | "retry" => Some(Self::Open),
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
pub struct LoopItem {
    pub id: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
    pub status: LoopItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_when: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<LoopItem>,
    #[serde(default)]
    pub phase: LoopPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<u32>,
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

impl LoopState {
    pub fn active(&self) -> bool {
        self.objective
            .as_deref()
            .is_some_and(|objective| !objective.trim().is_empty())
    }

    pub fn all_done(&self) -> bool {
        !self.items.is_empty()
            && self
                .items
                .iter()
                .all(|item| item.status == LoopItemStatus::Done)
    }

    pub fn open_items(&self) -> impl Iterator<Item = &LoopItem> {
        self.items
            .iter()
            .filter(|item| item.status != LoopItemStatus::Done)
    }

    pub fn current_item(&self) -> Option<&LoopItem> {
        let id = self.current?;
        self.items.iter().find(|item| item.id == id)
    }

    pub fn reset_rounds(&mut self) {
        self.rounds = 0;
        self.pending_turn = false;
    }

    pub fn from_goal(goal: crate::goal::GoalState) -> Self {
        let items: Vec<LoopItem> = goal
            .items
            .into_iter()
            .map(|item| LoopItem {
                id: item.id,
                text: item.text,
                expect: item.verify,
                status: match item.status {
                    crate::goal::GoalItemStatus::Open => LoopItemStatus::Open,
                    crate::goal::GoalItemStatus::Done => LoopItemStatus::Done,
                    crate::goal::GoalItemStatus::Failed => LoopItemStatus::Failed,
                },
                action: None,
                observation: item.evidence,
            })
            .collect();
        let current = items
            .iter()
            .find(|item| item.status != LoopItemStatus::Done)
            .map(|item| item.id);
        let phase = if current.is_some() {
            LoopPhase::Act
        } else {
            LoopPhase::Plan
        };
        Self {
            objective: goal.goal,
            done_when: goal.done_when,
            constraints: goal.constraints,
            phase,
            items,
            current,
            rounds: goal.rounds,
            turns: goal.turns,
            pending_turn: goal.pending_turn,
        }
    }
}

pub fn last_loop(events: &[crate::LoggedEvent]) -> Option<LoopState> {
    events.iter().rev().find_map(|logged| match &logged.event {
        crate::SessionEvent::LoopUpdated { state } => Some(state.clone()),
        crate::SessionEvent::GoalUpdated { state } => Some(LoopState::from_goal(state.clone())),
        _ => None,
    })
}
