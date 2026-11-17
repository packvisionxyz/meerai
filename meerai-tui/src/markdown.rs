use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::syntax;
use crate::theme;

pub struct MarkdownLine {
    pub spans: Vec<Span<'static>>,
    pub code: bool,
}

pub fn render(text: &str) -> Vec<MarkdownLine> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.table = true;

    let arena = Arena::new();
    let root = parse_document(&arena, text, &options);
    let mut out = Vec::new();
    let mut first = true;
    for node in root.children() {
        if !first {
            out.push(MarkdownLine {
                spans: Vec::new(),
                code: false,
            });
        }
        first = false;
        render_block(node, &mut out, 0);
    }
    while out
        .last()
        .is_some_and(|line| line.spans.is_empty() && !line.code)
    {
        out.pop();
    }
    out
}

fn render_block<'a>(node: &'a AstNode<'a>, out: &mut Vec<MarkdownLine>, indent: usize) {
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
                    .fg(theme::p().text)
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
            let literal = block.literal;
            let body = literal.trim_end_matches('\n');
            for spans in syntax::highlight(language, body, code_block_style()) {
                out.push(MarkdownLine { spans, code: true });
            }
            if literal.is_empty() {
                out.push(MarkdownLine {
                    spans: Vec::new(),
                    code: true,
                });
            }
        }
        NodeValue::List(list) => {
            render_list(node, &list, out, indent);
        }
        NodeValue::BlockQuote => {
            let start = out.len();
            for child in node.children() {
                render_block(child, out, indent);
            }
            for line in &mut out[start..] {
                let mut spans = vec![Span::styled(
                    format!("{}│ ", " ".repeat(indent)),
                    Style::default().fg(theme::p().faint),
                )];
                spans.extend(std::mem::take(&mut line.spans));
                for span in &mut spans[1..] {
                    span.style = span
                        .style
                        .fg(theme::p().muted)
                        .add_modifier(Modifier::ITALIC);
                }
                line.spans = spans;
            }
        }
        NodeValue::ThematicBreak => {
            out.push(MarkdownLine {
                spans: vec![Span::styled(
                    "─".repeat(24),
                    Style::default().fg(theme::p().faint),
                )],
                code: false,
            });
        }
        NodeValue::HtmlBlock(html) => {
            for line in html.literal.trim_end_matches('\n').split('\n') {
                out.push(MarkdownLine {
                    spans: vec![Span::styled(
                        line.to_string(),
                        Style::default().fg(theme::p().faint),
                    )],
                    code: false,
                });
            }
        }
        NodeValue::Table(..) => {
            let rows: Vec<Vec<Vec<Span<'static>>>> = node
                .children()
                .map(|row| {
                    row.children()
                        .map(|cell| {
                            inline_lines(cell, theme::text())
                                .into_iter()
                                .flatten()
                                .collect()
                        })
                        .collect()
                })
                .collect();
            let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
            let widths: Vec<usize> = (0..columns)
                .map(|col| {
                    rows.iter()
                        .filter_map(|row| row.get(col))
                        .map(|spans| spans.iter().map(|span| span.content.chars().count()).sum())
                        .max()
                        .unwrap_or(0)
                })
                .collect();
            let total_width = widths.iter().sum::<usize>() + 3 * columns.saturating_sub(1);
            for (row_idx, row) in rows.iter().enumerate() {
                let mut spans = Vec::new();
                for (col, cell) in row.iter().enumerate() {
                    if col > 0 {
                        spans.push(Span::styled(" │ ", Style::default().fg(theme::p().faint)));
                    }
                    spans.extend(cell.iter().cloned());
                    if col + 1 < columns {
                        let used: usize =
                            cell.iter().map(|span| span.content.chars().count()).sum();
                        spans.push(Span::raw(" ".repeat(widths[col].saturating_sub(used))));
                    }
                }
                push_line(out, indent, spans, false);
                if row_idx == 0 {
                    out.push(MarkdownLine {
                        spans: vec![Span::styled(
                            "─".repeat(total_width.clamp(1, 80)),
                            Style::default().fg(theme::p().faint),
                        )],
                        code: false,
                    });
                }
            }
        }
        NodeValue::DescriptionList | NodeValue::DescriptionItem(..) => {
            for child in node.children() {
                render_block(child, out, indent);
            }
        }
        _ => {
            for child in node.children() {
                render_block(child, out, indent);
            }
        }
    }
}

fn render_list<'a>(
    node: &'a AstNode<'a>,
    list: &comrak::nodes::NodeList,
    out: &mut Vec<MarkdownLine>,
    indent: usize,
) {
    let mut ordered_index = list.start;
    for item in node.children() {
        let value = item.data.borrow().value.clone();
        if !matches!(value, NodeValue::Item(..)) {
            continue;
        }
        let marker = match list.list_type {
            comrak::nodes::ListType::Bullet => "• ".to_string(),
            comrak::nodes::ListType::Ordered => {
                let marker = format!("{ordered_index}. ");
                ordered_index += 1;
                marker
            }
        };
        let mut first = true;
        for child in item.children() {
            let child_value = child.data.borrow().value.clone();
            match child_value {
                NodeValue::Paragraph => {
                    for spans in inline_lines(child, theme::text()) {
                        let prefix = if first {
                            format!("{}{marker}", " ".repeat(indent))
                        } else {
                            " ".repeat(indent + marker.chars().count())
                        };
                        let mut line_spans =
                            vec![Span::styled(prefix, Style::default().fg(theme::p().faint))];
                        line_spans.extend(spans);
                        push_spans(out, line_spans, false);
                        first = false;
                    }
                }
                NodeValue::List(_) => {
                    render_block(child, out, indent + 2);
                }
                _ => render_block(child, out, indent + marker.chars().count()),
            }
        }
        if first {
            push_line(
                out,
                indent,
                vec![Span::styled(marker, theme::text())],
                false,
            );
        }
    }
}

fn push_line(out: &mut Vec<MarkdownLine>, indent: usize, spans: Vec<Span<'static>>, code: bool) {
    let mut line_spans = Vec::with_capacity(spans.len() + 1);
    if indent > 0 {
        line_spans.push(Span::raw(" ".repeat(indent)));
    }
    line_spans.extend(spans);
    push_spans(out, line_spans, code);
}

fn push_spans(out: &mut Vec<MarkdownLine>, spans: Vec<Span<'static>>, code: bool) {
    if spans.is_empty() {
        out.push(MarkdownLine { spans, code });
        return;
    }
    out.push(MarkdownLine { spans, code });
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
            NodeValue::Strong => walk_inline(child, style.add_modifier(Modifier::BOLD), lines),
            NodeValue::Emph => walk_inline(child, style.add_modifier(Modifier::ITALIC), lines),
            NodeValue::Strikethrough => {
                walk_inline(child, style.add_modifier(Modifier::CROSSED_OUT), lines);
            }
            NodeValue::Link(..) => {
                walk_inline(child, Style::default().fg(theme::p().accent), lines);
            }
            NodeValue::Image(..) => {
                walk_inline(child, style.add_modifier(Modifier::ITALIC), lines);
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

fn code_block_style() -> Style {
    Style::default()
        .fg(theme::p().text)
        .bg(theme::p().surface_high)
}

fn inline_code_style() -> Style {
    Style::default()
        .fg(theme::p().accent)
        .bg(theme::p().surface_high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_code_blocks_with_a_background() {
        let lines = render("Run it:\n\n```sh\ncargo test --workspace\n```\n");
        let code: Vec<&MarkdownLine> = lines.iter().filter(|line| line.code).collect();
        assert_eq!(code.len(), 1);
        let text: String = code[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "cargo test --workspace");
        assert!(
            code[0]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme::p().surface_high))
        );
    }

    #[test]
    fn renders_inline_code_bold_and_lists() {
        let lines = render("Use `cargo test` now.\n\n**Stop** here\n\n- one\n- two\n");
        let first = &lines[0];
        let code = first
            .spans
            .iter()
            .find(|span| span.content == "cargo test")
            .expect("inline code span");
        assert_eq!(code.style.bg, Some(theme::p().surface_high));

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
        let lines = render("# Title\n\n> quoted\n\n---\n");
        assert!(lines[0].spans.iter().any(|span| span.content == "Title"));
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| span.content.contains('│'))
                && line
                    .spans
                    .iter()
                    .any(|span| span.content.contains("quoted"))
                && line
                    .spans
                    .iter()
                    .any(|span| span.style.add_modifier.contains(Modifier::ITALIC))
        }));
        assert!(
            lines
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.contains('─')) })
        );
    }

    #[test]
    fn leaves_unbalanced_backticks_as_text() {
        let lines = render("a `broken command\n");
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect();
        assert_eq!(text, "a `broken command");
    }

    #[test]
    fn highlights_fenced_code_and_keeps_table_cell_styles() {
        let lines = render(
            "```rust\nlet value = 1;\n```\n\n| file | note |\n| --- | --- |\n| `src/main.rs` | **new** |\n",
        );

        let code = lines.iter().find(|line| line.code).expect("code block");
        assert!(code.spans.iter().any(|span| span.content == "let"));
        assert!(
            code.spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );

        let path = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content == "src/main.rs")
            .expect("table path");
        assert_eq!(path.style.bg, Some(theme::p().surface_high));
        assert_eq!(path.style.fg, Some(theme::p().accent));

        assert!(
            lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content.contains('─')))
        );
    }
}
