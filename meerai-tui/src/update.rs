use std::time::Duration;

const RELEASES_URL: &str = "https://api.github.com/repos/packvisionxyz/meerai/releases/latest";
const CHECK_TIMEOUT: Duration = Duration::from_secs(3);

pub fn install_hint() -> &'static str {
    "curl -LsSf https://meerai.packvision.xyz/install.sh | sh"
}

pub async fn check_latest() -> Option<String> {
    if std::env::var("MEERAI_NO_UPDATE_CHECK")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
    {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .user_agent(format!("meerai/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let response = client.get(RELEASES_URL).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    let tag = body
        .get("tag_name")?
        .as_str()?
        .trim()
        .trim_start_matches('v');
    if is_newer(tag, env!("CARGO_PKG_VERSION")) {
        Some(tag.to_string())
    } else {
        None
    }
}

fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |value: &str| -> Vec<u64> {
        value
            .split(['.', '-', '+'])
            .map_while(|part| part.parse::<u64>().ok())
            .collect()
    };
    let candidate = parse(candidate);
    let current = parse(current);
    if candidate.is_empty() {
        return false;
    }
    for index in 0..candidate.len().max(current.len()) {
        let left = candidate.get(index).copied().unwrap_or(0);
        let right = current.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_numeric_versions() {
        assert!(is_newer("0.0.2", "0.0.1"));
        assert!(is_newer("0.1.0", "0.0.9"));
        assert!(!is_newer("0.0.1", "0.0.1"));
        assert!(!is_newer("0.0.0", "0.0.1"));
        assert!(!is_newer("", "0.0.1"));
    }
}
