use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderValue, Method};
use subtle::ConstantTimeEq;
use thiserror::Error;

pub const BASIC_USERNAME: &str = "meerai";
pub const MIN_SECRET_CHARS: usize = 8;
pub const WWW_AUTHENTICATE_VALUE: &str = "Basic realm=\"meerai\", charset=\"UTF-8\"";

const GENERATED_SECRET_BYTES: usize = 24;
const BACKOFF_BASE_MS: u64 = 50;
const BACKOFF_MAX_MS: u64 = 1_000;
const BACKOFF_MAX_SHIFT: u32 = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("serve secret must be at least {MIN_SECRET_CHARS} characters")]
    TooShort,
    #[error("serve secret must not be empty or contain control characters")]
    InvalidCharacters,
    #[error("failed to generate random secret from OS CSPRNG")]
    RandomUnavailable,
}

#[derive(Clone)]
pub struct ServeSecret {
    inner: String,
}

impl ServeSecret {
    pub fn validate(raw: impl Into<String>) -> Result<Self, SecretError> {
        let raw = raw.into();
        if raw.chars().count() < MIN_SECRET_CHARS {
            return Err(SecretError::TooShort);
        }
        if raw.trim().is_empty() || raw.chars().any(char::is_control) {
            return Err(SecretError::InvalidCharacters);
        }
        Ok(Self { inner: raw })
    }

    pub fn generate() -> Result<Self, SecretError> {
        let mut bytes = [0u8; GENERATED_SECRET_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| SecretError::RandomUnavailable)?;
        Ok(Self {
            inner: URL_SAFE_NO_PAD.encode(bytes),
        })
    }

    pub fn expose(&self) -> &str {
        &self.inner
    }
}

impl fmt::Debug for ServeSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServeSecret([REDACTED])")
    }
}

#[derive(Clone, Debug, Default)]
pub enum ServeAuth {
    #[default]
    None,
    Basic {
        username: Option<String>,
        secret: Option<ServeSecret>,
    },
}

impl ServeAuth {
    pub fn from_parts(username: Option<String>, secret: Option<ServeSecret>) -> Self {
        let username = username
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty());
        if username.is_none() && secret.is_none() {
            Self::None
        } else {
            Self::Basic { username, secret }
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Default)]
pub struct AuthBackoff {
    failures: Mutex<u32>,
}

impl AuthBackoff {
    pub fn new() -> Self {
        Self {
            failures: Mutex::new(0),
        }
    }

    pub fn record_failure(&self) -> Duration {
        let mut guard = self.failures.lock().unwrap();
        *guard = guard.saturating_add(1);
        backoff_duration(*guard)
    }

    pub fn reset(&self) {
        let mut guard = self.failures.lock().unwrap();
        *guard = 0;
    }
}

fn backoff_duration(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let exp = failures.saturating_sub(1).min(BACKOFF_MAX_SHIFT);
    let ms = BACKOFF_BASE_MS
        .saturating_mul(1u64 << exp)
        .min(BACKOFF_MAX_MS);
    Duration::from_millis(ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailureReason {
    MissingHeader,
    InvalidScheme,
    InvalidBase64,
    InvalidUtf8,
    MissingSeparator,
    InvalidCredentials,
}

impl AuthFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingHeader => "missing Authorization header",
            Self::InvalidScheme => "invalid Authorization scheme",
            Self::InvalidBase64 => "invalid Basic auth base64",
            Self::InvalidUtf8 => "invalid Basic auth UTF-8",
            Self::MissingSeparator => "invalid Basic auth format",
            Self::InvalidCredentials => "invalid Basic auth credentials",
        }
    }
}

pub fn verify_basic_auth(
    headers: &HeaderMap,
    expected: &ServeAuth,
) -> Result<(), AuthFailureReason> {
    let ServeAuth::Basic {
        username: expected_user,
        secret: expected_secret,
    } = expected
    else {
        return Ok(());
    };

    let mut values = headers.get_all(http::header::AUTHORIZATION).iter();
    let Some(header) = values.next() else {
        return Err(AuthFailureReason::MissingHeader);
    };
    if values.next().is_some() {
        return Err(AuthFailureReason::InvalidScheme);
    }
    let Ok(raw) = header.to_str() else {
        return Err(AuthFailureReason::InvalidScheme);
    };
    let Some((scheme, encoded)) = raw.split_once(' ') else {
        return Err(AuthFailureReason::InvalidScheme);
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return Err(AuthFailureReason::InvalidScheme);
    }
    let Ok(decoded) = STANDARD.decode(encoded.trim()) else {
        return Err(AuthFailureReason::InvalidBase64);
    };
    let Ok(creds) = std::str::from_utf8(&decoded) else {
        return Err(AuthFailureReason::InvalidUtf8);
    };
    let Some((user, secret)) = creds.split_once(':') else {
        return Err(AuthFailureReason::MissingSeparator);
    };

    let user_ok = match expected_user.as_deref() {
        Some(expected) => ct_str_eq(user, expected),
        None => true,
    };
    let secret_ok = match expected_secret.as_ref() {
        Some(expected) => ct_str_eq(secret, expected.expose()),
        None => ct_str_eq(secret, ""),
    };
    if user_ok & secret_ok {
        Ok(())
    } else {
        Err(AuthFailureReason::InvalidCredentials)
    }
}

fn ct_str_eq(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCheckError {
    MissingHost,
    MultipleHost,
    DisallowedHost,
    MissingOrigin,
    DisallowedOrigin,
    MissingContentType,
    UnsupportedContentType,
}

impl RequestCheckError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingHost => "missing Host header",
            Self::MultipleHost => "multiple Host headers",
            Self::DisallowedHost => "disallowed Host header",
            Self::MissingOrigin => "missing Origin header",
            Self::DisallowedOrigin => "disallowed Origin header",
            Self::MissingContentType => "missing Content-Type header",
            Self::UnsupportedContentType => "Content-Type must be application/json",
        }
    }
}

pub fn validate_host(headers: &HeaderMap, port: u16) -> Result<String, RequestCheckError> {
    let mut values = headers.get_all(http::header::HOST).iter();
    let Some(header) = values.next() else {
        return Err(RequestCheckError::MissingHost);
    };
    if values.next().is_some() {
        return Err(RequestCheckError::MultipleHost);
    }
    let Ok(host) = header.to_str() else {
        return Err(RequestCheckError::DisallowedHost);
    };
    let host = host.trim();
    let expected_ip = format!("127.0.0.1:{port}");
    let expected_local = format!("localhost:{port}");
    if host == expected_ip {
        Ok(expected_ip)
    } else if host.eq_ignore_ascii_case(&expected_local) {
        Ok(expected_local)
    } else {
        Err(RequestCheckError::DisallowedHost)
    }
}

pub fn is_state_changing(method: &Method) -> bool {
    matches!(method, &Method::POST | &Method::PUT | &Method::DELETE)
}

pub fn validate_state_changing(
    headers: &HeaderMap,
    validated_host: &str,
) -> Result<(), RequestCheckError> {
    let mut origins = headers.get_all(http::header::ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return Err(RequestCheckError::MissingOrigin);
    };
    if origins.next().is_some() {
        return Err(RequestCheckError::DisallowedOrigin);
    }
    let Ok(origin_str) = origin.to_str() else {
        return Err(RequestCheckError::DisallowedOrigin);
    };
    let expected_origin = format!("http://{validated_host}");
    if origin_str.trim() != expected_origin {
        return Err(RequestCheckError::DisallowedOrigin);
    }

    let mut content_types = headers.get_all(http::header::CONTENT_TYPE).iter();
    let Some(content_type) = content_types.next() else {
        return Err(RequestCheckError::MissingContentType);
    };
    if content_types.next().is_some() {
        return Err(RequestCheckError::UnsupportedContentType);
    }
    if !is_json_content_type(content_type) {
        return Err(RequestCheckError::UnsupportedContentType);
    }
    Ok(())
}

fn is_json_content_type(value: &HeaderValue) -> bool {
    let Ok(raw) = value.to_str() else {
        return false;
    };
    let media_type = raw.split(';').next().unwrap_or("").trim();
    media_type.eq_ignore_ascii_case("application/json")
}

pub fn log_rejection(method: &Method, path: &str, reason: &str) {
    eprintln!("meerai-web rejected {method} {path}: {reason}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_validation_and_redacted_debug() {
        assert_eq!(
            ServeSecret::validate("short").unwrap_err(),
            SecretError::TooShort
        );
        assert_eq!(
            ServeSecret::validate("7-chars").unwrap_err(),
            SecretError::TooShort
        );
        assert_eq!(
            ServeSecret::validate("        ").unwrap_err(),
            SecretError::InvalidCharacters
        );
        assert_eq!(
            ServeSecret::validate("secret-with-line\nbreak").unwrap_err(),
            SecretError::InvalidCharacters
        );

        let raw = "01234567";
        let secret = ServeSecret::validate(raw).unwrap();
        assert_eq!(secret.expose(), raw);
        let debug = format!("{secret:?}");
        assert_eq!(debug, "ServeSecret([REDACTED])");
        assert!(!debug.contains(raw));

        let generated = ServeSecret::generate().unwrap();
        assert!(generated.expose().len() >= 32);
        assert!(!format!("{generated:?}").contains(generated.expose()));
    }

    #[test]
    fn backoff_is_bounded_and_resets() {
        let backoff = AuthBackoff::new();
        assert_eq!(backoff.record_failure(), Duration::from_millis(50));
        assert_eq!(backoff.record_failure(), Duration::from_millis(100));
        assert_eq!(backoff.record_failure(), Duration::from_millis(200));
        assert_eq!(backoff.record_failure(), Duration::from_millis(400));
        assert_eq!(backoff.record_failure(), Duration::from_millis(800));
        assert_eq!(backoff.record_failure(), Duration::from_millis(1000));
        assert_eq!(backoff.record_failure(), Duration::from_millis(1000));
        backoff.reset();
        assert_eq!(backoff.record_failure(), Duration::from_millis(50));
    }

    #[test]
    fn basic_auth_verification_supports_optional_username_and_secret() {
        let secret = ServeSecret::validate("super-secret").unwrap();
        let auth_with_user =
            ServeAuth::from_parts(Some("meerai".to_string()), Some(secret.clone()));
        let mut headers = HeaderMap::new();
        assert_eq!(
            verify_basic_auth(&headers, &auth_with_user),
            Err(AuthFailureReason::MissingHeader)
        );
        assert_eq!(verify_basic_auth(&headers, &ServeAuth::None), Ok(()));

        let good = STANDARD.encode("meerai:super-secret");
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {good}")).unwrap(),
        );
        assert_eq!(verify_basic_auth(&headers, &auth_with_user), Ok(()));

        let wrong_user = STANDARD.encode("admin:super-secret");
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {wrong_user}")).unwrap(),
        );
        assert_eq!(
            verify_basic_auth(&headers, &auth_with_user),
            Err(AuthFailureReason::InvalidCredentials)
        );

        let secret_only = ServeAuth::from_parts(None, Some(secret));
        assert_eq!(verify_basic_auth(&headers, &secret_only), Ok(()));
        let empty_user = STANDARD.encode(":super-secret");
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {empty_user}")).unwrap(),
        );
        assert_eq!(verify_basic_auth(&headers, &secret_only), Ok(()));

        let wrong_secret = STANDARD.encode("meerai:wrong-secret");
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {wrong_secret}")).unwrap(),
        );
        assert_eq!(
            verify_basic_auth(&headers, &auth_with_user),
            Err(AuthFailureReason::InvalidCredentials)
        );
        assert_eq!(
            verify_basic_auth(&headers, &secret_only),
            Err(AuthFailureReason::InvalidCredentials)
        );
    }

    #[test]
    fn host_and_origin_validation() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::HOST,
            HeaderValue::from_static("127.0.0.1:4321"),
        );
        let host = validate_host(&headers, 4321).unwrap();
        assert_eq!(host, "127.0.0.1:4321");

        headers.insert(
            http::header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:4321"),
        );
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert_eq!(validate_state_changing(&headers, &host), Ok(()));

        headers.insert(
            http::header::ORIGIN,
            HeaderValue::from_static("http://localhost:4321"),
        );
        assert_eq!(
            validate_state_changing(&headers, &host),
            Err(RequestCheckError::DisallowedOrigin)
        );

        headers.insert(
            http::header::HOST,
            HeaderValue::from_static("evil.example:4321"),
        );
        assert_eq!(
            validate_host(&headers, 4321),
            Err(RequestCheckError::DisallowedHost)
        );
    }
}
