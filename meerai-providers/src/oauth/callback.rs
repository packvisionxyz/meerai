use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::oauth::{CALLBACK_PATH, PREFERRED_CALLBACK_PORT};

const CALLBACK_HOST: &str = "127.0.0.1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CALLBACK_BODY: &str =
    "<html><body>meerai sign-in is complete. You can close this window.</body></html>";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallbackQuery {
    pub code: String,
    pub state: Option<String>,
    pub client_id: Option<String>,
}

pub fn callback_redirect_uri(port: u16) -> String {
    format!("http://{CALLBACK_HOST}:{port}{CALLBACK_PATH}")
}

pub async fn bind_listener() -> Result<(TcpListener, u16)> {
    let listener = match TcpListener::bind((CALLBACK_HOST, PREFERRED_CALLBACK_PORT)).await {
        Ok(listener) => listener,
        Err(_) => TcpListener::bind((CALLBACK_HOST, 0))
            .await
            .context("failed to bind a loopback sign-in listener")?,
    };
    let port = listener
        .local_addr()
        .context("failed to read the sign-in listener address")?
        .port();
    Ok((listener, port))
}

pub async fn wait_for_redirect(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<CallbackQuery> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .context("failed to accept the sign-in callback")?;
        let mut buffer = [0u8; 8192];
        let read = match tokio::time::timeout(REQUEST_TIMEOUT, socket.read(&mut buffer)).await {
            Ok(Ok(read)) => read,
            Ok(Err(_)) => continue,
            Err(_) => continue,
        };
        let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or_default()
            .to_string();
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, query),
            None => (target.as_str(), ""),
        };
        if path != CALLBACK_PATH {
            let _ = socket.write_all(not_found().as_bytes()).await;
            let _ = socket.shutdown().await;
            continue;
        }
        let parsed = match callback_from_query(query) {
            Ok(parsed) => parsed,
            Err(err) => {
                if query.is_empty() {
                    let _ = socket.write_all(not_found().as_bytes()).await;
                    let _ = socket.shutdown().await;
                    continue;
                }
                let _ = socket
                    .write_all(error_page(&err.to_string()).as_bytes())
                    .await;
                let _ = socket.shutdown().await;
                return Err(err);
            }
        };
        if let Err(err) = verify_state(expected_state, parsed.state.as_deref()) {
            let _ = socket
                .write_all(error_page(&err.to_string()).as_bytes())
                .await;
            let _ = socket.shutdown().await;
            return Err(err);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{CALLBACK_BODY}",
            CALLBACK_BODY.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
        return Ok(parsed);
    }
}

fn error_page(message: &str) -> String {
    let body = format!(
        "<html><body>meerai sign-in did not complete: {}. Return to meerai and try again.</body></html>",
        escape_html(message)
    );
    format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn not_found() -> String {
    let body = "<html><body>meerai is waiting for a sign-in callback.</body></html>";
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

pub fn verify_state(expected: &str, actual: Option<&str>) -> Result<()> {
    match actual {
        Some(actual) if actual != expected => {
            anyhow::bail!("sign-in callback state does not match this request")
        }
        _ => Ok(()),
    }
}

pub fn parse_callback_input(raw: &str) -> Result<CallbackQuery> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("no sign-in callback was provided");
    }
    if let Some((_, query)) = trimmed.split_once('?') {
        return callback_from_query(query);
    }
    if let Some(query) = trimmed.strip_prefix('?') {
        return callback_from_query(query);
    }
    if trimmed.contains('=') {
        return callback_from_query(trimmed);
    }
    if let Some((code, state)) = trimmed.split_once('#') {
        if code.trim().is_empty() {
            anyhow::bail!("no sign-in code was provided");
        }
        return Ok(CallbackQuery {
            code: code.trim().to_string(),
            state: Some(state.trim().to_string()),
            client_id: None,
        });
    }
    Ok(CallbackQuery {
        code: trimmed.to_string(),
        state: None,
        client_id: None,
    })
}

fn callback_from_query(query: &str) -> Result<CallbackQuery> {
    let params = parse_query(query);
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("no description");
        anyhow::bail!("sign-in was rejected ({error}): {description}");
    }
    let code = params
        .get("code")
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
        .context("sign-in callback has no code")?;
    Ok(CallbackQuery {
        code,
        state: params.get("state").cloned(),
        client_id: params
            .get("client_id")
            .map(|client_id| client_id.trim().to_string())
            .filter(|client_id| !client_id.is_empty()),
    })
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        params
            .entry(percent_decode(key))
            .or_insert_with(|| percent_decode(value));
    }
    params
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1]);
                let low = hex_value(bytes[index + 2]);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push(high * 16 + low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uri_uses_the_bound_port() {
        assert_eq!(
            callback_redirect_uri(1455),
            "http://127.0.0.1:1455/auth/callback"
        );
        assert_eq!(
            callback_redirect_uri(51234),
            "http://127.0.0.1:51234/auth/callback"
        );
    }

    #[test]
    fn parses_a_full_redirect_url() {
        let parsed = parse_callback_input(
            "http://127.0.0.1:1455/auth/callback?code=abc123&state=xyz&client_id=oaiapp_9",
        )
        .unwrap();
        assert_eq!(parsed.code, "abc123");
        assert_eq!(parsed.state.as_deref(), Some("xyz"));
        assert_eq!(parsed.client_id.as_deref(), Some("oaiapp_9"));
    }

    #[test]
    fn parses_a_query_string_hash_pair_and_a_bare_code() {
        let query = parse_callback_input("code=abc123&state=xyz").unwrap();
        assert_eq!(query.code, "abc123");
        assert_eq!(query.state.as_deref(), Some("xyz"));

        let prefixed = parse_callback_input("?code=abc123&state=xyz").unwrap();
        assert_eq!(prefixed.code, "abc123");

        let hash = parse_callback_input("abc123#xyz").unwrap();
        assert_eq!(hash.code, "abc123");
        assert_eq!(hash.state.as_deref(), Some("xyz"));

        let bare = parse_callback_input("  abc123  ").unwrap();
        assert_eq!(bare.code, "abc123");
        assert_eq!(bare.state, None);
    }

    #[test]
    fn decodes_percent_escapes_and_plus() {
        let parsed = parse_callback_input("code=a%2Bb&state=x+y").unwrap();
        assert_eq!(parsed.code, "a+b");
        assert_eq!(parsed.state.as_deref(), Some("x y"));
    }

    #[test]
    fn rejects_a_state_mismatch() {
        assert!(verify_state("expected", Some("expected")).is_ok());
        assert!(verify_state("expected", None).is_ok());
        let err = verify_state("expected", Some("other")).unwrap_err();
        assert!(err.to_string().contains("state does not match"));
    }

    #[test]
    fn rejects_a_callback_without_a_code() {
        assert!(parse_callback_input("http://127.0.0.1:1455/auth/callback?state=x").is_err());
        assert!(parse_callback_input("   ").is_err());
        let err = parse_callback_input(
            "http://127.0.0.1:1455/auth/callback?error=access_denied&error_description=no",
        )
        .unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[tokio::test]
    async fn returns_the_code_only_after_the_state_matches() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream
                .write_all(
                    b"GET /auth/callback?code=abc&state=expected&client_id=oaiapp_1 HTTP/1.1\r\nHost: local\r\n\r\n",
                )
                .await
                .unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            String::from_utf8_lossy(&response).to_string()
        });

        let query = wait_for_redirect(&listener, "expected").await.unwrap();
        assert_eq!(query.code, "abc");
        assert_eq!(query.client_id.as_deref(), Some("oaiapp_1"));
        assert!(client.await.unwrap().starts_with("HTTP/1.1 200"));
    }

    #[tokio::test]
    async fn rejects_a_state_mismatch_with_a_400_page() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream
                .write_all(
                    b"GET /auth/callback?code=abc&state=wrong HTTP/1.1\r\nHost: local\r\n\r\n",
                )
                .await
                .unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            String::from_utf8_lossy(&response).to_string()
        });

        let err = wait_for_redirect(&listener, "expected").await.unwrap_err();
        assert!(err.to_string().contains("state"));
        let response = client.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("did not complete"));
    }
}
