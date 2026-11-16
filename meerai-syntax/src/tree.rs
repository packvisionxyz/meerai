use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use tree_sitter::{Node, Parser, Tree};

use crate::language::Grammar;
use crate::lex;
use crate::{Language, Token, TokenKind};

const STRING_KINDS: &[&str] = &[
    "ansi_c_string",
    "attribute_value",
    "char_literal",
    "character",
    "heredoc_body",
    "interpreted_string_literal",
    "quoted_attribute_value",
    "raw_string_literal",
    "regex",
    "rune_literal",
    "string",
    "string_content",
    "string_literal",
    "string_value",
    "template_string",
    "translated_string",
];

const NUMBER_KINDS: &[&str] = &[
    "color_value",
    "float",
    "float_literal",
    "float_value",
    "imaginary_literal",
    "int_literal",
    "integer",
    "integer_literal",
    "integer_value",
    "number",
];

const TYPE_KINDS: &[&str] = &[
    "predefined_type",
    "primitive_type",
    "property_name",
    "tag_name",
    "type_identifier",
    "variable_name",
    "erroneous_end_tag_name",
];

const FUNCTION_KINDS: &[&str] = &["attribute_name", "class_name", "command_name", "id_name"];

const KEYWORD_KINDS: &[&str] = &["at_keyword", "important"];

const IDENTIFIER_KINDS: &[&str] = &[
    "attribute_name",
    "class_name",
    "command_name",
    "field_identifier",
    "id_name",
    "identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
    "tag_name",
    "type_identifier",
    "variable_name",
];

#[derive(Clone, Copy, Debug)]
struct Span {
    start: usize,
    end: usize,
    kind: TokenKind,
}

impl Grammar {
    fn language(self) -> tree_sitter::Language {
        match self {
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

thread_local! {
    static PARSERS: RefCell<HashMap<Grammar, Parser>> = RefCell::new(HashMap::new());
}

pub(crate) fn highlight(grammar: Grammar, language: Language, code: &str) -> Vec<Vec<Token>> {
    let Some(tree) = parse(grammar, code) else {
        return lex::highlight(language, code);
    };
    let mut spans = Vec::new();
    collect(tree.root_node(), code.as_bytes(), language, &mut spans);
    split_lines(code, &spans)
}

fn parse(grammar: Grammar, code: &str) -> Option<Tree> {
    PARSERS.with(|cell| {
        let mut parsers = cell.borrow_mut();
        let parser = match parsers.entry(grammar) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let mut parser = Parser::new();
                parser.set_language(&grammar.language()).ok()?;
                entry.insert(parser)
            }
        };
        parser.parse(code, None)
    })
}

fn collect(node: Node<'_>, src: &[u8], language: Language, out: &mut Vec<Span>) {
    if node.end_byte() <= node.start_byte() {
        return;
    }
    if let Some(kind) = classify(node, src, language) {
        out.push(Span {
            start: node.start_byte(),
            end: node.end_byte(),
            kind,
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, src, language, out);
    }
}

fn classify(node: Node<'_>, src: &[u8], language: Language) -> Option<TokenKind> {
    let kind = node.kind();
    if kind.contains("comment") || kind == "doctype" {
        return Some(TokenKind::Comment);
    }
    if is_object_key(node) {
        return Some(TokenKind::Type);
    }
    if STRING_KINDS.contains(&kind) {
        return Some(TokenKind::String);
    }
    if NUMBER_KINDS.contains(&kind) {
        return Some(TokenKind::Number);
    }
    if TYPE_KINDS.contains(&kind) {
        return Some(TokenKind::Type);
    }
    if FUNCTION_KINDS.contains(&kind) {
        return Some(TokenKind::Function);
    }
    if KEYWORD_KINDS.contains(&kind) {
        return Some(TokenKind::Keyword);
    }
    if is_function_name(node) {
        return Some(TokenKind::Function);
    }
    if node.child_count() == 0
        && !IDENTIFIER_KINDS.contains(&kind)
        && node
            .utf8_text(src)
            .is_ok_and(|text| !text.is_empty() && lex::is_keyword(language, text))
    {
        return Some(TokenKind::Keyword);
    }
    None
}

fn is_object_key(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "pair" {
        return false;
    }
    parent
        .child_by_field_name("key")
        .is_some_and(|key| key == node)
}

fn is_function_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let parent_kind = parent.kind();
    if (parent_kind.contains("function") || parent_kind.contains("method"))
        && parent
            .child_by_field_name("name")
            .is_some_and(|name| name == node)
    {
        return true;
    }
    if matches!(
        parent_kind,
        "call" | "call_expression" | "macro_invocation" | "new_expression"
    ) {
        return ["function", "macro", "constructor"].iter().any(|field| {
            parent
                .child_by_field_name(field)
                .is_some_and(|item| item == node)
        });
    }
    false
}

fn split_lines(code: &str, spans: &[Span]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut current = 0usize;
    let mut offset = 0usize;

    for line in code.split('\n') {
        let start = offset;
        let end = start + line.len();
        while current < spans.len() && spans[current].end <= start {
            current += 1;
        }

        let mut tokens = Vec::new();
        let mut position = 0usize;
        let mut index = current;
        while index < spans.len() && spans[index].start < end {
            let span_start = spans[index].start.max(start);
            let span_end = spans[index].end.min(end);
            if span_start > start + position {
                lex::push_token(
                    &mut tokens,
                    &line[position..span_start - start],
                    TokenKind::Plain,
                );
                position = span_start - start;
            }
            if span_end > span_start {
                lex::push_token(
                    &mut tokens,
                    &line[span_start - start..span_end - start],
                    spans[index].kind,
                );
                position = span_end - start;
            }
            if spans[index].end <= end {
                index += 1;
            } else {
                break;
            }
        }
        if position < line.len() {
            lex::push_token(&mut tokens, &line[position..], TokenKind::Plain);
        }
        if tokens.is_empty() {
            tokens.push(Token {
                text: String::new(),
                kind: TokenKind::Plain,
            });
        }
        out.push(tokens);
        offset = end + 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(grammar: Grammar, language: Language, code: &str) -> Vec<Vec<Token>> {
        highlight(grammar, language, code)
    }

    fn texts(lines: &[Vec<Token>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.iter().map(|token| token.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn rust_classifies_keywords_strings_and_comments() {
        let lines = sample(
            Grammar::Rust,
            Language::Rust,
            "fn main() {\n    let name = \"meerai\"; // note\n}",
        );
        let kinds: Vec<TokenKind> = lines.iter().flatten().map(|token| token.kind).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::String));
        assert!(kinds.contains(&TokenKind::Comment));
        assert_eq!(texts(&lines)[1], "    let name = \"meerai\"; // note");
    }

    #[test]
    fn functions_and_types_are_classified() {
        let python = sample(
            Grammar::Python,
            Language::Python,
            "def run():\n    return 1",
        );
        assert!(
            python[0]
                .iter()
                .any(|token| token.text == "run" && token.kind == TokenKind::Function)
        );

        let go = sample(
            Grammar::Go,
            Language::Go,
            "func main() {\n\tvar x Widget\n}",
        );
        assert!(
            go[0]
                .iter()
                .any(|token| token.text == "main" && token.kind == TokenKind::Function)
        );
        assert!(
            go[1]
                .iter()
                .any(|token| token.text == "Widget" && token.kind == TokenKind::Type)
        );

        let ts = sample(
            Grammar::TypeScript,
            Language::TypeScript,
            "const x: number = 1;",
        );
        assert!(
            ts[0]
                .iter()
                .any(|token| token.text == "const" && token.kind == TokenKind::Keyword)
        );
        assert!(
            ts[0]
                .iter()
                .any(|token| token.text == "number" && token.kind == TokenKind::Type)
        );
    }

    #[test]
    fn block_comments_keep_every_line() {
        let lines = sample(
            Grammar::JavaScript,
            Language::JavaScript,
            "/* first\n   second */\nconst x = 1;",
        );
        assert_eq!(
            texts(&lines),
            vec!["/* first", "   second */", "const x = 1;"]
        );
        assert_eq!(lines[0][0].kind, TokenKind::Comment);
        assert_eq!(lines[1][0].kind, TokenKind::Comment);
    }

    #[test]
    fn json_keys_differ_from_values() {
        let lines = sample(
            Grammar::Json,
            Language::Json,
            "{\"name\": \"meerai\", \"ok\": true}",
        );
        let tokens: Vec<&Token> = lines[0].iter().collect();
        let key = tokens
            .iter()
            .find(|token| token.text == "\"name\"")
            .expect("key token");
        assert_eq!(key.kind, TokenKind::Type);
        let value = tokens
            .iter()
            .find(|token| token.text == "\"meerai\"")
            .expect("value token");
        assert_eq!(value.kind, TokenKind::String);
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Keyword));
    }

    #[test]
    fn html_marks_tags_attributes_and_values() {
        let lines = sample(Grammar::Html, Language::Html, "<a href=\"/docs\">docs</a>");
        let tokens: Vec<&Token> = lines[0].iter().collect();
        let tag = tokens.iter().find(|token| token.text == "a").expect("tag");
        assert_eq!(tag.kind, TokenKind::Type);
        let attr = tokens
            .iter()
            .find(|token| token.text == "href")
            .expect("attribute");
        assert_eq!(attr.kind, TokenKind::Function);
        let value = tokens
            .iter()
            .find(|token| token.text == "\"/docs\"")
            .expect("value");
        assert_eq!(value.kind, TokenKind::String);
    }

    #[test]
    fn css_marks_properties_selectors_and_values() {
        let lines = sample(
            Grammar::Css,
            Language::Css,
            ".card { color: #ff0000; margin: 4px; }",
        );
        let tokens: Vec<&Token> = lines[0].iter().collect();
        let class = tokens
            .iter()
            .find(|token| token.text == "card")
            .expect("class");
        assert_eq!(class.kind, TokenKind::Function);
        let property = tokens
            .iter()
            .find(|token| token.text == "color")
            .expect("property");
        assert_eq!(property.kind, TokenKind::Type);
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Number));
    }

    #[test]
    fn bash_marks_commands_and_keywords() {
        let lines = sample(
            Grammar::Bash,
            Language::Bash,
            "if [ -f file ]; then\n  cargo test\nfi",
        );
        let tokens: Vec<&Token> = lines.iter().flatten().collect();
        let command = tokens
            .iter()
            .find(|token| token.text == "cargo")
            .expect("command");
        assert_eq!(command.kind, TokenKind::Function);
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Keyword));
    }
}
