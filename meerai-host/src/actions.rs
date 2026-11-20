use std::path::Path;

use anyhow::{Context, Result, bail};
use meerai_agents::{AgentCommand, SubagentSpec};
use meerai_policy::ApprovalMode;
use meerai_store::{ModelRoute, NewProjectMemory, ProjectMemory, ProjectMemoryKind, TaskType};
use tokio::sync::mpsc;

use crate::session::{HostDefinitionsDiagnostics, SessionOpener};

#[derive(Clone, Debug, PartialEq)]
pub enum HostAction {
    InspectContext,
    PinTurn {
        turn_id: String,
        pinned: bool,
    },
    SetContextPolicy {
        verbatim_window_turns: Option<usize>,
        reasoning_pruning: Option<bool>,
        clear_old_tool_results: Option<bool>,
    },
    Summarize {
        focus: Option<String>,
        from_turn_id: Option<String>,
    },
    ReadBlob {
        blob_key: String,
    },
    ListMemories {
        query: Option<String>,
    },
    Remember {
        note: String,
    },
    ForgetMemory {
        id: String,
    },
    PinMemory {
        id: String,
        pinned: bool,
    },
    ListRoutes,
    SetRoute {
        task_type: String,
        provider: String,
        model: String,
        effort: Option<String>,
        max_cost_usd: Option<f64>,
        max_tokens_per_turn: Option<u64>,
        max_concurrency: Option<usize>,
    },
    ClearRoute {
        task_type: String,
    },
    ListPacks,
    InstallPack {
        source: String,
        rev: Option<String>,
    },
    SetPackEnabled {
        id: String,
        enabled: bool,
    },
    SetPackTrusted {
        id: String,
        trusted: bool,
    },
    PinPack {
        id: String,
        rev: String,
    },
    RemovePack {
        id: String,
    },
    Definitions,
    SpawnSubagent {
        spec: SubagentSpec,
    },
    CancelSubagent {
        item_id: String,
    },
    ResetVault,
}

pub struct HostActionContext<'a> {
    pub opener: &'a dyn SessionOpener,
    pub project_root: &'a Path,
    pub session_id: Option<&'a meerai_session::SessionId>,
    pub cmd_tx: Option<&'a mpsc::UnboundedSender<AgentCommand>>,
}

pub fn parse_host_action(raw: &str) -> Option<HostAction> {
    let trimmed = raw.trim();
    let (command, rest) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(command, rest)| (command, rest.trim()));
    match command {
        "/context" => Some(HostAction::InspectContext),
        "/pin-turn" => parse_pin_turn(rest),
        "/context-policy" => parse_context_policy(rest),
        "/summarize" => parse_summarize(rest),
        "/blob" => parse_id(rest).map(|blob_key| HostAction::ReadBlob { blob_key }),
        "/memory" => Some(HostAction::ListMemories {
            query: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/remember" => (!rest.is_empty()).then(|| HostAction::Remember {
            note: rest.to_string(),
        }),
        "/forget" => parse_id(rest).map(|id| HostAction::ForgetMemory { id }),
        "/pin" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?.to_string();
            let pinned = !matches!(parts.next(), Some("off" | "false" | "unpin"));
            Some(HostAction::PinMemory { id, pinned })
        }
        "/routing" | "/routes" => parse_routing(rest),
        "/packs" => Some(HostAction::ListPacks),
        "/pack" => parse_pack(rest),
        "/definitions" | "/skills" => Some(HostAction::Definitions),
        "/subagent" | "/subagents" => parse_subagent(rest),
        "/vault-reset" if rest.trim() == "reset" => Some(HostAction::ResetVault),
        _ => None,
    }
}

fn parse_id(rest: &str) -> Option<String> {
    let id = rest.split_whitespace().next()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

fn parse_toggle(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_pin_turn(rest: &str) -> Option<HostAction> {
    let mut parts = rest.split_whitespace();
    let turn_id = parts.next()?.to_string();
    let pinned = !matches!(parts.next(), Some("off" | "false" | "unpin"));
    Some(HostAction::PinTurn { turn_id, pinned })
}

fn parse_context_policy(rest: &str) -> Option<HostAction> {
    let mut parts = rest.split_whitespace();
    let key = parts.next()?;
    let value = parts.next()?;
    let mut action = HostAction::SetContextPolicy {
        verbatim_window_turns: None,
        reasoning_pruning: None,
        clear_old_tool_results: None,
    };
    match key {
        "verbatim" | "window" => {
            if let HostAction::SetContextPolicy {
                verbatim_window_turns,
                ..
            } = &mut action
            {
                *verbatim_window_turns = Some(value.parse().ok()?);
            }
        }
        "reasoning" => {
            if let HostAction::SetContextPolicy {
                reasoning_pruning, ..
            } = &mut action
            {
                *reasoning_pruning = Some(parse_toggle(value)?);
            }
        }
        "clear" => {
            if let HostAction::SetContextPolicy {
                clear_old_tool_results,
                ..
            } = &mut action
            {
                *clear_old_tool_results = Some(parse_toggle(value)?);
            }
        }
        _ => return None,
    }
    Some(action)
}

fn parse_summarize(rest: &str) -> Option<HostAction> {
    let mut tokens = rest.split_whitespace();
    let mut from_turn_id = None;
    let mut focus = Vec::new();
    while let Some(token) = tokens.next() {
        if token == "--from" {
            from_turn_id = Some(tokens.next()?.to_string());
        } else {
            focus.push(token);
        }
    }
    Some(HostAction::Summarize {
        focus: (!focus.is_empty()).then(|| focus.join(" ")),
        from_turn_id,
    })
}

fn parse_routing(rest: &str) -> Option<HostAction> {
    if rest.is_empty() {
        return Some(HostAction::ListRoutes);
    }
    let mut parts = rest.split_whitespace();
    match parts.next()? {
        "clear" | "remove" | "delete" => Some(HostAction::ClearRoute {
            task_type: parts.next()?.to_string(),
        }),
        "set" | "add" => {
            let task_type = parts.next()?.to_string();
            let spec = parts.next()?.to_string();
            let (provider, model) = spec.split_once("::").or_else(|| spec.split_once(':'))?;
            if provider.is_empty() || model.is_empty() {
                return None;
            }
            let mut effort = None;
            let mut max_cost_usd = None;
            let mut max_tokens_per_turn = None;
            let mut max_concurrency = None;
            while let Some(token) = parts.next() {
                match token {
                    "--effort" => effort = Some(parts.next()?.to_string()),
                    other if max_cost_usd.is_none() => {
                        max_cost_usd = Some(other.parse().ok()?);
                    }
                    other if max_tokens_per_turn.is_none() => {
                        max_tokens_per_turn = Some(other.parse().ok()?);
                    }
                    other if max_concurrency.is_none() => {
                        max_concurrency = Some(other.parse().ok()?);
                    }
                    _ => return None,
                }
            }
            Some(HostAction::SetRoute {
                task_type,
                provider: provider.to_string(),
                model: model.to_string(),
                effort,
                max_cost_usd,
                max_tokens_per_turn,
                max_concurrency,
            })
        }
        _ => None,
    }
}

fn parse_pack(rest: &str) -> Option<HostAction> {
    let mut parts = rest.split_whitespace();
    let verb = parts.next()?;
    let id = parts.next().map(str::to_string);
    match verb {
        "add" | "install" => Some(HostAction::InstallPack {
            source: id?,
            rev: parts.next().map(str::to_string),
        }),
        "enable" | "disable" => Some(HostAction::SetPackEnabled {
            id: id?,
            enabled: verb == "enable",
        }),
        "trust" | "untrust" => Some(HostAction::SetPackTrusted {
            id: id?,
            trusted: verb == "trust",
        }),
        "pin" => Some(HostAction::PinPack {
            id: id?,
            rev: parts.next()?.to_string(),
        }),
        "remove" | "rm" => Some(HostAction::RemovePack { id: id? }),
        _ => None,
    }
}

fn parse_subagent(rest: &str) -> Option<HostAction> {
    let mut tokens = rest.split_whitespace();
    let first = tokens.next()?;
    if first == "cancel" {
        let item_id = tokens.next()?.to_string();
        return Some(HostAction::CancelSubagent { item_id });
    }
    let mut spec = SubagentSpec {
        agent: first.to_string(),
        ..SubagentSpec::default()
    };
    let mut task_parts: Vec<&str> = Vec::new();
    while let Some(token) = tokens.next() {
        match token {
            "--model" => spec.model = Some(tokens.next()?.to_string()),
            "--mode" => {
                spec.approval_mode = Some(ApprovalMode::parse(tokens.next()?)?);
            }
            "--steps" => spec.max_steps = tokens.next()?.parse().ok(),
            "--tokens" => spec.max_tokens = tokens.next()?.parse().ok(),
            "--cost" => spec.max_cost_usd = tokens.next()?.parse().ok(),
            "--tools" => {
                spec.allowed_tools = Some(
                    tokens
                        .next()?
                        .split(',')
                        .map(str::trim)
                        .filter(|tool| !tool.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
            }
            other => task_parts.push(other),
        }
    }
    if spec.agent.is_empty() || task_parts.is_empty() {
        return None;
    }
    spec.task = task_parts.join(" ");
    Some(HostAction::SpawnSubagent { spec })
}

pub async fn execute_host_action(ctx: HostActionContext<'_>, action: HostAction) -> Result<String> {
    match action {
        HostAction::InspectContext => {
            let cmd_tx = ctx.cmd_tx.context("no active session for /context")?;
            let _ = cmd_tx.send(AgentCommand::InspectContext);
            Ok("Context inspection requested.".to_string())
        }
        HostAction::PinTurn { turn_id, pinned } => {
            let cmd_tx = ctx.cmd_tx.context("no active session for /pin-turn")?;
            let _ = cmd_tx.send(AgentCommand::SetTurnPinned {
                turn_id: meerai_session::TurnId::from_raw(turn_id.clone()),
                pinned,
            });
            Ok(format!(
                "Turn {turn_id} is {}.",
                if pinned { "pinned" } else { "unpinned" }
            ))
        }
        HostAction::SetContextPolicy {
            verbatim_window_turns,
            reasoning_pruning,
            clear_old_tool_results,
        } => {
            let cmd_tx = ctx
                .cmd_tx
                .context("no active session for /context-policy")?;
            let _ = cmd_tx.send(AgentCommand::SetProjectionOverrides {
                verbatim_window_turns,
                reasoning_pruning,
                clear_old_tool_results,
                pin_turns: None,
            });
            Ok("Context policy updated.".to_string())
        }
        HostAction::Summarize {
            focus,
            from_turn_id,
        } => {
            let cmd_tx = ctx.cmd_tx.context("no active session for /summarize")?;
            let _ = cmd_tx.send(AgentCommand::Compact {
                focus: focus.clone(),
                from_turn_id: from_turn_id.clone().map(meerai_session::TurnId::from_raw),
            });
            Ok(match from_turn_id {
                Some(turn) => format!("Summarizing from turn {turn}."),
                None => "Compaction requested.".to_string(),
            })
        }
        HostAction::ReadBlob { blob_key } => {
            let id = ctx.session_id.context("no active session for /blob")?;
            ctx.opener.read_session_blob(id, &blob_key).await
        }
        HostAction::ListMemories { query } => {
            let project = ctx.project_root.display().to_string();
            let memories = match query {
                Some(query) => ctx.opener.search_memories(&project, &query).await?,
                None => ctx.opener.list_memories(&project).await?,
            };
            Ok(render_memories(&memories))
        }
        HostAction::Remember { note } => {
            let title = note.lines().next().unwrap_or("memory").trim();
            let title = if title.is_empty() { "memory" } else { title };
            let memory = ctx
                .opener
                .save_memory(
                    NewProjectMemory {
                        project_root: ctx.project_root.display().to_string(),
                        kind: ProjectMemoryKind::Fact,
                        title: title.to_string(),
                        body: note.clone(),
                        source_session_id: None,
                        pinned: false,
                    },
                    None,
                )
                .await?;
            Ok(format!("Saved memory {} ({})", memory.id, memory.title))
        }
        HostAction::ForgetMemory { id } => {
            let removed = ctx
                .opener
                .delete_memory(&ctx.project_root.display().to_string(), &id)
                .await?;
            Ok(if removed {
                format!("Removed memory {id}")
            } else {
                format!("No memory {id}")
            })
        }
        HostAction::PinMemory { id, pinned } => {
            let memory = ctx
                .opener
                .set_memory_pinned(&ctx.project_root.display().to_string(), &id, pinned)
                .await?
                .with_context(|| format!("no memory {id}"))?;
            Ok(format!(
                "Memory {} is {}",
                memory.id,
                if memory.pinned { "pinned" } else { "unpinned" }
            ))
        }
        HostAction::ListRoutes => {
            let routes = ctx.opener.routes().await?;
            Ok(render_routes(&routes.routes))
        }
        HostAction::SetRoute {
            task_type,
            provider,
            model,
            effort,
            max_cost_usd,
            max_tokens_per_turn,
            max_concurrency,
        } => {
            let task = TaskType::parse(&task_type)?;
            ctx.opener
                .upsert_route(ModelRoute {
                    task_type: task,
                    provider,
                    model,
                    effort,
                    max_tokens_per_turn,
                    max_cost_usd,
                    max_concurrency,
                })
                .await?;
            Ok(format!("Saved route {task_type}"))
        }
        HostAction::ClearRoute { task_type } => {
            let removed = ctx.opener.delete_route(&task_type).await?;
            Ok(if removed {
                format!("Removed route {task_type}")
            } else {
                format!("No route {task_type}")
            })
        }
        HostAction::ListPacks => {
            let packs = ctx.opener.list_packs().await?;
            Ok(render_packs(&packs))
        }
        HostAction::InstallPack { source, rev } => {
            let pack = ctx.opener.install_pack(&source, rev.as_deref()).await?;
            Ok(format!("Installed pack {} at {}", pack.id, pack.pinned_rev))
        }
        HostAction::SetPackEnabled { id, enabled } => {
            let changed = ctx.opener.set_pack_enabled(&id, enabled).await?;
            Ok(if changed {
                format!(
                    "Pack {id} is {}",
                    if enabled { "enabled" } else { "disabled" }
                )
            } else {
                format!("No pack {id}")
            })
        }
        HostAction::SetPackTrusted { id, trusted } => {
            let changed = ctx.opener.set_pack_trusted(&id, trusted).await?;
            Ok(if changed {
                format!(
                    "Pack {id} is {}",
                    if trusted { "trusted" } else { "untrusted" }
                )
            } else {
                format!("No pack {id}")
            })
        }
        HostAction::PinPack { id, rev } => {
            let changed = ctx.opener.pin_pack(&id, &rev).await?;
            Ok(if changed {
                format!("Pinned pack {id} at {rev}")
            } else {
                format!("No pack {id}")
            })
        }
        HostAction::RemovePack { id } => {
            let removed = ctx.opener.remove_pack(&id).await?;
            Ok(if removed {
                format!("Removed pack {id}")
            } else {
                format!("No pack {id}")
            })
        }
        HostAction::Definitions => {
            let diagnostics = ctx.opener.definitions_diagnostics().await?;
            Ok(render_definitions(&diagnostics))
        }
        HostAction::SpawnSubagent { spec } => {
            let cmd_tx = ctx.cmd_tx.context("no active session for /subagent")?;
            let agent = spec.agent.clone();
            let _ = cmd_tx.send(AgentCommand::SpawnSubagent { spec });
            Ok(format!("Subagent {agent} started."))
        }
        HostAction::ResetVault => {
            let removed = ctx.opener.reset_vault().await?;
            Ok(format!(
                "Vault reset: removed {removed} saved credentials. Log in again."
            ))
        }
        HostAction::CancelSubagent { item_id } => {
            let cmd_tx = ctx.cmd_tx.context("no active session for /subagent")?;
            let _ = cmd_tx.send(AgentCommand::CancelSubagent {
                item_id: item_id.as_str().into(),
            });
            Ok(format!("Cancel requested for {item_id}"))
        }
    }
}

pub fn render_context(breakdown: Option<&meerai_session::ContextBreakdown>) -> String {
    let Some(breakdown) = breakdown else {
        return "No context report yet. Run /context again after the next step.".to_string();
    };
    format!(
        "Context: {used}/{window} tokens ({pct:.1}%), watermark {watermark}\n         system {system} · tools {tools} · memory {memory} · user {user} · assistant {assistant}\n         tool results {results} · reasoning {reasoning} · summaries {summaries}\n         pinned {pinned} · verbatim turns {verbatim} · cleared results {cleared} · spilled {spilled}\n         cached {cached}",
        used = breakdown.estimated_total_tokens,
        window = breakdown.context_window_tokens,
        pct = if breakdown.context_window_tokens == 0 {
            0.0
        } else {
            breakdown.estimated_total_tokens as f64 * 100.0 / breakdown.context_window_tokens as f64
        },
        watermark = breakdown.high_watermark_tokens,
        system = breakdown.system_prompt_tokens,
        tools = breakdown.tool_schemas_tokens,
        memory = breakdown.memory_tokens,
        user = breakdown.user_messages_tokens,
        assistant = breakdown.assistant_tokens,
        results = breakdown.tool_results_tokens,
        reasoning = breakdown.reasoning_tokens,
        summaries = breakdown.compaction_summary_tokens,
        pinned = breakdown.pinned_turn_ids.len(),
        verbatim = breakdown.turns_in_verbatim_window,
        cleared = breakdown.cleared_tool_results_count,
        spilled = breakdown.spilled_blob_count,
        cached = breakdown
            .last_prompt_cached_tokens
            .map_or_else(|| "n/a".to_string(), |tokens| tokens.to_string()),
    )
}

pub fn render_memories(memories: &[ProjectMemory]) -> String {
    if memories.is_empty() {
        return "No project memories.".to_string();
    }
    let mut out = format!("Memories ({}):", memories.len());
    for memory in memories {
        out.push_str(&format!(
            "\n- {} [{}] {}{}\n  {}",
            memory.id,
            kind_label(memory.kind),
            memory.title,
            if memory.pinned { " (pinned)" } else { "" },
            memory.body.replace('\n', " ")
        ));
    }
    out
}

pub fn render_routes(routes: &[ModelRoute]) -> String {
    if routes.is_empty() {
        return "No model routes.".to_string();
    }
    let mut out = format!("Routes ({}):", routes.len());
    for route in routes {
        let mut line = format!(
            "\n- {} -> {}::{}",
            route.task_type.as_key(),
            route.provider,
            route.model
        );
        if let Some(effort) = &route.effort {
            line.push_str(&format!(" effort={effort}"));
        }
        if let Some(cost) = route.max_cost_usd {
            line.push_str(&format!(" max_cost=${cost:.4}"));
        }
        if let Some(tokens) = route.max_tokens_per_turn {
            line.push_str(&format!(" max_tokens={tokens}"));
        }
        if let Some(concurrency) = route.max_concurrency {
            line.push_str(&format!(" concurrency={concurrency}"));
        }
        out.push_str(&line);
    }
    out
}

pub fn render_packs(packs: &[meerai_store::InstalledPack]) -> String {
    if packs.is_empty() {
        return "No installed packs.".to_string();
    }
    let mut out = format!("Packs ({}):", packs.len());
    for pack in packs {
        out.push_str(&format!(
            "\n- {} [{}] [{}] rev {} from {}",
            pack.id,
            if pack.enabled { "enabled" } else { "disabled" },
            if pack.trusted { "trusted" } else { "untrusted" },
            pack.pinned_rev,
            pack.source_uri
        ));
    }
    out
}

pub fn render_definitions(diagnostics: &HostDefinitionsDiagnostics) -> String {
    let mut out = format!(
        "Definitions: {} agents, {} skills",
        diagnostics.agents.len(),
        diagnostics.skills.len()
    );
    for agent in &diagnostics.agents {
        out.push_str(&format!(
            "\n- agent/{} ({}) ~{} tokens{}",
            agent.id,
            agent.source,
            agent.token_estimate,
            agent
                .mode
                .as_deref()
                .map(|mode| format!(" [{mode}]"))
                .unwrap_or_default()
        ));
    }
    for skill in &diagnostics.skills {
        out.push_str(&format!(
            "\n- skill/{} ({}) ~{} tokens",
            skill.id, skill.source, skill.token_estimate
        ));
    }
    if !diagnostics.warnings.is_empty() {
        out.push_str("\nWarnings:");
        for warning in &diagnostics.warnings {
            out.push_str(&format!("\n- {warning}"));
        }
    }
    out
}

fn kind_label(kind: ProjectMemoryKind) -> &'static str {
    match kind {
        ProjectMemoryKind::Decision => "decision",
        ProjectMemoryKind::Progress => "progress",
        ProjectMemoryKind::Fact => "fact",
        ProjectMemoryKind::Convention => "convention",
    }
}

pub fn route_task_type(route: &ModelRoute) -> String {
    route.task_type.as_key()
}

pub fn validate_action(action: &HostAction) -> Result<()> {
    if let HostAction::SetRoute {
        provider, model, ..
    } = action
    {
        if provider.trim().is_empty() {
            bail!("route provider must not be empty");
        }
        if model.trim().is_empty() {
            bail!("route model must not be empty");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_host_command() {
        assert_eq!(
            parse_host_action("/context"),
            Some(HostAction::InspectContext)
        );
        assert_eq!(
            parse_host_action("/memory parser"),
            Some(HostAction::ListMemories {
                query: Some("parser".to_string())
            })
        );
        assert_eq!(
            parse_host_action("/remember use the store"),
            Some(HostAction::Remember {
                note: "use the store".to_string()
            })
        );
        assert_eq!(
            parse_host_action("/pin mem-1 off"),
            Some(HostAction::PinMemory {
                id: "mem-1".to_string(),
                pinned: false
            })
        );
        assert_eq!(
            parse_host_action("/routing set build openai::o3 1.5 20000 2"),
            Some(HostAction::SetRoute {
                task_type: "build".to_string(),
                provider: "openai".to_string(),
                model: "o3".to_string(),
                effort: None,
                max_cost_usd: Some(1.5),
                max_tokens_per_turn: Some(20_000),
                max_concurrency: Some(2),
            })
        );
        assert_eq!(
            parse_host_action("/pack add ./packs/review"),
            Some(HostAction::InstallPack {
                source: "./packs/review".to_string(),
                rev: None
            })
        );
        assert_eq!(
            parse_host_action("/pack trust review"),
            Some(HostAction::SetPackTrusted {
                id: "review".to_string(),
                trusted: true
            })
        );
        assert_eq!(
            parse_host_action("/subagent explore find the parser"),
            Some(HostAction::SpawnSubagent {
                spec: SubagentSpec {
                    agent: "explore".to_string(),
                    task: "find the parser".to_string(),
                    ..SubagentSpec::default()
                }
            })
        );
        assert_eq!(
            parse_host_action("/subagent build --model o3 --mode ask --steps 6 fix it"),
            Some(HostAction::SpawnSubagent {
                spec: SubagentSpec {
                    agent: "build".to_string(),
                    task: "fix it".to_string(),
                    model: Some("o3".to_string()),
                    approval_mode: Some(ApprovalMode::Ask),
                    max_steps: Some(6),
                    ..SubagentSpec::default()
                }
            })
        );
        assert_eq!(
            parse_host_action("/subagent cancel item-1"),
            Some(HostAction::CancelSubagent {
                item_id: "item-1".to_string()
            })
        );
        assert_eq!(
            parse_host_action("/vault-reset reset"),
            Some(HostAction::ResetVault)
        );
        assert_eq!(parse_host_action("/vault-reset"), None);
        assert_eq!(parse_host_action("/bogus"), None);
        assert_eq!(parse_host_action("/routing set build o3"), None);
    }

    #[test]
    fn renders_empty_and_populated_collections() {
        assert_eq!(render_memories(&[]), "No project memories.");
        assert_eq!(render_routes(&[]), "No model routes.");
        assert_eq!(render_packs(&[]), "No installed packs.");
        assert_eq!(
            render_definitions(&HostDefinitionsDiagnostics::default()),
            "Definitions: 0 agents, 0 skills"
        );
    }
}
