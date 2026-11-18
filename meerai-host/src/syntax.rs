pub use meerai_syntax::{Language, Token, TokenKind, language_for_fence, language_for_path};

pub fn token_lines(language: Option<Language>, code: &str) -> Vec<Vec<Token>> {
    meerai_syntax::highlight_block(language, code)
}
