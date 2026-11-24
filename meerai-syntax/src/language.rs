#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Bash,
    C,
    CSharp,
    Cpp,
    Css,
    Go,
    Haskell,
    Html,
    Java,
    JavaScript,
    Json,
    Kotlin,
    Markdown,
    Php,
    Python,
    Ruby,
    Rust,
    Scala,
    Sql,
    Swift,
    Toml,
    Tsx,
    TypeScript,
    Yaml,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Grammar {
    Bash,
    C,
    CSharp,
    Cpp,
    Css,
    Go,
    Haskell,
    Html,
    Java,
    JavaScript,
    Json,
    Kotlin,
    Php,
    Python,
    Ruby,
    Rust,
    Scala,
    Swift,
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
            Self::C => Grammar::C,
            Self::CSharp => Grammar::CSharp,
            Self::Cpp => Grammar::Cpp,
            Self::Css => Grammar::Css,
            Self::Go => Grammar::Go,
            Self::Haskell => Grammar::Haskell,
            Self::Html => Grammar::Html,
            Self::Java => Grammar::Java,
            Self::JavaScript => Grammar::JavaScript,
            Self::Json => Grammar::Json,
            Self::Kotlin => Grammar::Kotlin,
            Self::Php => Grammar::Php,
            Self::Python => Grammar::Python,
            Self::Ruby => Grammar::Ruby,
            Self::Rust => Grammar::Rust,
            Self::Scala => Grammar::Scala,
            Self::Swift => Grammar::Swift,
            Self::Tsx => Grammar::Tsx,
            Self::TypeScript => Grammar::TypeScript,
            Self::Markdown | Self::Sql | Self::Toml | Self::Yaml => return None,
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
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Self::Cpp,
            "cs" | "csx" => Self::CSharp,
            "kt" | "kts" => Self::Kotlin,
            "scala" | "sc" => Self::Scala,
            "swift" => Self::Swift,
            "php" | "phtml" => Self::Php,
            "hs" | "lhs" => Self::Haskell,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "sql" => Self::Sql,
            "rb" | "rake" | "gemspec" => Self::Ruby,
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
            "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hxx" => Self::Cpp,
            "csharp" | "c#" | "cs" | "dotnet" => Self::CSharp,
            "kotlin" | "kt" | "kts" => Self::Kotlin,
            "scala" | "sc" => Self::Scala,
            "swift" => Self::Swift,
            "php" => Self::Php,
            "haskell" | "hs" => Self::Haskell,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "sql" => Self::Sql,
            "ruby" | "rb" => Self::Ruby,
            "markdown" | "md" => Self::Markdown,
            _ => return None,
        })
    }
}
