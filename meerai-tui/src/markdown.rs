use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

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
    for node in root.children() {
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
            let literal = block.literal;
            for line in literal.trim_end_matches('\n').split('\n') {
                out.push(MarkdownLine {
                    spans: vec![Span::styled(line.to_string(), code_style())],
                    code: true,
                });
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
            for row in node.children() {
                let mut spans = Vec::new();
                for cell in row.children() {
                    if !spans.is_empty() {
                        spans.push(Span::styled(" │ ", Style::default().fg(theme::p().faint)));
                    }
                    let text = plain_text(cell);
                    if !text.is_empty() {
                        spans.push(Span::styled(text, theme::text()));
                    }
                }
                push_line(out, indent, spans, false);
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
    lines.retain(|spans| !spans.is_empty());
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

fn walk_inline<'a>(node: &'a AstNode<'a>, style: Style, lines: &mut Vec<Vec<Span<'static>>>) {
    for child in node.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Text(text) => push_span(lines, Span::styled(text, style)),
            NodeValue::Code(code) => {
                push_span(lines, Span::styled(code.literal, code_style()));
            }
            NodeValue::SoftBreak => push_span(lines, Span::styled(" ", style)),
            NodeValue::LineBreak => lines.push(Vec::new()),
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

fn plain_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    for child in node.descendants() {
        match &child.data.borrow().value {
            NodeValue::Text(value) => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(value);
            }
            NodeValue::Code(comrak::nodes::NodeCode { literal, .. }) => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(literal);
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            _ => {}
        }
    }
    text
}

fn code_style() -> Style {
    Style::default()
        .fg(theme::p().text)
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
        assert_eq!(code[0].spans[0].content, "cargo test --workspace");
        assert_eq!(code[0].spans[0].style.bg, Some(theme::p().surface_high));
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
}
