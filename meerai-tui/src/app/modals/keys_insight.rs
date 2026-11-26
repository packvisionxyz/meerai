use super::*;

impl AppState {
    pub(crate) fn keys_insight(
        &mut self,
        modal: ActiveModal,
        key: KeyEvent,
        _has_ctrl: bool,
        _has_alt: bool,
    ) -> bool {
        match modal {
            ActiveModal::Context(mut panel) => {
                if panel.focus_input.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            panel.focus_input = None;
                            panel.error = None;
                        }
                        _ => {
                            let action = match panel.focus_input.as_mut() {
                                Some(input) => handle_text_key(input, key, 56, false, false),
                                None => TextAction::Ignored,
                            };
                            if action == TextAction::Submit {
                                let focus = panel
                                    .focus_input
                                    .take()
                                    .map(|input| input.text)
                                    .unwrap_or_default();
                                let from_turn_id = panel
                                    .breakdown
                                    .as_ref()
                                    .and_then(|breakdown| {
                                        breakdown.turn_ids.get(panel.selected_turn)
                                    })
                                    .map(|turn| turn.to_string());
                                queue_modal_host_action(
                                    self,
                                    meerai_host::HostAction::Summarize {
                                        focus: (!focus.trim().is_empty())
                                            .then(|| focus.trim().to_string()),
                                        from_turn_id,
                                    },
                                );
                            }
                        }
                    }
                    self.modal = Some(ActiveModal::Context(panel));
                    return true;
                }
                if panel.blob_content.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            panel.blob_key = None;
                            panel.blob_content = None;
                            panel.blob_scroll = 0;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            panel.blob_scroll = panel.blob_scroll.saturating_add(1);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            panel.blob_scroll = panel.blob_scroll.saturating_sub(1);
                        }
                        KeyCode::Char('g') => panel.blob_scroll = 0,
                        _ => {}
                    }
                    self.modal = Some(ActiveModal::Context(panel));
                    return true;
                }
                let turn_count = panel
                    .breakdown
                    .as_ref()
                    .map_or(0, |breakdown| breakdown.turn_ids.len());
                let blob_count = panel
                    .breakdown
                    .as_ref()
                    .map_or(0, |breakdown| breakdown.spilled_blobs.len());
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') => {
                        if turn_count > 0 {
                            panel.selected_turn = (panel.selected_turn + 1) % turn_count;
                        }
                        if blob_count > 0 {
                            panel.selected_blob = (panel.selected_blob + 1) % blob_count;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if turn_count > 0 {
                            panel.selected_turn = panel.selected_turn.saturating_sub(1);
                        }
                        if blob_count > 0 {
                            panel.selected_blob = panel.selected_blob.saturating_sub(1);
                        }
                    }
                    KeyCode::Char('g') => {
                        panel.scroll = 0;
                        panel.selected_turn = 0;
                        panel.selected_blob = 0;
                    }
                    KeyCode::Char('r') => {
                        let _ = self.tab().cmd_tx.send(AgentCommand::InspectContext);
                    }
                    KeyCode::Char('p') => {
                        if let Some(turn) = panel
                            .breakdown
                            .as_ref()
                            .and_then(|breakdown| breakdown.turn_ids.get(panel.selected_turn))
                        {
                            let pinned = panel
                                .breakdown
                                .as_ref()
                                .is_some_and(|breakdown| breakdown.pinned_turn_ids.contains(turn));
                            queue_modal_host_action(
                                self,
                                meerai_host::HostAction::PinTurn {
                                    turn_id: turn.to_string(),
                                    pinned: !pinned,
                                },
                            );
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(turn) = panel
                            .breakdown
                            .as_ref()
                            .and_then(|breakdown| breakdown.turn_ids.get(panel.selected_turn))
                        {
                            queue_modal_host_action(
                                self,
                                meerai_host::HostAction::Summarize {
                                    focus: None,
                                    from_turn_id: Some(turn.to_string()),
                                },
                            );
                        }
                    }
                    KeyCode::Char('f') => panel.focus_input = Some(TextInput::default()),
                    KeyCode::Char('b') => {
                        if let Some(key) = panel
                            .breakdown
                            .as_ref()
                            .and_then(|breakdown| breakdown.spilled_blobs.get(panel.selected_blob))
                        {
                            let tab_id = self.tab().tab_id;
                            self.pending_session_actions
                                .push(PendingSessionAction::LoadBlob {
                                    tab_id,
                                    key: key.clone(),
                                });
                        }
                    }
                    KeyCode::Char('t') => {
                        let current = panel
                            .breakdown
                            .as_ref()
                            .map_or(4, |breakdown| breakdown.verbatim_window_turns);
                        let next = match current {
                            0..=3 => 4,
                            4..=7 => 8,
                            8..=15 => 16,
                            _ => 4,
                        };
                        queue_modal_host_action(
                            self,
                            meerai_host::HostAction::SetContextPolicy {
                                verbatim_window_turns: Some(next),
                                reasoning_pruning: None,
                                clear_old_tool_results: None,
                            },
                        );
                    }
                    KeyCode::Char('c') => {
                        let next = !panel
                            .breakdown
                            .as_ref()
                            .is_some_and(|breakdown| breakdown.reasoning_pruning);
                        queue_modal_host_action(
                            self,
                            meerai_host::HostAction::SetContextPolicy {
                                verbatim_window_turns: None,
                                reasoning_pruning: Some(next),
                                clear_old_tool_results: None,
                            },
                        );
                    }
                    KeyCode::Char('C') => {
                        let next = !panel
                            .breakdown
                            .as_ref()
                            .is_some_and(|breakdown| breakdown.clear_old_tool_results);
                        queue_modal_host_action(
                            self,
                            meerai_host::HostAction::SetContextPolicy {
                                verbatim_window_turns: None,
                                reasoning_pruning: None,
                                clear_old_tool_results: Some(next),
                            },
                        );
                    }
                    _ => {}
                }
                self.modal = Some(ActiveModal::Context(panel));
                true
            }
            ActiveModal::Loop { state, mut scroll } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = scroll.saturating_add(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::PageDown => {
                        scroll = scroll.saturating_add(10);
                    }
                    KeyCode::PageUp => {
                        scroll = scroll.saturating_sub(10);
                    }
                    KeyCode::Home | KeyCode::Char('g') => scroll = 0,
                    _ => {}
                }
                if self.modal.is_some() {
                    self.modal = Some(ActiveModal::Loop { state, scroll });
                }
                true
            }
            ActiveModal::Memory {
                entries,
                query,
                mut selected,
                loading,
                error,
            } => {
                let count = entries.len();
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                        selected = (selected + 1) % count;
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                        selected = if selected == 0 {
                            count - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('r') => {
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadMemories {
                                query: (!query.is_empty()).then(|| query.clone()),
                            });
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    KeyCode::Char('d') | KeyCode::Char('p') if count > 0 => {
                        let Some(memory) = entries.get(selected) else {
                            self.modal = Some(ActiveModal::Memory {
                                entries,
                                query,
                                selected,
                                loading,
                                error,
                            });
                            return true;
                        };
                        let action = if key.code == KeyCode::Char('d') {
                            MemoryModalAction::Delete
                        } else {
                            MemoryModalAction::TogglePin
                        };
                        self.pending_session_actions
                            .push(PendingSessionAction::MemoryAction {
                                action,
                                id: memory.id.clone(),
                            });
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading,
                            error,
                        });
                    }
                }
                true
            }
            ActiveModal::Routing {
                routes,
                mut selected,
                loading,
                error,
            } => {
                let count = routes.len();
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                        selected = (selected + 1) % count;
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                        selected = if selected == 0 {
                            count - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('d') if count > 0 => {
                        let task_type = routes[selected].task_type.as_key();
                        self.pending_session_actions
                            .push(PendingSessionAction::RouteAction { task_type });
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    KeyCode::Char('r') => {
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadRoutes);
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading,
                            error,
                        });
                    }
                }
                true
            }
            ActiveModal::Packs {
                packs,
                definitions,
                tab,
                mut selected,
                loading,
                error,
            } => {
                let active_len = match tab {
                    PackTab::Packs => packs.len(),
                    PackTab::Definitions => definitions.agents.len() + definitions.skills.len(),
                };
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Tab | KeyCode::Char('1') | KeyCode::Char('2') => {
                        let tab = match key.code {
                            KeyCode::Char('1') => PackTab::Packs,
                            KeyCode::Char('2') => PackTab::Definitions,
                            _ if tab == PackTab::Packs => PackTab::Definitions,
                            _ => PackTab::Packs,
                        };
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected: 0,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('j') if active_len > 0 => {
                        selected = (selected + 1) % active_len;
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') if active_len > 0 => {
                        selected = if selected == 0 {
                            active_len - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('e') | KeyCode::Char('t') | KeyCode::Char('x')
                        if tab == PackTab::Packs && !packs.is_empty() =>
                    {
                        let Some(pack) = packs.get(selected) else {
                            self.modal = Some(ActiveModal::Packs {
                                packs,
                                definitions,
                                tab,
                                selected,
                                loading,
                                error,
                            });
                            return true;
                        };
                        let action = match key.code {
                            KeyCode::Char('e') => PackModalAction::ToggleEnabled,
                            KeyCode::Char('t') => PackModalAction::ToggleTrusted,
                            _ => PackModalAction::Remove,
                        };
                        self.pending_session_actions
                            .push(PendingSessionAction::PackAction {
                                action,
                                id: pack.id.clone(),
                            });
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    KeyCode::Char('r') => {
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadPacks);
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadDefinitions);
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading,
                            error,
                        });
                    }
                }
                true
            }
            ActiveModal::Sampling {
                mut effort,
                mut temperature,
                mut field,
                error,
            } => {
                if key.code == KeyCode::Esc {
                    self.modal = None;
                    return true;
                }
                if matches!(key.code, KeyCode::Tab | KeyCode::Down) && field == 0 {
                    field = 1;
                    self.modal = Some(ActiveModal::Sampling {
                        effort,
                        temperature,
                        field,
                        error: None,
                    });
                    return true;
                }
                if matches!(key.code, KeyCode::BackTab | KeyCode::Up) && field == 1 {
                    field = 0;
                    self.modal = Some(ActiveModal::Sampling {
                        effort,
                        temperature,
                        field,
                        error: None,
                    });
                    return true;
                }
                if field == 0 {
                    match key.code {
                        KeyCode::Left | KeyCode::Right => {
                            let allowed = self.active_model_effort_levels();
                            let current = effort.unwrap_or_default().clamp_to(&allowed);
                            effort = Some(match key.code {
                                KeyCode::Left => current.previous_in(&allowed),
                                _ => current.next_in(&allowed),
                            });
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error: None,
                            });
                        }
                        KeyCode::Char('l' | 'm' | 'h' | 'x') => {
                            let allowed = self.active_model_effort_levels();
                            let picked = match key.code {
                                KeyCode::Char('l') => EffortLevel::Low,
                                KeyCode::Char('h') => EffortLevel::High,
                                KeyCode::Char('x') => EffortLevel::ExtraHigh,
                                _ => EffortLevel::Medium,
                            };
                            effort = Some(picked.clamp_to(&allowed));
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error: None,
                            });
                        }
                        KeyCode::Enter => match self.apply_sampling(effort, &temperature) {
                            Ok(()) => self.modal = None,
                            Err(message) => {
                                self.modal = Some(ActiveModal::Sampling {
                                    effort,
                                    temperature,
                                    field,
                                    error: Some(message),
                                });
                            }
                        },
                        _ => {
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error,
                            });
                        }
                    }
                } else {
                    let action = match key.code {
                        KeyCode::Left | KeyCode::Right
                            if !key.modifiers.intersects(
                                KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
                            ) =>
                        {
                            let allowed = self.active_model_effort_levels();
                            let current = effort.unwrap_or_default().clamp_to(&allowed);
                            effort = Some(match key.code {
                                KeyCode::Left => current.previous_in(&allowed),
                                _ => current.next_in(&allowed),
                            });
                            TextAction::Edited
                        }
                        KeyCode::Char('l' | 'm' | 'h' | 'x')
                            if !typing_effort_keyword(&temperature) =>
                        {
                            let allowed = self.active_model_effort_levels();
                            let picked = match key.code {
                                KeyCode::Char('l') => EffortLevel::Low,
                                KeyCode::Char('h') => EffortLevel::High,
                                KeyCode::Char('x') => EffortLevel::ExtraHigh,
                                _ => EffortLevel::Medium,
                            };
                            effort = Some(picked.clamp_to(&allowed));
                            TextAction::Edited
                        }
                        _ => handle_text_key(&mut temperature, key, 16, false, false),
                    };
                    temperature
                        .text
                        .retain(|c| c.is_ascii_alphanumeric() || c == '.');
                    temperature.cursor = temperature.cursor.min(temperature.text.len());
                    match action {
                        TextAction::Submit => match self.apply_sampling(effort, &temperature) {
                            Ok(()) => self.modal = None,
                            Err(message) => {
                                self.modal = Some(ActiveModal::Sampling {
                                    effort,
                                    temperature,
                                    field,
                                    error: Some(message),
                                });
                            }
                        },
                        TextAction::Edited | TextAction::Ignored => {
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error: None,
                            });
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
}
