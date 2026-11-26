use anyhow::{Result, ensure};

pub trait KeychainBackend: Send + Sync + std::fmt::Debug {
    fn is_available(&self) -> bool;
    fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>>;
    fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()>;
    fn delete_secret(&self, service: &str, account: &str) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct NativeKeychainBackend {
    _private: (),
}

impl NativeKeychainBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

fn is_truthy_flag(value: &str) -> bool {
    let value = value.trim();
    value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
}

fn disabled() -> bool {
    std::env::var("MEERAI_DISABLE_KEYCHAIN").is_ok_and(|value| is_truthy_flag(&value))
}

impl KeychainBackend for NativeKeychainBackend {
    fn is_available(&self) -> bool {
        !disabled() && platform::is_available()
    }

    fn get_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
        ensure!(!disabled(), "native keychain is disabled");
        platform::get(service, account)
    }

    fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        ensure!(!disabled(), "native keychain is disabled");
        platform::set(service, account, secret)
    }

    fn delete_secret(&self, service: &str, account: &str) -> Result<()> {
        ensure!(!disabled(), "native keychain is disabled");
        platform::delete(service, account)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn decode_secret(bytes: Vec<u8>) -> Result<String> {
    use zeroize::Zeroize;

    String::from_utf8(bytes).map_err(|error| {
        error.into_bytes().zeroize();
        anyhow::anyhow!("native keychain returned a non-UTF-8 secret")
    })
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;
    use std::process::{Command, ExitStatus, Stdio};

    use anyhow::{Context, Result, anyhow};
    use zeroize::{Zeroize, Zeroizing};

    // The CLI is Apple-signed, so a keychain grant names a stable identity that
    // survives every meerai rebuild and upgrade.
    const SECURITY: &str = "/usr/bin/security";
    // errSecItemNotFound, as reported by the security CLI.
    const ITEM_NOT_FOUND: i32 = 44;
    const MAX_DIAGNOSTIC_CHARS: usize = 200;

    pub fn is_available() -> bool {
        Path::new(SECURITY).is_file()
    }

    fn command(action: &str, service: &str, account: &str) -> Command {
        let mut cmd = Command::new(SECURITY);
        cmd.arg(action)
            .args(["-s", service, "-a", account])
            .stdin(Stdio::null());
        cmd
    }

    fn not_found(status: ExitStatus) -> bool {
        status.code() == Some(ITEM_NOT_FOUND)
    }

    fn failure(operation: &str, status: ExitStatus, stderr: &[u8]) -> anyhow::Error {
        let text = String::from_utf8_lossy(stderr);
        let detail: String = text.trim().chars().take(MAX_DIAGNOSTIC_CHARS).collect();
        if detail.is_empty() {
            anyhow!("macOS Keychain {operation} failed ({status})")
        } else {
            anyhow!("macOS Keychain {operation} failed ({status}): {detail}")
        }
    }

    fn hex_encode(bytes: &[u8]) -> Zeroizing<String> {
        use std::fmt::Write;

        let mut out = String::with_capacity(bytes.len().saturating_mul(2));
        for byte in bytes {
            let _ = write!(out, "{byte:02x}");
        }
        Zeroizing::new(out)
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>> {
        let output = command("find-generic-password", service, account)
            .arg("-w")
            .output()
            .context("could not invoke security find-generic-password")?;
        if output.status.success() {
            let mut secret = super::decode_secret(output.stdout)?;
            // Handle the helper's line ending without exposing the secret in diagnostics.
            secret.truncate(secret.trim_end_matches(['\r', '\n']).len());
            return Ok(Some(secret));
        }
        if not_found(output.status) {
            let mut stdout = output.stdout;
            stdout.zeroize();
            return Ok(None);
        }
        let mut stdout = output.stdout;
        stdout.zeroize();
        Err(failure("read", output.status, &output.stderr))
    }

    pub fn set(service: &str, account: &str, secret: &str) -> Result<()> {
        // -X keeps the secret in hex form in argv; the CLI has no stdin input.
        let payload = hex_encode(secret.as_bytes());
        let output = command("add-generic-password", service, account)
            .args(["-U", "-X", payload.as_str()])
            .output()
            .context("could not invoke security add-generic-password")?;
        if !output.status.success() {
            return Err(failure("write", output.status, &output.stderr));
        }
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        let output = command("delete-generic-password", service, account)
            .output()
            .context("could not invoke security delete-generic-password")?;
        if output.status.success() || not_found(output.status) {
            return Ok(());
        }
        Err(failure("delete", output.status, &output.stderr))
    }

    #[cfg(test)]
    mod tests {
        use std::os::unix::process::ExitStatusExt;

        use super::*;

        #[test]
        fn hex_payload_pairs_every_byte() {
            assert_eq!(&*hex_encode(b"m\x00\xff"), "6d00ff");
        }

        #[test]
        fn only_exit_44_is_not_found() {
            assert!(not_found(ExitStatus::from_raw(44 << 8)));
            assert!(!not_found(ExitStatus::from_raw(1 << 8)));
            assert!(!not_found(ExitStatus::from_raw(51 << 8)));
        }

        #[test]
        fn failure_reports_status_and_trimmed_diagnostic() {
            let error = failure("read", ExitStatus::from_raw(51 << 8), b"  denied \n");
            let text = error.to_string();
            assert!(text.contains("51"));
            assert!(text.contains("denied"));
            assert!(!text.contains('\n'));
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, ExitStatus, Stdio};

    use anyhow::{Context, Result, ensure};
    use zeroize::{Zeroize, Zeroizing};

    pub fn is_available() -> bool {
        // A hint only: the service may be locked or unreachable even when both are present.
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|addr| !addr.is_empty())
            && std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| {
                    dir.join("secret-tool")
                        .metadata()
                        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                })
            })
    }

    fn command(action: &str, service: &str, account: &str) -> Command {
        let mut cmd = Command::new("secret-tool");
        cmd.arg(action);
        if action == "store" {
            cmd.arg("--label=meerai");
        }
        cmd.args(["--", "service", service, "account", account])
            .stdin(Stdio::null());
        cmd
    }

    fn quiet_not_found(status: ExitStatus, stderr: &[u8]) -> bool {
        status.code() == Some(1) && stderr.iter().all(u8::is_ascii_whitespace)
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>> {
        let output = command("lookup", service, account)
            .output()
            .context("could not invoke secret-tool lookup")?;
        let stderr = Zeroizing::new(output.stderr);
        if output.status.success() {
            let mut secret = super::decode_secret(output.stdout)?;
            // Handle the helper's line ending without exposing the secret in diagnostics.
            secret.truncate(secret.trim_end_matches(['\r', '\n']).len());
            return Ok(Some(secret));
        }
        let mut stdout = output.stdout;
        stdout.zeroize();
        ensure!(
            quiet_not_found(output.status, &stderr),
            "secret-tool lookup failed ({})",
            output.status
        );
        Ok(None)
    }

    pub fn set(service: &str, account: &str, secret: &str) -> Result<()> {
        // The helper treats values as NUL-terminated text. Vault keys are ASCII base64url.
        ensure!(
            !secret.contains('\0') && !secret.ends_with(['\r', '\n']),
            "invalid secret-tool secret format"
        );
        let mut child = command("store", service, account)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("could not invoke secret-tool store")?;
        // Drop stdin before waiting (the helper reads until EOF), even on a write error.
        let written = child
            .stdin
            .take()
            .context("missing secret-tool input pipe")
            .and_then(|mut stdin| {
                stdin
                    .write_all(secret.as_bytes())
                    .context("could not write to secret-tool")
            });
        let status = child
            .wait()
            .context("could not wait for secret-tool store")?;
        written?;
        ensure!(status.success(), "secret-tool store failed ({status})");
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        let output = command("clear", service, account)
            .stdout(Stdio::null())
            .output()
            .context("could not invoke secret-tool clear")?;
        let stderr = Zeroizing::new(output.stderr);
        ensure!(
            output.status.success() || quiet_not_found(output.status, &stderr),
            "secret-tool clear failed ({})",
            output.status
        );
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::process::ExitStatusExt;

        #[test]
        fn only_quiet_exit_one_is_not_found() {
            let failed = ExitStatus::from_raw(1 << 8);
            assert!(quiet_not_found(failed, b" \n"));
            assert!(!quiet_not_found(failed, b"service unavailable"));
            assert!(!quiet_not_found(ExitStatus::from_raw(9), b""));
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::{io, ptr, slice, str};

    use anyhow::{Result, anyhow, ensure};
    use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
    use windows_sys::Win32::Security::Credentials::{
        CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_MAX_GENERIC_TARGET_NAME_LENGTH,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use zeroize::Zeroizing;

    pub fn is_available() -> bool {
        // Actual access depends on the current user's logon session.
        true
    }

    fn target_name(service: &str, account: &str) -> Result<Vec<u16>> {
        // Encode bytes so case-insensitive target matching cannot alias distinct accounts.
        let mut name = String::from("meerai:");
        for part in [service, account] {
            for byte in part.bytes() {
                use std::fmt::Write;
                write!(name, "{byte:02x}").expect("writing to String cannot fail");
            }
            name.push(':');
        }
        ensure!(
            name.len() <= CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize,
            "Windows credential target is too long"
        );
        Ok(name.encode_utf16().chain([0]).collect())
    }

    struct OwnedCredential(*mut CREDENTIALW);

    impl Drop for OwnedCredential {
        fn drop(&mut self) {
            // SAFETY: CredReadW allocated this buffer; it must be released exactly once.
            unsafe { CredFree(self.0.cast()) };
        }
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>> {
        let name = target_name(service, account)?;
        let mut raw = ptr::null_mut();
        // SAFETY: name is NUL-terminated and raw is a writable out pointer.
        if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                return Ok(None);
            }
            return Err(anyhow!("Windows Credential Manager read failed: {error}"));
        }
        ensure!(
            !raw.is_null(),
            "Windows Credential Manager returned no credential"
        );
        let entry = OwnedCredential(raw);
        // SAFETY: on success CredReadW gives us a valid credential until CredFree.
        let credential = unsafe { &*entry.0 };
        let len = credential.CredentialBlobSize as usize;
        if len == 0 {
            return Ok(Some(String::new()));
        }
        ensure!(
            len <= CRED_MAX_CREDENTIAL_BLOB_SIZE as usize && !credential.CredentialBlob.is_null(),
            "invalid Windows credential blob"
        );
        // SAFETY: the returned buffer contains CredentialBlobSize initialized bytes.
        let bytes = unsafe { slice::from_raw_parts(credential.CredentialBlob, len) };
        let secret = str::from_utf8(bytes)
            .map_err(|_| anyhow!("Windows Credential Manager returned a non-UTF-8 secret"))?;
        Ok(Some(secret.to_owned()))
    }

    pub fn set(service: &str, account: &str, secret: &str) -> Result<()> {
        ensure!(
            secret.len() <= CRED_MAX_CREDENTIAL_BLOB_SIZE as usize,
            "Windows credential blob is too long"
        );
        let mut name = target_name(service, account)?;
        let mut blob = Zeroizing::new(secret.as_bytes().to_vec());
        let entry = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: if blob.is_empty() {
                ptr::null_mut()
            } else {
                blob.as_mut_ptr()
            },
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: entry and both its buffers remain valid for the synchronous call.
        if unsafe { CredWriteW(&entry, 0) } == 0 {
            return Err(anyhow!(
                "Windows Credential Manager write failed: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        let name = target_name(service, account)?;
        // SAFETY: name is NUL-terminated and valid for the duration of the call.
        if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                return Err(anyhow!("Windows Credential Manager delete failed: {error}"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn targets_distinguish_case_and_field_boundaries() {
            assert_ne!(
                target_name("meerai", "Foo").unwrap(),
                target_name("meerai", "foo").unwrap()
            );
            assert_ne!(
                target_name("a:b", "c").unwrap(),
                target_name("a", "b:c").unwrap()
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use anyhow::{Result, bail};

    pub fn is_available() -> bool {
        false
    }

    pub fn get(_: &str, _: &str) -> Result<Option<String>> {
        bail!("native keychain is not supported on this platform")
    }

    pub fn set(_: &str, _: &str, _: &str) -> Result<()> {
        bail!("native keychain is not supported on this platform")
    }

    pub fn delete(_: &str, _: &str) -> Result<()> {
        bail!("native keychain is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_flag_accepts_only_truthy_values() {
        for value in ["1", " TRUE ", "Yes"] {
            assert!(is_truthy_flag(value));
        }
        for value in ["", "0", "false", "off", "true-ish"] {
            assert!(!is_truthy_flag(value));
        }
    }
}
