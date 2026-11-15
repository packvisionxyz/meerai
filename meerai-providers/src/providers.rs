use std::sync::{Arc, RwLock};

use anyhow::Result;
use genai::chat::ChatOptions;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, Headers, ModelIden};
pub use meerai_client::providers::*;
pub use meerai_core::env::{MEERAI_USER_AGENT, generate_opencode_session_id};

use meerai_client::secrets::SecretStore;

pub fn build_genai_client(
    runtime: Arc<RwLock<RuntimeProviderState>>,
    secrets: Arc<dyn SecretStore>,
) -> Result<Client> {
    let auth_runtime = runtime.clone();
    let auth_secrets = secrets;
    let target_runtime = runtime;
    let default_session_id = generate_opencode_session_id();

    let client = Client::builder()
        .with_chat_options(ChatOptions::default().with_extra_headers(Headers::from([
            ("x-opencode-session", default_session_id.as_str()),
            ("user-agent", MEERAI_USER_AGENT),
        ])))
        .with_auth_resolver_fn(move |_model_iden: ModelIden| {
            let state = auth_runtime.read().unwrap().clone();
            let provider = state.active_provider_info();
            match resolve_api_key(&provider, auth_secrets.as_ref()) {
                Ok(Some(key)) => Ok(Some(AuthData::from_single(key))),
                Ok(None) if !provider.requires_api_key => Ok(None),
                Ok(None) => Err(genai::resolver::Error::Custom(format!(
                    "No API key configured for provider '{}'. Use /connect to save one.",
                    provider.id
                ))),
                Err(err) => Err(genai::resolver::Error::Custom(format!(
                    "Failed to read API key for '{}': {err:#}",
                    provider.id
                ))),
            }
        })
        .with_service_target_resolver_fn(move |mut target: genai::ServiceTarget| {
            let state = target_runtime.read().unwrap().clone();
            let provider = state.active_provider_info();
            if let Some(url) = state.effective_base_url(&provider) {
                let normalized = if url.ends_with('/') {
                    url
                } else {
                    format!("{url}/")
                };
                target.endpoint = Endpoint::from_owned(normalized);
            }
            Ok(target)
        })
        .build()?;

    Ok(client)
}
