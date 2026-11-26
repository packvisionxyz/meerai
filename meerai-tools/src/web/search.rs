use std::sync::LazyLock;

use meerai_core::{ToolDefinition, ToolRun};
use regex::Regex;
use reqwest::Url;
use reqwest::header::ACCEPT;

use super::WebTools;
use super::fetch::decode_redirect_target;
use super::text::plain_fragment;
use crate::code::{failure_run, success_run};

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 10;
const MAX_SNIPPET_CHARS: usize = 300;
const SEARCH_HINT: &str = "web search uses DuckDuckGo without an account and is best-effort; retry later or fetch a known URL with web-fetch";

// These patterns are source literals, not runtime input: a failure here is a
// programming error caught by the crate's tests, never a caller-triggered panic.
#[allow(clippy::expect_used)]
static ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<a\b([^>]*)>(.*?)</a>").expect("anchor pattern"));
static ELEMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:a|div|td|span)\b([^>]*)>(.*?)</(?:a|div|td|span)>")
        .expect("element pattern")
});
static CLASS_ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bclass\s*=\s*(?:"([^"]*)"|'([^']*)')"#).expect("class pattern")
});
static HREF_ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).expect("href pattern")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct SearchArgs {
    #[schemars(description = "Search query.")]
    #[serde(alias = "q", alias = "text")]
    pub(crate) query: String,

    #[schemars(description = "Number of results to return, 1 to 10. Defaults to 5.")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_usize_value")]
    pub(crate) limit: Option<usize>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<SearchArgs>(
        name,
        "Search the web with DuckDuckGo and return numbered results with title, URL, and snippet. Use web-fetch to read a result. Best-effort and needs no account.",
    )
}

impl WebTools {
    pub(crate) async fn search(&self, args: SearchArgs) -> ToolRun {
        let query = args.query.trim().to_string();
        if query.is_empty() {
            return failure_run("query is required".to_string());
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let mut last_error = String::new();
        for endpoint in self.search_endpoints() {
            let mut url = match Url::parse(endpoint) {
                Ok(url) => url,
                Err(err) => {
                    last_error = format!("invalid search endpoint '{endpoint}': {err}");
                    continue;
                }
            };
            let is_duckduckgo = url
                .host_str()
                .is_some_and(|host| host.ends_with("duckduckgo.com"));
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("q", &query);
                if is_duckduckgo {
                    pairs.append_pair("kl", "us-en");
                }
            }
            if let Err(reason) = self.allowed_target(&url).await {
                last_error = reason;
                continue;
            }
            let Some(client) = self.client() else {
                return failure_run(
                    "web search is unavailable: the HTTP client failed to start".to_string(),
                );
            };
            let mut response = match client
                .get(url.clone())
                .header(ACCEPT, "text/html,application/xhtml+xml")
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = format!("cannot reach the search provider: {err}");
                    continue;
                }
            };
            if !response.status().is_success() {
                last_error = format!("search provider returned HTTP {}", response.status());
                continue;
            }
            let (body, _) = match super::fetch::read_body(&mut response).await {
                Ok(body) => body,
                Err(reason) => {
                    last_error = reason;
                    continue;
                }
            };
            let html = String::from_utf8_lossy(&body);
            let results = parse_results(&html);
            if results.is_empty() {
                last_error = if is_blocked_page(&html) {
                    "the search provider blocked the request".to_string()
                } else {
                    "the search provider returned no results".to_string()
                };
                continue;
            }
            return success_run(render_results(&query, &results, limit));
        }
        let reason = if last_error.is_empty() {
            "web search failed".to_string()
        } else {
            last_error
        };
        failure_run(format!("{reason}\n{SEARCH_HINT}"))
    }
}

pub(crate) fn parse_results(html: &str) -> Vec<SearchHit> {
    let mut hits: Vec<SearchHit> = Vec::new();
    for capture in ANCHOR.captures_iter(html) {
        let attrs = capture.get(1).map(|m| m.as_str()).unwrap_or_default();
        let classes = class_tokens(attrs);
        let is_result = classes
            .iter()
            .any(|class| class == "result__a" || class == "result-link");
        if !is_result || classes.iter().any(|class| class.contains("snippet")) {
            continue;
        }
        let Some(href) = href_of(attrs) else {
            continue;
        };
        let Some(url) = decode_redirect_target(&href) else {
            continue;
        };
        if hits.iter().any(|hit| hit.url == url) {
            continue;
        }
        let title = capture
            .get(2)
            .map(|m| plain_fragment(m.as_str()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        hits.push(SearchHit {
            title,
            url,
            snippet: String::new(),
        });
    }
    if hits.is_empty() {
        return hits;
    }

    let mut snippets = Vec::new();
    for capture in ELEMENT.captures_iter(html) {
        let attrs = capture.get(1).map(|m| m.as_str()).unwrap_or_default();
        let classes = class_tokens(attrs);
        let is_snippet = classes
            .iter()
            .any(|class| class == "result__snippet" || class == "result-snippet");
        if is_snippet && let Some(inner) = capture.get(2) {
            snippets.push(plain_fragment(inner.as_str()));
        }
    }
    for (hit, snippet) in hits.iter_mut().zip(snippets) {
        hit.snippet = snippet;
    }
    hits
}

pub(crate) fn is_blocked_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "anomaly-modal",
        "bots use duckduckgo",
        "unusual traffic",
        "captcha",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn class_tokens(attrs: &str) -> Vec<String> {
    CLASS_ATTR
        .captures(attrs)
        .and_then(|capture| {
            capture
                .get(1)
                .or_else(|| capture.get(2))
                .map(|m| m.as_str().to_string())
        })
        .map(|classes| {
            classes
                .split_whitespace()
                .map(|class| class.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn href_of(attrs: &str) -> Option<String> {
    let capture = HREF_ATTR.captures(attrs)?;
    capture
        .get(1)
        .or_else(|| capture.get(2))
        .or_else(|| capture.get(3))
        .map(|m| m.as_str().to_string())
}

fn render_results(query: &str, results: &[SearchHit], limit: usize) -> String {
    let mut output = format!("Search results for '{query}':\n\n");
    for (index, hit) in results.iter().take(limit).enumerate() {
        let snippet = truncate_chars(&hit.snippet, MAX_SNIPPET_CHARS);
        output.push_str(&format!("{}. {}\n   {}\n", index + 1, hit.title, hit.url));
        if !snippet.is_empty() {
            output.push_str(&format!("   {snippet}\n"));
        }
    }
    output.push_str(&format!(
        "\n({} result(s); bodies are not fetched. Use web-fetch on a URL for the full text.)",
        results.len().min(limit)
    ));
    output
}

fn truncate_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML_PAGE: &str = r#"<html><body>
        <div class="result results_links">
          <h2 class="result__title">
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone&amp;rut=abc">Example <b>One</b></a>
          </h2>
          <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone">First &amp; best result.</a>
        </div>
        <div class="result results_links">
          <a class="result__a" href="https://two.example/path">Second</a>
          <div class="result__snippet">A second <b>snippet</b>.</div>
        </div>
        <div class="result results_links">
          <a class="result__a" href="https://two.example/path">Duplicate</a>
        </div>
        <a class="result__a" href="https://duckduckgo.com/y.js?ad=1">Ad</a>
        </body></html>"#;

    const LITE_PAGE: &str = r#"<html><body><table>
        <tr><td><a rel="nofollow" href="https://lite.example/" class='result-link'>Lite result</a></td></tr>
        <tr><td class='result-snippet'>Lite snippet.</td></tr>
        </table></body></html>"#;

    #[test]
    fn html_results_are_unwrapped_deduped_and_paired_with_snippets() {
        let hits = parse_results(HTML_PAGE);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Example One");
        assert_eq!(hits[0].url, "https://example.com/one");
        assert_eq!(hits[0].snippet, "First & best result.");
        assert_eq!(hits[1].title, "Second");
        assert_eq!(hits[1].url, "https://two.example/path");
        assert_eq!(hits[1].snippet, "A second snippet.");
    }

    #[test]
    fn lite_results_use_single_quoted_classes() {
        let hits = parse_results(LITE_PAGE);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Lite result");
        assert_eq!(hits[0].url, "https://lite.example/");
        assert_eq!(hits[0].snippet, "Lite snippet.");
    }

    #[test]
    fn blocked_pages_are_recognized() {
        assert!(is_blocked_page(
            "<div class=\"anomaly-modal__title\">Unfortunately, bots use DuckDuckGo too.</div>"
        ));
        assert!(!is_blocked_page(HTML_PAGE));
    }

    #[test]
    fn rendered_results_are_numbered_and_limited() {
        let hits = parse_results(HTML_PAGE);
        let rendered = render_results("query", &hits, 1);
        assert!(rendered.contains("1. Example One"));
        assert!(!rendered.contains("2. Second"));
        assert!(rendered.contains("https://example.com/one"));
    }
}
