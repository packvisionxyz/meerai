use std::sync::Arc;
use std::time::Duration;

use meerai_core::{
    EffectClass, PermissionAction, ToolObservation, ToolOutput, Toolset, async_trait,
};
use meerai_mcp::budget::AdmissionMode;
use meerai_mcp::tool::McpTool;
use meerai_mcp::toolset::{AdmissionPolicy, LOAD_TOOL_SCHEMA, McpToolset, ToolCaller};

struct FakeCaller {
    name: String,
    delay: Duration,
}

#[async_trait]
impl ToolCaller for FakeCaller {
    fn server_name(&self) -> &str {
        &self.name
    }

    async fn call(&self, tool: &McpTool, args: &str) -> Result<ToolObservation, String> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(ToolObservation::success(format!(
            "{} {args}",
            tool.runtime_name()
        )))
    }
}

fn tool(name: &str, schema_bytes: usize) -> McpTool {
    McpTool {
        server: "github".to_string(),
        name: name.to_string(),
        description: format!("{name} does a thing."),
        schema: serde_json::json!({
            "type": "object",
            "description": "x".repeat(schema_bytes),
            "properties": { "q": { "type": "string" } }
        }),
        read_only: false,
    }
}

fn caller() -> Arc<dyn ToolCaller> {
    Arc::new(FakeCaller {
        name: "github".to_string(),
        delay: Duration::ZERO,
    })
}

fn small() -> McpToolset {
    McpToolset::new(
        caller(),
        vec![tool("search_issues", 20), tool("create_issue", 20)],
        1_000,
    )
}

fn large(policy: AdmissionPolicy) -> McpToolset {
    let tools = (0..8).map(|i| tool(&format!("tool_{i}"), 4_000)).collect();
    McpToolset::with_policy(caller(), tools, 1_000, policy)
}

fn generous() -> AdmissionPolicy {
    AdmissionPolicy {
        full_budget_tokens: 1_000,
        max_loaded_tools: 3,
        max_loaded_tokens: 100_000,
    }
}

#[test]
fn a_small_surface_admits_every_schema_up_front() {
    let set = small();
    assert_eq!(set.mode(), AdmissionMode::Full);
    let names: Vec<String> = set.definition().into_iter().map(|d| d.name).collect();
    assert_eq!(
        names,
        vec!["mcp__github__search_issues", "mcp__github__create_issue"]
    );
    assert!(set.contain("mcp__github__search_issues"));
    assert!(!set.contain(LOAD_TOOL_SCHEMA));
}

#[test]
fn a_large_surface_admits_only_the_loader_at_first() {
    let set = large(AdmissionPolicy::default());
    assert_eq!(set.mode(), AdmissionMode::Index);

    let definitions = set.definition();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, LOAD_TOOL_SCHEMA);
    assert!(set.contain(LOAD_TOOL_SCHEMA));
    assert!(
        !set.contain("mcp__github__tool_0"),
        "an unloaded tool must not be callable"
    );
}

#[test]
fn the_loader_description_lists_every_tool_on_one_line_each() {
    let set = large(AdmissionPolicy::default());
    let definition = &set.definition()[0];
    for index in 0..8 {
        assert!(
            definition
                .description
                .contains(&format!("mcp__github__tool_{index}")),
            "missing tool_{index}"
        );
    }
    assert_eq!(
        definition.parameters["properties"]["name"]["type"],
        "string"
    );
    assert!(set.index_preview().lines().all(|line| !line.is_empty()));
}

#[tokio::test]
async fn loading_a_schema_admits_it_for_later_calls() {
    let set = large(generous());
    assert!(!set.contain("mcp__github__tool_0"));

    let loaded = set
        .invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_0"}"#)
        .await
        .unwrap();
    let ToolOutput::Text(text) = loaded else {
        panic!("expected text, got {loaded:?}");
    };
    assert!(text.contains("Loaded mcp__github__tool_0"), "{text}");
    assert!(text.contains("\"parameters\""), "{text}");

    assert!(set.contain("mcp__github__tool_0"));
    assert_eq!(set.loaded_tools(), vec!["mcp__github__tool_0".to_string()]);

    let names: Vec<String> = set.definition().into_iter().map(|d| d.name).collect();
    assert!(names.contains(&LOAD_TOOL_SCHEMA.to_string()));
    assert!(names.contains(&"mcp__github__tool_0".to_string()));

    let output = set
        .invoke("mcp__github__tool_0", r#"{"q":"x"}"#)
        .await
        .unwrap();
    assert_eq!(
        output,
        ToolOutput::Text(r#"mcp__github__tool_0 {"q":"x"}"#.to_string())
    );
}

#[tokio::test]
async fn calling_an_unloaded_tool_is_refused_with_a_hint() {
    let set = large(generous());
    let output = set.invoke("mcp__github__tool_0", "{}").await.unwrap();
    match output {
        ToolOutput::Fail(message) => {
            assert!(message.contains("is not loaded"), "{message}");
            assert!(message.contains(LOAD_TOOL_SCHEMA), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn loading_the_same_schema_twice_is_a_no_op() {
    let set = large(generous());
    set.invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_0"}"#)
        .await
        .unwrap();
    let tokens_after_first = set.loaded_tokens();

    let second = set
        .invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_0"}"#)
        .await
        .unwrap();
    let ToolOutput::Text(text) = second else {
        panic!("expected text");
    };
    assert!(text.contains("already loaded"), "{text}");
    assert_eq!(set.loaded_tokens(), tokens_after_first);
    assert_eq!(set.loaded_tools().len(), 1);
}

#[tokio::test]
async fn the_loaded_count_is_capped() {
    let set = large(AdmissionPolicy {
        full_budget_tokens: 1_000,
        max_loaded_tools: 2,
        max_loaded_tokens: 100_000,
    });

    for index in 0..2 {
        let out = set
            .invoke(
                LOAD_TOOL_SCHEMA,
                &format!(r#"{{"name":"mcp__github__tool_{index}"}}"#),
            )
            .await
            .unwrap();
        assert!(
            matches!(out, ToolOutput::Text(_)),
            "load {index} failed: {out:?}"
        );
    }

    let third = set
        .invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_2"}"#)
        .await
        .unwrap();
    match third {
        ToolOutput::Fail(message) => assert!(message.contains("limit 2"), "{message}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(set.loaded_tools().len(), 2);
}

#[tokio::test]
async fn the_loaded_token_budget_is_enforced() {
    let set = large(AdmissionPolicy {
        full_budget_tokens: 1_000,
        max_loaded_tools: 100,
        max_loaded_tokens: 1_500,
    });
    let first = set
        .invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_0"}"#)
        .await
        .unwrap();
    assert!(matches!(first, ToolOutput::Text(_)), "{first:?}");

    let second = set
        .invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_1"}"#)
        .await
        .unwrap();
    match second {
        ToolOutput::Fail(message) => assert!(message.contains("already used"), "{message}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(set.loaded_tools().len(), 1);
}

#[tokio::test]
async fn loading_an_unknown_tool_fails() {
    let set = large(generous());
    let output = set
        .invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__nope"}"#)
        .await
        .unwrap();
    match output {
        ToolOutput::Fail(message) => assert!(message.contains("has no tool"), "{message}"),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn loading_with_bad_arguments_fails_cleanly() {
    let set = large(generous());
    for args in ["not json", "{}", r#"{"name":42}"#] {
        let output = set.invoke(LOAD_TOOL_SCHEMA, args).await.unwrap();
        assert!(
            matches!(output, ToolOutput::Fail(_)),
            "{args} -> {output:?}"
        );
    }
}

#[test]
fn the_loader_is_read_effect_and_its_own_resource() {
    let set = large(generous());
    assert_eq!(set.effect_of(LOAD_TOOL_SCHEMA), EffectClass::Read);
    assert_eq!(
        set.permission_resource(LOAD_TOOL_SCHEMA, "{}"),
        Some("github/load_tool_schema".to_string())
    );
    let spec = set.permission_spec(LOAD_TOOL_SCHEMA).unwrap();
    assert_eq!(spec.action, PermissionAction::Mcp);
}

#[tokio::test]
async fn a_loaded_schema_survives_repeated_reads() {
    let set = large(generous());
    set.invoke(LOAD_TOOL_SCHEMA, r#"{"name":"mcp__github__tool_3"}"#)
        .await
        .unwrap();
    for _ in 0..3 {
        let definitions = set.definition();
        assert!(definitions.iter().any(|d| d.name == "mcp__github__tool_3"));
        assert!(set.contain("mcp__github__tool_3"));
    }
}

#[test]
fn the_index_stays_far_below_the_full_schema_cost() {
    let set = large(AdmissionPolicy::default());
    let full = set.schema_tokens();
    let index = match set.definition().first() {
        Some(definition) => {
            meerai_mcp::budget::estimate_tokens(&definition.description)
                + meerai_mcp::budget::value_tokens(&definition.parameters)
        }
        None => 0,
    };
    assert!(index * 4 < full, "index {index} vs full {full}");
}
