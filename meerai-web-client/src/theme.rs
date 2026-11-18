use meerai_web_protocol::ThemeTokensDto;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiTheme {
    pub vars: String,
}

impl UiTheme {
    pub fn resolve(tokens: &ThemeTokensDto) -> Self {
        let fallback = ThemeTokensDto::default();
        let value = |raw: &str, default: &str| {
            if is_valid_color(raw) {
                raw.trim().to_owned()
            } else {
                default.to_owned()
            }
        };
        let vars = format!(
            "--m-bg:{};--m-surface:{};--m-surface-high:{};--m-text:{};--m-muted:{};--m-faint:{};\
             --m-accent:{};--m-interactive:{};--m-border:{};--m-error:{};--m-warning:{};\
             --m-success:{};--m-info:{};--m-diff-added:{};--m-diff-removed:{};\
             --m-diff-context:{};--m-code-function:{};",
            value(&tokens.background, &fallback.background),
            value(&tokens.surface, &fallback.surface),
            value(&tokens.surface_high, &fallback.surface_high),
            value(&tokens.text, &fallback.text),
            value(&tokens.muted, &fallback.muted),
            value(&tokens.faint, &fallback.faint),
            value(&tokens.accent, &fallback.accent),
            value(&tokens.interactive, &fallback.interactive),
            value(&tokens.border, &fallback.border),
            value(&tokens.error, &fallback.error),
            value(&tokens.warning, &fallback.warning),
            value(&tokens.success, &fallback.success),
            value(&tokens.info, &fallback.info),
            value(&tokens.diff_added, &fallback.diff_added),
            value(&tokens.diff_removed, &fallback.diff_removed),
            value(&tokens.diff_context, &fallback.diff_context),
            value(&tokens.code_function, &fallback.code_function),
        );
        Self { vars }
    }
}

pub fn is_valid_color(raw: &str) -> bool {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix('#') {
        return matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    !raw.is_empty() && raw.len() <= 32 && raw.chars().all(|c| c.is_ascii_alphabetic())
}
