use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::oauth::error_code;

const MODELS_ENDPOINT: &str = "https://api.openai.com/v1/models";
const USAGE_LIMIT_CODE: &str = "subscription_sharing_usage_limit_exceeded";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanModel {
    pub id: String,
    pub display_name: String,
}

pub async fn fetch_plan_models(access_token: &str) -> Result<Vec<PlanModel>> {
    let token = access_token.trim();
    if token.is_empty() {
        anyhow::bail!("ChatGPT plan access token is empty; sign in again");
    }
    let response = reqwest::Client::new()
        .get(MODELS_ENDPOINT)
        .bearer_auth(token)
        .send()
        .await
        .context("failed to reach the OpenAI model catalog")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read the OpenAI model catalog")?;
    if !status.is_success() {
        anyhow::bail!("{}", failure_message(status.as_u16(), &body));
    }
    parse_plan_models(&body)
}

pub fn parse_plan_models(raw: &str) -> Result<Vec<PlanModel>> {
    let root: Value =
        serde_json::from_str(raw).context("failed to parse the OpenAI model catalog")?;
    let models = root
        .get("models")
        .and_then(Value::as_array)
        .context("OpenAI model catalog has no `models` list")?;

    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for model in models {
        let Some(slug) = model
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
        else {
            continue;
        };
        if let Some(visibility) = model.get("visibility").and_then(Value::as_str)
            && visibility != "list"
        {
            continue;
        }
        if !seen.insert(slug.to_string()) {
            continue;
        }
        let display_name = model
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(slug);
        out.push(PlanModel {
            id: slug.to_string(),
            display_name: display_name.to_string(),
        });
    }
    out.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

fn failure_message(status: u16, body: &str) -> String {
    match error_code(body) {
        Some(code) if code == USAGE_LIMIT_CODE => format!("plan usage limit reached ({code})"),
        Some(code) => format!("OpenAI model catalog request failed with status {status} ({code})"),
        None => format!("OpenAI model catalog request failed with status {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_visibility_and_keeps_absent_visibility() {
        let parsed = parse_plan_models(
            r#"{
  "models": [
    { "slug": "gpt-5", "display_name": "GPT-5", "visibility": "list" },
    { "slug": "gpt-5-codex", "display_name": "GPT-5 Codex" },
    { "slug": "gpt-5-internal", "display_name": "GPT-5 Internal", "visibility": "hidden" }
  ]
}"#,
        )
        .unwrap();
        let ids: Vec<&str> = parsed.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-5", "gpt-5-codex"]);
    }

    #[test]
    fn dedupes_by_slug_and_sorts_by_display_name() {
        let parsed = parse_plan_models(
            r#"{
  "models": [
    { "slug": "zeta", "display_name": "Alpha", "visibility": "list" },
    { "slug": "zeta", "display_name": "Alpha duplicate", "visibility": "list" },
    { "slug": "beta", "display_name": "Zulu", "visibility": "list" },
    { "slug": "gamma", "visibility": "list" }
  ]
}"#,
        )
        .unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].id, "zeta");
        assert_eq!(parsed[0].display_name, "Alpha");
        assert_eq!(parsed[1].id, "beta");
        assert_eq!(parsed[1].display_name, "Zulu");
        assert_eq!(parsed[2].id, "gamma");
        assert_eq!(parsed[2].display_name, "gamma");
    }

    #[test]
    fn rejects_a_catalog_without_a_models_list() {
        assert!(parse_plan_models(r#"{"data":[]}"#).is_err());
        assert!(parse_plan_models("not json").is_err());
    }

    #[test]
    fn failure_messages_name_the_provider_code() {
        let limit = failure_message(
            429,
            r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#,
        );
        assert_eq!(
            limit,
            "plan usage limit reached (subscription_sharing_usage_limit_exceeded)"
        );

        let other = failure_message(403, r#"{"error":"subscription_sharing_user_not_eligible"}"#);
        assert!(other.contains("subscription_sharing_user_not_eligible"));

        let anonymous = failure_message(503, "unavailable");
        assert_eq!(
            anonymous,
            "OpenAI model catalog request failed with status 503"
        );
    }
}
