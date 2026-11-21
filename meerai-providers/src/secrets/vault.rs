use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, AeadInOut, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use meerai_client::secrets::{Credential, CredentialKind, OAuthCredential, SecretStore};
use meerai_store::Store;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{KeychainBackend, REFRESH_WRITE, refresh_matches};

const PASSPHRASE_ENV: &str = "MEERAI_VAULT_PASSPHRASE";
const INITIALIZED: &str = "vault.initialized";
const KEY_META: &str = "vault.key.v1";
const WRAP_AAD: &[u8] = b"meerai vault Argon2id DEK wrap v1";
const SERVICE: &str = "meerai";

type Dek = Zeroizing<[u8; 32]>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultMode {
    Uninitialized,
    Keychain,
    Passphrase,
}

enum KeySource {
    Environment,
    Passphrase(Zeroizing<String>),
    Keychain(Box<dyn KeychainBackend>),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Envelope {
    Passphrase {
        salt: [u8; 16],
        nonce: [u8; 12],
        wrapped: Vec<u8>,
    },
    Keychain {
        id: [u8; 16],
        nonce: [u8; 12],
        verifier: Vec<u8>,
    },
}

struct CachedDek {
    envelope: String,
    dek: Dek,
}

pub struct VaultSecretStore {
    database: Store,
    source: KeySource,
    cache: Mutex<Option<CachedDek>>,
}

impl std::fmt::Debug for VaultSecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSecretStore").finish_non_exhaustive()
    }
}

impl VaultSecretStore {
    pub fn new(store: Store) -> Self {
        Self::configured(store, KeySource::Environment)
    }

    pub fn with_passphrase(store: Store, passphrase: Zeroizing<String>) -> Self {
        Self::configured(store, KeySource::Passphrase(passphrase))
    }

    pub fn with_keychain(store: Store, backend: Box<dyn KeychainBackend>) -> Self {
        Self::configured(store, KeySource::Keychain(backend))
    }

    pub fn reset(&self) -> Result<usize> {
        let store = self.database()?;
        let removed = store.immediate(|tx| {
            let encoded: Option<String> = tx
                .query_row("SELECT value FROM meta WHERE key = ?1", [KEY_META], |row| {
                    row.get(0)
                })
                .optional()?;
            if let (Some(encoded), KeySource::Keychain(backend)) = (encoded, &self.source)
                && let Ok(Envelope::Keychain { id, .. }) = serde_json::from_str(&encoded)
            {
                backend
                    .delete_secret(SERVICE, &account(&id))
                    .map_err(|error| anyhow!("could not delete native vault key: {error}"))?;
            }
            let removed = tx.execute("DELETE FROM vault_secrets", [])?;
            tx.execute(
                "DELETE FROM meta WHERE key IN (?1, ?2)",
                params![INITIALIZED, KEY_META],
            )?;
            self.initialize(tx)?;
            Ok(removed)
        })?;
        *self
            .cache
            .lock()
            .map_err(|_| anyhow!("vault key cache lock poisoned"))? = None;
        Ok(removed)
    }

    pub fn stored_mode(store: &Store) -> Result<VaultMode> {
        let encoded: Option<String> = store.read(|conn| {
            Ok(conn
                .query_row("SELECT value FROM meta WHERE key = ?1", [KEY_META], |row| {
                    row.get(0)
                })
                .optional()?)
        })?;
        let Some(encoded) = encoded else {
            return Ok(VaultMode::Uninitialized);
        };
        let envelope: Envelope =
            serde_json::from_str(&encoded).map_err(|_| anyhow!("invalid vault key metadata"))?;
        Ok(match envelope {
            Envelope::Passphrase { .. } => VaultMode::Passphrase,
            Envelope::Keychain { .. } => VaultMode::Keychain,
        })
    }

    fn configured(database: Store, source: KeySource) -> Self {
        Self {
            database,
            source,
            cache: Mutex::new(None),
        }
    }

    fn database(&self) -> Result<Store> {
        Ok(self.database.clone())
    }

    fn kek(&self, salt: &[u8; 16]) -> Result<Dek> {
        let env;
        let passphrase = match &self.source {
            KeySource::Passphrase(passphrase) => passphrase,
            KeySource::Environment => {
                env =
                    Zeroizing::new(std::env::var(PASSPHRASE_ENV).map_err(|_| {
                        anyhow!("set {PASSPHRASE_ENV} to unlock the credential vault")
                    })?);
                &env
            }
            KeySource::Keychain(_) => bail!("credential vault requires its original passphrase"),
        };
        ensure!(
            !passphrase.trim().is_empty(),
            "set a non-empty {PASSPHRASE_ENV} to unlock the credential vault"
        );
        let mut key = Zeroizing::new([0; 32]);
        let params =
            Params::new(64 * 1024, 3, 1, Some(32)).expect("valid vault v1 Argon2id parameters");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.as_bytes(), salt, &mut *key)
            .map_err(|_| anyhow!("could not derive vault wrapping key"))?;
        Ok(key)
    }

    fn initialize(&self, conn: &Connection) -> Result<Dek> {
        let mut dek = Zeroizing::new([0; 32]);
        fill(&mut *dek)?;
        let nonce = random()?;
        let envelope = match &self.source {
            KeySource::Environment | KeySource::Passphrase(_) => {
                let salt = random()?;
                let kek = self.kek(&salt)?;
                Envelope::Passphrase {
                    salt,
                    nonce,
                    wrapped: seal(&kek, &nonce, WRAP_AAD, &*dek)?,
                }
            }
            KeySource::Keychain(backend) => {
                ensure!(
                    backend.is_available(),
                    "native credential vault unavailable"
                );
                // A per-database account also keeps concurrent initialization of distinct DBs independent.
                let id = random()?;
                let account = account(&id);
                let verifier = seal(&dek, &nonce, account.as_bytes(), b"")?;
                ensure!(
                    backend
                        .get_secret(SERVICE, &account)
                        .map_err(|_| anyhow!("could not check native vault key"))?
                        .is_none(),
                    "native vault key already exists"
                );
                let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(*dek));
                backend
                    .set_secret(SERVICE, &account, &encoded)
                    .map_err(|_| anyhow!("could not save native vault key"))?;
                Envelope::Keychain {
                    id,
                    nonce,
                    verifier,
                }
            }
        };
        let encoded = serde_json::to_string(&envelope).context("encode vault key envelope")?;
        conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, '1'), (?2, ?3)",
            params![INITIALIZED, KEY_META, encoded],
        )?;
        if matches!(envelope, Envelope::Passphrase { .. }) {
            self.cache(encoded, dek.clone())?;
        }
        Ok(dek)
    }

    fn cache(&self, envelope: String, dek: Dek) -> Result<()> {
        *self
            .cache
            .lock()
            .map_err(|_| anyhow!("vault key cache lock poisoned"))? =
            Some(CachedDek { envelope, dek });
        Ok(())
    }

    fn key(&self, conn: &Connection, create: bool) -> Result<Option<Dek>> {
        let (marker, encoded): (Option<String>, Option<String>) = conn.query_row(
            "SELECT (SELECT value FROM meta WHERE key = ?1), (SELECT value FROM meta WHERE key = ?2)",
            params![INITIALIZED, KEY_META], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let Some(encoded) = encoded else {
            let occupied: bool =
                conn.query_row("SELECT EXISTS(SELECT 1 FROM vault_secrets)", [], |r| {
                    r.get(0)
                })?;
            ensure!(
                marker.is_none() && !occupied,
                "credential vault key metadata is missing or unreadable; restore the vault from                  backup or run `meerai auth reset-vault` to discard the unrecoverable saved                  credentials"
            );
            return if create {
                self.initialize(conn).map(Some)
            } else {
                Ok(None)
            };
        };
        ensure!(
            marker.as_deref() == Some("1"),
            "invalid credential vault marker"
        );
        if let Some(cached) = &*self
            .cache
            .lock()
            .map_err(|_| anyhow!("vault key cache lock poisoned"))?
            && cached.envelope == encoded
        {
            return Ok(Some(cached.dek.clone()));
        }
        let envelope = serde_json::from_str(&encoded)
            .map_err(|_| anyhow!("invalid credential vault key metadata"))?;
        match envelope {
            Envelope::Passphrase {
                salt,
                nonce,
                wrapped,
            } => {
                ensure!(wrapped.len() == 48, "invalid wrapped vault key length");
                let kek = self.kek(&salt)?;
                let clear = open(&kek, &nonce, WRAP_AAD, &wrapped).map_err(|_| {
                    anyhow!("could not unlock credential vault; check {PASSPHRASE_ENV}")
                })?;
                let mut dek = Zeroizing::new([0; 32]);
                dek.copy_from_slice(&clear);
                self.cache(encoded, dek.clone())?;
                Ok(Some(dek))
            }
            Envelope::Keychain {
                id,
                nonce,
                verifier,
            } => {
                ensure!(verifier.len() == 16, "invalid native vault key verifier");
                let KeySource::Keychain(backend) = &self.source else {
                    bail!("credential vault requires its original native keychain");
                };
                ensure!(
                    backend.is_available(),
                    "native credential vault unavailable"
                );
                let account = account(&id);
                let encoded = backend
                    .get_secret(SERVICE, &account)
                    .map_err(|_| anyhow!("could not read native vault key"))?
                    .context(
                        "native vault key is missing; restore it from backup or run                          `meerai auth reset-vault` to delete the unrecoverable saved credentials",
                    )?;
                let mut dek = Zeroizing::new([0; 32]);
                let len = URL_SAFE_NO_PAD
                    .decode_slice(encoded.as_bytes(), &mut *dek)
                    .map_err(|_| anyhow!("invalid native vault key"))?;
                ensure!(len == dek.len(), "invalid native vault key length");
                open(&dek, &nonce, account.as_bytes(), &verifier)
                    .context("native vault key does not match this database")?;
                Ok(Some(dek))
            }
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Connection, Option<&Dek>) -> Result<T>) -> Result<T> {
        self.database()?.read(|conn| {
            let tx = conn.unchecked_transaction()?;
            let key = self.key(&tx, false)?;
            let result = f(&tx, key.as_ref())?;
            tx.commit()?;
            Ok(result)
        })
    }

    fn delete(&self, provider: &str, kind: Option<CredentialKind>) -> Result<()> {
        self.database()?.immediate(|tx| {
            if let Some(key) = self.key(tx, false)?
                && let Some(current) = read_entry(tx, &key, provider)?
                && kind.is_none_or(|kind| current.kind() == kind)
            {
                tx.execute(
                    "DELETE FROM vault_secrets WHERE provider_id = ?1",
                    [provider],
                )?;
            }
            Ok(())
        })
    }

    fn compare_refresh(
        &self,
        provider: &str,
        expected: &OAuthCredential,
        replacement: &OAuthCredential,
    ) -> Result<bool> {
        self.database()?.immediate(|tx| {
            let Some(key) = self.key(tx, false)? else {
                return Ok(false);
            };
            if !refresh_matches(
                read_entry(tx, &key, provider)?.as_ref(),
                expected,
                replacement,
            ) {
                return Ok(false);
            }
            write_entry(tx, &key, provider, &Credential::OAuth(replacement.clone()))?;
            Ok(true)
        })
    }
}

fn fill(bytes: &mut [u8]) -> Result<()> {
    getrandom::fill(bytes).map_err(|_| anyhow!("could not generate vault randomness"))
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    fill(&mut bytes)?;
    Ok(bytes)
}

fn seal(key: &Dek, nonce: &[u8; 12], aad: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
    ChaCha20Poly1305::new((&**key).into())
        .encrypt(nonce.into(), Payload { msg, aad })
        .map_err(|_| anyhow!("vault encryption failed"))
}

fn open(key: &Dek, nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let mut clear = Zeroizing::new(ciphertext.to_vec());
    ChaCha20Poly1305::new((&**key).into())
        .decrypt_in_place(nonce.into(), aad, &mut *clear)
        .map_err(|_| anyhow!("vault authentication failed"))?;
    Ok(clear)
}

fn account(id: &[u8; 16]) -> String {
    format!("vault-master-key-v1-{}", URL_SAFE_NO_PAD.encode(id))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum Entry<'a> {
    Api(std::borrow::Cow<'a, str>),
    OAuth(std::borrow::Cow<'a, OAuthCredential>),
}

fn db_kind(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::ApiKey => "api",
        CredentialKind::OAuth => "oauth",
    }
}

fn decode_row(key: &Dek, provider: &str, row: &Row<'_>, offset: usize) -> Result<Credential> {
    let kind = row.get_ref(offset)?.as_str()?;
    let nonce: [u8; 12] = row
        .get_ref(offset + 1)?
        .as_blob()?
        .try_into()
        .context("invalid vault nonce length")?;
    let ciphertext = row.get_ref(offset + 2)?.as_blob()?;
    let clear = open(key, &nonce, provider.as_bytes(), ciphertext)?;
    let credential = match serde_json::from_slice(&clear)
        .map_err(|_| anyhow!("invalid encrypted credential payload"))?
    {
        Entry::Api(key) => Credential::ApiKey(key.into_owned()),
        Entry::OAuth(oauth) => Credential::OAuth(oauth.into_owned()),
    };
    ensure!(
        db_kind(credential.kind()) == kind,
        "vault credential kind mismatch"
    );
    Ok(credential)
}

fn read_entry(conn: &Connection, key: &Dek, provider: &str) -> Result<Option<Credential>> {
    let mut stmt =
        conn.prepare("SELECT kind, nonce, ciphertext FROM vault_secrets WHERE provider_id = ?1")?;
    let mut rows = stmt.query([provider])?;
    rows.next()?
        .map(|row| decode_row(key, provider, row, 0))
        .transpose()
}

fn write_entry(
    conn: &Connection,
    key: &Dek,
    provider: &str,
    credential: &Credential,
) -> Result<()> {
    ensure!(!provider.trim().is_empty(), "provider id cannot be empty");
    let entry = match credential {
        Credential::ApiKey(key) => {
            ensure!(!key.trim().is_empty(), "API key cannot be empty");
            Entry::Api(key.trim().into())
        }
        Credential::OAuth(oauth) => Entry::OAuth(std::borrow::Cow::Borrowed(oauth)),
    };
    let clear = Zeroizing::new(
        serde_json::to_vec(&entry).map_err(|_| anyhow!("could not encode credential"))?,
    );
    let nonce = random()?;
    let ciphertext = seal(key, &nonce, provider.as_bytes(), &clear)?;
    conn.execute(
        "INSERT INTO vault_secrets(provider_id, kind, nonce, ciphertext, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(provider_id) DO UPDATE SET kind=excluded.kind, nonce=excluded.nonce, ciphertext=excluded.ciphertext, updated_at_ms=excluded.updated_at_ms",
        params![provider, db_kind(credential.kind()), nonce, ciphertext, crate::oauth::now_ms() as i64],
    )?;
    Ok(())
}

impl SecretStore for VaultSecretStore {
    fn get_key(&self, provider: &str) -> Result<Option<String>> {
        Ok(self
            .read_credential(provider)?
            .and_then(|credential| match credential {
                Credential::ApiKey(key) => Some(key),
                Credential::OAuth(_) => None,
            }))
    }

    fn set_key(&self, provider: &str, key: &str) -> Result<()> {
        self.write_credential(provider, &Credential::ApiKey(key.into()))
    }

    fn delete_key(&self, provider: &str) -> Result<()> {
        self.delete(provider, Some(CredentialKind::ApiKey))
    }

    fn list_providers(&self) -> Result<Vec<String>> {
        Ok(self
            .list_credentials()?
            .into_iter()
            .filter_map(|(id, kind)| (kind == CredentialKind::ApiKey).then_some(id))
            .collect())
    }

    fn read_credential(&self, provider: &str) -> Result<Option<Credential>> {
        self.read(|conn, key| {
            key.map(|key| read_entry(conn, key, provider))
                .transpose()
                .map(Option::flatten)
        })
    }

    fn write_credential(&self, provider: &str, credential: &Credential) -> Result<()> {
        ensure!(!provider.trim().is_empty(), "provider id cannot be empty");
        ensure!(
            !matches!(credential, Credential::ApiKey(key) if key.trim().is_empty()),
            "API key cannot be empty"
        );
        self.database()?.immediate(|tx| {
            let key = self
                .key(tx, true)?
                .context("credential vault is not initialized")?;
            write_entry(tx, &key, provider, credential)
        })
    }

    fn write_credential_unlocked(&self, provider: &str, credential: &Credential) -> Result<()> {
        REFRESH_WRITE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(ctx) = slot.as_mut().filter(|ctx| ctx.provider == provider)
                && let Credential::OAuth(replacement) = credential
            {
                ctx.applied = Some(self.compare_refresh(provider, &ctx.expected, replacement)?);
                Ok(())
            } else {
                self.write_credential(provider, credential)
            }
        })
    }

    fn delete_credential(&self, provider: &str) -> Result<()> {
        self.delete(provider, None)
    }

    fn reset_vault(&self) -> Result<usize> {
        self.reset()
    }

    fn list_credentials(&self) -> Result<Vec<(String, CredentialKind)>> {
        self.read(|conn, key| {
            let Some(key) = key else { return Ok(Vec::new()); };
            let mut stmt = conn.prepare("SELECT provider_id, kind, nonce, ciphertext FROM vault_secrets ORDER BY provider_id")?;
            let mut rows = stmt.query([])?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                let provider = row.get::<_, String>(0)?;
                let kind = decode_row(key, &provider, row, 1)?.kind();
                result.push((provider, kind));
            }
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use meerai_client::secrets::SecretStore;
    use meerai_store::Store;
    use zeroize::Zeroizing;

    #[derive(Clone, Debug, Default)]
    struct MemoryKeychain {
        items: Arc<Mutex<HashMap<(String, String), String>>>,
    }

    impl KeychainBackend for MemoryKeychain {
        fn is_available(&self) -> bool {
            true
        }

        fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
            Ok(self
                .items
                .lock()
                .unwrap()
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }

        fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
            self.items.lock().unwrap().insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn delete_secret(&self, service: &str, account: &str) -> Result<()> {
            self.items
                .lock()
                .unwrap()
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    #[test]
    fn keychain_vault_initializes_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("vault.db")).unwrap();
        let keychain = MemoryKeychain::default();
        let first = VaultSecretStore::with_keychain(store.clone(), Box::new(keychain.clone()));
        first.set_key("openai", "sk-test").unwrap();
        assert_eq!(
            VaultSecretStore::stored_mode(&store).unwrap(),
            VaultMode::Keychain
        );
        assert_eq!(first.get_key("openai").unwrap().as_deref(), Some("sk-test"));

        let second = VaultSecretStore::with_keychain(store, Box::new(keychain));
        assert_eq!(
            second.get_key("openai").unwrap().as_deref(),
            Some("sk-test")
        );
    }

    #[test]
    fn passphrase_vault_reports_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("vault.db")).unwrap();
        let vault = VaultSecretStore::with_passphrase(
            store.clone(),
            Zeroizing::new("test passphrase".to_string()),
        );
        vault.set_key("openai", "sk-test").unwrap();
        assert_eq!(
            VaultSecretStore::stored_mode(&store).unwrap(),
            VaultMode::Passphrase
        );
        assert_eq!(
            VaultSecretStore::stored_mode(&Store::open(dir.path().join("empty.db")).unwrap())
                .unwrap(),
            VaultMode::Uninitialized
        );
    }

    #[test]
    fn reset_deletes_credentials_and_reinitializes_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("vault.db")).unwrap();
        let keychain = MemoryKeychain::default();
        let vault = VaultSecretStore::with_keychain(store.clone(), Box::new(keychain.clone()));
        vault.set_key("openai", "sk-test").unwrap();
        assert_eq!(keychain.items.lock().unwrap().len(), 1);

        let removed = vault.reset().unwrap();
        assert_eq!(removed, 1);
        assert_eq!(
            VaultSecretStore::stored_mode(&store).unwrap(),
            VaultMode::Keychain
        );
        assert!(vault.get_key("openai").unwrap().is_none());
        assert_eq!(keychain.items.lock().unwrap().len(), 1);
    }

    #[test]
    fn reset_reinitializes_a_passphrase_vault() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("vault.db")).unwrap();
        let vault = VaultSecretStore::with_passphrase(
            store.clone(),
            Zeroizing::new("test passphrase".to_string()),
        );
        vault.set_key("openai", "sk-test").unwrap();
        assert_eq!(vault.reset().unwrap(), 1);
        assert!(vault.get_key("openai").unwrap().is_none());
        assert_eq!(
            VaultSecretStore::stored_mode(&store).unwrap(),
            VaultMode::Passphrase
        );
    }

    #[test]
    fn reset_discards_rows_when_the_key_metadata_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("vault.db")).unwrap();
        store
            .immediate(|tx| {
                tx.execute(
                    "INSERT INTO vault_secrets
                     (provider_id, kind, nonce, ciphertext, updated_at_ms)
                     VALUES ('openai', 'api', ?1, ?2, 1)",
                    params![[0u8; 12].to_vec(), [1u8, 2, 3, 4].to_vec()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            VaultSecretStore::stored_mode(&store).unwrap(),
            VaultMode::Uninitialized
        );
        let keychain = MemoryKeychain::default();
        let vault = VaultSecretStore::with_keychain(store, Box::new(keychain));
        assert_eq!(vault.reset().unwrap(), 1);
        assert!(vault.get_key("openai").unwrap().is_none());
    }

    #[test]
    fn keychain_vault_fails_closed_without_its_backend() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("vault.db")).unwrap();
        let keychain = MemoryKeychain::default();
        VaultSecretStore::with_keychain(store.clone(), Box::new(keychain))
            .set_key("openai", "sk-test")
            .unwrap();

        let wrong =
            VaultSecretStore::with_passphrase(store, Zeroizing::new("different".to_string()));
        assert!(wrong.get_key("openai").is_err());
    }
}
