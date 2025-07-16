use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const AUTH_FILE_NAME: &str = "auth.json";
const LEGACY_AUTH_FILE_NAME: &str = "auth.yml";

pub fn auth_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(AUTH_FILE_NAME)
}

pub trait SecretStore: Send + Sync {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>>;
    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()>;
    fn delete_key(&self, provider_id: &str) -> Result<()>;
    fn list_providers(&self) -> Result<Vec<String>>;
}

#[derive(Debug, Serialize)]
struct ApiAuthEntry<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    key: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacyYamlAuth {
    Wrapped { providers: BTreeMap<String, String> },
    Flat(BTreeMap<String, String>),
}

fn insert_non_empty(target: &mut BTreeMap<String, String>, provider_id: &str, key: &str) {
    let trimmed = key.trim();
    if !trimmed.is_empty() {
        target.insert(provider_id.to_string(), trimmed.to_string());
    }
}

fn parse_auth_entries(path: &Path, raw: &str) -> Result<BTreeMap<String, String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(BTreeMap::new());
    }
    if trimmed.starts_with('{') {
        let root: serde_json::Value = serde_json::from_str(trimmed)
            .with_context(|| format!("failed to parse credentials file {}", path.display()))?;
        let obj = root
            .as_object()
            .with_context(|| format!("expected JSON object in {}", path.display()))?;
        let mut out = BTreeMap::new();
        for (key, value) in obj {
            match value {
                serde_json::Value::String(secret) => insert_non_empty(&mut out, key, secret),
                serde_json::Value::Object(map) => {
                    if key == "providers" && map.values().all(|v| v.is_string()) && !map.is_empty()
                    {
                        for (p, s) in map {
                            if let Some(secret) = s.as_str() {
                                insert_non_empty(&mut out, p, secret);
                            }
                        }
                    } else if map
                        .get("type")
                        .and_then(|v| v.as_str())
                        .is_none_or(|kind| kind == "api")
                        && let Some(secret) = map.get("key").and_then(|v| v.as_str())
                    {
                        insert_non_empty(&mut out, key, secret);
                    }
                }
                _ => {}
            }
        }
        return Ok(out);
    }

    let parsed: LegacyYamlAuth = serde_yaml::from_str(trimmed)
        .with_context(|| format!("failed to parse credentials file {}", path.display()))?;
    let raw_map = match parsed {
        LegacyYamlAuth::Wrapped { providers } | LegacyYamlAuth::Flat(providers) => providers,
    };
    let mut out = BTreeMap::new();
    for (k, v) in raw_map {
        insert_non_empty(&mut out, &k, &v);
    }
    Ok(out)
}

#[derive(Debug)]
pub struct FileSecretStore {
    path: PathBuf,
    cache: Mutex<Option<BTreeMap<String, String>>>,
}

impl FileSecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cache: Mutex::new(None),
        }
    }

    pub fn in_dir(data_dir: &Path) -> Self {
        Self::new(auth_file_path(data_dir))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn ensure_loaded(&self) -> Result<BTreeMap<String, String>> {
        let mut guard = self.cache.lock().unwrap();
        if let Some(entries) = guard.as_ref() {
            return Ok(entries.clone());
        }
        let loaded = match fs::read_to_string(&self.path) {
            Ok(raw) => parse_auth_entries(&self.path, &raw)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let legacy_path = self.path.with_file_name(LEGACY_AUTH_FILE_NAME);
                if self.path.file_name().is_some_and(|n| n == AUTH_FILE_NAME)
                    && legacy_path.exists()
                {
                    let raw = fs::read_to_string(&legacy_path).with_context(|| {
                        format!("failed to read credentials file {}", legacy_path.display())
                    })?;
                    parse_auth_entries(&legacy_path, &raw)?
                } else {
                    BTreeMap::new()
                }
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to read credentials file {}", self.path.display())
                });
            }
        };
        *guard = Some(loaded.clone());
        Ok(loaded)
    }

    fn write_entries(&self, entries: &BTreeMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create credentials directory {}",
                    parent.display()
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }
        let payload: BTreeMap<&str, ApiAuthEntry<'_>> = entries
            .iter()
            .map(|(provider, key)| {
                (
                    provider.as_str(),
                    ApiAuthEntry {
                        kind: "api",
                        key: key.as_str(),
                    },
                )
            })
            .collect();
        let body = format!(
            "{}\n",
            serde_json::to_string_pretty(&payload).context("failed to serialize credentials")?
        );

        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&self.path)
        }
        .with_context(|| format!("failed to write credentials file {}", self.path.display()))?;

        #[cfg(not(unix))]
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)
            .with_context(|| format!("failed to write credentials file {}", self.path.display()))?;

        file.write_all(body.as_bytes())
            .with_context(|| format!("failed to write credentials file {}", self.path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).with_context(
                || {
                    format!(
                        "failed to set permissions on credentials file {}",
                        self.path.display()
                    )
                },
            )?;
        }

        Ok(())
    }
}

impl SecretStore for FileSecretStore {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>> {
        let entries = self.ensure_loaded()?;
        Ok(entries.get(provider_id).cloned())
    }

    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        let mut entries = self.ensure_loaded()?;
        entries.insert(provider_id.to_string(), trimmed.to_string());
        self.write_entries(&entries)?;
        let mut guard = self.cache.lock().unwrap();
        *guard = Some(entries);
        Ok(())
    }

    fn delete_key(&self, provider_id: &str) -> Result<()> {
        let mut entries = self.ensure_loaded()?;
        if entries.remove(provider_id).is_some() {
            self.write_entries(&entries)?;
            let mut guard = self.cache.lock().unwrap();
            *guard = Some(entries);
        }
        Ok(())
    }

    fn list_providers(&self) -> Result<Vec<String>> {
        let entries = self.ensure_loaded()?;
        Ok(entries.keys().cloned().collect())
    }
}

pub fn default_secret_store(data_dir: Option<&Path>) -> Arc<dyn SecretStore> {
    match data_dir {
        Some(dir) => Arc::new(FileSecretStore::in_dir(dir)),
        None => Arc::new(MemorySecretStore::new()),
    }
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

    #[test]
    fn file_secret_store_roundtrip_writes_opencode_auth_json() {
        let dir = std::env::temp_dir().join(format!("meerai-secrets-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = FileSecretStore::in_dir(&dir);
        assert_eq!(store.path(), dir.join("auth.json"));
        assert_eq!(store.get_key("openai").unwrap(), None);
        assert!(!store.path().exists());

        store.set_key("openai", "  sk-file-123  ").unwrap();
        assert!(store.path().exists());
        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-file-123")
        );
        assert_eq!(store.list_providers().unwrap(), vec!["openai".to_string()]);

        let raw = fs::read_to_string(store.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["openai"]["type"], "api");
        assert_eq!(parsed["openai"]["key"], "sk-file-123");

        let reloaded = FileSecretStore::in_dir(&dir);
        assert_eq!(
            reloaded.get_key("openai").unwrap().as_deref(),
            Some("sk-file-123")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        reloaded.delete_key("openai").unwrap();
        let after_delete = FileSecretStore::in_dir(&dir);
        assert_eq!(after_delete.get_key("openai").unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_secret_store_reads_legacy_auth_yml_and_ignores_oauth() {
        let dir =
            std::env::temp_dir().join(format!("meerai-secrets-legacy-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("auth.yml"),
            "providers:\n  anthropic: sk-ant-legacy\n",
        )
        .unwrap();

        let store = FileSecretStore::in_dir(&dir);
        assert_eq!(
            store.get_key("anthropic").unwrap().as_deref(),
            Some("sk-ant-legacy")
        );

        fs::write(
            dir.join("auth.json"),
            r#"{
  "openai": { "type": "api", "key": "sk-json" },
  "github-copilot": { "type": "oauth", "access": "gho_abc" }
}"#,
        )
        .unwrap();
        let json_store = FileSecretStore::in_dir(&dir);
        assert_eq!(
            json_store.get_key("openai").unwrap().as_deref(),
            Some("sk-json")
        );
        assert_eq!(json_store.get_key("github-copilot").unwrap(), None);
        assert_eq!(
            json_store.list_providers().unwrap(),
            vec!["openai".to_string()]
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
