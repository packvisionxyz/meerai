use std::fs;
use std::sync::Arc;

use meerai_common::definitions::SkillDefinition;
use meerai_core::{EffectClass, ToolDefinition, ToolError, ToolOutput, async_trait};
use serde::Deserialize;

pub const SKILL_TOOL: &str = "skill-load";
pub const MAX_SKILL_TEXT_BYTES: usize = 16 * 1024;
const MAX_LISTED_FILES: usize = 50;
const MAX_LISTED_IDS: usize = 40;

pub struct SkillToolset {
    skills: Arc<Vec<SkillDefinition>>,
}

impl SkillToolset {
    pub fn new(skills: Arc<Vec<SkillDefinition>>) -> Self {
        Self { skills }
    }

    fn advertised(&self) -> impl Iterator<Item = &SkillDefinition> {
        self.skills.iter().filter(|skill| skill.model_invocable)
    }

    fn load(&self, args: SkillLoadArgs) -> ToolOutput {
        let id = args.id.trim();
        let Some(skill) = self.advertised().find(|skill| skill.id == id) else {
            let mut ids: Vec<&str> = self.advertised().map(|skill| skill.id.as_str()).collect();
            ids.sort_unstable();
            ids.truncate(MAX_LISTED_IDS);
            let list = if ids.is_empty() {
                "(none)".to_string()
            } else {
                ids.join(", ")
            };
            return ToolOutput::Fail(format!("unknown skill '{id}'; available skills: {list}"));
        };
        match args
            .file
            .as_deref()
            .map(str::trim)
            .filter(|file| !file.is_empty())
        {
            None => ToolOutput::Text(render_body(skill)),
            Some(file) => read_supporting_file(skill, file),
        }
    }
}

#[async_trait]
impl meerai_core::Toolset for SkillToolset {
    fn name(&self) -> String {
        "skill".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: SKILL_TOOL.to_string(),
            description: "Load a skill by id. Use it when a task matches a skill description in \
                          the Skills section. Without `file`, it returns the skill instructions \
                          and the supporting file names. With `file`, it returns that supporting \
                          file."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Skill id from the Skills section"
                    },
                    "file": {
                        "type": "string",
                        "description": "Optional supporting file name from the skill listing"
                    }
                },
                "required": ["id"]
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == SKILL_TOOL
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        if fn_name != SKILL_TOOL {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }
        let args: SkillLoadArgs = serde_json::from_str(args)?;
        Ok(self.load(args))
    }
}

#[derive(Debug, Deserialize)]
struct SkillLoadArgs {
    id: String,
    #[serde(default)]
    file: Option<String>,
}

fn render_body(skill: &SkillDefinition) -> String {
    let (body, truncated) = cap(&skill.body);
    let mut out = format!(
        "# skill: {}\n\nSource: {}\n\n{}",
        skill.id,
        skill.source.label(),
        body
    );
    if truncated {
        out.push_str("\n... [skill truncated]");
    }
    if !skill.supporting_files.is_empty() {
        out.push_str("\n\nSupporting files (load one with `skill-load` and `file`):\n");
        for file in skill.supporting_files.iter().take(MAX_LISTED_FILES) {
            out.push_str("- ");
            out.push_str(file);
            out.push('\n');
        }
    }
    out
}

fn read_supporting_file(skill: &SkillDefinition, file: &str) -> ToolOutput {
    if file.contains(['/', '\\']) || file == "." || file == ".." {
        return ToolOutput::Fail(format!(
            "'{file}' is not a supporting file name for skill '{}'",
            skill.id
        ));
    }
    if !skill.supporting_files.iter().any(|name| name == file) {
        return ToolOutput::Fail(format!(
            "skill '{}' has no supporting file '{file}'",
            skill.id
        ));
    }
    let path = skill.directory.join(file);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => ToolOutput::Fail(format!(
            "supporting file '{file}' is a symlink and is not read"
        )),
        Ok(meta) if meta.is_file() => match fs::read_to_string(&path) {
            Ok(text) => {
                let (text, truncated) = cap(&text);
                let mut out = format!("# {} (skill {})\n\n{}", file, skill.id, text);
                if truncated {
                    out.push_str("\n... [file truncated]");
                }
                ToolOutput::Text(out)
            }
            Err(error) => {
                ToolOutput::Fail(format!("could not read supporting file '{file}': {error}"))
            }
        },
        _ => ToolOutput::Fail(format!("supporting file '{file}' is not a regular file")),
    }
}

fn cap(text: &str) -> (&str, bool) {
    if text.len() <= MAX_SKILL_TEXT_BYTES {
        return (text, false);
    }
    let end = meerai_core::text::floor_char_boundary(text, MAX_SKILL_TEXT_BYTES);
    (&text[..end], true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use meerai_common::definitions::DefinitionSource;
    use meerai_core::Toolset;

    use super::*;

    fn skill(id: &str, directory: &Path, model_invocable: bool) -> SkillDefinition {
        SkillDefinition {
            id: id.to_string(),
            source: DefinitionSource::Agents,
            description: Some("Use for tests".to_string()),
            model_invocable,
            body: "Follow these steps.".to_string(),
            path: directory.join("SKILL.md"),
            directory: directory.to_path_buf(),
            token_estimate: 4,
            warnings: Vec::new(),
            supporting_files: vec!["notes.md".to_string()],
        }
    }

    fn toolset(dir: &Path, model_invocable: bool) -> SkillToolset {
        let skills = Arc::new(vec![skill("review", dir, model_invocable)]);
        SkillToolset::new(skills)
    }

    #[tokio::test]
    async fn loads_the_body_and_lists_supporting_files() {
        let dir = tempfile::tempdir().unwrap();
        let toolset = toolset(dir.path(), true);

        let output = toolset
            .invoke(SKILL_TOOL, r#"{"id":"review"}"#)
            .await
            .unwrap();

        let ToolOutput::Text(text) = output else {
            panic!("expected text output");
        };
        assert!(text.contains("# skill: review"));
        assert!(text.contains("Follow these steps."));
        assert!(text.contains("- notes.md"));
    }

    #[tokio::test]
    async fn unknown_id_lists_the_advertised_skills() {
        let dir = tempfile::tempdir().unwrap();
        let toolset = toolset(dir.path(), true);

        let output = toolset
            .invoke(SKILL_TOOL, r#"{"id":"missing"}"#)
            .await
            .unwrap();

        let ToolOutput::Fail(text) = output else {
            panic!("expected failure output");
        };
        assert!(text.contains("unknown skill 'missing'"));
        assert!(text.contains("review"));
    }

    #[tokio::test]
    async fn hidden_skills_are_not_loadable() {
        let dir = tempfile::tempdir().unwrap();
        let toolset = toolset(dir.path(), false);

        let output = toolset
            .invoke(SKILL_TOOL, r#"{"id":"review"}"#)
            .await
            .unwrap();

        assert!(matches!(output, ToolOutput::Fail(_)));
    }

    #[tokio::test]
    async fn rejects_path_separators_and_unknown_files() {
        let dir = tempfile::tempdir().unwrap();
        let toolset = toolset(dir.path(), true);

        for file in [
            "../SKILL.md",
            "/etc/passwd",
            "nested/notes.md",
            "missing.md",
        ] {
            let args = format!(r#"{{"id":"review","file":"{file}"}}"#);
            let output = toolset.invoke(SKILL_TOOL, &args).await.unwrap();
            assert!(
                matches!(output, ToolOutput::Fail(_)),
                "{file} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn reads_a_supporting_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.md"), "Reference text.").unwrap();
        let toolset = toolset(dir.path(), true);

        let output = toolset
            .invoke(SKILL_TOOL, r#"{"id":"review","file":"notes.md"}"#)
            .await
            .unwrap();

        let ToolOutput::Text(text) = output else {
            panic!("expected text output");
        };
        assert!(text.contains("Reference text."));
    }

    #[tokio::test]
    async fn caps_a_large_supporting_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("notes.md"),
            "x".repeat(MAX_SKILL_TEXT_BYTES + 1),
        )
        .unwrap();
        let toolset = toolset(dir.path(), true);

        let output = toolset
            .invoke(SKILL_TOOL, r#"{"id":"review","file":"notes.md"}"#)
            .await
            .unwrap();

        let ToolOutput::Text(text) = output else {
            panic!("expected text output");
        };
        assert!(text.contains("... [file truncated]"));
    }

    #[tokio::test]
    async fn rejects_a_symlinked_supporting_file() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.md"), "secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            dir.path().join("notes.md"),
        )
        .unwrap();
        let toolset = toolset(dir.path(), true);

        let output = toolset
            .invoke(SKILL_TOOL, r#"{"id":"review","file":"notes.md"}"#)
            .await
            .unwrap();

        let ToolOutput::Fail(text) = output else {
            panic!("expected failure output");
        };
        assert!(text.contains("symlink"));
    }
}
