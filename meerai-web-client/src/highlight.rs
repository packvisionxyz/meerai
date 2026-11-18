#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Comment,
    String,
    Number,
    Keyword,
    Function,
    Type,
}

impl HighlightKind {
    pub fn class(self) -> &'static str {
        match self {
            Self::Comment => "tok-comment",
            Self::String => "tok-string",
            Self::Number => "tok-number",
            Self::Keyword => "tok-keyword",
            Self::Function => "tok-function",
            Self::Type => "tok-type",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: HighlightKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    JsTs,
    Python,
    Go,
    Shell,
    Json,
    YamlToml,
    Sql,
    Generic,
}

impl Lang {
    fn from_hint(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        match lower.as_str() {
            "rs" | "rust" => Self::Rust,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "javascript" | "typescript" => Self::JsTs,
            "py" | "python" => Self::Python,
            "go" | "golang" => Self::Go,
            "sh" | "bash" | "zsh" | "shell" => Self::Shell,
            "json" | "jsonc" => Self::Json,
            "yml" | "yaml" | "toml" => Self::YamlToml,
            "sql" => Self::Sql,
            _ => Self::Generic,
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            Self::JsTs => &[
                "as",
                "async",
                "await",
                "break",
                "case",
                "catch",
                "class",
                "const",
                "continue",
                "default",
                "delete",
                "do",
                "else",
                "enum",
                "export",
                "extends",
                "false",
                "finally",
                "for",
                "from",
                "function",
                "if",
                "implements",
                "import",
                "in",
                "instanceof",
                "interface",
                "let",
                "new",
                "null",
                "of",
                "private",
                "public",
                "readonly",
                "return",
                "static",
                "switch",
                "this",
                "throw",
                "true",
                "try",
                "type",
                "typeof",
                "undefined",
                "var",
                "void",
                "while",
                "yield",
            ],
            Self::Python => &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if",
                "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise",
                "return", "True", "try", "while", "with", "yield",
            ],
            Self::Go => &[
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
            Self::Shell => &[
                "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function",
                "if", "in", "local", "readonly", "return", "set", "then", "unset", "until",
                "while",
            ],
            Self::Json | Self::YamlToml => &["true", "false", "null"],
            Self::Sql => &[
                "ALL", "AND", "AS", "ASC", "BY", "CREATE", "DELETE", "DESC", "DISTINCT", "DROP",
                "FROM", "GROUP", "HAVING", "IN", "INDEX", "INNER", "INSERT", "INTO", "IS", "JOIN",
                "LEFT", "LIMIT", "NOT", "NULL", "ON", "OR", "ORDER", "PRIMARY", "SELECT", "SET",
                "TABLE", "UNION", "UPDATE", "VALUES", "WHERE", "all", "and", "as", "by", "create",
                "delete", "from", "group", "in", "insert", "into", "is", "join", "limit", "not",
                "null", "on", "or", "order", "select", "set", "table", "update", "values", "where",
            ],
            Self::Generic => &[
                "async", "await", "break", "class", "const", "continue", "else", "enum", "false",
                "fn", "for", "func", "function", "if", "impl", "import", "in", "let", "match",
                "null", "pub", "return", "struct", "true", "type", "use", "var", "while",
            ],
        }
    }
}

pub fn highlight_code(lang_hint: &str, code: &str) -> Vec<HighlightSpan> {
    let lang = Lang::from_hint(lang_hint);
    let keywords = lang.keywords();
    let bytes = code.as_bytes();
    let len = bytes.len();
    let mut spans = Vec::new();
    let mut i = 0usize;
    let hash_comments = matches!(
        lang,
        Lang::Python | Lang::Shell | Lang::YamlToml | Lang::Generic
    );
    let slash_comments = matches!(
        lang,
        Lang::Rust | Lang::JsTs | Lang::Go | Lang::Json | Lang::Generic
    );

    while i < len {
        let b = bytes[i];

        if slash_comments && i + 1 < len && b == b'/' && bytes[i + 1] == b'/' {
            let start = i;
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            spans.push(span(start, i, HighlightKind::Comment));
            continue;
        }

        if slash_comments && i + 1 < len && b == b'/' && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            spans.push(span(start, i, HighlightKind::Comment));
            continue;
        }

        if hash_comments && b == b'#' {
            let start = i;
            i += 1;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            spans.push(span(start, i, HighlightKind::Comment));
            continue;
        }

        if b == b'"' || b == b'\'' || (b == b'`' && lang == Lang::JsTs) {
            let quote = b;
            let start = i;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                if bytes[i] == b'\n' && lang == Lang::Json {
                    break;
                }
                i += 1;
            }
            spans.push(span(start, i, HighlightKind::String));
            continue;
        }

        if b.is_ascii_digit() && (i == 0 || !is_ident_continue(bytes[i - 1])) {
            let start = i;
            i += 1;
            while i < len
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            spans.push(span(start, i, HighlightKind::Number));
            continue;
        }

        if is_ident_start(b) {
            let start = i;
            i += 1;
            while i < len && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let word = &code[start..i];
            if keywords.contains(&word) {
                spans.push(span(start, i, HighlightKind::Keyword));
            } else if i < len && bytes[i] == b'(' {
                spans.push(span(start, i, HighlightKind::Function));
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                spans.push(span(start, i, HighlightKind::Type));
            }
            continue;
        }

        i += code[i..].chars().next().map_or(1, char::len_utf8);
    }

    spans
}

fn span(start: usize, end: usize, kind: HighlightKind) -> HighlightSpan {
    HighlightSpan { start, end, kind }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
