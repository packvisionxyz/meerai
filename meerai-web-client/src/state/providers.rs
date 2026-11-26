use super::completion::*;
use super::forms::*;
use super::misc::*;
use super::settings::*;
use super::types::*;
use super::views::*;
use super::*;

pub fn apply_providers_response(state: &mut AppState, providers: ProvidersResponse) {
    state.providers = Some(providers);
    update_completion(state);
}

pub fn forget_model(state: &mut AppState, provider: &str, model: &str) {
    if let Some(providers) = state.providers.as_mut()
        && let Some(list) = providers.recent_models.get_mut(provider)
    {
        list.retain(|m| m != model);
        if list.is_empty() {
            providers.recent_models.remove(provider);
        }
    }
    if let Some(t) = crate::transport::current() {
        t.update_settings(UpdateSettingsRequest {
            reset_theme: false,
            theme: None,
            context: None,
            session: None,
            sandbox: None,
            forget_model: Some(meerai_web_protocol::ForgetModelDto {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }),
        });
    }
    set_status_notice(state, format!("Removed {provider}/{model}"));
}

pub fn open_provider_select(state: &mut AppState) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    state.modal = Some(ActiveModal::ProviderSelect {
        filter: TextBuffer::new(""),
        selected: 0,
    });
}

pub fn open_model_input(state: &mut AppState) {
    open_model_input_for(state, None, ProviderModalReturn::Close);
}

pub fn open_model_input_for(
    state: &mut AppState,
    provider_id: Option<String>,
    return_to: ProviderModalReturn,
) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    state.modal = Some(ActiveModal::ModelInput {
        filter: TextBuffer::new(""),
        provider_id,
        selected: 0,
        return_to,
    });
}

pub fn return_from_provider_modal(state: &mut AppState, return_to: ProviderModalReturn) {
    match return_to {
        ProviderModalReturn::Close => state.modal = None,
        ProviderModalReturn::ProviderSelect => open_provider_select(state),
        ProviderModalReturn::SettingsConnections => {
            open_settings(state, Some(SettingsSection::Connections));
        }
    }
}

pub fn activate_provider_from_modal(
    state: &mut AppState,
    provider: ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    if provider.configured {
        state.modal = Some(ActiveModal::ProviderConnection {
            provider_id: provider.id,
            provider_name: provider.display_name,
            auth_kind: provider.auth_kind,
            status_badge: provider.status_badge,
            configured: provider.configured,
            base_url: provider.base_url,
            env_vars: provider.env_vars,
            default_model: provider.default_model,
            return_to,
            selected: 0,
        });
        return;
    }
    match provider.auth_kind {
        meerai_web_protocol::AuthKindDto::None => {
            open_model_input_for(state, Some(provider.id), return_to);
        }
        meerai_web_protocol::AuthKindDto::ApiKey => {
            open_api_key_modal(state, &provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::OAuth => {
            start_oauth_modal(state, &provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::Either => {
            state.modal = Some(ActiveModal::AuthMethodSelect {
                provider_id: provider.id,
                provider_name: provider.display_name,
                return_to,
                selected: 0,
            });
        }
    }
}

pub fn open_provider_model_select(
    state: &mut AppState,
    provider_id: String,
    return_to: ProviderModalReturn,
) {
    open_model_input_for(state, Some(provider_id), return_to);
}

pub fn open_provider_credential_update(
    state: &mut AppState,
    provider: &ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    match provider.auth_kind {
        meerai_web_protocol::AuthKindDto::None | meerai_web_protocol::AuthKindDto::ApiKey => {
            open_api_key_modal(state, provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::OAuth => {
            start_oauth_modal(state, provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::Either => {
            state.modal = Some(ActiveModal::AuthMethodSelect {
                provider_id: provider.id.clone(),
                provider_name: provider.display_name.clone(),
                return_to,
                selected: 0,
            });
        }
    }
}

pub fn is_removable_credential_badge(badge: &str) -> bool {
    matches!(badge, "saved" | "plan")
}

pub fn provider_for_model<'a>(
    providers: Option<&'a ProvidersResponse>,
    provider_id: &str,
) -> Option<&'a ProviderItemDto> {
    providers.and_then(|resp| resp.providers.iter().find(|p| p.id == provider_id))
}

pub fn is_recent_model(
    providers: Option<&ProvidersResponse>,
    provider_id: &str,
    model_id: &str,
) -> bool {
    providers
        .and_then(|resp| resp.recent_models.get(provider_id))
        .is_some_and(|list| list.iter().any(|m| m == model_id))
}

pub fn filtered_providers(
    providers: Option<&ProvidersResponse>,
    filter: &str,
) -> Vec<ProviderItemDto> {
    filtered_providers_sorted(providers, None, filter)
}

pub fn filtered_providers_sorted(
    providers: Option<&ProvidersResponse>,
    active_provider: Option<&str>,
    filter: &str,
) -> Vec<ProviderItemDto> {
    let Some(resp) = providers else {
        return Vec::new();
    };
    let q = filter.trim().to_ascii_lowercase();
    let mut items: Vec<ProviderItemDto> = resp
        .providers
        .iter()
        .filter(|p| {
            q.is_empty()
                || p.id.to_ascii_lowercase().contains(&q)
                || p.display_name.to_ascii_lowercase().contains(&q)
                || p.adapter.to_ascii_lowercase().contains(&q)
                || p.status_badge.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    items.sort_by_key(|p| {
        let is_active = active_provider.is_some_and(|a| a == p.id);
        let is_default = resp.default_provider.as_deref() == Some(p.id.as_str());
        (!is_active, !p.configured, !is_default)
    });
    items
}

pub fn filtered_models(
    providers: Option<&ProvidersResponse>,
    provider_filter: Option<&str>,
    filter: &str,
) -> Vec<ModelItemDto> {
    filtered_models_sorted(providers, provider_filter, None, filter)
}

pub fn complete_model_filter(state: &mut AppState, model_id: &str) {
    if let Some(ActiveModal::ModelInput {
        filter, selected, ..
    }) = &mut state.modal
    {
        filter.replace_text(model_id.to_string());
        *selected = 0;
    }
}

pub fn filtered_models_sorted(
    providers: Option<&ProvidersResponse>,
    provider_filter: Option<&str>,
    active_provider: Option<&str>,
    filter: &str,
) -> Vec<ModelItemDto> {
    let Some(resp) = providers else {
        return Vec::new();
    };
    let mut combined = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for m in &resp.models {
        if seen.insert((m.provider.clone(), m.id.clone())) {
            combined.push(m.clone());
        }
    }
    for (provider, recent) in &resp.recent_models {
        for model_id in recent {
            if seen.insert((provider.clone(), model_id.clone())) {
                combined.push(ModelItemDto {
                    id: model_id.clone(),
                    provider: provider.clone(),
                    display_name: model_id.clone(),
                    context_window_tokens: None,
                    max_output_tokens: None,
                    supports_reasoning: true,
                    supports_vision: false,
                    supports_tools: true,
                    is_custom: true,
                });
            }
        }
    }
    for provider in &resp.providers {
        if let Some(model_id) = &provider.default_model
            && !model_id.trim().is_empty()
            && seen.insert((provider.id.clone(), model_id.clone()))
        {
            combined.push(ModelItemDto {
                id: model_id.clone(),
                provider: provider.id.clone(),
                display_name: model_id.clone(),
                context_window_tokens: None,
                max_output_tokens: None,
                supports_reasoning: true,
                supports_vision: false,
                supports_tools: true,
                is_custom: false,
            });
        }
    }

    let q = filter.trim().to_ascii_lowercase();
    let mut items: Vec<ModelItemDto> = combined
        .into_iter()
        .filter(|m| {
            provider_filter.is_none_or(|p| m.provider == p || q.contains('/'))
                && (q.is_empty()
                    || m.id.to_ascii_lowercase().contains(&q)
                    || m.display_name.to_ascii_lowercase().contains(&q)
                    || format!("{}/{}", m.provider, m.id)
                        .to_ascii_lowercase()
                        .contains(&q))
        })
        .collect();

    let configured_map: HashMap<&str, bool> = resp
        .providers
        .iter()
        .map(|p| (p.id.as_str(), p.configured))
        .collect();
    let default_map: HashMap<&str, &str> = resp
        .providers
        .iter()
        .filter_map(|p| p.default_model.as_deref().map(|d| (p.id.as_str(), d)))
        .collect();

    items.sort_by_key(|m| {
        let scope_tier = match provider_filter {
            Some(p) => {
                if m.provider == p {
                    0u8
                } else {
                    1
                }
            }
            None => {
                let is_active = active_provider.is_some_and(|a| a == m.provider);
                let is_configured = configured_map
                    .get(m.provider.as_str())
                    .copied()
                    .unwrap_or(false);
                if is_active {
                    0
                } else if is_configured {
                    1
                } else {
                    2
                }
            }
        };
        let recent_rank = resp
            .recent_models
            .get(&m.provider)
            .and_then(|list| list.iter().position(|r| r == &m.id))
            .unwrap_or(usize::MAX);
        let is_default = default_map
            .get(m.provider.as_str())
            .is_some_and(|d| *d == m.id);
        let query_rank = if q.is_empty() || m.id.to_ascii_lowercase().starts_with(&q) {
            0u8
        } else {
            1
        };
        (query_rank, scope_tier, recent_rank, !is_default)
    });
    items
}

pub fn resolve_model_input_target(
    state: &AppState,
    provider_scope: Option<&str>,
    raw: &str,
) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let fallback = provider_scope
        .map(str::to_owned)
        .or_else(|| state.active_session().map(|s| s.provider.clone()))
        .or_else(|| {
            state
                .providers
                .as_ref()
                .and_then(|p| p.default_provider.clone())
        })
        .unwrap_or_else(|| "openai".to_owned());
    Some(parse_provider_model_arg(trimmed, &fallback))
}

pub fn custom_model_candidate(
    state: &AppState,
    provider_scope: Option<&str>,
    filter: &str,
    models: &[ModelItemDto],
) -> Option<(String, String)> {
    let (provider, model) = resolve_model_input_target(state, provider_scope, filter)?;
    let exact = models
        .iter()
        .any(|m| m.provider == provider && m.id == model);
    (!exact).then_some((provider, model))
}

pub fn parse_provider_model_arg(raw: &str, fallback_provider: &str) -> (String, String) {
    let trimmed = raw.trim();
    if let Some((p, m)) = trimmed.split_once('/') {
        let p = p.trim();
        let m = m.trim();
        if !p.is_empty() && !m.is_empty() {
            return (p.to_owned(), m.to_owned());
        }
    }
    (fallback_provider.to_owned(), trimmed.to_owned())
}
