use meerai_agents::AgentCommand;
use meerai_client::providers::format_auth_list;
use meerai_client::secrets::auth_file_path;
use meerai_common::definitions::{AgentDefinition, SkillDefinition, validate_agent_model};
use meerai_common::settings::{Temperature, default_data_dir};
use meerai_host::parse_host_action;

use super::*;
use crate::search::SearchMode;

impl AppState {
    pub(super) fn apply_mentions(&mut self, plan: &MentionPlan) {
        for index in &plan.skill_indices {
            let Some(skill) = self.skills.get(*index).cloned() else {
                continue;
            };
            self.send_skill(&skill);
        }
        for index in &plan.agent_indices {
            let Some(agent) = self.agents.get(*index).cloned() else {
                continue;
            };
            let model = self.agent_model_spec(&agent);
            self.send_agent(&agent, model);
        }
    }

    pub(super) fn acknowledge_mentions(&mut self, plan: &MentionPlan) {
        let mut messages = Vec::new();
        for index in &plan.skill_indices {
            if let Some(skill) = self.skills.get(*index) {
                messages.push(format!(
                    "Applied skill {} ({}).",
                    skill.id,
                    skill.source.label()
                ));
            }
        }
        for index in &plan.agent_indices {
            if let Some(agent) = self.agents.get(*index) {
                messages.push(format!(
                    "Applied agent {} ({}).",
                    agent.id,
                    agent.source.label()
                ));
            }
        }
        for message in messages {
            self.tab_mut()
                .transcript
                .push(TranscriptEntry::System { text: message });
        }
    }

    fn open_sampling(&mut self, field: usize) {
        let allowed = self.active_model_effort_levels();
        let effort = self
            .context_effort
            .or_else(|| {
                self.settings
                    .context
                    .as_ref()
                    .and_then(|context| context.effort)
            })
            .map(|effort| effort.clamp_to(&allowed));
        let temperature = self
            .settings
            .context
            .as_ref()
            .and_then(|context| context.temperature)
            .map(|temperature| format!("{:.2}", temperature.value()))
            .unwrap_or_default();
        self.modal = Some(ActiveModal::Sampling {
            effort,
            temperature: temperature.into(),
            field,
            error: None,
        });
    }

    pub(super) fn apply_sampling(
        &mut self,
        effort: Option<EffortLevel>,
        temperature: &str,
    ) -> std::result::Result<(), String> {
        let allowed = self.active_model_effort_levels();
        let text = temperature.trim();
        let override_temperature = if text.is_empty() || text.eq_ignore_ascii_case("default") {
            None
        } else {
            let value = text
                .parse::<f32>()
                .map_err(|_| "Temperature must be a number from 0.0 to 2.0.".to_string())?;
            Some(
                Temperature::new(value)
                    .effective()
                    .ok_or_else(|| "Temperature must be between 0.0 and 2.0.".to_string())?,
            )
        };
        let effort = effort.unwrap_or_default().clamp_to(&allowed);
        let effective = override_temperature.unwrap_or_else(|| effort.default_temperature());
        let context = self.settings.context.get_or_insert_with(Default::default);
        context.effort = Some(effort);
        context.temperature = override_temperature.map(Temperature::new);
        self.context_effort = Some(effort);
        self.context_temperature = Some(effective);
        self.persist_settings();
        self.pending_session_actions
            .push(PendingSessionAction::SyncContext);
        self.status_notice = Some(format!(
            "Effort {} · temp {:.2}{}",
            effort.label(),
            effective,
            if override_temperature.is_some() {
                " (override)"
            } else {
                " (tier default)"
            }
        ));
        Ok(())
    }

    pub(super) fn set_effort_from_arg(&mut self, arg: &str) {
        let value = arg.trim().to_lowercase();
        let allowed = self.active_model_effort_levels();
        let next = if value.is_empty() {
            self.context_effort
                .unwrap_or_default()
                .clamp_to(&allowed)
                .next_in(&allowed)
        } else if let Some(next) = EffortLevel::parse(&value) {
            next
        } else {
            self.status_notice = Some(format!(
                "Unknown effort '{value}' · use {}",
                allowed
                    .iter()
                    .map(|level| level.keyword())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return;
        };
        let next = next.clamp_to(&allowed);
        self.context_effort = Some(next);
        self.context_temperature = Some(next.default_temperature());
        let context = self.settings.context.get_or_insert_with(Default::default);
        context.effort = Some(next);
        context.temperature = None;
        self.persist_settings();
        self.pending_session_actions
            .push(PendingSessionAction::SyncContext);
        self.status_notice = Some(format!(
            "Effort {} · temp {:.2}",
            next.label(),
            next.default_temperature()
        ));
    }

    pub(super) fn handle_slash_command(&mut self, command: &str) {
        let mut parts = command.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or_default();
        let arg = parts.next().map(str::trim).unwrap_or_default();

        match cmd {
            "/help" => {
                let tab = self.tab_mut();
                tab.auto_follow = true;
                tab.transcript.push(TranscriptEntry::System {
                    text: format_help_text(),
                });
            }
            "/sessions" => {
                if arg.is_empty() {
                    self.open_session_select();
                } else {
                    self.switch_to_session_query(arg);
                }
            }
            "/new" => {
                self.request_new_tab();
            }
            "/close" => {
                self.close_active_tab();
            }
            "/acp" => {
                if self.session_opener.is_some() {
                    self.status_notice = Some("Opening ACP session (agy)...".to_string());
                    self.pending_session_actions
                        .push(PendingSessionAction::CreateAcpTab {
                            preset: "agy".to_string(),
                        });
                } else {
                    self.tab_mut().transcript.push(TranscriptEntry::Error {
                        text: "ACP sessions are not available in this context.".to_string(),
                        expanded: false,
                    });
                }
            }
            "/auth" => {
                let data_dir = default_data_dir();
                let auth_path = data_dir.as_deref().map(auth_file_path);
                let text = {
                    let runtime = self.runtime.read().unwrap();
                    format_auth_list(
                        &runtime.catalog,
                        self.secrets.as_ref(),
                        auth_path.as_deref(),
                        self.config_dir.as_deref(),
                    )
                };
                let tab = self.tab_mut();
                tab.auto_follow = true;
                tab.transcript.push(TranscriptEntry::System { text });
            }
            "/sidebar" => {
                self.toggle_sidebar();
            }
            "/diagnose" => {
                self.toggle_diagnose_panel();
            }
            "/search" | "/tv" | "/find" => {
                self.open_search_modal(SearchMode::Files, arg);
            }
            "/grep" => {
                self.open_search_modal(SearchMode::Content, arg);
            }
            "/diff" => {
                if self.file_changes.is_empty() {
                    self.status_notice = Some("Working tree is clean".to_string());
                } else {
                    let idx = if arg.is_empty() {
                        0
                    } else {
                        self.file_changes
                            .iter()
                            .position(|c| c.path.contains(arg))
                            .unwrap_or(0)
                    };
                    self.open_diff_modal(idx);
                }
            }
            "/connect" | "/provider" => {
                self.open_provider_select();
            }
            "/models" => {
                self.open_model_input();
            }
            "/agent" => {
                self.apply_agent(arg);
            }
            "/skill" => {
                self.apply_skill(arg);
            }
            "/agents" => {
                self.open_mention_picker("agent");
            }
            "/skills" => {
                self.modal = Some(ActiveModal::Packs {
                    packs: Vec::new(),
                    definitions: meerai_host::HostDefinitionsDiagnostics::default(),
                    tab: PackTab::Definitions,
                    selected: 0,
                    loading: true,
                    error: None,
                });
                self.pending_session_actions
                    .push(PendingSessionAction::LoadDefinitions);
                self.pending_session_actions
                    .push(PendingSessionAction::LoadPacks);
            }
            "/model" => {
                self.tab_mut().auto_follow = true;
                if arg.is_empty() {
                    self.open_model_input();
                } else {
                    let (target_provider, target_model) = self.resolve_model_target(arg);
                    self.select_provider_and_model(&target_provider, &target_model);
                }
            }
            "/effort" => {
                if arg.is_empty() {
                    self.open_sampling(0);
                } else {
                    self.set_effort_from_arg(arg);
                }
            }
            "/sampling" => self.open_sampling(0),
            "/temperature" => self.open_sampling(1),
            "/clear" => {
                let tab = self.tab_mut();
                tab.transcript.clear();
                tab.pending_assistant_idx = None;
                tab.assistant_items.clear();
                tab.tool_items.clear();
                tab.subagent_items.clear();
                tab.turn_activity.clear();
                tab.scroll_offset = 0;
                tab.total_lines = 0;
                tab.auto_follow = true;
            }
            "/reset" => {
                let session_id = {
                    let tab = self.tab_mut();
                    tab.transcript.clear();
                    tab.pending_assistant_idx = None;
                    tab.assistant_items.clear();
                    tab.tool_items.clear();
                    tab.subagent_items.clear();
                    tab.turn_activity.clear();
                    tab.turn_id = None;
                    tab.usage = UsageStats::default();
                    tab.metrics = TurnMetrics::default();
                    tab.scroll_offset = 0;
                    tab.total_lines = 0;
                    tab.auto_follow = true;
                    tab.is_running = false;
                    tab.active_tool = None;
                    let _ = tab.cmd_tx.send(AgentCommand::Reset);
                    tab.transcript.push(TranscriptEntry::System {
                        text: "Conversation reset.".to_string(),
                    });
                    tab.session_id.clone()
                };
                if let Some(reg) = &self.registry {
                    reg.note_tui_reset(&session_id);
                }
            }
            "/cancel" => {
                let tab = self.tab_mut();
                if tab.is_running {
                    let _ = tab.cmd_tx.send(AgentCommand::Cancel);
                    self.status_notice = Some("Cancelling turn...".to_string());
                } else {
                    tab.transcript.push(TranscriptEntry::System {
                        text: "No active turn to cancel.".to_string(),
                    });
                }
            }
            "/compact" => {
                let tab = self.tab_mut();
                tab.auto_follow = true;
                tab.transcript.push(TranscriptEntry::System {
                    text: "Compaction requested.".to_string(),
                });
                let _ = tab.cmd_tx.send(AgentCommand::Compact {
                    focus: None,
                    from_turn_id: None,
                });
            }
            "/mode" => {
                if self.tab().ephemeral {
                    self.tab_mut().transcript.push(TranscriptEntry::System {
                        text: "btw is read-only".to_string(),
                    });
                    return;
                }
                let tab = self.tab_mut();
                tab.auto_follow = true;
                if arg.is_empty() {
                    let text = format!("Approval mode: {}", approval_mode_label(tab.approval_mode));
                    tab.transcript.push(TranscriptEntry::System { text });
                } else {
                    match parse_approval_mode(arg) {
                        Some(mode) => {
                            let session_id = {
                                let tab = self.tab_mut();
                                tab.approval_mode = mode;
                                let _ = tab.cmd_tx.send(AgentCommand::SetPermissionMode(mode));
                                tab.transcript.push(TranscriptEntry::System {
                                    text: format!("Approval mode: {}", approval_mode_label(mode)),
                                });
                                tab.session_id.clone()
                            };
                            if let Some(reg) = &self.registry {
                                reg.note_tui_mode(&session_id, mode);
                            }
                        }
                        None => {
                            tab.transcript.push(TranscriptEntry::Error {
                                text: format!("Invalid mode: {arg}. Expected: plan, ask, auto"),
                                expanded: false,
                            });
                        }
                    }
                }
            }
            "/rewind" => {
                if self.tab().is_running {
                    self.status_notice = Some("Cannot rewind while a turn is running".to_string());
                } else if let Some(turn_id) = self.latest_user_turn() {
                    self.modal = Some(ActiveModal::RewindMenu {
                        turn_id,
                        selected: 0,
                    });
                } else {
                    self.status_notice = Some("No closed turn to rewind".to_string());
                }
            }
            "/fork" => {
                if self.tab().is_running {
                    self.status_notice = Some("Cancel the running turn before forking".to_string());
                } else if self.session_opener.is_none() {
                    self.status_notice = Some("Fork needs an open session".to_string());
                } else if let Some(turn_id) = self.latest_user_turn() {
                    let session_id = self.tab().session_id.clone();
                    self.pending_session_actions
                        .push(PendingSessionAction::ForkSession {
                            session_id,
                            turn: Some(turn_id),
                            draft: String::new(),
                        });
                    self.status_notice = Some("Forking session...".to_string());
                } else {
                    self.status_notice = Some("No closed turn to fork".to_string());
                }
            }
            "/btw" => {
                if self.session_opener.is_none() {
                    self.status_notice = Some("btw needs an open session".to_string());
                } else if self.tab().is_acp() {
                    self.status_notice =
                        Some("btw needs a native session (ACP keeps its own history)".to_string());
                } else {
                    let session_id = self.tab().session_id.clone();
                    self.pending_session_actions
                        .push(PendingSessionAction::BtwSession {
                            session_id,
                            text: arg.to_string(),
                        });
                    self.status_notice = Some("Opening btw...".to_string());
                }
            }
            "/reload" => {
                self.pending_reload = true;
                self.status_notice = Some("Reloading configuration...".to_string());
            }
            "/serve" => {
                if arg.eq_ignore_ascii_case("stop") {
                    if self.web_server.is_none() {
                        self.status_notice = Some("Web UI server is not running".to_string());
                    } else {
                        self.pending_session_actions
                            .push(PendingSessionAction::StopWebServer);
                        self.status_notice = Some("Stopping web UI server...".to_string());
                    }
                } else if !arg.is_empty() {
                    self.tab_mut().transcript.push(TranscriptEntry::Error {
                        text: format!(
                            "Unknown /serve argument '{arg}'. Expected `/serve` or `/serve stop`."
                        ),
                        expanded: false,
                    });
                } else if let Some(server) = &self.web_server {
                    let msg = format!(
                        "Web UI already running at {} (use /serve stop to stop).",
                        server.url
                    );
                    self.status_notice = Some(msg.clone());
                    self.tab_mut()
                        .transcript
                        .push(TranscriptEntry::System { text: msg });
                } else if self.web_launcher.is_none() {
                    self.tab_mut().transcript.push(TranscriptEntry::Error {
                        text: "Web UI server is not enabled in this build (build with --features web)."
                            .to_string(),
                        expanded: false,
                    });
                } else {
                    self.modal = Some(ActiveModal::ServeDialog {
                        port_input: TextInput::default(),
                        secret_input: TextInput::default(),
                        editing_secret: false,
                        error: None,
                        busy: false,
                    });
                }
            }
            "/update" => {
                let current = env!("CARGO_PKG_VERSION");
                let text = match &self.update_available {
                    Some(latest) => format!(
                        "Update available: v{current} -> v{latest}\nRun: {}",
                        crate::update::install_hint()
                    ),
                    None => {
                        format!("meerai v{current} is up to date (or the update check failed).")
                    }
                };
                let tab = self.tab_mut();
                tab.auto_follow = true;
                tab.transcript.push(TranscriptEntry::System { text });
            }
            "/quit" | "/q" => {
                self.quit();
            }
            "/context" | "/ctx" => {
                let breakdown = self.tab().context.clone();
                self.modal = Some(ActiveModal::Context(ContextPanel {
                    breakdown,
                    ..ContextPanel::default()
                }));
                let _ = self.tab().cmd_tx.send(AgentCommand::InspectContext);
            }
            "/loop" => {
                let state = self.tab().loop_state.clone();
                self.modal = Some(ActiveModal::Loop { state, scroll: 0 });
            }
            "/memory" => {
                self.modal = Some(ActiveModal::Memory {
                    entries: Vec::new(),
                    query: arg.to_string(),
                    selected: 0,
                    loading: true,
                    error: None,
                });
                self.pending_session_actions
                    .push(PendingSessionAction::LoadMemories {
                        query: (!arg.is_empty()).then(|| arg.to_string()),
                    });
            }
            "/remember" | "/forget" | "/pin" => match parse_host_action(command) {
                Some(action) => self.queue_host_action(action),
                None => {
                    self.tab_mut().transcript.push(TranscriptEntry::Error {
                        text: format!("Usage: {command} is incomplete."),
                        expanded: false,
                    });
                }
            },
            "/routing" | "/routes" => {
                if arg.is_empty() || arg.starts_with("set") || arg.starts_with("clear") {
                    if arg.is_empty() {
                        self.modal = Some(ActiveModal::Routing {
                            routes: Vec::new(),
                            selected: 0,
                            loading: true,
                            error: None,
                        });
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadRoutes);
                    } else {
                        match parse_host_action(command) {
                            Some(action) => self.queue_host_action(action),
                            None => self.host_action_usage(command),
                        }
                    }
                } else {
                    self.host_action_usage(command);
                }
            }
            "/packs" => {
                self.modal = Some(ActiveModal::Packs {
                    packs: Vec::new(),
                    definitions: meerai_host::HostDefinitionsDiagnostics::default(),
                    tab: PackTab::Packs,
                    selected: 0,
                    loading: true,
                    error: None,
                });
                self.pending_session_actions
                    .push(PendingSessionAction::LoadPacks);
            }
            "/definitions" => {
                self.modal = Some(ActiveModal::Packs {
                    packs: Vec::new(),
                    definitions: meerai_host::HostDefinitionsDiagnostics::default(),
                    tab: PackTab::Definitions,
                    selected: 0,
                    loading: true,
                    error: None,
                });
                self.pending_session_actions
                    .push(PendingSessionAction::LoadDefinitions);
            }
            "/pack" => match parse_host_action(command) {
                Some(action) => self.queue_host_action(action),
                None => self.host_action_usage(command),
            },
            "/open" => match parse_host_action(command) {
                Some(action) => self.queue_host_action(action),
                None => self.host_action_usage(command),
            },
            "/vault-reset" | "/vault" => {
                self.modal = Some(ActiveModal::VaultReset {
                    input: TextInput::default(),
                    error: None,
                });
            }
            "/subagent" | "/subagents" => {
                if arg.is_empty() {
                    self.modal = Some(ActiveModal::SubagentSpawn(Box::new(SubagentSpawnPanel {
                        agent: TextInput::new("explore"),
                        ..SubagentSpawnPanel::default()
                    })));
                } else {
                    match parse_host_action(command) {
                        Some(action) => self.queue_host_action(action),
                        None => self.host_action_usage(command),
                    }
                }
            }
            other => {
                let tab = self.tab_mut();
                tab.auto_follow = true;
                tab.transcript.push(TranscriptEntry::Error {
                    text: format!("Unknown command: {other}. Type /help for available commands."),
                    expanded: false,
                });
            }
        }
    }

    fn host_action_usage(&mut self, command: &str) {
        let tab = self.tab_mut();
        tab.auto_follow = true;
        tab.transcript.push(TranscriptEntry::Error {
            text: format!(
                "Usage: {command} is incomplete. Try /memory, /remember <note>,                  /routing set <task> <provider::model>, or /pack <add|trust|enable|pin|remove> <id>."
            ),
            expanded: false,
        });
    }

    pub(crate) fn queue_host_action(&mut self, action: meerai_host::HostAction) {
        let tab_id = self.tab().tab_id;
        self.pending_session_actions
            .push(PendingSessionAction::HostAction { tab_id, action });
    }

    pub fn open_session_select(&mut self) {
        let open_summaries: Vec<SessionSummary> = self
            .tabs
            .iter()
            .map(|tab| SessionSummary {
                id: tab.session_id.clone(),
                dir: self.workspace_root.clone(),
                title: Some(tab.display_title()),
                model: (!tab.model.trim().is_empty()).then(|| tab.model.clone()),
                created_at_ms: 0,
                updated_at_ms: 0,
                bytes: 0,
                parent_session_id: None,
            })
            .collect();
        let has_opener = self.session_opener.is_some();
        self.modal = Some(ActiveModal::SessionSelect {
            filter: TextInput::default(),
            selected: 0,
            sessions: open_summaries,
            loading: has_opener,
            error: None,
        });
        if has_opener {
            self.pending_session_actions
                .push(PendingSessionAction::RefreshSessions);
        }
    }

    pub(super) fn switch_to_session_query(&mut self, query: &str) {
        let trimmed = query.trim();
        if let Some(exact_idx) = self
            .tabs
            .iter()
            .position(|tab| tab.session_id.as_str() == trimmed)
        {
            self.switch_to_tab(exact_idx);
            self.status_notice = Some(format!("Switched to tab {}", exact_idx + 1));
            return;
        }
        let prefix_matches: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.session_id.as_str().starts_with(trimmed))
            .map(|(idx, _)| idx)
            .collect();
        if prefix_matches.len() == 1 && self.session_opener.is_none() {
            let idx = prefix_matches[0];
            self.switch_to_tab(idx);
            self.status_notice = Some(format!("Switched to tab {}", idx + 1));
            return;
        }
        if self.session_opener.is_some() {
            self.status_notice = Some(format!("Opening session {trimmed}..."));
            self.pending_session_actions
                .push(PendingSessionAction::OpenSessionQuery(trimmed.to_string()));
        } else if prefix_matches.len() == 1 {
            let idx = prefix_matches[0];
            self.switch_to_tab(idx);
            self.status_notice = Some(format!("Switched to tab {}", idx + 1));
        } else {
            self.tab_mut().transcript.push(TranscriptEntry::Error {
                text: format!("Session not found: {trimmed}"),
                expanded: false,
            });
        }
    }

    pub(super) fn available_agent_ids(&self) -> String {
        if self.agents.is_empty() {
            return "(none)".to_string();
        }
        self.agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn available_skill_ids(&self) -> String {
        if self.skills.is_empty() {
            return "(none)".to_string();
        }
        self.skills
            .iter()
            .map(|skill| skill.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn apply_agent(&mut self, id: &str) {
        self.tab_mut().auto_follow = true;
        let Some(agent) = self.agents.iter().find(|agent| agent.id == id).cloned() else {
            let available = self.available_agent_ids();
            let message = if id.is_empty() {
                format!("Usage: /agent <id>. Available agents: {available}")
            } else {
                format!("Unknown agent: {id}. Available agents: {available}")
            };
            self.tab_mut()
                .transcript
                .push(TranscriptEntry::System { text: message });
            return;
        };
        let model = self.agent_model_spec(&agent);
        let source = agent.source.label();
        self.send_agent(&agent, model);
        self.tab_mut().transcript.push(TranscriptEntry::System {
            text: format!("Applied agent {} ({source}).", agent.id),
        });
    }

    pub(super) fn apply_skill(&mut self, id: &str) {
        self.tab_mut().auto_follow = true;
        let Some(skill) = self.skills.iter().find(|skill| skill.id == id).cloned() else {
            let available = self.available_skill_ids();
            let message = if id.is_empty() {
                format!("Usage: /skill <id>. Available skills: {available}")
            } else {
                format!("Unknown skill: {id}. Available skills: {available}")
            };
            self.tab_mut()
                .transcript
                .push(TranscriptEntry::System { text: message });
            return;
        };
        let source = skill.source.label();
        self.send_skill(&skill);
        self.tab_mut().transcript.push(TranscriptEntry::System {
            text: format!("Applied skill {} ({source}).", skill.id),
        });
    }

    fn open_mention_picker(&mut self, kind: &str) {
        let tab = self.tab_mut();
        tab.auto_follow = true;
        tab.input = format!("@{kind}/");
        tab.cursor = tab.input.len();
        self.mode = InputMode::Insert;
        self.update_completion();
        if !self.completion.is_active() {
            self.status_notice = Some(format!("No {kind}s available"));
        }
    }

    pub(super) fn send_agent(&mut self, agent: &AgentDefinition, model: Option<String>) {
        let provider = self.tab().active_provider.clone();
        let context_window_tokens = model
            .as_ref()
            .and_then(|model| self.model_context_window(&provider, model));
        let effort_levels = model.as_ref().and_then(|model| {
            self.runtime
                .read()
                .ok()
                .and_then(|runtime| runtime.catalog.effort_levels_for(&provider, model))
        });
        let _ = self.tab().cmd_tx.send(AgentCommand::ApplyAgent {
            name: agent.id.clone(),
            instructions: agent.prompt.clone(),
            model,
            context_window_tokens,
            effort_levels,
        });
    }

    pub(super) fn send_skill(&mut self, skill: &SkillDefinition) {
        let _ = self.tab().cmd_tx.send(AgentCommand::ApplySkill {
            name: skill.id.clone(),
            instructions: skill.body.clone(),
        });
    }

    pub(super) fn agent_model_spec(&mut self, agent: &AgentDefinition) -> Option<String> {
        let raw = match validate_agent_model(agent.model.as_deref()) {
            Ok(Some(raw)) => raw,
            Ok(None) => return None,
            Err(raw) => {
                self.tab_mut().transcript.push(TranscriptEntry::System {
                    text: format!(
                        "Agent {} model '{raw}' implies a provider change; it is not applied.",
                        agent.id
                    ),
                });
                return None;
            }
        };
        let provider = self.tab().active_provider.clone();
        let spec = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.format_genai_spec(&provider, raw)
        };
        self.tab_mut().model = raw.to_string();
        Some(spec)
    }
}

fn format_help_text() -> String {
    let commands = crate::completion::SLASH_COMMANDS
        .iter()
        .map(|(cmd, _, _)| match *cmd {
            "/sessions" => "/sessions [id]".to_string(),
            "/model" => "/model [name]".to_string(),
            "/effort" => "/effort [low|medium|high|xhigh|max] (no arg opens sampling)".to_string(),
            "/sampling" => "/sampling [effort and temperature]".to_string(),
            "/temperature" => "/temperature [0.0..2.0|default]".to_string(),
            "/agent" => "/agent <id> (or @agent/<id>)".to_string(),
            "/skill" => "/skill <id> (or @skill/<id>)".to_string(),
            "/loop" => "/loop (or @goal / @orchestrate <instruction>)".to_string(),
            "/mode" => "/mode [plan|ask|auto]".to_string(),
            "/btw" => "/btw [question] (read-only side session with this history)".to_string(),
            "/search" => "/search [query]".to_string(),
            "/grep" => "/grep [query]".to_string(),
            "/diff" => "/diff [path]".to_string(),
            "/memory" => "/memory [query]".to_string(),
            "/remember" => "/remember <note>".to_string(),
            "/forget" => "/forget <id>".to_string(),
            "/pin" => "/pin <id> [off]".to_string(),
            "/routing" => {
                "/routing [set <task> <provider::model> [max_cost] [max_tokens] [concurrency]]"
                    .to_string()
            }
            "/pack" => "/pack <add|enable|disable|trust|untrust|pin|remove> <id> [rev]".to_string(),
            "/subagent" => "/subagent <agent> <task> | /subagent cancel <item-id>".to_string(),
            "/open" => "/open <path>[:line]".to_string(),
            "/vault-reset" => "/vault-reset (type reset to confirm)".to_string(),
            other => (*other).to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Commands: {commands}\n\
         Tabs: alt+1..9 / alt+left/right / ctrl+pageup/down / click tab, alt+shift+left/right move tab, ctrl+w close tab, click [+] new tab\n\
         Shortcuts: ctrl+p command palette, shift+tab approval mode, ctrl+f search, ctrl+b toggle sidebar, drag+scroll or click then shift+click to copy, click a thought block to expand it\n\
         Insert: Enter send (queues while a turn runs), ctrl+g send queued now, shift/alt/ctrl+enter or ctrl+j or \\+Enter newline, @ file, / command, Esc Normal\n\
         Normal: j/k/Up/Down scroll, gt/gT/[ / ] switch tab, / search, t toggle thinking, y copy reply, ctrl+d/ctrl+u half-page, gg/G top/bottom, i/a Insert, q quit"
    )
}

const MENTION_PREFIX_LEN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MentionKind {
    Agent,
    Skill,
}

pub(crate) fn mention_is_known(
    kind: MentionKind,
    id: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> bool {
    match kind {
        MentionKind::Agent => agents.iter().any(|agent| agent.id == id),
        MentionKind::Skill => skills.iter().any(|skill| skill.id == id),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct MentionPlan {
    pub(super) agent_indices: Vec<usize>,
    pub(super) skill_indices: Vec<usize>,
    pub(super) text: Option<String>,
}

pub(super) fn parse_mentions(
    text: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> MentionPlan {
    let mut agent_indices: Vec<usize> = Vec::new();
    let mut skill_indices: Vec<usize> = Vec::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut changed = false;
    let mut index = 0;

    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        if ch == '@'
            && let Some((kind, id, end)) = parse_mention_token(text, index)
        {
            let found = match kind {
                MentionKind::Agent => agents.iter().position(|agent| agent.id == id).map(|pos| {
                    if !agent_indices.contains(&pos) {
                        agent_indices.push(pos);
                    }
                }),
                MentionKind::Skill => skills.iter().position(|skill| skill.id == id).map(|pos| {
                    if !skill_indices.contains(&pos) {
                        skill_indices.push(pos);
                    }
                }),
            };
            if found.is_some() {
                changed = true;
                index = end;
                continue;
            }
        }
        cleaned.push(ch);
        index += ch.len_utf8();
    }

    MentionPlan {
        agent_indices,
        skill_indices,
        text: changed.then(|| collapse_whitespace(&cleaned)),
    }
}

pub(crate) fn parse_mention_token(text: &str, start: usize) -> Option<(MentionKind, &str, usize)> {
    if !text.get(start..).is_some_and(|rest| rest.starts_with('@')) {
        return None;
    }
    let rest = text.get(start + 1..)?;
    let (kind, rest) = if let Some(rest) = rest.strip_prefix("agent/") {
        (MentionKind::Agent, rest)
    } else if let Some(rest) = rest.strip_prefix("skill/") {
        (MentionKind::Skill, rest)
    } else {
        let (id, after) = ["orchestrate", "goal"]
            .into_iter()
            .find_map(|id| rest.strip_prefix(id).map(|after| (id, after)))?;
        if after.chars().next().is_some_and(is_mention_id_char) {
            return None;
        }
        return Some((MentionKind::Skill, id, start + 1 + id.len()));
    };
    let id_len = rest
        .char_indices()
        .find(|(_, ch)| !is_mention_id_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    if id_len == 0 {
        return None;
    }
    Some((
        kind,
        &rest[..id_len],
        start + 1 + MENTION_PREFIX_LEN + id_len,
    ))
}

fn is_mention_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut pending_newline = false;
    for ch in text.chars() {
        if ch == '\n' {
            pending_newline = true;
            pending_space = false;
        } else if ch.is_whitespace() {
            pending_space = true;
        } else {
            if !result.is_empty() {
                if pending_newline {
                    result.push('\n');
                } else if pending_space {
                    result.push(' ');
                }
            }
            pending_newline = false;
            pending_space = false;
            result.push(ch);
        }
    }
    result
}
