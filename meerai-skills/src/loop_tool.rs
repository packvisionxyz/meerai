use meerai_core::{EffectClass, ToolDefinition, ToolError, ToolOutput, async_trait};
use meerai_session::loop_state::{LoopItem, LoopItemStatus, LoopPhase, MAX_LOOP_ITEMS};
use serde::Deserialize;

use crate::loop_engine::{LoopHandle, cap_text, render};

pub const LOOP_PLAN: &str = "loop-plan";
pub const LOOP_ACT: &str = "loop-act";
pub const LOOP_OBSERVE: &str = "loop-observe";
pub const LOOP_ADJUST: &str = "loop-adjust";
pub const LOOP_STATUS: &str = "loop-status";

const MAX_TEXT_CHARS: usize = 1_000;
const MAX_OBSERVATION_CHARS: usize = 600;

pub struct LoopToolset {
    state: LoopHandle,
}

impl LoopToolset {
    pub fn new(state: LoopHandle) -> Self {
        Self { state }
    }

    fn plan(&self, args: PlanArgs) -> ToolOutput {
        let objective = args.objective.trim();
        if objective.is_empty() {
            return ToolOutput::Fail("loop-plan needs a non-empty objective".to_string());
        }
        if args.items.is_empty() {
            return ToolOutput::Fail("loop-plan needs at least one item".to_string());
        }
        let mut state = self.state.lock();
        state.objective = Some(cap_text(objective, MAX_TEXT_CHARS));
        state.done_when = normalized_list(args.done_when);
        state.constraints = normalized_list(args.constraints);
        let mut existing = std::mem::take(&mut state.items);
        let mut next_id = existing.iter().map(|item| item.id).max().unwrap_or(0);
        let mut items = Vec::new();
        for arg in args.items.into_iter().take(MAX_LOOP_ITEMS) {
            let reused = match arg.id {
                Some(id) => existing.iter().position(|item| item.id == id),
                None => (!existing.is_empty()).then_some(0),
            };
            match reused {
                Some(index) => {
                    let mut item = existing.remove(index);
                    item.text = cap_text(arg.text.trim(), MAX_TEXT_CHARS);
                    item.expect = normalized_text(arg.expect);
                    items.push(item);
                }
                None => {
                    next_id += 1;
                    items.push(new_item(next_id, arg));
                }
            }
        }
        state.items = items;
        state.current = state
            .items
            .iter()
            .find(|item| item.status != LoopItemStatus::Done)
            .map(|item| item.id);
        state.phase = if state.current.is_some() {
            LoopPhase::Act
        } else {
            LoopPhase::Plan
        };
        state.rounds = 0;
        state.pending_turn = false;
        ToolOutput::Text(render(&state))
    }

    fn act(&self, args: ActArgs) -> ToolOutput {
        let mut state = self.state.lock();
        if !state.active() {
            return ToolOutput::Fail("no active loop; call loop-plan first".to_string());
        }
        if state.phase != LoopPhase::Act {
            return ToolOutput::Fail(format!(
                "loop-act runs in the act phase; the loop is in the {} phase",
                state.phase.label()
            ));
        }
        let Some(id) = args.item_id.or(state.current) else {
            return ToolOutput::Fail("no current item; call loop-plan".to_string());
        };
        if state.current != Some(id) {
            return ToolOutput::Fail(format!(
                "item {id} is not the current item; the current item is {}",
                state.current.unwrap_or_default()
            ));
        }
        let action = args.action.trim();
        if action.is_empty() {
            return ToolOutput::Fail("loop-act needs a non-empty action".to_string());
        }
        let expect = args
            .expect
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| cap_text(text, MAX_TEXT_CHARS));
        let Some(index) = state.items.iter().position(|item| item.id == id) else {
            return ToolOutput::Fail(format!("unknown item id {id}"));
        };
        state.items[index].action = Some(cap_text(action, MAX_TEXT_CHARS));
        if let Some(expect) = expect {
            state.items[index].expect = Some(expect);
        }
        state.phase = LoopPhase::Observe;
        ToolOutput::Text(render(&state))
    }

    fn observe(&self, args: ObserveArgs) -> ToolOutput {
        let mut state = self.state.lock();
        if state.phase != LoopPhase::Observe {
            return ToolOutput::Fail(format!(
                "loop-observe runs in the observe phase; the loop is in the {} phase",
                state.phase.label()
            ));
        }
        let Some(id) = args.item_id.or(state.current) else {
            return ToolOutput::Fail("no current item; call loop-plan".to_string());
        };
        let observation = args.observation.trim();
        if observation.is_empty() {
            return ToolOutput::Fail(
                "loop-observe needs the raw observation, not an empty result".to_string(),
            );
        }
        let Some(index) = state.items.iter().position(|item| item.id == id) else {
            return ToolOutput::Fail(format!("unknown item id {id}"));
        };
        state.items[index].observation = Some(cap_text(observation, MAX_OBSERVATION_CHARS));
        state.phase = LoopPhase::Adjust;
        ToolOutput::Text(render(&state))
    }

    fn adjust(&self, args: AdjustArgs) -> ToolOutput {
        let Some(status) = LoopItemStatus::parse(&args.status) else {
            return ToolOutput::Fail(format!(
                "unknown status '{}'; use done, failed, or open",
                args.status
            ));
        };
        let mut state = self.state.lock();
        if state.phase != LoopPhase::Adjust {
            return ToolOutput::Fail(format!(
                "loop-adjust runs in the adjust phase; the loop is in the {} phase",
                state.phase.label()
            ));
        }
        let Some(id) = args.item_id.or(state.current) else {
            return ToolOutput::Fail("no current item; call loop-plan".to_string());
        };
        let Some(index) = state.items.iter().position(|item| item.id == id) else {
            return ToolOutput::Fail(format!("unknown item id {id}"));
        };
        if state.items[index].observation.is_none() {
            return ToolOutput::Fail(
                "record the observation with loop-observe before you adjust the item".to_string(),
            );
        }
        state.items[index].status = status;
        let base = state.items.iter().map(|item| item.id).max().unwrap_or(0);
        for (offset, added) in args.add.into_iter().take(MAX_LOOP_ITEMS).enumerate() {
            if state.items.len() >= MAX_LOOP_ITEMS {
                break;
            }
            state.items.push(new_item(base + 1 + offset as u32, added));
        }
        match status {
            LoopItemStatus::Done => {
                state.current = state
                    .items
                    .iter()
                    .find(|item| item.status != LoopItemStatus::Done)
                    .map(|item| item.id);
                state.phase = if state.current.is_some() {
                    LoopPhase::Act
                } else {
                    LoopPhase::Plan
                };
            }
            LoopItemStatus::Open => {
                state.current = Some(id);
                state.phase = LoopPhase::Act;
            }
            LoopItemStatus::Failed => {
                state.current = Some(id);
                state.phase = LoopPhase::Plan;
            }
        }
        ToolOutput::Text(render(&state))
    }

    fn status(&self) -> ToolOutput {
        let state = self.state.lock();
        if !state.active() {
            return ToolOutput::Text(
                "No active loop. Call loop-plan with the objective, done condition, constraints, \
                 and items. On a resumed session, the previous plan is in the transcript; \
                 recreate it with loop-plan if compaction removed it."
                    .to_string(),
            );
        }
        ToolOutput::Text(render(&state))
    }
}

#[async_trait]
impl meerai_core::Toolset for LoopToolset {
    fn name(&self) -> String {
        "loop".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                r#type: "function".to_string(),
                name: LOOP_PLAN.to_string(),
                description: "Plan phase. Store the objective, the done condition, constraints, \
                              and an ordered list of items. Each item has one action and one \
                              expected observation. Add a review item when a human must \
                              approve the result. Replaces the previous plan."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "objective": {
                            "type": "string",
                            "description": "The outcome in one sentence"
                        },
                        "done_when": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Observable results that prove completion"
                        },
                        "constraints": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Frozen decisions, owned resources, and budgets"
                        },
                        "items": {
                            "type": "array",
                            "description": "Ordered work items; 3-8 small items",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {
                                        "type": "integer",
                                        "description": "Existing item id to keep its evidence"
                                    },
                                    "text": {
                                        "type": "string",
                                        "description": "One action"
                                    },
                                    "expect": {
                                        "type": "string",
                                        "description": "One observable result"
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["objective", "items"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: LOOP_ACT.to_string(),
                description: "Act phase. Record what you did on the current item and the \
                              observation you expect. Moves the loop to the observe phase."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "integer",
                            "description": "Item id; defaults to the current item"
                        },
                        "action": {
                            "type": "string",
                            "description": "What you did, in one or two sentences"
                        },
                        "expect": {
                            "type": "string",
                            "description": "The observation that would confirm the action"
                        }
                    },
                    "required": ["action"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: LOOP_OBSERVE.to_string(),
                description: "Observe phase. Record the raw observation for the current item: \
                              command output, URL text, measurement, or document quote. Moves the \
                              loop to the adjust phase."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "integer",
                            "description": "Item id; defaults to the current item"
                        },
                        "observation": {
                            "type": "string",
                            "description": "Raw evidence, not a summary"
                        }
                    },
                    "required": ["observation"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: LOOP_ADJUST.to_string(),
                description: "Adjust phase. Judge the recorded observation and set the item \
                              status. Done completes the item. Open retries it. Failed requires a \
                              replan. A review item needs the human answer before done. Add \
                              newly discovered items in the same call."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "item_id": {
                            "type": "integer",
                            "description": "Item id; defaults to the current item"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["done", "failed", "open"],
                            "description": "done completes the item"
                        },
                        "add": {
                            "type": "array",
                            "description": "New items discovered during the observation",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "expect": { "type": "string" }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["status"]
                }),
            },
            ToolDefinition {
                r#type: "function".to_string(),
                name: LOOP_STATUS.to_string(),
                description:
                    "Read the active loop, the current phase, and the items. Use it after \
                              compaction or when you resume a session."
                        .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ]
    }

    fn contain(&self, fn_name: &str) -> bool {
        matches!(
            fn_name,
            LOOP_PLAN | LOOP_ACT | LOOP_OBSERVE | LOOP_ADJUST | LOOP_STATUS
        )
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        match fn_name {
            LOOP_PLAN => Ok(self.plan(serde_json::from_str(args)?)),
            LOOP_ACT => Ok(self.act(serde_json::from_str(args)?)),
            LOOP_OBSERVE => Ok(self.observe(serde_json::from_str(args)?)),
            LOOP_ADJUST => Ok(self.adjust(serde_json::from_str(args)?)),
            LOOP_STATUS => Ok(self.status()),
            _ => Err(ToolError::InvalidFunctionName(fn_name.to_string())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PlanArgs {
    objective: String,
    #[serde(default)]
    done_when: Vec<String>,
    #[serde(default)]
    constraints: Vec<String>,
    #[serde(default)]
    items: Vec<ItemArg>,
}

#[derive(Debug, Deserialize)]
struct ActArgs {
    #[serde(default)]
    item_id: Option<u32>,
    action: String,
    #[serde(default)]
    expect: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ObserveArgs {
    #[serde(default)]
    item_id: Option<u32>,
    observation: String,
}

#[derive(Debug, Deserialize)]
struct AdjustArgs {
    #[serde(default)]
    item_id: Option<u32>,
    status: String,
    #[serde(default)]
    add: Vec<ItemArg>,
}

#[derive(Debug, Deserialize)]
struct ItemArg {
    #[serde(default)]
    id: Option<u32>,
    text: String,
    #[serde(default)]
    expect: Option<String>,
}

fn new_item(id: u32, item: ItemArg) -> LoopItem {
    LoopItem {
        id,
        text: cap_text(item.text.trim(), MAX_TEXT_CHARS),
        expect: normalized_text(item.expect),
        status: LoopItemStatus::Open,
        action: None,
        observation: None,
    }
}

fn normalized_text(text: Option<String>) -> Option<String> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| cap_text(text, MAX_TEXT_CHARS))
}

fn normalized_list(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .take(MAX_LOOP_ITEMS)
        .collect()
}
#[cfg(test)]
mod tests {
    use meerai_core::Toolset;
    use meerai_session::loop_state::{LoopItemStatus, LoopPhase};

    use super::*;
    use crate::loop_engine::new_loop_handle;

    fn toolset() -> (LoopToolset, LoopHandle) {
        let handle = new_loop_handle();
        (LoopToolset::new(handle.clone()), handle)
    }

    async fn call(toolset: &LoopToolset, name: &str, args: serde_json::Value) -> ToolOutput {
        toolset.invoke(name, &args.to_string()).await.unwrap()
    }

    async fn plan_one(toolset: &LoopToolset) -> ToolOutput {
        call(
            toolset,
            LOOP_PLAN,
            serde_json::json!({
                "objective": "Ship the report",
                "done_when": ["the report is published"],
                "items": [{ "text": "Draft it", "expect": "a draft exists" }]
            }),
        )
        .await
    }

    #[tokio::test]
    async fn the_four_phases_complete_an_item() {
        let (toolset, handle) = toolset();
        let output = plan_one(&toolset).await;
        let ToolOutput::Text(text) = output else {
            panic!("expected text");
        };
        assert!(text.contains("Objective: Ship the report"));
        assert_eq!(handle.lock().phase, LoopPhase::Act);

        call(
            &toolset,
            LOOP_ACT,
            serde_json::json!({ "item_id": 1, "action": "wrote the draft" }),
        )
        .await;
        assert_eq!(handle.lock().phase, LoopPhase::Observe);

        call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "item_id": 1, "observation": "draft.md exists" }),
        )
        .await;
        assert_eq!(handle.lock().phase, LoopPhase::Adjust);

        call(
            &toolset,
            LOOP_ADJUST,
            serde_json::json!({ "item_id": 1, "status": "done" }),
        )
        .await;
        let state = handle.lock();
        assert!(state.all_done());
        assert_eq!(state.items[0].status, LoopItemStatus::Done);
        assert_eq!(
            state.items[0].observation.as_deref(),
            Some("draft.md exists")
        );
        assert!(state.current.is_none());
    }

    #[tokio::test]
    async fn the_phase_order_is_enforced() {
        let (toolset, _handle) = toolset();
        let output = call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "observation": "too early" }),
        )
        .await;
        assert!(matches!(output, ToolOutput::Fail(text) if text.contains("observe phase")));

        plan_one(&toolset).await;
        let output = call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "observation": "still too early" }),
        )
        .await;
        assert!(matches!(output, ToolOutput::Fail(text) if text.contains("observe phase")));

        let output = call(&toolset, LOOP_ACT, serde_json::json!({ "action": "  " })).await;
        assert!(matches!(output, ToolOutput::Fail(text) if text.contains("non-empty action")));

        call(
            &toolset,
            LOOP_ACT,
            serde_json::json!({ "action": "wrote the draft" }),
        )
        .await;
        let output = call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "observation": " " }),
        )
        .await;
        assert!(matches!(output, ToolOutput::Fail(text) if text.contains("raw observation")));
        assert_eq!(handle_phase(&toolset), LoopPhase::Observe);
    }

    fn handle_phase(toolset: &LoopToolset) -> LoopPhase {
        toolset.state.lock().phase
    }

    #[tokio::test]
    async fn a_failed_item_returns_to_plan() {
        let (toolset, handle) = toolset();
        plan_one(&toolset).await;
        call(
            &toolset,
            LOOP_ACT,
            serde_json::json!({ "action": "wrote the draft" }),
        )
        .await;
        call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "observation": "the build rejected it" }),
        )
        .await;
        call(
            &toolset,
            LOOP_ADJUST,
            serde_json::json!({ "status": "failed" }),
        )
        .await;
        let state = handle.lock();
        assert_eq!(state.phase, LoopPhase::Plan);
        assert_eq!(state.items[0].status, LoopItemStatus::Failed);
        assert_eq!(state.current, Some(1));
    }

    #[tokio::test]
    async fn adjust_can_add_items_and_advance() {
        let (toolset, handle) = toolset();
        plan_one(&toolset).await;
        call(
            &toolset,
            LOOP_ACT,
            serde_json::json!({ "action": "wrote the draft" }),
        )
        .await;
        call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "observation": "draft.md exists" }),
        )
        .await;
        call(
            &toolset,
            LOOP_ADJUST,
            serde_json::json!({
                "status": "done",
                "add": [{ "text": "Publish it", "expect": "a public URL" }]
            }),
        )
        .await;
        let state = handle.lock();
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.current, Some(2));
        assert_eq!(state.phase, LoopPhase::Act);
    }

    #[tokio::test]
    async fn status_reads_the_state_and_plan_replaces_it() {
        let (toolset, handle) = toolset();
        let output = call(&toolset, LOOP_STATUS, serde_json::json!({})).await;
        assert!(matches!(output, ToolOutput::Text(text) if text.contains("No active loop")));

        plan_one(&toolset).await;
        call(
            &toolset,
            LOOP_PLAN,
            serde_json::json!({
                "objective": "Rebuild it",
                "items": [{ "text": "Restart" }]
            }),
        )
        .await;
        let state = handle.lock();
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].text, "Restart");
        assert_eq!(state.objective.as_deref(), Some("Rebuild it"));
    }

    #[tokio::test]
    async fn replanning_keeps_the_evidence_of_kept_items() {
        let (toolset, handle) = toolset();
        plan_one(&toolset).await;
        call(
            &toolset,
            LOOP_ACT,
            serde_json::json!({ "action": "wrote the draft" }),
        )
        .await;
        call(
            &toolset,
            LOOP_OBSERVE,
            serde_json::json!({ "observation": "draft.md exists" }),
        )
        .await;
        call(
            &toolset,
            LOOP_PLAN,
            serde_json::json!({
                "objective": "Ship the report",
                "items": [
                    { "id": 1, "text": "Draft it well" },
                    { "text": "Publish it", "expect": "a public URL" }
                ]
            }),
        )
        .await;
        let state = handle.lock();
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].id, 1);
        assert_eq!(state.items[0].text, "Draft it well");
        assert_eq!(
            state.items[0].observation.as_deref(),
            Some("draft.md exists")
        );
        assert_eq!(state.items[1].id, 2);
        assert_eq!(state.current, Some(1));
    }
}
