use comrak::nodes::{AlertType, AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

use crate::highlight::highlight_code;

pub fn render_markdown(source: &str) -> AnyView {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
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

    let root = parse_document(&arena, source, &options);
    let blocks = root.children().filter_map(render_block).collect::<Vec<_>>();
    view! { <div class="markdown">{AnyView::from(Fragment::new(blocks))}</div> }.into_any()
}

fn render_block<'a>(node: &'a AstNode<'a>) -> Option<AnyView> {
    let value = &node.data.borrow().value;
    match value {
        NodeValue::Paragraph => {
            Some(view! { <p class="md-p">{render_inlines(node)}</p> }.into_any())
        }
        NodeValue::Heading(heading) => {
            let inner = render_inlines(node);
            let view = match heading.level {
                1 => view! { <h1 class="md-h">{inner}</h1> }.into_any(),
                2 => view! { <h2 class="md-h">{inner}</h2> }.into_any(),
                3 => view! { <h3 class="md-h">{inner}</h3> }.into_any(),
                4 => view! { <h4 class="md-h">{inner}</h4> }.into_any(),
                5 => view! { <h5 class="md-h">{inner}</h5> }.into_any(),
                _ => view! { <h6 class="md-h">{inner}</h6> }.into_any(),
            };
            Some(view)
        }
        NodeValue::CodeBlock(block) => Some(render_code_block(
            block.info.trim(),
            block.literal.trim_end_matches('\n'),
        )),
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(..) => Some(render_blockquote(node)),
        NodeValue::List(list) => {
            let items = node
                .children()
                .map(|item| {
                    let blocks = item.children().filter_map(render_block).collect::<Vec<_>>();
                    let inner = AnyView::from(Fragment::new(blocks));
                    match &item.data.borrow().value {
                        NodeValue::TaskItem(task) if task.symbol.is_some() => view! {
                            <li class="md-task md-task-done" role="checkbox" aria-checked="true">
                                {inner}
                            </li>
                        }
                        .into_any(),
                        NodeValue::TaskItem(_) => view! {
                            <li class="md-task md-task-open" role="checkbox" aria-checked="false">
                                {inner}
                            </li>
                        }
                        .into_any(),
                        _ => view! { <li>{inner}</li> }.into_any(),
                    }
                })
                .collect::<Vec<_>>();
            let items = AnyView::from(Fragment::new(items));
            Some(match list.list_type {
                ListType::Ordered => view! { <ol class="md-list">{items}</ol> }.into_any(),
                _ => view! { <ul class="md-list">{items}</ul> }.into_any(),
            })
        }
        NodeValue::TaskItem(_) => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            (!blocks.is_empty()).then(|| AnyView::from(Fragment::new(blocks)))
        }
        NodeValue::Table(table) => Some(render_table(node, &table.alignments)),
        NodeValue::ThematicBreak(_) => Some(view! { <hr class="md-rule" /> }.into_any()),
        NodeValue::HtmlBlock(html) => {
            let text = html.literal.trim().to_owned();
            (!text.is_empty()).then(|| view! { <pre class="md-html">{text}</pre> }.into_any())
        }
        NodeValue::FrontMatter(text) => {
            let text = text.trim().to_owned();
            (!text.is_empty())
                .then(|| view! { <pre class="md-frontmatter">{text}</pre> }.into_any())
        }
        NodeValue::FootnoteDefinition(definition) => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            let label = footnote_label(&definition.name);
            let id = format!("fn-{}", definition.name);
            Some(
                view! {
                    <div class="md-footnote" id=id>
                        <span class="md-footnote-label">{format!("[{label}]")}</span>
                        <div class="md-footnote-body">{AnyView::from(Fragment::new(blocks))}</div>
                    </div>
                }
                .into_any(),
            )
        }
        NodeValue::MathBlock(math) => {
            let text = math.literal.trim_end_matches('\n').to_owned();
            Some(view! { <pre class="md-math-block">{text}</pre> }.into_any())
        }
        NodeValue::Alert(alert) => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            let kind = alert_kind(&alert.alert_type);
            let title = alert
                .title
                .clone()
                .unwrap_or_else(|| alert.alert_type.default_title().to_string());
            let class = format!("md-alert md-alert-{kind}");
            Some(
                view! {
                    <div class=class>
                        <div class="md-alert-title">{title}</div>
                        {AnyView::from(Fragment::new(blocks))}
                    </div>
                }
                .into_any(),
            )
        }
        NodeValue::DescriptionList => {
            let items = node.children().filter_map(render_block).collect::<Vec<_>>();
            Some(view! { <dl class="md-dl">{AnyView::from(Fragment::new(items))}</dl> }.into_any())
        }
        NodeValue::DescriptionItem(..) => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            (!blocks.is_empty()).then(|| AnyView::from(Fragment::new(blocks)))
        }
        NodeValue::DescriptionTerm => {
            Some(view! { <dt class="md-dt">{render_inlines(node)}</dt> }.into_any())
        }
        NodeValue::DescriptionDetails => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            Some(view! { <dd class="md-dd">{AnyView::from(Fragment::new(blocks))}</dd> }.into_any())
        }
        _ => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            (!blocks.is_empty()).then(|| AnyView::from(Fragment::new(blocks)))
        }
    }
}

fn render_blockquote<'a>(node: &'a AstNode<'a>) -> AnyView {
    let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
    view! {
        <blockquote class="md-quote">
            {AnyView::from(Fragment::new(blocks))}
        </blockquote>
    }
    .into_any()
}

fn render_table<'a>(node: &'a AstNode<'a>, alignments: &[TableAlignment]) -> AnyView {
    let mut head = Vec::new();
    let mut body = Vec::new();
    for row in node.children() {
        let header = matches!(
            &row.data.borrow().value,
            NodeValue::TableRow(row) if row.header
        );
        let cells = row
            .children()
            .enumerate()
            .map(|(column, cell)| {
                let inner = render_inlines(cell);
                let style = alignment_style(alignments.get(column)).to_string();
                if header {
                    view! { <th style=style.clone()>{inner}</th> }.into_any()
                } else {
                    view! { <td style=style.clone()>{inner}</td> }.into_any()
                }
            })
            .collect::<Vec<_>>();
        let cells = AnyView::from(Fragment::new(cells));
        let row = view! { <tr>{cells}</tr> }.into_any();
        if header {
            head.push(row);
        } else {
            body.push(row);
        }
    }
    view! {
        <div class="md-table-wrap">
            <table class="md-table">
                <thead>{AnyView::from(Fragment::new(head))}</thead>
                <tbody>{AnyView::from(Fragment::new(body))}</tbody>
            </table>
        </div>
    }
    .into_any()
}

fn render_inlines<'a>(node: &'a AstNode<'a>) -> AnyView {
    AnyView::from(Fragment::new(collect_inlines(node)))
}

fn collect_inlines<'a>(node: &'a AstNode<'a>) -> Vec<AnyView> {
    let mut out = Vec::new();
    for child in node.children() {
        collect_inline(child, &mut out);
    }
    out
}

fn collect_inline<'a>(node: &'a AstNode<'a>, out: &mut Vec<AnyView>) {
    let value = &node.data.borrow().value;
    match value {
        NodeValue::Text(text) => out.push(text.clone().into_any()),
        NodeValue::SoftBreak => out.push(view! { <br /> }.into_any()),
        NodeValue::LineBreak => out.push(view! { <br /> }.into_any()),
        NodeValue::Code(code) => {
            let text = code.literal.to_owned();
            // The server re-validates through /open, so this only decides what looks clickable.
            match crate::pathlink::path_command(&text) {
                Some(command) => out.push(
                    view! {
                        <code
                            class="md-code md-code-path"
                            title="Open in editor"
                            on:click=move |_| {
                                if let Some(transport) = crate::transport::current() {
                                    transport.run_host_action(command.clone());
                                }
                            }
                        >
                            {text}
                        </code>
                    }
                    .into_any(),
                ),
                None => out.push(view! { <code class="md-code">{text}</code> }.into_any()),
            }
        }
        NodeValue::HtmlInline(raw) => out.push(raw.to_owned().into_any()),
        NodeValue::Strong(_) => {
            let inner = render_inlines(node);
            out.push(view! { <strong>{inner}</strong> }.into_any());
        }
        NodeValue::Emph(_) => {
            let inner = render_inlines(node);
            out.push(view! { <em>{inner}</em> }.into_any());
        }
        NodeValue::Strikethrough(_) => {
            let inner = render_inlines(node);
            out.push(view! { <del>{inner}</del> }.into_any());
        }
        NodeValue::Superscript => {
            let inner = render_inlines(node);
            out.push(view! { <sup>{inner}</sup> }.into_any());
        }
        NodeValue::Subscript => {
            let inner = render_inlines(node);
            out.push(view! { <sub>{inner}</sub> }.into_any());
        }
        NodeValue::EscapedTag(tag) => out.push((*tag).to_owned().into_any()),
        NodeValue::Link(link) => {
            let inner = render_inlines(node);
            match safe_url(&link.url) {
                Some(url) => out.push(
                    view! {
                        <a class="md-link" href=url target="_blank" rel="noopener noreferrer" referrerpolicy="no-referrer">
                            {inner}
                        </a>
                    }
                    .into_any(),
                ),
                None => out.push(view! { <span class="md-link-blocked">{inner}</span> }.into_any()),
            }
        }
        NodeValue::Image(link) => {
            let alt = collect_inlines(node);
            let label: AnyView = if alt.is_empty() {
                link.url.to_owned().into_any()
            } else {
                AnyView::from(Fragment::new(alt))
            };
            match safe_url(&link.url) {
                Some(url) => out.push(
                    view! {
                        <a class="md-image" href=url target="_blank" rel="noopener noreferrer" referrerpolicy="no-referrer">
                            {label}
                        </a>
                    }
                    .into_any(),
                ),
                None => out.push(view! { <span class="md-link-blocked">{label}</span> }.into_any()),
            }
        }
        NodeValue::FootnoteReference(reference) => {
            let label = footnote_label(&reference.name);
            out.push(
                view! { <sup class="md-footnote-ref">{format!("[{label}]")}</sup> }.into_any(),
            );
        }
        NodeValue::Math(math) => {
            let literal = math.literal.trim().replace('\n', " ");
            out.push(view! { <code class="md-math">{literal}</code> }.into_any());
        }
        _ => {
            for child in node.children() {
                collect_inline(child, out);
            }
        }
    }
}

fn safe_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:"))
        .then(|| url.to_owned())
}

fn alignment_style(alignment: Option<&TableAlignment>) -> &'static str {
    match alignment {
        Some(TableAlignment::Center) => "text-align: center",
        Some(TableAlignment::Right) => "text-align: right",
        _ => "text-align: left",
    }
}

fn alert_kind(alert: &AlertType) -> &'static str {
    match alert {
        AlertType::Note => "note",
        AlertType::Tip => "tip",
        AlertType::Important => "important",
        AlertType::Warning => "warning",
        AlertType::Caution => "caution",
        _ => "note",
    }
}

fn footnote_label(name: &str) -> &str {
    name.strip_prefix("__inline_").unwrap_or(name)
}

fn render_code_block(lang: &str, code: &str) -> AnyView {
    let label = if lang.is_empty() {
        "text".to_owned()
    } else {
        lang.to_owned()
    };
    let spans = highlight_code(lang, code);
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    for span in spans {
        if span.start > cursor {
            parts.push(code[cursor..span.start].to_owned().into_any());
        }
        let text = code[span.start..span.end].to_owned();
        let class = span.kind.class();
        parts.push(view! { <span class=class>{text}</span> }.into_any());
        cursor = span.end;
    }
    if cursor < code.len() {
        parts.push(code[cursor..].to_owned().into_any());
    }
    let code_text = code.to_owned();
    let body = AnyView::from(Fragment::new(parts));
    view! {
        <div class="md-code-block">
            <div class="md-code-head">
                <span class="md-code-lang">{label}</span>
                <button
                    class="md-code-copy"
                    title="Copy code"
                    on:click=move |_| crate::store::copy_text(code_text.clone())
                >
                    <crate::views::icons::IconCopy />
                    <span>"Copy"</span>
                </button>
            </div>
            <pre><code>{body}</code></pre>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathlink::path_command;

    #[test]
    fn clicks_a_workspace_path() {
        assert_eq!(
            path_command("src/main.rs"),
            Some("/open src/main.rs".to_string())
        );
        assert_eq!(
            path_command("meerai-tui/src/app/mod.rs:530"),
            Some("/open meerai-tui/src/app/mod.rs:530".to_string())
        );
    }

    #[test]
    fn ignores_words_urls_and_unknown_extensions() {
        assert_eq!(path_command("cargo"), None);
        assert_eq!(path_command("memory-search"), None);
        assert_eq!(path_command("https://example.com/a.rs"), None);
        assert_eq!(path_command("archive.unknownext"), None);
        assert_eq!(path_command(""), None);
    }

    #[test]
    fn ignores_absolute_and_escaping_paths() {
        assert_eq!(path_command("/etc/passwd"), None);
        assert_eq!(path_command("../outside.rs"), None);
        assert_eq!(path_command("src/../../main.rs"), None);
    }

    #[test]
    fn maps_table_alignment_to_css() {
        assert_eq!(
            alignment_style(Some(&TableAlignment::Right)),
            "text-align: right"
        );
        assert_eq!(
            alignment_style(Some(&TableAlignment::Center)),
            "text-align: center"
        );
        assert_eq!(alignment_style(None), "text-align: left");
    }

    #[test]
    fn strips_inline_footnote_names() {
        assert_eq!(footnote_label("__inline_1"), "1");
        assert_eq!(footnote_label("note"), "note");
    }
}
