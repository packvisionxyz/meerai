use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use meerai_policy::{Effect, PermissionRule};
use serde::Deserialize;

use crate::packs::{PACK_AGENTS_DIR, PACK_SKILLS_DIR, load_pack};

const MEERAI_AGENTS_DIR: &str = ".meerai/agents";
const MEERAI_SKILLS_DIR: &str = ".meerai/skills";
const AGENTS_AGENTS_DIR: &str = ".agents/agents";
const AGENTS_SKILLS_DIR: &str = ".agents/skills";
const CLAUDE_AGENTS_DIR: &str = ".claude/agents";
const CLAUDE_SKILLS_DIR: &str = ".claude/skills";
const USER_AGENTS_DIR: &str = "agents";
const USER_SKILLS_DIR: &str = "skills";
const SKILL_FILE: &str = "SKILL.md";
const MAX_SUPPORTING_FILES: usize = 32;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionSource {
    User,
    Pack(String),
    Meerai,
    Agents,
    Claude,
}

impl DefinitionSource {
    pub fn label(&self) -> String {
        match self {
            Self::User => "user".to_string(),
            Self::Pack(id) => format!("pack:{id}"),
            Self::Meerai => ".meerai".to_string(),
            Self::Agents => ".agents".to_string(),
            Self::Claude => ".claude".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentMode {
    #[default]
    All,
    Primary,
    Subagent,
}

impl AgentMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "all" => Some(Self::All),
            "primary" => Some(Self::Primary),
            "subagent" => Some(Self::Subagent),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Primary => "primary",
            Self::Subagent => "subagent",
        }
    }

    pub fn selectable_primary(self) -> bool {
        matches!(self, Self::All | Self::Primary)
    }

    pub fn runnable_as_subagent(self) -> bool {
        matches!(self, Self::All | Self::Subagent)
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
    pub mode: AgentMode,
    pub steps: Option<usize>,
    pub permissions: Vec<PermissionRule>,
    pub prompt: String,
    pub path: PathBuf,
    pub token_estimate: u64,
    pub warnings: Vec<String>,
    pub supporting_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDefinition {
    pub id: String,
    pub source: DefinitionSource,
    pub description: Option<String>,
    pub model_invocable: bool,
    pub body: String,
    pub path: PathBuf,
    pub directory: PathBuf,
    pub token_estimate: u64,
    pub warnings: Vec<String>,
    pub supporting_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowedDefinition {
    pub id: String,
    pub kind: &'static str,
    pub source: DefinitionSource,
    pub path: PathBuf,
    pub kept_source: DefinitionSource,
    pub kept_path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct DefinitionCatalog {
    pub agents: Vec<AgentDefinition>,
    pub skills: Vec<SkillDefinition>,
    pub warnings: Vec<String>,
    pub shadows: Vec<ShadowedDefinition>,
}

impl DefinitionCatalog {
    pub fn advertised_skills(&self) -> impl Iterator<Item = &SkillDefinition> {
        self.skills.iter().filter(|skill| skill.model_invocable)
    }
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
    #[serde(rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
}

struct ParsedFile {
    meta: Frontmatter,
    body: String,
}

pub fn load_definitions(project_root: &Path) -> DefinitionCatalog {
    load_definitions_with_sources(project_root, None, &[])
}

pub fn load_definitions_for(
    store: &meerai_store::Store,
    config_dir: Option<&Path>,
    project_root: &Path,
) -> DefinitionCatalog {
    let pack_roots = crate::packs::trusted_pack_roots(store);
    load_definitions_with_sources(project_root, config_dir, &pack_roots)
}

pub fn load_definitions_with_sources(
    project_root: &Path,
    user_root: Option<&Path>,
    pack_roots: &[PathBuf],
) -> DefinitionCatalog {
    let mut catalog = DefinitionCatalog::default();
    let mut agents: BTreeMap<String, AgentDefinition> = BTreeMap::new();
    let mut skills: BTreeMap<String, SkillDefinition> = BTreeMap::new();

    if let Some(user_root) = user_root {
        let source = DefinitionSource::User;
        for agent in load_agents_in_dir(
            &user_root.join(USER_AGENTS_DIR),
            source.clone(),
            &mut catalog.warnings,
        ) {
            insert_agent(&mut catalog, &mut agents, agent);
        }
        for skill in load_skills_in_dir(
            &user_root.join(USER_SKILLS_DIR),
            source,
            &mut catalog.warnings,
        ) {
            insert_skill(&mut catalog, &mut skills, skill);
        }
    }

    for pack_root in pack_roots {
        let pack = match load_pack(pack_root) {
            Ok(pack) => pack,
            Err(error) => {
                catalog.warnings.push(format!("{error:#}"));
                continue;
            }
        };
        let source = DefinitionSource::Pack(pack.id().to_string());
        let (agent_paths, mut warnings) = if pack.manifest.agents.is_empty() {
            (
                markdown_files(&pack.root.join(PACK_AGENTS_DIR), &mut Vec::new()),
                Vec::new(),
            )
        } else {
            pack.agent_paths()
        };
        catalog.warnings.append(&mut warnings);
        for path in agent_paths {
            if let Some(agent) = parse_agent_file(&path, source.clone(), &mut catalog.warnings) {
                insert_agent(&mut catalog, &mut agents, agent);
            }
        }
        let (skill_paths, mut warnings) = if pack.manifest.skills.is_empty() {
            (
                skill_files(&pack.root.join(PACK_SKILLS_DIR), &mut Vec::new()),
                Vec::new(),
            )
        } else {
            pack.skill_paths()
        };
        catalog.warnings.append(&mut warnings);
        for path in skill_paths {
            if let Some(skill) = parse_skill_file(&path, source.clone(), &mut catalog.warnings) {
                insert_skill(&mut catalog, &mut skills, skill);
            }
        }
    }

    for source in [
        DefinitionSource::Meerai,
        DefinitionSource::Agents,
        DefinitionSource::Claude,
    ] {
        let agents_dir = match source {
            DefinitionSource::Meerai => MEERAI_AGENTS_DIR,
            DefinitionSource::Agents => AGENTS_AGENTS_DIR,
            DefinitionSource::Claude => CLAUDE_AGENTS_DIR,
            _ => unreachable!(),
        };
        let skills_dir = match source {
            DefinitionSource::Meerai => MEERAI_SKILLS_DIR,
            DefinitionSource::Agents => AGENTS_SKILLS_DIR,
            DefinitionSource::Claude => CLAUDE_SKILLS_DIR,
            _ => unreachable!(),
        };
        for agent in load_agents_in_dir(
            &project_root.join(agents_dir),
            source.clone(),
            &mut catalog.warnings,
        ) {
            insert_agent(&mut catalog, &mut agents, agent);
        }
        for skill in load_skills_in_dir(
            &project_root.join(skills_dir),
            source.clone(),
            &mut catalog.warnings,
        ) {
            insert_skill(&mut catalog, &mut skills, skill);
        }
    }

    catalog.agents = agents.into_values().collect();
    catalog.skills = skills.into_values().collect();
    catalog
}

fn insert_agent(
    catalog: &mut DefinitionCatalog,
    agents: &mut BTreeMap<String, AgentDefinition>,
    agent: AgentDefinition,
) {
    if let Some(kept) = agents.get(&agent.id) {
        catalog.shadows.push(ShadowedDefinition {
            id: agent.id.clone(),
            kind: "agent",
            source: agent.source.clone(),
            path: agent.path.clone(),
            kept_source: kept.source.clone(),
            kept_path: kept.path.clone(),
        });
        catalog.warnings.push(duplicate_warning(
            &agent.id,
            &agent.source,
            &agent.path,
            &kept.source,
            &kept.path,
        ));
        return;
    }
    agents.insert(agent.id.clone(), agent);
}

fn insert_skill(
    catalog: &mut DefinitionCatalog,
    skills: &mut BTreeMap<String, SkillDefinition>,
    skill: SkillDefinition,
) {
    if let Some(kept) = skills.get(&skill.id) {
        catalog.shadows.push(ShadowedDefinition {
            id: skill.id.clone(),
            kind: "skill",
            source: skill.source.clone(),
            path: skill.path.clone(),
            kept_source: kept.source.clone(),
            kept_path: kept.path.clone(),
        });
        catalog.warnings.push(duplicate_warning(
            &skill.id,
            &skill.source,
            &skill.path,
            &kept.source,
            &kept.path,
        ));
        return;
    }
    skills.insert(skill.id.clone(), skill);
}

fn load_agents_in_dir(
    dir: &Path,
    source: DefinitionSource,
    warnings: &mut Vec<String>,
) -> Vec<AgentDefinition> {
    markdown_files(dir, warnings)
        .into_iter()
        .filter_map(|path| parse_agent_file(&path, source.clone(), warnings))
        .collect()
}

fn load_skills_in_dir(
    dir: &Path,
    source: DefinitionSource,
    warnings: &mut Vec<String>,
) -> Vec<SkillDefinition> {
    skill_files(dir, warnings)
        .into_iter()
        .filter_map(|path| parse_skill_file(&path, source.clone(), warnings))
        .collect()
}

fn parse_agent_file(
    path: &Path,
    source: DefinitionSource,
    warnings: &mut Vec<String>,
) -> Option<AgentDefinition> {
    let text = read_text(path, warnings)?;
    let mut local = Vec::new();
    let parsed = parse_markdown(&text, path, warnings, &mut local)?;
    let fallback = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let id = definition_id(parsed.meta.name.as_deref(), fallback)?;
    let mode = parse_agent_mode(parsed.meta.mode.as_deref(), path, warnings, &mut local);
    let permissions = parse_agent_permissions(parsed.meta.permissions, path, warnings, &mut local);
    Some(AgentDefinition {
        id,
        source,
        description: parsed.meta.description,
        model: parsed.meta.model,
        mode,
        steps: parsed.meta.steps,
        permissions,
        token_estimate: token_estimate(&parsed.body),
        warnings: local,
        supporting_files: Vec::new(),
        prompt: parsed.body,
        path: path.to_path_buf(),
    })
}

fn parse_agent_mode(
    raw: Option<&str>,
    path: &Path,
    warnings: &mut Vec<String>,
    local: &mut Vec<String>,
) -> AgentMode {
    let Some(raw) = raw else {
        return AgentMode::All;
    };
    AgentMode::parse(raw).unwrap_or_else(|| {
        let warning = format!(
            "{}: unknown agent mode '{raw}'; expected all, primary, or subagent (using all)",
            path.display()
        );
        warnings.push(warning.clone());
        local.push(warning);
        AgentMode::All
    })
}

fn parse_agent_permissions(
    raw: Option<serde_yaml::Value>,
    path: &Path,
    warnings: &mut Vec<String>,
    local: &mut Vec<String>,
) -> Vec<PermissionRule> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let rules: Vec<PermissionRule> = match serde_yaml::from_value(raw) {
        Ok(rules) => rules,
        Err(err) => {
            let warning = format!(
                "{}: invalid agent permissions dropped: {err}",
                path.display()
            );
            warnings.push(warning.clone());
            local.push(warning);
            return Vec::new();
        }
    };
    let mut applied = Vec::with_capacity(rules.len());
    for rule in rules {
        if rule.effect == Effect::Allow {
            let warning = format!(
                "{}: agent permission allow rule for {} {} is not applied; definitions tighten only",
                path.display(),
                String::from(rule.action),
                rule.resource
            );
            warnings.push(warning.clone());
            local.push(warning);
            continue;
        }
        applied.push(rule);
    }
    applied
}

fn parse_skill_file(
    path: &Path,
    source: DefinitionSource,
    warnings: &mut Vec<String>,
) -> Option<SkillDefinition> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        warnings.push(format!("skipping symlink {}", path.display()));
        return None;
    }
    if !path.is_file() {
        return None;
    }
    let text = read_text(path, warnings)?;
    let mut local = Vec::new();
    let parsed = parse_markdown(&text, path, warnings, &mut local)?;
    if parsed.meta.permissions.is_some() {
        let warning = format!(
            "{}: skill permission fields are warning-only and are not applied",
            path.display()
        );
        warnings.push(warning.clone());
        local.push(warning);
    }
    let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let fallback = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let declared = parsed
        .meta
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = declared
        && name != fallback
    {
        let warning = format!(
            "{}: skill name '{name}' differs from its directory '{fallback}'; using '{name}'",
            path.display()
        );
        warnings.push(warning.clone());
        local.push(warning);
    }
    let id = definition_id(declared, fallback)?;
    let description = normalized_description(parsed.meta.description, path, warnings, &mut local);
    let model_invocable =
        description.is_some() && !parsed.meta.disable_model_invocation.unwrap_or(false);
    let estimate = token_estimate(&parsed.body);
    Some(SkillDefinition {
        id,
        source,
        description,
        model_invocable,
        body: parsed.body,
        path: path.to_path_buf(),
        directory,
        token_estimate: estimate,
        warnings: local,
        supporting_files: supporting_files(path),
    })
}

fn normalized_description(
    raw: Option<String>,
    path: &Path,
    warnings: &mut Vec<String>,
    local: &mut Vec<String>,
) -> Option<String> {
    let raw = raw.unwrap_or_default();
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        let warning = format!(
            "{}: skill has no description; it is hidden from the model (apply it with /skill)",
            path.display()
        );
        warnings.push(warning.clone());
        local.push(warning);
        return None;
    }
    Some(text.chars().take(MAX_SKILL_DESCRIPTION_CHARS).collect())
}

fn parse_markdown(
    text: &str,
    path: &Path,
    warnings: &mut Vec<String>,
    local: &mut Vec<String>,
) -> Option<ParsedFile> {
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

    if meta.tools.is_some() || meta.allowed_tools.is_some() {
        let warning = format!(
            "{}: agent tool fields are warning-only and are not applied",
            path.display()
        );
        warnings.push(warning.clone());
        local.push(warning);
    }

    Some(ParsedFile {
        meta,
        body: body.trim().to_string(),
    })
}

fn supporting_files(skill_path: &Path) -> Vec<String> {
    let Some(dir) = skill_path.parent() else {
        return Vec::new();
    };
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name() != SKILL_FILE)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names.truncate(MAX_SUPPORTING_FILES);
    names
}

fn token_estimate(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn skill_files(dir: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in dir_entries(dir, warnings) {
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
            }
            Ok(meta) if meta.is_file() => files.push(skill_path),
            Ok(_) => {}
            Err(_) => {}
        }
    }
    files.sort();
    files
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
    incoming_source: &DefinitionSource,
    incoming_path: &Path,
    kept_source: &DefinitionSource,
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
        assert_eq!(agent.mode, AgentMode::Subagent);
        assert_eq!(
            agent.prompt,
            "Review the changes.\n\nList findings by severity."
        );
        assert_eq!(agent.path, dir.join(".agents/agents/reviewer.md"));
        assert_eq!(agent.token_estimate, token_estimate(&agent.prompt));

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
        assert_eq!(skill.directory, dir.join(".agents/skills/investigate"));
        assert!(skill.model_invocable);

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
    fn user_global_wins_over_project_and_pack() {
        let dir = temp_dir("user_global");
        let user = dir.join("user");
        let packs = dir.join("packs");
        write_file(
            &user.join("agents/reviewer.md"),
            "---\nname: reviewer\ndescription: User copy\n---\nUser prompt.\n",
        );
        write_file(
            &packs.join("review-pack/meerai-pack.yml"),
            "name: review-pack\nagents: [reviewer]\n",
        );
        write_file(
            &packs.join("review-pack/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Pack copy\n---\nPack prompt.\n",
        );
        write_file(
            &dir.join(".agents/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Project copy\n---\nProject prompt.\n",
        );
        write_file(
            &dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Claude copy\n---\nClaude prompt.\n",
        );

        let catalog =
            load_definitions_with_sources(&dir, Some(&user), &[packs.join("review-pack")]);

        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].source, DefinitionSource::User);
        assert_eq!(catalog.agents[0].prompt, "User prompt.");
        assert_eq!(catalog.shadows.len(), 3);
        assert!(catalog.warnings.len() == 3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trusted_pack_definitions_load_and_expose_supporting_files() {
        let dir = temp_dir("pack_definitions");
        let pack = dir.join("pack");
        write_file(
            &pack.join("meerai-pack.yml"),
            "name: review-pack\nversion: 1.0.0\nagents: [reviewer]\nskills: [review]\n",
        );
        write_file(
            &pack.join("agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Pack reviewer\n---\nReview pack changes.\n",
        );
        write_file(
            &pack.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review skill\n---\nReview the diff.\n",
        );
        write_file(&pack.join("skills/review/notes.md"), "extra");

        let catalog = load_definitions_with_sources(&dir, None, &[pack]);

        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(
            catalog.agents[0].source,
            DefinitionSource::Pack("review-pack".to_string())
        );
        assert_eq!(catalog.agents[0].token_estimate, 5);
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].supporting_files, vec!["notes.md"]);
        assert_eq!(catalog.skills[0].source.label(), "pack:review-pack");

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
        assert_eq!(catalog.shadows.len(), 1);
        assert_eq!(catalog.shadows[0].source, DefinitionSource::Claude);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_skill_wins_duplicate_id_over_claude_skill() {
        let dir = temp_dir("agents_skill_precedence");
        write_file(
            &dir.join(".agents/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploy the app\n---\nAgents body.\n",
        );
        write_file(
            &dir.join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploy the app\n---\nClaude body.\n",
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

        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("skill has no description"));
        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].id, "helper");
        assert_eq!(catalog.agents[0].description, None);
        assert_eq!(catalog.agents[0].prompt, "# Helper\n\nAnswer questions.");
        assert_eq!(catalog.skills[0].id, "planner");
        assert_eq!(catalog.skills[0].body, "# Planner\n\nBuild a plan.");
        assert!(!catalog.skills[0].model_invocable);

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
    fn tool_fields_warn_but_permissions_apply() {
        let dir = temp_dir("tool_fields");
        write_file(
            &dir.join(".claude/agents/locked.md"),
            "---\nname: locked\ntools: [read_file]\npermissions:\n  - { action: edit, resource: \"*\", effect: deny }\nallowed-tools: shell\n---\nRead only.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        let agent = &catalog.agents[0];
        assert_eq!(agent.prompt, "Read only.");
        assert_eq!(agent.permissions.len(), 1);
        assert_eq!(agent.permissions[0].effect, Effect::Deny);
        assert_eq!(agent.permissions[0].resource, "*");
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("tool fields"));
        assert_eq!(agent.warnings.len(), 1);
        assert!(agent.warnings[0].contains("tool fields"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn permission_allow_rules_are_dropped_with_a_warning() {
        let dir = temp_dir("permission_allow");
        write_file(
            &dir.join(".meerai/agents/partial.md"),
            "---\nname: partial\npermissions:\n  - { action: shell, resource: \"*\", effect: allow }\n  - { action: edit, resource: \"src/**\", effect: ask }\n---\nBody.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        let agent = &catalog.agents[0];
        assert_eq!(agent.permissions.len(), 1);
        assert_eq!(agent.permissions[0].effect, Effect::Ask);
        assert_eq!(agent.permissions[0].resource, "src/**");
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("allow rule"));
        assert!(catalog.warnings[0].contains("tighten only"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_agent_permissions_are_dropped() {
        let dir = temp_dir("permission_invalid");
        write_file(
            &dir.join(".meerai/agents/broken.md"),
            "---\nname: broken\npermissions: not-a-list\n---\nBody.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        assert!(catalog.agents[0].permissions.is_empty());
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("invalid agent permissions"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_mode_parses_and_rejects_unknown_values() {
        let dir = temp_dir("agent_mode");
        write_file(
            &dir.join(".meerai/agents/sub.md"),
            "---\nname: sub\nmode: subagent\n---\nBody.\n",
        );
        write_file(
            &dir.join(".meerai/agents/primary.md"),
            "---\nname: primary\nmode: primary\n---\nBody.\n",
        );
        write_file(
            &dir.join(".meerai/agents/plain.md"),
            "---\nname: plain\n---\nBody.\n",
        );
        write_file(
            &dir.join(".meerai/agents/odd.md"),
            "---\nname: odd\nmode: sometimes\n---\nBody.\n",
        );

        let catalog = load_definitions(&dir);
        let mode = |id: &str| {
            catalog
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .map(|agent| agent.mode)
                .unwrap()
        };

        assert_eq!(mode("sub"), AgentMode::Subagent);
        assert!(!mode("sub").selectable_primary());
        assert!(mode("sub").runnable_as_subagent());
        assert_eq!(mode("primary"), AgentMode::Primary);
        assert!(mode("primary").selectable_primary());
        assert!(!mode("primary").runnable_as_subagent());
        assert_eq!(mode("plain"), AgentMode::All);
        assert!(mode("plain").selectable_primary());
        assert!(mode("plain").runnable_as_subagent());
        assert_eq!(mode("odd"), AgentMode::All);
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("unknown agent mode"));

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
    fn meerai_directories_are_project_definitions_with_highest_priority() {
        let dir = temp_dir("meerai_native");
        write_file(
            &dir.join(".meerai/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Native\nmode: subagent\n---\nNative prompt.\n",
        );
        write_file(
            &dir.join(".meerai/skills/fix/SKILL.md"),
            "---\nname: fix\ndescription: Native fix\n---\nNative body.\n",
        );
        write_file(
            &dir.join(".agents/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Agents\n---\nAgents prompt.\n",
        );
        write_file(
            &dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Claude\n---\nClaude prompt.\n",
        );
        write_file(
            &dir.join(".agents/skills/fix/SKILL.md"),
            "---\nname: fix\ndescription: Agents fix\n---\nAgents body.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.agents.len(), 1);
        assert_eq!(catalog.agents[0].source, DefinitionSource::Meerai);
        assert_eq!(catalog.agents[0].prompt, "Native prompt.");
        assert_eq!(catalog.agents[0].mode, AgentMode::Subagent);
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].source, DefinitionSource::Meerai);
        assert_eq!(catalog.skills[0].body, "Native body.");
        assert_eq!(catalog.shadows.len(), 3);
        assert_eq!(catalog.warnings.len(), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn meerai_source_labels_are_stable() {
        assert_eq!(DefinitionSource::Meerai.label(), ".meerai");
        assert_eq!(DefinitionSource::Agents.label(), ".agents");
        assert_eq!(DefinitionSource::Claude.label(), ".claude");
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

    #[test]
    fn disable_model_invocation_hides_a_described_skill() {
        let dir = temp_dir("skill_hidden");
        write_file(
            &dir.join(".claude/skills/seed/SKILL.md"),
            "---\nname: seed\ndescription: Seed the database\ndisable-model-invocation: true\n---\nSeed.\n",
        );

        let catalog = load_definitions(&dir);

        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(
            catalog.skills[0].description.as_deref(),
            Some("Seed the database")
        );
        assert!(!catalog.skills[0].model_invocable);
        assert_eq!(catalog.advertised_skills().count(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skill_name_mismatch_warns_and_keeps_the_name() {
        let dir = temp_dir("skill_name_mismatch");
        write_file(
            &dir.join(".agents/skills/folder/SKILL.md"),
            "---\nname: declared\ndescription: Uses the declared name\n---\nBody.\n",
        );

        let catalog = load_definitions(&dir);

        assert_eq!(catalog.skills[0].id, "declared");
        assert_eq!(catalog.warnings.len(), 1);
        assert!(catalog.warnings[0].contains("differs from its directory"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skill_description_is_collapsed_and_capped() {
        let dir = temp_dir("skill_description");
        write_file(
            &dir.join(".agents/skills/notes/SKILL.md"),
            &format!(
                "---\nname: notes\ndescription: |\n  First line\n  second line {}\n---\nBody.\n",
                "x".repeat(2000)
            ),
        );

        let catalog = load_definitions(&dir);

        let description = catalog.skills[0].description.as_deref().unwrap();
        assert!(description.starts_with("First line second line "));
        assert_eq!(description.chars().count(), MAX_SKILL_DESCRIPTION_CHARS);
        assert!(catalog.skills[0].model_invocable);

        let _ = fs::remove_dir_all(&dir);
    }
}
