use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use meerai_core::env::{data_dir, now_millis};
use meerai_store::{InstalledPack, PackSourceKind};
use serde::{Deserialize, Serialize};

pub const PACK_MANIFEST_FILE: &str = "meerai-pack.yml";
pub const PACK_SKILLS_DIR: &str = "skills";
pub const PACK_AGENTS_DIR: &str = "agents";
pub const PACK_SKILL_FILE: &str = "SKILL.md";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDeclaration {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<serde_yaml::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerDeclaration>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedPack {
    pub root: PathBuf,
    pub manifest: PackManifest,
}

#[derive(Clone, Debug, Default)]
pub struct PackCatalog {
    pub packs: Vec<LoadedPack>,
    pub warnings: Vec<String>,
}

impl LoadedPack {
    pub fn id(&self) -> &str {
        &self.manifest.name
    }

    pub fn skill_paths(&self) -> (Vec<PathBuf>, Vec<String>) {
        resolve_entries(&self.root, PACK_SKILLS_DIR, &self.manifest.skills)
    }

    pub fn agent_paths(&self) -> (Vec<PathBuf>, Vec<String>) {
        resolve_entries(&self.root, PACK_AGENTS_DIR, &self.manifest.agents)
    }
}

pub fn load_pack(root: &Path) -> Result<LoadedPack> {
    let manifest_path = root.join(PACK_MANIFEST_FILE);
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read pack manifest at {}", manifest_path.display()))?;
    let manifest: PackManifest = serde_yaml::from_str(&raw)
        .with_context(|| format!("parse pack manifest at {}", manifest_path.display()))?;
    validate_manifest(&manifest)?;
    Ok(LoadedPack {
        root: root.to_path_buf(),
        manifest,
    })
}

pub fn load_manifest(root: &Path) -> Result<PackManifest> {
    load_pack(root).map(|pack| pack.manifest)
}

pub fn discover_packs(packs_root: &Path) -> PackCatalog {
    let mut catalog = PackCatalog::default();
    let entries = match fs::read_dir(packs_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return catalog,
        Err(error) => {
            catalog.warnings.push(format!(
                "cannot read pack directory {}: {error}",
                packs_root.display()
            ));
            return catalog;
        }
    };
    let mut roots: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join(PACK_MANIFEST_FILE).is_file())
        .collect();
    roots.sort();
    for root in roots {
        match load_pack(&root) {
            Ok(pack) => {
                if let Some(existing) = catalog.packs.iter().find(|loaded| loaded.id() == pack.id())
                {
                    catalog.warnings.push(format!(
                        "pack '{}' at {} shadows {}",
                        pack.id(),
                        pack.root.display(),
                        existing.root.display()
                    ));
                    continue;
                }
                catalog.packs.push(pack);
            }
            Err(error) => catalog.warnings.push(format!("{error:#}")),
        }
    }
    catalog
}

fn validate_manifest(manifest: &PackManifest) -> Result<()> {
    ensure!(
        !manifest.name.trim().is_empty(),
        "pack manifest name must not be empty"
    );
    let mut seen = BTreeMap::new();
    for server in &manifest.mcp_servers {
        ensure!(
            !server.name.trim().is_empty(),
            "MCP server name must not be empty"
        );
        ensure!(
            !server.command.trim().is_empty(),
            "MCP server '{}' command must not be empty",
            server.name
        );
        if seen.insert(server.name.clone(), ()).is_some() {
            anyhow::bail!("duplicate MCP server '{}'", server.name);
        }
    }
    Ok(())
}

fn resolve_entries(
    root: &Path,
    default_dir: &str,
    entries: &[String],
) -> (Vec<PathBuf>, Vec<String>) {
    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    for entry in entries {
        match resolve_entry(root, default_dir, entry) {
            Some(path) => paths.push(path),
            None => warnings.push(format!(
                "pack '{}' is missing the definition '{entry}'",
                root.display()
            )),
        }
    }
    (paths, warnings)
}

fn resolve_entry(root: &Path, default_dir: &str, entry: &str) -> Option<PathBuf> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let direct = root.join(entry);
    if direct.is_file() {
        return Some(direct);
    }
    if entry.ends_with(".md") {
        return None;
    }
    let skill_dir = root.join(default_dir).join(entry);
    let skill_md = skill_dir.join(PACK_SKILL_FILE);
    if skill_md.is_file() {
        return Some(skill_md);
    }
    let agent_md = root.join(default_dir).join(format!("{entry}.md"));
    if agent_md.is_file() {
        return Some(agent_md);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "meerai-pack-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn loads_and_validates_a_manifest() {
        let root = temp_root("manifest");
        fs::write(
            root.join(PACK_MANIFEST_FILE),
            "name: review-pack\nversion: 1.2.0\nskills: [review]\nagents: [reviewer]\n\
             mcp_servers:\n  - name: docs\n    command: docs-server\n    args: [--stdio]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("skills/review")).unwrap();
        fs::create_dir_all(root.join("agents")).unwrap();
        fs::write(root.join("skills/review/SKILL.md"), "body").unwrap();
        fs::write(root.join("agents/reviewer.md"), "prompt").unwrap();

        let pack = load_pack(&root).unwrap();
        assert_eq!(pack.id(), "review-pack");
        assert_eq!(pack.manifest.version.as_deref(), Some("1.2.0"));
        assert_eq!(pack.manifest.mcp_servers.len(), 1);

        let (skills, warnings) = pack.skill_paths();
        assert_eq!(skills, vec![root.join("skills/review/SKILL.md")]);
        assert!(warnings.is_empty());
        let (agents, _) = pack.agent_paths();
        assert_eq!(agents, vec![root.join("agents/reviewer.md")]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reports_missing_definitions_and_duplicate_servers() {
        let root = temp_root("invalid");
        fs::write(
            root.join(PACK_MANIFEST_FILE),
            "name: broken\nskills: [missing]\nmcp_servers:\n\
             - {name: a, command: x}\n- {name: a, command: y}\n",
        )
        .unwrap();
        assert!(load_pack(&root).is_err());

        fs::write(
            root.join(PACK_MANIFEST_FILE),
            "name: partial\nskills: [missing]\n",
        )
        .unwrap();
        let pack = load_pack(&root).unwrap();
        let (skills, warnings) = pack.skill_paths();
        assert!(skills.is_empty());
        assert_eq!(warnings.len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discovery_skips_directories_without_a_manifest() {
        let root = temp_root("discover");
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::write(root.join("one").join(PACK_MANIFEST_FILE), "name: one\n").unwrap();
        let catalog = discover_packs(&root);
        assert_eq!(catalog.packs.len(), 1);
        assert_eq!(catalog.packs[0].id(), "one");
        assert!(catalog.warnings.is_empty());
        assert!(discover_packs(&root.join("absent")).packs.is_empty());
        fs::remove_dir_all(&root).ok();
    }
}

pub fn packs_root() -> Result<PathBuf> {
    Ok(data_dir()
        .context("no user data directory; set HOME or XDG_DATA_HOME")?
        .join("packs"))
}

pub fn trusted_pack_roots(store: &meerai_store::Store) -> Vec<PathBuf> {
    let Ok(root) = packs_root() else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = store
        .packs()
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|pack| pack.enabled && pack.trusted)
        .map(|pack| root.join(pack.id))
        .filter(|path| path.is_dir())
        .collect();
    roots.sort();
    roots
}

pub fn install_pack(source: &str, rev: Option<&str>) -> Result<InstalledPack> {
    let root = packs_root()?;
    install_pack_at(&root, source, rev)
}

pub fn install_pack_at(root: &Path, source: &str, rev: Option<&str>) -> Result<InstalledPack> {
    let source = source.trim();
    ensure!(!source.is_empty(), "pack source must not be empty");
    fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    let temp = root.join(format!(".install-{}-{}", std::process::id(), now_millis()));
    if temp.exists() {
        fs::remove_dir_all(&temp).ok();
    }

    let (source_kind, pinned_rev) = if is_git_source(source) {
        clone_git(source, &temp, rev)?;
        let pinned = git_head_sha(&temp)?.unwrap_or_else(|| rev.unwrap_or("git").to_string());
        (PackSourceKind::Git, pinned)
    } else {
        copy_local(source, &temp)?;
        (PackSourceKind::Local, rev.unwrap_or("local").to_string())
    };

    let manifest = load_pack(&temp)
        .with_context(|| format!("load {}", temp.join(PACK_MANIFEST_FILE).display()))?;
    let id = manifest.id().trim().to_string();
    validate_pack_id(&id)?;
    let dest = root.join(&id);
    if dest.exists() {
        fs::remove_dir_all(&dest)
            .with_context(|| format!("replace installed pack at {}", dest.display()))?;
    }
    fs::rename(&temp, &dest).with_context(|| {
        format!(
            "move installed pack from {} to {}",
            temp.display(),
            dest.display()
        )
    })?;
    let manifest_json = serde_json::to_string(&manifest.manifest)?;
    let now = now_millis();
    Ok(InstalledPack {
        id,
        source_kind,
        source_uri: source.to_string(),
        pinned_rev,
        manifest_json,
        enabled: true,
        trusted: false,
        installed_at_ms: now,
        updated_at_ms: now,
    })
}

fn validate_pack_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.contains(':')
    {
        bail!("invalid pack id: {id}");
    }
    Ok(())
}

fn is_git_source(source: &str) -> bool {
    source.ends_with(".git")
        || source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("ssh://")
        || source.starts_with("git@")
        || Path::new(source).join(".git").exists()
}

fn clone_git(source: &str, dest: &Path, rev: Option<&str>) -> Result<()> {
    let mut builder = git2::build::RepoBuilder::new();
    let repo = builder
        .clone(source, dest)
        .with_context(|| format!("clone pack repository {source}"))?;
    if let Some(rev) = rev {
        let object = repo
            .revparse_single(rev)
            .with_context(|| format!("resolve pack revision {rev}"))?;
        let commit = object
            .peel_to_commit()
            .with_context(|| format!("revision {rev} is not a commit"))?;
        repo.checkout_tree(commit.as_object(), None)
            .with_context(|| format!("checkout pack revision {rev}"))?;
        repo.set_head_detached(commit.id())
            .with_context(|| format!("detach pack repository at {rev}"))?;
    }
    Ok(())
}

fn git_head_sha(root: &Path) -> Result<Option<String>> {
    let repo = match git2::Repository::open(root) {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };
    Ok(repo
        .head()
        .ok()
        .and_then(|head| head.target().map(|oid| oid.to_string())))
}

fn copy_local(source: &str, dest: &Path) -> Result<()> {
    let source = Path::new(source);
    ensure!(
        source.join(PACK_MANIFEST_FILE).is_file(),
        "no {} in {}",
        PACK_MANIFEST_FILE,
        source.display()
    );
    copy_tree(source, dest)
}

fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_tree(&from, &to)?;
        } else if metadata.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod install_tests {
    use super::*;

    #[test]
    fn installs_a_local_pack_into_the_pack_root() {
        let base = std::env::temp_dir().join(format!(
            "meerai-install-pack-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(base.join("source/skills/review")).unwrap();
        std::fs::write(
            base.join("source/meerai-pack.yml"),
            "name: local-pack\nversion: 1.0.0\nskills: [review]\n",
        )
        .unwrap();
        std::fs::write(base.join("source/skills/review/SKILL.md"), "body").unwrap();
        let root = base.join("data/packs");
        let pack =
            install_pack_at(&root, &base.join("source").display().to_string(), None).unwrap();
        assert_eq!(pack.id, "local-pack");
        assert_eq!(pack.pinned_rev, "local");
        assert!(root.join("local-pack/meerai-pack.yml").is_file());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn rejects_unsafe_pack_ids() {
        assert!(validate_pack_id("review").is_ok());
        assert!(validate_pack_id("../escape").is_err());
        assert!(validate_pack_id("a/b").is_err());
        assert!(validate_pack_id("").is_err());
    }
}
