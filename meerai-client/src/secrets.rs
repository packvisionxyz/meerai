use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;

const AUTH_FILE_NAME: &str = "auth.json";

pub fn auth_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(AUTH_FILE_NAME)
}

pub trait SecretStore: Send + Sync {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>>;
    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()>;
    fn delete_key(&self, provider_id: &str) -> Result<()>;
    fn list_providers(&self) -> Result<Vec<String>>;
}

#[derive(Debug, Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>> {
        let guard = self.entries.lock().unwrap();
        Ok(guard.get(provider_id).cloned())
    }

    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        let mut guard = self.entries.lock().unwrap();
        guard.insert(provider_id.to_string(), trimmed.to_string());
        Ok(())
    }

    fn delete_key(&self, provider_id: &str) -> Result<()> {
        let mut guard = self.entries.lock().unwrap();
        guard.remove(provider_id);
        Ok(())
    }

    fn list_providers(&self) -> Result<Vec<String>> {
        let guard = self.entries.lock().unwrap();
        let mut keys: Vec<String> = guard.keys().cloned().collect();
        keys.sort();
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
