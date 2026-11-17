use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::theme;

pub const M_ROWS: [&str; 4] = ["██      ██", "████  ████", "██  ██  ██", "██      ██"];

fn lerp_channel(from: u8, to: u8, t: f32) -> u8 {
    (from as f32 + (to as f32 - from as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn lerp_color(from: Color, to: Color, t: f32) -> Color {
    match (from, to) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) => Color::Rgb(
            lerp_channel(fr, tr, t),
            lerp_channel(fg, tg, t),
            lerp_channel(fb, tb, t),
        ),
        _ if t < 0.5 => from,
        _ => to,
    }
}

fn logo_color(t: f32) -> Color {
    let palette = theme::p();
    if t < 0.5 {
        lerp_color(palette.info, palette.accent, t * 2.0)
    } else {
        lerp_color(palette.accent, palette.interactive, (t - 0.5) * 2.0)
    }
}

fn char_t(row: usize, col: usize) -> f32 {
    let width = M_ROWS[0].chars().count();
    let last = (M_ROWS.len() * width).saturating_sub(1) as f32;
    if last > 0.0 {
        (row * width + col) as f32 / last
    } else {
        0.0
    }
}

pub fn m_spans(row: usize) -> Vec<Span<'static>> {
    M_ROWS[row]
        .chars()
        .enumerate()
        .map(|(col, symbol)| {
            Span::styled(
                symbol.to_string(),
                Style::default()
                    .fg(logo_color(char_t(row, col)))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

pub fn ansi() -> String {
    let mut out = String::new();
    for (row, text) in M_ROWS.iter().enumerate() {
        out.push_str("\x1b[1m");
        for (col, symbol) in text.chars().enumerate() {
            out.push_str(&fg_escape(logo_color(char_t(row, col))));
            out.push(symbol);
        }
        out.push_str("\x1b[0m");
        if row + 1 < M_ROWS.len() {
            out.push('\n');
        }
    }
    out
}

fn fg_escape(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(i) => format!("\x1b[38;5;{i}m"),
        Color::Black => "\x1b[30m".to_string(),
        Color::Red => "\x1b[31m".to_string(),
        Color::Green => "\x1b[32m".to_string(),
        Color::Yellow => "\x1b[33m".to_string(),
        Color::Blue => "\x1b[34m".to_string(),
        Color::Magenta => "\x1b[35m".to_string(),
        Color::Cyan => "\x1b[36m".to_string(),
        Color::Gray => "\x1b[37m".to_string(),
        Color::DarkGray => "\x1b[90m".to_string(),
        Color::LightRed => "\x1b[91m".to_string(),
        Color::LightGreen => "\x1b[92m".to_string(),
        Color::LightYellow => "\x1b[93m".to_string(),
        Color::LightBlue => "\x1b[94m".to_string(),
        Color::LightMagenta => "\x1b[95m".to_string(),
        Color::LightCyan => "\x1b[96m".to_string(),
        Color::White => "\x1b[97m".to_string(),
        Color::Reset => "\x1b[39m".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m_spans_cover_the_row_with_bold_gradient_styles() {
        for (row, text) in M_ROWS.iter().enumerate() {
            let spans = m_spans(row);
            assert_eq!(spans.len(), text.chars().count());
            assert!(
                spans
                    .iter()
                    .all(|span| span.style.add_modifier.contains(Modifier::BOLD))
            );
        }
        let first = m_spans(0);
        let last_row = M_ROWS.len() - 1;
        let last = m_spans(last_row);
        assert_eq!(first[0].style.fg, Some(theme::p().info));
        assert_eq!(last[last.len() - 1].style.fg, Some(theme::p().interactive));
    }

    #[test]
    fn ansi_renders_four_bold_rows_with_color_and_reset() {
        let rendered = ansi();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), M_ROWS.len());
        for (line, text) in lines.iter().zip(M_ROWS.iter()) {
            assert!(line.starts_with("\x1b[1m"));
            assert!(line.ends_with("\x1b[0m"));
            let plain: String = line
                .split("\x1b[")
                .skip(1)
                .map(|part| part.split_once('m').map(|(_, rest)| rest).unwrap_or(""))
                .collect();
            assert_eq!(plain, *text);
        }
    }
}
