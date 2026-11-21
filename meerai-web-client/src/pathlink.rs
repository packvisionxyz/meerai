pub fn path_command(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() || text.contains(char::is_whitespace) || text.contains("://") {
        return None;
    }
    if text.starts_with('/') || text.split('/').any(|part| part == "..") {
        return None;
    }
    let name = text.rsplit('/').next().unwrap_or(text);
    let stem_and_ext = match name.rsplit_once(':') {
        Some((head, suffix)) if suffix.chars().all(|ch| ch.is_ascii_digit()) => head,
        _ => name,
    };
    let (stem, extension) = stem_and_ext.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() {
        return None;
    }
    let known = matches!(
        extension,
        "rs" | "toml"
            | "md"
            | "json"
            | "yml"
            | "yaml"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "css"
            | "html"
            | "sh"
            | "sql"
            | "lock"
    );
    if !known {
        return None;
    }
    Some(format!("/open {text}"))
}

#[cfg(test)]
mod tests {
    use super::path_command;

    #[test]
    fn clicks_a_workspace_path() {
        assert_eq!(
            path_command("src/main.rs"),
            Some("/open src/main.rs".to_string())
        );
        assert_eq!(
            path_command("meerai-tui/src/app/mod.rs:530"),
            Some("/open meerai-tui/src/app/mod.rs:530".to_string())
        );
    }

    #[test]
    fn ignores_words_urls_and_unknown_extensions() {
        assert_eq!(path_command("cargo"), None);
        assert_eq!(path_command("memory-search"), None);
        assert_eq!(path_command("https://example.com/a.rs"), None);
        assert_eq!(path_command("archive.unknownext"), None);
        assert_eq!(path_command(""), None);
    }

    #[test]
    fn ignores_absolute_and_escaping_paths() {
        assert_eq!(path_command("/etc/passwd"), None);
        assert_eq!(path_command("../outside.rs"), None);
        assert_eq!(path_command("src/../../main.rs"), None);
    }
}
