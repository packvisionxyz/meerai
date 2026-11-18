use meerai_common::settings::ThemeSettings;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum HostColor {
    Rgb([u8; 3]),
    Named(NamedColor),
}

impl HostColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Rgb([r, g, b])
    }

    pub fn to_hex(self, fallback: &str) -> String {
        match self {
            Self::Rgb([r, g, b]) => format!("#{r:02x}{g:02x}{b:02x}"),
            Self::Named(named) => match named {
                NamedColor::Black => "#000000",
                NamedColor::Red => "#cd3131",
                NamedColor::Green => "#0dbc79",
                NamedColor::Yellow => "#e5e510",
                NamedColor::Blue => "#2472c8",
                NamedColor::Magenta => "#bc3fbc",
                NamedColor::Cyan => "#11a8cd",
                NamedColor::Gray => "#808080",
                NamedColor::DarkGray => "#555555",
                NamedColor::LightRed => "#f14c4c",
                NamedColor::LightGreen => "#23d18b",
                NamedColor::LightYellow => "#f5f543",
                NamedColor::LightBlue => "#3b8eea",
                NamedColor::LightMagenta => "#d670d6",
                NamedColor::LightCyan => "#29b8db",
                NamedColor::White => "#ffffff",
                NamedColor::Reset => fallback,
            }
            .to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostPalette {
    pub background: HostColor,
    pub surface: HostColor,
    pub surface_high: HostColor,
    pub text: HostColor,
    pub muted: HostColor,
    pub faint: HostColor,
    pub accent: HostColor,
    pub interactive: HostColor,
    pub border: HostColor,
    pub error: HostColor,
    pub warning: HostColor,
    pub success: HostColor,
    pub info: HostColor,
    pub diff_added: HostColor,
    pub diff_removed: HostColor,
    pub diff_context: HostColor,
    pub code_function: HostColor,
}

impl Default for HostPalette {
    fn default() -> Self {
        Self {
            background: HostColor::rgb(10, 10, 10),
            surface: HostColor::rgb(20, 20, 20),
            surface_high: HostColor::rgb(30, 30, 30),
            text: HostColor::rgb(238, 238, 238),
            muted: HostColor::rgb(128, 128, 128),
            faint: HostColor::rgb(76, 76, 76),
            accent: HostColor::rgb(45, 212, 191),
            interactive: HostColor::rgb(122, 186, 255),
            border: HostColor::rgb(72, 72, 72),
            error: HostColor::rgb(224, 108, 117),
            warning: HostColor::rgb(255, 158, 100),
            success: HostColor::rgb(127, 216, 143),
            info: HostColor::rgb(86, 182, 194),
            diff_added: HostColor::rgb(79, 214, 190),
            diff_removed: HostColor::rgb(197, 59, 83),
            diff_context: HostColor::rgb(125, 133, 144),
            code_function: HostColor::rgb(198, 120, 221),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedThemeHex {
    pub background: String,
    pub surface: String,
    pub surface_high: String,
    pub text: String,
    pub muted: String,
    pub faint: String,
    pub accent: String,
    pub interactive: String,
    pub border: String,
    pub error: String,
    pub warning: String,
    pub success: String,
    pub info: String,
    pub diff_added: String,
    pub diff_removed: String,
    pub diff_context: String,
    pub code_function: String,
}

impl HostPalette {
    pub fn from_settings(settings: &ThemeSettings) -> (Self, Vec<String>) {
        let mut palette = Self::default();
        let mut warnings = Vec::new();
        macro_rules! apply {
            ($($field:ident),* $(,)?) => {
                $(
                    if let Some(raw) = settings.$field.as_deref() {
                        match parse_color(raw) {
                            Some(color) => palette.$field = color,
                            None => warnings.push(format!(
                                "invalid theme.{} color '{}': use #rrggbb or a color name",
                                stringify!($field),
                                raw
                            )),
                        }
                    }
                )*
            };
        }
        apply!(
            background,
            surface,
            surface_high,
            text,
            muted,
            faint,
            accent,
            interactive,
            border,
            error,
            warning,
            success,
            info,
            diff_added,
            diff_removed,
            diff_context,
            code_function,
        );
        (palette, warnings)
    }

    pub fn to_hex_tokens(self) -> ResolvedThemeHex {
        let def = Self::default();
        ResolvedThemeHex {
            background: self.background.to_hex(&def.background.to_hex("#0a0a0a")),
            surface: self.surface.to_hex(&def.surface.to_hex("#141414")),
            surface_high: self
                .surface_high
                .to_hex(&def.surface_high.to_hex("#1e1e1e")),
            text: self.text.to_hex(&def.text.to_hex("#eeeeee")),
            muted: self.muted.to_hex(&def.muted.to_hex("#808080")),
            faint: self.faint.to_hex(&def.faint.to_hex("#4c4c4c")),
            accent: self.accent.to_hex(&def.accent.to_hex("#2dd4bf")),
            interactive: self.interactive.to_hex(&def.interactive.to_hex("#7abaff")),
            border: self.border.to_hex(&def.border.to_hex("#484848")),
            error: self.error.to_hex(&def.error.to_hex("#e06c75")),
            warning: self.warning.to_hex(&def.warning.to_hex("#ff9e64")),
            success: self.success.to_hex(&def.success.to_hex("#7fd88f")),
            info: self.info.to_hex(&def.info.to_hex("#56b6c2")),
            diff_added: self.diff_added.to_hex(&def.diff_added.to_hex("#4fd6be")),
            diff_removed: self
                .diff_removed
                .to_hex(&def.diff_removed.to_hex("#c53b53")),
            diff_context: self
                .diff_context
                .to_hex(&def.diff_context.to_hex("#7d8590")),
            code_function: self
                .code_function
                .to_hex(&def.code_function.to_hex("#c678dd")),
        }
    }
}

pub fn parse_color(raw: &str) -> Option<HostColor> {
    let value = raw.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex.bytes().collect();
        let hex_pair = |a: usize| -> Option<u8> {
            let hi = (digits.get(a).copied()? as char).to_digit(16)?;
            let lo = (digits.get(a + 1).copied()? as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        };
        return match digits.len() {
            3 => {
                let expand = |c: char| c.to_digit(16).map(|d| (d * 17) as u8);
                Some(HostColor::rgb(
                    expand(digits[0] as char)?,
                    expand(digits[1] as char)?,
                    expand(digits[2] as char)?,
                ))
            }
            6 => Some(HostColor::rgb(hex_pair(0)?, hex_pair(2)?, hex_pair(4)?)),
            _ => None,
        };
    }
    let named = match value.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "black" => NamedColor::Black,
        "red" => NamedColor::Red,
        "green" => NamedColor::Green,
        "yellow" => NamedColor::Yellow,
        "blue" => NamedColor::Blue,
        "magenta" => NamedColor::Magenta,
        "cyan" => NamedColor::Cyan,
        "gray" | "grey" => NamedColor::Gray,
        "darkgray" | "darkgrey" => NamedColor::DarkGray,
        "lightred" => NamedColor::LightRed,
        "lightgreen" => NamedColor::LightGreen,
        "lightyellow" => NamedColor::LightYellow,
        "lightblue" => NamedColor::LightBlue,
        "lightmagenta" => NamedColor::LightMagenta,
        "lightcyan" => NamedColor::LightCyan,
        "white" => NamedColor::White,
        "reset" | "default" => NamedColor::Reset,
        _ => return None,
    };
    Some(HostColor::Named(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_named_colors_and_formats_hex_tokens() {
        assert_eq!(parse_color("#2dd4bf"), Some(HostColor::rgb(45, 212, 191)));
        assert_eq!(parse_color("#f83"), Some(HostColor::rgb(255, 136, 51)));
        assert_eq!(
            parse_color(" dark-gray "),
            Some(HostColor::Named(NamedColor::DarkGray))
        );
        assert_eq!(parse_color("#zzzzzz"), None);

        let settings = ThemeSettings {
            accent: Some("#112233".to_string()),
            text: Some("nope".to_string()),
            ..ThemeSettings::default()
        };
        let (palette, warnings) = HostPalette::from_settings(&settings);
        assert_eq!(warnings.len(), 1);
        assert_eq!(palette.to_hex_tokens().accent, "#112233");
    }
}
