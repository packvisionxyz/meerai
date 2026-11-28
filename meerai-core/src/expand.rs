use std::sync::OnceLock;

use regex::Regex;

pub fn expand_env_reference(value: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    static ENV_BRACE_RE: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
    static DOLLAR_BRACE_RE: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();

    let (Ok(env_re), Ok(dollar_re)) = (
        ENV_BRACE_RE.get_or_init(|| Regex::new(r"\{env:([A-Za-z_][A-Za-z0-9_]*)\}")),
        DOLLAR_BRACE_RE.get_or_init(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:\|([^}]*))?\}")),
    ) else {
        return value.to_string();
    };

    let stage1 = env_re.replace_all(value, |caps: &regex::Captures| {
        lookup(&caps[1]).unwrap_or_default()
    });
    let stage2 = dollar_re.replace_all(&stage1, |caps: &regex::Captures| {
        lookup(&caps[1]).unwrap_or_else(|| caps.get(2).map_or("", |m| m.as_str()).to_string())
    });
    stage2.to_string()
}

pub fn expand_env_reference_trimmed(
    value: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let expanded = expand_env_reference(value, lookup);
    let clean = expanded.trim();
    if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    }
}

pub fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "OPENAI_API_KEY" => Some("sk-live".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    #[test]
    fn all_three_reference_forms_expand() {
        assert_eq!(
            expand_env_reference("{env:OPENAI_API_KEY}", &lookup),
            "sk-live"
        );
        assert_eq!(
            expand_env_reference("${OPENAI_API_KEY}", &lookup),
            "sk-live"
        );
        assert_eq!(
            expand_env_reference("${MISSING|fallback}", &lookup),
            "fallback"
        );
    }

    #[test]
    fn unknown_references_become_empty() {
        assert_eq!(expand_env_reference("{env:MISSING}", &lookup), "");
        assert_eq!(expand_env_reference("${MISSING}", &lookup), "");
        assert_eq!(expand_env_reference("${EMPTY}", &lookup), "");
    }

    #[test]
    fn literals_and_embedded_references_pass_through() {
        assert_eq!(expand_env_reference("info", &lookup), "info");
        assert_eq!(
            expand_env_reference("Bearer ${OPENAI_API_KEY}", &lookup),
            "Bearer sk-live"
        );
        assert_eq!(expand_env_reference("", &lookup), "");
    }

    #[test]
    fn malformed_references_are_left_alone() {
        assert_eq!(expand_env_reference("${1BAD}", &lookup), "${1BAD}");
        assert_eq!(expand_env_reference("{env:}", &lookup), "{env:}");
        assert_eq!(
            expand_env_reference("$OPENAI_API_KEY", &lookup),
            "$OPENAI_API_KEY"
        );
    }

    #[test]
    fn trimmed_form_reports_absence() {
        assert_eq!(
            expand_env_reference_trimmed("  {env:MISSING}  ", &lookup),
            None
        );
        assert_eq!(
            expand_env_reference_trimmed(" {env:OPENAI_API_KEY} ", &lookup).as_deref(),
            Some("sk-live")
        );
    }
}
