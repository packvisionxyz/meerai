use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use meerai_client::secrets::{Credential, SecretStore};
use serde::Deserialize;
use serde_json::Value;

use crate::oauth::{RESOURCE, REVOKE_ENDPOINT, TOKEN_ENDPOINT, error_code, error_description};

const JWKS_ENDPOINT: &str = "https://auth.openai.com/.well-known/jwks.json";
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Deserialize)]
struct JsonWebKey {
    kid: Option<String>,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct JsonWebKeySet {
    keys: Vec<JsonWebKey>,
}

type JwksCache = tokio::sync::Mutex<Option<(Instant, Vec<JsonWebKey>)>>;

static JWKS_CACHE: OnceLock<JwksCache> = OnceLock::new();

const TERMINAL_REFRESH_CODES: [&str; 6] = [
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

#[derive(Clone, Debug, Default)]
pub(crate) struct TokenPayload {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub id_token: Option<String>,
    pub scope: Option<String>,
    pub client_id: Option<String>,
}

#[derive(Debug)]
pub(crate) enum TokenFailure {
    Terminal {
        code: String,
        description: Option<String>,
    },
    Transient(anyhow::Error),
}

impl TokenFailure {
    pub(crate) fn into_error(self) -> anyhow::Error {
        match self {
            Self::Terminal { code, description } => match description {
                Some(description) => anyhow!("{code}: {description}"),
                None => anyhow!("{code}"),
            },
            Self::Transient(err) => err,
        }
    }
}

pub(crate) fn is_terminal_refresh_code(code: &str) -> bool {
    TERMINAL_REFRESH_CODES.contains(&code)
}

pub(crate) async fn exchange_code(
    client_id: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TokenPayload> {
    request_token(&[
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
        ("resource", RESOURCE),
    ])
    .await
    .map_err(TokenFailure::into_error)
}

pub(crate) async fn refresh_tokens(
    client_id: &str,
    refresh_token: &str,
) -> std::result::Result<TokenPayload, TokenFailure> {
    request_token(&[
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("resource", RESOURCE),
    ])
    .await
}

pub async fn revoke_chatgpt_credential(store: &dyn SecretStore, provider_id: &str) -> Result<()> {
    if let Some(Credential::OAuth(oauth)) = store.read_credential(provider_id)? {
        let client_id = oauth.client_id.trim();
        if !client_id.is_empty() {
            let _ = reqwest::Client::new()
                .post(REVOKE_ENDPOINT)
                .form(&[("token", oauth.refresh.as_str()), ("client_id", client_id)])
                .send()
                .await;
        }
    }
    store.delete_credential(provider_id)
}

pub(crate) async fn validate_id_token(
    id_token: &str,
    client_id: &str,
    nonce: &str,
    current_ms: u64,
) -> Result<String> {
    let kid = require_rs256_kid(id_token)?;
    let claims = verify_signature(id_token, kid.as_deref(), client_id).await?;
    validate_claims(&claims, client_id, nonce, current_ms)
}

fn require_rs256_kid(id_token: &str) -> Result<Option<String>> {
    let header = decode_header(id_token).context("OpenAI ID token header is invalid")?;
    if header.alg != Algorithm::RS256 {
        anyhow::bail!("OpenAI ID token is not signed with RS256");
    }
    Ok(header.kid)
}

async fn verify_signature(id_token: &str, kid: Option<&str>, client_id: &str) -> Result<Value> {
    let mut keys = load_jwks(false).await?;
    if let Some(kid) = kid
        && select_key(&keys, kid)?.is_none()
    {
        keys = load_jwks(true).await?;
    }
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp"]);
    match kid {
        Some(kid) => {
            let key = select_key(&keys, kid)?
                .context("OpenAI ID token key id is not in the published key set")?;
            Ok(decode::<Value>(id_token, &key, &validation)
                .context("OpenAI ID token signature or claims are invalid")?
                .claims)
        }
        None => {
            for key in &keys {
                let key = DecodingKey::from_rsa_components(&key.n, &key.e)?;
                if let Ok(token) = decode::<Value>(id_token, &key, &validation) {
                    return Ok(token.claims);
                }
            }
            anyhow::bail!("OpenAI ID token signature did not match any published key")
        }
    }
}

fn select_key(keys: &[JsonWebKey], kid: &str) -> Result<Option<DecodingKey>> {
    let Some(key) = keys.iter().find(|key| key.kid.as_deref() == Some(kid)) else {
        return Ok(None);
    };
    Ok(Some(DecodingKey::from_rsa_components(&key.n, &key.e)?))
}

async fn load_jwks(force: bool) -> Result<Vec<JsonWebKey>> {
    let cache = JWKS_CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    let mut guard = cache.lock().await;
    if !force
        && let Some((fetched_at, keys)) = guard.as_ref()
        && fetched_at.elapsed() < JWKS_TTL
    {
        return Ok(keys.clone());
    }
    let response = reqwest::Client::new()
        .get(JWKS_ENDPOINT)
        .send()
        .await
        .context("failed to reach the OpenAI key set")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read the OpenAI key set")?;
    if !status.is_success() {
        anyhow::bail!("OpenAI key set request failed with status {status}");
    }
    let parsed: JsonWebKeySet =
        serde_json::from_str(&body).context("failed to parse the OpenAI key set")?;
    if parsed.keys.is_empty() {
        anyhow::bail!("OpenAI key set returned no keys");
    }
    *guard = Some((Instant::now(), parsed.keys.clone()));
    Ok(parsed.keys)
}

fn validate_claims(
    claims: &Value,
    client_id: &str,
    nonce: &str,
    current_ms: u64,
) -> Result<String> {
    let issuer = claims
        .get("iss")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if issuer != "auth.openai.com" && issuer != "https://auth.openai.com" {
        anyhow::bail!("ID token issuer is not OpenAI");
    }

    let audience_matches = match claims.get("aud") {
        Some(Value::String(audience)) => audience == client_id,
        Some(Value::Array(audiences)) => audiences
            .iter()
            .any(|audience| audience.as_str() == Some(client_id)),
        _ => false,
    };
    if !audience_matches {
        anyhow::bail!("ID token audience does not match the issued client id");
    }

    let expires_at = claims.get("exp").and_then(Value::as_u64).unwrap_or(0);
    if expires_at.saturating_mul(1000) <= current_ms {
        anyhow::bail!("ID token is expired");
    }

    let token_nonce = claims
        .get("nonce")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if token_nonce != nonce {
        anyhow::bail!("ID token nonce does not match this sign-in");
    }

    let account = claims
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .or_else(|| claims.get("sub").and_then(Value::as_str))
        .map(str::to_string);
    Ok(account.unwrap_or_else(|| "chatgpt".to_string()))
}

async fn request_token(fields: &[(&str, &str)]) -> std::result::Result<TokenPayload, TokenFailure> {
    let response = reqwest::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(fields)
        .send()
        .await
        .map_err(|err| {
            TokenFailure::Transient(
                anyhow::Error::new(err).context("failed to reach the OpenAI token endpoint"),
            )
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|err| {
        TokenFailure::Transient(
            anyhow::Error::new(err).context("failed to read the OpenAI token response"),
        )
    })?;
    if !status.is_success() {
        let code = error_code(&body);
        let description = error_description(&body);
        if let Some(code) = code.clone()
            && is_terminal_refresh_code(&code)
        {
            return Err(TokenFailure::Terminal { code, description });
        }
        let detail = match (&code, &description) {
            (Some(code), Some(description)) => format!("{status} ({code}): {description}"),
            (Some(code), None) => format!("{status} ({code})"),
            (None, Some(description)) => format!("{status}: {description}"),
            (None, None) => status.to_string(),
        };
        return Err(TokenFailure::Transient(anyhow!(
            "OpenAI token endpoint returned {detail}"
        )));
    }
    TokenPayload::parse(&body).map_err(TokenFailure::Transient)
}

impl TokenPayload {
    fn parse(body: &str) -> Result<Self> {
        let value: Value =
            serde_json::from_str(body).context("failed to parse the OpenAI token response")?;
        Ok(Self {
            access_token: string_field(&value, "access_token"),
            refresh_token: string_field(&value, "refresh_token"),
            expires_in: value.get("expires_in").and_then(Value::as_u64),
            id_token: string_field(&value, "id_token"),
            scope: scope_field(&value),
            client_id: string_field(&value, "client_id"),
        })
    }

    pub(crate) fn issued_client_id(&self) -> Option<String> {
        self.client_id
            .as_deref()
            .map(str::trim)
            .filter(|client_id| !client_id.is_empty())
            .map(str::to_string)
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn scope_field(value: &Value) -> Option<String> {
    if let Some(scope) = string_field(value, "scope") {
        return Some(scope);
    }
    let scopes = value.get("scopes")?.as_array()?;
    let joined = scopes
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.is_empty()).then_some(joined)
}

pub(crate) fn expires_at_ms(expires_in_secs: u64, current_ms: u64) -> u64 {
    current_ms.saturating_add(expires_in_secs.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use crate::oauth::now_ms;

    fn id_token(claims: Value) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        format!("eyJhbGciOiJub25lIn0.{payload}.signature")
    }

    fn claims(nonce: &str, client_id: &str, expires_in_secs: u64) -> Value {
        serde_json::json!({
            "iss": "https://auth.openai.com",
            "aud": [client_id],
            "exp": (now_ms() / 1000) + expires_in_secs,
            "nonce": nonce,
            "email": "user@example.com",
        })
    }

    #[test]
    fn validates_a_matching_id_token_and_returns_the_account() {
        let account = validate_claims(
            &claims("nonce-1", "oaiapp_1", 600),
            "oaiapp_1",
            "nonce-1",
            now_ms(),
        )
        .unwrap();
        assert_eq!(account, "user@example.com");
    }

    #[test]
    fn rejects_nonce_audience_issuer_and_expiry_mismatches() {
        let matching = claims("nonce-1", "oaiapp_1", 600);
        assert!(validate_claims(&matching, "oaiapp_1", "other", now_ms()).is_err());
        assert!(validate_claims(&matching, "oaiapp_2", "nonce-1", now_ms()).is_err());

        let expired = claims("nonce-1", "oaiapp_1", 0);
        assert!(validate_claims(&expired, "oaiapp_1", "nonce-1", now_ms() + 2000).is_err());

        let mut wrong_issuer = claims("nonce-1", "oaiapp_1", 600);
        wrong_issuer["iss"] = Value::String("https://example.com".to_string());
        assert!(validate_claims(&wrong_issuer, "oaiapp_1", "nonce-1", now_ms()).is_err());
    }

    #[test]
    fn rejects_unsigned_id_tokens() {
        let token = id_token(claims("nonce-1", "oaiapp_1", 600));
        assert!(require_rs256_kid(&token).is_err());
    }

    #[test]
    fn jwt_crypto_provider_is_available() {
        assert!(jsonwebtoken::crypto::CryptoProvider::try_get_default().is_some());
    }

    #[test]
    fn falls_back_to_subject_without_an_email() {
        let mut value = claims("nonce-1", "oaiapp_1", 600);
        value.as_object_mut().unwrap().remove("email");
        value["sub"] = Value::String("user-123".to_string());
        assert_eq!(
            validate_claims(&value, "oaiapp_1", "nonce-1", now_ms()).unwrap(),
            "user-123"
        );
    }

    #[test]
    fn terminal_refresh_codes_are_recognized() {
        for code in TERMINAL_REFRESH_CODES {
            assert!(is_terminal_refresh_code(code));
        }
        assert!(!is_terminal_refresh_code(
            "subscription_sharing_usage_unavailable"
        ));
    }

    #[test]
    fn token_payload_parsing_keeps_only_non_empty_fields() {
        let payload = TokenPayload::parse(
            r#"{"access_token":"a","refresh_token":"","expires_in":3600,"id_token":"i","scope":"s","client_id":"oaiapp_1"}"#,
        )
        .unwrap();
        assert_eq!(payload.access_token.as_deref(), Some("a"));
        assert_eq!(payload.refresh_token, None);
        assert_eq!(payload.expires_in, Some(3600));
        assert_eq!(payload.issued_client_id().as_deref(), Some("oaiapp_1"));
    }
}
