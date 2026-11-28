use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, anyhow};
use futures::StreamExt;
use meerai_agents::{BoxedChatStream, ChatStreamer};
use meerai_client::providers::{RuntimeProviderState, canonical_provider_id};
use meerai_client::secrets::SecretStore;
use meerai_core::async_trait;
use meerai_core::genai::Client;
use meerai_core::genai::chat::{ChatOptions, ChatRequest};
use meerai_providers::providers::build_genai_client_for;

// One genai client per provider, built on demand. Each client's resolvers close over
// their provider id, so a request can never authenticate as another provider.
pub struct ProviderStreamer {
    runtime: Arc<RwLock<RuntimeProviderState>>,
    secrets: Arc<dyn SecretStore>,
    clients: Mutex<HashMap<String, Client>>,
    default_provider: String,
}

impl ProviderStreamer {
    pub fn new(
        runtime: Arc<RwLock<RuntimeProviderState>>,
        secrets: Arc<dyn SecretStore>,
        default_provider: impl Into<String>,
    ) -> Self {
        Self {
            runtime,
            secrets,
            clients: Mutex::new(HashMap::new()),
            default_provider: default_provider.into(),
        }
    }

    fn client_for(&self, provider: &str) -> Result<Client> {
        if let Some(client) = self
            .clients
            .lock()
            .map_err(|_| anyhow!("streamer client cache poisoned"))?
            .get(provider)
            .cloned()
        {
            return Ok(client);
        }
        let client = build_genai_client_for(
            self.runtime.clone(),
            self.secrets.clone(),
            provider.to_string(),
        )
        .with_context(|| format!("failed to build the client for provider '{provider}'"))?;
        self.clients
            .lock()
            .map_err(|_| anyhow!("streamer client cache poisoned"))?
            .insert(provider.to_string(), client.clone());
        Ok(client)
    }
}

#[async_trait]
impl ChatStreamer for ProviderStreamer {
    async fn stream_chat(
        &self,
        provider: Option<&str>,
        model: &str,
        request: ChatRequest,
        options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        let provider = provider
            .map(canonical_provider_id)
            .unwrap_or_else(|| canonical_provider_id(&self.default_provider));
        let client = self.client_for(&provider)?;
        let response = client
            .exec_chat_stream(model, request, Some(options))
            .await
            .map_err(|err| anyhow::Error::new(err).context("LLM stream request failed"))?;
        let mapped = response
            .stream
            .map(|item| item.map_err(|err| anyhow::Error::new(err).context("LLM stream error")));
        Ok(Box::pin(mapped))
    }
}
