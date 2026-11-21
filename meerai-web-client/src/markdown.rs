use comrak::nodes::{AstNode, ListType, NodeValue};
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
    options.parse.smart = false;

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
                _ => view! { <h4 class="md-h">{inner}</h4> }.into_any(),
            };
            Some(view)
        }
        NodeValue::CodeBlock(block) => Some(render_code_block(
            block.info.trim(),
            block.literal.trim_end_matches('\n'),
        )),
        NodeValue::BlockQuote => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            Some(
                view! {
                    <blockquote class="md-quote">
                        {AnyView::from(Fragment::new(blocks))}
                    </blockquote>
                }
                .into_any(),
            )
        }
        NodeValue::List(list) => {
            let items = node
                .children()
                .map(|item| {
                    let blocks = item.children().filter_map(render_block).collect::<Vec<_>>();
                    view! { <li>{AnyView::from(Fragment::new(blocks))}</li> }.into_any()
                })
                .collect::<Vec<_>>();
            let items = AnyView::from(Fragment::new(items));
            Some(match list.list_type {
                ListType::Ordered => view! { <ol class="md-list">{items}</ol> }.into_any(),
                _ => view! { <ul class="md-list">{items}</ul> }.into_any(),
            })
        }
        NodeValue::Table(_) => {
            let mut rows = Vec::new();
            let mut header = true;
            for row in node.children() {
                let cells = row
                    .children()
                    .map(|cell| {
                        let inner = render_inlines(cell);
                        if header {
                            view! { <th>{inner}</th> }.into_any()
                        } else {
                            view! { <td>{inner}</td> }.into_any()
                        }
                    })
                    .collect::<Vec<_>>();
                rows.push(view! { <tr>{AnyView::from(Fragment::new(cells))}</tr> }.into_any());
                header = false;
            }
            Some(
                view! {
                    <div class="md-table-wrap">
                        <table class="md-table">{AnyView::from(Fragment::new(rows))}</table>
                    </div>
                }
                .into_any(),
            )
        }
        NodeValue::ThematicBreak(_) => Some(view! { <hr class="md-rule" /> }.into_any()),
        NodeValue::HtmlBlock(html) => {
            let text = html.literal.trim().to_owned();
            (!text.is_empty()).then(|| view! { <pre class="md-html">{text}</pre> }.into_any())
        }
        _ => {
            let blocks = node.children().filter_map(render_block).collect::<Vec<_>>();
            (!blocks.is_empty()).then(|| AnyView::from(Fragment::new(blocks)))
        }
    }
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
        NodeValue::SoftBreak => out.push(" ".to_owned().into_any()),
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
        NodeValue::Link(link) => {
            let inner = render_inlines(node);
            match safe_url(&link.url) {
                Some(url) => out.push(
                    view! {
                        <a href=url target="_blank" rel="noopener noreferrer" referrerpolicy="no-referrer">
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
                    "Copy"
                </button>
            </div>
            <pre><code>{body}</code></pre>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::path_command;

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
}
