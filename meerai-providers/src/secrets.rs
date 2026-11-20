use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use meerai_client::secrets::OAuthCredential;
pub use meerai_client::secrets::{Credential, MemorySecretStore, SecretStore, auth_file_path};

mod keychain;
mod vault;

pub use keychain::{KeychainBackend, NativeKeychainBackend};
pub use vault::{VaultMode, VaultSecretStore};

pub fn default_secret_store(data_dir: Option<&Path>) -> Result<Arc<dyn SecretStore>> {
    if data_dir.is_none() && std::env::var_os("MEERAI_DB_PATH").is_none() {
        return Ok(Arc::new(MemorySecretStore::new()));
    }
    Ok(Arc::new(open_vault(data_dir)?))
}

pub fn open_vault(data_dir: Option<&Path>) -> Result<VaultSecretStore> {
    let store = if std::env::var_os("MEERAI_DB_PATH").is_some() {
        meerai_store::Store::open_default()?
    } else {
        let dir = data_dir.context("no data directory; set HOME or XDG_DATA_HOME")?;
        meerai_store::Store::open(dir.join("meerai.db"))?
    };
    let backend = NativeKeychainBackend::new();
    let mode = VaultSecretStore::stored_mode(&store).unwrap_or(VaultMode::Uninitialized);
    let use_keychain = match mode {
        VaultMode::Keychain => true,
        VaultMode::Passphrase => false,
        VaultMode::Uninitialized => backend.is_available(),
    };
    Ok(if use_keychain {
        VaultSecretStore::with_keychain(store, Box::new(backend))
    } else {
        VaultSecretStore::new(store)
    })
}

struct RefreshWrite {
    provider: String,
    expected: OAuthCredential,
    applied: Option<bool>,
}

thread_local! {
    static REFRESH_WRITE: RefCell<Option<RefreshWrite>> = const { RefCell::new(None) };
}

// The shared SecretStore trait has no CAS hook. Pass the expectation only during the
// synchronous write hook; no context or database transaction spans the network await.
pub(crate) fn commit_refresh(
    store: &dyn SecretStore,
    provider: &str,
    expected: OAuthCredential,
    replacement: OAuthCredential,
) -> Result<bool> {
    if !refresh_matches(
        store.read_credential(provider)?.as_ref(),
        &expected,
        &replacement,
    ) {
        return Ok(false);
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            REFRESH_WRITE.with(|slot| *slot.borrow_mut() = None);
        }
    }
    REFRESH_WRITE.with(|slot| {
        let mut slot = slot.borrow_mut();
        anyhow::ensure!(slot.is_none(), "nested credential refresh write");
        *slot = Some(RefreshWrite {
            provider: provider.into(),
            expected,
            applied: None,
        });
        Ok(())
    })?;
    let _reset = Reset;
    store.write_credential_unlocked(provider, &Credential::OAuth(replacement))?;
    Ok(REFRESH_WRITE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|ctx| ctx.applied)
            .unwrap_or(true)
    }))
}

fn refresh_matches(
    latest: Option<&Credential>,
    expected: &OAuthCredential,
    replacement: &OAuthCredential,
) -> bool {
    let Some(Credential::OAuth(latest)) = latest else {
        return false;
    };
    latest == expected
        || (!replacement.needs_relogin && latest.needs_relogin && {
            let mut blocked = expected.clone();
            blocked.needs_relogin = true;
            *latest == blocked
        })
}
