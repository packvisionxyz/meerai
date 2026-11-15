use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const AGENTS_AGENTS_DIR: &str = ".agents/agents";
const AGENTS_SKILLS_DIR: &str = ".agents/skills";
const CLAUDE_AGENTS_DIR: &str = ".claude/agents";
const CLAUDE_SKILLS_DIR: &str = ".claude/skills";
const SKILL_FILE: &str = "SKILL.md";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionSource {
    Agents,
    Claude,
}

impl DefinitionSource {
    pub fn label(self) -> &'static str {
        match self {
            DefinitionSource::Agents => ".agents",
            DefinitionSource::Claude => ".claude",
        }
    }
}

pub fn validate_agent_model(model: Option<&str>) -> Result<Option<&str>, &str> {
    let Some(raw) = model.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if raw.contains(':') {
        return Err(raw);
    }
    Ok(Some(raw))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDefinition {
    pub id: String,
    pub source: DefinitionSource,
    pub description: Option<String>,
    pub model: Option<String>,
    pub steps: Option<usize>,
    pub mode: Option<String>,
    pub prompt: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDefinition {
    pub id: String,
    pub source: DefinitionSource,
    pub description: Option<String>,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct DefinitionCatalog {
    pub agents: Vec<AgentDefinition>,
    pub skills: Vec<SkillDefinition>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    steps: Option<usize>,
    mode: Option<String>,
    tools: Option<serde_yaml::Value>,
    permissions: Option<serde_yaml::Value>,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<serde_yaml::Value>,
}

struct ParsedFile {
    meta: Frontmatter,
    body: String,
}

pub fn load_definitions(project_root: &Path) -> DefinitionCatalog {
    let mut catalog = DefinitionCatalog::default();
    let mut agents: BTreeMap<String, AgentDefinition> = BTreeMap::new();
    let mut skills: BTreeMap<String, SkillDefinition> = BTreeMap::new();

    for source in [DefinitionSource::Agents, DefinitionSource::Claude] {
        for agent in load_agents(project_root, source, &mut catalog.warnings) {
            if let Some(kept) = agents.get(&agent.id) {
                catalog.warnings.push(duplicate_warning(
                    &agent.id,
                    source,
                    &agent.path,
                    kept.source,
                    &kept.path,
                ));
            } else {
                agents.insert(agent.id.clone(), agent);
            }
        }
        for skill in load_skills(project_root, source, &mut catalog.warnings) {
            if let Some(kept) = skills.get(&skill.id) {
                catalog.warnings.push(duplicate_warning(
                    &skill.id,
                    source,
                    &skill.path,
                    kept.source,
                    &kept.path,
                ));
            } else {
                skills.insert(skill.id.clone(), skill);
            }
        }
    }

    catalog.agents = agents.into_values().collect();
    catalog.skills = skills.into_values().collect();
    catalog
}

fn load_agents(
    project_root: &Path,
    source: DefinitionSource,
    warnings: &mut Vec<String>,
) -> Vec<AgentDefinition> {
    let dir_name = match source {
        DefinitionSource::Agents => AGENTS_AGENTS_DIR,
        DefinitionSource::Claude => CLAUDE_AGENTS_DIR,
    };
    let dir = project_root.join(dir_name);
    let mut agents = Vec::new();

    for path in markdown_files(&dir, warnings) {
        let Some(text) = read_text(&path, warnings) else {
            continue;
        };
        let Some(parsed) = parse_markdown(&text, &path, warnings) else {
            continue;
        };
        let fallback = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let Some(id) = definition_id(parsed.meta.name.as_deref(), fallback) else {
            warnings.push(format!("cannot determine id for {}", path.display()));
            continue;
        };
        agents.push(AgentDefinition {
            id,
            source,
            description: parsed.meta.description,
            model: parsed.meta.model,
            steps: parsed.meta.steps,
            mode: parsed.meta.mode,
            prompt: parsed.body,
            path,
        });
    }

    agents.sort_by(|a, b| a.id.cmp(&b.id));
    agents
}

fn load_skills(
    project_root: &Path,
    source: DefinitionSource,
    warnings: &mut Vec<String>,
) -> Vec<SkillDefinition> {
    let dir_name = match source {
        DefinitionSource::Agents => AGENTS_SKILLS_DIR,
        DefinitionSource::Claude => CLAUDE_SKILLS_DIR,
    };
    let dir = project_root.join(dir_name);
    let mut skills = Vec::new();

    for entry in dir_entries(&dir, warnings) {
        let skill_dir = entry.path();
        match fs::symlink_metadata(&skill_dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                warnings.push(format!("skipping symlink {}", skill_dir.display()));
                continue;
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            Err(err) => {
                warnings.push(format!("failed to inspect {}: {err}", skill_dir.display()));
                continue;
            }
        }

        let skill_path = skill_dir.join(SKILL_FILE);
        match fs::symlink_metadata(&skill_path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                warnings.push(format!("skipping symlink {}", skill_path.display()));
                continue;
            }
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(_) => continue,
        }

        let Some(text) = read_text(&skill_path, warnings) else {
            continue;
        };
        let Some(parsed) = parse_markdown(&text, &skill_path, warnings) else {
            continue;
        };
        let fallback = skill_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let Some(id) = definition_id(parsed.meta.name.as_deref(), fallback) else {
            warnings.push(format!("cannot determine id for {}", skill_path.display()));
            continue;
        };
        skills.push(SkillDefinition {
            id,
            source,
            description: parsed.meta.description,
            body: parsed.body,
            path: skill_path,
        });
    }

    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills
}

fn parse_markdown(text: &str, path: &Path, warnings: &mut Vec<String>) -> Option<ParsedFile> {
    let (frontmatter, body) = match split_frontmatter(text) {
        Ok(parts) => parts,
        Err(err) => {
            warnings.push(format!("invalid frontmatter in {}: {err}", path.display()));
            return None;
        }
    };

    let meta = match frontmatter {
        None => Frontmatter::default(),
        Some(raw) if raw.trim().is_empty() => Frontmatter::default(),
        Some(raw) => match serde_yaml::from_str::<Frontmatter>(raw) {
            Ok(meta) => meta,
            Err(err) => {
                warnings.push(format!("invalid frontmatter in {}: {err}", path.display()));
                return None;
            }
        },
    };

    if meta.tools.is_some() || meta.permissions.is_some() || meta.allowed_tools.is_some() {
        warnings.push(format!(
            "{}: agent tool/permission fields are not applied yet",
            path.display()
        ));
    }

    Some(ParsedFile {
        meta,
        body: body.trim().to_string(),
    })
}

fn split_frontmatter(text: &str) -> Result<(Option<&str>, &str), &'static str> {
    let Some(rest) = after_open_delimiter(text) else {
        return Ok((None, text));
    };

    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.trim_end() == "---" {
            return Ok((Some(&rest[..offset]), &rest[offset + line.len()..]));
        }
        offset += line.len();
    }

    Err("missing closing --- delimiter")
}

fn after_open_delimiter(text: &str) -> Option<&str> {
    let (first, rest) = match text.find('\n') {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    if first.trim_end() == "---" {
        Some(rest)
    } else {
        None
    }
}

fn definition_id(name: Option<&str>, fallback: &str) -> Option<String> {
    let id = match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name,
        None => fallback.trim(),
    };
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

fn markdown_files(dir: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in dir_entries(dir, warnings) {
        let path = entry.path();
        let is_markdown = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_markdown {
            continue;
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                warnings.push(format!("skipping symlink {}", path.display()));
            }
            Ok(meta) if meta.is_file() => files.push(path),
            Ok(_) => {}
            Err(err) => warnings.push(format!("failed to inspect {}: {err}", path.display())),
        }
    }
    files.sort();
    files
}

fn dir_entries(dir: &Path, warnings: &mut Vec<String>) -> Vec<fs::DirEntry> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            warnings.push(format!("failed to read {}: {err}", dir.display()));
            return Vec::new();
        }
    };

    let mut collected = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => collected.push(entry),
            Err(err) => warnings.push(format!("failed to read {}: {err}", dir.display())),
        }
    }
    collected.sort_by_key(|entry| entry.file_name());
    collected
}

fn read_text(path: &Path, warnings: &mut Vec<String>) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) => {
            warnings.push(format!("failed to read {}: {err}", path.display()));
            None
        }
    }
}

fn duplicate_warning(
    id: &str,
    incoming_source: DefinitionSource,
    incoming_path: &Path,
    kept_source: DefinitionSource,
    kept_path: &Path,
) -> String {
    format!(
        "duplicate definition id {id} from {} ({}) ignored; kept {} ({})",
        incoming_source.label(),
        incoming_path.display(),
        kept_source.label(),
        kept_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "meerai_definitions_{name}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    stack.push(path);
                } else {
                    files.push((path.clone(), fs::read(&path).unwrap()));
                }
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    #[test]
    fn parses_agent_frontmatter_and_preserves_body() {
        let dir = temp_dir("agent_frontmatter");
        write_file(
            &dir.join(".agents/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews changes for correctness\nmodel: openrouter:deepseek-v4.1#high\nsteps: 8\nmode: subagent\n---\nReview the changes.\n\nList findings by severity.\n",
        );

        let catalog = load_definitions(&dir);

        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.agents.len(), 1);
        let agent = &catalog.agents[0];
        assert_eq!(agent.id, "reviewer");
        assert_eq!(agent.source, DefinitionSource::Agents);
        assert_eq!(
            agent.description.as_deref(),
            Some("Reviews changes for correctness")
        );
        assert_eq!(
            agent.model.as_deref(),
            Some("openrouter:deepseek-v4.1#high")
        );
        assert_eq!(agent.steps, Some(8));
        assert_eq!(agent.mode.as_deref(), Some("subagent"));
        assert_eq!(
            agent.prompt,
            "Review the changes.\n\nList findings by severity."
        );
        assert_eq!(agent.path, dir.join(".agents/agents/reviewer.md"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_agents_skill_with_frontmatter() {
        let dir = temp_dir("agents_skill");
        write_file(
            &dir.join(".agents/skills/investigate/SKILL.md"),
            "---\nname: investigate\ndescription: Investigate a codebase\nlicense: MIT\nmetadata:\n  version: \"1.2.0\"\n---\n\n# Explore\n\nRead-only reconnaissance.\n",
        );

        let catalog = load_definitions(&dir);

        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.skills.len(), 1);
        let skill = &catalog.skills[0];
        assert_eq!(skill.id, "investigate");
        assert_eq!(skill.source, DefinitionSource::Agents);
        assert_eq!(skill.description.as_deref(), Some("Investigate a codebase"));
        assert_eq!(skill.body, "# Explore\n\nRead-only reconnaissance.");
        assert_eq!(skill.path, dir.join(".agents/skills/investigate/SKILL.md"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_claude_agent_and_skill() {
        let dir = temp_dir("claude_definitions");
        write_file(
            &dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews changes\nmodel: openrouter:fast\n---\nReview carefully.\n",
        );
        write_file(
            &dir.join(".claude/skills/fix/SKILL.md"),
            "---\ndescription: Fixes failures\n---\nFix the failing test.\n",
        );

        let catalog = load_definitions(&dir);

        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].source, DefinitionSource::Claude);
        assert_eq!(catalog.agents[0].model.as_deref(), Some("openrouter:fast"));
        assert_eq!(catalog.agents[0].prompt, "Review carefully.");
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].id, "fix");
        assert_eq!(catalog.skills[0].source, DefinitionSource::Claude);
        assert_eq!(catalog.skills[0].body, "Fix the failing test.");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_wins_duplicate_id_over_claude() {
        let dir = temp_dir("agents_precedence");
        write_file(
            &dir.join(".agents/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: From agents\n---\nAgents prompt.\n",
        );
        write_file(
            &dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: From claude\n---\nClaude prompt.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].source, DefinitionSource::Agents);
        assert_eq!(
            catalog.agents[0].description.as_deref(),
            Some("From agents")
        );
        assert_eq!(catalog.agents[0].prompt, "Agents prompt.");
        assert_eq!(catalog.warnings.len(), 1);
        let warning = &catalog.warnings[0];
        assert!(warning.contains("duplicate definition id reviewer"));
        assert!(warning.contains(".claude"));
        assert!(warning.contains(".agents"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_skill_wins_duplicate_id_over_claude_skill() {
        let dir = temp_dir("agents_skill_precedence");
        write_file(
            &dir.join(".agents/skills/deploy/SKILL.md"),
            "---\nname: deploy\n---\nAgents body.\n",
        );
        write_file(
            &dir.join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\n---\nClaude body.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].source, DefinitionSource::Agents);
        assert_eq!(catalog.skills[0].body, "Agents body.");
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("duplicate definition id deploy"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_is_sorted_by_id_across_sources() {
        let dir = temp_dir("sorted_ids");
        write_file(
            &dir.join(".claude/agents/zeta.md"),
            "---\nname: zeta\n---\nZeta.\n",
        );
        write_file(
            &dir.join(".claude/agents/alpha.md"),
            "---\nname: alpha\n---\nAlpha.\n",
        );
        write_file(
            &dir.join(".agents/agents/gamma.md"),
            "---\nname: gamma\n---\nGamma.\n",
        );
        write_file(
            &dir.join(".agents/agents/beta.md"),
            "---\nname: beta\n---\nBeta.\n",
        );

        let catalog = load_definitions(&dir);
        let ids: Vec<&str> = catalog
            .agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect();
        assert_eq!(ids, ["alpha", "beta", "gamma", "zeta"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_frontmatter_uses_file_and_directory_names() {
        let dir = temp_dir("missing_frontmatter");
        write_file(
            &dir.join(".claude/agents/helper.md"),
            "# Helper\n\nAnswer questions.\n",
        );
        write_file(
            &dir.join(".agents/skills/planner/SKILL.md"),
            "# Planner\n\nBuild a plan.\n",
        );

        let catalog = load_definitions(&dir);

        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].id, "helper");
        assert_eq!(catalog.agents[0].description, None);
        assert_eq!(catalog.agents[0].prompt, "# Helper\n\nAnswer questions.");
        assert_eq!(catalog.skills[0].id, "planner");
        assert_eq!(catalog.skills[0].body, "# Planner\n\nBuild a plan.");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_frontmatter_warns_and_skips() {
        let dir = temp_dir("invalid_frontmatter");
        write_file(
            &dir.join(".claude/agents/unterminated.md"),
            "---\nname: broken\n\nNo closing delimiter.\n",
        );
        write_file(
            &dir.join(".agents/agents/bad_steps.md"),
            "---\nname: bad_steps\nsteps: [1, 2]\n---\nBody.\n",
        );
        write_file(
            &dir.join(".agents/agents/valid.md"),
            "---\nname: valid\n---\nBody.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].id, "valid");
        assert_eq!(catalog.warnings.len(), 2);
        assert!(
            catalog
                .warnings
                .iter()
                .all(|warning| warning.contains("invalid frontmatter"))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tool_and_permission_fields_warn_but_still_load() {
        let dir = temp_dir("tool_fields");
        write_file(
            &dir.join(".claude/agents/locked.md"),
            "---\nname: locked\ntools: [read_file]\npermissions:\n  - { action: edit, resource: \"*\", effect: deny }\nallowed-tools: shell\n---\nRead only.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].prompt, "Read only.");
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("not applied yet"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn absent_directories_are_silent() {
        let dir = temp_dir("absent_dirs");
        fs::create_dir_all(&dir).unwrap();

        let catalog = load_definitions(&dir);

        assert!(catalog.agents.is_empty());
        assert!(catalog.skills.is_empty());
        assert!(catalog.warnings.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn meerai_definition_directories_are_ignored() {
        let dir = temp_dir("meerai_ignored");
        write_file(
            &dir.join(".meerai/agents/reviewer.md"),
            "---\nname: reviewer\n---\nNative prompt.\n",
        );
        write_file(
            &dir.join(".meerai/skills/fix/SKILL.md"),
            "---\nname: fix\n---\nNative body.\n",
        );

        let catalog = load_definitions(&dir);

        assert!(catalog.agents.is_empty());
        assert!(catalog.skills.is_empty());
        assert!(catalog.warnings.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_writes_nothing() {
        let dir = temp_dir("read_only");
        write_file(
            &dir.join(".agents/skills/explore/SKILL.md"),
            "---\nname: explore\ndescription: Search the codebase\n---\nExplore.\n",
        );
        write_file(
            &dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\n---\nReview.\n",
        );
        let before = snapshot(&dir);

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(snapshot(&dir), before);
        assert!(!dir.join(".meerai").exists());
        assert!(!dir.join(".meerai/imports.lock").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
