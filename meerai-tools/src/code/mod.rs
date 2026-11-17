mod diff;
mod edit_file;
mod glob;
mod grep;
mod list_dir;
pub(crate) mod paths;
mod read_file;
pub(crate) mod shell;
mod write_file;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) use meerai_core::text::truncate_middle;
use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolObservation, ToolOutput,
    ToolPermissionSpec, ToolRun, Toolset, async_trait,
};

use crate::world::{ExecutionWorld, HostWorld};

pub const TOOL_NAMES: [&str; 7] = [
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "grep",
    "glob",
    "shell",
];

const TOOLSET_NAME: &str = "code";

#[derive(Clone, Debug)]
pub struct CodeTools {
    workspace_root: PathBuf,
    writable_roots: Vec<PathBuf>,
    readable_roots: Vec<PathBuf>,
    sensitive_env_vars: Vec<String>,
    allowed_tools: Option<Vec<&'static str>>,
    allow_sensitive_files: bool,
    extra_roots: Arc<std::sync::RwLock<Vec<PathBuf>>>,
    execution_world: Arc<dyn ExecutionWorld>,
    require_sandbox: bool,
    strict_env: bool,
}

impl CodeTools {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            writable_roots: Vec::new(),
            readable_roots: Vec::new(),
            sensitive_env_vars: Vec::new(),
            allowed_tools: None,
            allow_sensitive_files: false,
            extra_roots: Arc::new(std::sync::RwLock::new(Vec::new())),
            execution_world: Arc::new(HostWorld::new()),
            require_sandbox: false,
            strict_env: false,
        }
    }

    pub fn with_writable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.writable_roots = roots;
        self
    }

    pub fn with_readable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.readable_roots = roots;
        self
    }

    pub fn with_sensitive_env_vars(mut self, vars: Vec<String>) -> Self {
        self.sensitive_env_vars = vars
            .into_iter()
            .map(|v| v.trim().to_ascii_uppercase())
            .filter(|v| !v.is_empty())
            .collect();
        self
    }

    pub fn with_allowed_tools(mut self, allowed: impl IntoIterator<Item = &'static str>) -> Self {
        self.allowed_tools = Some(allowed.into_iter().collect());
        self
    }

    pub fn with_allow_sensitive_files(mut self, allow: bool) -> Self {
        self.allow_sensitive_files = allow;
        self
    }

    pub fn with_execution_world(mut self, world: Arc<dyn ExecutionWorld>) -> Self {
        self.execution_world = world;
        self
    }

    pub fn with_require_sandbox(mut self, require: bool) -> Self {
        self.require_sandbox = require;
        self
    }

    pub fn require_sandbox(&self) -> bool {
        self.require_sandbox
    }

    pub fn with_strict_env(mut self, strict: bool) -> Self {
        self.strict_env = strict;
        self
    }

    pub fn strict_env(&self) -> bool {
        self.strict_env
    }

    fn is_allowed(&self, tool: &str) -> bool {
        self.allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&tool))
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn writable_roots(&self) -> &[PathBuf] {
        &self.writable_roots
    }

    pub fn readable_roots(&self) -> &[PathBuf] {
        &self.readable_roots
    }

    pub fn extra_roots(&self) -> Vec<PathBuf> {
        self.extra_roots
            .read()
            .map(|roots| roots.clone())
            .unwrap_or_default()
    }

    pub fn add_extra_root(&self, root: &std::path::Path) {
        if let Ok(mut roots) = self.extra_roots.write()
            && !roots.iter().any(|existing| existing == root)
        {
            roots.push(root.to_path_buf());
        }
    }

    pub fn allow_sensitive_files(&self) -> bool {
        self.allow_sensitive_files
    }

    pub fn execution_world(&self) -> &Arc<dyn ExecutionWorld> {
        &self.execution_world
    }

    pub(crate) fn sensitive_env_vars(&self) -> &[String] {
        &self.sensitive_env_vars
    }
}

fn runtime_name(tool: &str) -> String {
    format!("{TOOLSET_NAME}-{tool}")
}

fn bare_name(fn_name: &str) -> Option<&str> {
    fn_name.strip_prefix(TOOLSET_NAME)?.strip_prefix('-')
}

pub(crate) fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, ToolError> {
    let value = match serde_json::from_str::<serde_json::Value>(args) {
        Ok(value) => value,
        Err(err) => meerai_core::lenient::parse_tool_arguments(args)
            .ok_or(ToolError::WrongArguments(err))?,
    };
    let value = meerai_core::lenient::normalize_tool_arguments(&value);
    serde_json::from_value(value).map_err(ToolError::WrongArguments)
}

pub(crate) fn definition_for<T: schemars::JsonSchema>(
    name: &str,
    description: &str,
) -> ToolDefinition {
    let settings = schemars::generate::SchemaSettings::draft07().with(|settings| {
        settings.meta_schema = None;
    });
    let mut generator = schemars::generate::SchemaGenerator::new(settings);
    let schema = generator.root_schema_for::<T>();
    let parameters = serde_json::to_value(schema)
        .unwrap_or_else(|_| serde_json::json!({ "type": "object", "properties": {} }));

    ToolDefinition {
        r#type: "function".to_string(),
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

fn run_from_observation(observation: ToolObservation) -> ToolRun {
    let output = ToolOutput::from(observation.clone());
    ToolRun {
        output,
        observation,
        file_changes: Vec::new(),
    }
}

pub(crate) fn success_run(stdout: String) -> ToolRun {
    run_from_observation(ToolObservation::success(stdout))
}

pub(crate) fn failure_run(message: String) -> ToolRun {
    run_from_observation(ToolObservation::error(message))
}

pub(crate) fn refused_run(reason: String) -> ToolRun {
    run_from_observation(ToolObservation::refused(reason))
}

#[async_trait]
impl Toolset for CodeTools {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        let mut defs = Vec::new();
        if self.is_allowed("read_file") {
            defs.push(read_file::definition(&runtime_name("read_file")));
        }
        if self.is_allowed("write_file") {
            defs.push(write_file::definition(&runtime_name("write_file")));
        }
        if self.is_allowed("edit_file") {
            defs.push(edit_file::definition(&runtime_name("edit_file")));
        }
        if self.is_allowed("list_dir") {
            defs.push(list_dir::definition(&runtime_name("list_dir")));
        }
        if self.is_allowed("grep") {
            defs.push(grep::definition(&runtime_name("grep")));
        }
        if self.is_allowed("glob") {
            defs.push(glob::definition(&runtime_name("glob")));
        }
        if self.is_allowed("shell") {
            defs.push(shell::definition(&runtime_name("shell")));
        }
        defs
    }

    fn contain(&self, fn_name: &str) -> bool {
        bare_name(fn_name).is_some_and(|tool| TOOL_NAMES.contains(&tool) && self.is_allowed(tool))
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        match bare_name(fn_name).unwrap_or(fn_name) {
            "read_file" | "list_dir" | "grep" | "glob" => EffectClass::Read,
            "write_file" | "edit_file" => EffectClass::Write,
            "shell" => EffectClass::Execute,
            _ => EffectClass::Read,
        }
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        match bare_name(fn_name)? {
            "read_file" | "list_dir" | "grep" | "glob" => Some(ToolPermissionSpec {
                action: PermissionAction::Read,
                resource_arg: Some("path"),
            }),
            "write_file" | "edit_file" => Some(ToolPermissionSpec {
                action: PermissionAction::Edit,
                resource_arg: Some("path"),
            }),
            "shell" => Some(ToolPermissionSpec {
                action: PermissionAction::Shell,
                resource_arg: Some("command"),
            }),
            _ => None,
        }
    }

    fn should_spill_output(&self, fn_name: &str) -> bool {
        bare_name(fn_name) != Some("read_file")
    }

    fn grant_path_root(&self, root: &std::path::Path) {
        self.add_extra_root(root);
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        let Some(tool) = bare_name(fn_name) else {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        };
        if !self.is_allowed(tool) {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }

        match tool {
            "read_file" => Ok(self.read_file(parse_args(args)?).await),
            "write_file" => Ok(self.write_file(parse_args(args)?).await),
            "edit_file" => Ok(self.edit_file(parse_args(args)?).await),
            "list_dir" => Ok(self.list_dir(parse_args(args)?).await),
            "grep" => Ok(self.grep(parse_args(args)?).await),
            "glob" => Ok(self.glob(parse_args(args)?).await),
            "shell" => Ok(self.shell(parse_args(args)?).await),
            _ => Err(ToolError::InvalidFunctionName(fn_name.to_string())),
        }
    }
}
