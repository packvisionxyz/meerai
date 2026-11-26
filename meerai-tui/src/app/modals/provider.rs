use super::*;

impl AppState {
    pub(crate) fn reopen_provider_select_for(&mut self, provider_id: &str) {
        let selected = {
            let rt = self.runtime.read().unwrap();
            rt.catalog
                .providers()
                .iter()
                .position(|p| p.id == provider_id)
                .unwrap_or(0)
        };
        self.modal = Some(ActiveModal::ProviderSelect {
            filter: TextInput::default(),
            selected,
        });
    }
    pub(crate) fn activate_provider_at(&mut self, filter: &str, selected: usize) {
        let providers = self.filtered_providers(filter);
        if let Some((provider, status)) = providers.get(selected) {
            self.open_provider_connect(provider, status);
        } else {
            self.modal = Some(ActiveModal::ProviderSelect {
                filter: TextInput::new(filter),
                selected,
            });
        }
    }
    pub(crate) fn open_provider_connect(&mut self, provider: &ProviderInfo, status: &KeyStatus) {
        if status.is_configured() && provider.auth.requires_credential() {
            self.modal = Some(ActiveModal::ProviderConnection {
                provider_id: provider.id.clone(),
                provider_name: provider.display_name.clone(),
                status: status.clone(),
                selected: 0,
            });
            return;
        }
        match provider.auth {
            ProviderAuthKind::None => {
                self.activate_provider_only(&provider.id);
                self.open_model_input();
            }
            ProviderAuthKind::ApiKey => self.open_api_key_input(provider, status),
            ProviderAuthKind::OAuth => self.begin_chatgpt_login(provider),
            ProviderAuthKind::Either => {
                self.modal = Some(ActiveModal::AuthMethodSelect {
                    provider_id: provider.id.clone(),
                    provider_name: provider.display_name.clone(),
                    selected: 0,
                });
            }
        }
    }
    pub(crate) fn apply_provider_connection_action(
        &mut self,
        provider_id: &str,
        provider_name: &str,
        status: &KeyStatus,
        selected: usize,
    ) {
        let Some(action) = ProviderConnectionAction::options(status)
            .get(selected)
            .copied()
        else {
            return;
        };
        match action {
            ProviderConnectionAction::Next => {
                self.activate_provider_only(provider_id);
                self.open_model_input();
            }
            ProviderConnectionAction::Update => {
                let provider = {
                    let rt = self.runtime.read().unwrap();
                    rt.catalog.provider(provider_id).cloned()
                };
                let Some(provider) = provider else {
                    self.modal = None;
                    return;
                };
                match provider.auth {
                    ProviderAuthKind::ApiKey => self.open_api_key_input(&provider, status),
                    ProviderAuthKind::OAuth => self.begin_chatgpt_login(&provider),
                    ProviderAuthKind::Either => {
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id: provider.id.clone(),
                            provider_name: provider.display_name.clone(),
                            selected: 0,
                        });
                    }
                    ProviderAuthKind::None => self.open_model_input(),
                }
            }
            ProviderConnectionAction::Remove => match self.secrets.delete_credential(provider_id) {
                Ok(()) => {
                    self.invalidate_key_status();
                    self.status_notice = Some(format!("Removed credential for {provider_id}"));
                    self.reopen_provider_select_for(provider_id);
                }
                Err(error) => {
                    self.status_notice = Some(format!("Failed to remove credential: {error:#}"));
                    self.modal = Some(ActiveModal::ProviderConnection {
                        provider_id: provider_id.to_string(),
                        provider_name: provider_name.to_string(),
                        status: status.clone(),
                        selected,
                    });
                }
            },
        }
    }
    pub(crate) fn open_api_key_input(&mut self, provider: &ProviderInfo, status: &KeyStatus) {
        let saved_url = {
            let rt = self.runtime.read().unwrap();
            rt.base_urls.get(&provider.id).cloned().unwrap_or_default()
        };
        self.modal = Some(ActiveModal::ApiKeyInput {
            provider_id: provider.id.clone(),
            provider_name: provider.display_name.clone(),
            api_key: TextInput::default(),
            base_url: TextInput::new(saved_url),
            editing_base_url: false,
            existing_key_status: status.clone(),
            error: None,
        });
    }
    pub(crate) fn choose_auth_method(&mut self, provider_id: &str, selected: usize) {
        if selected == 0 {
            let provider = {
                let rt = self.runtime.read().unwrap();
                rt.catalog.provider(provider_id).cloned()
            };
            let Some(provider) = provider else {
                self.modal = None;
                return;
            };
            self.begin_chatgpt_login(&provider);
            return;
        }
        let provider = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.provider(provider_id).cloned()
        };
        let Some(provider) = provider else {
            self.modal = None;
            return;
        };
        let status = self.cached_key_status(&provider);
        self.open_api_key_input(&provider, &status);
    }
    pub(crate) fn begin_chatgpt_login(&mut self, provider: &ProviderInfo) {
        self.activate_provider_only(&provider.id);
        self.modal = Some(ActiveModal::ChatGptLogin {
            provider_id: provider.id.clone(),
            provider_name: provider.display_name.clone(),
            url: None,
            input: TextInput::default(),
            error: None,
            busy: true,
        });
        self.pending_login_request = Some(provider.id.clone());
    }
    pub fn apply_login_started(&mut self, url: String) {
        if let Some(ActiveModal::ChatGptLogin {
            url: slot, busy, ..
        }) = &mut self.modal
        {
            *slot = Some(url);
            *busy = false;
        }
    }
    pub fn apply_login_failed(&mut self, message: String) {
        self.login_input = None;
        self.login_cancel = None;
        self.status_notice = Some(message.clone());
        if let Some(ActiveModal::ChatGptLogin { error, busy, .. }) = &mut self.modal {
            *error = Some(message);
            *busy = false;
        }
    }
    pub fn apply_login_succeeded(&mut self, provider_id: &str, account: Option<String>) {
        self.login_input = None;
        self.login_cancel = None;
        self.modal = None;
        self.invalidate_key_status();
        self.reload_settings_from_disk();
        self.activate_provider_only(provider_id);
        self.settings.active_provider = Some(provider_id.to_string());
        self.open_model_input();
        self.status_notice = Some(match account {
            Some(account) => format!("Signed in with ChatGPT as {account}"),
            None => "Signed in with ChatGPT".to_string(),
        });
    }
    pub(crate) fn cancel_chatgpt_login(&mut self) {
        self.pending_login_request = None;
        self.login_input = None;
        self.login_cancel = None;
        self.modal = None;
        self.status_notice = Some("ChatGPT sign-in cancelled".to_string());
    }
    pub(crate) fn activate_provider_only(&mut self, provider_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let current_model = self.tab().model.clone();
        let switching_provider = self.tab().active_provider != canonical;
        self.tab_mut().active_provider = canonical.clone();
        self.settings.active_provider = Some(canonical.clone());

        let (spec, next_model) = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();

            let candidate = self
                .settings
                .provider_model(&canonical)
                .map(str::to_string)
                .or_else(|| rt.catalog.default_model_for_provider(&canonical))
                .or_else(|| {
                    (!switching_provider && !current_model.trim().is_empty())
                        .then_some(current_model)
                })
                .unwrap_or_default();

            let spec = (!candidate.is_empty())
                .then(|| rt.catalog.format_genai_spec(&canonical, &candidate));
            (spec, candidate)
        };

        let context_window_tokens = self.model_context_window(&canonical, &next_model);
        let effort_levels = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.effort_levels_for(&canonical, &next_model)
        };

        self.tab_mut().model = next_model.clone();
        self.settings.active_model = (!next_model.is_empty()).then_some(next_model);
        self.sync_completion_models();
        self.persist_settings();
        if let Some(spec) = spec {
            let _ = self.tab().cmd_tx.send(AgentCommand::SetModel {
                model: spec,
                provider: Some(canonical.clone()),
                context_window_tokens,
                effort_levels,
            });
        }
        self.send_plan_usage(&canonical);
    }
    pub(crate) fn select_provider_and_model(&mut self, provider_id: &str, model_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let clean_model = model_id.trim().to_string();
        self.tab_mut().active_provider = canonical.clone();
        self.tab_mut().model = clean_model.clone();
        self.settings
            .record_provider_model(&canonical, &clean_model);
        self.settings.active_model = Some(clean_model.clone());

        let spec = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();
            rt.catalog.format_genai_spec(&canonical, &clean_model)
        };

        self.sync_completion_models();
        self.persist_settings();
        let context_window_tokens = self.model_context_window(&canonical, &clean_model);
        let effort_levels = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.effort_levels_for(&canonical, &clean_model)
        };
        let _ = self.tab().cmd_tx.send(AgentCommand::SetModel {
            model: spec.clone(),
            provider: Some(canonical.clone()),
            context_window_tokens,
            effort_levels,
        });
        self.send_plan_usage(&canonical);
        let session_id = {
            let tab = self.tab_mut();
            tab.transcript.push(TranscriptEntry::System {
                text: format!("Active model: {canonical}/{clean_model}"),
            });
            tab.session_id.clone()
        };
        if let Some(reg) = &self.registry {
            reg.note_tui_model(&session_id, &canonical, &spec);
        }
    }
    pub(crate) fn open_provider_select(&mut self) {
        let active_provider = self.tab().active_provider.clone();
        let selected = {
            let rt = self.runtime.read().unwrap();
            rt.catalog
                .providers()
                .iter()
                .position(|p| p.id == active_provider)
                .unwrap_or(0)
        };
        self.modal = Some(ActiveModal::ProviderSelect {
            filter: TextInput::default(),
            selected,
        });
    }
}
