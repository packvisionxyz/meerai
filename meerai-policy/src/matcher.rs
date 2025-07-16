use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use crate::rule::{ActionSelector, PermissionAction};

pub(crate) const MATCH_ALL: &str = "*";

const ACTIONS: [PermissionAction; 6] = [
    PermissionAction::Read,
    PermissionAction::Edit,
    PermissionAction::Shell,
    PermissionAction::Subagent,
    PermissionAction::WebFetch,
    PermissionAction::ExternalDirectory,
];

pub(crate) fn is_path_action(action: PermissionAction) -> bool {
    matches!(
        action,
        PermissionAction::Read | PermissionAction::Edit | PermissionAction::ExternalDirectory
    )
}

#[derive(Debug)]
pub(crate) struct Candidate<'a> {
    action: PermissionAction,
    raw: &'a str,
    path: String,
}

impl<'a> Candidate<'a> {
    pub(crate) fn new(action: PermissionAction, resource: &'a str, workspace_root: &Path) -> Self {
        let path = if is_path_action(action) {
            normalize_path(workspace_root, resource)
        } else {
            resource.to_string()
        };
        Self {
            action,
            raw: resource,
            path,
        }
    }

    pub(crate) fn action(&self) -> PermissionAction {
        self.action
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn basename(&self) -> &str {
        match self.path.rsplit_once('/') {
            Some((_, basename)) => basename,
            None => &self.path,
        }
    }

    fn text(&self) -> &str {
        self.raw
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResourcePattern {
    pattern: String,
    path_glob: Option<GlobMatcher>,
    basename_glob: Option<GlobMatcher>,
    text_glob: Option<GlobMatcher>,
}

impl ResourcePattern {
    pub(crate) fn compile(
        selector: ActionSelector,
        workspace_root: &Path,
        pattern: &str,
    ) -> Result<Self, String> {
        let mut compiled = Self {
            pattern: pattern.to_string(),
            path_glob: None,
            basename_glob: None,
            text_glob: None,
        };

        if pattern == MATCH_ALL {
            return Ok(compiled);
        }

        let mut needs_path = false;
        let mut needs_text = false;
        for action in ACTIONS {
            if selector.matches(action) {
                if is_path_action(action) {
                    needs_path = true;
                } else {
                    needs_text = true;
                }
            }
        }

        if needs_path {
            let normalized = normalize_path(workspace_root, pattern);
            if normalized.contains('/') {
                compiled.path_glob = Some(compile_glob(&normalized, true)?);
            } else {
                compiled.basename_glob = Some(compile_glob(&normalized, true)?);
            }
        }
        if needs_text {
            compiled.text_glob = Some(compile_glob(pattern, false)?);
        }

        Ok(compiled)
    }

    pub(crate) fn matches(&self, candidate: &Candidate<'_>) -> bool {
        if self.pattern == MATCH_ALL {
            return true;
        }
        if is_path_action(candidate.action()) {
            if let Some(glob) = &self.path_glob {
                return glob.is_match(candidate.path());
            }
            return self
                .basename_glob
                .as_ref()
                .is_some_and(|glob| glob.is_match(candidate.basename()));
        }
        self.text_glob
            .as_ref()
            .is_some_and(|glob| glob.is_match(candidate.text()))
    }
}

fn compile_glob(pattern: &str, literal_separator: bool) -> Result<GlobMatcher, String> {
    GlobBuilder::new(pattern)
        .literal_separator(literal_separator)
        .backslash_escape(false)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| format!("invalid glob pattern {pattern:?}: {error}"))
}

pub(crate) fn normalize_path(workspace_root: &Path, resource: &str) -> String {
    let expanded = expand_home(resource);
    let path = PathBuf::from(&expanded);
    let stripped = path
        .strip_prefix(workspace_root)
        .map(Path::to_path_buf)
        .unwrap_or(path);
    let normalized = stripped.to_string_lossy().replace('\\', "/");
    match normalized.strip_prefix("./") {
        Some(rest) => rest.to_string(),
        None => normalized,
    }
}

fn expand_home(resource: &str) -> String {
    if resource == "~" {
        return home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_else(|| resource.to_string());
    }
    match resource.strip_prefix("~/") {
        Some(rest) => home_dir()
            .map(|home| home.join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| resource.to_string()),
        None => resource.to_string(),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_candidate(root: &str, resource: &'static str) -> Candidate<'static> {
        Candidate::new(PermissionAction::Read, resource, Path::new(root))
    }

    fn shell_candidate(resource: &'static str) -> Candidate<'static> {
        Candidate::new(PermissionAction::Shell, resource, Path::new("."))
    }

    fn path_pattern(root: &str, pattern: &str) -> ResourcePattern {
        ResourcePattern::compile(
            ActionSelector::Action(PermissionAction::Read),
            Path::new(root),
            pattern,
        )
        .unwrap()
    }

    fn shell_pattern(pattern: &str) -> ResourcePattern {
        ResourcePattern::compile(
            ActionSelector::Action(PermissionAction::Shell),
            Path::new("."),
            pattern,
        )
        .unwrap()
    }

    #[test]
    fn star_does_not_cross_directories() {
        let pattern = path_pattern(".", "src/*.rs");
        assert!(pattern.matches(&path_candidate(".", "src/main.rs")));
        assert!(!pattern.matches(&path_candidate(".", "src/nested/main.rs")));
    }

    #[test]
    fn double_star_crosses_directories() {
        let pattern = path_pattern(".", "src/**");
        assert!(pattern.matches(&path_candidate(".", "src/main.rs")));
        assert!(pattern.matches(&path_candidate(".", "src/nested/deep/main.rs")));
        assert!(!pattern.matches(&path_candidate(".", "tests/main.rs")));
    }

    #[test]
    fn workspace_root_prefix_is_stripped() {
        let pattern = path_pattern("/tmp/ws", "src/**");
        assert!(pattern.matches(&path_candidate("/tmp/ws", "/tmp/ws/src/main.rs")));
        assert!(!pattern.matches(&path_candidate("/tmp/ws", "/elsewhere/src/main.rs")));
    }

    #[test]
    fn dot_slash_prefix_is_removed() {
        assert_eq!(
            normalize_path(Path::new("."), "./src/main.rs"),
            "src/main.rs"
        );
        assert_eq!(normalize_path(Path::new("."), "src/main.rs"), "src/main.rs");
    }

    #[test]
    fn home_prefix_is_expanded() {
        let home = home_dir().map(|home| home.to_string_lossy().into_owned());
        let Some(home) = home else {
            return;
        };
        assert_eq!(
            normalize_path(Path::new("."), "~/secrets/key"),
            format!("{home}/secrets/key")
        );
    }

    #[test]
    fn wildcard_matches_everything() {
        let pattern = path_pattern(".", MATCH_ALL);
        assert!(pattern.matches(&path_candidate(".", "anywhere/at/all.env")));
    }

    #[test]
    fn shell_star_crosses_slashes() {
        let pattern = shell_pattern("cargo *");
        assert!(pattern.matches(&shell_candidate("cargo test -p x")));
        assert!(pattern.matches(&shell_candidate("cargo run -- src/main.rs")));
        assert!(!pattern.matches(&shell_candidate("cargo")));
    }

    #[test]
    fn basename_patterns_match_at_any_depth() {
        let pattern = path_pattern(".", "*.env*");
        assert!(pattern.matches(&path_candidate(".", ".env")));
        assert!(pattern.matches(&path_candidate(".", "config/prod.env")));
        assert!(pattern.matches(&path_candidate(".", "a/b/c/.env.local")));
        assert!(!pattern.matches(&path_candidate(".", "a/b/c/env.txt")));
    }

    #[test]
    fn slash_patterns_match_the_relative_path_only() {
        let pattern = path_pattern(".", "config/*.env*");
        assert!(pattern.matches(&path_candidate(".", "config/prod.env")));
        assert!(!pattern.matches(&path_candidate(".", ".env")));
        assert!(!pattern.matches(&path_candidate(".", "a/b/config/prod.env")));
    }

    #[test]
    fn recursive_prefix_matches_root_level_names() {
        let pattern = path_pattern(".", "**/*.env*");
        assert!(pattern.matches(&path_candidate(".", ".env")));
        assert!(pattern.matches(&path_candidate(".", "config/prod.env")));
    }

    #[test]
    fn invalid_glob_is_rejected() {
        assert!(
            ResourcePattern::compile(
                ActionSelector::Action(PermissionAction::Read),
                Path::new("."),
                "src/[",
            )
            .is_err()
        );
    }
}
