use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal, Write as _};
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::{Result, bail};
use meerai_agents::events::{
    ApprovalDecision, ApprovalKind, ApprovalRequest, ItemStatus, RequestId,
};
use meerai_agents::{
    AgentCommand, AgentEvent, AgentHarness, HarnessConfig, ItemId, MultiTurnAgent, SessionItem,
    spawn_worker,
};
use meerai_common::definitions::{AgentDefinition, SkillDefinition, load_definitions};
use meerai_common::providers::{
    ProviderCatalog, RuntimeProviderState, build_genai_client, canonical_provider_id,
    first_configured_provider, format_auth_list,
};
use meerai_common::secrets::{auth_file_path, default_secret_store};
use meerai_common::settings::{
    AppSettings, ContextSettings, default_config_dir, default_data_dir, find_project_root,
    load_project_settings, load_settings_from, merge_project_settings,
};
use meerai_policy::{ApprovalMode, PolicyEngine, rules_from_yaml};
use meerai_session::{
    Session, SessionConfig, SessionId, SessionSummary, default_sessions_root, delete_session,
    latest_session, list_sessions,
};
use meerai_tools::CodeTools;
use meerai_tui::{TuiLaunchConfig, parse_approval_mode, transcript_from_events};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const FALLBACK_PROVIDER: &str = "opencode-go";
const EXIT_APPROVAL_REQUIRED: i32 = 2;

const HELP_TEXT: &str = "Usage:\n  \
    meerai [-p|--provider <PROVIDER>] [-m|--model <MODEL>] [--system <INSTRUCTIONS>]\n          \
    [--mode <plan|ask|auto-edit|yolo>] [--continue | --resume <ID>] [--env-file <PATH>]\n          \
    [--agent <ID>] [--skill <ID>]... [-h|--help] [-V|--version]\n  \
    meerai --prompt <TEXT> [--json] [--continue | --resume <ID>] [--mode <MODE>]\n          \
    [--agent <ID>] [--skill <ID>]...\n  \
    meerai auth <list|login|logout>\n  \
    meerai sessions <list|rm|prune>\n\
    \n\
    Commands:\n  \
    auth list                       Show credentials across auth.json, config, and env\n  \
    auth login <provider> [key]     Save an API key in ~/.local/share/meerai/auth.json\n  \
    auth logout <provider>          Remove a stored API key from auth.json\n  \
    sessions list                   List saved sessions: id, updated ms, bytes, title/model\n  \
    sessions rm <ID>                Delete one session\n  \
    sessions rm --all               Delete every saved session\n  \
    sessions prune [--days N] [--keep N]\n                                              \
    Delete sessions older than N days (default 30), keeping the newest N (default 0)\n\
    \n\
    TUI commands:\n  \
    /agent <id>                     Apply an agent by id (or mention @agent/<id>)\n  \
    /skill <id>                     Apply a skill by id (or mention @skill/<id>)\n\
    \n\
    Options:\n  \
    -p, --provider <PROVIDER>  Provider id (env: MEERAI_PROVIDER)\n  \
    -m, --model <MODEL>        Model id (env: MEERAI_MODEL)\n  \
        --system <PROMPT>      Additional system instructions\n  \
        --prompt, --print <TEXT>  Run one headless turn (no TUI)\n  \
        --continue             Resume the most recent session (TUI or headless)\n  \
        --resume <ID>          Resume a session by id (TUI or headless)\n  \
        --json                 Print one NDJSON AgentEvent per line (with --prompt)\n  \
        --mode <MODE>          Approval mode: plan, ask, auto-edit, yolo\n  \
        --agent <ID>           Apply a markdown agent by id\n  \
        --skill <ID>           Apply a markdown skill by id (repeatable)\n  \
        --env-file <PATH>         Load variables from PATH (no implicit .env loading)\n  \
    -h, --help                 Show this help message\n  \
    -V, --version              Print version and exit";

#[derive(Debug, PartialEq, Eq)]
enum AuthCommand {
    List,
    Login {
        provider: String,
        key: Option<String>,
    },
    Logout {
        provider: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum SessionTarget {
    Id(String),
    All,
}

#[derive(Debug, PartialEq, Eq)]
enum SessionsCommand {
    List,
    Remove { target: SessionTarget },
    Prune { days: u64, keep: usize },
}

#[derive(Debug, PartialEq, Eq)]
enum CliSubcommand {
    Auth(AuthCommand),
    Sessions(SessionsCommand),
}

#[derive(Debug)]
struct CliArgs {
    subcommand: Option<CliSubcommand>,
    provider: Option<String>,
    model: Option<String>,
    system: String,
    prompt: Option<String>,
    json: bool,
    mode: Option<String>,
    env_file: Option<String>,
    continue_session: bool,
    resume: Option<String>,
    agents: Vec<String>,
    skills: Vec<String>,
    show_help: bool,
    version: bool,
}

fn parse_count(flag: &str, raw: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("invalid value for {flag}: {raw} (expected a number)"))
}

fn parse_keep(raw: &str) -> Result<usize> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("invalid value for --keep: {raw} (expected a number)"))
}

fn parse_args<I>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut subcommand = None;
    let mut provider = None;
    let mut model = None;
    let mut system = String::new();
    let mut prompt = None;
    let mut json = false;
    let mut mode = None;
    let mut env_file = None;
    let mut continue_session = false;
    let mut resume = None;
    let mut agents = Vec::new();
    let mut skills = Vec::new();
    let mut show_help = false;
    let mut version = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                show_help = true;
            }
            "-V" | "--version" => {
                version = true;
            }
            "auth" if subcommand.is_none() => match iter.next().as_deref() {
                None | Some("list") | Some("ls") => {
                    if let Some(extra) = iter.next() {
                        match extra.as_str() {
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for auth list: {extra}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Auth(AuthCommand::List));
                }
                Some("-h" | "--help") => {
                    show_help = true;
                }
                Some("login") => {
                    let target_provider = iter.next().ok_or_else(|| {
                        anyhow::anyhow!("usage: meerai auth login <provider> [api-key]")
                    })?;
                    if target_provider == "-h" || target_provider == "--help" {
                        show_help = true;
                        continue;
                    }
                    let key = iter.next();
                    if let Some(extra) = iter.next() {
                        bail!("unexpected argument for auth login: {extra}");
                    }
                    subcommand = Some(CliSubcommand::Auth(AuthCommand::Login {
                        provider: target_provider,
                        key,
                    }));
                }
                Some("logout") => {
                    let target_provider = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai auth logout <provider>"))?;
                    if target_provider == "-h" || target_provider == "--help" {
                        show_help = true;
                        continue;
                    }
                    if let Some(extra) = iter.next() {
                        bail!("unexpected argument for auth logout: {extra}");
                    }
                    subcommand = Some(CliSubcommand::Auth(AuthCommand::Logout {
                        provider: target_provider,
                    }));
                }
                Some(other) => {
                    bail!("unknown auth subcommand: {other}. Expected: list, login, logout");
                }
            },
            "sessions" if subcommand.is_none() => match iter.next().as_deref() {
                None | Some("list") | Some("ls") => {
                    if let Some(extra) = iter.next() {
                        match extra.as_str() {
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for sessions list: {extra}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Sessions(SessionsCommand::List));
                }
                Some("-h" | "--help") => {
                    show_help = true;
                }
                Some("rm") => {
                    let mut id = None;
                    let mut all = false;
                    let mut help = false;
                    for arg in iter.by_ref() {
                        match arg.as_str() {
                            "-h" | "--help" => help = true,
                            "--all" => all = true,
                            flag if flag.starts_with('-') => {
                                bail!("unknown option for sessions rm: {flag}")
                            }
                            _ => {
                                if id.is_some() {
                                    bail!("unexpected argument for sessions rm: {arg}");
                                }
                                id = Some(arg);
                            }
                        }
                    }
                    if help {
                        show_help = true;
                        continue;
                    }
                    let target = match (id, all) {
                        (Some(_), true) => {
                            bail!("sessions rm takes a session id or --all, not both")
                        }
                        (None, false) => bail!(
                            "sessions rm needs a session id or --all\n\
                             usage: meerai sessions rm <ID>\n       \
                             meerai sessions rm --all"
                        ),
                        (Some(id), false) => SessionTarget::Id(id),
                        (None, true) => SessionTarget::All,
                    };
                    subcommand = Some(CliSubcommand::Sessions(SessionsCommand::Remove { target }));
                }
                Some("prune") => {
                    let mut days: u64 = 30;
                    let mut keep: usize = 0;
                    let mut help = false;
                    while let Some(arg) = iter.next() {
                        match arg.as_str() {
                            "-h" | "--help" => help = true,
                            "--days" => {
                                let value = iter
                                    .next()
                                    .ok_or_else(|| anyhow::anyhow!("missing value for --days"))?;
                                days = parse_count("--days", &value)?;
                            }
                            "--keep" => {
                                let value = iter
                                    .next()
                                    .ok_or_else(|| anyhow::anyhow!("missing value for --keep"))?;
                                keep = parse_keep(&value)?;
                            }
                            flag if flag.starts_with("--days=") => {
                                days = parse_count("--days", &flag["--days=".len()..])?;
                            }
                            flag if flag.starts_with("--keep=") => {
                                keep = parse_keep(&flag["--keep=".len()..])?;
                            }
                            flag if flag.starts_with('-') => {
                                bail!("unknown option for sessions prune: {flag}")
                            }
                            other => bail!("unexpected argument for sessions prune: {other}"),
                        }
                    }
                    if help {
                        show_help = true;
                        continue;
                    }
                    subcommand = Some(CliSubcommand::Sessions(SessionsCommand::Prune {
                        days,
                        keep,
                    }));
                }
                Some(other) => {
                    bail!("unknown sessions subcommand: {other}. Expected: list, rm, prune");
                }
            },
            "-p" | "--provider" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
                provider = Some(value);
            }
            "-m" | "--model" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
                model = Some(value);
            }
            "--system" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --system"))?;
                system = value;
            }
            "--prompt" | "--print" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
                prompt = Some(value);
            }
            "--json" => {
                json = true;
            }
            "--continue" => {
                continue_session = true;
            }
            "--resume" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --resume"))?;
                resume = Some(value);
            }
            "--mode" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --mode"))?;
                mode = Some(value);
            }
            "--env-file" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --env-file"))?;
                env_file = Some(value);
            }
            "--agent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --agent"))?;
                agents.push(value);
            }
            "--skill" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --skill"))?;
                skills.push(value);
            }
            _ if arg.starts_with("--provider=") => {
                provider = Some(arg["--provider=".len()..].to_string());
            }
            _ if arg.starts_with("-p=") => {
                provider = Some(arg["-p=".len()..].to_string());
            }
            _ if arg.starts_with("--model=") => {
                model = Some(arg["--model=".len()..].to_string());
            }
            _ if arg.starts_with("-m=") => {
                model = Some(arg["-m=".len()..].to_string());
            }
            _ if arg.starts_with("--system=") => {
                system = arg["--system=".len()..].to_string();
            }
            _ if arg.starts_with("--prompt=") => {
                prompt = Some(arg["--prompt=".len()..].to_string());
            }
            _ if arg.starts_with("--print=") => {
                prompt = Some(arg["--print=".len()..].to_string());
            }
            _ if arg.starts_with("--resume=") => {
                resume = Some(arg["--resume=".len()..].to_string());
            }
            _ if arg.starts_with("--mode=") => {
                mode = Some(arg["--mode=".len()..].to_string());
            }
            _ if arg.starts_with("--env-file=") => {
                env_file = Some(arg["--env-file=".len()..].to_string());
            }
            _ if arg.starts_with("--agent=") => {
                agents.push(arg["--agent=".len()..].to_string());
            }
            _ if arg.starts_with("--skill=") => {
                skills.push(arg["--skill=".len()..].to_string());
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    if json && prompt.is_none() {
        bail!("--json requires --prompt or --print");
    }
    if continue_session && resume.is_some() {
        bail!("--continue and --resume cannot be combined");
    }
    if prompt.is_some() && subcommand.is_some() {
        bail!("--prompt/--print cannot be combined with a subcommand");
    }
    if agents.len() > 1 {
        bail!("--agent can be given only once");
    }

    Ok(CliArgs {
        subcommand,
        provider,
        model,
        system,
        prompt,
        json,
        mode,
        env_file,
        continue_session,
        resume,
        agents,
        skills,
        show_help,
        version,
    })
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn pick_model(
    cli_model: Option<&str>,
    env_model: Option<&str>,
    agent_model: Option<&str>,
    configured_model: Option<&str>,
) -> Option<String> {
    [cli_model, env_model, agent_model, configured_model]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|candidate| !candidate.is_empty())
        .map(str::to_string)
}

fn agent_model_candidate(agent_model: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(raw) = agent_model.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return (None, None);
    };
    if raw.contains(':') {
        return (
            None,
            Some(format!(
                "agent model '{raw}' implies a provider change; ignoring it"
            )),
        );
    }
    (Some(raw.to_string()), None)
}

fn select_definition<'a, T>(
    flag: &str,
    kind: &str,
    id: &str,
    available: &'a [T],
    id_of: impl Fn(&T) -> &str,
) -> Result<&'a T> {
    if let Some(found) = available.iter().find(|item| id_of(item) == id) {
        return Ok(found);
    }
    let ids: Vec<&str> = available.iter().map(id_of).collect();
    let list = if ids.is_empty() {
        "(none)".to_string()
    } else {
        ids.join(", ")
    };
    bail!("unknown {flag} id '{id}'; available {kind}: {list}")
}

fn compose_definition_prompt(
    mut system: String,
    skills: &[&SkillDefinition],
    agent: Option<&AgentDefinition>,
) -> String {
    for skill in skills {
        system.push_str(&format!(
            "\n\n## Skill instructions ({})\n\n{}",
            skill.id, skill.body
        ));
    }
    if let Some(agent) = agent {
        system.push_str(&format!(
            "\n\n## Agent instructions ({})\n\n{}",
            agent.id, agent.prompt
        ));
    }
    system
}

fn dotenv_flag_enabled(value: Option<&str>) -> bool {
    match value.map(str::trim).map(str::to_ascii_lowercase) {
        Some(flag) => matches!(flag.as_str(), "1" | "true" | "yes"),
        None => false,
    }
}

fn load_dotenv(explicit: Option<&Path>) -> Result<()> {
    if let Some(path) = explicit {
        if !path.is_file() {
            bail!("env file not found: {}", path.display());
        }
        dotenvy::from_path(path)
            .map_err(|err| anyhow::anyhow!("failed to load env file {}: {err}", path.display()))?;
        return Ok(());
    }
    let value = std::env::var("MEERAI_LOAD_DOTENV").ok();
    if dotenv_flag_enabled(value.as_deref()) {
        dotenvy::dotenv().ok();
    }
    Ok(())
}

fn session_label(summary: &SessionSummary) -> &str {
    summary
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .or_else(|| summary.model.as_deref().filter(|model| !model.is_empty()))
        .unwrap_or("")
}

fn format_session_row(summary: &SessionSummary) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        summary.id,
        summary.updated_at_ms,
        summary.bytes,
        session_label(summary)
    )
}

fn select_prunable(
    sessions: &[SessionSummary],
    now_ms: u64,
    days: u64,
    keep: usize,
    exclude: Option<&SessionId>,
) -> Vec<SessionId> {
    let cutoff = now_ms.saturating_sub(days.saturating_mul(86_400_000));
    let mut ordered: Vec<&SessionSummary> = sessions.iter().collect();
    ordered.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at_ms));
    ordered
        .into_iter()
        .skip(keep)
        .filter(|summary| Some(&summary.id) != exclude)
        .filter(|summary| summary.updated_at_ms < cutoff)
        .map(|summary| summary.id.clone())
        .collect()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

async fn run_blocking<T>(task: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|err| anyhow::anyhow!("session task failed: {err}"))?
}

fn auto_prune_config(settings: &AppSettings) -> Option<(u64, usize)> {
    let sessions = settings.sessions.as_ref()?;
    let days = sessions.auto_prune_days?;
    Some((days, sessions.keep.unwrap_or(0)))
}

async fn prune_old_sessions(
    root: std::path::PathBuf,
    days: u64,
    keep: usize,
    exclude: &SessionId,
) -> Result<usize> {
    let list_root = root.clone();
    let sessions = run_blocking(move || list_sessions(&list_root)).await?;
    let ids = select_prunable(&sessions, now_millis(), days, keep, Some(exclude));
    let count = ids.len();
    if count == 0 {
        return Ok(0);
    }
    run_blocking(move || {
        for id in &ids {
            delete_session(&root, id)?;
        }
        Ok(())
    })
    .await?;
    Ok(count)
}

fn format_prune_message(count: usize, days: u64) -> String {
    format!("Pruned {count} sessions (older than {days} days)")
}

async fn auto_prune_sessions(settings: &AppSettings, exclude: &SessionId) {
    let Some((days, keep)) = auto_prune_config(settings) else {
        return;
    };
    match prune_old_sessions(default_sessions_root(), days, keep, exclude).await {
        Ok(0) => {}
        Ok(count) => eprintln!("{}", format_prune_message(count, days)),
        Err(err) => eprintln!("Warning: automatic session prune failed: {err:#}"),
    }
}

async fn run_sessions(command: &SessionsCommand) -> Result<()> {
    let root = default_sessions_root();
    match command {
        SessionsCommand::List => {
            let list_root = root.clone();
            let sessions = run_blocking(move || list_sessions(&list_root)).await?;
            println!("ID\tUPDATED_MS\tBYTES\tTITLE_OR_MODEL");
            for summary in sessions {
                println!("{}", format_session_row(&summary));
            }
        }
        SessionsCommand::Remove {
            target: SessionTarget::Id(raw),
        } => {
            let id = SessionId::parse(raw)?;
            let delete_root = root.clone();
            let delete_id = id.clone();
            let removed = run_blocking(move || delete_session(&delete_root, &delete_id)).await?;
            if !removed {
                bail!("session not found: {id}");
            }
            println!("removed {id}");
        }
        SessionsCommand::Remove {
            target: SessionTarget::All,
        } => {
            let list_root = root.clone();
            let sessions = run_blocking(move || list_sessions(&list_root)).await?;
            let count = sessions.len();
            let ids: Vec<SessionId> = sessions.into_iter().map(|summary| summary.id).collect();
            let delete_root = root.clone();
            run_blocking(move || {
                for id in &ids {
                    delete_session(&delete_root, id)?;
                }
                Ok(())
            })
            .await?;
            println!("removed {count} sessions");
        }
        SessionsCommand::Prune { days, keep } => {
            let list_root = root.clone();
            let sessions = run_blocking(move || list_sessions(&list_root)).await?;
            let ids = select_prunable(&sessions, now_millis(), *days, *keep, None);
            let count = ids.len();
            let delete_root = root.clone();
            run_blocking(move || {
                for id in &ids {
                    delete_session(&delete_root, id)?;
                }
                Ok(())
            })
            .await?;
            println!("removed {count} sessions");
        }
    }
    Ok(())
}

async fn continue_session_id(root: &Path) -> Result<SessionId> {
    let root = root.to_path_buf();
    run_blocking(move || latest_session(&root))
        .await?
        .map(|summary| summary.id)
        .ok_or_else(|| anyhow::anyhow!("no sessions found to continue"))
}

fn resolve_approval_mode(flag: Option<&str>, configured: Option<&str>) -> Result<ApprovalMode> {
    if let Some(raw) = flag {
        return parse_approval_mode(raw).ok_or_else(|| {
            anyhow::anyhow!("invalid --mode value '{raw}': expected plan, ask, auto-edit, yolo")
        });
    }
    match configured {
        Some(raw) => parse_approval_mode(raw).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid approval_mode '{raw}' in settings.yml: expected plan, ask, auto-edit, yolo"
            )
        }),
        None => Ok(ApprovalMode::Ask),
    }
}

fn build_policy(
    mode: ApprovalMode,
    permissions: Option<&serde_yaml::Value>,
    workspace: &Path,
) -> Result<PolicyEngine> {
    let mut policy = PolicyEngine::new(mode);
    if let Some(value) = permissions {
        policy = policy.with_rules(rules_from_yaml(value)?);
    }
    Ok(policy.with_workspace_root(workspace.to_path_buf()))
}

fn harness_config(context: Option<&ContextSettings>) -> HarnessConfig {
    let mut config = HarnessConfig::default();
    if let Some(context) = context {
        if let Some(value) = context.context_window_tokens {
            config.context_window_tokens = value;
        }
        if let Some(value) = context.high_watermark_pct {
            config.high_watermark_pct = value;
        }
        if let Some(value) = context.spill_max_inline_tokens {
            config.spill_max_inline_tokens = value;
        }
        if let Some(value) = context.verbatim_window_turns {
            config.verbatim_window_turns = value;
        }
        if let Some(value) = context.reasoning_pruning {
            config.reasoning_pruning = value;
        }
        if let Some(value) = context.max_steps_per_turn {
            config.max_steps_per_turn = value;
        }
        if let Some(value) = context.tool_timeout_ms {
            config.tool_timeout_ms = value;
        }
        if let Some(value) = context.max_session_tokens {
            config.max_session_tokens = Some(value);
        }
        if let Some(value) = context.max_turn_secs {
            config.max_turn_secs = value;
        }
        if let Some(value) = &context.summarizer_model {
            config.summarizer_model = Some(value.clone());
        }
    }
    config
}

fn item_status_label(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Proposed => "proposed",
        ItemStatus::Pending => "pending",
        ItemStatus::InProgress => "in-progress",
        ItemStatus::Succeeded => "succeeded",
        ItemStatus::Failed => "failed",
        ItemStatus::Stopped => "stopped",
        ItemStatus::Denied => "denied",
        ItemStatus::Cancelled => "cancelled",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadlessOutcome {
    Continue,
    ApprovalRequired,
    SpendDeclined,
    Completed,
    Failed,
}

fn headless_outcome(event: &AgentEvent) -> HeadlessOutcome {
    match event {
        AgentEvent::ApprovalRequested { request } => match request.kind {
            ApprovalKind::Spend => HeadlessOutcome::SpendDeclined,
            _ => HeadlessOutcome::ApprovalRequired,
        },
        AgentEvent::TurnCompleted { reason, .. } => match reason {
            meerai_agents::TurnEndReason::Completed => HeadlessOutcome::Completed,
            _ => HeadlessOutcome::Failed,
        },
        AgentEvent::Error { .. } => HeadlessOutcome::Failed,
        _ => HeadlessOutcome::Continue,
    }
}

fn format_token_summary(usage: &meerai_agents::Usage) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(prompt) = usage.prompt_tokens {
        parts.push(format!("prompt={prompt}"));
    }
    if let Some(completion) = usage.completion_tokens {
        parts.push(format!("completion={completion}"));
    }
    if let Some(total) = usage.total_tokens {
        parts.push(format!("total={total}"));
    }
    (!parts.is_empty()).then(|| format!("tokens: {}", parts.join(" ")))
}

#[derive(Debug, PartialEq)]
enum HeadlessApprovalAction {
    Cancel,
    Decline { request_id: RequestId },
}

impl HeadlessApprovalAction {
    fn for_request(request: &ApprovalRequest) -> Self {
        match request.kind {
            ApprovalKind::Spend => Self::Decline {
                request_id: request.request_id.clone(),
            },
            _ => Self::Cancel,
        }
    }

    fn command(&self) -> AgentCommand {
        match self {
            Self::Cancel => AgentCommand::Cancel,
            Self::Decline { request_id } => AgentCommand::Approve {
                request_id: request_id.clone(),
                decision: ApprovalDecision::Decline,
                scope: None,
            },
        }
    }
}

struct HeadlessExit {
    code: i32,
    approval_cancelled: bool,
    failed: bool,
}

impl HeadlessExit {
    fn new() -> Self {
        Self {
            code: 1,
            approval_cancelled: false,
            failed: false,
        }
    }

    fn mark_failed(&mut self) {
        self.failed = true;
        if !self.approval_cancelled {
            self.code = 1;
        }
    }

    fn observe(&mut self, outcome: HeadlessOutcome) {
        match outcome {
            HeadlessOutcome::Continue => {}
            HeadlessOutcome::SpendDeclined | HeadlessOutcome::Failed => {
                self.mark_failed();
            }
            HeadlessOutcome::ApprovalRequired => {
                self.code = EXIT_APPROVAL_REQUIRED;
                self.approval_cancelled = true;
            }
            HeadlessOutcome::Completed => {
                if !self.approval_cancelled && !self.failed {
                    self.code = 0;
                }
            }
        }
    }
}

async fn run_headless(mut agent: MultiTurnAgent, prompt: String, json: bool) -> Result<i32> {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let turn = tokio::spawn(async move {
        agent
            .prompt_streaming_with_commands(
                &prompt,
                &event_tx,
                CancellationToken::new(),
                &mut cmd_rx,
            )
            .await
    });

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut tool_names: HashMap<ItemId, String> = HashMap::new();
    let mut streamed_text: HashSet<ItemId> = HashSet::new();
    let mut exit = HeadlessExit::new();
    let mut line_open = false;
    let mut last_usage: Option<meerai_agents::Usage> = None;

    while let Some(event) = event_rx.recv().await {
        if let AgentEvent::ApprovalRequested { request } = &event {
            let action = HeadlessApprovalAction::for_request(request);
            match &action {
                HeadlessApprovalAction::Decline { .. } => eprintln!(
                    "session token budget reached; raise context.max_session_tokens in settings.yml or unset it"
                ),
                HeadlessApprovalAction::Cancel => {
                    eprintln!("approval required: {}", request.preview.title);
                    eprintln!("rerun with --mode auto-edit or --mode yolo, or use the TUI");
                }
            }
            let _ = cmd_tx.send(action.command());
        }
        exit.observe(headless_outcome(&event));

        if json {
            writeln!(out, "{}", serde_json::to_string(&event)?)?;
            out.flush()?;
            continue;
        }

        match &event {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::ToolCall {
                        id, name, status, ..
                    },
            } => {
                tool_names.insert(id.clone(), name.clone());
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                writeln!(out, "▸ {name} {}", item_status_label(*status))?;
                out.flush()?;
            }
            AgentEvent::ItemDelta {
                item_id,
                delta: meerai_agents::ItemDelta::AgentText(chunk),
            } => {
                if !chunk.is_empty() {
                    streamed_text.insert(item_id.clone());
                    write!(out, "{chunk}")?;
                    out.flush()?;
                    line_open = !chunk.ends_with('\n');
                }
            }
            AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage { id, text },
            } => {
                if !streamed_text.remove(id) && !text.is_empty() {
                    if line_open {
                        writeln!(out)?;
                    }
                    writeln!(out, "{text}")?;
                    out.flush()?;
                    line_open = false;
                }
            }
            AgentEvent::ItemDelta {
                item_id,
                delta: meerai_agents::ItemDelta::StatusChanged(status),
            } => {
                if let Some(name) = tool_names.get(item_id) {
                    if line_open {
                        writeln!(out)?;
                        line_open = false;
                    }
                    writeln!(out, "▸ {name} {}", item_status_label(*status))?;
                    out.flush()?;
                }
            }
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        id, name, status, ..
                    },
            } => {
                tool_names.remove(id);
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                writeln!(out, "▸ {name} {}", item_status_label(*status))?;
                out.flush()?;
            }
            AgentEvent::Error { message, .. } => {
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                eprintln!("error: {message}");
            }
            AgentEvent::TurnCompleted { reason, .. } => {
                if line_open {
                    writeln!(out)?;
                    line_open = false;
                }
                if let meerai_agents::TurnEndReason::Failed { error } = reason {
                    eprintln!("error: {error}");
                }
            }
            AgentEvent::UsageUpdated { cumulative, .. } => {
                last_usage = Some(*cumulative);
            }
            _ => {}
        }
    }

    if line_open {
        writeln!(out)?;
    }
    out.flush()?;
    drop(out);

    match turn.await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            eprintln!("error: {err:#}");
            exit.mark_failed();
        }
        Err(err) => {
            eprintln!("error: headless turn task failed: {err}");
            exit.mark_failed();
        }
    }

    if !json && let Some(summary) = last_usage.as_ref().and_then(format_token_summary) {
        eprintln!("{summary}");
    }

    Ok(exit.code)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = parse_args(std::env::args().skip(1))?;
    load_dotenv(cli.env_file.as_deref().map(Path::new))?;

    if cli.show_help {
        println!("{HELP_TEXT}");
        return Ok(());
    }

    if cli.version {
        println!("meerai {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let Some(CliSubcommand::Sessions(command)) = cli.subcommand.as_ref() {
        return run_sessions(command).await;
    }

    let workspace_root = std::env::current_dir()?;
    let project_root = find_project_root(&workspace_root);
    let definitions = load_definitions(&project_root);
    for warning in &definitions.warnings {
        eprintln!("Warning: {warning}");
    }
    let project_settings_path = project_root.join(".meerai").join("settings.yml");
    let config_dir = default_config_dir();
    let data_dir = default_data_dir();
    let mut user_settings = config_dir
        .as_deref()
        .and_then(|dir| load_settings_from(dir).ok())
        .unwrap_or_default();
    let mut settings = user_settings.clone();
    match load_project_settings(&project_root) {
        Ok(Some(project)) => {
            let (merged, warnings) = merge_project_settings(user_settings.clone(), project);
            for warning in warnings {
                eprintln!("Warning: {warning}");
            }
            settings = merged;
        }
        Ok(None) => {}
        Err(err) => {
            eprintln!(
                "Warning: ignoring project settings ({}): {err:#}",
                project_settings_path.display()
            );
        }
    }
    let (mut catalog, warnings) =
        ProviderCatalog::load_with_user_dir_checked(config_dir.as_deref());
    for warning in warnings {
        eprintln!("Warning: {warning}");
    }
    catalog.apply_settings(&settings);
    let secrets = default_secret_store(data_dir.as_deref());
    let auth_path = data_dir.as_deref().map(auth_file_path);

    if let Some(CliSubcommand::Auth(auth_cmd)) = cli.subcommand {
        match auth_cmd {
            AuthCommand::List => {
                println!(
                    "{}",
                    format_auth_list(
                        &catalog,
                        secrets.as_ref(),
                        auth_path.as_deref(),
                        config_dir.as_deref(),
                    )
                );
            }
            AuthCommand::Login { provider, key } => {
                let provider_id = canonical_provider_id(&provider);
                let api_key = match key {
                    Some(k) => k,
                    None => {
                        print!("API key for {provider_id}: ");
                        io::stdout().flush()?;
                        let mut input = String::new();
                        io::stdin().read_line(&mut input)?;
                        input
                    }
                };
                secrets.set_key(&provider_id, &api_key)?;
                let target = auth_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "auth.json".to_string());
                println!("Saved credentials for {provider_id} in {target}");
            }
            AuthCommand::Logout { provider } => {
                let provider_id = canonical_provider_id(&provider);
                secrets.delete_key(&provider_id)?;
                println!("Removed stored credentials for {provider_id}");
            }
        }
        return Ok(());
    }

    let mut selected_agents = Vec::new();
    for id in &cli.agents {
        selected_agents.push(select_definition(
            "--agent",
            "agents",
            id,
            &definitions.agents,
            |agent| agent.id.as_str(),
        )?);
    }
    let mut selected_skills = Vec::new();
    for id in &cli.skills {
        selected_skills.push(select_definition(
            "--skill",
            "skills",
            id,
            &definitions.skills,
            |skill| skill.id.as_str(),
        )?);
    }
    let selected_agent = selected_agents.first().copied();
    if let Some(agent) = selected_agent {
        let mut unapplied = Vec::new();
        if agent.steps.is_some() {
            unapplied.push("steps");
        }
        if agent.mode.is_some() {
            unapplied.push("mode");
        }
        if !unapplied.is_empty() {
            eprintln!(
                "Note: agent '{}' sets {}; those fields are not applied yet",
                agent.id,
                unapplied.join(" and ")
            );
        }
    }

    if cli.prompt.is_none() && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        bail!("no terminal detected; use --prompt \"...\" for headless mode (see --help)");
    }

    let active_provider = cli
        .provider
        .or_else(|| non_empty_env("MEERAI_PROVIDER"))
        .or_else(|| settings.active_provider.clone())
        .or_else(|| first_configured_provider(&catalog, secrets.as_ref()))
        .map(|p| canonical_provider_id(&p))
        .unwrap_or_else(|| FALLBACK_PROVIDER.to_string());

    let (agent_model, agent_model_warning) =
        agent_model_candidate(selected_agent.and_then(|agent| agent.model.as_deref()));
    if let Some(warning) = agent_model_warning {
        eprintln!("Warning: {warning}");
    }

    let active_model = pick_model(
        cli.model.as_deref(),
        non_empty_env("MEERAI_MODEL").as_deref(),
        agent_model.as_deref(),
        settings.active_model.as_deref(),
    )
    .or_else(|| {
        settings
            .provider_model(&active_provider)
            .map(str::to_string)
    })
    .or_else(|| catalog.default_model_for_provider(&active_provider))
    .unwrap_or_default();

    settings.active_provider = Some(active_provider.clone());
    settings.active_model = (!active_model.trim().is_empty()).then(|| active_model.clone());
    user_settings.active_provider = Some(active_provider.clone());
    user_settings.active_model = (!active_model.trim().is_empty()).then(|| active_model.clone());

    let model_spec = if active_model.trim().is_empty() {
        String::new()
    } else {
        catalog.format_genai_spec(&active_provider, &active_model)
    };
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings, catalog,
    )));
    let client = build_genai_client(runtime.clone(), secrets.clone())?;

    let approval_mode =
        resolve_approval_mode(cli.mode.as_deref(), settings.approval_mode.as_deref())?;
    let policy = build_policy(
        approval_mode,
        settings.permissions.as_ref(),
        &workspace_root,
    )?;
    for warning in policy.warnings() {
        eprintln!("Warning: {warning}");
    }

    let session_config = SessionConfig::new(workspace_root.clone());
    let session = if cli.continue_session {
        let root = default_sessions_root();
        let id = continue_session_id(&root).await?;
        Session::open(&session_config, id, workspace_root.clone()).await?
    } else if let Some(raw) = cli.resume.as_deref() {
        let id = SessionId::parse(raw)?;
        Session::open(&session_config, id, workspace_root.clone()).await?
    } else {
        Session::open_or_create(&session_config, SessionId::new(), workspace_root.clone()).await?
    };
    if !active_model.trim().is_empty() {
        session.set_model(&active_model).await?;
    }
    auto_prune_sessions(&settings, session.id()).await;
    let initial_events = if cli.prompt.is_none() {
        session.events()
    } else {
        Vec::new()
    };
    let initial_transcript = transcript_from_events(&initial_events);
    let mut readable_roots = vec![workspace_root.clone()];
    if let Some(dir) = session.dir() {
        readable_roots.push(dir.to_path_buf());
    }
    let code_tools = CodeTools::new(workspace_root.clone())
        .with_writable_roots(vec![workspace_root.clone()])
        .with_readable_roots(readable_roots);

    let system = compose_definition_prompt(
        meerai_agents::compose_system_prompt(&cli.system, &workspace_root).await,
        &selected_skills,
        selected_agent,
    );
    let mut agent = MultiTurnAgent::new(client, model_spec, vec![Box::new(code_tools)], system);
    agent.set_harness(AgentHarness {
        session,
        policy,
        config: harness_config(settings.context.as_ref()),
    });

    if let Some(prompt) = cli.prompt {
        let exit_code = run_headless(agent, prompt, cli.json).await?;
        std::process::exit(exit_code);
    }

    let (cmd_tx, event_rx) = spawn_worker(agent);
    meerai_tui::run_tui(
        cmd_tx,
        event_rx,
        TuiLaunchConfig {
            initial_provider: active_provider,
            initial_model: active_model,
            workspace_root,
            runtime,
            secrets,
            settings: user_settings,
            config_dir,
            approval_mode,
            initial_transcript,
            initial_events,
            agents: definitions.agents,
            skills: definitions.skills,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_model_and_system_flags() {
        let parsed = parse_args(vec![
            "-p".to_string(),
            "opencode-go".to_string(),
            "-m".to_string(),
            "glm-5.3".to_string(),
            "--system".to_string(),
            "Be concise.".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.provider.as_deref(), Some("opencode-go"));
        assert_eq!(parsed.model.as_deref(), Some("glm-5.3"));
        assert_eq!(parsed.system, "Be concise.");
        assert!(!parsed.show_help);

        let equals_parsed = parse_args(vec![
            "--provider=openrouter".to_string(),
            "--model=claude-sonnet-4-5".to_string(),
            "--system=Test".to_string(),
        ])
        .unwrap();
        assert_eq!(equals_parsed.provider.as_deref(), Some("openrouter"));
        assert_eq!(equals_parsed.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(equals_parsed.system, "Test");

        assert!(parse_args(vec!["--unknown".to_string()]).is_err());
    }

    #[test]
    fn parses_headless_and_mode_flags() {
        let prompt = parse_args(vec!["--prompt".to_string(), "hi".to_string()]).unwrap();
        assert_eq!(prompt.prompt.as_deref(), Some("hi"));
        assert!(!prompt.json);
        assert_eq!(prompt.provider, None);

        let print_alias = parse_args(vec!["--print=hello".to_string()]).unwrap();
        assert_eq!(print_alias.prompt.as_deref(), Some("hello"));

        let json = parse_args(vec![
            "--prompt".to_string(),
            "hi".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert!(json.json);

        assert!(parse_args(vec!["--json".to_string()]).is_err());

        let mode = parse_args(vec![
            "--prompt".to_string(),
            "hi".to_string(),
            "--mode=yolo".to_string(),
        ])
        .unwrap();
        assert_eq!(mode.mode.as_deref(), Some("yolo"));

        let provider = parse_args(vec!["-p".to_string(), "gemini".to_string()]).unwrap();
        assert_eq!(provider.provider.as_deref(), Some("gemini"));

        assert!(
            parse_args(vec![
                "auth".to_string(),
                "list".to_string(),
                "--prompt".to_string(),
                "hi".to_string(),
            ])
            .is_err()
        );
        assert!(parse_args(vec!["--mode".to_string()]).is_err());
        assert!(parse_args(vec!["--prompt".to_string()]).is_err());
    }

    #[test]
    fn parses_version_flag() {
        let long = parse_args(vec!["--version".to_string()]).unwrap();
        assert!(long.version);
        assert!(!long.show_help);
        assert_eq!(long.prompt, None);

        let short = parse_args(vec!["-V".to_string()]).unwrap();
        assert!(short.version);

        let prompt = parse_args(vec!["--prompt".to_string(), "hi".to_string()]).unwrap();
        assert!(!prompt.version);
    }

    #[test]
    fn parses_env_file_flag_and_reports_missing_path() {
        let spaced = parse_args(vec!["--env-file".to_string(), ".env.local".to_string()]).unwrap();
        assert_eq!(spaced.env_file.as_deref(), Some(".env.local"));

        let equals = parse_args(vec!["--env-file=/tmp/meerai.env".to_string()]).unwrap();
        assert_eq!(equals.env_file.as_deref(), Some("/tmp/meerai.env"));

        assert!(parse_args(vec!["--env-file".to_string()]).is_err());

        let missing = std::env::temp_dir().join("meerai-r1-missing-env-file.env");
        let error = load_dotenv(Some(missing.as_path())).unwrap_err();
        assert!(error.to_string().contains("env file not found"));
    }

    #[test]
    fn parses_agent_and_skill_flags() {
        let parsed = parse_args(vec![
            "--agent".to_string(),
            "reviewer".to_string(),
            "--skill".to_string(),
            "explore".to_string(),
            "--skill=fix".to_string(),
            "--skill=deploy".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.agents, vec!["reviewer"]);
        assert_eq!(parsed.skills, vec!["explore", "fix", "deploy"]);

        assert!(parse_args(vec!["--agent".to_string()]).is_err());
        assert!(
            parse_args(vec![
                "--agent".to_string(),
                "a".to_string(),
                "--agent".to_string(),
                "b".to_string(),
            ])
            .is_err()
        );
        assert!(parse_args(vec!["--skill".to_string()]).is_err());
    }

    fn test_agent(id: &str, model: Option<&str>, steps: Option<usize>) -> AgentDefinition {
        AgentDefinition {
            id: id.to_string(),
            source: meerai_common::definitions::DefinitionSource::Agents,
            description: None,
            model: model.map(str::to_string),
            steps,
            mode: None,
            prompt: format!("Prompt for {id}."),
            path: std::path::PathBuf::from(format!(".agents/agents/{id}.md")),
        }
    }

    fn test_skill(id: &str) -> SkillDefinition {
        SkillDefinition {
            id: id.to_string(),
            source: meerai_common::definitions::DefinitionSource::Claude,
            description: None,
            body: format!("Body for {id}."),
            path: std::path::PathBuf::from(format!(".claude/skills/{id}/SKILL.md")),
        }
    }

    #[test]
    fn unknown_definition_ids_error_with_available_ids() {
        let agents = vec![
            test_agent("reviewer", None, None),
            test_agent("writer", None, None),
        ];
        let found = select_definition("--agent", "agents", "writer", &agents, |agent| {
            agent.id.as_str()
        })
        .unwrap();
        assert_eq!(found.id, "writer");

        let error = select_definition("--agent", "agents", "missing", &agents, |agent| {
            agent.id.as_str()
        })
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("unknown --agent id 'missing'"));
        assert!(message.contains("reviewer, writer"));

        let skills = vec![test_skill("explore")];
        let skill_error = select_definition("--skill", "skills", "nope", &skills, |skill| {
            skill.id.as_str()
        })
        .unwrap_err();
        assert!(
            skill_error
                .to_string()
                .contains("available skills: explore")
        );

        let empty: Vec<AgentDefinition> = Vec::new();
        let empty_error = select_definition("--agent", "agents", "missing", &empty, |agent| {
            agent.id.as_str()
        })
        .unwrap_err();
        assert!(empty_error.to_string().contains("available agents: (none)"));
    }

    #[test]
    fn agent_model_precedence_and_colon_warning() {
        assert_eq!(
            pick_model(
                Some("cli-model"),
                Some("env-model"),
                Some("agent-model"),
                Some("settings-model"),
            )
            .as_deref(),
            Some("cli-model")
        );
        assert_eq!(
            pick_model(None, None, Some("agent-model"), Some("settings-model")).as_deref(),
            Some("agent-model")
        );
        assert_eq!(
            pick_model(None, Some("env-model"), Some("agent-model"), None).as_deref(),
            Some("env-model")
        );
        assert_eq!(
            pick_model(None, None, None, Some("settings-model")).as_deref(),
            Some("settings-model")
        );
        assert_eq!(pick_model(None, None, None, None), None);

        assert_eq!(
            agent_model_candidate(Some("gemini-3.1-pro")),
            (Some("gemini-3.1-pro".to_string()), None)
        );
        let (candidate, warning) = agent_model_candidate(Some("openrouter:deepseek-v4.1"));
        assert_eq!(candidate, None);
        assert!(warning.unwrap().contains("implies a provider change"));
        assert_eq!(agent_model_candidate(Some("   ")), (None, None));
        assert_eq!(agent_model_candidate(None), (None, None));
    }

    #[test]
    fn composes_skill_then_agent_instructions() {
        let first = test_skill("explore");
        let second = test_skill("fix");
        let agent = test_agent("reviewer", None, None);

        let composed =
            compose_definition_prompt("BASE".to_string(), &[&first, &second], Some(&agent));
        assert_eq!(
            composed,
            "BASE\n\n## Skill instructions (explore)\n\nBody for explore.\n\n\
             ## Skill instructions (fix)\n\nBody for fix.\n\n\
             ## Agent instructions (reviewer)\n\nPrompt for reviewer."
        );

        assert_eq!(
            compose_definition_prompt("BASE".to_string(), &[], None),
            "BASE"
        );
    }

    #[test]
    fn prune_message_names_count_and_window() {
        assert_eq!(
            format_prune_message(3, 30),
            "Pruned 3 sessions (older than 30 days)"
        );
        assert_eq!(
            format_prune_message(1, 7),
            "Pruned 1 sessions (older than 7 days)"
        );
    }

    #[test]
    fn help_text_lists_agent_and_skill_surfaces() {
        for needle in [
            "--agent <ID>",
            "--skill <ID>",
            "/agent <id>",
            "/skill <id>",
            "@agent/<id>",
            "@skill/<id>",
        ] {
            assert!(HELP_TEXT.contains(needle), "help is missing {needle}");
        }
        for removed in ["/agents", "/skills"] {
            assert!(!HELP_TEXT.contains(removed), "help still lists {removed}");
        }
    }

    #[test]
    fn dotenv_loads_only_when_explicitly_enabled() {
        assert!(!dotenv_flag_enabled(None));
        for value in ["1", "true", "TRUE", "True", "yes", " Yes "] {
            assert!(dotenv_flag_enabled(Some(value)), "{value} should opt in");
        }
        for value in ["", "0", "no", "false", "off"] {
            assert!(
                !dotenv_flag_enabled(Some(value)),
                "{value} should not opt in"
            );
        }

        let path = std::env::temp_dir().join("meerai-r1-dotenv-probe.env");
        std::fs::write(&path, "MEERAI_R1_DOTENV_PROBE=loaded\n").unwrap();
        load_dotenv(Some(path.as_path())).unwrap();
        assert_eq!(
            std::env::var("MEERAI_R1_DOTENV_PROBE").as_deref(),
            Ok("loaded")
        );
        let _ = std::fs::remove_file(&path);
    }

    fn sample_approval_event() -> AgentEvent {
        use meerai_agents::TurnId;
        use meerai_agents::events::ApprovalPreview;
        use meerai_core::EffectClass;

        AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id: RequestId::from_raw("req-headless"),
                turn_id: TurnId::from_raw("turn-headless"),
                kind: ApprovalKind::ToolCall,
                preview: ApprovalPreview {
                    title: "edit: src/main.rs".to_string(),
                    detail: "tool: code-edit_file\neffect: write".to_string(),
                },
                effect: Some(EffectClass::Write),
            },
        }
    }

    fn sample_spend_approval_event() -> AgentEvent {
        use meerai_agents::TurnId;
        use meerai_agents::events::ApprovalPreview;

        AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id: RequestId::from_raw("req-spend"),
                turn_id: TurnId::from_raw("turn-headless"),
                kind: ApprovalKind::Spend,
                preview: ApprovalPreview {
                    title: "Session token budget reached".to_string(),
                    detail: "session used 1200 of 1000 tokens".to_string(),
                },
                effect: None,
            },
        }
    }

    fn approval_request(event: &AgentEvent) -> &ApprovalRequest {
        match event {
            AgentEvent::ApprovalRequested { request } => request,
            other => panic!("expected approval request, got {other:?}"),
        }
    }

    #[test]
    fn headless_tool_call_approval_cancels_and_exits_2() {
        let event = sample_approval_event();
        assert_eq!(headless_outcome(&event), HeadlessOutcome::ApprovalRequired);

        let action = HeadlessApprovalAction::for_request(approval_request(&event));
        assert_eq!(action, HeadlessApprovalAction::Cancel);
        assert_eq!(action.command(), AgentCommand::Cancel);

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&event));
        assert_eq!(exit.code, EXIT_APPROVAL_REQUIRED);
        assert!(exit.approval_cancelled);
    }

    #[test]
    fn headless_spend_approval_declines_and_exits_1() {
        let event = sample_spend_approval_event();
        assert_eq!(headless_outcome(&event), HeadlessOutcome::SpendDeclined);

        let action = HeadlessApprovalAction::for_request(approval_request(&event));
        assert_eq!(
            action,
            HeadlessApprovalAction::Decline {
                request_id: RequestId::from_raw("req-spend"),
            }
        );
        assert_eq!(
            action.command(),
            AgentCommand::Approve {
                request_id: RequestId::from_raw("req-spend"),
                decision: ApprovalDecision::Decline,
                scope: None,
            }
        );

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&event));
        assert_eq!(exit.code, 1);

        use meerai_agents::{TurnEndReason, TurnId};
        exit.observe(headless_outcome(&AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-headless"),
            reason: TurnEndReason::Completed,
        }));
        assert_eq!(exit.code, 1);
    }

    #[test]
    fn headless_exit_codes_follow_turn_outcome() {
        use meerai_agents::{AbortCause, TurnEndReason, TurnId};

        let completed = AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-headless"),
            reason: TurnEndReason::Completed,
        };
        let failed = AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-headless"),
            reason: TurnEndReason::Failed {
                error: "boom".to_string(),
            },
        };
        let aborted = AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-headless"),
            reason: TurnEndReason::Aborted {
                cause: AbortCause::User,
            },
        };

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&completed));
        assert_eq!(exit.code, 0);

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&failed));
        assert_eq!(exit.code, 1);

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&aborted));
        assert_eq!(exit.code, 1);

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&AgentEvent::Error {
            code: meerai_agents::ErrorCode::Provider,
            message: "provider down".to_string(),
        }));
        assert_eq!(exit.code, 1);

        let mut exit = HeadlessExit::new();
        exit.observe(headless_outcome(&sample_approval_event()));
        exit.observe(headless_outcome(&aborted));
        exit.observe(headless_outcome(&completed));
        exit.mark_failed();
        assert_eq!(exit.code, EXIT_APPROVAL_REQUIRED);
    }

    #[test]
    fn resolves_approval_mode_from_flag_then_settings_then_default() {
        assert_eq!(
            resolve_approval_mode(None, None).unwrap(),
            ApprovalMode::Ask
        );
        assert_eq!(
            resolve_approval_mode(Some("plan"), Some("yolo")).unwrap(),
            ApprovalMode::Plan
        );
        assert_eq!(
            resolve_approval_mode(None, Some("auto-edit")).unwrap(),
            ApprovalMode::AutoEdit
        );
        assert_eq!(
            resolve_approval_mode(None, Some("AUTO-EDIT")).unwrap(),
            ApprovalMode::AutoEdit
        );

        let error = resolve_approval_mode(Some("wild"), None).unwrap_err();
        assert!(error.to_string().contains("invalid --mode value"));
        let settings_error = resolve_approval_mode(None, Some("wild")).unwrap_err();
        assert!(settings_error.to_string().contains("settings.yml"));
    }

    #[test]
    fn harness_config_uses_defaults_and_context_overrides() {
        let defaults = harness_config(None);
        assert_eq!(defaults, HarnessConfig::default());

        let context = ContextSettings {
            context_window_tokens: Some(200_000),
            high_watermark_pct: Some(80),
            spill_max_inline_tokens: Some(8_000),
            verbatim_window_turns: Some(6),
            reasoning_pruning: Some(false),
            max_steps_per_turn: Some(48),
            tool_timeout_ms: Some(60_000),
            max_session_tokens: Some(2_000_000),
            max_turn_secs: Some(600),
            summarizer_model: Some("openrouter:small-model".to_string()),
        };
        let config = harness_config(Some(&context));
        assert_eq!(config.context_window_tokens, 200_000);
        assert_eq!(config.high_watermark_pct, 80);
        assert_eq!(config.spill_max_inline_tokens, 8_000);
        assert_eq!(config.verbatim_window_turns, 6);
        assert!(!config.reasoning_pruning);
        assert_eq!(config.max_steps_per_turn, 48);
        assert_eq!(config.tool_timeout_ms, 60_000);
        assert_eq!(config.max_session_tokens, Some(2_000_000));
    }

    #[test]
    fn agent_events_serialize_to_single_line_ndjson_and_roundtrip() {
        use meerai_agents::events::{ApprovalKind, ApprovalPreview, ApprovalRequest};
        use meerai_agents::{CallId, ItemId, ItemStatus, SessionItem, TurnEndReason, TurnId};
        use meerai_core::EffectClass;

        let turn_id = TurnId::from_raw("turn-ndjson");
        let events = vec![
            AgentEvent::TurnStarted {
                turn_id: turn_id.clone(),
                model: "gemini::gemini-3.8-flash".to_string(),
            },
            AgentEvent::ItemStarted {
                item: SessionItem::ToolCall {
                    id: ItemId::from_raw("item-ndjson"),
                    call_id: CallId::from_raw("call-ndjson"),
                    name: "code-write_file".to_string(),
                    args: "{\"path\":\"src/lib.rs\"}".to_string(),
                    effect: EffectClass::Write,
                    status: ItemStatus::Proposed,
                    output: None,
                },
            },
            AgentEvent::ApprovalRequested {
                request: ApprovalRequest {
                    request_id: meerai_agents::events::RequestId::from_raw("req-ndjson"),
                    turn_id: turn_id.clone(),
                    kind: ApprovalKind::ToolCall,
                    preview: ApprovalPreview {
                        title: "edit: src/lib.rs".to_string(),
                        detail: "tool: code-write_file\neffect: write".to_string(),
                    },
                    effect: Some(EffectClass::Write),
                },
            },
            AgentEvent::TurnCompleted {
                turn_id: turn_id.clone(),
                reason: TurnEndReason::Completed,
            },
        ];

        for event in events {
            let line = serde_json::to_string(&event).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<AgentEvent>(&line).unwrap(), event);
        }
    }

    #[test]
    fn parses_auth_subcommands() {
        let list = parse_args(vec!["auth".to_string(), "list".to_string()]).unwrap();
        assert_eq!(
            list.subcommand,
            Some(CliSubcommand::Auth(AuthCommand::List))
        );

        let ls = parse_args(vec!["auth".to_string(), "ls".to_string()]).unwrap();
        assert_eq!(ls.subcommand, Some(CliSubcommand::Auth(AuthCommand::List)));

        let default_auth = parse_args(vec!["auth".to_string()]).unwrap();
        assert_eq!(
            default_auth.subcommand,
            Some(CliSubcommand::Auth(AuthCommand::List))
        );

        let login = parse_args(vec![
            "auth".to_string(),
            "login".to_string(),
            "openai".to_string(),
            "sk-test".to_string(),
        ])
        .unwrap();
        assert_eq!(
            login.subcommand,
            Some(CliSubcommand::Auth(AuthCommand::Login {
                provider: "openai".to_string(),
                key: Some("sk-test".to_string()),
            }))
        );

        let login_prompt = parse_args(vec![
            "auth".to_string(),
            "login".to_string(),
            "anthropic".to_string(),
        ])
        .unwrap();
        assert_eq!(
            login_prompt.subcommand,
            Some(CliSubcommand::Auth(AuthCommand::Login {
                provider: "anthropic".to_string(),
                key: None,
            }))
        );

        let logout = parse_args(vec![
            "auth".to_string(),
            "logout".to_string(),
            "openai".to_string(),
        ])
        .unwrap();
        assert_eq!(
            logout.subcommand,
            Some(CliSubcommand::Auth(AuthCommand::Logout {
                provider: "openai".to_string(),
            }))
        );

        assert!(parse_args(vec!["auth".to_string(), "bad".to_string()]).is_err());
        assert!(parse_args(vec!["auth".to_string(), "login".to_string()]).is_err());
        assert!(parse_args(vec!["auth".to_string(), "logout".to_string()]).is_err());
    }

    #[test]
    fn parses_sessions_subcommands() {
        let sessions = |args: &[&str]| -> Vec<String> {
            std::iter::once("sessions".to_string())
                .chain(args.iter().map(|arg| arg.to_string()))
                .collect()
        };

        assert_eq!(
            parse_args(sessions(&["list"])).unwrap().subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::List))
        );
        assert_eq!(
            parse_args(sessions(&[])).unwrap().subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::List))
        );
        assert_eq!(
            parse_args(sessions(&["rm", "abc"])).unwrap().subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::Remove {
                target: SessionTarget::Id("abc".to_string()),
            }))
        );
        assert_eq!(
            parse_args(sessions(&["rm", "--all"])).unwrap().subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::Remove {
                target: SessionTarget::All,
            }))
        );

        let missing = parse_args(sessions(&["rm"])).unwrap_err();
        assert!(missing.to_string().contains("session id or --all"));

        let both = parse_args(sessions(&["rm", "abc", "--all"])).unwrap_err();
        assert!(both.to_string().contains("not both"));

        assert!(parse_args(sessions(&["rm", "abc", "def"])).is_err());
        assert!(parse_args(sessions(&["rm", "--bogus"])).is_err());
        assert!(parse_args(sessions(&["list", "extra"])).is_err());

        let unknown = parse_args(sessions(&["bad"])).unwrap_err();
        assert!(unknown.to_string().contains("list, rm"));
    }

    #[test]
    fn parses_continue_and_resume_flags() {
        let resume = parse_args(vec![
            "--resume".to_string(),
            "ses-abc".to_string(),
            "--prompt".to_string(),
            "hi".to_string(),
        ])
        .unwrap();
        assert_eq!(resume.resume.as_deref(), Some("ses-abc"));
        assert!(!resume.continue_session);

        let equals = parse_args(vec![
            "--resume=ses-xyz".to_string(),
            "--prompt=hi".to_string(),
        ])
        .unwrap();
        assert_eq!(equals.resume.as_deref(), Some("ses-xyz"));

        let continued = parse_args(vec![
            "--continue".to_string(),
            "--prompt".to_string(),
            "hi".to_string(),
        ])
        .unwrap();
        assert!(continued.continue_session);
        assert_eq!(continued.resume, None);

        let tui_continue = parse_args(vec!["--continue".to_string()]).unwrap();
        assert!(tui_continue.continue_session);
        assert_eq!(tui_continue.prompt, None);

        let tui_resume = parse_args(vec!["--resume".to_string(), "ses-abc".to_string()]).unwrap();
        assert_eq!(tui_resume.resume.as_deref(), Some("ses-abc"));
        assert_eq!(tui_resume.prompt, None);

        let conflict = parse_args(vec![
            "--continue".to_string(),
            "--resume".to_string(),
            "ses-abc".to_string(),
            "--prompt".to_string(),
            "hi".to_string(),
        ])
        .unwrap_err();
        assert!(conflict.to_string().contains("cannot be combined"));

        let conflict_tui = parse_args(vec![
            "--continue".to_string(),
            "--resume".to_string(),
            "ses-abc".to_string(),
        ])
        .unwrap_err();
        assert!(conflict_tui.to_string().contains("cannot be combined"));

        assert!(parse_args(vec!["--resume".to_string()]).is_err());
    }

    #[test]
    fn session_rows_prefer_title_then_model() {
        let base = SessionSummary {
            id: SessionId::from_raw("ses-1"),
            dir: std::path::PathBuf::from("/tmp/ses-1"),
            title: Some("Fix build".to_string()),
            model: Some("glm-5.3".to_string()),
            created_at_ms: 1,
            updated_at_ms: 2,
            bytes: 3,
        };
        assert_eq!(format_session_row(&base), "ses-1\t2\t3\tFix build");

        let model_only = SessionSummary {
            title: None,
            ..base.clone()
        };
        assert_eq!(format_session_row(&model_only), "ses-1\t2\t3\tglm-5.3");

        let empty = SessionSummary {
            title: Some(String::new()),
            model: None,
            ..base
        };
        assert_eq!(format_session_row(&empty), "ses-1\t2\t3\t");
    }

    #[tokio::test]
    async fn continue_with_empty_session_root_reports_no_sessions() {
        let root = std::env::temp_dir().join(format!("meerai-r2b-continue-{}", SessionId::new()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(latest_session(&root).unwrap().is_none());
        let error = continue_session_id(&root).await.unwrap_err();
        assert!(error.to_string().contains("no sessions found to continue"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn resume_missing_session_errors_without_creating_it() {
        let root = std::env::temp_dir().join(format!("meerai-r2b-resume-{}", SessionId::new()));
        let config = SessionConfig {
            root: root.clone(),
            workspace: root.clone(),
            project_local: false,
        };
        let id = SessionId::new();
        let error = Session::open(&config, id.clone(), root.clone())
            .await
            .err()
            .expect("missing session must not open");
        assert!(error.to_string().contains("session not found"));
        assert!(!root.join(id.as_str()).exists());
        std::fs::remove_dir_all(&root).ok();
    }

    fn session_summary(id: &str, updated_at_ms: u64) -> SessionSummary {
        SessionSummary {
            id: SessionId::from_raw(id),
            dir: std::path::PathBuf::from(format!("/tmp/{id}")),
            title: None,
            model: None,
            created_at_ms: updated_at_ms,
            updated_at_ms,
            bytes: 0,
        }
    }

    #[test]
    fn parses_sessions_prune_flags() {
        let sessions = |args: &[&str]| -> Vec<String> {
            std::iter::once("sessions".to_string())
                .chain(args.iter().map(|arg| arg.to_string()))
                .collect()
        };

        assert_eq!(
            parse_args(sessions(&["prune"])).unwrap().subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::Prune {
                days: 30,
                keep: 0,
            }))
        );
        assert_eq!(
            parse_args(sessions(&["prune", "--days", "7", "--keep", "3"]))
                .unwrap()
                .subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::Prune {
                days: 7,
                keep: 3,
            }))
        );
        assert_eq!(
            parse_args(sessions(&["prune", "--days=14", "--keep=2"]))
                .unwrap()
                .subcommand,
            Some(CliSubcommand::Sessions(SessionsCommand::Prune {
                days: 14,
                keep: 2,
            }))
        );

        assert!(parse_args(sessions(&["prune", "--days"])).is_err());
        assert!(parse_args(sessions(&["prune", "--days", "soon"])).is_err());
        assert!(parse_args(sessions(&["prune", "--keep", "-1"])).is_err());
        assert!(parse_args(sessions(&["prune", "extra"])).is_err());
        assert!(parse_args(sessions(&["prune", "--bogus"])).is_err());
    }

    #[test]
    fn select_prunable_uses_cutoff_and_keeps_newest_sessions() {
        let day = 86_400_000;
        let now = 1_000 * day;
        let sessions = vec![
            session_summary("ses-old", now - 40 * day),
            session_summary("ses-mid", now - 31 * day),
            session_summary("ses-edge", now - 30 * day),
            session_summary("ses-new", now - day),
        ];

        assert_eq!(
            select_prunable(&sessions, now, 30, 0, None),
            vec![
                SessionId::from_raw("ses-mid"),
                SessionId::from_raw("ses-old")
            ]
        );
        assert_eq!(
            select_prunable(&sessions, now, 30, 2, None),
            vec![
                SessionId::from_raw("ses-mid"),
                SessionId::from_raw("ses-old")
            ]
        );
        assert_eq!(
            select_prunable(&sessions, now, 30, 3, None),
            vec![SessionId::from_raw("ses-old")]
        );
        assert!(select_prunable(&sessions, now, 30, 4, None).is_empty());
    }

    #[test]
    fn select_prunable_never_selects_the_excluded_session() {
        let day = 86_400_000;
        let now = 1_000 * day;
        let sessions = vec![
            session_summary("ses-old", now - 40 * day),
            session_summary("ses-mid", now - 31 * day),
            session_summary("ses-new", now - day),
        ];

        let excluded = SessionId::from_raw("ses-old");
        assert_eq!(
            select_prunable(&sessions, now, 30, 0, Some(&excluded)),
            vec![SessionId::from_raw("ses-mid")]
        );

        let active_but_recent = SessionId::from_raw("ses-new");
        assert_eq!(
            select_prunable(&sessions, now, 30, 0, Some(&active_but_recent)),
            vec![
                SessionId::from_raw("ses-mid"),
                SessionId::from_raw("ses-old")
            ]
        );
    }

    fn write_fake_session(root: &Path, id: &str, updated_at_ms: u64) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = meerai_session::SessionMeta {
            id: SessionId::from_raw(id),
            title: None,
            cwd: root.to_path_buf(),
            git_branch: None,
            git_sha: None,
            model: None,
            created_at_ms: updated_at_ms,
            updated_at_ms,
            forked_from: None,
            inherited_event_count: None,
        };
        std::fs::write(dir.join("meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
    }

    #[test]
    fn auto_prune_is_opt_in_via_settings() {
        let manual: AppSettings = serde_yaml::from_str("active_provider: openrouter\n").unwrap();
        assert_eq!(auto_prune_config(&manual), None);

        let keep_only: AppSettings = serde_yaml::from_str("sessions:\n  keep: 3\n").unwrap();
        assert_eq!(auto_prune_config(&keep_only), None);

        let enabled: AppSettings =
            serde_yaml::from_str("sessions:\n  auto_prune_days: 7\n").unwrap();
        assert_eq!(auto_prune_config(&enabled), Some((7, 0)));

        let with_keep: AppSettings =
            serde_yaml::from_str("sessions:\n  auto_prune_days: 14\n  keep: 2\n").unwrap();
        assert_eq!(auto_prune_config(&with_keep), Some((14, 2)));
    }

    #[tokio::test]
    async fn prune_old_sessions_respects_days_keep_and_exclusion() {
        let day: u64 = 86_400_000;
        let now = now_millis();
        let root = std::env::temp_dir().join(format!("meerai-cfg1-prune-{}", SessionId::new()));
        std::fs::create_dir_all(&root).unwrap();
        write_fake_session(&root, "ses-oldest", now.saturating_sub(40 * day));
        write_fake_session(&root, "ses-old", now.saturating_sub(31 * day));
        write_fake_session(&root, "ses-recent", now.saturating_sub(29 * day));
        write_fake_session(&root, "ses-new", now.saturating_sub(day));

        let excluded = SessionId::from_raw("ses-old");
        let count = prune_old_sessions(root.clone(), 30, 0, &excluded)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert!(!root.join("ses-oldest").exists());
        assert!(root.join("ses-old").exists(), "excluded session must stay");
        assert!(root.join("ses-recent").exists());
        assert!(root.join("ses-new").exists());

        let none = SessionId::from_raw("ses-none");
        let count = prune_old_sessions(root.clone(), 30, 3, &none)
            .await
            .unwrap();
        assert_eq!(count, 0);

        let count = prune_old_sessions(root.clone(), 30, 0, &none)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert!(!root.join("ses-old").exists());
        assert!(root.join("ses-recent").exists());
        assert!(root.join("ses-new").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn prune_old_sessions_missing_root_is_not_an_error() {
        let root = std::env::temp_dir().join(format!("meerai-cfg1-missing-{}", SessionId::new()));
        let count = prune_old_sessions(root, 7, 0, &SessionId::from_raw("ses-none"))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn formats_token_summary_and_omits_missing_fields() {
        assert_eq!(format_token_summary(&meerai_agents::Usage::default()), None);

        let full = meerai_agents::Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
            cached_tokens: None,
        };
        assert_eq!(
            format_token_summary(&full).as_deref(),
            Some("tokens: prompt=10 completion=5 total=15")
        );

        let partial = meerai_agents::Usage {
            prompt_tokens: None,
            completion_tokens: Some(4),
            total_tokens: None,
            cached_tokens: None,
        };
        assert_eq!(
            format_token_summary(&partial).as_deref(),
            Some("tokens: completion=4")
        );
    }
}
