use std::net::IpAddr;

use meerai_core::text::floor_char_boundary;
use meerai_core::{ToolDefinition, ToolRun};
use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION};
use url::Host;

use super::text::{html_to_text, percent_decode};
use super::{MAX_BODY_BYTES, MAX_OUTPUT_CHARS, WebTools};
use crate::code::{failure_run, success_run};

const MAX_REDIRECTS: usize = 5;
const MIN_OUTPUT_CHARS: usize = 200;

const ACCEPT_VALUE: &str =
    "text/html,application/xhtml+xml,application/json;q=0.9,text/plain;q=0.8,*/*;q=0.5";

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct FetchArgs {
    #[schemars(description = "Absolute http or https URL to fetch.")]
    #[serde(alias = "uri", alias = "link")]
    pub(crate) url: String,

    #[schemars(
        description = "Maximum characters to return. Defaults to 60000 and is capped at that value."
    )]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_usize_value")]
    pub(crate) max_chars: Option<usize>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<FetchArgs>(
        name,
        "Fetch an http or https URL and return its readable text. HTML is converted to plain text; JSON, XML, and plain text are returned as-is. Private and local addresses are refused.",
    )
}

impl WebTools {
    pub(crate) async fn fetch(&self, args: FetchArgs) -> ToolRun {
        let max_chars = args
            .max_chars
            .unwrap_or(MAX_OUTPUT_CHARS)
            .clamp(MIN_OUTPUT_CHARS, MAX_OUTPUT_CHARS);
        let mut url = match parse_target(&args.url) {
            Ok(url) => url,
            Err(reason) => return failure_run(reason),
        };

        for _ in 0..=MAX_REDIRECTS {
            if let Err(reason) = self.allowed_target(&url).await {
                return failure_run(reason);
            }
            let Some(client) = self.client() else {
                return failure_run(
                    "web fetch is unavailable: the HTTP client failed to start".to_string(),
                );
            };
            let response = match client
                .get(url.clone())
                .header(ACCEPT, ACCEPT_VALUE)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => return failure_run(format!("cannot fetch {url}: {err}")),
            };
            let status = response.status();
            if status.is_redirection() {
                let Some(location) = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                else {
                    return failure_run(format!("{url} redirected without a Location header"));
                };
                let next = match url.join(location) {
                    Ok(next) => next,
                    Err(err) => return failure_run(format!("invalid redirect from {url}: {err}")),
                };
                if url.scheme() == "https" && next.scheme() != "https" {
                    return failure_run(format!(
                        "refusing to follow a redirect from {url} to insecure {next}"
                    ));
                }
                url = next;
                continue;
            }
            if !status.is_success() {
                return failure_run(format!("{url} returned HTTP {status}"));
            }
            return self.read_page(url, response, max_chars).await;
        }
        failure_run(format!("too many redirects (limit {MAX_REDIRECTS})"))
    }

    async fn read_page(
        &self,
        url: Url,
        mut response: reqwest::Response,
        max_chars: usize,
    ) -> ToolRun {
        let media = media_type(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
        );
        let (body, body_truncated) = match read_body(&mut response).await {
            Ok(body) => body,
            Err(reason) => return failure_run(reason),
        };
        let raw = String::from_utf8_lossy(&body);
        let text = match render_body(&media, &raw) {
            Ok(text) => text,
            Err(reason) => return failure_run(reason),
        };
        let (sliced, text_truncated) = cap_chars(&text, max_chars);
        let mut output = format!(
            "URL: {url}\nContent-Type: {}\n\n",
            if media.is_empty() { "unknown" } else { &media }
        );
        output.push_str(sliced);
        if body_truncated || text_truncated {
            output.push_str(&format!(
                "\n\n(truncated; raise max_chars for more text, up to {MAX_OUTPUT_CHARS} characters)"
            ));
        }
        success_run(output)
    }

    pub(super) async fn allowed_target(&self, url: &Url) -> Result<(), String> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!(
                "refusing to fetch {url}: only http and https are allowed"
            ));
        }
        if self.allow_private_networks() {
            return Ok(());
        }
        match url.host() {
            Some(Host::Ipv4(ip)) => ensure_public(IpAddr::V4(ip), url),
            Some(Host::Ipv6(ip)) => ensure_public(IpAddr::V6(ip), url),
            Some(Host::Domain(domain)) => {
                let domain = domain.to_ascii_lowercase();
                if domain == "localhost"
                    || domain.ends_with(".localhost")
                    || domain.ends_with(".local")
                {
                    return Err(format!("refusing to fetch {url}: local hosts are blocked"));
                }
                let port = url.port_or_known_default().unwrap_or(443);
                let addrs = match tokio::net::lookup_host((domain.as_str(), port)).await {
                    Ok(addrs) => addrs.collect::<Vec<_>>(),
                    Err(err) => return Err(format!("cannot resolve {domain}: {err}")),
                };
                if addrs.is_empty() {
                    return Err(format!("refusing to fetch {url}: {domain} did not resolve"));
                }
                for addr in addrs {
                    ensure_public(addr.ip(), url)?;
                }
                Ok(())
            }
            None => Err(format!("refusing to fetch {url}: the URL has no host")),
        }
    }
}

fn parse_target(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("url is required".to_string());
    }
    let url = Url::parse(trimmed).map_err(|err| format!("invalid URL '{trimmed}': {err}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "unsupported URL scheme '{other}': only http and https are allowed"
            ));
        }
    }
    if url.host_str().is_none() {
        return Err(format!("invalid URL '{trimmed}': the URL has no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "URL userinfo is not allowed; pass credentials through the provider settings"
                .to_string(),
        );
    }
    Ok(url)
}

fn ensure_public(ip: IpAddr, url: &Url) -> Result<(), String> {
    if is_blocked_ip(ip) {
        Err(format!(
            "refusing to fetch {url}: {ip} is a private or local address"
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(mapped));
            }
            let segments = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

fn media_type(header: Option<&str>) -> String {
    header
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_textual(media: &str) -> bool {
    media.starts_with("text/")
        || media.ends_with("+json")
        || media.ends_with("+xml")
        || matches!(
            media,
            "application/json"
                | "application/xml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/ecmascript"
                | "application/sql"
                | "application/graphql"
                | "application/ld+json"
        )
}

fn render_body(media: &str, raw: &str) -> Result<String, String> {
    if media.is_empty() {
        return Ok(if looks_like_html(raw) {
            html_to_text(raw)
        } else {
            raw.to_string()
        });
    }
    if media == "text/html" || media == "application/xhtml+xml" {
        return Ok(html_to_text(raw));
    }
    if is_textual(media) {
        return Ok(raw.to_string());
    }
    Err(format!(
        "unsupported content type '{media}': web fetch returns text only; use shell curl for binary content"
    ))
}

fn looks_like_html(raw: &str) -> bool {
    let head = raw
        .trim_start()
        .chars()
        .take(1024)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.contains("<html")
}

pub(super) async fn read_body(response: &mut reqwest::Response) -> Result<(Vec<u8>, bool), String> {
    if let Some(length) = response.content_length()
        && length > MAX_BODY_BYTES as u64
    {
        return Err(format!(
            "response is too large ({length} bytes; limit {MAX_BODY_BYTES})"
        ));
    }
    let mut body = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|err| format!("cannot read the response: {err}"))?;
        let Some(chunk) = chunk else {
            return Ok((body, false));
        };
        let remaining = MAX_BODY_BYTES.saturating_sub(body.len());
        if chunk.len() >= remaining {
            body.extend_from_slice(&chunk[..remaining]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
}

fn cap_chars(text: &str, limit: usize) -> (&str, bool) {
    match text.char_indices().nth(limit) {
        Some((index, _)) => (&text[..floor_char_boundary(text, index)], true),
        None => (text, false),
    }
}

pub(crate) fn decode_redirect_target(href: &str) -> Option<String> {
    let decoded = super::text::decode_entities(href);
    let absolute = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else if decoded.starts_with('/') {
        format!("https://duckduckgo.com{decoded}")
    } else {
        decoded
    };
    let parsed = Url::parse(&absolute).ok()?;
    let redirect = parsed
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("duckduckgo.com"))
        && parsed.path().starts_with("/l/");
    let target = if redirect {
        parsed
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| percent_decode(&value).unwrap_or_else(|| value.into_owned()))?
    } else {
        absolute
    };
    let url = Url::parse(&target).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    if url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("duckduckgo.com"))
    {
        return None;
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_addresses_cover_private_loopback_and_mapped_ranges() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.4.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "{ip} should be blocked");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(
                !is_blocked_ip(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[test]
    fn parse_target_rejects_other_schemes_and_userinfo() {
        assert!(parse_target("file:///etc/passwd").is_err());
        assert!(parse_target("ftp://example.com").is_err());
        assert!(parse_target("https://user:pass@example.com").is_err());
        assert!(parse_target("https://example.com/docs").is_ok());
    }

    #[test]
    fn body_rendering_accepts_text_and_refuses_binary() {
        assert_eq!(render_body("text/plain", "hello").unwrap(), "hello");
        assert_eq!(
            render_body("text/html", "<p>hi</p>").unwrap(),
            "hi".to_string()
        );
        assert!(render_body("image/png", "...").is_err());
        assert_eq!(
            render_body("", "<html><body>x</body></html>").unwrap(),
            "x".to_string()
        );
    }

    #[test]
    fn cap_chars_slices_on_a_character_boundary() {
        assert_eq!(cap_chars("abc", 10), ("abc", false));
        let (sliced, cut) = cap_chars("aébc", 2);
        assert_eq!(sliced, "aé");
        assert!(cut);
    }

    #[test]
    fn redirect_targets_unwrap_duckduckgo_and_drop_ads() {
        assert_eq!(
            decode_redirect_target(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdoc&amp;rut=abc"
            ),
            Some("https://example.com/doc".to_string())
        );
        assert_eq!(
            decode_redirect_target("https://example.com/direct"),
            Some("https://example.com/direct".to_string())
        );
        assert_eq!(
            decode_redirect_target("https://duckduckgo.com/y.js?ad=1"),
            None
        );
    }

    #[tokio::test]
    async fn fetch_follows_redirects_and_converts_html_on_a_local_server() {
        let page = "<html><body><h1>Hello &amp; hi</h1></body></html>";
        let server = crate::web::test_server::spawn(vec![
            "HTTP/1.1 302 Found\r\nLocation: /page\r\nContent-Length: 0\r\n\r\n".to_string(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                page.len(),
                page
            ),
        ])
        .await;
        let tools = WebTools::new().with_allow_private_networks(true);
        let run = tools
            .fetch(FetchArgs {
                url: server.url("/start"),
                max_chars: None,
            })
            .await;
        assert_eq!(
            run.observation.status,
            meerai_core::ObservationStatus::Success
        );
        assert!(run.observation.stdout.contains("Hello & hi"));
        assert!(!run.observation.stdout.contains("<h1>"));
        server.stop();
    }

    #[tokio::test]
    async fn fetch_refuses_private_targets_unless_explicitly_allowed() {
        let tools = WebTools::new();
        let run = tools
            .fetch(FetchArgs {
                url: "http://127.0.0.1:9/".to_string(),
                max_chars: None,
            })
            .await;
        assert_eq!(
            run.observation.status,
            meerai_core::ObservationStatus::Error
        );
        assert!(run.observation.stderr.contains("private or local"));
    }
}
