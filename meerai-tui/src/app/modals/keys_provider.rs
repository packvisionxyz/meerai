use super::*;

impl AppState {
    pub(crate) fn keys_provider(
        &mut self,
        modal: ActiveModal,
        key: KeyEvent,
        has_ctrl: bool,
        _has_alt: bool,
    ) -> bool {
        match modal {
            ActiveModal::ProviderSelect {
                mut filter,
                mut selected,
            } => {
                let providers = self.filtered_providers(&filter);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !providers.is_empty() {
                            selected = if selected == 0 {
                                providers.len() - 1
                            } else {
                                selected - 1
                            };
                        }
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !providers.is_empty() {
                            selected = (selected + 1) % providers.len();
                        }
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    _ => match handle_text_key(&mut filter, key, 40, false, false) {
                        TextAction::Submit => {
                            if let Some((provider, status)) = providers.get(selected) {
                                self.open_provider_connect(provider, status);
                            } else {
                                self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                            }
                        }
                        TextAction::Edited => {
                            selected = 0;
                            self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                        }
                    },
                }
                true
            }
            ActiveModal::AuthMethodSelect {
                provider_id,
                provider_name,
                mut selected,
            } => {
                const AUTH_METHODS: usize = 2;
                match key.code {
                    KeyCode::Esc => self.reopen_provider_select_for(&provider_id),
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        selected = if selected == 0 {
                            AUTH_METHODS - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id,
                            provider_name,
                            selected,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        selected = (selected + 1) % AUTH_METHODS;
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id,
                            provider_name,
                            selected,
                        });
                    }
                    KeyCode::Enter => self.choose_auth_method(&provider_id, selected),
                    _ => {
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id,
                            provider_name,
                            selected,
                        });
                    }
                }
                true
            }
            ActiveModal::ProviderConnection {
                provider_id,
                provider_name,
                status,
                mut selected,
            } => {
                let count = ProviderConnectionAction::options(&status).len();
                match key.code {
                    KeyCode::Esc => self.reopen_provider_select_for(&provider_id),
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        selected = if selected == 0 {
                            count - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::ProviderConnection {
                            provider_id,
                            provider_name,
                            status,
                            selected,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        selected = (selected + 1) % count;
                        self.modal = Some(ActiveModal::ProviderConnection {
                            provider_id,
                            provider_name,
                            status,
                            selected,
                        });
                    }
                    KeyCode::Enter => {
                        self.apply_provider_connection_action(
                            &provider_id,
                            &provider_name,
                            &status,
                            selected,
                        );
                    }
                    _ => {
                        self.modal = Some(ActiveModal::ProviderConnection {
                            provider_id,
                            provider_name,
                            status,
                            selected,
                        });
                    }
                }
                true
            }
            ActiveModal::ChatGptLogin {
                provider_id,
                provider_name,
                mut url,
                mut input,
                mut error,
                mut busy,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.cancel_chatgpt_login();
                        return true;
                    }
                    KeyCode::Char('y') if has_ctrl => {
                        if let Some(link) = url.as_deref() {
                            self.copy_text_to_clipboard(link);
                            self.status_notice =
                                Some("Authorize URL copied to clipboard".to_string());
                        }
                    }
                    _ => match handle_text_key(&mut input, key, 64, false, false) {
                        TextAction::Submit => {
                            if error.is_some() {
                                self.login_input = None;
                                self.login_cancel = None;
                                self.pending_login_request = Some(provider_id.clone());
                                input.clear();
                                error = None;
                                busy = true;
                                url = None;
                            } else {
                                let trimmed = input.trim().to_string();
                                if !trimmed.is_empty()
                                    && let Some(tx) = &self.login_input
                                {
                                    let _ = tx.send(trimmed);
                                    input.clear();
                                    error = None;
                                    busy = true;
                                }
                            }
                        }
                        TextAction::Edited => {
                            error = None;
                        }
                        TextAction::Ignored => {}
                    },
                }
                self.modal = Some(ActiveModal::ChatGptLogin {
                    provider_id,
                    provider_name,
                    url,
                    input,
                    error,
                    busy,
                });
                true
            }
            ActiveModal::ApiKeyInput {
                provider_id,
                provider_name,
                mut api_key,
                mut base_url,
                mut editing_base_url,
                mut existing_key_status,
                mut error,
            } => {
                let action = match key.code {
                    KeyCode::Esc => {
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
                        return true;
                    }
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                        editing_base_url = !editing_base_url;
                        TextAction::Ignored
                    }
                    KeyCode::Char('d') if has_ctrl => {
                        match self.secrets.delete_credential(&provider_id) {
                            Ok(()) => {
                                api_key.clear();
                                self.invalidate_key_status();
                                let provider_info = {
                                    let rt = self.runtime.read().unwrap();
                                    rt.catalog.provider(&provider_id).cloned()
                                };
                                if let Some(info) = provider_info {
                                    existing_key_status = self.cached_key_status(&info);
                                } else {
                                    existing_key_status = KeyStatus::Missing;
                                }
                                error = None;
                                self.status_notice =
                                    Some(format!("Removed credential for {provider_id}"));
                            }
                            Err(err) => {
                                error = Some(format!("{err:#}"));
                            }
                        }
                        TextAction::Edited
                    }
                    _ => {
                        let field = if editing_base_url {
                            &mut base_url
                        } else {
                            &mut api_key
                        };
                        handle_text_key(field, key, 64, false, false)
                    }
                };
                if action == TextAction::Submit {
                    let trimmed_key = api_key.trim();
                    if !trimmed_key.is_empty() {
                        if let Err(err) = self.secrets.set_key(&provider_id, trimmed_key) {
                            self.modal = Some(ActiveModal::ApiKeyInput {
                                provider_id,
                                provider_name,
                                api_key,
                                base_url,
                                editing_base_url,
                                existing_key_status,
                                error: Some(format!("{err:#}")),
                            });
                            return true;
                        }
                        self.invalidate_key_status();
                    } else if !existing_key_status.is_configured() {
                        self.modal = Some(ActiveModal::ApiKeyInput {
                            provider_id,
                            provider_name,
                            api_key,
                            base_url,
                            editing_base_url,
                            existing_key_status,
                            error: Some("API key is required".to_string()),
                        });
                        return true;
                    }

                    let clean_url = base_url.trim().to_string();
                    {
                        let mut rt = self.runtime.write().unwrap();
                        if clean_url.is_empty() {
                            rt.base_urls.remove(&provider_id);
                        } else {
                            rt.base_urls.insert(provider_id.clone(), clean_url.clone());
                        }
                    }
                    self.settings.set_provider_base_url(
                        &provider_id,
                        (!clean_url.is_empty()).then_some(clean_url),
                    );

                    self.activate_provider_only(&provider_id);
                    self.status_notice = Some(format!("Connected {provider_id}"));
                    self.open_model_input();
                    return true;
                }
                if action == TextAction::Edited {
                    error = None;
                }
                self.modal = Some(ActiveModal::ApiKeyInput {
                    provider_id,
                    provider_name,
                    api_key,
                    base_url,
                    editing_base_url,
                    existing_key_status,
                    error,
                });
                true
            }
            ActiveModal::ModelInput {
                mut input,
                mut selected,
                mut error,
            } => {
                let matches = self.model_candidates(&input);
                let highlighted = selected.and_then(|index| matches.get(index)).cloned();
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Char('u') if has_ctrl => {
                        input.clear();
                        selected = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Char('d') if has_ctrl => {
                        let target = highlighted.or_else(|| {
                            let typed = input.trim();
                            (!typed.is_empty()).then(|| typed.to_string())
                        });
                        if let Some(model) = target {
                            let provider = self.tab().active_provider.clone();
                            let (target_provider, target_model) = self.resolve_model_target(&model);
                            let provider_id = if target_provider.is_empty() {
                                provider
                            } else {
                                target_provider
                            };
                            self.settings
                                .forget_provider_model(&provider_id, &target_model);
                            self.persist_settings();
                            input.clear();
                            selected = None;
                            error = None;
                            self.status_notice =
                                Some(format!("Removed {provider_id}/{target_model}"));
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !matches.is_empty() {
                            selected = Some(match selected {
                                None | Some(0) => matches.len() - 1,
                                Some(index) => index.min(matches.len() - 1).saturating_sub(1),
                            });
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !matches.is_empty() {
                            selected = Some(match selected {
                                None => 0,
                                Some(index) => (index + 1) % matches.len(),
                            });
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Tab => {
                        if let Some(model) = highlighted.or_else(|| matches.first().cloned()) {
                            input.set_text(model);
                            selected = Some(0);
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    _ => match handle_text_key(&mut input, key, 60, false, false) {
                        TextAction::Submit => {
                            let typed = input.trim().to_string();
                            let target = if let Some(model) = highlighted {
                                Some(model)
                            } else if !typed.is_empty() {
                                Some(typed)
                            } else {
                                let provider = self.tab().active_provider.clone();
                                self.current_or_default_model(&provider)
                            };
                            match target {
                                Some(model) => {
                                    let (target_provider, target_model) =
                                        self.resolve_model_target(&model);
                                    if target_model.is_empty() {
                                        self.modal = Some(ActiveModal::ModelInput {
                                            input,
                                            selected,
                                            error: Some("Model name is required".to_string()),
                                        });
                                    } else {
                                        self.select_provider_and_model(
                                            &target_provider,
                                            &target_model,
                                        );
                                        self.modal = None;
                                    }
                                }
                                None => {
                                    self.modal = Some(ActiveModal::ModelInput {
                                        input,
                                        selected,
                                        error: Some("Model name is required".to_string()),
                                    });
                                }
                            }
                        }
                        TextAction::Edited => {
                            selected = None;
                            error = None;
                            self.modal = Some(ActiveModal::ModelInput {
                                input,
                                selected,
                                error,
                            });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::ModelInput {
                                input,
                                selected,
                                error,
                            });
                        }
                    },
                }
                true
            }
            _ => false,
        }
    }
}
