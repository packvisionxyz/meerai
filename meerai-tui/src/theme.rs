use std::sync::{OnceLock, RwLock};

use meerai_common::settings::ThemeSettings;
use meerai_host::theme::{HostColor, HostPalette, NamedColor};
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
    pub code_function: Color,
}

fn to_ratatui_color(color: HostColor) -> Color {
    match color {
        HostColor::Rgb([r, g, b]) => Color::Rgb(r, g, b),
        HostColor::Named(named) => match named {
            NamedColor::Black => Color::Black,
            NamedColor::Red => Color::Red,
            NamedColor::Green => Color::Green,
            NamedColor::Yellow => Color::Yellow,
            NamedColor::Blue => Color::Blue,
            NamedColor::Magenta => Color::Magenta,
            NamedColor::Cyan => Color::Cyan,
            NamedColor::Gray => Color::Gray,
            NamedColor::DarkGray => Color::DarkGray,
            NamedColor::LightRed => Color::LightRed,
            NamedColor::LightGreen => Color::LightGreen,
            NamedColor::LightYellow => Color::LightYellow,
            NamedColor::LightBlue => Color::LightBlue,
            NamedColor::LightMagenta => Color::LightMagenta,
            NamedColor::LightCyan => Color::LightCyan,
            NamedColor::White => Color::White,
            NamedColor::Reset => Color::Reset,
        },
    }
}

impl From<HostPalette> for Palette {
    fn from(host: HostPalette) -> Self {
        Self {
            background: to_ratatui_color(host.background),
            surface: to_ratatui_color(host.surface),
            surface_high: to_ratatui_color(host.surface_high),
            text: to_ratatui_color(host.text),
            muted: to_ratatui_color(host.muted),
            faint: to_ratatui_color(host.faint),
            accent: to_ratatui_color(host.accent),
            interactive: to_ratatui_color(host.interactive),
            border: to_ratatui_color(host.border),
            error: to_ratatui_color(host.error),
            warning: to_ratatui_color(host.warning),
            success: to_ratatui_color(host.success),
            info: to_ratatui_color(host.info),
            diff_added: to_ratatui_color(host.diff_added),
            diff_removed: to_ratatui_color(host.diff_removed),
            diff_context: to_ratatui_color(host.diff_context),
            code_function: to_ratatui_color(host.code_function),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        HostPalette::default().into()
    }
}

impl Palette {
    pub fn from_settings(settings: &ThemeSettings) -> (Self, Vec<String>) {
        let (host, warnings) = HostPalette::from_settings(settings);
        (host.into(), warnings)
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
    meerai_host::theme::parse_color(raw).map(to_ratatui_color)
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
