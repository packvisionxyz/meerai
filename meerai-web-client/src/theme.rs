use meerai_web_protocol::ThemeTokensDto;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowserTheme {
    #[default]
    Host,
    System,
    Light,
    Dark,
}

impl BrowserTheme {
    pub const OPTIONS: [(Self, &'static str); 4] = [
        (Self::Host, "Host theme"),
        (Self::System, "System"),
        (Self::Light, "Light"),
        (Self::Dark, "Dark"),
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::OPTIONS
            .iter()
            .find(|(theme, _)| theme.key() == key)
            .map(|(theme, _)| *theme)
            .unwrap_or_default()
    }

    #[cfg(target_arch = "wasm32")]
    const STORAGE_KEY: &str = "meerai.browser-theme";

    #[cfg(target_arch = "wasm32")]
    pub fn load() -> Self {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .and_then(|storage| storage.get_item(Self::STORAGE_KEY).ok().flatten())
            .map(|key| Self::from_key(&key))
            .unwrap_or_default()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn save(self) {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(Self::STORAGE_KEY, self.key());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiTheme {
    pub vars: String,
    pub light: bool,
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
        let bg = value(&tokens.background, &fallback.background);
        let accent = value(&tokens.accent, &fallback.accent);
        let warning = value(&tokens.warning, &fallback.warning);
        let light = is_light_color(&bg);
        let scheme = if light { "light" } else { "dark" };
        let on_accent = if is_light_color(&accent) {
            "#04201d"
        } else {
            "#ffffff"
        };
        let on_warning = if is_light_color(&warning) {
            "#1a1206"
        } else {
            "#ffffff"
        };
        let vars = format!(
            "color-scheme:{scheme};--m-bg:{bg};--m-surface:{};--m-surface-high:{};--m-text:{};--m-muted:{};--m-faint:{};\
             --m-accent:{accent};--m-on-accent:{on_accent};--m-interactive:{};--m-border:{};--m-error:{};--m-warning:{warning};\
             --m-on-warning:{on_warning};--m-success:{};--m-info:{};--m-diff-added:{};--m-diff-removed:{};\
             --m-diff-context:{};--m-code-function:{};",
            value(&tokens.surface, &fallback.surface),
            value(&tokens.surface_high, &fallback.surface_high),
            value(&tokens.text, &fallback.text),
            value(&tokens.muted, &fallback.muted),
            value(&tokens.faint, &fallback.faint),
            value(&tokens.interactive, &fallback.interactive),
            value(&tokens.border, &fallback.border),
            value(&tokens.error, &fallback.error),
            value(&tokens.success, &fallback.success),
            value(&tokens.info, &fallback.info),
            value(&tokens.diff_added, &fallback.diff_added),
            value(&tokens.diff_removed, &fallback.diff_removed),
            value(&tokens.diff_context, &fallback.diff_context),
            value(&tokens.code_function, &fallback.code_function),
        );
        Self { vars, light }
    }
}

pub fn is_valid_color(raw: &str) -> bool {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix('#') {
        return matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    !raw.is_empty() && raw.len() <= 32 && raw.chars().all(|c| c.is_ascii_alphabetic())
}

pub fn is_light_color(raw: &str) -> bool {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix('#') {
        let rgb = if hex.len() == 3 {
            let mut bytes = hex.bytes().filter_map(|b| (b as char).to_digit(16));
            match (bytes.next(), bytes.next(), bytes.next()) {
                (Some(r), Some(g), Some(b)) => Some((r * 17, g * 17, b * 17)),
                _ => None,
            }
        } else if hex.len() == 6 {
            let r = u32::from_str_radix(&hex[0..2], 16).ok();
            let g = u32::from_str_radix(&hex[2..4], 16).ok();
            let b = u32::from_str_radix(&hex[4..6], 16).ok();
            r.zip(g).zip(b).map(|((r, g), b)| (r, g, b))
        } else {
            None
        };
        if let Some((r, g, b)) = rgb {
            let luma = 2126 * r + 7152 * g + 722 * b;
            return luma > 128 * 10000;
        }
    }
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "white" | "ivory" | "snow" | "beige" | "linen" | "aliceblue" | "ghostwhite" | "whitesmoke"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_contrast_foregrounds_for_light_and_dark_accents() {
        let dark = UiTheme::resolve(&ThemeTokensDto::default());
        assert!(!dark.light);
        assert!(dark.vars.contains("color-scheme:dark;"));
        assert!(dark.vars.contains("--m-on-accent:#04201d;"));

        let light = UiTheme::resolve(&ThemeTokensDto {
            background: "#fafbfc".to_owned(),
            accent: "#0f766e".to_owned(),
            ..ThemeTokensDto::default()
        });
        assert!(light.light);
        assert!(light.vars.contains("color-scheme:light;"));
        assert!(light.vars.contains("--m-on-accent:#ffffff;"));
    }

    #[test]
    fn browser_theme_round_trips_known_keys() {
        for (variant, _) in BrowserTheme::OPTIONS {
            assert_eq!(BrowserTheme::from_key(variant.key()), variant);
        }
        assert_eq!(BrowserTheme::from_key("unknown"), BrowserTheme::Host);
    }
}
