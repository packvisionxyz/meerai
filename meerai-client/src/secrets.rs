use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};

const AUTH_FILE_NAME: &str = "auth.json";
pub const OAUTH_REFRESH_MARGIN_MS: u64 = 3 * 60 * 1000;

pub fn auth_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(AUTH_FILE_NAME)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    ApiKey,
    OAuth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    #[serde(default)]
    pub expires: u64,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_relogin: bool,
}

impl OAuthCredential {
    pub fn needs_refresh(&self, now_ms: u64) -> bool {
        self.expires.saturating_sub(now_ms) < OAUTH_REFRESH_MARGIN_MS
    }

    pub fn has_plan_scope(&self) -> bool {
        self.scopes
            .iter()
            .any(|scope| scope == "chatgpt.tokens.use.direct")
    }

    pub fn usable(&self, now_ms: u64) -> bool {
        !self.needs_relogin && !self.access.is_empty() && self.expires > now_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    ApiKey(String),
    OAuth(OAuthCredential),
}

impl Credential {
    pub fn kind(&self) -> CredentialKind {
        match self {
            Self::ApiKey(_) => CredentialKind::ApiKey,
            Self::OAuth(_) => CredentialKind::OAuth,
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(key) => Some(key),
            Self::OAuth(_) => None,
        }
    }

    pub fn oauth(&self) -> Option<&OAuthCredential> {
        match self {
            Self::ApiKey(_) => None,
            Self::OAuth(credential) => Some(credential),
        }
    }
}

pub trait SecretStore: Send + Sync {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>>;
    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()>;
    fn delete_key(&self, provider_id: &str) -> Result<()>;
    fn list_providers(&self) -> Result<Vec<String>>;

    fn read_credential(&self, provider_id: &str) -> Result<Option<Credential>> {
        Ok(self.get_key(provider_id)?.map(Credential::ApiKey))
    }

    fn write_credential(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        match credential {
            Credential::ApiKey(key) => self.set_key(provider_id, key),
            Credential::OAuth(_) => {
                anyhow::bail!("this credential store cannot persist OAuth credentials")
            }
        }
    }

    fn delete_credential(&self, provider_id: &str) -> Result<()> {
        self.delete_key(provider_id)
    }

    fn auth_lock_path(&self) -> Option<PathBuf> {
        None
    }

    fn write_credential_unlocked(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        self.write_credential(provider_id, credential)
    }

    fn list_credentials(&self) -> Result<Vec<(String, CredentialKind)>> {
        Ok(self
            .list_providers()?
            .into_iter()
            .map(|provider| (provider, CredentialKind::ApiKey))
            .collect())
    }
}

#[derive(Debug, Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<String, Credential>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_credential(&self, provider_id: &str, credential: Credential) {
        self.entries
            .lock()
            .unwrap()
            .insert(provider_id.to_string(), credential);
    }
}

impl SecretStore for MemorySecretStore {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>> {
        let guard = self.entries.lock().unwrap();
        Ok(guard
            .get(provider_id)
            .and_then(|credential| credential.api_key().map(str::to_string)))
    }

    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        let mut guard = self.entries.lock().unwrap();
        guard.insert(
            provider_id.to_string(),
            Credential::ApiKey(trimmed.to_string()),
        );
        Ok(())
    }

    fn delete_key(&self, provider_id: &str) -> Result<()> {
        let mut guard = self.entries.lock().unwrap();
        guard.remove(provider_id);
        Ok(())
    }

    fn list_providers(&self) -> Result<Vec<String>> {
        let guard = self.entries.lock().unwrap();
        let mut keys: Vec<String> = guard
            .iter()
            .filter(|(_, credential)| credential.api_key().is_some())
            .map(|(provider, _)| provider.clone())
            .collect();
        keys.sort();
        Ok(keys)
    }

    fn read_credential(&self, provider_id: &str) -> Result<Option<Credential>> {
        Ok(self.entries.lock().unwrap().get(provider_id).cloned())
    }

    fn write_credential(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        if matches!(credential, Credential::ApiKey(key) if key.trim().is_empty()) {
            anyhow::bail!("API key cannot be empty");
        }
        self.set_credential(provider_id, credential.clone());
        Ok(())
    }

    fn delete_credential(&self, provider_id: &str) -> Result<()> {
        self.entries.lock().unwrap().remove(provider_id);
        Ok(())
    }

    fn list_credentials(&self) -> Result<Vec<(String, CredentialKind)>> {
        let guard = self.entries.lock().unwrap();
        let mut entries: Vec<(String, CredentialKind)> = guard
            .iter()
            .map(|(provider, credential)| (provider.clone(), credential.kind()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth() -> OAuthCredential {
        OAuthCredential {
            access: "access-token".to_string(),
            refresh: "refresh-token".to_string(),
            expires: 1_800_000_000_000,
            client_id: "oaiapp_test".to_string(),
            scopes: vec!["chatgpt.tokens.use.direct".to_string()],
            account: Some("user@example.com".to_string()),
            needs_relogin: false,
        }
    }

    #[test]
    fn memory_secret_store_roundtrip() {
        let store = MemorySecretStore::new();
        assert_eq!(store.get_key("openai").unwrap(), None);

        store.set_key("openai", "  sk-test-123  ").unwrap();
        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-test-123")
        );

        assert!(store.set_key("openai", "   ").is_err());

        store.delete_key("openai").unwrap();
        assert_eq!(store.get_key("openai").unwrap(), None);
    }

    #[test]
    fn memory_store_keeps_oauth_and_api_keys_apart() {
        let store = MemorySecretStore::new();
        store.set_key("openai", "sk-test").unwrap();
        store
            .write_credential("anthropic", &Credential::OAuth(oauth()))
            .unwrap();

        assert_eq!(store.get_key("openai").unwrap().as_deref(), Some("sk-test"));
        assert_eq!(store.get_key("anthropic").unwrap(), None);
        assert_eq!(
            store.read_credential("anthropic").unwrap(),
            Some(Credential::OAuth(oauth()))
        );
        assert_eq!(store.list_providers().unwrap(), vec!["openai".to_string()]);
        assert_eq!(
            store.list_credentials().unwrap(),
            vec![
                ("anthropic".to_string(), CredentialKind::OAuth),
                ("openai".to_string(), CredentialKind::ApiKey),
            ]
        );

        store.delete_credential("anthropic").unwrap();
        assert_eq!(store.read_credential("anthropic").unwrap(), None);
    }

    #[test]
    fn oauth_scope_and_expiry_rules() {
        let credential = oauth();
        assert!(credential.has_plan_scope());
        assert!(credential.usable(1_700_000_000_000));
        assert!(!credential.needs_refresh(1_700_000_000_000));
        assert!(credential.needs_refresh(1_800_000_000_000 - 1000));
        assert!(!credential.usable(1_800_000_000_000));

        let mut blocked = credential.clone();
        blocked.needs_relogin = true;
        assert!(!blocked.usable(1_700_000_000_000));

        let mut no_scope = credential;
        no_scope.scopes.clear();
        assert!(!no_scope.has_plan_scope());
    }
}
