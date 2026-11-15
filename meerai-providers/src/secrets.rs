use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use meerai_core::fs_perms::{create_private_dir_sync, write_private_sync};
use serde::{Deserialize, Serialize};

const AUTH_FILE_NAME: &str = "auth.json";
const LEGACY_AUTH_FILE_NAME: &str = "auth.yml";
const KEYCHAIN_SERVICE: &str = "meerai";
const KEYCHAIN_INDEX_ACCOUNT: &str = "__providers__";
const DISABLE_KEYCHAIN_ENV: &str = "MEERAI_DISABLE_KEYCHAIN";

#[cfg(target_os = "macos")]
const MACOS_SECURITY_BIN: &str = "/usr/bin/security";

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
            create_private_dir_sync(parent).with_context(|| {
                format!(
                    "failed to create credentials directory {}",
                    parent.display()
                )
            })?;
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

        write_private_sync(&self.path, body.as_bytes())
            .with_context(|| format!("failed to write credentials file {}", self.path.display()))
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

pub trait KeychainBackend: Send + Sync + std::fmt::Debug {
    fn is_available(&self) -> bool;
    fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>>;
    fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()>;
    fn delete_secret(&self, service: &str, account: &str) -> Result<()>;
}

fn is_truthy_flag(value: Option<&str>) -> bool {
    match value.map(str::trim).map(str::to_ascii_lowercase) {
        Some(flag) => matches!(flag.as_str(), "1" | "true" | "yes"),
        None => false,
    }
}

fn keychain_disabled_by_env() -> bool {
    let value = std::env::var(DISABLE_KEYCHAIN_ENV).ok();
    is_truthy_flag(value.as_deref())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn trim_trailing_newlines(raw: &str) -> String {
    raw.trim_end_matches(['\r', '\n']).to_string()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn command_failure(action: &str, output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string());
    if stderr.is_empty() {
        anyhow::anyhow!("{action} failed with exit status {code}")
    } else {
        anyhow::anyhow!("{action} failed with exit status {code}: {stderr}")
    }
}

#[cfg(target_os = "macos")]
fn is_macos_not_found(output: &Output) -> bool {
    if output.status.code() == Some(44) {
        return true;
    }
    String::from_utf8_lossy(&output.stderr)
        .to_ascii_lowercase()
        .contains("could not be found")
}

#[cfg(target_os = "linux")]
fn binary_on_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| {
        let candidate = dir.join(name);
        if !candidate.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&candidate)
                .map(|meta| meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            true
        }
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NativeKeychainBackend;

impl NativeKeychainBackend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "macos")]
impl KeychainBackend for NativeKeychainBackend {
    fn is_available(&self) -> bool {
        if keychain_disabled_by_env() || !Path::new(MACOS_SECURITY_BIN).is_file() {
            return false;
        }
        let Ok(default_out) = Command::new(MACOS_SECURITY_BIN)
            .arg("default-keychain")
            .stdin(Stdio::null())
            .output()
        else {
            return false;
        };
        if !default_out.status.success() {
            return false;
        }
        let keychain_path = String::from_utf8_lossy(&default_out.stdout)
            .trim()
            .trim_matches('"')
            .to_string();
        if keychain_path.is_empty() || !Path::new(&keychain_path).exists() {
            return false;
        }
        Command::new(MACOS_SECURITY_BIN)
            .args(["show-keychain-info", &keychain_path])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
        let output = Command::new(MACOS_SECURITY_BIN)
            .args(["find-generic-password", "-s", service, "-a", account, "-w"])
            .stdin(Stdio::null())
            .output()
            .context("failed to invoke macOS security command")?;
        if output.status.success() {
            let secret = String::from_utf8(output.stdout)
                .context("macOS keychain returned non-UTF-8 secret")?;
            return Ok(Some(trim_trailing_newlines(&secret)));
        }
        if is_macos_not_found(&output) {
            return Ok(None);
        }
        Err(command_failure("security find-generic-password", &output))
    }

    fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        let output = Command::new(MACOS_SECURITY_BIN)
            .args([
                "add-generic-password",
                "-U",
                "-s",
                service,
                "-a",
                account,
                "-w",
                secret,
            ])
            .stdin(Stdio::null())
            .output()
            .context("failed to invoke macOS security command")?;
        if output.status.success() {
            return Ok(());
        }
        Err(command_failure("security add-generic-password", &output))
    }

    fn delete_secret(&self, service: &str, account: &str) -> Result<()> {
        let output = Command::new(MACOS_SECURITY_BIN)
            .args(["delete-generic-password", "-s", service, "-a", account])
            .stdin(Stdio::null())
            .output()
            .context("failed to invoke macOS security command")?;
        if output.status.success() || is_macos_not_found(&output) {
            return Ok(());
        }
        Err(command_failure("security delete-generic-password", &output))
    }
}

#[cfg(target_os = "linux")]
impl KeychainBackend for NativeKeychainBackend {
    fn is_available(&self) -> bool {
        if keychain_disabled_by_env() {
            return false;
        }
        let has_dbus =
            std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|val| !val.is_empty());
        has_dbus && binary_on_path("secret-tool")
    }

    fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
        let output = Command::new("secret-tool")
            .args(["lookup", "service", service, "account", account])
            .stdin(Stdio::null())
            .output()
            .context("failed to invoke secret-tool")?;
        if output.status.success() {
            let secret = String::from_utf8(output.stdout)
                .context("secret-tool returned non-UTF-8 secret")?;
            return Ok(Some(trim_trailing_newlines(&secret)));
        }
        if output.status.code() == Some(1)
            && String::from_utf8_lossy(&output.stderr).trim().is_empty()
        {
            return Ok(None);
        }
        Err(command_failure("secret-tool lookup", &output))
    }

    fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        use std::io::Write;

        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label=meerai",
                "service",
                service,
                "account",
                account,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to invoke secret-tool")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(secret.as_bytes())
                .context("failed to pipe secret to secret-tool")?;
            stdin
                .flush()
                .context("failed to flush secret to secret-tool")?;
        }
        let output = child
            .wait_with_output()
            .context("failed to wait for secret-tool")?;
        if output.status.success() {
            return Ok(());
        }
        Err(command_failure("secret-tool store", &output))
    }

    fn delete_secret(&self, service: &str, account: &str) -> Result<()> {
        let output = Command::new("secret-tool")
            .args(["clear", "service", service, "account", account])
            .stdin(Stdio::null())
            .output()
            .context("failed to invoke secret-tool")?;
        if output.status.success()
            || (output.status.code() == Some(1)
                && String::from_utf8_lossy(&output.stderr).trim().is_empty())
        {
            return Ok(());
        }
        Err(command_failure("secret-tool clear", &output))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl KeychainBackend for NativeKeychainBackend {
    fn is_available(&self) -> bool {
        false
    }

    fn get_secret(&self, _service: &str, _account: &str) -> Result<Option<String>> {
        anyhow::bail!("native keychain is not supported on this platform")
    }

    fn set_secret(&self, _service: &str, _account: &str, _secret: &str) -> Result<()> {
        anyhow::bail!("native keychain is not supported on this platform")
    }

    fn delete_secret(&self, _service: &str, _account: &str) -> Result<()> {
        anyhow::bail!("native keychain is not supported on this platform")
    }
}

#[derive(Debug)]
pub struct KeychainSecretStore {
    backend: Arc<dyn KeychainBackend>,
    fallback: FileSecretStore,
    index_lock: Mutex<()>,
}

impl KeychainSecretStore {
    pub fn new(fallback: FileSecretStore) -> Self {
        Self::with_backend(fallback, Arc::new(NativeKeychainBackend::new()))
    }

    pub fn with_backend(fallback: FileSecretStore, backend: Arc<dyn KeychainBackend>) -> Self {
        Self {
            backend,
            fallback,
            index_lock: Mutex::new(()),
        }
    }

    pub fn fallback(&self) -> &FileSecretStore {
        &self.fallback
    }

    fn read_index(&self) -> Result<BTreeSet<String>> {
        let Some(raw) = self
            .backend
            .get_secret(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT)?
        else {
            return Ok(BTreeSet::new());
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(BTreeSet::new());
        }
        if trimmed.starts_with('[') {
            let items: Vec<String> =
                serde_json::from_str(trimmed).context("failed to parse keychain provider index")?;
            return Ok(items
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != KEYCHAIN_INDEX_ACCOUNT)
                .collect());
        }
        Ok(trimmed
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != KEYCHAIN_INDEX_ACCOUNT)
            .map(ToOwned::to_owned)
            .collect())
    }

    fn update_index(&self, mutate: impl FnOnce(&mut BTreeSet<String>)) -> Result<()> {
        let mut providers = self.read_index()?;
        mutate(&mut providers);
        if providers.is_empty() {
            return self
                .backend
                .delete_secret(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT);
        }
        let encoded =
            serde_json::to_string(&providers).context("failed to serialize keychain index")?;
        self.backend
            .set_secret(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT, &encoded)
    }
}

impl SecretStore for KeychainSecretStore {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>> {
        if provider_id == KEYCHAIN_INDEX_ACCOUNT {
            return Ok(None);
        }
        if self.backend.is_available()
            && let Ok(Some(secret)) = self.backend.get_secret(KEYCHAIN_SERVICE, provider_id)
        {
            let trimmed = secret.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
        self.fallback.get_key(provider_id)
    }

    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        if provider_id.trim().is_empty() || provider_id == KEYCHAIN_INDEX_ACCOUNT {
            anyhow::bail!("invalid provider id '{provider_id}'");
        }
        if self.backend.is_available() {
            let _guard = self.index_lock.lock().unwrap();
            if self
                .backend
                .set_secret(KEYCHAIN_SERVICE, provider_id, trimmed)
                .is_ok()
            {
                if self
                    .update_index(|providers| {
                        providers.insert(provider_id.to_string());
                    })
                    .is_ok()
                {
                    let _ = self.fallback.delete_key(provider_id);
                    return Ok(());
                }
                let _ = self.backend.delete_secret(KEYCHAIN_SERVICE, provider_id);
            }
        }
        self.fallback.set_key(provider_id, trimmed)
    }

    fn delete_key(&self, provider_id: &str) -> Result<()> {
        if provider_id == KEYCHAIN_INDEX_ACCOUNT {
            return Ok(());
        }
        let mut keychain_err = None;
        if self.backend.is_available() {
            let _guard = self.index_lock.lock().unwrap();
            if let Err(err) = self.backend.delete_secret(KEYCHAIN_SERVICE, provider_id) {
                keychain_err = Some(err);
            } else if let Err(err) = self.update_index(|providers| {
                providers.remove(provider_id);
            }) {
                keychain_err = Some(err);
            }
        }
        let fallback_res = self.fallback.delete_key(provider_id);
        if let Some(err) = keychain_err {
            fallback_res?;
            return Err(err);
        }
        fallback_res
    }

    fn list_providers(&self) -> Result<Vec<String>> {
        let mut providers = BTreeSet::new();
        if self.backend.is_available()
            && let Ok(indexed) = self.read_index()
        {
            providers.extend(indexed);
        }
        providers.extend(self.fallback.list_providers()?);
        Ok(providers.into_iter().collect())
    }
}

pub fn default_secret_store(data_dir: Option<&Path>) -> Arc<dyn SecretStore> {
    match data_dir {
        Some(dir) => Arc::new(KeychainSecretStore::new(FileSecretStore::in_dir(dir))),
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

    #[derive(Debug, Default)]
    struct MockKeychainBackend {
        available: Mutex<bool>,
        fail_get: Mutex<bool>,
        fail_set: Mutex<bool>,
        fail_delete: Mutex<bool>,
        entries: Mutex<BTreeMap<(String, String), String>>,
    }

    impl MockKeychainBackend {
        fn new(available: bool) -> Arc<Self> {
            Arc::new(Self {
                available: Mutex::new(available),
                ..Self::default()
            })
        }

        fn set_available(&self, value: bool) {
            *self.available.lock().unwrap() = value;
        }

        fn set_fail_get(&self, value: bool) {
            *self.fail_get.lock().unwrap() = value;
        }

        fn set_fail_set(&self, value: bool) {
            *self.fail_set.lock().unwrap() = value;
        }

        fn raw_get(&self, service: &str, account: &str) -> Option<String> {
            self.entries
                .lock()
                .unwrap()
                .get(&(service.to_string(), account.to_string()))
                .cloned()
        }

        fn raw_set(&self, service: &str, account: &str, secret: &str) {
            self.entries.lock().unwrap().insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
        }
    }

    impl KeychainBackend for MockKeychainBackend {
        fn is_available(&self) -> bool {
            *self.available.lock().unwrap()
        }

        fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
            if *self.fail_get.lock().unwrap() {
                anyhow::bail!("mock keychain get failure");
            }
            Ok(self
                .entries
                .lock()
                .unwrap()
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }

        fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
            if *self.fail_set.lock().unwrap() {
                anyhow::bail!("mock keychain set failure");
            }
            self.entries.lock().unwrap().insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn delete_secret(&self, service: &str, account: &str) -> Result<()> {
            if *self.fail_delete.lock().unwrap() {
                anyhow::bail!("mock keychain delete failure");
            }
            self.entries
                .lock()
                .unwrap()
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    fn temp_secrets_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meerai-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keychain_secret_store_roundtrip_and_migrates_auth_json() {
        let dir = temp_secrets_dir("keychain-migrate");
        let file_store = FileSecretStore::in_dir(&dir);
        file_store.set_key("openai", "sk-old-file").unwrap();
        file_store.set_key("anthropic", "sk-keep-in-file").unwrap();

        let backend = MockKeychainBackend::new(true);
        let store =
            KeychainSecretStore::with_backend(FileSecretStore::in_dir(&dir), backend.clone());

        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-old-file")
        );
        assert_eq!(
            store.list_providers().unwrap(),
            vec!["anthropic".to_string(), "openai".to_string()]
        );

        store.set_key("openai", "  sk-new-keychain  ").unwrap();
        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-new-keychain")
        );
        assert_eq!(
            backend.raw_get(KEYCHAIN_SERVICE, "openai").as_deref(),
            Some("sk-new-keychain")
        );
        assert_eq!(
            backend
                .raw_get(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT)
                .as_deref(),
            Some(r#"["openai"]"#)
        );

        let reloaded_file = FileSecretStore::in_dir(&dir);
        assert_eq!(reloaded_file.get_key("openai").unwrap(), None);
        assert_eq!(
            reloaded_file.get_key("anthropic").unwrap().as_deref(),
            Some("sk-keep-in-file")
        );

        store.set_key("gemini", "sk-gemini").unwrap();
        assert_eq!(
            store.list_providers().unwrap(),
            vec![
                "anthropic".to_string(),
                "gemini".to_string(),
                "openai".to_string()
            ]
        );
        assert!(store.set_key("openai", "   ").is_err());
        assert!(store.set_key(KEYCHAIN_INDEX_ACCOUNT, "sk-bad").is_err());

        store.delete_key("openai").unwrap();
        assert_eq!(store.get_key("openai").unwrap(), None);
        assert_eq!(backend.raw_get(KEYCHAIN_SERVICE, "openai"), None);
        assert_eq!(
            store.list_providers().unwrap(),
            vec!["anthropic".to_string(), "gemini".to_string()]
        );

        store.delete_key("anthropic").unwrap();
        store.delete_key("gemini").unwrap();
        assert!(store.list_providers().unwrap().is_empty());
        assert_eq!(
            backend.raw_get(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT),
            None
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keychain_secret_store_reads_legacy_auth_yml_when_not_in_keychain() {
        let dir = temp_secrets_dir("keychain-legacy-yml");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("auth.yml"),
            "providers:\n  anthropic: sk-ant-legacy\n",
        )
        .unwrap();

        let backend = MockKeychainBackend::new(true);
        let store =
            KeychainSecretStore::with_backend(FileSecretStore::in_dir(&dir), backend.clone());

        assert_eq!(
            store.get_key("anthropic").unwrap().as_deref(),
            Some("sk-ant-legacy")
        );
        assert_eq!(
            store.list_providers().unwrap(),
            vec!["anthropic".to_string()]
        );

        store.set_key("anthropic", "sk-ant-migrated").unwrap();
        assert_eq!(
            store.get_key("anthropic").unwrap().as_deref(),
            Some("sk-ant-migrated")
        );
        assert_eq!(
            FileSecretStore::in_dir(&dir).get_key("anthropic").unwrap(),
            None
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keychain_secret_store_falls_back_when_unavailable_or_failing() {
        let dir = temp_secrets_dir("keychain-fallback");
        let backend = MockKeychainBackend::new(false);
        let store =
            KeychainSecretStore::with_backend(FileSecretStore::in_dir(&dir), backend.clone());

        store.set_key("openai", "sk-fallback").unwrap();
        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-fallback")
        );
        assert_eq!(backend.raw_get(KEYCHAIN_SERVICE, "openai"), None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        backend.set_available(true);
        backend.set_fail_set(true);
        store.set_key("anthropic", "sk-op-fail").unwrap();
        assert_eq!(
            FileSecretStore::in_dir(&dir)
                .get_key("anthropic")
                .unwrap()
                .as_deref(),
            Some("sk-op-fail")
        );

        backend.set_fail_get(true);
        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-fallback")
        );
        assert_eq!(
            store.list_providers().unwrap(),
            vec!["anthropic".to_string(), "openai".to_string()]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keychain_secret_store_prefers_keychain_and_deletes_both() {
        let dir = temp_secrets_dir("keychain-precedence");
        FileSecretStore::in_dir(&dir)
            .set_key("openai", "sk-stale")
            .unwrap();

        let backend = MockKeychainBackend::new(true);
        backend.raw_set(KEYCHAIN_SERVICE, "openai", "sk-fresh");
        backend.raw_set(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT, r#"["openai"]"#);

        let store = KeychainSecretStore::with_backend(FileSecretStore::in_dir(&dir), backend);
        assert_eq!(
            store.get_key("openai").unwrap().as_deref(),
            Some("sk-fresh")
        );

        store.delete_key("openai").unwrap();
        assert_eq!(store.get_key("openai").unwrap(), None);
        assert_eq!(
            FileSecretStore::in_dir(&dir).get_key("openai").unwrap(),
            None
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disable_keychain_flag_parses_truthy_values() {
        assert!(is_truthy_flag(Some("1")));
        assert!(is_truthy_flag(Some("true")));
        assert!(is_truthy_flag(Some("TRUE")));
        assert!(is_truthy_flag(Some(" yes ")));
        assert!(!is_truthy_flag(Some("0")));
        assert!(!is_truthy_flag(Some("false")));
        assert!(!is_truthy_flag(Some("")));
        assert!(!is_truthy_flag(None));
    }
}
