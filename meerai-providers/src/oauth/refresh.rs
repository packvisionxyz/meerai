use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use meerai_client::secrets::{Credential, OAuthCredential, SecretStore};
use tokio::sync::Mutex as AsyncMutex;

use crate::oauth::now_ms;
use crate::oauth::token::{TokenFailure, TokenPayload, expires_at_ms, refresh_tokens};
use crate::secrets::commit_refresh;

static REFRESH_LOCKS: OnceLock<Mutex<BTreeMap<String, Arc<AsyncMutex<()>>>>> = OnceLock::new();

fn refresh_lock(provider_id: &str) -> Arc<AsyncMutex<()>> {
    let locks = REFRESH_LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut guard = locks.lock().unwrap();
    guard.entry(provider_id.to_string()).or_default().clone()
}

fn relogin_message(provider_id: &str) -> String {
    format!(
        "ChatGPT plan credential for '{provider_id}' needs sign-in. Run `meerai auth login {provider_id} --chatgpt`."
    )
}

pub async fn ensure_fresh_credential(store: &dyn SecretStore, provider_id: &str) -> Result<()> {
    let Some(Credential::OAuth(credential)) = store.read_credential(provider_id)? else {
        return Ok(());
    };
    if credential.needs_relogin {
        anyhow::bail!("{}", relogin_message(provider_id));
    }
    if !credential.needs_refresh(now_ms()) {
        return Ok(());
    }
    let lock = refresh_lock(provider_id);
    let _guard = lock.lock().await;
    refresh_locked(store, provider_id).await
}

async fn refresh_locked(store: &dyn SecretStore, provider_id: &str) -> Result<()> {
    let Some(Credential::OAuth(current)) = store.read_credential(provider_id)? else {
        return Ok(());
    };
    if current.needs_relogin {
        anyhow::bail!("{}", relogin_message(provider_id));
    }
    if !current.needs_refresh(now_ms()) {
        return Ok(());
    }

    let client_id = current.client_id.trim();
    if client_id.is_empty() {
        anyhow::bail!(
            "ChatGPT plan credential for '{provider_id}' has no issued client id. Run `meerai auth login {provider_id} --chatgpt`."
        );
    }
    let client_id = client_id.to_string();

    let result = refresh_tokens(&client_id, &current.refresh).await;
    finish_refresh(store, provider_id, current, &client_id, result)
}

fn finish_refresh(
    store: &dyn SecretStore,
    provider_id: &str,
    current: OAuthCredential,
    client_id: &str,
    result: std::result::Result<TokenPayload, TokenFailure>,
) -> Result<()> {
    let (replacement, terminal_code) = match result {
        Ok(payload) => (merge_refresh(current.clone(), payload, client_id)?, None),
        Err(TokenFailure::Terminal { code, .. }) => {
            let mut blocked = current.clone();
            blocked.needs_relogin = true;
            (blocked, Some(code))
        }
        Err(err) => return Err(err.into_error()),
    };

    if commit_refresh(store, provider_id, current, replacement)? {
        if let Some(code) = terminal_code {
            anyhow::bail!(
                "ChatGPT plan credential for '{provider_id}' expired ({code}). Run `meerai auth login {provider_id} --chatgpt`."
            );
        }
        return Ok(());
    }

    if let Some(Credential::OAuth(latest)) = store.read_credential(provider_id)? {
        if latest.needs_relogin {
            anyhow::bail!("{}", relogin_message(provider_id));
        }
        if latest.needs_refresh(now_ms()) {
            anyhow::bail!(
                "ChatGPT plan credential for '{provider_id}' changed during refresh. Retry the request."
            );
        }
    }
    Ok(())
}

fn merge_refresh(
    current: OAuthCredential,
    payload: TokenPayload,
    client_id: &str,
) -> Result<OAuthCredential> {
    let issued_client_id = payload
        .issued_client_id()
        .unwrap_or_else(|| client_id.to_string());
    let access = payload
        .access_token
        .context("OpenAI did not return a refreshed access token")?;
    let expires_in = payload
        .expires_in
        .context("OpenAI did not return a refreshed token expiry")?;
    let refresh = payload.refresh_token.unwrap_or(current.refresh);
    let scopes = match payload.scope {
        Some(scope) => scope.split_whitespace().map(str::to_string).collect(),
        None => current.scopes,
    };

    Ok(OAuthCredential {
        access,
        refresh,
        expires: expires_at_ms(expires_in, now_ms()),
        client_id: issued_client_id,
        scopes,
        account: current.account,
        needs_relogin: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_client::secrets::OAUTH_REFRESH_MARGIN_MS;

    fn credential(expires: u64) -> OAuthCredential {
        OAuthCredential {
            access: "access".to_string(),
            refresh: "refresh".to_string(),
            expires,
            client_id: "oaiapp_1".to_string(),
            scopes: vec!["chatgpt.tokens.use.direct".to_string()],
            account: Some("user@example.com".to_string()),
            needs_relogin: false,
        }
    }

    #[test]
    fn refresh_decision_uses_the_three_minute_margin() {
        let now = 1_000_000_000_000u64;
        let outside = credential(now + OAUTH_REFRESH_MARGIN_MS);
        assert!(!outside.needs_refresh(now));

        let inside = credential(now + OAUTH_REFRESH_MARGIN_MS - 1);
        assert!(inside.needs_refresh(now));

        let expired = credential(now - 1);
        assert!(expired.needs_refresh(now));
    }

    #[test]
    fn merge_refresh_rotates_tokens_and_keeps_the_account() {
        let current = credential(now_ms() + 1000);
        let payload = TokenPayload {
            access_token: Some("new-access".to_string()),
            refresh_token: Some("new-refresh".to_string()),
            expires_in: Some(3600),
            id_token: None,
            scope: Some("openid chatgpt.tokens.use.direct".to_string()),
            client_id: None,
        };
        let merged = merge_refresh(current, payload, "oaiapp_1").unwrap();
        assert_eq!(merged.access, "new-access");
        assert_eq!(merged.refresh, "new-refresh");
        assert_eq!(merged.client_id, "oaiapp_1");
        assert_eq!(
            merged.scopes,
            vec![
                "openid".to_string(),
                "chatgpt.tokens.use.direct".to_string()
            ]
        );
        assert_eq!(merged.account.as_deref(), Some("user@example.com"));
        assert!(merged.expires > now_ms());
        assert!(!merged.needs_relogin);
    }

    #[test]
    fn merge_refresh_keeps_the_previous_refresh_token_when_absent() {
        let current = credential(now_ms() + 1000);
        let payload = TokenPayload {
            access_token: Some("new-access".to_string()),
            refresh_token: None,
            expires_in: Some(3600),
            id_token: None,
            scope: None,
            client_id: Some("oaiapp_new".to_string()),
        };
        let merged = merge_refresh(current, payload, "oaiapp_1").unwrap();
        assert_eq!(merged.refresh, "refresh");
        assert_eq!(merged.client_id, "oaiapp_new");
        assert_eq!(merged.scopes, vec!["chatgpt.tokens.use.direct".to_string()]);
    }

    #[test]
    fn merge_refresh_requires_an_access_token_and_expiry() {
        let current = credential(now_ms() + 1000);
        let mut payload = TokenPayload {
            access_token: None,
            refresh_token: Some("refresh".to_string()),
            expires_in: Some(3600),
            id_token: None,
            scope: None,
            client_id: None,
        };
        assert!(merge_refresh(current.clone(), payload.clone(), "oaiapp_1").is_err());

        payload.access_token = Some("access".to_string());
        payload.expires_in = None;
        assert!(merge_refresh(current, payload, "oaiapp_1").is_err());
    }

    fn rotated_tokens() -> TokenPayload {
        TokenPayload {
            access_token: Some("new-access".into()),
            refresh_token: Some("new-refresh".into()),
            expires_in: Some(3600),
            ..TokenPayload::default()
        }
    }

    fn rejected_token() -> TokenFailure {
        TokenFailure::Terminal {
            code: "invalid_grant".into(),
            description: None,
        }
    }

    #[test]
    fn vault_refresh_preserves_changes_from_another_connection() {
        use crate::secrets::VaultSecretStore;
        use meerai_store::Store;
        use zeroize::Zeroizing;

        struct TestDir(std::path::PathBuf);
        impl Drop for TestDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let id = crate::oauth::pkce::uuid_v4().unwrap();
        let dir = TestDir(std::env::temp_dir().join(format!("meerai-oauth-refresh-{id}")));
        let open = || {
            VaultSecretStore::with_passphrase(
                Store::open(dir.0.join("vault.db")).unwrap(),
                Zeroizing::new("test-only passphrase".into()),
            )
        };
        let first = open();
        let second = open();
        first
            .write_credential("openai", &Credential::OAuth(credential(0)))
            .unwrap();
        let snapshot = |store: &VaultSecretStore| {
            store
                .read_credential("openai")
                .unwrap()
                .unwrap()
                .oauth()
                .unwrap()
                .clone()
        };
        let expected = snapshot(&first);
        let stale = snapshot(&second);

        finish_refresh(&first, "openai", expected, "oaiapp_1", Ok(rotated_tokens())).unwrap();
        let refreshed = snapshot(&first);
        assert_eq!(refreshed.refresh, "new-refresh");
        finish_refresh(
            &second,
            "openai",
            stale.clone(),
            "oaiapp_1",
            Err(rejected_token()),
        )
        .unwrap();
        assert_eq!(snapshot(&second), refreshed);

        first.delete_credential("openai").unwrap();
        finish_refresh(
            &second,
            "openai",
            refreshed.clone(),
            "oaiapp_1",
            Ok(rotated_tokens()),
        )
        .unwrap();
        assert!(first.read_credential("openai").unwrap().is_none());

        first.set_key("openai", "test-api-key").unwrap();
        finish_refresh(&second, "openai", stale, "oaiapp_1", Err(rejected_token())).unwrap();
        assert_eq!(
            first.get_key("openai").unwrap().as_deref(),
            Some("test-api-key")
        );
    }

    #[test]
    fn terminal_failure_blocks_the_matching_credential() {
        let store = meerai_client::secrets::MemorySecretStore::new();
        let current = credential(0);
        store.set_credential("openai", Credential::OAuth(current.clone()));

        let err = finish_refresh(
            &store,
            "openai",
            current.clone(),
            "oaiapp_1",
            Err(rejected_token()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid_grant"));
        let mut blocked = current;
        blocked.needs_relogin = true;
        assert_eq!(
            store.read_credential("openai").unwrap(),
            Some(Credential::OAuth(blocked))
        );
    }

    #[tokio::test]
    async fn ensure_fresh_credential_is_a_noop_without_a_plan_credential() {
        let store = meerai_client::secrets::MemorySecretStore::new();
        ensure_fresh_credential(&store, "openai").await.unwrap();

        store.set_key("openai", "sk-test").unwrap();
        ensure_fresh_credential(&store, "openai").await.unwrap();

        store
            .write_credential(
                "openai",
                &Credential::OAuth(credential(now_ms() + 60 * 60_000)),
            )
            .unwrap();
        ensure_fresh_credential(&store, "openai").await.unwrap();
    }

    #[tokio::test]
    async fn ensure_fresh_credential_fails_fast_when_relogin_is_required() {
        let store = meerai_client::secrets::MemorySecretStore::new();
        let mut blocked = credential(now_ms() + 60 * 60_000);
        blocked.needs_relogin = true;
        store
            .write_credential("openai", &Credential::OAuth(blocked))
            .unwrap();

        let err = ensure_fresh_credential(&store, "openai").await.unwrap_err();
        assert!(
            err.to_string()
                .contains("meerai auth login openai --chatgpt")
        );
    }
}
