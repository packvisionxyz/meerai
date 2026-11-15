use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MEERAI_USER_AGENT: &str = concat!("meerai/", env!("CARGO_PKG_VERSION"));

pub fn generate_opencode_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let millis = now_millis();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    format!("ses_{millis:012x}{pid:08x}{seq:08x}")
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn resolve_lexical_subpath(root: &Path, raw: &str) -> Option<PathBuf> {
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let mut resolved = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved.starts_with(root).then_some(resolved)
}
