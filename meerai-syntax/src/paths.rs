use crate::language::Language;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRef {
    pub path: String,
    pub line: Option<u32>,
}

const MAX_PATH_LEN: usize = 240;

pub fn detect_path_reference(raw: &str) -> Option<PathRef> {
    let trimmed = raw.trim().trim_end_matches([':', ',', ';']);
    let trimmed = trimmed.strip_suffix(')').unwrap_or(trimmed);
    let trimmed = trimmed.trim_end_matches('.');
    if trimmed.is_empty() || trimmed.len() > MAX_PATH_LEN {
        return None;
    }
    if trimmed.contains(char::is_whitespace) || trimmed.contains(['(', ')', '[', ']', '<', '>']) {
        return None;
    }
    let (body, line) = split_line_suffix(trimmed);
    if body.is_empty() || !looks_like_path(body) {
        return None;
    }
    if Language::from_path(body).is_none() {
        return None;
    }
    Some(PathRef {
        path: body.to_string(),
        line,
    })
}

pub fn format_path_reference(reference: &PathRef) -> String {
    match reference.line {
        Some(line) => format!("{}:{line}", reference.path),
        None => reference.path.clone(),
    }
}

fn split_line_suffix(raw: &str) -> (&str, Option<u32>) {
    let mut body = raw;
    let mut lines = Vec::new();
    while let Some((head, tail)) = body.rsplit_once(':') {
        if tail.is_empty() || !tail.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let Ok(parsed) = tail.parse::<u32>() else {
            break;
        };
        lines.push(parsed);
        body = head;
    }
    let line = lines
        .into_iter()
        .rev()
        .find(|line| *line > 0)
        .filter(|_| body.contains('/') || body.contains('.'));
    (body, line.map(|line| line))
}

fn looks_like_path(raw: &str) -> bool {
    if raw.starts_with('/') || raw.starts_with("./") || raw.starts_with("../") {
        return !raw.contains("://");
    }
    if raw.contains("://") {
        return false;
    }
    let has_separator = raw.contains('/');
    let has_extension = raw.rsplit('/').next().is_some_and(|name| {
        name.rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
    });
    has_separator || has_extension
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(raw: &str) -> Option<(String, Option<u32>)> {
        detect_path_reference(raw).map(|reference| (reference.path, reference.line))
    }

    #[test]
    fn detects_a_bare_relative_path() {
        assert_eq!(detect("src/main.rs"), Some(("src/main.rs".into(), None)));
    }

    #[test]
    fn detects_a_path_with_a_line_number() {
        assert_eq!(
            detect("meerai-providers/src/oauth/callback.rs:325"),
            Some(("meerai-providers/src/oauth/callback.rs".into(), Some(325)))
        );
    }

    #[test]
    fn detects_a_path_with_a_line_range() {
        assert_eq!(
            detect("src/app.rs:120:140"),
            Some(("src/app.rs".into(), Some(120)))
        );
    }

    #[test]
    fn detects_absolute_and_parent_paths() {
        assert_eq!(
            detect("/etc/app.toml"),
            Some(("/etc/app.toml".into(), None))
        );
        assert_eq!(
            detect("../lib/mod.ts"),
            Some(("../lib/mod.ts".into(), None))
        );
        assert_eq!(
            detect("./docs/readme.md"),
            Some(("./docs/readme.md".into(), None))
        );
    }

    #[test]
    fn strips_trailing_prose_punctuation() {
        assert_eq!(detect("src/main.rs."), Some(("src/main.rs".into(), None)));
        assert_eq!(
            detect("src/main.rs:12,"),
            Some(("src/main.rs".into(), Some(12)))
        );
        assert_eq!(detect("(src/main.rs)"), None);
    }

    #[test]
    fn rejects_urls_and_plain_words() {
        assert_eq!(detect("https://example.com/a.rs"), None);
        assert_eq!(detect("http://127.0.0.1:1455/auth/callback"), None);
        assert_eq!(detect("memory-search"), None);
        assert_eq!(detect("cargo"), None);
        assert_eq!(detect(""), None);
    }

    #[test]
    fn rejects_prose_and_sentences() {
        assert_eq!(detect("see src/main.rs for details"), None);
        assert_eq!(detect("a b c"), None);
    }

    #[test]
    fn rejects_unknown_extensions_and_clock_values() {
        assert_eq!(detect("12:30"), None);
        assert_eq!(detect("0"), None);
        assert_eq!(detect("archive.unknownext"), None);
    }

    #[test]
    fn formats_back_to_the_display_form() {
        let reference = detect_path_reference("src/lib.rs:44").unwrap();
        assert_eq!(format_path_reference(&reference), "src/lib.rs:44");
        let bare = detect_path_reference("src/lib.rs").unwrap();
        assert_eq!(format_path_reference(&bare), "src/lib.rs");
    }
}
