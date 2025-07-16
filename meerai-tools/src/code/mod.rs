mod diff;
mod edit_file;
mod glob;
mod grep;
mod list_dir;
mod paths;
mod read_file;
mod shell;
mod write_file;

use std::path::{Path, PathBuf};

use meerai_core::{
    EffectClass, ToolDefinition, ToolError, ToolObservation, ToolOutput, ToolRun, Toolset,
    async_trait,
};

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
}

impl CodeTools {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            writable_roots: Vec::new(),
            readable_roots: Vec::new(),
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

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn writable_roots(&self) -> &[PathBuf] {
        &self.writable_roots
    }

    pub fn readable_roots(&self) -> &[PathBuf] {
        &self.readable_roots
    }
}

fn runtime_name(tool: &str) -> String {
    format!("{TOOLSET_NAME}-{tool}")
}

fn bare_name(fn_name: &str) -> Option<&str> {
    fn_name.strip_prefix(TOOLSET_NAME)?.strip_prefix('-')
}

fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, ToolError> {
    serde_json::from_str(args).map_err(ToolError::WrongArguments)
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

pub(crate) fn truncate_middle(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let head_budget = max_bytes / 2;
    let tail_budget = max_bytes - head_budget;
    let head_end = floor_char_boundary(text, head_budget);
    let tail_start = ceil_char_boundary(text, text.len() - tail_budget);
    let omitted = tail_start.saturating_sub(head_end);

    format!(
        "{}\n... [{omitted} bytes omitted] ...\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[async_trait]
impl Toolset for CodeTools {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![
            read_file::definition(&runtime_name("read_file")),
            write_file::definition(&runtime_name("write_file")),
            edit_file::definition(&runtime_name("edit_file")),
            list_dir::definition(&runtime_name("list_dir")),
            grep::definition(&runtime_name("grep")),
            glob::definition(&runtime_name("glob")),
            shell::definition(&runtime_name("shell")),
        ]
    }

    fn contain(&self, fn_name: &str) -> bool {
        bare_name(fn_name).is_some_and(|tool| TOOL_NAMES.contains(&tool))
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        match bare_name(fn_name).unwrap_or(fn_name) {
            "read_file" | "list_dir" | "grep" | "glob" => EffectClass::Read,
            "write_file" | "edit_file" => EffectClass::Write,
            "shell" => EffectClass::Execute,
            _ => EffectClass::Read,
        }
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        let Some(tool) = bare_name(fn_name) else {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        };

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
