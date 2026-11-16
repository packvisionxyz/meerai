use std::path::{Path, PathBuf};

use meerai_core::{
    EffectClass, FileChangeKind, ObservationStatus, ToolError, ToolOutput, ToolRun, Toolset,
};
use meerai_tools::CodeTools;
use serde_json::{Value, json};

struct Workspace {
    directory: tempfile::TempDir,
}

impl Workspace {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn tools(&self) -> CodeTools {
        CodeTools::new(self.root())
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root().join(relative)).unwrap()
    }
}

async fn call(tools: &CodeTools, tool: &str, args: Value) -> ToolRun {
    tools
        .invoke_structured(&format!("code-{tool}"), &args.to_string())
        .await
        .unwrap()
}

fn failure_text(run: &ToolRun) -> String {
    match &run.output {
        ToolOutput::Fail(text) => text.clone(),
        other => panic!("expected failure output, got {other:?}"),
    }
}

fn text(run: &ToolRun) -> String {
    match &run.output {
        ToolOutput::Text(text) => text.clone(),
        other => panic!("expected text output, got {other:?}"),
    }
}

#[test]
fn exposes_workspace_configuration() {
    let tools = CodeTools::new("/tmp/workspace")
        .with_writable_roots(vec![PathBuf::from("/tmp/extra")])
        .with_readable_roots(vec![PathBuf::from("/tmp/blobs")]);

    assert_eq!(tools.workspace_root(), Path::new("/tmp/workspace"));
    assert_eq!(tools.writable_roots(), [PathBuf::from("/tmp/extra")]);
    assert_eq!(tools.readable_roots(), [PathBuf::from("/tmp/blobs")]);
}

#[test]
fn definitions_and_effect_classes_cover_all_tools() {
    let workspace = Workspace::new();
    let tools = workspace.tools();
    let definitions = tools.definition();

    assert_eq!(definitions.len(), 7);
    assert_eq!(tools.name(), "code");

    let names: Vec<&str> = definitions.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "code-read_file",
            "code-write_file",
            "code-edit_file",
            "code-list_dir",
            "code-grep",
            "code-glob",
            "code-shell",
        ]
    );

    for definition in &definitions {
        assert!(tools.contain(&definition.name));
        assert!(!definition.description.is_empty());
        assert_eq!(definition.r#type, "function");
        assert_eq!(definition.parameters["type"], "object");
        assert!(
            !definition.parameters["properties"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    assert_eq!(definitions[0].parameters["required"], json!(["path"]));
    assert!(definitions[0].parameters["properties"]["offset"].is_object());
    assert!(definitions[0].parameters["properties"]["limit"].is_object());
    assert_eq!(definitions[6].parameters["required"], json!(["command"]));
    assert!(definitions[6].parameters["properties"]["timeout_ms"].is_object());

    assert!(!tools.contain("read_file"));
    assert!(!tools.contain("code-remove_file"));

    assert_eq!(tools.effect_of("code-read_file"), EffectClass::Read);
    assert_eq!(tools.effect_of("code-list_dir"), EffectClass::Read);
    assert_eq!(tools.effect_of("code-grep"), EffectClass::Read);
    assert_eq!(tools.effect_of("code-glob"), EffectClass::Read);
    assert_eq!(tools.effect_of("code-write_file"), EffectClass::Write);
    assert_eq!(tools.effect_of("code-edit_file"), EffectClass::Write);
    assert_eq!(tools.effect_of("code-shell"), EffectClass::Execute);
    assert_eq!(tools.effect_of("code-unknown"), EffectClass::Read);
}

#[tokio::test]
async fn unknown_tools_and_malformed_arguments_are_typed_errors() {
    let workspace = Workspace::new();
    let tools = workspace.tools();

    let error = tools.invoke("code-remove_file", "{}").await.unwrap_err();
    assert!(matches!(error, ToolError::InvalidFunctionName(name) if name == "code-remove_file"));

    let error = tools.invoke("read_file", "{}").await.unwrap_err();
    assert!(matches!(error, ToolError::InvalidFunctionName(name) if name == "read_file"));

    let error = tools
        .invoke("code-read_file", "{ not json")
        .await
        .unwrap_err();
    assert!(matches!(error, ToolError::WrongArguments(_)));

    let error = tools
        .invoke("code-shell", json!({ "command": 42 }).to_string().as_str())
        .await
        .unwrap_err();
    assert!(matches!(error, ToolError::WrongArguments(_)));
}

#[tokio::test]
async fn read_file_windows_lines_with_offset_and_limit() {
    let workspace = Workspace::new();
    workspace.write("notes.txt", "one\ntwo\nthree\nfour\nfive\n");
    let tools = workspace.tools();

    let run = call(&tools, "read_file", json!({ "path": "notes.txt" })).await;
    assert_eq!(run.observation.status, ObservationStatus::Success);
    assert_eq!(text(&run), "1:one\n2:two\n3:three\n4:four\n5:five\n");

    let run = call(
        &tools,
        "read_file",
        json!({ "path": "notes.txt", "offset": 3, "limit": 2 }),
    )
    .await;
    assert_eq!(
        text(&run),
        "3:three\n4:four\n(truncated: showing lines 3-4 of 5)\n"
    );

    let run = call(
        &tools,
        "read_file",
        json!({ "path": "notes.txt", "offset": 4, "limit": 50 }),
    )
    .await;
    assert_eq!(text(&run), "4:four\n5:five\n");

    let run = call(
        &tools,
        "read_file",
        json!({ "path": "notes.txt", "offset": 99 }),
    )
    .await;
    assert!(text(&run).contains("past the end"));

    workspace.write(
        "long.txt",
        &(1..=2100)
            .map(|line| format!("line{line}\n"))
            .collect::<String>(),
    );
    let run = call(&tools, "read_file", json!({ "path": "long.txt" })).await;
    let output = text(&run);
    assert!(output.starts_with("1:line1\n"));
    assert!(output.contains("\n2000:line2000\n"));
    assert!(!output.contains("\n2001:"));
    assert!(output.contains("showing lines 1-2000 of 2100"));

    let run = call(&tools, "read_file", json!({ "path": "missing.txt" })).await;
    let failure = failure_text(&run);
    assert!(failure.contains("does not exist"));
    assert!(failure.contains("(workspace root: "));

    let run = call(&tools, "read_file", json!({ "path": "notes.txt/child" })).await;
    assert_eq!(run.observation.status, ObservationStatus::Error);
}

#[tokio::test]
async fn read_file_refuses_paths_outside_workspace() {
    let workspace = Workspace::new();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();

    let run = call(&workspace.tools(), "read_file", json!({ "path": secret })).await;

    assert_eq!(run.observation.status, ObservationStatus::Refused);
    let message = failure_text(&run);
    assert!(message.starts_with("Refused:"));
    assert!(message.contains("escapes"));

    let run = call(
        &workspace.tools(),
        "read_file",
        json!({ "path": "../outside.txt" }),
    )
    .await;
    assert_eq!(run.observation.status, ObservationStatus::Refused);
}

#[tokio::test]
async fn readable_roots_allow_reading_outside_workspace() {
    let workspace = Workspace::new();
    let blobs = tempfile::tempdir().unwrap();
    let blob = blobs.path().join("blob.txt");
    std::fs::write(&blob, "stored output\n").unwrap();

    let tools = workspace
        .tools()
        .with_readable_roots(vec![blobs.path().to_path_buf()]);

    let run = call(&tools, "read_file", json!({ "path": blob })).await;
    assert_eq!(run.observation.status, ObservationStatus::Success);
    assert_eq!(text(&run), "1:stored output\n");
}

#[tokio::test]
async fn write_file_creates_directories_and_reports_diff_kinds() {
    let workspace = Workspace::new();
    let tools = workspace.tools();

    let run = call(
        &tools,
        "write_file",
        json!({ "path": "src/deep/mod.rs", "content": "fn added() {}\n" }),
    )
    .await;

    assert_eq!(workspace.read("src/deep/mod.rs"), "fn added() {}\n");
    assert_eq!(run.observation.status, ObservationStatus::Success);
    assert_eq!(run.file_changes.len(), 1);
    assert_eq!(run.file_changes[0].kind, FileChangeKind::Added);
    assert_eq!(run.file_changes[0].path, "src/deep/mod.rs");
    assert!(run.file_changes[0].unified_diff.contains("+fn added() {}"));
    assert!(text(&run).contains("Wrote"));

    let run = call(
        &tools,
        "write_file",
        json!({ "path": "src/deep/mod.rs", "content": "fn changed() {}\n" }),
    )
    .await;

    assert_eq!(run.file_changes[0].kind, FileChangeKind::Modified);
    assert!(run.file_changes[0].unified_diff.contains("-fn added() {}"));
    assert!(
        run.file_changes[0]
            .unified_diff
            .contains("+fn changed() {}")
    );
    assert!(text(&run).contains("@@"));
}

#[tokio::test]
async fn write_file_refuses_paths_outside_workspace() {
    let workspace = Workspace::new();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("escape.txt");

    let run = call(
        &workspace.tools(),
        "write_file",
        json!({ "path": target, "content": "nope" }),
    )
    .await;

    assert_eq!(run.observation.status, ObservationStatus::Refused);
    assert!(failure_text(&run).contains("escapes"));
    assert!(!target.exists());

    let run = call(
        &workspace.tools(),
        "write_file",
        json!({ "path": "../escape.txt", "content": "nope" }),
    )
    .await;
    assert_eq!(run.observation.status, ObservationStatus::Refused);
    assert!(
        !workspace
            .root()
            .parent()
            .unwrap()
            .join("escape.txt")
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_refuses_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let workspace = Workspace::new();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), workspace.root().join("link")).unwrap();

    let run = call(
        &workspace.tools(),
        "write_file",
        json!({ "path": "link/escape.txt", "content": "nope" }),
    )
    .await;

    assert_eq!(run.observation.status, ObservationStatus::Refused);
    assert!(!outside.path().join("escape.txt").exists());
}

#[tokio::test]
async fn writable_roots_allow_writing_outside_workspace() {
    let workspace = Workspace::new();
    let extra = tempfile::tempdir().unwrap();
    let tools = workspace
        .tools()
        .with_writable_roots(vec![extra.path().to_path_buf()]);

    let target = extra.path().join("allowed.txt");
    let run = call(
        &tools,
        "write_file",
        json!({ "path": target, "content": "allowed\n" }),
    )
    .await;

    assert_eq!(run.observation.status, ObservationStatus::Success);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "allowed\n");
}

#[tokio::test]
async fn edit_file_requires_unique_match_unless_replace_all() {
    let workspace = Workspace::new();
    workspace.write("src/lib.rs", "alpha\nbeta\nbeta\n");
    let tools = workspace.tools();

    let run = call(
        &tools,
        "edit_file",
        json!({ "path": "src/lib.rs", "old_string": "gamma", "new_string": "delta" }),
    )
    .await;
    assert!(failure_text(&run).contains("not found"));
    assert_eq!(workspace.read("src/lib.rs"), "alpha\nbeta\nbeta\n");

    let run = call(
        &tools,
        "edit_file",
        json!({ "path": "src/lib.rs", "old_string": "beta", "new_string": "BETA" }),
    )
    .await;
    let message = failure_text(&run);
    assert!(message.contains("occurs 2 times"), "{message}");
    assert!(message.contains("lines 2, 3"), "{message}");
    assert_eq!(workspace.read("src/lib.rs"), "alpha\nbeta\nbeta\n");
    assert_eq!(run.file_changes.len(), 0);

    let run = call(
        &tools,
        "edit_file",
        json!({ "path": "src/lib.rs", "old_string": "alpha", "new_string": "ALPHA" }),
    )
    .await;
    assert_eq!(workspace.read("src/lib.rs"), "ALPHA\nbeta\nbeta\n");
    assert_eq!(run.file_changes.len(), 1);
    assert_eq!(run.file_changes[0].kind, FileChangeKind::Modified);
    assert!(run.file_changes[0].unified_diff.contains("+ALPHA"));
    assert!(run.file_changes[0].unified_diff.contains("-alpha"));

    let run = call(
        &tools,
        "edit_file",
        json!({
            "path": "src/lib.rs",
            "old_string": "beta",
            "new_string": "BETA",
            "replace_all": true,
        }),
    )
    .await;
    assert_eq!(workspace.read("src/lib.rs"), "ALPHA\nBETA\nBETA\n");
    assert!(text(&run).contains("Replaced 2 occurrences"));

    let run = call(
        &tools,
        "edit_file",
        json!({ "path": "src/lib.rs", "old_string": "", "new_string": "x" }),
    )
    .await;
    assert!(failure_text(&run).contains("must not be empty"));

    let run = call(
        &tools,
        "edit_file",
        json!({ "path": "absent.rs", "old_string": "a", "new_string": "b" }),
    )
    .await;
    assert!(failure_text(&run).contains("does not exist"));
}

#[tokio::test]
async fn list_dir_sorts_entries_with_markers() {
    let workspace = Workspace::new();
    workspace.write("beta.txt", "12345");
    workspace.write("alpha/inner.txt", "x");
    workspace.write("zeta.rs", "");
    let tools = workspace.tools();

    let run = call(&tools, "list_dir", json!({ "path": "." })).await;
    let output = text(&run);

    assert_eq!(
        output,
        "d alpha/\nf beta.txt (5 bytes)\nf zeta.rs (0 bytes)\n"
    );

    let run = call(&tools, "list_dir", json!({ "path": "beta.txt" })).await;
    assert!(failure_text(&run).contains("not a directory"));
}

#[tokio::test]
async fn list_dir_skips_gitignored_entries() {
    let workspace = Workspace::new();
    workspace.write(".gitignore", "secret.txt\nignored_dir/\n");
    workspace.write("secret.txt", "hidden by gitignore");
    workspace.write("ignored_dir/inner.txt", "hidden by gitignore");
    workspace.write("keep.txt", "hello");
    workspace.write("src/main.rs", "");
    let tools = workspace.tools();

    let run = call(&tools, "list_dir", json!({ "path": "." })).await;
    let output = text(&run);

    assert!(output.contains("f keep.txt (5 bytes)"), "{output}");
    assert!(output.contains("d src/"), "{output}");
    assert!(!output.contains("secret.txt"), "{output}");
    assert!(!output.contains("ignored_dir"), "{output}");
}

#[tokio::test]
async fn grep_finds_matches_and_respects_gitignore() {
    let workspace = Workspace::new();
    workspace.write("src/a.rs", "fn needle() {}\nlet other = 1;\n");
    workspace.write("src/b.txt", "needle here\n");
    workspace.write("ignored/c.rs", "needle\n");
    workspace.write(".gitignore", "ignored/\n");
    let tools = workspace.tools();

    let run = call(&tools, "grep", json!({ "pattern": "needle" })).await;
    let output = text(&run);
    assert!(output.contains("src/a.rs:1:fn needle() {}"));
    assert!(output.contains("src/b.txt:1:needle here"));
    assert!(!output.contains("ignored/c.rs"));

    let run = call(
        &tools,
        "grep",
        json!({ "pattern": "needle", "include": "*.rs" }),
    )
    .await;
    let output = text(&run);
    assert!(output.contains("src/a.rs:1:"));
    assert!(!output.contains("src/b.txt"));

    let run = call(&tools, "grep", json!({ "pattern": "NEEDLE" })).await;
    assert!(text(&run).contains("No matches"));

    let run = call(&tools, "grep", json!({ "pattern": "needle[0" })).await;
    assert!(failure_text(&run).contains("invalid pattern"));
}

#[tokio::test]
async fn grep_caps_matches_per_file_and_in_total() {
    let workspace = Workspace::new();
    let body = "needle\n".repeat(60);
    for index in 0..5 {
        workspace.write(&format!("src/file{index}.txt"), &body);
    }
    let tools = workspace.tools();

    let run = call(&tools, "grep", json!({ "pattern": "needle" })).await;
    let output = text(&run);
    let matches = output
        .lines()
        .filter(|line| line.ends_with(":needle"))
        .count();
    assert_eq!(matches, 200);
    assert!(output.contains("truncated"), "{output}");

    workspace.write("single.txt", &body);
    let run = call(
        &tools,
        "grep",
        json!({ "pattern": "needle", "path": "single.txt" }),
    )
    .await;
    let output = text(&run);
    assert_eq!(
        output
            .lines()
            .filter(|line| line.ends_with(":needle"))
            .count(),
        50
    );
    assert!(output.contains("stopped after 50 matches"));
}

#[tokio::test]
async fn glob_matches_files_by_pattern() {
    let workspace = Workspace::new();
    workspace.write("src/a.rs", "");
    workspace.write("src/b.txt", "");
    workspace.write("docs/c.rs", "");
    workspace.write("ignored/d.rs", "");
    workspace.write(".gitignore", "ignored/\n");
    let tools = workspace.tools();

    let run = call(&tools, "glob", json!({ "pattern": "**/*.rs" })).await;
    assert_eq!(text(&run), "docs/c.rs\nsrc/a.rs");

    let run = call(&tools, "glob", json!({ "pattern": "*.txt" })).await;
    assert_eq!(text(&run), "src/b.txt");

    let run = call(&tools, "glob", json!({ "pattern": "*.toml" })).await;
    assert!(text(&run).contains("No files matching"));

    let run = call(&tools, "glob", json!({ "pattern": "[" })).await;
    assert!(failure_text(&run).contains("invalid glob pattern"));
}

#[tokio::test]
async fn shell_reports_output_and_exit_code() {
    let workspace = Workspace::new();
    let tools = workspace.tools();

    let run = call(&tools, "shell", json!({ "command": "printf 'hello'" })).await;
    assert_eq!(run.observation.status, ObservationStatus::Success);
    assert_eq!(run.observation.exit_code, Some(0));
    assert_eq!(run.observation.stdout, "hello");
    assert_eq!(text(&run), "hello");

    let run = call(&tools, "shell", json!({ "command": "pwd" })).await;
    assert_eq!(
        Path::new(run.observation.stdout.trim()),
        workspace.root().canonicalize().unwrap().as_path()
    );

    let run = call(
        &tools,
        "shell",
        json!({ "command": "echo boom >&2; exit 3" }),
    )
    .await;
    assert_eq!(run.observation.status, ObservationStatus::Error);
    assert_eq!(run.observation.exit_code, Some(3));
    assert!(run.observation.stderr.contains("boom"));
    let message = failure_text(&run);
    assert!(message.contains("boom"));
    assert!(message.contains('3'));

    let run = call(
        &tools,
        "shell",
        json!({ "command": "yes a | head -n 40000" }),
    )
    .await;
    assert_eq!(run.observation.status, ObservationStatus::Success);
    let stdout = &run.observation.stdout;
    assert!(stdout.len() <= 40 * 1024);
    assert!(stdout.contains("bytes omitted"));
    assert!(stdout.starts_with("a\n"));
    assert!(stdout.trim_end().ends_with('a'));
}

#[tokio::test]
async fn shell_timeout_returns_failure() {
    let workspace = Workspace::new();
    let tools = workspace.tools();

    let started = std::time::Instant::now();
    let run = call(
        &tools,
        "shell",
        json!({ "command": "sleep 5", "timeout_ms": 150 }),
    )
    .await;

    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    assert_eq!(run.observation.status, ObservationStatus::Error);
    assert!(failure_text(&run).contains("timed out"));
    assert!(run.observation.hint.is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn shell_exposes_normal_environment_variables() {
    let workspace = Workspace::new();
    let tools = workspace.tools();
    let expected = std::env::var("PATH").unwrap();

    let run = call(&tools, "shell", json!({ "command": "printenv PATH" })).await;

    assert_eq!(run.observation.status, ObservationStatus::Success);
    assert_eq!(run.observation.stdout.trim_end(), expected.trim_end());
}

#[cfg(unix)]
#[test]
fn shell_definition_names_the_platform_shell() {
    let definitions = Workspace::new().tools().definition();
    let shell = definitions
        .iter()
        .find(|definition| definition.name == "code-shell")
        .unwrap();

    assert!(shell.description.contains("sh -c"), "{}", shell.description);
    let hint = shell.parameters["properties"]["command"]["description"]
        .as_str()
        .unwrap();
    assert!(hint.contains("sh -c"), "{hint}");
}
