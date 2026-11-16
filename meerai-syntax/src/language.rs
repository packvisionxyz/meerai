#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Bash,
    C,
    Cpp,
    Css,
    Go,
    Html,
    Java,
    JavaScript,
    Json,
    Markdown,
    Python,
    Ruby,
    Rust,
    Sql,
    Toml,
    Tsx,
    TypeScript,
    Yaml,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Grammar {
    Bash,
    Css,
    Go,
    Html,
    JavaScript,
    Json,
    Python,
    Rust,
    Tsx,
    TypeScript,
}

impl Language {
    pub fn from_path(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit('.').next()?.to_ascii_lowercase();
        Self::from_extension(&ext)
    }

    pub fn from_fence(info: &str) -> Option<Self> {
        let name = info
            .trim()
            .split([' ', '\t', ',', '{'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        Self::from_alias(&name)
    }

    pub(crate) fn grammar(self) -> Option<Grammar> {
        Some(match self {
            Self::Bash => Grammar::Bash,
            Self::Css => Grammar::Css,
            Self::Go => Grammar::Go,
            Self::Html => Grammar::Html,
            Self::JavaScript => Grammar::JavaScript,
            Self::Json => Grammar::Json,
            Self::Python => Grammar::Python,
            Self::Rust => Grammar::Rust,
            Self::Tsx => Grammar::Tsx,
            Self::TypeScript => Grammar::TypeScript,
            Self::C
            | Self::Cpp
            | Self::Java
            | Self::Markdown
            | Self::Ruby
            | Self::Sql
            | Self::Toml
            | Self::Yaml => return None,
        })
    }

    fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext {
            "rs" => Self::Rust,
            "py" | "pyi" => Self::Python,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "go" => Self::Go,
            "sh" | "bash" | "zsh" => Self::Bash,
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            "json" | "jsonc" => Self::Json,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Self::Cpp,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "sql" => Self::Sql,
            "rb" => Self::Ruby,
            "md" | "markdown" => Self::Markdown,
            _ => return None,
        })
    }

    fn from_alias(alias: &str) -> Option<Self> {
        Some(match alias {
            "rust" | "rs" => Self::Rust,
            "python" | "py" => Self::Python,
            "javascript" | "js" | "jsx" | "mjs" | "cjs" | "node" => Self::JavaScript,
            "typescript" | "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "go" => Self::Go,
            "shell" | "sh" | "bash" | "zsh" | "console" => Self::Bash,
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            "json" | "jsonc" => Self::Json,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" => Self::Cpp,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "sql" => Self::Sql,
            "ruby" | "rb" => Self::Ruby,
            "markdown" | "md" => Self::Markdown,
            _ => return None,
        })
    }
}
