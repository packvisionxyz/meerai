//! Syntax highlighting for meerai: tree-sitter grammars plus a lexical fallback.

mod language;
mod lex;
mod paths;
mod tree;

pub use language::Language;
pub use paths::{PathRef, detect_path_reference, format_path_reference};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub kind: TokenKind,
}

pub fn language_for_path(path: &str) -> Option<Language> {
    Language::from_path(path)
}

pub fn language_for_fence(info: &str) -> Option<Language> {
    Language::from_fence(info)
}

pub fn highlight_block(language: Option<Language>, code: &str) -> Vec<Vec<Token>> {
    let Some(language) = language else {
        return code.split('\n').map(plain_line).collect();
    };
    match language.grammar() {
        Some(grammar) => tree::highlight(grammar, language, code),
        None => lex::highlight(language, code),
    }
}

fn plain_line(line: &str) -> Vec<Token> {
    vec![Token {
        text: line.to_string(),
        kind: TokenKind::Plain,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(tokens: &[Token]) -> String {
        tokens.iter().map(|token| token.text.as_str()).collect()
    }

    fn assert_aligned(language: Language, code: &str) {
        let lines = highlight_block(Some(language), code);
        let expected: Vec<&str> = code.split('\n').collect();
        assert_eq!(lines.len(), expected.len(), "{language:?} line count");
        for (tokens, source) in lines.iter().zip(expected) {
            assert_eq!(line_text(tokens), source, "{language:?} line text");
        }
    }

    #[test]
    fn maps_extensions_to_languages() {
        assert_eq!(language_for_path("src/main.rs"), Some(Language::Rust));
        assert_eq!(language_for_path("a/b/script.py"), Some(Language::Python));
        assert_eq!(language_for_path("web/app.tsx"), Some(Language::Tsx));
        assert_eq!(language_for_path("web/app.ts"), Some(Language::TypeScript));
        assert_eq!(language_for_path("site/index.html"), Some(Language::Html));
        assert_eq!(language_for_path("site/style.css"), Some(Language::Css));
        assert_eq!(language_for_path("data.json"), Some(Language::Json));
        assert_eq!(language_for_path("Makefile"), None);
    }

    #[test]
    fn maps_fence_languages_and_aliases() {
        assert_eq!(language_for_fence("sh"), Some(Language::Bash));
        assert_eq!(language_for_fence("rust,ignore"), Some(Language::Rust));
        assert_eq!(language_for_fence("js title=x"), Some(Language::JavaScript));
        assert_eq!(language_for_fence("html"), Some(Language::Html));
        assert_eq!(language_for_fence("css"), Some(Language::Css));
        assert_eq!(language_for_fence("json"), Some(Language::Json));
        assert_eq!(language_for_fence("text"), None);
        assert_eq!(language_for_fence(""), None);
    }

    #[test]
    fn keeps_every_line_of_each_language() {
        for (language, code) in [
            (Language::Rust, "fn main() {\n    let x = 1;\n}"),
            (Language::Python, "def run():\n    return 1"),
            (Language::JavaScript, "const x = `a\nb`;\n"),
            (Language::TypeScript, "const x: number = 1;"),
            (Language::Tsx, "const el = <div />;"),
            (Language::Go, "func main() {\n\tprintln(\"x\")\n}"),
            (Language::Bash, "if true; then\n  cargo test\nfi"),
            (Language::Html, "<div class=\"a\">\ntext\n</div>"),
            (Language::Css, ".a {\n  color: red;\n}"),
            (Language::Json, "{\n  \"a\": [1, 2]\n}"),
            (Language::Java, "class A {}"),
            (Language::C, "int main() { return 0; }"),
            (Language::Cpp, "int main() { return 0; }"),
            (Language::Yaml, "key: value"),
            (Language::Toml, "key = 1"),
            (Language::Sql, "select 1"),
            (Language::Ruby, "puts 1"),
            (Language::Markdown, "# title"),
        ] {
            assert_aligned(language, code);
        }
    }

    #[test]
    fn plain_text_without_language_stays_plain() {
        let lines = highlight_block(None, "let x = 1;\nsecond");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].kind, TokenKind::Plain);
        assert_eq!(lines[0][0].text, "let x = 1;");
    }

    #[test]
    fn lexical_languages_mark_keywords_and_functions() {
        let lines = highlight_block(Some(Language::Java), "public void run() {}");
        let kinds: Vec<TokenKind> = lines[0].iter().map(|token| token.kind).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Function));
    }
}
