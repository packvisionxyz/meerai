use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use meerai_client::secrets::{Credential, OAuthCredential, SecretStore};

use crate::oauth::authorize::{AuthorizeParams, authorize_url};
use crate::oauth::callback::{
    bind_listener, callback_redirect_uri, parse_callback_input, verify_state, wait_for_redirect,
};
use crate::oauth::host::load_or_create_host_id;
use crate::oauth::pkce::{challenge_for, code_verifier, random_token};
use crate::oauth::token::{TokenPayload, exchange_code, expires_at_ms, validate_id_token};
use crate::oauth::{AGENT_NAME_HINT, DYNAMIC_CLIENT_ID, PLAN_SCOPE, now_ms};

#[async_trait]
pub trait LoginInteraction: Send + Sync {
    async fn present_url(&self, url: &str) -> Result<()>;
    async fn read_pasted_input(&self) -> Result<String>;
}

pub async fn login_chatgpt(
    store: &dyn SecretStore,
    provider_id: &str,
    data_dir: &Path,
    interaction: &dyn LoginInteraction,
) -> Result<OAuthCredential> {
    let host_id = load_or_create_host_id(data_dir)?;
    let stored_client_id = stored_client_id(store, provider_id)?;
    let new_registration = stored_client_id.is_none();
    let requested_client_id = stored_client_id.unwrap_or_else(|| DYNAMIC_CLIENT_ID.to_string());

    let (listener, port) = bind_listener().await?;
    let redirect_uri = callback_redirect_uri(port);
    let verifier = code_verifier()?;
    let challenge = challenge_for(&verifier);
    let state = random_token()?;
    let nonce = random_token()?;

    let url = authorize_url(&AuthorizeParams {
        client_id: &requested_client_id,
        redirect_uri: &redirect_uri,
        state: &state,
        nonce: &nonce,
        code_challenge: &challenge,
        host_id: &host_id,
        agent_name_hint: new_registration.then_some(AGENT_NAME_HINT),
    });
    interaction.present_url(&url).await?;

    let callback = tokio::select! {
        result = wait_for_redirect(&listener, &state) => result?,
        pasted = interaction.read_pasted_input() => parse_callback_input(&pasted?)?,
    };
    verify_state(&state, callback.state.as_deref())?;

    if new_registration && callback.client_id.is_none() {
        anyhow::bail!("sign-in callback did not include the issued client id; start a new sign-in");
    }
    if !new_registration
        && let Some(returned) = callback.client_id.as_deref()
        && returned != requested_client_id
    {
        anyhow::bail!(
            "sign-in callback returned a different client id; sign out and start a new sign-in"
        );
    }

    let exchange_client_id = callback
        .client_id
        .unwrap_or_else(|| requested_client_id.clone());
    let payload = exchange_code(
        &exchange_client_id,
        &callback.code,
        &verifier,
        &redirect_uri,
    )
    .await?;
    finish_login(payload, &exchange_client_id, &nonce).await
}

async fn finish_login(
    payload: TokenPayload,
    exchange_client_id: &str,
    nonce: &str,
) -> Result<OAuthCredential> {
    let parts = finish_login_parts(payload, exchange_client_id)?;
    let current_ms = now_ms();
    let account =
        validate_id_token(&parts.id_token, &parts.issued_client_id, nonce, current_ms).await?;
    Ok(build_credential(parts, account, current_ms))
}

struct LoginParts {
    access: String,
    refresh: String,
    expires_in: u64,
    id_token: String,
    scope: String,
    issued_client_id: String,
}

fn finish_login_parts(payload: TokenPayload, exchange_client_id: &str) -> Result<LoginParts> {
    let TokenPayload {
        access_token,
        refresh_token,
        expires_in,
        id_token,
        scope,
        client_id,
    } = payload;

    let access = access_token.context("OpenAI did not return an access token")?;
    let refresh = refresh_token.context("OpenAI did not return a refresh token")?;
    let expires_in = expires_in.context("OpenAI did not return a token expiry")?;
    let id_token = id_token.context("OpenAI did not return an ID token")?;
    let scope = scope.context("OpenAI did not return the granted scopes")?;

    if !scope
        .split_whitespace()
        .any(|granted| granted == PLAN_SCOPE)
    {
        anyhow::bail!(
            "ChatGPT did not grant plan usage ({PLAN_SCOPE}). Sign in again, or connect an OpenAI API key."
        );
    }

    Ok(LoginParts {
        access,
        refresh,
        expires_in,
        id_token,
        scope,
        issued_client_id: client_id.unwrap_or_else(|| exchange_client_id.to_string()),
    })
}

fn build_credential(parts: LoginParts, account: String, current_ms: u64) -> OAuthCredential {
    OAuthCredential {
        access: parts.access,
        refresh: parts.refresh,
        expires: expires_at_ms(parts.expires_in, current_ms),
        client_id: parts.issued_client_id,
        scopes: parts.scope.split_whitespace().map(str::to_string).collect(),
        account: Some(account),
        needs_relogin: false,
    }
}

fn stored_client_id(store: &dyn SecretStore, provider_id: &str) -> Result<Option<String>> {
    let Some(Credential::OAuth(oauth)) = store.read_credential(provider_id)? else {
        return Ok(None);
    };
    let client_id = oauth.client_id.trim();
    Ok((!client_id.is_empty()).then(|| client_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use meerai_client::secrets::MemorySecretStore;

    fn id_token(nonce: &str, client_id: &str) -> String {
        let claims = serde_json::json!({
            "iss": "auth.openai.com",
            "aud": client_id,
            "exp": (now_ms() / 1000) + 600,
            "nonce": nonce,
            "email": "user@example.com",
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        format!("eyJhbGciOiJub25lIn0.{payload}.signature")
    }

    fn payload(scope: &str) -> TokenPayload {
        TokenPayload {
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            expires_in: Some(3600),
            id_token: Some(id_token("nonce-1", "oaiapp_1")),
            scope: Some(scope.to_string()),
            client_id: None,
        }
    }

    #[test]
    fn finish_login_requires_the_plan_scope() {
        let Err(err) = finish_login_parts(payload("openid profile email"), "oaiapp_1") else {
            panic!("a grant without plan usage must fail");
        };
        assert!(err.to_string().contains("did not grant plan usage"));
    }

    #[test]
    fn finish_login_builds_a_reusable_credential() {
        let parts =
            finish_login_parts(payload("openid chatgpt.tokens.use.direct"), "oaiapp_1").unwrap();
        let credential = build_credential(parts, "user@example.com".to_string(), now_ms());
        assert_eq!(credential.access, "access");
        assert_eq!(credential.refresh, "refresh");
        assert_eq!(credential.client_id, "oaiapp_1");
        assert_eq!(
            credential.scopes,
            vec![
                "openid".to_string(),
                "chatgpt.tokens.use.direct".to_string()
            ]
        );
        assert_eq!(credential.account.as_deref(), Some("user@example.com"));
        assert!(!credential.needs_relogin);
        assert!(credential.expires > now_ms());
    }

    #[test]
    fn finish_login_reports_missing_fields() {
        let mut missing = payload("chatgpt.tokens.use.direct");
        missing.refresh_token = None;
        let Err(err) = finish_login_parts(missing, "oaiapp_1") else {
            panic!("a missing refresh token must fail");
        };
        assert!(err.to_string().contains("refresh token"));
    }

    #[test]
    fn stored_client_id_is_reused_for_the_same_provider() {
        let store = MemorySecretStore::new();
        assert_eq!(stored_client_id(&store, "openai").unwrap(), None);

        store
            .write_credential(
                "openai",
                &Credential::OAuth(OAuthCredential {
                    access: "access".to_string(),
                    refresh: "refresh".to_string(),
                    expires: 0,
                    client_id: "oaiapp_stored".to_string(),
                    scopes: Vec::new(),
                    account: None,
                    needs_relogin: false,
                }),
            )
            .unwrap();
        assert_eq!(
            stored_client_id(&store, "openai").unwrap().as_deref(),
            Some("oaiapp_stored")
        );
        assert_eq!(stored_client_id(&store, "anthropic").unwrap(), None);
    }
}
