use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
pub use meerai_client::secrets::{Credential, MemorySecretStore, SecretStore, auth_file_path};
use meerai_client::secrets::{CredentialKind, OAuthCredential};
use meerai_core::fs_perms::{FILE_MODE, create_private_dir_sync, set_mode_sync};
use serde::Serialize;

const KEYCHAIN_SERVICE: &str = "meerai";
const KEYCHAIN_INDEX_ACCOUNT: &str = "__providers__";
const DISABLE_KEYCHAIN_ENV: &str = "MEERAI_DISABLE_KEYCHAIN";
const LOCK_BUSY_MESSAGE: &str = "credential file is busy; retry";
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const LOCK_WAIT_LIMIT: Duration = Duration::from_secs(15);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(100);

#[cfg(target_os = "macos")]
const MACOS_SECURITY_BIN: &str = "/usr/bin/security";

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize)]
struct ApiAuthEntry<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    key: &'a str,
}

#[derive(Debug, Serialize)]
struct OAuthAuthEntry<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(flatten)]
    credential: &'a OAuthCredential,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AuthEntry<'a> {
    Api(ApiAuthEntry<'a>),
    OAuth(OAuthAuthEntry<'a>),
}

fn parse_auth_entries(path: &Path, raw: &str) -> Result<BTreeMap<String, Credential>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(BTreeMap::new());
    }
    let root: serde_json::Value = serde_json::from_str(trimmed)
        .with_context(|| format!("failed to parse credentials file {}", path.display()))?;
    let obj = root
        .as_object()
        .with_context(|| format!("expected JSON object in {}", path.display()))?;
    let mut out = BTreeMap::new();
    for (provider, value) in obj {
        let Some(entry) = value.as_object() else {
            continue;
        };
        match entry
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("api")
        {
            "api" => {
                if let Some(secret) = entry.get("key").and_then(|key| key.as_str()) {
                    let trimmed = secret.trim();
                    if !trimmed.is_empty() {
                        out.insert(provider.clone(), Credential::ApiKey(trimmed.to_string()));
                    }
                }
            }
            "oauth" => {
                if let Ok(credential) = serde_json::from_value::<OAuthCredential>(value.clone())
                    && !credential.access.trim().is_empty()
                {
                    out.insert(provider.clone(), Credential::OAuth(credential));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn lock_path(auth_path: &Path) -> PathBuf {
    let mut name = auth_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    auth_path.with_file_name(name)
}

fn create_lock_file(path: &Path) -> std::io::Result<()> {
    write_new_private(path, b"")
}

fn lock_file_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > LOCK_STALE_AFTER)
}

fn try_acquire_lock(path: &Path) -> std::io::Result<bool> {
    match create_lock_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if lock_file_is_stale(path) {
                let _ = fs::remove_file(path);
                if create_lock_file(path).is_ok() {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

pub(crate) struct AuthFileLock {
    path: PathBuf,
}

impl AuthFileLock {
    fn acquire_sync(auth_path: &Path) -> Result<Self> {
        let path = lock_path(auth_path);
        match try_acquire_lock(&path) {
            Ok(true) => Ok(Self { path }),
            Ok(false) => anyhow::bail!(LOCK_BUSY_MESSAGE),
            Err(err) => Err(err).with_context(|| {
                format!("failed to lock credentials file {}", auth_path.display())
            }),
        }
    }

    pub(crate) async fn acquire_path_async(lock_path: &Path) -> Result<Self> {
        let path = lock_path.to_path_buf();
        let deadline = Instant::now() + LOCK_WAIT_LIMIT;
        loop {
            match try_acquire_lock(&path) {
                Ok(true) => return Ok(Self { path }),
                Ok(false) if Instant::now() >= deadline => anyhow::bail!(LOCK_BUSY_MESSAGE),
                Ok(false) => tokio::time::sleep(LOCK_RETRY_DELAY).await,
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to lock credentials file {}", path.display())
                    });
                }
            }
        }
    }

    async fn acquire_async(auth_path: &Path) -> Result<Self> {
        Self::acquire_path_async(&lock_path(auth_path)).await
    }
}

impl Drop for AuthFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn write_new_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    set_mode_sync(path, FILE_MODE).map_err(|err| std::io::Error::other(err.to_string()))
}

pub(crate) fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .with_context(|| format!("path {} has no parent directory", path.display()))?;
    create_private_dir_sync(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "auth.json".to_string());
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let write_result = (|| -> Result<()> {
        let mut file = options
            .open(&temp_path)
            .with_context(|| format!("failed to create temporary file {}", temp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush {}", temp_path.display()))?;
        drop(file);
        set_mode_sync(&temp_path, FILE_MODE)?;
        fs::rename(&temp_path, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

#[derive(Debug)]
pub struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn in_dir(data_dir: &Path) -> Self {
        Self::new(auth_file_path(data_dir))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_from_disk(&self) -> Result<BTreeMap<String, Credential>> {
        match fs::read_to_string(&self.path) {
            Ok(raw) => parse_auth_entries(&self.path, &raw),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(err) => Err(err).with_context(|| {
                format!("failed to read credentials file {}", self.path.display())
            }),
        }
    }

    fn write_entries(&self, entries: &BTreeMap<String, Credential>) -> Result<()> {
        let payload: BTreeMap<&str, AuthEntry<'_>> = entries
            .iter()
            .map(|(provider, credential)| {
                let entry = match credential {
                    Credential::ApiKey(key) => AuthEntry::Api(ApiAuthEntry {
                        kind: "api",
                        key: key.as_str(),
                    }),
                    Credential::OAuth(oauth) => AuthEntry::OAuth(OAuthAuthEntry {
                        kind: "oauth",
                        credential: oauth,
                    }),
                };
                (provider.as_str(), entry)
            })
            .collect();
        let body = format!(
            "{}\n",
            serde_json::to_string_pretty(&payload).context("failed to serialize credentials")?
        );
        write_atomic_private(&self.path, body.as_bytes())
            .with_context(|| format!("failed to write credentials file {}", self.path.display()))
    }

    fn mutate(&self, mutate: impl FnOnce(&mut BTreeMap<String, Credential>) -> bool) -> Result<()> {
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            create_private_dir_sync(parent).with_context(|| {
                format!(
                    "failed to create credentials directory {}",
                    parent.display()
                )
            })?;
        }
        let _guard = AuthFileLock::acquire_sync(&self.path)?;
        self.mutate_unlocked(mutate)
    }

    fn mutate_unlocked(
        &self,
        mutate: impl FnOnce(&mut BTreeMap<String, Credential>) -> bool,
    ) -> Result<()> {
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            create_private_dir_sync(parent).with_context(|| {
                format!(
                    "failed to create credentials directory {}",
                    parent.display()
                )
            })?;
        }
        let mut entries = self.read_from_disk()?;
        if mutate(&mut entries) {
            self.write_entries(&entries)?;
        }
        Ok(())
    }

    pub async fn write_credential_waiting(
        &self,
        provider_id: &str,
        credential: &Credential,
    ) -> Result<()> {
        if let Credential::ApiKey(key) = credential {
            let trimmed = key.trim();
            if trimmed.is_empty() {
                anyhow::bail!("API key cannot be empty");
            }
        }
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            create_private_dir_sync(parent).with_context(|| {
                format!(
                    "failed to create credentials directory {}",
                    parent.display()
                )
            })?;
        }
        let _guard = AuthFileLock::acquire_async(&self.path).await?;
        let mut entries = self.read_from_disk()?;
        entries.insert(provider_id.to_string(), credential.clone());
        self.write_entries(&entries)
    }
}

impl SecretStore for FileSecretStore {
    fn get_key(&self, provider_id: &str) -> Result<Option<String>> {
        Ok(match self.read_from_disk()?.remove(provider_id) {
            Some(Credential::ApiKey(key)) => Some(key),
            _ => None,
        })
    }

    fn set_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        self.mutate(|entries| {
            entries.insert(
                provider_id.to_string(),
                Credential::ApiKey(trimmed.to_string()),
            );
            true
        })
    }

    fn delete_key(&self, provider_id: &str) -> Result<()> {
        self.mutate(|entries| {
            if matches!(entries.get(provider_id), Some(Credential::ApiKey(_))) {
                entries.remove(provider_id);
                true
            } else {
                false
            }
        })
    }

    fn list_providers(&self) -> Result<Vec<String>> {
        let mut providers: Vec<String> = self
            .read_from_disk()?
            .into_iter()
            .filter(|(_, credential)| matches!(credential, Credential::ApiKey(_)))
            .map(|(provider, _)| provider)
            .collect();
        providers.sort();
        Ok(providers)
    }

    fn read_credential(&self, provider_id: &str) -> Result<Option<Credential>> {
        Ok(self.read_from_disk()?.remove(provider_id))
    }

    fn write_credential(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        if let Credential::ApiKey(key) = credential
            && key.trim().is_empty()
        {
            anyhow::bail!("API key cannot be empty");
        }
        self.mutate(|entries| {
            entries.insert(provider_id.to_string(), credential.clone());
            true
        })
    }

    fn auth_lock_path(&self) -> Option<PathBuf> {
        Some(lock_path(&self.path))
    }

    fn write_credential_unlocked(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        if let Credential::ApiKey(key) = credential
            && key.trim().is_empty()
        {
            anyhow::bail!("API key cannot be empty");
        }
        self.mutate_unlocked(|entries| {
            entries.insert(provider_id.to_string(), credential.clone());
            true
        })
    }

    fn delete_credential(&self, provider_id: &str) -> Result<()> {
        self.mutate(|entries| entries.remove(provider_id).is_some())
    }

    fn list_credentials(&self) -> Result<Vec<(String, CredentialKind)>> {
        let mut entries: Vec<(String, CredentialKind)> = self
            .read_from_disk()?
            .into_iter()
            .map(|(provider, credential)| (provider, credential.kind()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
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

#[derive(Debug, Default)]
pub struct NativeKeychainBackend {
    available: OnceLock<bool>,
}

impl NativeKeychainBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(target_os = "macos")]
impl NativeKeychainBackend {
    fn probe(&self) -> bool {
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
}

#[cfg(target_os = "linux")]
impl NativeKeychainBackend {
    fn probe(&self) -> bool {
        if keychain_disabled_by_env() {
            return false;
        }
        let has_dbus =
            std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|val| !val.is_empty());
        has_dbus && binary_on_path("secret-tool")
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl NativeKeychainBackend {
    fn probe(&self) -> bool {
        false
    }
}

#[cfg(target_os = "macos")]
impl KeychainBackend for NativeKeychainBackend {
    fn is_available(&self) -> bool {
        *self.available.get_or_init(|| self.probe())
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
        *self.available.get_or_init(|| self.probe())
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
        *self.available.get_or_init(|| self.probe())
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

    fn keychain_api_key(&self, provider_id: &str) -> Option<String> {
        if provider_id == KEYCHAIN_INDEX_ACCOUNT || !self.backend.is_available() {
            return None;
        }
        match self.backend.get_secret(KEYCHAIN_SERVICE, provider_id) {
            Ok(Some(secret)) => {
                let trimmed = secret.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            _ => None,
        }
    }

    fn drop_keychain_key(&self, provider_id: &str) {
        if !self.backend.is_available() {
            return;
        }
        let _guard = self.index_lock.lock().unwrap();
        if self
            .backend
            .delete_secret(KEYCHAIN_SERVICE, provider_id)
            .is_ok()
        {
            let _ = self.update_index(|providers| {
                providers.remove(provider_id);
            });
        }
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

    fn read_credential(&self, provider_id: &str) -> Result<Option<Credential>> {
        if let Some(key) = self.keychain_api_key(provider_id) {
            return Ok(Some(Credential::ApiKey(key)));
        }
        self.fallback.read_credential(provider_id)
    }

    fn write_credential(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        match credential {
            Credential::ApiKey(key) => self.set_key(provider_id, key),
            Credential::OAuth(_) => {
                if provider_id.trim().is_empty() || provider_id == KEYCHAIN_INDEX_ACCOUNT {
                    anyhow::bail!("invalid provider id '{provider_id}'");
                }
                let result = self.fallback.write_credential(provider_id, credential);
                if result.is_ok() {
                    self.drop_keychain_key(provider_id);
                }
                result
            }
        }
    }

    fn delete_credential(&self, provider_id: &str) -> Result<()> {
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
        let fallback_res = self.fallback.delete_credential(provider_id);
        if let Some(err) = keychain_err {
            fallback_res?;
            return Err(err);
        }
        fallback_res
    }

    fn auth_lock_path(&self) -> Option<PathBuf> {
        self.fallback.auth_lock_path()
    }

    fn write_credential_unlocked(&self, provider_id: &str, credential: &Credential) -> Result<()> {
        match credential {
            Credential::ApiKey(_) => self.write_credential(provider_id, credential),
            Credential::OAuth(_) => {
                if provider_id.trim().is_empty() || provider_id == KEYCHAIN_INDEX_ACCOUNT {
                    anyhow::bail!("invalid provider id '{provider_id}'");
                }
                let result = self
                    .fallback
                    .write_credential_unlocked(provider_id, credential);
                if result.is_ok() {
                    self.drop_keychain_key(provider_id);
                }
                result
            }
        }
    }

    fn list_credentials(&self) -> Result<Vec<(String, CredentialKind)>> {
        let mut merged: BTreeMap<String, CredentialKind> = BTreeMap::new();
        for (provider, kind) in self.fallback.list_credentials()? {
            merged.insert(provider, kind);
        }
        if self.backend.is_available()
            && let Ok(indexed) = self.read_index()
        {
            for provider in indexed {
                merged.insert(provider, CredentialKind::ApiKey);
            }
        }
        Ok(merged.into_iter().collect())
    }
}

pub fn default_secret_store(data_dir: Option<&Path>) -> Arc<dyn SecretStore> {
    match data_dir {
        Some(dir) => Arc::new(KeychainSecretStore::new(FileSecretStore::in_dir(dir))),
        None => Arc::new(MemorySecretStore::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn file_secret_store_reads_and_preserves_oauth_entries() {
        let dir = temp_secrets_dir("secrets-oauth-read");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("auth.json"),
            r#"{
  "openai": { "type": "api", "key": "sk-json" },
  "github-copilot": {
    "type": "oauth",
    "access": "gho_abc",
    "refresh": "ghr_def",
    "expires": 1800000000000,
    "client_id": "oaiapp_test",
    "scopes": ["chatgpt.tokens.use.direct"],
    "account": "user@example.com",
    "needs_relogin": true
  }
}"#,
        )
        .unwrap();

        let store = FileSecretStore::in_dir(&dir);
        assert_eq!(store.get_key("openai").unwrap().as_deref(), Some("sk-json"));
        assert_eq!(store.get_key("github-copilot").unwrap(), None);
        assert_eq!(store.list_providers().unwrap(), vec!["openai".to_string()]);
        assert_eq!(
            store.read_credential("github-copilot").unwrap(),
            Some(Credential::OAuth(OAuthCredential {
                access: "gho_abc".to_string(),
                refresh: "ghr_def".to_string(),
                expires: 1_800_000_000_000,
                client_id: "oaiapp_test".to_string(),
                scopes: vec!["chatgpt.tokens.use.direct".to_string()],
                account: Some("user@example.com".to_string()),
                needs_relogin: true,
            }))
        );
        assert_eq!(
            store.list_credentials().unwrap(),
            vec![
                ("github-copilot".to_string(), CredentialKind::OAuth),
                ("openai".to_string(), CredentialKind::ApiKey),
            ]
        );

        assert!(!dir.join("auth.json.lock").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_secret_store_api_write_preserves_oauth_and_oauth_write_preserves_api() {
        let dir = temp_secrets_dir("secrets-oauth-preserve");
        let store = FileSecretStore::in_dir(&dir);
        store.set_key("openai", "sk-api").unwrap();
        store
            .write_credential("github-copilot", &Credential::OAuth(sample_oauth("gho_1")))
            .unwrap();

        let reloaded = FileSecretStore::in_dir(&dir);
        assert_eq!(
            reloaded.read_credential("github-copilot").unwrap(),
            Some(Credential::OAuth(sample_oauth("gho_1")))
        );
        assert_eq!(
            reloaded.get_key("openai").unwrap().as_deref(),
            Some("sk-api")
        );

        store
            .write_credential("anthropic", &Credential::OAuth(sample_oauth("access-2")))
            .unwrap();
        store.set_key("gemini", "sk-gemini").unwrap();

        let raw = fs::read_to_string(store.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["openai"]["type"], "api");
        assert_eq!(parsed["openai"]["key"], "sk-api");
        assert_eq!(parsed["gemini"]["type"], "api");
        assert_eq!(parsed["github-copilot"]["type"], "oauth");
        assert_eq!(parsed["github-copilot"]["access"], "gho_1");
        assert_eq!(parsed["anthropic"]["type"], "oauth");
        assert_eq!(parsed["anthropic"]["access"], "access-2");
        assert_eq!(
            parsed["github-copilot"]["scopes"][0],
            "chatgpt.tokens.use.direct"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_secret_store_delete_key_keeps_oauth_and_delete_credential_removes_it() {
        let dir = temp_secrets_dir("secrets-delete-kinds");
        let store = FileSecretStore::in_dir(&dir);
        store
            .write_credential("openai", &Credential::OAuth(sample_oauth("access")))
            .unwrap();

        store.delete_key("openai").unwrap();
        assert!(matches!(
            store.read_credential("openai").unwrap(),
            Some(Credential::OAuth(_))
        ));

        store.delete_credential("openai").unwrap();
        assert_eq!(store.read_credential("openai").unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_secret_store_releases_lock_after_success_and_error() {
        let dir = temp_secrets_dir("secrets-lock");
        let store = FileSecretStore::in_dir(&dir);
        let lock = dir.join("auth.json.lock");

        store.set_key("openai", "sk-lock").unwrap();
        assert!(store.path().exists());
        assert!(!lock.exists());

        fs::remove_file(store.path()).unwrap();
        fs::create_dir_all(store.path()).unwrap();
        assert!(store.set_key("openai", "sk-lock").is_err());
        assert!(!lock.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_secret_store_reports_a_busy_credential_file() {
        let dir = temp_secrets_dir("secrets-lock-busy");
        let store = FileSecretStore::in_dir(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("auth.json.lock"), b"").unwrap();

        let err = store.set_key("openai", "sk-busy").unwrap_err();
        assert!(err.to_string().contains("credential file is busy; retry"));
        assert!(dir.join("auth.json.lock").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn file_secret_store_async_write_releases_lock() {
        let dir = temp_secrets_dir("secrets-lock-async");
        let store = FileSecretStore::in_dir(&dir);
        store
            .write_credential_waiting("openai", &Credential::OAuth(sample_oauth("async")))
            .await
            .unwrap();

        assert!(!dir.join("auth.json.lock").exists());
        assert_eq!(
            store.read_credential("openai").unwrap(),
            Some(Credential::OAuth(sample_oauth("async")))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn sample_oauth(access: &str) -> OAuthCredential {
        OAuthCredential {
            access: access.to_string(),
            refresh: "refresh-token".to_string(),
            expires: 1_800_000_000_000,
            client_id: "oaiapp_test".to_string(),
            scopes: vec!["chatgpt.tokens.use.direct".to_string()],
            account: Some("user@example.com".to_string()),
            needs_relogin: false,
        }
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
    fn keychain_secret_store_keeps_oauth_in_the_file_fallback() {
        let dir = temp_secrets_dir("keychain-oauth");
        let backend = MockKeychainBackend::new(true);
        backend.raw_set(KEYCHAIN_SERVICE, "openai", "sk-keychain");
        backend.raw_set(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT, r#"["openai"]"#);

        let store =
            KeychainSecretStore::with_backend(FileSecretStore::in_dir(&dir), backend.clone());
        store
            .write_credential("github-copilot", &Credential::OAuth(sample_oauth("gho_1")))
            .unwrap();

        assert_eq!(backend.raw_get(KEYCHAIN_SERVICE, "github-copilot"), None);
        assert_eq!(
            FileSecretStore::in_dir(&dir)
                .read_credential("github-copilot")
                .unwrap(),
            Some(Credential::OAuth(sample_oauth("gho_1")))
        );
        assert_eq!(
            store.read_credential("github-copilot").unwrap(),
            Some(Credential::OAuth(sample_oauth("gho_1")))
        );
        assert_eq!(
            store.read_credential("openai").unwrap(),
            Some(Credential::ApiKey("sk-keychain".to_string()))
        );
        assert_eq!(store.list_providers().unwrap(), vec!["openai".to_string()]);
        assert_eq!(
            store.list_credentials().unwrap(),
            vec![
                ("github-copilot".to_string(), CredentialKind::OAuth),
                ("openai".to_string(), CredentialKind::ApiKey),
            ]
        );

        store
            .write_credential("openai", &Credential::OAuth(sample_oauth("access-plan")))
            .unwrap();
        assert_eq!(backend.raw_get(KEYCHAIN_SERVICE, "openai"), None);
        assert_eq!(
            store.read_credential("openai").unwrap(),
            Some(Credential::OAuth(sample_oauth("access-plan")))
        );
        assert_eq!(
            backend.raw_get(KEYCHAIN_SERVICE, KEYCHAIN_INDEX_ACCOUNT),
            None
        );

        store.delete_credential("openai").unwrap();
        assert_eq!(store.read_credential("openai").unwrap(), None);
        assert_eq!(
            FileSecretStore::in_dir(&dir)
                .read_credential("openai")
                .unwrap(),
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
