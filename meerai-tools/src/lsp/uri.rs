use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

pub(crate) fn path_to_uri(path: &Path) -> Result<Uri, String> {
    let raw = path.to_string_lossy();
    let mut encoded = String::from("file://");
    let normalized = raw.replace('\\', "/");
    if !normalized.starts_with('/') {
        encoded.push('/');
    }
    for (idx, byte) in normalized.bytes().enumerate() {
        let keep_colon = cfg!(windows) && idx == 1 && byte == b':';
        if keep_colon || is_uri_unreserved(byte) || byte == b'/' {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&encoded).map_err(|err| format!("invalid file URI '{encoded}': {err}"))
}

pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let rest = s
        .strip_prefix("file://localhost")
        .or_else(|| s.strip_prefix("file://"))?;
    let decoded = percent_decode(rest)?;
    #[cfg(windows)]
    let decoded = {
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
            decoded[1..].to_string()
        } else {
            decoded
        }
    };
    Some(PathBuf::from(decoded))
}

fn is_uri_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let val = u8::from_str_radix(hex, 16).ok()?;
            out.push(val);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
