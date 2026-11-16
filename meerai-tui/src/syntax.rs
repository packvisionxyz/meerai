use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme;

pub use meerai_syntax::{Language, Token, TokenKind, language_for_fence, language_for_path};

pub fn token_lines(language: Option<Language>, code: &str) -> Vec<Vec<Token>> {
    meerai_syntax::highlight_block(language, code)
}

pub fn highlight(language: Option<Language>, code: &str, base: Style) -> Vec<Vec<Span<'static>>> {
    token_lines(language, code)
        .into_iter()
        .map(|tokens| line_spans(&tokens, base))
        .collect()
}

pub fn line_spans(tokens: &[Token], base: Style) -> Vec<Span<'static>> {
    tokens
        .iter()
        .map(|token| Span::styled(token.text.clone(), token_style(token.kind, base)))
        .collect()
}

fn token_style(kind: TokenKind, base: Style) -> Style {
    match kind {
        TokenKind::Plain => base,
        TokenKind::Keyword => base.fg(theme::p().accent).add_modifier(Modifier::BOLD),
        TokenKind::String => base.fg(theme::p().success),
        TokenKind::Number => base.fg(theme::p().info),
        TokenKind::Comment => base.fg(theme::p().faint).add_modifier(Modifier::ITALIC),
        TokenKind::Function => base.fg(theme::p().interactive),
        TokenKind::Type => base.fg(theme::p().accent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tokens_to_theme_styles() {
        let tokens = vec![
            Token {
                text: "let".to_string(),
                kind: TokenKind::Keyword,
            },
            Token {
                text: " value".to_string(),
                kind: TokenKind::Plain,
            },
        ];
        let spans = line_spans(&tokens, Style::default());
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "let");
        assert_eq!(spans[0].style.fg, Some(theme::p().accent));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[1].style.fg, None);
    }

    #[test]
    fn highlights_blocks_line_by_line() {
        let lines = highlight(
            Some(Language::Rust),
            "fn main() {\n    let x = 1;\n}",
            Style::default(),
        );
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].iter().any(
                |span| span.content == "fn" && span.style.add_modifier.contains(Modifier::BOLD)
            )
        );
        assert!(lines[1].iter().any(|span| span.content == "let"));
    }

    #[test]
    fn highlights_plain_text_without_language() {
        let lines = highlight(None, "let x = 1;", Style::default());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].content, "let x = 1;");
    }
}
