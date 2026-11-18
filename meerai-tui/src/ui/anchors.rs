use super::*;

pub(super) fn render_anchor_rail(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().background)),
        area,
    );

    let layout = app.anchor_rail_layout(area);
    let current = app.anchor_rail_current_number();
    let width = area.width as usize;

    if layout.top_gap {
        put(
            frame,
            area.x + area.width.saturating_sub(1),
            area.y,
            "⋮",
            gap_color(app, area.y),
        );
    }
    for mark in &layout.marks {
        let color = if app.anchor_rail_hover == Some(mark.row) {
            theme::p().text
        } else if current == Some(mark.number) {
            theme::p().accent
        } else {
            theme::p().muted
        };
        put(
            frame,
            area.x,
            mark.row,
            &format!("{:>width$}", mark.number),
            color,
        );
    }
    if layout.bottom_gap {
        let row = area.y + area.height - 1;
        put(
            frame,
            area.x + area.width.saturating_sub(1),
            row,
            "⋮",
            gap_color(app, row),
        );
    }
}

fn gap_color(app: &AppState, row: u16) -> Color {
    if app.anchor_rail_hover == Some(row) {
        theme::p().text
    } else {
        theme::p().faint
    }
}

fn put(frame: &mut Frame<'_>, x: u16, y: u16, text: &str, color: Color) {
    for (offset, glyph) in text.chars().enumerate() {
        let col = x.saturating_add(offset as u16);
        if let Some(cell) = frame.buffer_mut().cell_mut((col, y)) {
            let mut buf = [0u8; 4];
            cell.set_symbol(glyph.encode_utf8(&mut buf));
            cell.set_fg(color);
        }
    }
}
