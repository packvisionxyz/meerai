use meerai_core::{EffectClass, ToolDefinition, ToolError, ToolOutput, async_trait};
use serde::Deserialize;

use crate::goal::{GoalHandle, GoalItem, GoalItemStatus, MAX_GOAL_ITEMS, cap_text};

pub const GOAL_SET: &str = "goal-set";
pub const GOAL_CHECK: &str = "goal-check";
pub const GOAL_STATUS: &str = "goal-status";
const MAX_GOAL_CHARS: usize = 1_000;
const MAX_EVIDENCE_CHARS: usize = 400;

pub struct GoalToolset {
    state: GoalHandle,
}

impl GoalToolset {
    pub fn new(state: GoalHandle) -> Self {
        Self { state }
    }

    fn set(&self, args: SetArgs) -> ToolOutput {
        let goal = args.goal.trim();
        if goal.is_empty() {
            return ToolOutput::Fail("goal-set needs a non-empty goal".to_string());
        }
        let mut state = self.state.lock();
        state.goal = Some(cap_text(goal, MAX_GOAL_CHARS));
        state.done_when = normalized_list(args.done_when);
        state.constraints = normalized_list(args.constraints);
        state.rounds = 0;
        state.items = args
            .items
            .into_iter()
            .take(MAX_GOAL_ITEMS)
            .enumerate()
            .map(|(index, item)| new_item(index as u32 + 1, item))
            .collect();
        ToolOutput::Text(state.render())
    }

    fn check(&self, args: CheckArgs) -> ToolOutput {
        let Some(status) = GoalItemStatus::parse(&args.status) else {
            return ToolOutput::Fail(format!(
                "unknown status '{}'; use open, done, or failed",
                args.status
            ));
        };
        let evidence = args
            .evidence
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| cap_text(text, MAX_EVIDENCE_CHARS));
        if status == GoalItemStatus::Done && evidence.is_none() {
            return ToolOutput::Fail(
                "goal-check needs non-empty evidence to mark an item done".to_string(),
            );
        }
        let mut state = self.state.lock();
        if !state.active() {
            return ToolOutput::Fail("no active goal; call goal-set before goal-check".to_string());
        }
        let Some(index) = state.items.iter().position(|item| item.id == args.item_id) else {
            let ids = state
                .items
                .iter()
                .map(|item| item.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return ToolOutput::Fail(format!(
                "unknown item id {}; known ids: {ids}",
                args.item_id
            ));
        };
        state.items[index].status = status;
        if let Some(evidence) = evidence {
            state.items[index].evidence = Some(evidence);
        }
        let base = state.items.iter().map(|item| item.id).max().unwrap_or(0);
        for (offset, added) in args.add.into_iter().take(MAX_GOAL_ITEMS).enumerate() {
            if state.items.len() >= MAX_GOAL_ITEMS {
                break;
            }
            state.items.push(new_item(base + 1 + offset as u32, added));
        }
        ToolOutput::Text(state.render())
    }

    fn status(&self) -> ToolOutput {
        let state = self.state.lock();
        if !state.active() {
            return ToolOutput::Text(
                "No active goal. Call goal-set with the goal, done condition, and a checklist. \
                 On a resumed session, the previous checklist is in the transcript; recreate it \
                 with goal-set if compaction removed it."
                    .to_string(),
            );
        }
        ToolOutput::Text(state.render())
    }
}

#[async_trait]
impl meerai_core::Toolset for GoalToolset {
    fn name(&self) -> String {
        "goal".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                r#type: "function".to_string(),
                name: GOAL_SET.to_string(),
                description: "Start a goal loop. Store the goal, the done condition, constraints, \
                              and an ordered checklist. Each item has one action and one \
                              verification."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "goal": { "type": "string", "description": "The goal in one sentence" },
                        "done_when": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Commands or observable results that prove completion"
                        },
                        "constraints": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Frozen decisions, owned files, and budgets"
                        },
                        "items": {
                            "type": "array",
                            "description": "Ordered checklist; 3-8 small items",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string", "description": "One action" },
                                    "verify": { "type": "string", "description": "One verification command or check" }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["goal"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: GOAL_CHECK.to_string(),
                description: "Record the result of one checklist item. Mark it done only with \
                              evidence from its verification. Failed items stay open. Add newly \
                              discovered items in the same call."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "item_id": { "type": "integer", "description": "Checklist item id" },
                        "status": {
                            "type": "string",
                            "enum": ["open", "done", "failed"],
                            "description": "done needs evidence"
                        },
                        "evidence": {
                            "type": "string",
                            "description": "Raw verification result or the failure text"
                        },
                        "add": {
                            "type": "array",
                            "description": "New items discovered during this item",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "verify": { "type": "string" }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["item_id", "status"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: GOAL_STATUS.to_string(),
                description: "Read the active goal and checklist. Use it after compaction or when \
                              you resume a session."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ]
    }

    fn contain(&self, fn_name: &str) -> bool {
        matches!(fn_name, GOAL_SET | GOAL_CHECK | GOAL_STATUS)
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        match fn_name {
            GOAL_SET => Ok(self.set(serde_json::from_str(args)?)),
            GOAL_CHECK => Ok(self.check(serde_json::from_str(args)?)),
            GOAL_STATUS => Ok(self.status()),
            _ => Err(ToolError::InvalidFunctionName(fn_name.to_string())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SetArgs {
    goal: String,
    #[serde(default)]
    done_when: Vec<String>,
    #[serde(default)]
    constraints: Vec<String>,
    #[serde(default)]
    items: Vec<ItemArg>,
}

#[derive(Debug, Deserialize)]
struct CheckArgs {
    item_id: u32,
    status: String,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    add: Vec<ItemArg>,
}

#[derive(Debug, Deserialize)]
struct ItemArg {
    text: String,
    #[serde(default)]
    verify: Option<String>,
}

fn new_item(id: u32, item: ItemArg) -> GoalItem {
    GoalItem {
        id,
        text: cap_text(item.text.trim(), MAX_GOAL_CHARS),
        verify: item
            .verify
            .as_deref()
            .map(str::trim)
            .filter(|verify| !verify.is_empty())
            .map(|verify| cap_text(verify, MAX_GOAL_CHARS)),
        status: GoalItemStatus::Open,
        evidence: None,
    }
}

fn normalized_list(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .take(MAX_GOAL_ITEMS)
        .collect()
}

#[cfg(test)]
mod tests {
    use meerai_core::Toolset;

    use super::*;
    use crate::goal::{MAX_GOAL_ROUNDS, new_goal_handle};

    fn toolset() -> (GoalToolset, GoalHandle) {
        let handle = new_goal_handle();
        (GoalToolset::new(handle.clone()), handle)
    }

    #[tokio::test]
    async fn set_stores_the_goal_and_its_checklist() {
        let (toolset, handle) = toolset();
        let output = toolset
            .invoke(
                GOAL_SET,
                r#"{"goal":"Fix the test","done_when":["cargo test passes"],
                    "constraints":["no generated edits"],
                    "items":[{"text":"Reproduce","verify":"cargo test -p x"},{"text":"Fix"}]}"#,
            )
            .await
            .unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected text");
        };
        assert!(text.contains("Goal: Fix the test"));
        assert!(text.contains("- [ ] 1. Reproduce (verify: cargo test -p x)"));
        assert!(text.contains("- [ ] 2. Fix"));
        assert_eq!(handle.lock().items.len(), 2);
    }

    #[tokio::test]
    async fn set_rejects_an_empty_goal() {
        let (toolset, _) = toolset();
        let output = toolset.invoke(GOAL_SET, r#"{"goal":"  "}"#).await.unwrap();
        assert!(matches!(output, ToolOutput::Fail(_)));
    }

    #[tokio::test]
    async fn done_requires_evidence() {
        let (toolset, handle) = toolset();
        toolset
            .invoke(GOAL_SET, r#"{"goal":"G","items":[{"text":"A"}]}"#)
            .await
            .unwrap();
        let output = toolset
            .invoke(GOAL_CHECK, r#"{"item_id":1,"status":"done"}"#)
            .await
            .unwrap();
        assert!(matches!(output, ToolOutput::Fail(_)));
        assert_eq!(handle.lock().items[0].status, GoalItemStatus::Open);
    }

    #[tokio::test]
    async fn check_records_evidence_and_adds_items() {
        let (toolset, handle) = toolset();
        toolset
            .invoke(GOAL_SET, r#"{"goal":"G","items":[{"text":"A"}]}"#)
            .await
            .unwrap();
        let output = toolset
            .invoke(
                GOAL_CHECK,
                r#"{"item_id":1,"status":"done","evidence":"test passed",
                    "add":[{"text":"B","verify":"cargo check"}]}"#,
            )
            .await
            .unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected text");
        };
        assert!(text.contains("- [x] 1. A"));
        assert!(text.contains("evidence: test passed"));
        assert!(text.contains("- [ ] 2. B (verify: cargo check)"));
        let state = handle.lock();
        assert!(!state.all_done());
        assert_eq!(state.items[1].id, 2);
    }

    #[tokio::test]
    async fn check_rejects_an_unknown_item_id() {
        let (toolset, _) = toolset();
        toolset
            .invoke(GOAL_SET, r#"{"goal":"G","items":[{"text":"A"}]}"#)
            .await
            .unwrap();
        let output = toolset
            .invoke(
                GOAL_CHECK,
                r#"{"item_id":9,"status":"done","evidence":"x"}"#,
            )
            .await
            .unwrap();
        let ToolOutput::Fail(text) = output else {
            panic!("expected failure");
        };
        assert!(text.contains("known ids: 1"));
    }

    #[tokio::test]
    async fn status_reports_no_active_goal() {
        let (toolset, _) = toolset();
        let output = toolset.invoke(GOAL_STATUS, "{}").await.unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected text");
        };
        assert!(text.contains("No active goal"));
    }

    #[test]
    fn the_tool_is_read_classified_and_never_spills() {
        let (toolset, _) = toolset();
        assert_eq!(toolset.effect_of(GOAL_SET), EffectClass::Read);
        assert!(!toolset.should_spill_output(GOAL_CHECK));
        assert!(toolset.contain(GOAL_STATUS));
        assert!(!toolset.contain("goal-drop"));
        assert_eq!(MAX_GOAL_ROUNDS, 24);
    }
}
