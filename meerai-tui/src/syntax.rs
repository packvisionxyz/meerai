use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Function,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub kind: TokenKind,
}

pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "sh" | "bash" | "zsh" => "shell",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "sql" => "sql",
        "rb" => "ruby",
        "md" | "markdown" => "markdown",
        _ => return None,
    })
}

fn keywords(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        "python" => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "True", "try", "while", "with", "yield",
        ],
        "javascript" | "typescript" => &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "null",
            "of",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "var",
            "void",
            "while",
            "yield",
        ],
        "go" => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "false",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "nil",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "true",
            "type",
            "var",
        ],
        "java" => &[
            "abstract",
            "boolean",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "else",
            "enum",
            "extends",
            "false",
            "final",
            "finally",
            "for",
            "if",
            "implements",
            "import",
            "instanceof",
            "interface",
            "new",
            "null",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "throws",
            "true",
            "try",
            "void",
            "while",
        ],
        "c" | "cpp" => &[
            "auto",
            "bool",
            "break",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "double",
            "else",
            "enum",
            "explicit",
            "extern",
            "false",
            "float",
            "for",
            "friend",
            "if",
            "inline",
            "int",
            "long",
            "namespace",
            "new",
            "nullptr",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "signed",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "typename",
            "union",
            "unsigned",
            "using",
            "virtual",
            "void",
            "volatile",
            "while",
        ],
        "shell" => &[
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if",
            "in", "local", "readonly", "return", "then", "until", "while",
        ],
        "ruby" => &[
            "alias", "begin", "break", "case", "class", "def", "do", "else", "elsif", "end",
            "ensure", "false", "for", "if", "in", "module", "nil", "raise", "rescue", "return",
            "self", "super", "then", "true", "unless", "until", "when", "while", "yield",
        ],
        "json" => &["false", "null", "true"],
        "yaml" | "toml" => &["false", "null", "true"],
        "sql" => &[
            "AND", "AS", "ASC", "BY", "CREATE", "DELETE", "DESC", "DROP", "FROM", "GROUP",
            "HAVING", "IN", "INSERT", "INTO", "JOIN", "LIMIT", "NOT", "NULL", "ON", "OR", "ORDER",
            "SELECT", "SET", "TABLE", "UPDATE", "VALUES", "WHERE",
        ],
        _ => &[],
    }
}

fn comment_prefix(language: &str) -> &'static str {
    match language {
        "python" | "shell" | "yaml" | "toml" | "ruby" | "markdown" => "#",
        "sql" => "--",
        _ => "//",
    }
}

pub fn tokenize(language: &str, line: &str) -> Vec<Token> {
    if line.is_empty() {
        return vec![Token {
            text: String::new(),
            kind: TokenKind::Plain,
        }];
    }
    let comment = comment_prefix(language);
    let keywords = keywords(language);
    let mut out: Vec<Token> = Vec::new();
    let mut index = 0usize;

    while index < line.len() {
        let rest = &line[index..];
        let ch = rest.chars().next().unwrap_or(' ');
        if rest.starts_with(comment) {
            push_token(&mut out, rest, TokenKind::Comment);
            break;
        }
        if ch == '"' || ch == '\'' || ch == '`' {
            let mut end = index + ch.len_utf8();
            let mut escaped = false;
            let mut closed = false;
            for next in line[end..].chars() {
                end += next.len_utf8();
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == ch {
                    closed = true;
                    break;
                }
            }
            let end = if closed { end } else { line.len() };
            push_token(&mut out, &line[index..end], TokenKind::String);
            index = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let mut end = index + ch.len_utf8();
            for next in line[end..].chars() {
                if next.is_ascii_alphanumeric() || next == '_' || next == '.' {
                    end += next.len_utf8();
                } else {
                    break;
                }
            }
            push_token(&mut out, &line[index..end], TokenKind::Number);
            index = end;
            continue;
        }
        if ch.is_alphanumeric() || ch == '_' {
            let mut end = index + ch.len_utf8();
            for next in line[end..].chars() {
                if next.is_alphanumeric() || next == '_' {
                    end += next.len_utf8();
                } else {
                    break;
                }
            }
            let word = &line[index..end];
            let kind = if keywords.contains(&word) {
                TokenKind::Keyword
            } else if line[end..].trim_start().starts_with('(') {
                TokenKind::Function
            } else {
                TokenKind::Plain
            };
            push_token(&mut out, word, kind);
            index = end;
            continue;
        }
        push_token(
            &mut out,
            &line[index..index + ch.len_utf8()],
            TokenKind::Plain,
        );
        index += ch.len_utf8();
    }

    if out.is_empty() {
        out.push(Token {
            text: line.to_string(),
            kind: TokenKind::Plain,
        });
    }
    out
}

fn push_token(out: &mut Vec<Token>, text: &str, kind: TokenKind) {
    if let Some(last) = out.last_mut()
        && last.kind == kind
    {
        last.text.push_str(text);
        return;
    }
    out.push(Token {
        text: text.to_string(),
        kind,
    });
}

fn token_style(kind: TokenKind, base: Style) -> Style {
    match kind {
        TokenKind::Plain => base,
        TokenKind::Keyword => Style::default()
            .fg(theme::p().accent)
            .add_modifier(Modifier::BOLD),
        TokenKind::String => Style::default().fg(theme::p().success),
        TokenKind::Number => Style::default().fg(theme::p().info),
        TokenKind::Comment => Style::default()
            .fg(theme::p().faint)
            .add_modifier(Modifier::ITALIC),
        TokenKind::Function => Style::default().fg(theme::p().interactive),
    }
}

pub fn highlight(language: Option<&str>, code: &str, base: Style) -> Vec<Span<'static>> {
    let Some(language) = language else {
        return vec![Span::styled(code.to_string(), base)];
    };
    tokenize(language, code)
        .into_iter()
        .map(|token| Span::styled(token.text, token_style(token.kind, base)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_extensions_to_languages() {
        assert_eq!(language_for_path("src/main.rs"), Some("rust"));
        assert_eq!(language_for_path("a/b/script.py"), Some("python"));
        assert_eq!(language_for_path("web/app.tsx"), Some("typescript"));
        assert_eq!(language_for_path("Makefile"), None);
    }

    #[test]
    fn tokenizes_comments_strings_and_keywords() {
        let tokens = tokenize("rust", "let name = \"x\"; // note");
        let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::String));
        assert!(kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn highlights_plain_text_without_language() {
        let spans = highlight(None, "let x = 1;", Style::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "let x = 1;");
    }
}
