mod extract;
mod rank;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use ignore::WalkBuilder;
use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolOutput, ToolPermissionSpec,
    ToolRun, Toolset, async_trait,
};

use crate::code::{CodeTools, definition_for, failure_run, parse_args, success_run};
use extract::{FileTags, SourceLang, extract_tags};
use rank::{normalize_rel, rank_files, render_repo_map};

const TOOLSET_NAME: &str = "retrieval";
const REPO_MAP_TOOL: &str = "retrieval-repo_map";
const DEFAULT_MAX_TOKENS: usize = 1500;
const MIN_TOKENS: usize = 200;
const MAX_TOKENS: usize = 4000;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const SKIPPED_DIRS: [&str; 8] = [
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Clone, Debug)]
struct CachedFileEntry {
    stamp: FileStamp,
    rel_path: String,
    tags: FileTags,
}

#[derive(Clone, Debug)]
struct RenderedCache {
    snapshot: Vec<(PathBuf, FileStamp)>,
    max_tokens: usize,
    focus_files: Vec<String>,
    output: String,
}

#[derive(Debug, Default)]
struct RepoMapCache {
    entries: HashMap<PathBuf, CachedFileEntry>,
    rendered: Option<RenderedCache>,
    parse_count: usize,
}

#[derive(Clone, Debug)]
pub struct RepoMapToolset {
    workspace_root: PathBuf,
    readable_roots: Vec<PathBuf>,
    cache: Arc<Mutex<RepoMapCache>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RepoMapArgs {
    #[schemars(
        description = "Approximate token budget for the rendered repo map (200..=4000, default 1500)."
    )]
    max_tokens: Option<usize>,
    #[schemars(description = "Optional workspace-relative file paths to boost in ranking.")]
    focus_files: Option<Vec<String>>,
}

impl RepoMapToolset {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            readable_roots: Vec::new(),
            cache: Arc::new(Mutex::new(RepoMapCache::default())),
        }
    }

    pub fn with_readable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.readable_roots = roots;
        self
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn readable_roots(&self) -> &[PathBuf] {
        &self.readable_roots
    }

    async fn build_repo_map(&self, args: RepoMapArgs) -> ToolRun {
        let root = CodeTools::new(&self.workspace_root)
            .canonical_workspace_root()
            .await;
        match tokio::fs::metadata(&root).await {
            Ok(meta) if meta.is_dir() => {}
            _ => {
                return failure_run(format!(
                    "workspace root is not a directory: {}",
                    root.display()
                ));
            }
        }

        let max_tokens = args
            .max_tokens
            .unwrap_or(DEFAULT_MAX_TOKENS)
            .clamp(MIN_TOKENS, MAX_TOKENS);
        let focus_files: Vec<String> = args
            .focus_files
            .unwrap_or_default()
            .into_iter()
            .map(|s| normalize_rel(&s))
            .filter(|s| !s.is_empty())
            .collect();

        let cache = Arc::clone(&self.cache);
        let result = tokio::task::spawn_blocking(move || {
            generate_map(&root, &cache, max_tokens, focus_files)
        })
        .await;

        match result {
            Ok(Ok(rendered)) => success_run(rendered),
            Ok(Err(err)) => failure_run(err),
            Err(err) => failure_run(format!("repo map task failed: {err}")),
        }
    }
}

fn generate_map(
    root: &Path,
    cache: &Mutex<RepoMapCache>,
    max_tokens: usize,
    focus_files: Vec<String>,
) -> Result<String, String> {
    let discovered = discover_source_files(root);

    let mut guard = cache
        .lock()
        .map_err(|_| "repo map cache lock poisoned".to_string())?;

    let current_paths: HashSet<PathBuf> = discovered
        .iter()
        .map(|(path, _, _, _)| path.clone())
        .collect();
    guard.entries.retain(|path, _| current_paths.contains(path));

    let mut snapshot = Vec::with_capacity(discovered.len());
    for (path, rel_path, lang, stamp) in discovered {
        snapshot.push((path.clone(), stamp.clone()));
        let needs_parse = guard
            .entries
            .get(&path)
            .is_none_or(|entry| entry.stamp != stamp);
        if needs_parse {
            let Ok(content) = std::fs::read_to_string(&path) else {
                guard.entries.remove(&path);
                continue;
            };
            let tags = extract_tags(lang, &content);
            guard.parse_count += 1;
            guard.entries.insert(
                path,
                CachedFileEntry {
                    stamp,
                    rel_path,
                    tags,
                },
            );
        }
    }
    snapshot.sort_by(|a, b| a.0.cmp(&b.0));

    let mut focus_sorted = focus_files;
    focus_sorted.sort();
    focus_sorted.dedup();

    if let Some(rendered) = &guard.rendered
        && rendered.max_tokens == max_tokens
        && rendered.focus_files == focus_sorted
        && rendered.snapshot == snapshot
    {
        return Ok(rendered.output.clone());
    }

    let mut files: Vec<(String, &FileTags)> = guard
        .entries
        .values()
        .map(|entry| (entry.rel_path.clone(), &entry.tags))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let order = rank_files(&files, &focus_sorted);
    let output = render_repo_map(&files, &order, max_tokens);

    guard.rendered = Some(RenderedCache {
        snapshot,
        max_tokens,
        focus_files: focus_sorted,
        output: output.clone(),
    });

    Ok(output)
}

fn discover_source_files(root: &Path) -> Vec<(PathBuf, String, SourceLang, FileStamp)> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref())
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(lang) = SourceLang::from_path(path) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let len = meta.len();
        if len > MAX_FILE_BYTES {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if rel_path.is_empty() {
            continue;
        }
        let stamp = FileStamp {
            modified: meta.modified().ok(),
            len,
        };
        files.push((path.to_path_buf(), rel_path, lang, stamp));
    }

    files
}

#[async_trait]
impl Toolset for RepoMapToolset {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![definition_for::<RepoMapArgs>(
            REPO_MAP_TOOL,
            "Build an AST-based repository map of symbols ranked by cross-file references within a token budget.",
        )]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == REPO_MAP_TOOL
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        (fn_name == REPO_MAP_TOOL).then_some(ToolPermissionSpec {
            action: PermissionAction::Read,
            resource_arg: None,
        })
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        true
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        if fn_name != REPO_MAP_TOOL {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }
        Ok(self.build_repo_map(parse_args(args)?).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn extracts_symbols_across_all_supported_languages() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(
            root,
            "src/lib.rs",
            "pub const VERSION: u32 = 1;\npub trait Engine {}\npub struct Runner;\nimpl Engine for Runner {}\npub enum Mode { Fast }\npub type Alias = Runner;\npub mod sub {}\npub fn start() {}\n",
        );
        write(
            root,
            "web/app.ts",
            "export interface Config { port: number }\nexport type Id = string;\nexport class Service { run() {} }\nexport const helper = () => 1;\nexport function boot() {}\n",
        );
        write(
            root,
            "web/view.tsx",
            "export function Panel() { return <div />; }\n",
        );
        write(
            root,
            "web/util.js",
            "class Store { get() {} }\nfunction makeStore() { return new Store(); }\n",
        );
        write(
            root,
            "py/worker.py",
            "MAX_JOBS = 4\nclass Worker:\n    def execute(self):\n        pass\ndef launch():\n    pass\n",
        );
        write(
            root,
            "go/main.go",
            "package main\nconst Limit = 10\ntype Handler interface { Serve() }\ntype Server struct{}\nfunc NewServer() *Server { return nil }\n",
        );

        let toolset = RepoMapToolset::new(root);
        let run = toolset
            .invoke_structured(REPO_MAP_TOOL, "{}")
            .await
            .unwrap();
        let ToolOutput::Text(out) = run.output else {
            panic!("expected text output, got {:?}", run.output);
        };

        for expected in [
            "src/lib.rs:",
            "const VERSION",
            "trait Engine",
            "struct Runner",
            "impl Engine for Runner",
            "enum Mode",
            "type Alias",
            "mod sub",
            "fn start",
            "web/app.ts:",
            "interface Config",
            "type Id",
            "class Service",
            "fn run",
            "fn helper",
            "fn boot",
            "web/view.tsx:",
            "fn Panel",
            "web/util.js:",
            "class Store",
            "fn makeStore",
            "py/worker.py:",
            "const MAX_JOBS",
            "class Worker",
            "fn execute",
            "fn launch",
            "go/main.go:",
            "const Limit",
            "interface Handler",
            "struct Server",
            "fn NewServer",
        ] {
            assert!(out.contains(expected), "missing `{expected}` in:\n{out}");
        }
    }

    #[tokio::test]
    async fn ranks_referenced_and_focused_files_higher() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(
            root,
            "core.rs",
            "pub struct CoreEngine;\npub fn core_tick() {}\n",
        );
        write(
            root,
            "a_caller.rs",
            "use crate::core::{CoreEngine, core_tick};\npub fn alpha() { let _ = CoreEngine; core_tick(); }\n",
        );
        write(
            root,
            "b_caller.rs",
            "use crate::core::{CoreEngine, core_tick};\npub fn beta() { let _ = CoreEngine; core_tick(); }\n",
        );
        write(root, "z_isolated.rs", "pub fn lonely() {}\n");

        let toolset = RepoMapToolset::new(root);
        let run = toolset
            .invoke_structured(REPO_MAP_TOOL, "{}")
            .await
            .unwrap();
        let ToolOutput::Text(out) = run.output else {
            panic!("expected text output");
        };

        let core_pos = out.find("core.rs:").unwrap();
        let isolated_pos = out.find("z_isolated.rs:").unwrap();
        assert!(
            core_pos < isolated_pos,
            "expected core.rs before z_isolated.rs:\n{out}"
        );

        let focus_run = toolset
            .invoke_structured(REPO_MAP_TOOL, r#"{"focus_files": ["z_isolated.rs"]}"#)
            .await
            .unwrap();
        let ToolOutput::Text(focus_out) = focus_run.output else {
            panic!("expected text output");
        };
        let z_pos = focus_out.find("z_isolated.rs:").unwrap();
        let a_pos = focus_out.find("a_caller.rs:").unwrap();
        assert!(z_pos < a_pos, "expected focused file boosted:\n{focus_out}");
    }

    #[tokio::test]
    async fn respects_max_tokens_budget() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        for i in 0..30 {
            let mut src = String::new();
            for j in 0..10 {
                src.push_str(&format!("pub fn symbol_{i}_{j}() {{}}\n"));
            }
            write(root, &format!("mod_{i:02}.rs"), &src);
        }

        let toolset = RepoMapToolset::new(root);
        let run = toolset
            .invoke_structured(REPO_MAP_TOOL, r#"{"max_tokens": 200}"#)
            .await
            .unwrap();
        let ToolOutput::Text(out) = run.output else {
            panic!("expected text output");
        };

        assert!(
            out.len() <= 200 * 4,
            "output length {} exceeded budget",
            out.len()
        );
        assert!(
            out.contains("more files omitted"),
            "expected truncation footer:\n{out}"
        );
    }

    #[tokio::test]
    async fn caches_unmodified_files_and_updates_on_change() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "alpha.rs", "pub fn alpha_one() {}\n");
        write(root, "beta.rs", "pub fn beta_one() {}\n");

        let toolset = RepoMapToolset::new(root);

        let first = toolset
            .invoke_structured(REPO_MAP_TOOL, "{}")
            .await
            .unwrap();
        assert_eq!(toolset.cache.lock().unwrap().parse_count, 2);
        assert!(matches!(first.output, ToolOutput::Text(ref s) if s.contains("alpha_one")));

        let second = toolset
            .invoke_structured(REPO_MAP_TOOL, "{}")
            .await
            .unwrap();
        assert_eq!(toolset.cache.lock().unwrap().parse_count, 2);
        assert_eq!(first.output, second.output);

        write(
            root,
            "alpha.rs",
            "pub fn alpha_one() {}\npub fn alpha_two_updated() {}\n",
        );

        let third = toolset
            .invoke_structured(REPO_MAP_TOOL, "{}")
            .await
            .unwrap();
        assert_eq!(toolset.cache.lock().unwrap().parse_count, 3);
        let ToolOutput::Text(out) = third.output else {
            panic!("expected text output");
        };
        assert!(out.contains("alpha_two_updated"));
        assert!(out.contains("beta_one"));
    }
}
