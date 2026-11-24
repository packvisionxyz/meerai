use std::path::{Path, PathBuf};

use meerai_common::definitions::{
    DefinitionCatalog, DefinitionSource, SkillDefinition, load_definitions,
    load_definitions_with_sources, token_estimate,
};
use meerai_store::Store;

const BUILTIN_ROOT: &str = "<builtin>";
const GOAL_SKILL: &str = include_str!("../skills/goal/SKILL.md");
const ORCHESTRATE_SKILL: &str = include_str!("../skills/orchestrate/SKILL.md");

pub fn builtin_skills() -> Vec<SkillDefinition> {
    vec![
        builtin_skill(
            "goal",
            "Run a plan, act, observe, adjust loop on one objective until every item is done. \
             Use it for long multi-step tasks that must not stop early.",
            GOAL_SKILL,
        ),
        builtin_skill(
            "orchestrate",
            "Plan and run a multi-area objective with subagents: research, delegate disjoint work, \
             observe the result, adjust, and report. Use it when the work spans more than one area \
             or needs parallel effort.",
            ORCHESTRATE_SKILL,
        ),
    ]
}

fn builtin_skill(id: &str, description: &str, body: &'static str) -> SkillDefinition {
    SkillDefinition {
        id: id.to_string(),
        source: DefinitionSource::Builtin,
        description: Some(description.to_string()),
        model_invocable: true,
        body: body.to_string(),
        path: PathBuf::from(BUILTIN_ROOT).join(id).join("SKILL.md"),
        directory: PathBuf::from(BUILTIN_ROOT).join(id),
        token_estimate: token_estimate(body),
        warnings: Vec::new(),
        supporting_files: Vec::new(),
    }
}

pub fn with_builtin_definitions(mut catalog: DefinitionCatalog) -> DefinitionCatalog {
    for skill in builtin_skills() {
        if !catalog
            .skills
            .iter()
            .any(|existing| existing.id == skill.id)
        {
            catalog.skills.push(skill);
        }
    }
    catalog.skills.sort_by(|a, b| a.id.cmp(&b.id));
    catalog
}

pub fn load_definitions_for(
    store: &Store,
    config_dir: Option<&Path>,
    project_root: &Path,
) -> DefinitionCatalog {
    let pack_roots = meerai_common::packs::trusted_pack_roots(store);
    with_builtin_definitions(load_definitions_with_sources(
        project_root,
        config_dir,
        &pack_roots,
    ))
}

pub fn load_definitions_with_builtins(project_root: &Path) -> DefinitionCatalog {
    with_builtin_definitions(load_definitions(project_root))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn builtin_skills_load_and_a_project_can_shadow_them() {
        let dir = tempfile::tempdir().unwrap();

        let catalog = load_definitions_with_builtins(dir.path());
        for id in ["goal", "orchestrate"] {
            let skill = catalog
                .skills
                .iter()
                .find(|skill| skill.id == id)
                .unwrap_or_else(|| panic!("built-in {id} skill"));
            assert_eq!(skill.source, DefinitionSource::Builtin);
            assert!(skill.model_invocable);
            assert!(skill.description.is_some());
            assert!(skill.supporting_files.is_empty());
            assert!(skill.body.contains("loop-plan"));
        }

        write_file(
            &dir.path().join(".agents/skills/goal/SKILL.md"),
            "---\nname: goal\ndescription: Local goal\n---\nLocal body.\n",
        );
        let shadowed = load_definitions_with_builtins(dir.path());
        let goal = shadowed
            .skills
            .iter()
            .find(|skill| skill.id == "goal")
            .expect("shadowed goal skill");
        assert_eq!(goal.source, DefinitionSource::Agents);
        assert_eq!(goal.body, "Local body.");
    }
}
