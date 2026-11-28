use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolObservation, ToolOutput,
    ToolPermissionSpec, ToolRun, Toolset, async_trait,
};

use crate::budget::{
    AdmissionMode, DEFAULT_FULL_BUDGET_TOKENS, DEFAULT_MAX_LOADED_TOKENS, DEFAULT_MAX_LOADED_TOOLS,
    render_index, select_mode, tool_tokens, value_tokens,
};
use crate::tool::McpTool;

pub const LOAD_TOOL_SCHEMA: &str = "load_tool_schema";

#[async_trait]
pub trait ToolCaller: Send + Sync {
    fn server_name(&self) -> &str;

    async fn call(&self, tool: &McpTool, args: &str) -> Result<ToolObservation, String>;
}

#[derive(Clone, Copy, Debug)]
pub struct AdmissionPolicy {
    pub full_budget_tokens: usize,
    pub max_loaded_tools: usize,
    pub max_loaded_tokens: usize,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self {
            full_budget_tokens: DEFAULT_FULL_BUDGET_TOKENS,
            max_loaded_tools: DEFAULT_MAX_LOADED_TOOLS,
            max_loaded_tokens: DEFAULT_MAX_LOADED_TOKENS,
        }
    }
}

#[derive(Debug, Default)]
struct LoadedSchemas {
    names: BTreeSet<String>,
    tokens: usize,
}

pub struct McpToolset {
    caller: Arc<dyn ToolCaller>,
    tools: Arc<Vec<McpTool>>,
    call_timeout: std::time::Duration,
    mode: AdmissionMode,
    policy: AdmissionPolicy,
    loaded: RwLock<LoadedSchemas>,
}

impl McpToolset {
    pub fn new(caller: Arc<dyn ToolCaller>, tools: Vec<McpTool>, timeout_ms: u64) -> Self {
        Self::with_policy(caller, tools, timeout_ms, AdmissionPolicy::default())
    }

    pub fn with_policy(
        caller: Arc<dyn ToolCaller>,
        tools: Vec<McpTool>,
        timeout_ms: u64,
        policy: AdmissionPolicy,
    ) -> Self {
        let mode = select_mode(
            tools.iter().map(tool_tokens).sum(),
            policy.full_budget_tokens,
        );
        Self {
            caller,
            tools: Arc::new(tools),
            call_timeout: std::time::Duration::from_millis(timeout_ms.max(1)),
            mode,
            policy,
            loaded: RwLock::new(LoadedSchemas::default()),
        }
    }

    pub fn server_name(&self) -> &str {
        self.caller.server_name()
    }

    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    pub fn mode(&self) -> AdmissionMode {
        self.mode
    }

    pub fn loaded_tools(&self) -> Vec<String> {
        self.read_loaded().names.iter().cloned().collect()
    }

    pub fn loaded_tokens(&self) -> usize {
        self.read_loaded().tokens
    }

    fn read_loaded(&self) -> std::sync::RwLockReadGuard<'_, LoadedSchemas> {
        self.loaded
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_loaded(&self) -> std::sync::RwLockWriteGuard<'_, LoadedSchemas> {
        self.loaded
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn tool(&self, fn_name: &str) -> Option<&McpTool> {
        self.tools
            .iter()
            .find(|tool| tool.runtime_name() == fn_name)
    }

    fn is_loaded(&self, fn_name: &str) -> bool {
        self.read_loaded().names.contains(fn_name)
    }

    fn load_schema(&self, requested: &str) -> ToolObservation {
        let Some(tool) = self.tool(requested) else {
            return ToolObservation::error(format!(
                "server '{}' has no tool '{requested}'",
                self.caller.server_name()
            ));
        };
        let name = tool.runtime_name();
        {
            let loaded = self.read_loaded();
            if loaded.names.contains(&name) {
                return ToolObservation::success(format!(
                    "Schema for {name} is already loaded. Call it directly."
                ));
            }
            if loaded.names.len() >= self.policy.max_loaded_tools {
                return ToolObservation::error(format!(
                    "cannot load {name}: {} schemas are already loaded (limit {}). Call a loaded tool first or start a new turn.",
                    loaded.names.len(),
                    self.policy.max_loaded_tools
                ));
            }
        }

        let cost = tool_tokens(tool);
        let mut loaded = self.write_loaded();
        if loaded.tokens + cost > self.policy.max_loaded_tokens {
            return ToolObservation::error(format!(
                "cannot load {name}: it needs about {cost} tokens and {} of {} are already used",
                loaded.tokens, self.policy.max_loaded_tokens
            ));
        }
        loaded.tokens += cost;
        loaded.names.insert(name.clone());
        ToolObservation::success(format!(
            "Loaded {name}. Its parameters are now available; call it directly.\n\n{}",
            serde_json::json!({
                "name": name,
                "description": tool.description,
                "parameters": tool.schema,
            })
        ))
    }

    fn definitions_full(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| ToolDefinition {
                r#type: "function".to_string(),
                name: tool.runtime_name(),
                description: tool.description.clone(),
                parameters: tool.schema.clone(),
            })
            .collect()
    }

    fn index_definition(&self) -> ToolDefinition {
        let indexed: Vec<&McpTool> = self
            .tools
            .iter()
            .filter(|tool| !self.is_loaded(&tool.runtime_name()))
            .collect();
        let listing = indexed
            .iter()
            .map(|tool| crate::budget::index_entry(tool))
            .collect::<Vec<_>>()
            .join("\n");
        let server = self.caller.server_name();
        let description = format!(
            "Load the parameter schema for one tool from MCP server '{server}' before calling it.\n\nAvailable tools:\n{listing}"
        );
        ToolDefinition {
            r#type: "function".to_string(),
            name: LOAD_TOOL_SCHEMA.to_string(),
            description,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Exact tool name from the list, for example mcp__github__search_issues."
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        }
    }

    fn loaded_definitions(&self) -> Vec<ToolDefinition> {
        let loaded = self.read_loaded();
        self.tools
            .iter()
            .filter(|tool| loaded.names.contains(&tool.runtime_name()))
            .map(|tool| ToolDefinition {
                r#type: "function".to_string(),
                name: tool.runtime_name(),
                description: tool.description.clone(),
                parameters: tool.schema.clone(),
            })
            .collect()
    }

    async fn dispatch(&self, fn_name: &str, args: &str) -> ToolObservation {
        let Some(tool) = self.tool(fn_name) else {
            return ToolObservation::error(format!(
                "server '{}' has no tool '{fn_name}'",
                self.caller.server_name()
            ));
        };
        if self.mode.is_indexed() && !self.is_loaded(fn_name) {
            return ToolObservation::error(format!(
                "the schema for {fn_name} is not loaded. Call {LOAD_TOOL_SCHEMA} with {{\"name\":\"{fn_name}\"}} first."
            ));
        }
        match tokio::time::timeout(self.call_timeout, self.caller.call(tool, args)).await {
            Ok(Ok(observation)) => observation,
            Ok(Err(error)) => ToolObservation::error(error),
            Err(_) => ToolObservation::error(format!(
                "mcp tool '{fn_name}' timed out after {}ms",
                self.call_timeout.as_millis()
            )),
        }
    }
}

#[async_trait]
impl Toolset for McpToolset {
    fn name(&self) -> String {
        format!("mcp-{}", self.caller.server_name())
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        if !self.mode.is_indexed() {
            return self.definitions_full();
        }
        let mut definitions = vec![self.index_definition()];
        definitions.extend(self.loaded_definitions());
        definitions
    }

    fn contain(&self, fn_name: &str) -> bool {
        if !self.mode.is_indexed() {
            return self.tool(fn_name).is_some();
        }
        if fn_name == LOAD_TOOL_SCHEMA {
            return true;
        }
        self.tool(fn_name).is_some() && self.is_loaded(fn_name)
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        if fn_name == LOAD_TOOL_SCHEMA {
            return EffectClass::Read;
        }
        match self.tool(fn_name) {
            Some(tool) if tool.read_only => EffectClass::Read,
            _ => EffectClass::External,
        }
    }

    fn permission_spec(&self, _fn_name: &str) -> Option<ToolPermissionSpec> {
        Some(ToolPermissionSpec {
            action: PermissionAction::Mcp,
            resource_arg: None,
        })
    }

    fn permission_resource(&self, fn_name: &str, _args: &str) -> Option<String> {
        if fn_name == LOAD_TOOL_SCHEMA {
            return Some(format!(
                "{}/{}",
                self.caller.server_name(),
                LOAD_TOOL_SCHEMA
            ));
        }
        let tool = self.tool(fn_name)?;
        Some(format!("{}/{}", tool.server, tool.name))
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        true
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        if fn_name == LOAD_TOOL_SCHEMA {
            return Ok(ToolOutput::from(self.load_from_args(args)));
        }
        Ok(ToolOutput::from(self.dispatch(fn_name, args).await))
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        let observation = if fn_name == LOAD_TOOL_SCHEMA {
            self.load_from_args(args)
        } else {
            self.dispatch(fn_name, args).await
        };
        Ok(ToolRun {
            output: ToolOutput::from(observation.clone()),
            observation,
            file_changes: Vec::new(),
        })
    }
}

impl McpToolset {
    fn load_from_args(&self, args: &str) -> ToolObservation {
        let parsed: serde_json::Value = match serde_json::from_str(args) {
            Ok(value) => value,
            Err(error) => {
                return ToolObservation::error(format!("invalid arguments: {error}"));
            }
        };
        let Some(name) = parsed.get("name").and_then(|value| value.as_str()) else {
            return ToolObservation::error("missing required string field 'name'");
        };
        self.load_schema(name)
    }

    pub fn index_preview(&self) -> String {
        let tools: Vec<McpTool> = self
            .tools
            .iter()
            .filter(|tool| !self.is_loaded(&tool.runtime_name()))
            .cloned()
            .collect();
        render_index(&tools, self.policy.full_budget_tokens)
    }

    pub fn schema_tokens(&self) -> usize {
        self.tools.iter().map(tool_tokens).sum()
    }

    pub fn loaded_schema_tokens(&self) -> usize {
        self.read_loaded().tokens
    }

    pub fn index_tokens(&self) -> usize {
        value_tokens(&serde_json::to_value(self.index_definition().parameters).unwrap_or_default())
    }
}

impl std::fmt::Debug for McpToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolset")
            .field("server", &self.caller.server_name())
            .field("tools", &self.tools.len())
            .field("mode", &self.mode)
            .field("loaded", &self.read_loaded().names.len())
            .finish()
    }
}
