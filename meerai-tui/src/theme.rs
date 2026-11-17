use std::sync::{OnceLock, RwLock};

use meerai_common::settings::ThemeSettings;
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub surface_high: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub interactive: Color,
    pub border: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_context: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: Color::Rgb(10, 10, 10),
            surface: Color::Rgb(20, 20, 20),
            surface_high: Color::Rgb(30, 30, 30),
            text: Color::Rgb(238, 238, 238),
            muted: Color::Rgb(128, 128, 128),
            faint: Color::Rgb(76, 76, 76),
            accent: Color::Rgb(45, 212, 191),
            interactive: Color::Rgb(122, 186, 255),
            border: Color::Rgb(72, 72, 72),
            error: Color::Rgb(224, 108, 117),
            warning: Color::Rgb(255, 158, 100),
            success: Color::Rgb(127, 216, 143),
            info: Color::Rgb(86, 182, 194),
            diff_added: Color::Rgb(79, 214, 190),
            diff_removed: Color::Rgb(197, 59, 83),
            diff_context: Color::Rgb(125, 133, 144),
        }
    }
}

impl Palette {
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
        );
        (palette, warnings)
    }
}

static PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

fn palette_lock() -> &'static RwLock<Palette> {
    PALETTE.get_or_init(|| RwLock::new(Palette::default()))
}

pub fn install(palette: Palette) {
    set(palette);
}

pub fn set(palette: Palette) {
    let lock = palette_lock();
    if let Ok(mut current) = lock.write() {
        *current = palette;
    }
}

pub fn p() -> Palette {
    palette_lock()
        .read()
        .map(|palette| *palette)
        .unwrap_or_default()
}

pub fn parse_color(raw: &str) -> Option<Color> {
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
                Some(Color::Rgb(
                    expand(digits[0] as char)?,
                    expand(digits[1] as char)?,
                    expand(digits[2] as char)?,
                ))
            }
            6 => Some(Color::Rgb(hex_pair(0)?, hex_pair(2)?, hex_pair(4)?)),
            _ => None,
        };
    }
    let color = match value.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        "reset" | "default" => Color::Reset,
        _ => return None,
    };
    Some(color)
}

pub fn text() -> Style {
    Style::default().fg(p().text)
}

pub fn muted() -> Style {
    Style::default().fg(p().muted)
}

pub fn faint() -> Style {
    Style::default().fg(p().faint)
}

pub fn label() -> Style {
    Style::default().fg(p().muted).add_modifier(Modifier::BOLD)
}

pub fn heading() -> Style {
    Style::default().fg(p().accent).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_named_colors() {
        assert_eq!(parse_color("#2dd4bf"), Some(Color::Rgb(45, 212, 191)));
        assert_eq!(parse_color("#f83"), Some(Color::Rgb(255, 136, 51)));
        assert_eq!(parse_color(" dark-gray "), Some(Color::DarkGray));
        assert_eq!(parse_color("#zzzzzz"), None);
        assert_eq!(parse_color("not-a-color"), None);
    }

    #[test]
    fn palette_overrides_tokens_and_warns_on_invalid_values() {
        let settings = ThemeSettings {
            accent: Some("#112233".to_string()),
            background: Some("#000000".to_string()),
            text: Some("nope".to_string()),
            ..ThemeSettings::default()
        };
        let (palette, warnings) = Palette::from_settings(&settings);
        assert_eq!(palette.accent, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(palette.background, Color::Rgb(0, 0, 0));
        assert_eq!(palette.text, Palette::default().text);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("theme.text"));
    }
}
