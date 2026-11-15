use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use meerai_core::ToolRun;

use crate::code::{CodeTools, failure_run, refused_run};

pub(crate) enum PathIssue {
    Missing(String),
    Escape(String),
    Denied(String),
    Failure(String),
}

impl PathIssue {
    pub(crate) fn into_run(self) -> ToolRun {
        match self {
            PathIssue::Escape(reason) | PathIssue::Denied(reason) => refused_run(reason),
            PathIssue::Missing(reason) | PathIssue::Failure(reason) => failure_run(reason),
        }
    }
}

pub fn is_sensitive_path(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    let norm_path = Path::new(&normalized);
    let Some(name) = norm_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let ext = norm_path.extension().and_then(|ext| ext.to_str());

    let is_env_template = matches!(name, ".env.example" | ".env.sample" | ".env.template");
    if !is_env_template
        && (name == ".env"
            || name.starts_with(".env.")
            || name == ".envrc"
            || name.ends_with(".env"))
    {
        return true;
    }

    if let Some(ext) = ext
        && matches!(ext, "pem" | "key" | "p12" | "pfx" | "jks" | "keystore")
    {
        return true;
    }

    if matches!(
        name,
        "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | ".netrc"
            | "_netrc"
            | ".npmrc"
            | ".pypirc"
            | "auth.json"
            | "auth.yml"
            | "credentials.json"
            | "credentials"
            | ".htpasswd"
    ) {
        return true;
    }

    if parts.ends_with(&[".aws", "credentials"])
        || parts.ends_with(&[".docker", "config.json"])
        || parts.ends_with(&[".kube", "config"])
    {
        return true;
    }

    if parts.len() >= 2
        && parts[..parts.len() - 1].contains(&".ssh")
        && !(ext == Some("pub") || name == "known_hosts" || name == "config")
    {
        return true;
    }

    if (name.contains("secret") || name.contains("credential")) && !is_source_or_doc_ext(ext) {
        return true;
    }

    false
}

fn is_source_or_doc_ext(ext: Option<&str>) -> bool {
    matches!(
        ext,
        Some(
            "rs" | "py"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "mjs"
                | "cjs"
                | "go"
                | "md"
                | "txt"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "java"
                | "kt"
                | "rb"
                | "php"
                | "swift"
                | "scala"
                | "html"
                | "css"
                | "scss"
        )
    )
}

impl CodeTools {
    pub(crate) async fn canonical_workspace_root(&self) -> PathBuf {
        canonicalize_or_keep(&self.workspace_root).await
    }

    pub(crate) async fn resolve_for_read(&self, raw: &str) -> Result<PathBuf, PathIssue> {
        let mut roots = self.readable_roots().to_vec();
        roots.extend(self.extra_roots());
        resolve_read_path_with_policy(
            &self.workspace_root,
            &roots,
            raw,
            self.allow_sensitive_files(),
        )
        .await
    }

    pub(crate) async fn resolve_for_write(&self, raw: &str) -> Result<PathBuf, PathIssue> {
        let mut roots = self.writable_roots().to_vec();
        roots.extend(self.extra_roots());
        resolve_write_path_with_policy(
            &self.workspace_root,
            &roots,
            raw,
            self.allow_sensitive_files(),
        )
        .await
    }
}

async fn locate(candidate: &Path) -> Result<(PathBuf, bool), PathIssue> {
    match tokio::fs::canonicalize(candidate).await {
        Ok(canonical) => Ok((canonical, true)),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let (existing, missing) = nearest_existing(candidate).await?;
            let canonical = tokio::fs::canonicalize(&existing).await.map_err(|error| {
                PathIssue::Failure(format!(
                    "cannot resolve directory {}: {error}",
                    existing.display()
                ))
            })?;
            let mut resolved = canonical;
            for component in missing.into_iter().rev() {
                resolved.push(component);
            }
            Ok((resolved, false))
        }
        Err(error) => Err(PathIssue::Failure(format!(
            "cannot resolve path {}: {error}",
            candidate.display()
        ))),
    }
}

async fn nearest_existing(candidate: &Path) -> Result<(PathBuf, Vec<OsString>), PathIssue> {
    let mut missing: Vec<OsString> = Vec::new();
    let mut probe = candidate;

    loop {
        match tokio::fs::symlink_metadata(probe).await {
            Ok(_) => return Ok((probe.to_path_buf(), missing)),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(name) = probe.file_name() else {
                    return Err(PathIssue::Failure(format!(
                        "cannot resolve parent directory: {}",
                        candidate.display()
                    )));
                };
                missing.push(name.to_os_string());
                let Some(parent) = probe.parent() else {
                    return Err(PathIssue::Failure(format!(
                        "cannot resolve parent directory: {}",
                        candidate.display()
                    )));
                };
                probe = parent;
            }
            Err(error) => {
                return Err(PathIssue::Failure(format!(
                    "cannot inspect path {}: {error}",
                    probe.display()
                )));
            }
        }
    }
}

fn join(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(crate) async fn resolve_read_path_with_policy(
    workspace_root: &Path,
    readable_roots: &[PathBuf],
    raw: &str,
    allow_sensitive_files: bool,
) -> Result<PathBuf, PathIssue> {
    let workspace = canonicalize_or_keep(workspace_root).await;
    let candidate = join(&workspace, raw);
    let (resolved, exists) = locate(&candidate).await?;

    let mut readable = vec![workspace];
    for root in readable_roots {
        readable.push(canonicalize_or_keep(root).await);
    }

    if !readable.iter().any(|root| resolved.starts_with(root)) {
        return Err(PathIssue::Escape(format!("path escapes workspace: {raw}")));
    }

    if !allow_sensitive_files && (is_sensitive_path(&resolved) || is_sensitive_path(Path::new(raw)))
    {
        return Err(PathIssue::Denied(format!(
            "access to sensitive file denied: {raw}"
        )));
    }

    if exists {
        Ok(resolved)
    } else {
        Err(PathIssue::Missing(format!("path does not exist: {raw}")))
    }
}

pub(crate) async fn resolve_write_path_with_policy(
    workspace_root: &Path,
    writable_roots: &[PathBuf],
    raw: &str,
    allow_sensitive_files: bool,
) -> Result<PathBuf, PathIssue> {
    let workspace = canonicalize_or_keep(workspace_root).await;
    let candidate = join(&workspace, raw);
    let (resolved, _) = locate(&candidate).await?;

    let mut writable = vec![workspace];
    for root in writable_roots {
        writable.push(canonicalize_or_keep(root).await);
    }

    if !writable.iter().any(|root| resolved.starts_with(root)) {
        return Err(PathIssue::Escape(format!("path escapes workspace: {raw}")));
    }

    if !allow_sensitive_files && (is_sensitive_path(&resolved) || is_sensitive_path(Path::new(raw)))
    {
        return Err(PathIssue::Denied(format!(
            "access to sensitive file denied: {raw}"
        )));
    }

    Ok(resolved)
}

pub(crate) async fn canonicalize_or_keep(path: &Path) -> PathBuf {
    tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn display_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if !relative.as_os_str().is_empty() => flatten(relative),
        Ok(_) => ".".to_string(),
        Err(_) => flatten(path),
    }
}

pub(crate) fn relative_path(workspace: &Path, root: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(workspace)
        && !relative.as_os_str().is_empty()
    {
        return flatten(relative);
    }

    if let Ok(relative) = path.strip_prefix(root)
        && !relative.as_os_str().is_empty()
    {
        return flatten(relative);
    }

    flatten(path)
}

pub(crate) fn flatten(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use meerai_core::{ObservationStatus, ToolOutput, Toolset};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn is_sensitive_path_matches_env_and_key_and_secret_files() {
        for path in [
            ".env",
            ".ENV",
            "app/.env.local",
            ".env.production",
            ".envrc",
            "prod.env",
            "config/staging.ENV",
            "certs/private.pem",
            "certs/server.KEY",
            "keystore.p12",
            "cert.pfx",
            "trust.jks",
            "app.keystore",
            "id_rsa",
            "id_dsa",
            "id_ecdsa",
            "id_ed25519",
            ".netrc",
            "_netrc",
            ".npmrc",
            ".pypirc",
            "auth.json",
            "auth.yml",
            "credentials.json",
            "credentials",
            ".htpasswd",
            "config/secrets.yml",
            "config/secrets.yaml",
            "app_secret.toml",
            "service_credential.json",
            "client_secret",
            "/home/user/.aws/credentials",
            "C:\\Users\\dev\\.aws\\credentials",
            "/home/user/.docker/config.json",
            "/home/user/.kube/config",
            "/home/user/.ssh/id_rsa",
            "/home/user/.ssh/authorized_keys",
            ".ssh/custom_key",
        ] {
            assert!(
                is_sensitive_path(Path::new(path)),
                "expected sensitive: {path}"
            );
        }
    }

    #[test]
    fn is_sensitive_path_allows_templates_source_docs_and_public_ssh_files() {
        for path in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".ENV.EXAMPLE",
            "meerai-providers/src/secrets.rs",
            "src/credentials.py",
            "lib/secret_helper.ts",
            "docs/secrets.md",
            "notes/credential_flow.txt",
            "pkg/secrets.go",
            "src/main.rs",
            "Cargo.toml",
            "/home/user/.ssh/id_rsa.pub",
            "/home/user/.ssh/id_ed25519.pub",
            "/home/user/.ssh/known_hosts",
            "/home/user/.ssh/config",
            "not.kube/config",
            "not.ssh/authorized_keys",
        ] {
            assert!(
                !is_sensitive_path(Path::new(path)),
                "expected allowed: {path}"
            );
        }
    }

    #[tokio::test]
    async fn sensitive_files_are_refused_by_read_write_and_filtered_from_search() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join(".env"), "API_KEY=secret123\n").unwrap();
        fs::write(root.join(".env.example"), "API_KEY=placeholder\n").unwrap();
        fs::write(root.join("prod.env"), "TOKEN=secret123\n").unwrap();
        fs::write(root.join("secrets.yml"), "db_pass: secret123\n").unwrap();
        fs::write(root.join("credentials.json"), "{\"token\":\"secret123\"}\n").unwrap();
        fs::write(root.join("private.pem"), "SECRET PEM\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/secrets.rs"),
            "pub const TOKEN: &str = \"secret123\";\n",
        )
        .unwrap();
        fs::write(root.join("notes.txt"), "secret123\n").unwrap();

        let tools = CodeTools::new(root);

        for blocked in [
            ".env",
            ".env.local",
            "prod.env",
            "secrets.yml",
            "credentials.json",
            "private.pem",
        ] {
            let read_run = tools
                .invoke_structured("code-read_file", &json!({ "path": blocked }).to_string())
                .await
                .unwrap();
            assert_eq!(
                read_run.observation.status,
                ObservationStatus::Refused,
                "{blocked}"
            );

            let write_run = tools
                .invoke_structured(
                    "code-write_file",
                    &json!({ "path": blocked, "content": "x" }).to_string(),
                )
                .await
                .unwrap();
            assert_eq!(
                write_run.observation.status,
                ObservationStatus::Refused,
                "{blocked}"
            );

            let grep_direct = tools
                .invoke_structured(
                    "code-grep",
                    &json!({ "pattern": "secret", "path": blocked }).to_string(),
                )
                .await
                .unwrap();
            assert_eq!(
                grep_direct.observation.status,
                ObservationStatus::Refused,
                "{blocked}"
            );

            let glob_direct = tools
                .invoke_structured(
                    "code-glob",
                    &json!({ "pattern": "*", "path": blocked }).to_string(),
                )
                .await
                .unwrap();
            assert_eq!(
                glob_direct.observation.status,
                ObservationStatus::Refused,
                "{blocked}"
            );
        }

        let allowed_example = tools
            .invoke_structured(
                "code-read_file",
                &json!({ "path": ".env.example" }).to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            allowed_example.observation.status,
            ObservationStatus::Success
        );

        let allowed_rs = tools
            .invoke_structured(
                "code-read_file",
                &json!({ "path": "src/secrets.rs" }).to_string(),
            )
            .await
            .unwrap();
        assert_eq!(allowed_rs.observation.status, ObservationStatus::Success);

        let grep_all = tools
            .invoke_structured("code-grep", &json!({ "pattern": "secret123" }).to_string())
            .await
            .unwrap();
        let ToolOutput::Text(grep_text) = grep_all.output else {
            panic!("expected grep text output");
        };
        assert!(grep_text.contains("src/secrets.rs"));
        assert!(grep_text.contains("notes.txt"));
        assert!(!grep_text.contains(".env"));
        assert!(!grep_text.contains("prod.env"));
        assert!(!grep_text.contains("secrets.yml"));
        assert!(!grep_text.contains("credentials.json"));
        assert!(!grep_text.contains("private.pem"));

        let glob_all = tools
            .invoke_structured("code-glob", &json!({ "pattern": "**/*" }).to_string())
            .await
            .unwrap();
        let ToolOutput::Text(glob_text) = glob_all.output else {
            panic!("expected glob text output");
        };
        assert!(glob_text.contains("src/secrets.rs"));
        assert!(glob_text.contains("notes.txt"));
        assert!(!glob_text.contains("prod.env"));
        assert!(!glob_text.contains("secrets.yml"));
        assert!(!glob_text.contains("credentials.json"));
        assert!(!glob_text.contains("private.pem"));

        let list_all = tools
            .invoke_structured("code-list_dir", &json!({ "path": "." }).to_string())
            .await
            .unwrap();
        let ToolOutput::Text(list_text) = list_all.output else {
            panic!("expected list_dir text output");
        };
        assert!(list_text.contains("notes.txt"));
        assert!(!list_text.contains("prod.env"));
        assert!(!list_text.contains("secrets.yml"));
        assert!(!list_text.contains("credentials.json"));
        assert!(!list_text.contains("private.pem"));

        let permissive = CodeTools::new(root).with_allow_sensitive_files(true);
        assert!(permissive.allow_sensitive_files());
        let read_env = permissive
            .invoke_structured("code-read_file", &json!({ "path": ".env" }).to_string())
            .await
            .unwrap();
        assert_eq!(read_env.observation.status, ObservationStatus::Success);
    }
}

#[cfg(test)]
mod outside_root_tests {
    use std::fs;

    use meerai_core::{ObservationStatus, Toolset};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn outside_access_is_refused_until_a_root_is_granted() {
        let workspace = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let file = outside.path().join("note.txt");
        fs::write(&file, "outside").unwrap();
        let args = json!({ "path": file.to_string_lossy() }).to_string();

        let tools = CodeTools::new(workspace.path());
        let refused = tools
            .invoke_structured("code-read_file", &args)
            .await
            .unwrap();
        assert_eq!(refused.observation.status, ObservationStatus::Refused);

        tools.grant_path_root(outside.path());
        let allowed = tools
            .invoke_structured("code-read_file", &args)
            .await
            .unwrap();
        assert_eq!(allowed.observation.status, ObservationStatus::Success);
        let text = match &allowed.output {
            meerai_core::ToolOutput::Text(text) | meerai_core::ToolOutput::Fail(text) => text,
            meerai_core::ToolOutput::Stop(reason) => reason,
        };
        assert!(text.contains("outside"));
    }
}
