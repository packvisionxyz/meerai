use comrak::nodes::{AstNode, NodeList, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::syntax;
use crate::theme;

const TABLE_MIN_COLUMN: usize = 3;

pub struct MarkdownLine {
    pub spans: Vec<Span<'static>>,
    pub code: bool,
    pub hang: String,
}

pub fn render(text: &str, width: usize) -> Vec<MarkdownLine> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.inline_footnotes = true;
    options.extension.description_lists = true;
    options.extension.front_matter_delimiter = Some("---".to_string());
    options.extension.multiline_block_quotes = true;
    options.extension.alerts = true;
    options.extension.superscript = true;
    options.extension.subscript = true;
    options.extension.math_dollars = true;
    options.extension.math_dollars_block = true;

    let arena = Arena::new();
    let root = parse_document(&arena, text, &options);
    let max_width = width.saturating_sub(2).max(1);
    let mut out = Vec::new();
    let mut first = true;
    for node in root.children() {
        if !first {
            out.push(MarkdownLine {
                spans: Vec::new(),
                code: false,
                hang: String::new(),
            });
        }
        first = false;
        render_block(node, &mut out, 0, 0, max_width);
    }
    while out
        .last()
        .is_some_and(|line| line.spans.is_empty() && !line.code)
    {
        out.pop();
    }
    out
}

fn render_block<'a>(
    node: &'a AstNode<'a>,
    out: &mut Vec<MarkdownLine>,
    indent: usize,
    reserved: usize,
    max_width: usize,
) {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Paragraph => {
            for spans in inline_lines(node, theme::text()) {
                push_line(out, indent, spans, false);
            }
        }
        NodeValue::Heading(heading) => {
            let style = if heading.level <= 2 {
                Style::default()
                    .fg(theme::p().success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme::p().muted)
                    .add_modifier(Modifier::BOLD)
            };
            for spans in inline_lines(node, style) {
                push_line(out, indent, spans, false);
            }
        }
        NodeValue::CodeBlock(block) => {
            let language = syntax::language_for_fence(&block.info);
            let literal = expand_tabs(&block.literal, 4);
            let body = literal.trim_end_matches('\n');
            let pad = " ".repeat(indent);
            for spans in syntax::highlight(language, body, code_block_style()) {
                push_prefixed(out, &pad, &pad, spans, true);
            }
            if literal.is_empty() {
                push_prefixed(out, &pad, &pad, Vec::new(), true);
            }
        }
        NodeValue::List(list) => {
            render_list(node, &list, out, indent, reserved, max_width);
        }
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(..) => {
            render_blockquote(node, out, indent, reserved, max_width);
        }
        NodeValue::ThematicBreak(_) => {
            push_line(
                out,
                indent,
                vec![Span::styled(
                    "─".repeat(24),
                    Style::default().fg(theme::p().faint),
                )],
                false,
            );
        }
        NodeValue::HtmlBlock(html) => {
            for line in html.literal.trim_end_matches('\n').split('\n') {
                push_line(
                    out,
                    indent,
                    vec![Span::styled(
                        line.to_string(),
                        Style::default().fg(theme::p().faint),
                    )],
                    false,
                );
            }
        }
        NodeValue::FrontMatter(literal) => {
            for line in literal.trim_end_matches('\n').split('\n') {
                push_line(
                    out,
                    indent,
                    vec![Span::styled(
                        line.to_string(),
                        Style::default().fg(theme::p().faint),
                    )],
                    false,
                );
            }
        }
        NodeValue::Table(table) => {
            render_table(
                node,
                &table,
                out,
                indent,
                max_width.saturating_sub(reserved),
            );
        }
        NodeValue::FootnoteDefinition(definition) => {
            push_line(
                out,
                indent,
                vec![Span::styled(
                    format!("[{}]", footnote_label(&definition.name)),
                    Style::default()
                        .fg(theme::p().interactive)
                        .add_modifier(Modifier::BOLD),
                )],
                false,
            );
            for child in node.children() {
                render_block(child, out, indent + 2, reserved + 2, max_width);
            }
        }
        NodeValue::TaskItem(..) => {
            for child in node.children() {
                render_block(child, out, indent, reserved, max_width);
            }
        }
        NodeValue::DescriptionList => {
            for child in node.children() {
                render_block(child, out, indent, reserved, max_width);
            }
        }
        NodeValue::DescriptionItem(..) => {
            for child in node.children() {
                render_block(child, out, indent, reserved, max_width);
            }
        }
        NodeValue::DescriptionTerm => {
            let style = Style::default().add_modifier(Modifier::BOLD);
            for spans in inline_lines(node, style) {
                push_line(out, indent, spans, false);
            }
        }
        NodeValue::DescriptionDetails => {
            for child in node.children() {
                render_block(child, out, indent + 2, reserved + 2, max_width);
            }
        }
        NodeValue::MathBlock(block) => {
            let literal = block.literal.trim_end_matches('\n');
            let pad = " ".repeat(indent);
            for line in literal.split('\n') {
                push_prefixed(
                    out,
                    &pad,
                    &pad,
                    vec![Span::styled(line.to_string(), code_block_style())],
                    true,
                );
            }
        }
        NodeValue::Alert(alert) => {
            let title = alert
                .title
                .clone()
                .unwrap_or_else(|| alert.alert_type.default_title().to_string());
            push_line(
                out,
                indent,
                vec![Span::styled(
                    format!("[!{}]", title.to_uppercase()),
                    Style::default()
                        .fg(theme::p().warning)
                        .add_modifier(Modifier::BOLD),
                )],
                false,
            );
            for child in node.children() {
                render_block(child, out, indent + 2, reserved + 2, max_width);
            }
        }
        _ => {
            for child in node.children() {
                render_block(child, out, indent, reserved, max_width);
            }
        }
    }
}

fn render_blockquote<'a>(
    node: &'a AstNode<'a>,
    out: &mut Vec<MarkdownLine>,
    indent: usize,
    reserved: usize,
    max_width: usize,
) {
    let start = out.len();
    for child in node.children() {
        render_block(child, out, 0, reserved + 2, max_width);
    }
    let prefix = format!("{}│ ", " ".repeat(indent));
    for line in &mut out[start..] {
        let mut spans = vec![Span::styled(
            prefix.clone(),
            Style::default().fg(theme::p().faint),
        )];
        for span in std::mem::take(&mut line.spans) {
            let style = if span.style.bg.is_none() && span.style.fg == Some(theme::p().text) {
                span.style.fg(theme::p().muted)
            } else {
                span.style
            };
            spans.push(Span::styled(span.content.into_owned(), style));
        }
        line.spans = spans;
        line.hang = format!("{prefix}{}", line.hang);
    }
}

fn render_list<'a>(
    node: &'a AstNode<'a>,
    list: &NodeList,
    out: &mut Vec<MarkdownLine>,
    indent: usize,
    reserved: usize,
    max_width: usize,
) {
    let mut ordered_index = list.start;
    for item in node.children() {
        let value = item.data.borrow().value.clone();
        let task = match &value {
            NodeValue::Item(..) => None,
            NodeValue::TaskItem(task) => Some(task.symbol),
            _ => continue,
        };
        let marker = match list.list_type {
            comrak::nodes::ListType::Bullet => "• ".to_string(),
            comrak::nodes::ListType::Ordered => {
                let marker = format!("{ordered_index}. ");
                ordered_index += 1;
                marker
            }
        };
        let marker = match task {
            Some(symbol) => format!("{marker}{} ", if symbol.is_some() { "☑" } else { "☐" }),
            None => marker,
        };
        let marker_width = display_width(&marker);
        let pad = " ".repeat(indent);
        let hang = " ".repeat(indent + marker_width);
        let mut first = true;
        let mut rendered = false;
        render_item_children(
            item,
            out,
            ItemLayout {
                indent,
                reserved,
                marker: &marker,
                marker_width,
                max_width,
            },
            &mut first,
            &mut rendered,
        );
        if !rendered {
            push_prefixed(out, &format!("{pad}{marker}"), &hang, Vec::new(), false);
        }
    }
}

struct ItemLayout<'a> {
    indent: usize,
    reserved: usize,
    marker: &'a str,
    marker_width: usize,
    max_width: usize,
}

fn render_item_children<'a>(
    item: &'a AstNode<'a>,
    out: &mut Vec<MarkdownLine>,
    layout: ItemLayout<'_>,
    first: &mut bool,
    rendered: &mut bool,
) {
    let ItemLayout {
        indent,
        reserved,
        marker,
        marker_width,
        max_width,
    } = layout;
    for child in item.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::TaskItem(..) => {
                render_item_children(
                    child,
                    out,
                    ItemLayout {
                        indent,
                        reserved,
                        marker,
                        marker_width,
                        max_width,
                    },
                    first,
                    rendered,
                );
            }
            NodeValue::Paragraph => {
                let hang = " ".repeat(indent + marker_width);
                for (line_idx, spans) in inline_lines(child, theme::text()).into_iter().enumerate()
                {
                    let prefix = if *first && line_idx == 0 {
                        format!("{}{}", " ".repeat(indent), marker)
                    } else {
                        hang.clone()
                    };
                    push_prefixed(out, &prefix, &hang, spans, false);
                }
                *first = false;
                *rendered = true;
            }
            NodeValue::List(_) => {
                render_block(child, out, indent + 2, reserved + 2, max_width);
                *first = false;
                *rendered = true;
            }
            _ => {
                let inner = indent + marker_width;
                render_block(child, out, inner, reserved + marker_width, max_width);
                *first = false;
                *rendered = true;
            }
        }
    }
}

fn render_table<'a>(
    node: &'a AstNode<'a>,
    table: &comrak::nodes::NodeTable,
    out: &mut Vec<MarkdownLine>,
    indent: usize,
    available: usize,
) {
    let mut rows: Vec<(bool, Vec<Vec<Vec<Span<'static>>>>)> = Vec::new();
    for row in node.children() {
        let header = match &row.data.borrow().value {
            NodeValue::TableRow(row) => row.header,
            _ => false,
        };
        let cells = row
            .children()
            .map(|cell| inline_lines(cell, theme::text()))
            .collect();
        rows.push((header, cells));
    }
    if rows.is_empty() {
        return;
    }
    let columns = rows
        .iter()
        .map(|(_, cells)| cells.len())
        .max()
        .unwrap_or(0)
        .max(table.alignments.len());
    if columns == 0 {
        return;
    }

    let mut widths = vec![TABLE_MIN_COLUMN; columns];
    for (_, cells) in &rows {
        for (col, lines) in cells.iter().enumerate() {
            for line in lines {
                let width = line_width(line);
                widths[col] = widths[col].max(width);
            }
        }
    }
    let gaps = 3 * columns.saturating_sub(1);
    let target = available.saturating_sub(gaps).max(columns);
    while widths.iter().sum::<usize>() > target {
        let Some((col, _)) = widths.iter().enumerate().max_by_key(|(_, width)| **width) else {
            break;
        };
        if widths[col] <= 1 {
            break;
        }
        widths[col] -= 1;
    }

    let pad = " ".repeat(indent);
    let total = widths.iter().sum::<usize>() + gaps;
    let divider = Span::styled(
        "─".repeat(total.max(1)),
        Style::default().fg(theme::p().faint),
    );
    for (header, cells) in rows {
        let wrapped: Vec<Vec<Vec<Span<'static>>>> = (0..columns)
            .map(|col| {
                let lines = cells.get(col).cloned().unwrap_or_default();
                wrap_cell(&lines, widths[col])
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for line_idx in 0..height {
            let mut spans = Vec::new();
            for (col, width) in widths.iter().enumerate() {
                if col > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(theme::p().faint)));
                }
                let cell = wrapped
                    .get(col)
                    .and_then(|lines| lines.get(line_idx))
                    .cloned()
                    .unwrap_or_default();
                let used = line_width(&cell);
                let padding = width.saturating_sub(used);
                let (left, right) = match table.alignments.get(col) {
                    Some(TableAlignment::Right) => (padding, 0),
                    Some(TableAlignment::Center) => (padding / 2, padding - padding / 2),
                    _ => (0, padding),
                };
                if left > 0 {
                    spans.push(Span::raw(" ".repeat(left)));
                }
                spans.extend(cell);
                if right > 0 {
                    spans.push(Span::raw(" ".repeat(right)));
                }
            }
            push_prefixed(out, &pad, &pad, spans, false);
        }
        if header {
            push_prefixed(out, &pad, &pad, vec![divider.clone()], false);
        }
    }
}

fn inline_lines<'a>(node: &'a AstNode<'a>, base: Style) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    walk_inline(node, base, &mut lines);
    while lines.len() > 1 && lines.first().is_some_and(|spans| spans.is_empty()) {
        lines.remove(0);
    }
    while lines.len() > 1 && lines.last().is_some_and(|spans| spans.is_empty()) {
        lines.pop();
    }
    lines
}

fn walk_inline<'a>(node: &'a AstNode<'a>, style: Style, lines: &mut Vec<Vec<Span<'static>>>) {
    for child in node.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Text(text) => push_span(lines, Span::styled(text, style)),
            NodeValue::Code(code) => {
                push_span(lines, Span::styled(code.literal, inline_code_style()));
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => lines.push(Vec::new()),
            NodeValue::Strong(_) => walk_inline(child, style.add_modifier(Modifier::BOLD), lines),
            NodeValue::Emph(_) => walk_inline(child, style.add_modifier(Modifier::ITALIC), lines),
            NodeValue::Strikethrough(_) => {
                walk_inline(child, style.add_modifier(Modifier::CROSSED_OUT), lines);
            }
            NodeValue::Superscript | NodeValue::Subscript => {
                walk_inline(child, style, lines);
            }
            NodeValue::EscapedTag(tag) => push_span(lines, Span::styled(tag, style)),
            NodeValue::Link(link) => {
                let label = plain_text(child);
                walk_inline(child, Style::default().fg(theme::p().code_function), lines);
                if !link_is_redundant(&label, &link.url) {
                    push_span(
                        lines,
                        Span::styled(
                            format!(" ({})", link.url),
                            Style::default().fg(theme::p().faint),
                        ),
                    );
                }
            }
            NodeValue::Image(link) => {
                let label = plain_text(child);
                walk_inline(child, style.add_modifier(Modifier::ITALIC), lines);
                if label.trim().is_empty() {
                    push_span(
                        lines,
                        Span::styled(link.url.clone(), Style::default().fg(theme::p().faint)),
                    );
                } else if !link_is_redundant(&label, &link.url) {
                    push_span(
                        lines,
                        Span::styled(
                            format!(" ({})", link.url),
                            Style::default().fg(theme::p().faint),
                        ),
                    );
                }
            }
            NodeValue::FootnoteReference(reference) => {
                push_span(
                    lines,
                    Span::styled(
                        format!("[{}]", footnote_label(&reference.name)),
                        Style::default().fg(theme::p().interactive),
                    ),
                );
            }
            NodeValue::Math(math) => {
                let literal = math.literal.trim().replace('\n', " ");
                push_span(lines, Span::styled(literal, inline_code_style()));
            }
            NodeValue::HtmlInline(html) => {
                push_span(
                    lines,
                    Span::styled(html, Style::default().fg(theme::p().faint)),
                );
            }
            _ => walk_inline(child, style, lines),
        }
    }
}

fn push_span(lines: &mut Vec<Vec<Span<'static>>>, span: Span<'static>) {
    let Some(line) = lines.last_mut() else {
        return;
    };
    if let Some(last) = line.last_mut()
        && last.style == span.style
    {
        last.content.to_mut().push_str(span.content.as_ref());
        return;
    }
    line.push(span);
}

fn push_line(out: &mut Vec<MarkdownLine>, indent: usize, spans: Vec<Span<'static>>, code: bool) {
    let pad = " ".repeat(indent);
    push_prefixed(out, &pad, &pad, spans, code);
}

fn push_prefixed(
    out: &mut Vec<MarkdownLine>,
    prefix: &str,
    hang: &str,
    spans: Vec<Span<'static>>,
    code: bool,
) {
    let mut line_spans = Vec::with_capacity(spans.len() + 1);
    if !prefix.is_empty() {
        line_spans.push(Span::raw(prefix.to_string()));
    }
    line_spans.extend(spans);
    out.push(MarkdownLine {
        spans: line_spans,
        code,
        hang: hang.to_string(),
    });
}

fn plain_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut out = String::new();
    collect_plain(node, &mut out);
    out
}

fn collect_plain<'a>(node: &'a AstNode<'a>, out: &mut String) {
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(text) => out.push_str(text),
            NodeValue::Code(code) => out.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            NodeValue::HtmlInline(html) => out.push_str(html),
            _ => collect_plain(child, out),
        }
    }
}

fn footnote_label(name: &str) -> &str {
    name.strip_prefix("__inline_").unwrap_or(name)
}

fn link_is_redundant(label: &str, url: &str) -> bool {
    let label = label.trim();
    if label.is_empty() {
        return false;
    }
    let url = url.trim();
    let bare = url.strip_prefix("mailto:").unwrap_or(url);
    let bare = strip_scheme(bare).trim_end_matches('/');
    let label = strip_scheme(label).trim_end_matches('/');
    bare == label
}

fn strip_scheme(text: &str) -> &str {
    text.strip_prefix("http://")
        .or_else(|| text.strip_prefix("https://"))
        .unwrap_or(text)
}

fn expand_tabs(text: &str, tab: usize) -> String {
    let mut out = String::new();
    let mut column = 0usize;
    for ch in text.chars() {
        if ch == '\t' {
            let spaces = tab - column % tab;
            out.push_str(&" ".repeat(spaces));
            column += spaces;
            continue;
        }
        out.push(ch);
        if ch == '\n' {
            column = 0;
        } else {
            column += char_width(ch);
        }
    }
    out
}

fn wrap_cell(lines: &[Vec<Span<'static>>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in lines {
        let flat: Vec<(char, Style)> = line
            .iter()
            .flat_map(|span| span.content.chars().map(|ch| (ch, span.style)))
            .collect();
        if flat.is_empty() {
            out.push(Vec::new());
            continue;
        }
        let mut start = 0;
        while start < flat.len() {
            let mut column = 0usize;
            let mut end = start;
            while end < flat.len() {
                let next = char_width(flat[end].0);
                if end > start && column + next > width {
                    break;
                }
                column += next;
                end += 1;
            }
            if end < flat.len()
                && let Some(position) = (start..end).rev().find(|&idx| flat[idx].0 == ' ')
                && position > start
            {
                end = position;
            }
            out.push(spans_from_chars(&flat[start..end]));
            start = end;
        }
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

fn spans_from_chars(chars: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, style) in chars {
        if let Some(last) = spans.last_mut()
            && last.style == *style
        {
            last.content.to_mut().push(*ch);
        } else {
            spans.push(Span::styled(ch.to_string(), *style));
        }
    }
    spans
}

fn line_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn char_width(ch: char) -> usize {
    if ch == '\t' {
        1
    } else {
        UnicodeWidthChar::width(ch).unwrap_or(0)
    }
}

fn code_block_style() -> Style {
    Style::default()
        .fg(theme::p().text)
        .bg(theme::p().surface_high)
}

fn inline_code_style() -> Style {
    Style::default().fg(theme::p().warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(line: &MarkdownLine) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn rows(source: &str, width: usize) -> Vec<String> {
        render(source, width).iter().map(row_text).collect()
    }

    fn find_span<'a>(lines: &'a [MarkdownLine], text: &str) -> &'a Span<'static> {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content == text)
            .expect("span")
    }

    #[test]
    fn renders_code_blocks_with_a_background() {
        let lines = render("Run it:\n\n```sh\ncargo test --workspace\n```\n", 80);
        let code: Vec<&MarkdownLine> = lines.iter().filter(|line| line.code).collect();
        assert_eq!(code.len(), 1);
        assert_eq!(row_text(code[0]), "cargo test --workspace");
        assert!(
            code[0]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme::p().surface_high))
        );
    }

    #[test]
    fn renders_inline_code_bold_and_lists() {
        let lines = render(
            "Use `cargo test` now.\n\n**Stop** here\n\n- one\n- two\n",
            80,
        );
        let code = find_span(&lines, "cargo test");
        assert_eq!(code.style.fg, Some(theme::p().warning));
        assert_eq!(code.style.bg, None);

        let stop = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains("Stop")))
            .expect("bold line");
        assert!(
            stop.spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );

        let bullets: Vec<&MarkdownLine> = lines
            .iter()
            .filter(|line| line.spans.iter().any(|span| span.content.contains('•')))
            .collect();
        assert_eq!(bullets.len(), 2);
    }

    #[test]
    fn renders_headings_blockquotes_and_rules() {
        let lines = render("# Title\n\n> quoted\n\n---\n", 80);
        let title = find_span(&lines, "Title");
        assert_eq!(title.style.fg, Some(theme::p().success));
        assert!(title.style.add_modifier.contains(Modifier::BOLD));

        let quote = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains("quoted"))
            })
            .expect("quoted line");
        assert!(quote.spans.iter().any(|span| span.content.contains('│')));
        let quoted = quote
            .spans
            .iter()
            .find(|span| span.content.contains("quoted"))
            .expect("text");
        assert_eq!(quoted.style.fg, Some(theme::p().muted));

        assert!(
            lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content.contains('─')))
        );
    }

    #[test]
    fn leaves_unbalanced_backticks_as_text() {
        let lines = render("a `broken command\n", 80);
        assert_eq!(rows("a `broken command\n", 80)[0], "a `broken command");
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content == "a `broken command")
        );
    }

    #[test]
    fn highlights_fenced_code_and_keeps_table_cell_styles() {
        let lines = render(
            "```rust\nlet value = 1;\n```\n\n| file | note |\n| --- | --- |\n| `src/main.rs` | **new** |\n",
            80,
        );

        let code = lines.iter().find(|line| line.code).expect("code block");
        assert!(code.spans.iter().any(|span| span.content == "let"));
        assert!(
            code.spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );

        let path = find_span(&lines, "src/main.rs");
        assert_eq!(path.style.bg, None);
        assert_eq!(path.style.fg, Some(theme::p().warning));

        assert!(
            lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content.contains('─')))
        );
    }

    #[test]
    fn task_lists_render_checkboxes() {
        let text = rows("- [ ] open\n- [x] done\n", 80);
        assert!(text.iter().any(|row| row.contains("☐ open")));
        assert!(text.iter().any(|row| row.contains("☑ done")));
    }

    #[test]
    fn ordered_lists_keep_the_start_number() {
        let text = rows("3. three\n4. four\n   1. sub\n", 80);
        assert_eq!(text, vec!["3. three", "4. four", "  1. sub"]);
    }

    #[test]
    fn list_item_starting_with_a_code_block_has_no_stray_marker() {
        let lines = render("- ```\n  code\n  ```\n", 80);
        let text: Vec<String> = lines.iter().map(row_text).collect();
        assert!(text.iter().any(|row| row.trim() == "code"));
        assert!(!text.iter().any(|row| row.trim() == "•"));
        assert!(lines.iter().any(|line| line.code));
    }

    #[test]
    fn table_divider_keeps_list_indentation() {
        let text = rows("- item\n\n  | a |\n  | --- |\n  | 1 |\n", 80);
        let divider = text
            .iter()
            .find(|row| row.contains('─'))
            .expect("table divider");
        assert!(divider.starts_with("  "), "{divider:?}");
    }

    #[test]
    fn wide_tables_fit_the_available_width() {
        let source =
            "| aaa | bbb |\n| --- | --- |\n| ccccccccccccccccccccccccc | ddddddddddddddddddd |\n";
        let lines = render(source, 40);
        for line in &lines {
            let text = row_text(line);
            if !text.is_empty() {
                assert!(
                    UnicodeWidthStr::width(text.as_str()) <= 38,
                    "{text:?} is wider than 38"
                );
            }
        }
        assert!(rows(source, 40).iter().any(|row| row.contains('─')));
    }

    #[test]
    fn tables_use_display_width_for_wide_characters() {
        let source = "| 名前 | 説明 |\n| --- | --- |\n| テスト | 値 |\n";
        let text = rows(source, 80);
        assert_eq!(text.len(), 3);
        let widths: Vec<usize> = text.iter().map(|row| display_width(row)).collect();
        assert!(widths.iter().all(|width| *width == widths[0]), "{widths:?}");
    }

    #[test]
    fn table_alignment_pads_the_right_edge() {
        let text = rows("| a | b |\n| --- | ---: |\n| 1 | 2 |\n", 80);
        assert!(text[0].ends_with('b'), "{:?}", text[0]);
        assert!(text[2].ends_with('2'), "{:?}", text[2]);
    }

    #[test]
    fn links_show_their_target() {
        let text = rows("[docs](https://example.com/x)\n", 80);
        assert!(text[0].contains("docs"));
        assert!(text[0].contains("(https://example.com/x)"), "{:?}", text[0]);
    }

    #[test]
    fn autolinks_do_not_repeat_the_url() {
        let text = rows("<https://example.com/x>\n", 80);
        assert_eq!(text[0].matches("https://example.com/x").count(), 1);
    }

    #[test]
    fn footnotes_render_markers_and_definitions() {
        let text = rows("text[^1]\n\n[^1]: note\n", 80);
        assert!(text.iter().any(|row| row.contains("text[1]")), "{text:?}");
        assert!(text.iter().any(|row| row.trim() == "[1]"), "{text:?}");
        assert!(text.iter().any(|row| row.trim() == "note"), "{text:?}");
    }

    #[test]
    fn alerts_render_their_label() {
        let text = rows("> [!NOTE]\n> body\n", 80);
        assert!(text.iter().any(|row| row.contains("[!NOTE]")), "{text:?}");
        assert!(text.iter().any(|row| row.trim() == "body"), "{text:?}");
    }

    #[test]
    fn description_lists_render_term_and_details() {
        let text = rows("Term\n\n: Definition\n", 80);
        assert!(text.iter().any(|row| row == "Term"), "{text:?}");
        assert!(
            text.iter().any(|row| row.trim() == "Definition"),
            "{text:?}"
        );
    }

    #[test]
    fn math_renders_its_source() {
        let text = rows("inline $x^2$ end\n", 80);
        assert!(text[0].contains("x^2"), "{text:?}");

        let lines = render("$$\nx^2\n$$\n", 80);
        assert!(
            lines
                .iter()
                .any(|line| line.code && row_text(line) == "x^2")
        );
    }

    #[test]
    fn front_matter_renders_as_text() {
        let text = rows("---\ntitle: x\n---\n\nbody\n", 80);
        assert!(text.iter().any(|row| row.contains("title: x")), "{text:?}");
        assert!(text.iter().any(|row| row.trim() == "body"), "{text:?}");
    }

    #[test]
    fn code_tabs_expand_to_spaces() {
        let lines = render("```\n\ta\n```\n", 80);
        let code = lines.iter().find(|line| line.code).expect("code");
        assert_eq!(row_text(code), "    a");
    }

    #[test]
    fn blockquotes_keep_inline_code_styling() {
        let lines = render("> `code`\n", 80);
        let code = find_span(&lines, "code");
        assert_eq!(code.style.fg, Some(theme::p().warning));
    }

    #[test]
    fn empty_list_items_render_their_marker() {
        let text = rows("- \n- b\n", 80);
        assert_eq!(text, vec!["• ", "• b"]);
    }

    #[test]
    fn nested_blockquotes_keep_both_bars() {
        let lines = render("> outer\n> > inner\n", 80);
        let inner = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains("inner")))
            .expect("inner line");
        assert!(
            row_text(inner).starts_with("│ │ inner"),
            "{:?}",
            row_text(inner)
        );
        assert_eq!(inner.hang, "│ │ ");
    }
}
