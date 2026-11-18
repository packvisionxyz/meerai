use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use genai::chat::ChatOptions;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, Headers, ModelIden};
pub use meerai_client::providers::*;
pub use meerai_core::env::{MEERAI_USER_AGENT, generate_opencode_session_id};

use meerai_client::secrets::{Credential, SecretStore};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn resolve_auth_data(
    provider: &ProviderInfo,
    secrets: &dyn SecretStore,
) -> std::result::Result<Option<AuthData>, String> {
    let credential = resolve_credential(provider, secrets).map_err(|err| {
        format!(
            "Failed to read credentials for provider '{}': {err:#}",
            provider.id
        )
    })?;
    match credential {
        Some(Credential::ApiKey(key)) => Ok(Some(AuthData::from_single(key))),
        Some(Credential::OAuth(oauth)) => {
            if oauth.needs_relogin {
                return Err(format!(
                    "ChatGPT plan credential for '{}' needs sign-in. Run `meerai auth login {} --chatgpt`.",
                    provider.id, provider.id
                ));
            }
            if oauth.access.trim().is_empty() || oauth.expires <= now_ms() {
                return Err(format!(
                    "ChatGPT plan credential for '{}' is expired. Run `meerai auth login {} --chatgpt`.",
                    provider.id, provider.id
                ));
            }
            Ok(Some(AuthData::from_single(oauth.access)))
        }
        None if !provider.auth.requires_credential() => Ok(None),
        None => Err(format!(
            "No credential configured for provider '{}'. Use /connect to sign in or save a key.",
            provider.id
        )),
    }
}

pub fn build_genai_client(
    runtime: Arc<RwLock<RuntimeProviderState>>,
    secrets: Arc<dyn SecretStore>,
) -> Result<Client> {
    let auth_runtime = runtime.clone();
    let auth_secrets = secrets;
    let target_runtime = runtime;
    let default_session_id = generate_opencode_session_id();

    let client = Client::builder()
        .with_chat_options(ChatOptions::default().with_extra_headers(Headers::from([
            ("x-opencode-session", default_session_id.as_str()),
            ("user-agent", MEERAI_USER_AGENT),
        ])))
        .with_auth_resolver_fn(move |_model_iden: ModelIden| {
            let state = auth_runtime.read().unwrap().clone();
            let provider = state.active_provider_info();
            resolve_auth_data(&provider, auth_secrets.as_ref())
                .map_err(genai::resolver::Error::Custom)
        })
        .with_service_target_resolver_fn(move |mut target: genai::ServiceTarget| {
            let state = target_runtime.read().unwrap().clone();
            let provider = state.active_provider_info();
            if let Some(url) = state.effective_base_url(&provider) {
                let normalized = if url.ends_with('/') {
                    url
                } else {
                    format!("{url}/")
                };
                target.endpoint = Endpoint::from_owned(normalized);
            }
            Ok(target)
        })
        .build()?;

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_client::secrets::{MemorySecretStore, OAuthCredential};

    fn oauth(expires: u64) -> OAuthCredential {
        OAuthCredential {
            access: "access-token".to_string(),
            refresh: "refresh-token".to_string(),
            expires,
            client_id: "oaiapp_test".to_string(),
            scopes: vec!["chatgpt.tokens.use.direct".to_string()],
            account: Some("user@example.com".to_string()),
            needs_relogin: false,
        }
    }

    #[test]
    fn oauth_credentials_resolve_to_the_access_token() {
        let store = MemorySecretStore::new();
        store
            .write_credential("openai", &Credential::OAuth(oauth(now_ms() + 60 * 60_000)))
            .unwrap();

        let catalog = ProviderCatalog::new();
        let provider = catalog.provider("openai").unwrap();
        let auth = resolve_auth_data(provider, &store).unwrap();
        assert!(matches!(auth, Some(AuthData::Key(ref value)) if value == "access-token"));
    }

    #[test]
    fn needs_relogin_credential_fails_with_a_sign_in_message() {
        let store = MemorySecretStore::new();
        let mut credential = oauth(now_ms() + 60 * 60_000);
        credential.needs_relogin = true;
        store
            .write_credential("openai", &Credential::OAuth(credential))
            .unwrap();

        let catalog = ProviderCatalog::new();
        let provider = catalog.provider("openai").unwrap();
        let err = resolve_auth_data(provider, &store).unwrap_err();
        assert!(err.contains("needs sign-in"));
        assert!(err.contains("meerai auth login openai --chatgpt"));
    }

    #[test]
    fn expired_credential_fails_without_refreshing() {
        let store = MemorySecretStore::new();
        store
            .write_credential("openai", &Credential::OAuth(oauth(now_ms() - 1)))
            .unwrap();

        let catalog = ProviderCatalog::new();
        let provider = catalog.provider("openai").unwrap();
        let err = resolve_auth_data(provider, &store).unwrap_err();
        assert!(err.contains("expired"));
        assert!(!err.contains("access-token"));
    }

    #[test]
    fn api_keys_and_keyless_providers_keep_the_previous_behavior() {
        let store = MemorySecretStore::new();
        store.set_key("openai", "sk-test").unwrap();

        let catalog = ProviderCatalog::new();
        let provider = catalog.provider("openai").unwrap();
        assert!(matches!(
            resolve_auth_data(provider, &store).unwrap(),
            Some(AuthData::Key(ref value)) if value == "sk-test"
        ));

        let ollama = catalog.provider("ollama").unwrap();
        assert!(resolve_auth_data(ollama, &store).unwrap().is_none());

        let unconfigured = ProviderInfo {
            id: "unit-test-provider".to_string(),
            display_name: "Unit Test".to_string(),
            adapter: "openai".to_string(),
            default_base_url: None,
            env_vars: vec!["MEERAI_UNIT_TEST_UNUSED_KEY".to_string()],
            auth: ProviderAuthKind::ApiKey,
            default_model: None,
            config_api_key: None,
        };
        let err = resolve_auth_data(&unconfigured, &store).unwrap_err();
        assert!(err.contains("No credential configured"));
    }
}
