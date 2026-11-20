#![cfg_attr(
    not(feature = "tui"),
    allow(dead_code, unused_imports, unused_variables)
)]

mod acp_cmd;
mod auth_cmd;
mod cli;
mod config_cmd;
mod headless;
mod hooks_cmd;
mod sessions_cmd;
mod store_cmd;
#[cfg(test)]
mod tests;

#[cfg(feature = "web")]
use std::io::Write as _;
use std::io::{self, IsTerminal};
#[cfg(feature = "web")]
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, bail};
use futures::FutureExt;
use meerai_acp::{AGY, AcpStart, AcpWorkerConfig, spawn_acp_worker};
#[cfg(any(feature = "tui", feature = "web"))]
use meerai_agents::spawn_worker;
use meerai_agents::{
    AcpChild, AcpRunToolset, AcpSpawner, AgentHarness, DEFAULT_GLOBAL_CONCURRENCY, HarnessConfig,
    HookEngine, MemoryCapture, MemoryToolset, MultiTurnAgent, QuestionToolset, RoutingCoordinator,
    SKILL_TOOL, SkillToolset, SubagentToolFactory, SubagentToolset, TokenPricing,
};
use meerai_client::providers::{
    ProviderAuthKind, ProviderCatalog, RuntimeProviderState, canonical_provider_id,
    check_key_status, first_configured_provider, resolve_credential,
};
use meerai_client::secrets::{Credential, SecretStore};
use meerai_common::definitions::{
    AgentDefinition, SkillDefinition, load_definitions, load_definitions_for, validate_agent_model,
};
use meerai_common::hooks::{add_trusted_project, load_hooks_for_run, project_is_trusted};
#[cfg(feature = "tui")]
use meerai_common::settings::Temperature;
use meerai_common::settings::{
    AppSettings, HooksSettings, default_config_dir, default_data_dir, find_project_root,
    load_project_settings, load_settings_from, merge_project_settings, save_settings_to,
};
use meerai_core::{Toolset, async_trait};
#[cfg(any(feature = "tui", feature = "web"))]
use meerai_host::{
    AcpPresetInfo, Backend, ChatGptLogin, ChatGptLoginOutcome, HostAgentInfo, HostAuthKind,
    HostDefinitionInfo, HostDefinitionsDiagnostics, HostInfoSnapshot, HostModelItem, HostPalette,
    HostProviderItem, HostProvidersSnapshot, HostReloadOutcome, HostReloadPayload,
    HostSettingsUpdate, HostSettingsView, HostSkillInfo, OpenedSession, ResolvedAgentApplication,
    ResolvedModelSelection, ResolvedSkillApplication, SessionOpener, SessionRegistry,
    WebServerLauncher,
};
#[cfg(feature = "web")]
use meerai_host::{WebServerHandle, WebServerStart};
use meerai_policy::{ApprovalMode, PolicyEngine, parse_approval_mode, rules_from_yaml};
use meerai_providers::providers::build_genai_client;
use meerai_providers::secrets::default_secret_store;
use meerai_session::{
    LoggedEvent, Session, SessionConfig, SessionId, SessionSummary, TurnId, default_sessions_root,
    delete_session as delete_session_dir, list_sessions,
};
use meerai_store::{
    InstalledPack, ModelRoute, NewProjectMemory, ProjectMemory, RoutingSettings, Store, TaskType,
};
use meerai_tools::search_as_code::ScriptRuntime;
use meerai_tools::{
    CodeTools, ExecutionWorld, LspToolset, RepoMapToolset, SearchAsCodeToolset,
    detect_execution_world,
};
#[cfg(feature = "tui")]
use meerai_tui::{TuiLaunchConfig, transcript_from_events};
#[cfg(feature = "web")]
use meerai_web::{
    BASIC_USERNAME, DEFAULT_SERVE_PORT, DEFAULT_SERVE_SECRET, RunningServer, ServeAuth,
    ServeConfig, ServeSecret, start_server,
};

use auth_cmd::run_auth;
use cli::{CliSubcommand, help_text, parse_args};
use headless::{run_headless, run_headless_channels};
use hooks_cmd::run_hooks;
#[cfg(feature = "tui")]
use sessions_cmd::empty_session_ids;
use sessions_cmd::{
    auto_prune_sessions, continue_session_id, fork_session, prune_empty_sessions, run_sessions,
};

const FALLBACK_PROVIDER: &str = "opencode-go";

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
    match meerai_common::definitions::validate_agent_model(agent_model) {
        Ok(valid) => (valid.map(str::to_string), None),
        Err(raw) => (
            None,
            Some(format!(
                "agent model '{raw}' implies a provider change; ignoring it"
            )),
        ),
    }
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

const MAX_DEFINITION_BYTES: usize = 16 * 1024;
const MAX_DEFINITION_BLOCK_BYTES: usize = 32 * 1024;

fn capped_definition(body: &str) -> (String, bool) {
    if body.len() <= MAX_DEFINITION_BYTES {
        return (body.to_string(), false);
    }
    let end = meerai_core::text::floor_char_boundary(body, MAX_DEFINITION_BYTES);
    (
        format!("{}\n... [definition truncated]", &body[..end]),
        true,
    )
}

fn load_definitions_for_runtime(
    config_dir: Option<&Path>,
    project_root: &Path,
) -> meerai_common::definitions::DefinitionCatalog {
    match meerai_common::settings::open_store_for_config_dir(config_dir) {
        Ok(store) => load_definitions_for(&store, config_dir, project_root),
        Err(_) => load_definitions(project_root),
    }
}

fn compose_definition_prompt(
    mut system: String,
    skills: &[&SkillDefinition],
    agent: Option<&AgentDefinition>,
    catalog: &[SkillDefinition],
) -> String {
    let mut block = String::new();
    if let Some(index) = meerai_agents::prompt::skill_index(catalog) {
        block.push_str("\n\n");
        block.push_str(&index);
    }
    let mut truncated = false;
    for skill in skills {
        let (body, cut) = capped_definition(&skill.body);
        truncated |= cut;
        block.push_str(&format!(
            "\n\n## Skill instructions ({})\n\n{}",
            skill.id, body
        ));
    }
    if let Some(agent) = agent {
        let (body, cut) = capped_definition(&agent.prompt);
        truncated |= cut;
        block.push_str(&format!(
            "\n\n## Agent instructions ({})\n\n{}",
            agent.id, body
        ));
    }
    if block.len() > MAX_DEFINITION_BLOCK_BYTES {
        let end = meerai_core::text::floor_char_boundary(&block, MAX_DEFINITION_BLOCK_BYTES);
        block.truncate(end);
        block.push_str("\n... [definition block truncated]");
        truncated = true;
    }
    if truncated {
        eprintln!(
            "warning: agent and skill instructions exceed the definition cap (16 KiB per definition, 32 KiB total) and were truncated"
        );
    }
    system.push_str(&block);
    system
}

fn dotenv_flag_enabled(value: Option<&str>) -> bool {
    match value.map(str::trim).map(str::to_ascii_lowercase) {
        Some(flag) => matches!(flag.as_str(), "1" | "true" | "yes"),
        None => false,
    }
}

fn env_flag(key: &str) -> bool {
    dotenv_flag_enabled(std::env::var(key).ok().as_deref())
}

fn prompt_project_hook_trust(
    hooks_path: &Path,
    project_root: &Path,
    config_dir: Option<&Path>,
    user_settings: &mut AppSettings,
) -> bool {
    eprintln!("Project hooks found at {}", hooks_path.display());
    eprintln!("Hooks run arbitrary commands as you. Trust this project? [y/N]");
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    let answer = answer.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        return false;
    }
    let Some(dir) = config_dir else {
        let hooks = user_settings
            .hooks
            .get_or_insert_with(HooksSettings::default);
        add_trusted_project(&mut hooks.trusted_projects, project_root);
        return true;
    };

    let mut disk_settings = match load_settings_from(dir) {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("Warning: failed to save project hook trust: {err:#}");
            return false;
        }
    };
    let trusted = {
        let hooks = disk_settings
            .hooks
            .get_or_insert_with(HooksSettings::default);
        add_trusted_project(&mut hooks.trusted_projects, project_root);
        hooks.trusted_projects.clone()
    };
    if let Err(err) = save_settings_to(dir, &disk_settings) {
        eprintln!("Warning: failed to save project hook trust: {err:#}");
        return false;
    }
    let hooks = user_settings
        .hooks
        .get_or_insert_with(HooksSettings::default);
    hooks.trusted_projects = trusted;
    true
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

fn resolve_approval_mode(flag: Option<&str>, configured: Option<&str>) -> Result<ApprovalMode> {
    if let Some(raw) = flag {
        return parse_approval_mode(raw).ok_or_else(|| {
            anyhow::anyhow!("invalid --mode value '{raw}': expected plan, ask, auto")
        });
    }
    match configured {
        Some(raw) => parse_approval_mode(raw).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid approval_mode '{raw}' in settings.yml: expected plan, ask, auto"
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

fn build_workspace_toolsets(
    code_tools: &CodeTools,
    repo_map: &RepoMapToolset,
    lsp: &LspToolset,
    sac: &SearchAsCodeToolset,
    read_only: bool,
) -> Vec<Box<dyn Toolset>> {
    let script_runtime = sac.runtime();
    let code = if read_only {
        code_tools
            .clone()
            .with_allowed_tools(["read_file", "list_dir", "grep", "glob"])
    } else {
        code_tools.clone()
    };
    let mut toolsets: Vec<Box<dyn Toolset>> = vec![
        Box::new(code),
        Box::new(repo_map.clone()),
        Box::new(lsp.clone()),
    ];
    if !read_only && script_runtime.is_some() {
        toolsets.push(Box::new(sac.clone()));
    }
    toolsets
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = parse_args(std::env::args().skip(1))?;
    load_dotenv(cli.env_file.as_deref().map(Path::new))?;

    if cli.show_help {
        println!("{}", help_text());
        return Ok(());
    }

    if cli.version {
        println!("meerai {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let Some(CliSubcommand::Config(command)) = cli.subcommand.as_ref() {
        return config_cmd::run(command);
    }

    match cli.subcommand.as_ref() {
        Some(CliSubcommand::Memory(command)) => return store_cmd::run_memory(command.clone()),
        Some(CliSubcommand::Routes(command)) => return store_cmd::run_routes(command.clone()),
        Some(CliSubcommand::Pack(command)) => return store_cmd::run_pack(command.clone()),
        _ => {}
    }

    let is_serve = matches!(cli.subcommand, Some(CliSubcommand::Serve(_)));
    #[cfg(not(feature = "web"))]
    if is_serve {
        bail!("meerai serve requires a build with the `web` feature enabled");
    }

    if let Some(CliSubcommand::Sessions(command)) = cli.subcommand.as_ref() {
        return run_sessions(command).await;
    }

    let workspace_root = std::env::current_dir()?;
    let project_root = find_project_root(&workspace_root);
    let config_dir = default_config_dir();
    if let Some(CliSubcommand::Acp(command)) = cli.subcommand.as_ref() {
        return acp_cmd::run(*command, &workspace_root).await;
    }
    let definitions = load_definitions_for_runtime(config_dir.as_deref(), &project_root);
    for warning in &definitions.warnings {
        eprintln!("Warning: {warning}");
    }
    let project_settings_path = project_root.join(".meerai").join("settings.yml");
    let data_dir = default_data_dir();
    let mut user_settings = config_dir
        .as_deref()
        .and_then(|dir| load_settings_from(dir).ok())
        .unwrap_or_default();
    if let Some(CliSubcommand::Hooks(command)) = cli.subcommand {
        return run_hooks(
            command,
            config_dir.as_deref(),
            &project_root,
            &mut user_settings,
        );
    }
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
    #[cfg(feature = "tui")]
    {
        let (palette, theme_warnings) =
            meerai_tui::theme::Palette::from_settings(&settings.theme.clone().unwrap_or_default());
        for warning in theme_warnings {
            eprintln!("Warning: {warning}");
        }
        meerai_tui::theme::install(palette);
    }
    let secrets = default_secret_store(data_dir.as_deref())?;
    let auth_path = data_dir.as_deref().map(|dir| dir.join("meerai.db"));

    if let Some(CliSubcommand::Auth(auth_cmd)) = cli.subcommand {
        return run_auth(
            auth_cmd,
            &catalog,
            secrets.as_ref(),
            auth_path.as_deref(),
            config_dir.as_deref(),
            data_dir.as_deref(),
            &mut user_settings,
        )
        .await;
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

    if cli.prompt.is_none()
        && !is_serve
        && (!io::stdin().is_terminal() || !io::stdout().is_terminal())
    {
        bail!("no terminal detected; use --prompt \"...\" for headless mode (see --help)");
    }
    #[cfg(not(feature = "tui"))]
    if cli.prompt.is_none() && !is_serve {
        bail!("this build has no TUI support; use --prompt \"...\" for headless mode");
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

    let sensitive_env_vars = catalog.sensitive_env_vars();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings, catalog,
    )));
    {
        let plan_usage = {
            let rt = runtime.read().unwrap();
            let provider = rt.active_provider_info();
            matches!(
                resolve_credential(&provider, secrets.as_ref())
                    .ok()
                    .flatten(),
                Some(Credential::OAuth(_))
            )
        };
        runtime.write().unwrap().plan_usage = plan_usage;
    }
    if let Err(err) =
        meerai_providers::oauth::ensure_fresh_credential(secrets.as_ref(), &active_provider).await
    {
        eprintln!("Warning: ChatGPT plan credential: {err:#}");
    }
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
    #[cfg(feature = "tui")]
    let (reuse_empty, restore_ids) = if cli.prompt.is_none()
        && !is_serve
        && !cli.continue_session
        && cli.resume.is_none()
        && cli.fork.is_none()
    {
        let restored = meerai_common::settings::open_store_for_config_dir(config_dir.as_deref())
            .map(|store| meerai_tui::tabs::restored_tabs_for(&store, &workspace_root))
            .unwrap_or_default();
        let mut candidates = Vec::with_capacity(restored.sessions.len() + 1);
        if let Some(hint) = restored.empty_session.clone() {
            candidates.push(hint);
        }
        for id in &restored.sessions {
            if !candidates.contains(id) {
                candidates.push(id.clone());
            }
        }
        let empty = empty_session_ids(&default_sessions_root(), &candidates).await;
        let reuse = empty.first().cloned();
        let restore = restored
            .sessions
            .into_iter()
            .filter(|id| !empty.contains(id))
            .collect();
        (reuse, restore)
    } else {
        (None, Vec::new())
    };
    #[cfg(not(feature = "tui"))]
    let (reuse_empty, restore_ids): (Option<SessionId>, Vec<SessionId>) = (None, Vec::new());
    let session = if cli.continue_session {
        let root = default_sessions_root();
        let id = continue_session_id(&root).await?;
        Session::open(&session_config, id, workspace_root.clone()).await?
    } else if let Some(raw) = cli.resume.as_deref() {
        let id = SessionId::parse(raw)?;
        Session::open(&session_config, id, workspace_root.clone()).await?
    } else if let Some(raw) = cli.fork.as_deref() {
        let root = default_sessions_root();
        fork_session(&root, &workspace_root, raw, None).await?
    } else if let Some(id) = reuse_empty {
        match Session::open(&session_config, id, workspace_root.clone()).await {
            Ok(session) => session,
            Err(_) => {
                Session::open_or_create(&session_config, SessionId::new(), workspace_root.clone())
                    .await?
            }
        }
    } else {
        Session::open_or_create(&session_config, SessionId::new(), workspace_root.clone()).await?
    };
    if !active_model.trim().is_empty() {
        session.set_model(&active_model).await?;
    }
    let approval_mode = if cli.mode.is_some() {
        approval_mode
    } else {
        session
            .meta()
            .approval_mode
            .as_deref()
            .and_then(parse_approval_mode)
            .unwrap_or(approval_mode)
    };
    let _ = session.set_approval_mode(approval_mode.label()).await;
    prune_empty_sessions(session.id(), &restore_ids).await;
    auto_prune_sessions(&settings, session.id(), &restore_ids).await;

    let session_is_acp = session.meta().harness.as_deref() == Some("acp:agy");
    let use_acp = cli.acp || session_is_acp;
    if use_acp && matches!(approval_mode, ApprovalMode::Plan) {
        bail!("ACP cannot run in plan mode; the external agent cannot be made read-only");
    }
    if use_acp && (cli.continue_session || cli.resume.is_some()) && !session_is_acp {
        bail!("selected session is not an ACP session; remove --acp to resume it natively");
    }
    let acp_start = if cli.fork.is_some() {
        AcpStart::New
    } else {
        session
            .meta()
            .remote_session_id
            .clone()
            .map(|id| AcpStart::Load { session_id: id })
            .unwrap_or(AcpStart::New)
    };

    let execution_world = detect_execution_world(true).await;
    if let Some(warning) = execution_world.warning() {
        eprintln!("Warning: {warning}");
    }

    let script_runtime = ScriptRuntime::detect();
    let memory_bytes = settings
        .context
        .as_ref()
        .and_then(|context| context.memory_prompt_bytes)
        .map_or(meerai_store::DEFAULT_MEMORY_PROMPT_BYTES as u64, |bytes| {
            bytes
        }) as usize;
    let memory_block = meerai_common::settings::open_store_for_config_dir(config_dir.as_deref())
        .ok()
        .and_then(|store| meerai_agents::load_memory_prompt(&store, &project_root, memory_bytes));
    let base_system = meerai_agents::prompt::compose_system_prompt_with_memory(
        &cli.system,
        &workspace_root,
        memory_block.as_deref(),
    )
    .await;
    let base_system = meerai_agents::prompt::append_script_guidance(
        base_system,
        script_runtime.map(ScriptRuntime::program),
    );
    let initial_system = compose_definition_prompt(
        base_system.clone(),
        &selected_skills,
        selected_agent,
        &definitions.skills,
    );
    let launch_definitions = initial_system
        .strip_prefix(base_system.as_str())
        .unwrap_or_default()
        .to_string();

    let hooks_enabled = settings
        .hooks
        .as_ref()
        .and_then(|hooks| hooks.enabled)
        .unwrap_or(true);
    let mut trust_project = env_flag("MEERAI_TRUST_PROJECT_HOOKS")
        || settings
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.trust_project)
            .unwrap_or(false)
        || settings
            .hooks
            .as_ref()
            .is_some_and(|hooks| project_is_trusted(&hooks.trusted_projects, &project_root));
    let project_hooks_path = project_root.join(".meerai").join("hooks.json");
    if hooks_enabled
        && !trust_project
        && project_hooks_path.exists()
        && cli.prompt.is_none()
        && io::stdin().is_terminal()
        && prompt_project_hook_trust(
            &project_hooks_path,
            &project_root,
            config_dir.as_deref(),
            &mut user_settings,
        )
    {
        trust_project = true;
        let hooks = settings.hooks.get_or_insert_with(HooksSettings::default);
        add_trusted_project(&mut hooks.trusted_projects, &project_root);
    }
    if hooks_enabled && project_hooks_path.exists() && !trust_project {
        eprintln!(
            "Warning: ignoring project hooks: run `meerai hooks trust`, set MEERAI_TRUST_PROJECT_HOOKS=1, or set hooks.trust_project: true"
        );
    }
    let hooks = if hooks_enabled {
        let (entries, hook_warnings) = load_hooks_for_run(
            config_dir.as_deref(),
            &project_root,
            settings.hooks.as_ref(),
        );
        for warning in &hook_warnings {
            eprintln!("Warning: {warning}");
        }
        Arc::new(HookEngine::new(entries))
    } else {
        Arc::new(HookEngine::default())
    };

    let sandbox_active = execution_world.is_sandboxed();
    let sandbox_kind = execution_world.kind().to_string();
    let sandbox_warning = execution_world.warning().map(str::to_string);

    let host = Arc::new(CliSessionHost {
        workspace_root: workspace_root.clone(),
        session_config,
        runtime: runtime.clone(),
        secrets: secrets.clone(),
        settings: settings.clone(),
        user_settings: std::sync::Mutex::new(user_settings.clone()),
        config_dir: config_dir.clone(),
        data_dir: data_dir.clone(),
        client,
        approval_mode,
        execution_world,
        sensitive_env_vars,
        hooks,
        custom_agents: definitions.agents.clone(),
        skill_catalog: Arc::new(definitions.skills.clone()),
        base_system,
        launch_system: cli.system.clone(),
        launch_definitions: launch_definitions.clone(),
        script_runtime,
        default_provider: active_provider.clone(),
        default_model: active_model.clone(),
    });

    if let Some(prompt) = cli.prompt {
        if use_acp {
            let (cmd_tx, event_rx) = spawn_acp_worker(AcpWorkerConfig {
                session,
                preset: AGY,
                cwd: workspace_root.clone(),
                start: acp_start,
            });
            let exit_code = run_headless_channels(cmd_tx, event_rx, prompt, cli.json).await?;
            std::process::exit(exit_code);
        }
        let agent = host.build_agent(session, &active_provider, &active_model, initial_system)?;
        if let Err(err) =
            meerai_providers::oauth::ensure_fresh_credential(secrets.as_ref(), &active_provider)
                .await
        {
            eprintln!("Warning: ChatGPT plan credential: {err:#}");
        }
        let exit_code = run_headless(agent, prompt, cli.json).await?;
        std::process::exit(exit_code);
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    let opened = if use_acp {
        host.build_acp_opened_session(session, acp_start)?
    } else {
        let meta = session.meta();
        let events = session.events();
        let agent = host.build_agent(session, &active_provider, &active_model, initial_system)?;
        let (cmd_tx, event_rx) = spawn_worker(agent);
        OpenedSession {
            session_id: meta.id,
            title: meta.title,
            backend: Backend::Native,
            provider: active_provider.clone(),
            model: active_model.clone(),
            approval_mode,
            events,
            cmd_tx,
            event_rx,
        }
    };

    #[cfg(feature = "web")]
    if let Some(CliSubcommand::Serve(serve_cmd)) = cli.subcommand {
        return run_serve(serve_cmd, opened, host).await;
    }

    #[cfg(feature = "tui")]
    let update_available = meerai_tui::update::check_latest().await;

    #[cfg(feature = "tui")]
    {
        let registry = SessionRegistry::new();
        #[cfg(feature = "web")]
        let web_launcher: Option<Arc<dyn WebServerLauncher>> = Some(Arc::new(CliWebServerLauncher));
        #[cfg(not(feature = "web"))]
        let web_launcher: Option<Arc<dyn WebServerLauncher>> = None;

        let exit = meerai_tui::run_tui(
            opened.cmd_tx,
            opened.event_rx,
            TuiLaunchConfig {
                initial_session_id: opened.session_id,
                initial_session_title: opened.title,
                initial_backend: opened.backend,
                initial_provider: opened.provider,
                initial_model: opened.model,
                workspace_root,
                runtime,
                secrets,
                settings: user_settings,
                context_temperature: settings
                    .context
                    .as_ref()
                    .and_then(|context| context.temperature)
                    .and_then(Temperature::effective),
                context_effort: settings.context.as_ref().and_then(|context| context.effort),
                explicit_context_window: settings
                    .context
                    .as_ref()
                    .and_then(|context| context.context_window_tokens),
                config_dir,
                approval_mode: opened.approval_mode,
                initial_transcript: transcript_from_events(&opened.events),
                initial_events: opened.events,
                agents: definitions.agents,
                skills: definitions.skills,
                session_opener: Some(host),
                update_available,
                restore_sessions: restore_ids,
                launch_system: cli.system,
                launch_definitions,
                sandbox_active,
                sandbox_kind,
                sandbox_warning,
                workspace_query: None,
                registry: Some(registry),
                web_launcher,
            },
        )
        .await?;
        print!(
            "{}",
            exit_summary(exit.session_id.as_str(), exit.title.as_deref())
        );
        return Ok(());
    }

    #[cfg(not(any(feature = "tui", feature = "web")))]
    {
        let _ = session;
        let _ = initial_system;
        Ok(())
    }
    #[cfg(all(not(feature = "tui"), feature = "web"))]
    {
        let _ = opened;
        let _ = restore_ids;
        let _ = launch_definitions;
        let _ = sandbox_active;
        let _ = sandbox_kind;
        let _ = sandbox_warning;
        Ok(())
    }
}

#[cfg(feature = "web")]
struct CliWebServerHandle {
    addr: SocketAddr,
    url: String,
    server: tokio::sync::Mutex<Option<RunningServer>>,
}

#[cfg(feature = "web")]
#[async_trait]
impl WebServerHandle for CliWebServerHandle {
    fn port(&self) -> u16 {
        self.addr.port()
    }

    fn url(&self) -> &str {
        &self.url
    }

    async fn stop(&self) {
        let server = self.server.lock().await.take();
        if let Some(server) = server {
            let _ = server.shutdown().await;
        }
    }
}

#[cfg(feature = "web")]
struct CliWebServerLauncher;

#[cfg(feature = "web")]
#[async_trait]
impl WebServerLauncher for CliWebServerLauncher {
    async fn start(
        &self,
        registry: Arc<SessionRegistry>,
        opener: Arc<dyn SessionOpener>,
        port: Option<u16>,
        secret: Option<String>,
    ) -> Result<(Arc<dyn WebServerHandle>, WebServerStart)> {
        let validated = match secret {
            Some(raw) if !raw.is_empty() => ServeSecret::validate(raw)?,
            _ => ServeSecret::validate(DEFAULT_SERVE_SECRET)?,
        };
        let secret_plain = validated.expose().to_string();
        let auth = ServeAuth::from_parts(Some(BASIC_USERNAME.to_string()), Some(validated));
        let bind_addr = SocketAddr::from(([127, 0, 0, 1], port.unwrap_or(DEFAULT_SERVE_PORT)));
        let server = start_server(ServeConfig { bind_addr, auth }, opener, registry).await?;
        let addr = server.addr;
        let url = format!("http://{addr}");
        let start = WebServerStart {
            port: addr.port(),
            url: url.clone(),
            secret: secret_plain,
        };
        let handle = Arc::new(CliWebServerHandle {
            addr,
            url,
            server: tokio::sync::Mutex::new(Some(server)),
        });
        Ok((handle, start))
    }
}

#[cfg(feature = "web")]
async fn run_serve(
    serve_cmd: cli::ServeCommand,
    opened: OpenedSession,
    host: Arc<CliSessionHost>,
) -> Result<()> {
    let resolved = resolve_serve_auth(serve_cmd.username, serve_cmd.password, serve_cmd.auth)?;
    let registry = SessionRegistry::new();
    registry.register_opened(opened);
    let bind_addr =
        SocketAddr::from(([127, 0, 0, 1], serve_cmd.port.unwrap_or(DEFAULT_SERVE_PORT)));
    let server = start_server(
        ServeConfig {
            bind_addr,
            auth: resolved.auth.clone(),
        },
        host,
        registry.clone(),
    )
    .await?;
    println!("URL:      http://{}", server.addr);
    match &resolved.auth {
        ServeAuth::None => println!("Auth:     none (loopback only)"),
        ServeAuth::Basic { username, .. } => {
            let user_label = username.as_deref().unwrap_or(BASIC_USERNAME);
            println!("Auth:     Basic (username: {user_label})");
        }
    }
    if let Some(secret) = resolved.default_secret {
        println!("Password: {secret} (default)");
    }
    io::stdout().flush()?;
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for Ctrl-C")?;
    registry.shutdown_all().await;
    server.shutdown().await?;
    Ok(())
}

#[cfg(feature = "web")]
#[derive(Debug)]
struct ResolvedServeAuth {
    auth: ServeAuth,
    default_secret: Option<String>,
}

#[cfg(feature = "web")]
fn resolve_serve_auth(
    cli_username: Option<String>,
    cli_secret: Option<String>,
    prompt_secret: bool,
) -> Result<ResolvedServeAuth> {
    resolve_serve_auth_with(
        cli_username,
        cli_secret,
        std::env::var("MEERAI_SERVE_USER").ok(),
        std::env::var("MEERAI_SERVE_SECRET").ok(),
        prompt_secret && io::stdin().is_terminal(),
        prompt_serve_secret,
    )
}

#[cfg(feature = "web")]
fn resolve_serve_auth_with<F>(
    cli_username: Option<String>,
    cli_secret: Option<String>,
    env_username: Option<String>,
    env_secret: Option<String>,
    should_prompt: bool,
    prompt_fn: F,
) -> Result<ResolvedServeAuth>
where
    F: FnOnce() -> Result<String>,
{
    let username = cli_username
        .or(env_username)
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| BASIC_USERNAME.to_string());

    let (secret, default_secret) = if let Some(raw) = cli_secret.filter(|s| !s.is_empty()) {
        (
            ServeSecret::validate(raw).context("invalid --password")?,
            None,
        )
    } else if let Some(raw) = env_secret.filter(|s| !s.is_empty()) {
        (
            ServeSecret::validate(raw).context("invalid MEERAI_SERVE_SECRET")?,
            None,
        )
    } else if should_prompt {
        let raw = prompt_fn()?;
        if raw.is_empty() {
            (
                ServeSecret::validate(DEFAULT_SERVE_SECRET)?,
                Some(DEFAULT_SERVE_SECRET.to_string()),
            )
        } else {
            (ServeSecret::validate(raw)?, None)
        }
    } else {
        (
            ServeSecret::validate(DEFAULT_SERVE_SECRET)?,
            Some(DEFAULT_SERVE_SECRET.to_string()),
        )
    };

    Ok(ResolvedServeAuth {
        auth: ServeAuth::from_parts(Some(username), Some(secret)),
        default_secret,
    })
}

#[cfg(feature = "web")]
fn prompt_serve_secret() -> Result<String> {
    eprint!("Enter serve password (min 8 chars, leave empty for default): ");
    io::stderr().flush()?;
    read_masked_line()
}

#[cfg(all(feature = "web", unix))]
fn read_masked_line() -> Result<String> {
    use std::process::{Command, Stdio};

    struct EchoGuard {
        saved: Option<String>,
    }

    impl Drop for EchoGuard {
        fn drop(&mut self) {
            let arg = self.saved.as_deref().unwrap_or("echo");
            let _ = Command::new("stty")
                .arg(arg)
                .stdin(Stdio::inherit())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    let saved = Command::new("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let status = Command::new("stty")
        .arg("-echo")
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to disable terminal echo with stty")?;
    if !status.success() {
        bail!("failed to disable terminal echo with stty");
    }
    let _guard = EchoGuard { saved };

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read serve secret from terminal")?;
    eprintln!();
    Ok(line.trim_end_matches(&['\r', '\n'][..]).to_string())
}

#[cfg(all(feature = "web", not(unix)))]
fn read_masked_line() -> Result<String> {
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read serve secret from terminal")?;
    Ok(line.trim_end_matches(&['\r', '\n'][..]).to_string())
}

#[cfg(feature = "tui")]
fn exit_summary(session_id: &str, title: Option<&str>) -> String {
    let session = title.filter(|t| !t.trim().is_empty()).unwrap_or(session_id);
    let logo = meerai_tui::logo::ansi()
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n{logo}\n\n  Session   {session}\n  Continue  meerai --resume {session_id}\n")
}

struct CliAcpSpawner;

impl AcpSpawner for CliAcpSpawner {
    fn spawn(&self, session: Session, cwd: PathBuf) -> AcpChild {
        let (cmd_tx, event_rx) = spawn_acp_worker(AcpWorkerConfig {
            session,
            preset: AGY,
            cwd,
            start: AcpStart::New,
        });
        AcpChild { cmd_tx, event_rx }
    }
}

#[cfg(any(feature = "tui", feature = "web"))]
struct HostLoginInteraction {
    url_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
    paste_rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<String>>,
    cancel_rx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[cfg(any(feature = "tui", feature = "web"))]
#[async_trait]
impl meerai_providers::oauth::LoginInteraction for HostLoginInteraction {
    async fn present_url(&self, url: &str) -> Result<()> {
        let _ = auth_cmd::open_browser(url);
        if let Some(tx) = self.url_tx.lock().await.take() {
            let _ = tx.send(url.to_string());
        }
        Ok(())
    }

    async fn read_pasted_input(&self) -> Result<String> {
        let mut paste = self.paste_rx.lock().await;
        let cancel = self.cancel_rx.lock().await.take();
        match cancel {
            Some(cancel) => tokio::select! {
                line = paste.recv() => line.context("ChatGPT sign-in was cancelled"),
                _ = cancel => bail!("ChatGPT sign-in cancelled"),
            },
            None => paste.recv().await.context("ChatGPT sign-in was cancelled"),
        }
    }
}

fn split_session_model(
    catalog: &ProviderCatalog,
    current_provider: &str,
    raw: &str,
) -> (String, String) {
    if let Some((prefix, model)) = raw.split_once("::") {
        let provider = catalog
            .provider(prefix)
            .map(|provider| provider.id.clone())
            .or_else(|| {
                catalog
                    .providers()
                    .iter()
                    .find(|provider| provider.adapter == prefix)
                    .map(|provider| provider.id.clone())
            })
            .unwrap_or_else(|| current_provider.to_string());
        return (provider, model.trim().to_string());
    }
    (current_provider.to_string(), raw.to_string())
}

#[cfg(any(feature = "tui", feature = "web"))]
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

struct CliSessionHost {
    workspace_root: PathBuf,
    session_config: SessionConfig,
    runtime: Arc<RwLock<RuntimeProviderState>>,
    secrets: Arc<dyn SecretStore>,
    settings: AppSettings,
    user_settings: std::sync::Mutex<AppSettings>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    client: meerai_core::genai::Client,
    approval_mode: ApprovalMode,
    execution_world: Arc<dyn ExecutionWorld>,
    sensitive_env_vars: Vec<String>,
    hooks: Arc<HookEngine>,
    custom_agents: Vec<AgentDefinition>,
    skill_catalog: Arc<Vec<SkillDefinition>>,
    base_system: String,
    launch_system: String,
    launch_definitions: String,
    script_runtime: Option<ScriptRuntime>,
    default_provider: String,
    default_model: String,
}

impl CliSessionHost {
    fn store(&self) -> Result<Store> {
        meerai_common::settings::open_store_for_config_dir(self.config_dir.as_deref())
    }

    fn memory_prompt_block(
        &self,
        context: Option<&meerai_common::settings::ContextSettings>,
    ) -> Option<String> {
        let bytes = context
            .and_then(|context| context.memory_prompt_bytes)
            .map_or(meerai_store::DEFAULT_MEMORY_PROMPT_BYTES as u64, |bytes| {
                bytes
            }) as usize;
        let project_root = find_project_root(&self.workspace_root);
        self.store()
            .ok()
            .and_then(|store| meerai_agents::load_memory_prompt(&store, &project_root, bytes))
    }

    fn resolve_session_provider_model(&self, meta_model: Option<&str>) -> (String, String) {
        let current_provider = self
            .runtime
            .read()
            .map(|rt| rt.active_provider.clone())
            .unwrap_or_else(|_| self.default_provider.clone());
        if let Some(raw) = meta_model.map(str::trim).filter(|m| !m.is_empty()) {
            let catalog = self.runtime.read().ok().map(|rt| rt.catalog.clone());
            if let Some(catalog) = catalog {
                return split_session_model(&catalog, &current_provider, raw);
            }
            if let Some((prefix, model)) = raw.split_once("::") {
                let provider = canonical_provider_id(prefix);
                return (provider, model.trim().to_string());
            }
            return (current_provider, raw.to_string());
        }
        let fallback_model = if current_provider == self.default_provider {
            self.default_model.clone()
        } else {
            self.settings
                .provider_model(&current_provider)
                .map(str::to_string)
                .or_else(|| {
                    self.runtime
                        .read()
                        .ok()
                        .and_then(|rt| rt.catalog.default_model_for_provider(&current_provider))
                })
                .unwrap_or_else(|| self.default_model.clone())
        };
        (current_provider, fallback_model)
    }

    fn build_agent(
        &self,
        session: Session,
        provider: &str,
        model: &str,
        system: String,
    ) -> Result<MultiTurnAgent> {
        let policy = build_policy(
            self.approval_mode,
            self.settings.permissions.as_ref(),
            &self.workspace_root,
        )?;

        let mut readable_roots = vec![self.workspace_root.clone()];
        if let Some(dir) = session.dir() {
            readable_roots.push(dir.to_path_buf());
        }

        let require_for_shell = self
            .settings
            .sandbox
            .as_ref()
            .and_then(|sandbox| sandbox.require_for_shell)
            .unwrap_or(false);
        let require_for_scripts = self
            .settings
            .sandbox
            .as_ref()
            .and_then(|sandbox| sandbox.require_for_scripts)
            .unwrap_or(false);
        let strict_env = self
            .settings
            .sandbox
            .as_ref()
            .and_then(|sandbox| sandbox.strict_env)
            .unwrap_or(false);

        let code_tools = CodeTools::new(self.workspace_root.clone())
            .with_writable_roots(vec![self.workspace_root.clone()])
            .with_readable_roots(readable_roots.clone())
            .with_sensitive_env_vars(self.sensitive_env_vars.clone())
            .with_execution_world(self.execution_world.clone())
            .with_require_sandbox(require_for_shell)
            .with_strict_env(strict_env);
        let repo_map = RepoMapToolset::new(self.workspace_root.clone())
            .with_readable_roots(readable_roots.clone());
        let lsp = LspToolset::new(self.workspace_root.clone()).with_readable_roots(readable_roots);
        let mut sac = SearchAsCodeToolset::new(self.workspace_root.clone())
            .with_sensitive_env_vars(self.sensitive_env_vars.clone())
            .with_execution_world(self.execution_world.clone())
            .with_require_sandbox(require_for_scripts)
            .with_strict_env(strict_env);
        if let Some(runtime) = self.script_runtime {
            sac = sac.with_runtime(runtime);
        }
        let mut initial_tools = build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, false);
        let store = self.store()?;
        let memory_project_root = find_project_root(&self.workspace_root);
        initial_tools.push(Box::new(
            MemoryToolset::new(store.clone(), &memory_project_root)
                .with_source_session(Some(session.id().clone())),
        ));
        initial_tools.push(Box::new(QuestionToolset));
        if self.skill_catalog.iter().any(|skill| skill.model_invocable) {
            initial_tools.push(Box::new(SkillToolset::new(self.skill_catalog.clone())));
        }
        initial_tools.push(Box::new(
            AcpRunToolset::new(Arc::new(CliAcpSpawner)).with_hooks(self.hooks.clone()),
        ));
        let subagent_factory: SubagentToolFactory = Arc::new(move |read_only| {
            build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, read_only)
        });

        let model_spec = if model.trim().is_empty() {
            String::new()
        } else {
            self.runtime
                .read()
                .unwrap()
                .catalog
                .format_genai_spec(provider, model)
        };

        let mut agent = MultiTurnAgent::new(self.client.clone(), model_spec, initial_tools, system);
        agent.set_hooks(self.hooks.clone());
        agent.set_memory_capture(MemoryCapture::new(
            store,
            &memory_project_root,
            Some(session.id().clone()),
        ));
        agent.set_post_edit_diagnostics(
            self.settings
                .context
                .as_ref()
                .and_then(|context| context.post_edit_diagnostics)
                .unwrap_or(false),
        );
        let subagent_tool = SubagentToolset::from_agent(&agent, subagent_factory.clone())
            .with_hooks(self.hooks.clone())
            .with_custom_agents(self.custom_agents.clone());
        agent.add_tool(subagent_tool);
        agent.set_subagent_support(subagent_factory, self.custom_agents.clone());
        let config = self.harness_config(provider, model);
        let routing = Arc::new(RoutingCoordinator::from_settings(
            &config.routing,
            DEFAULT_GLOBAL_CONCURRENCY,
        ));
        let plan_usage = config.plan_usage;
        if let Ok(mut runtime) = self.runtime.write() {
            runtime.active_provider = canonical_provider_id(provider);
            runtime.plan_usage = plan_usage;
        }
        agent.set_harness(AgentHarness {
            session,
            policy,
            config,
            routing: Some(routing),
        });
        Ok(agent)
    }

    fn harness_config(&self, provider: &str, model: &str) -> HarnessConfig {
        let mut config = HarnessConfig::from_context_settings(self.settings.context.as_ref())
            .with_routing(self.settings.routing.clone().unwrap_or_default());
        config.plan_usage = self
            .runtime
            .read()
            .ok()
            .and_then(|runtime| runtime.catalog.provider(provider).cloned())
            .and_then(|provider| {
                resolve_credential(&provider, self.secrets.as_ref())
                    .ok()
                    .flatten()
            })
            .is_some_and(|credential| matches!(credential, Credential::OAuth(_)));
        let configured_window = self
            .settings
            .context
            .as_ref()
            .and_then(|context| context.context_window_tokens);
        let runtime = self.runtime.read().unwrap();
        if let Some(info) = runtime.catalog.model_info(provider, model) {
            if configured_window.is_none()
                && let Some(window) = info.context_window
            {
                config.context_window_tokens = window;
            }
            if let (Some(input), Some(output)) =
                (info.input_cost_per_mtok, info.output_cost_per_mtok)
            {
                config.token_pricing = Some(TokenPricing {
                    input_per_mtok: input,
                    output_per_mtok: output,
                });
            }
        }
        config
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    fn build_opened_session(
        &self,
        session: Session,
        provider: String,
        model: String,
    ) -> Result<OpenedSession> {
        let meta = session.meta();
        if meta.harness.as_deref() == Some("acp:agy") {
            let start = meta
                .remote_session_id
                .clone()
                .map(|id| AcpStart::Load { session_id: id })
                .unwrap_or(AcpStart::New);
            return self.build_acp_opened_session(session, start);
        }
        let events = session.events();
        let agent = self.build_agent(session, &provider, &model, self.base_system.clone())?;
        let (cmd_tx, event_rx) = spawn_worker(agent);
        let approval_mode = meta
            .approval_mode
            .as_deref()
            .and_then(parse_approval_mode)
            .unwrap_or(self.approval_mode);
        Ok(OpenedSession {
            session_id: meta.id,
            title: meta.title,
            backend: Backend::Native,
            provider,
            model,
            approval_mode,
            events,
            cmd_tx,
            event_rx,
        })
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    fn build_acp_opened_session(&self, session: Session, start: AcpStart) -> Result<OpenedSession> {
        let meta = session.meta();
        let events = session.events();
        let (cmd_tx, event_rx) = spawn_acp_worker(AcpWorkerConfig {
            session,
            preset: AGY,
            cwd: self.workspace_root.clone(),
            start,
        });
        let approval_mode = meta
            .approval_mode
            .as_deref()
            .and_then(parse_approval_mode)
            .unwrap_or(self.approval_mode);
        Ok(OpenedSession {
            session_id: meta.id,
            title: meta.title,
            backend: Backend::Acp {
                preset: "agy".to_string(),
                remote_session_id: meta.remote_session_id.clone(),
            },
            provider: "acp:agy".to_string(),
            model: "agy".to_string(),
            approval_mode,
            events,
            cmd_tx,
            event_rx,
        })
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    fn enabled_tool_names(&self) -> Vec<String> {
        let code_tools = CodeTools::new(self.workspace_root.clone());
        let repo_map = RepoMapToolset::new(self.workspace_root.clone());
        let lsp = LspToolset::new(self.workspace_root.clone());
        let mut sac = SearchAsCodeToolset::new(self.workspace_root.clone());
        if let Some(runtime) = self.script_runtime {
            sac = sac.with_runtime(runtime);
        }
        let mut toolsets = build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, false);
        toolsets.push(Box::new(QuestionToolset));
        toolsets.push(Box::new(AcpRunToolset::new(Arc::new(CliAcpSpawner))));
        let mut names = toolsets
            .into_iter()
            .flat_map(|toolset| toolset.definition().into_iter().map(|def| def.name))
            .collect::<Vec<_>>();
        names.push("subagent-run".to_string());
        if self.skill_catalog.iter().any(|skill| skill.model_invocable) {
            names.push(SKILL_TOOL.to_string());
        }
        names
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    fn persist_user_settings(&self, user: &AppSettings) -> Result<()> {
        let dir = self
            .config_dir
            .as_deref()
            .context("no configuration directory available (set HOME or XDG_CONFIG_HOME)")?;
        save_settings_to(dir, user)
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    fn load_merged_settings(&self) -> (AppSettings, AppSettings, Vec<String>) {
        let project_root = find_project_root(&self.workspace_root);
        let user = self
            .config_dir
            .as_deref()
            .and_then(|dir| load_settings_from(dir).ok())
            .unwrap_or_else(|| self.user_settings.lock().unwrap().clone());
        let mut merged = user.clone();
        let mut warnings = Vec::new();
        match load_project_settings(&project_root) {
            Ok(Some(project)) => {
                let (project_merged, project_warnings) =
                    merge_project_settings(user.clone(), project);
                merged = project_merged;
                warnings.extend(project_warnings);
            }
            Ok(None) => {}
            Err(err) => warnings.push(format!("ignoring project settings: {err:#}")),
        }
        (user, merged, warnings)
    }

    #[cfg(any(feature = "tui", feature = "web"))]
    fn compute_host_info_from(&self, merged: &AppSettings, hooks_count: usize) -> HostInfoSnapshot {
        let project_root = find_project_root(&self.workspace_root);
        let definitions = load_definitions_for_runtime(self.config_dir.as_deref(), &project_root);
        let git_branch = meerai_host::current_branch(&self.workspace_root);
        let acp_presets = meerai_acp::presets()
            .iter()
            .map(|preset| AcpPresetInfo {
                id: preset.id.to_string(),
                label: preset.id.to_string(),
                command: format!("{} {}", preset.command, preset.args.join(" "))
                    .trim()
                    .to_string(),
            })
            .collect();
        let agents = definitions
            .agents
            .into_iter()
            .map(|agent| HostAgentInfo {
                id: agent.id,
                source: agent.source.label().to_string(),
                description: agent.description,
                model: agent.model,
            })
            .collect();
        let skills = definitions
            .skills
            .into_iter()
            .map(|skill| HostSkillInfo {
                id: skill.id,
                source: skill.source.label().to_string(),
                description: skill.description,
            })
            .collect();
        let raw_theme = merged.theme.clone().unwrap_or_default();
        let (palette, _) = HostPalette::from_settings(&raw_theme);
        let permissions_summary = merged
            .permissions
            .as_ref()
            .and_then(|value| rules_from_yaml(value).ok())
            .map(|rules| {
                rules
                    .into_iter()
                    .map(|rule| {
                        let action = String::from(rule.action);
                        let effect = match rule.effect {
                            meerai_policy::Effect::Allow => "allow",
                            meerai_policy::Effect::Ask => "ask",
                            meerai_policy::Effect::Deny => "deny",
                        };
                        format!("{effect}: {action}({})", rule.resource)
                    })
                    .collect()
            })
            .unwrap_or_default();
        HostInfoSnapshot {
            workspace_root: self.workspace_root.display().to_string(),
            git_branch,
            acp_presets,
            agents,
            skills,
            mcp_servers: Vec::new(),
            enabled_tools: self.enabled_tool_names(),
            hooks_count,
            update_available: None,
            install_hint: meerai_host::update::install_hint().to_string(),
            settings: HostSettingsView {
                theme: palette.to_hex_tokens(),
                raw_theme,
                context: merged.context.clone().unwrap_or_default(),
                session: merged.sessions.clone().unwrap_or_default(),
                sandbox: merged.sandbox.clone().unwrap_or_default(),
                permissions_summary,
                config_writable: self.config_dir.is_some(),
            },
        }
    }
}

#[cfg(any(feature = "tui", feature = "web"))]
#[async_trait]
impl SessionOpener for CliSessionHost {
    async fn ensure_credentials(&self) -> Result<()> {
        let provider = self
            .runtime
            .read()
            .map(|runtime| runtime.active_provider.clone())
            .unwrap_or_else(|_| self.default_provider.clone());
        meerai_providers::oauth::ensure_fresh_credential(self.secrets.as_ref(), &provider).await
    }

    async fn start_chatgpt_login(&self, provider_id: &str) -> Result<ChatGptLogin> {
        let provider = self
            .runtime
            .read()
            .ok()
            .and_then(|runtime| runtime.catalog.provider(provider_id).cloned())
            .with_context(|| format!("unknown provider '{provider_id}'"))?;
        if !provider.auth.supports_oauth() {
            bail!("provider '{provider_id}' does not support ChatGPT plan sign-in");
        }
        let data_dir = self
            .data_dir
            .clone()
            .context("no data directory; set HOME or XDG_DATA_HOME before signing in")?;
        let store = self.secrets.clone();
        let provider_id = provider_id.to_string();
        let (url_tx, url_rx) = tokio::sync::oneshot::channel();
        let (paste_tx, paste_rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let interaction = HostLoginInteraction {
                url_tx: tokio::sync::Mutex::new(Some(url_tx)),
                paste_rx: tokio::sync::Mutex::new(paste_rx),
                cancel_rx: tokio::sync::Mutex::new(Some(cancel_rx)),
            };
            let login = std::panic::AssertUnwindSafe(meerai_providers::oauth::login_chatgpt(
                store.as_ref(),
                &provider_id,
                &data_dir,
                &interaction,
            ))
            .catch_unwind()
            .await;
            let result = match login {
                Ok(Ok(credential)) => store
                    .write_credential(&provider_id, &Credential::OAuth(credential.clone()))
                    .map(|()| ChatGptLoginOutcome {
                        provider_id: provider_id.clone(),
                        account: credential.account,
                    }),
                Ok(Err(err)) => Err(err),
                Err(panic) => Err(anyhow::anyhow!(
                    "ChatGPT sign-in crashed: {}",
                    panic_message(panic)
                )),
            };
            let _ = result_tx.send(result);
        });
        let url = url_rx.await.context("ChatGPT sign-in did not start")?;
        Ok(ChatGptLogin {
            url,
            paste_tx,
            cancel_tx,
            result_rx,
        })
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let root = self.session_config.root.clone();
        sessions_cmd::run_blocking(move || list_sessions(&root)).await
    }

    async fn open_session(&self, id: &SessionId) -> Result<OpenedSession> {
        let session = Session::open(
            &self.session_config,
            id.clone(),
            self.workspace_root.clone(),
        )
        .await?;
        let meta = session.meta();
        let (provider, model) = self.resolve_session_provider_model(meta.model.as_deref());
        self.build_opened_session(session, provider, model)
    }

    async fn create_session(&self) -> Result<OpenedSession> {
        let session = Session::open_or_create(
            &self.session_config,
            SessionId::new(),
            self.workspace_root.clone(),
        )
        .await?;
        let (provider, model) = self.resolve_session_provider_model(None);
        if !model.trim().is_empty() {
            session.set_model(&model).await?;
        }
        self.build_opened_session(session, provider, model)
    }

    async fn run_user_shell(&self, command: &str) -> Result<String> {
        if self.approval_mode == ApprovalMode::Plan {
            anyhow::bail!("shell commands are disabled in plan mode");
        }
        let sandbox = self.settings.sandbox.as_ref();
        let tools = CodeTools::new(self.workspace_root.clone())
            .with_writable_roots(vec![self.workspace_root.clone()])
            .with_sensitive_env_vars(self.sensitive_env_vars.clone())
            .with_execution_world(self.execution_world.clone())
            .with_require_sandbox(
                sandbox
                    .and_then(|sandbox| sandbox.require_for_shell)
                    .unwrap_or(false),
            )
            .with_strict_env(
                sandbox
                    .and_then(|sandbox| sandbox.strict_env)
                    .unwrap_or(false),
            );
        let args = serde_json::json!({ "command": command }).to_string();
        match tools.invoke("code-shell", &args).await {
            Ok(meerai_core::ToolOutput::Text(text))
            | Ok(meerai_core::ToolOutput::Fail(text))
            | Ok(meerai_core::ToolOutput::Stop(text)) => Ok(text),
            Err(error) => anyhow::bail!("{error}"),
        }
    }

    async fn delete_session(&self, id: &SessionId) -> Result<bool> {
        let root = self.session_config.root.clone();
        let id = id.clone();
        sessions_cmd::run_blocking(move || delete_session_dir(&root, &id)).await
    }

    async fn fork_session(&self, id: &SessionId, turn: &TurnId) -> Result<OpenedSession> {
        let forked = Session::fork_stored(&self.session_config, id, Some(turn)).await?;
        let source_is_acp = forked.meta().harness.as_deref() == Some("acp:agy");
        if source_is_acp {
            return self.build_acp_opened_session(forked, AcpStart::New);
        }
        let meta = forked.meta();
        let (provider, model) = self.resolve_session_provider_model(meta.model.as_deref());
        self.build_opened_session(forked, provider, model)
    }

    async fn create_acp_session(&self, preset: &str) -> Result<OpenedSession> {
        if preset != AGY.id {
            bail!(
                "unknown ACP preset: {preset}. Available: {}",
                meerai_acp::presets()
                    .iter()
                    .map(|preset| preset.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let session = Session::open_or_create(
            &self.session_config,
            SessionId::new(),
            self.workspace_root.clone(),
        )
        .await?;
        self.build_acp_opened_session(session, AcpStart::New)
    }

    async fn read_session_events(&self, id: &SessionId) -> Result<Vec<LoggedEvent>> {
        Session::load_events(&self.session_config, id).await
    }

    fn workspace_root(&self) -> Option<PathBuf> {
        Some(self.workspace_root.clone())
    }

    async fn host_info(&self) -> HostInfoSnapshot {
        let (_user, merged, _warnings) = self.load_merged_settings();
        self.compute_host_info_from(&merged, self.hooks.entries().len())
    }

    async fn providers_snapshot(&self) -> Result<HostProvidersSnapshot> {
        let bundled = ProviderCatalog::load_with_user_dir(None);
        let user_settings = self.user_settings.lock().unwrap().clone();
        let rt = self
            .runtime
            .read()
            .map_err(|_| anyhow::anyhow!("provider runtime lock poisoned"))?;
        let providers = rt
            .catalog
            .providers()
            .iter()
            .map(|provider| {
                let status = check_key_status(provider, self.secrets.as_ref());
                let auth_kind = match provider.auth {
                    ProviderAuthKind::ApiKey => HostAuthKind::ApiKey,
                    ProviderAuthKind::OAuth => HostAuthKind::OAuth,
                    ProviderAuthKind::Either => HostAuthKind::Either,
                    ProviderAuthKind::None => HostAuthKind::None,
                };
                HostProviderItem {
                    id: provider.id.clone(),
                    display_name: provider.display_name.clone(),
                    adapter: provider.adapter.clone(),
                    auth_kind,
                    configured: status.is_configured(),
                    status_badge: status.badge(),
                    env_vars: provider.env_vars.clone(),
                    default_model: provider.default_model.clone(),
                    base_url: rt.effective_base_url(provider),
                    is_custom: bundled.provider(&provider.id).is_none(),
                }
            })
            .collect();
        let models = rt
            .catalog
            .models()
            .iter()
            .map(|model| HostModelItem {
                id: model.id.clone(),
                provider: model.provider.clone(),
                display_name: model.id.clone(),
                context_window_tokens: model.context_window,
                max_output_tokens: None,
                supports_reasoning: true,
                supports_vision: false,
                supports_tools: true,
                is_custom: bundled.model_info(&model.provider, &model.id).is_none(),
            })
            .collect();
        let recent_models = rt
            .catalog
            .providers()
            .iter()
            .filter_map(|provider| {
                let recent = user_settings.recent_models_for(&provider.id);
                (!recent.is_empty()).then(|| (provider.id.clone(), recent.to_vec()))
            })
            .collect();
        Ok(HostProvidersSnapshot {
            default_provider: Some(rt.active_provider.clone()),
            providers,
            models,
            recent_models,
        })
    }

    async fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let canonical = canonical_provider_id(provider_id);
        let exists = self
            .runtime
            .read()
            .ok()
            .and_then(|rt| rt.catalog.provider(&canonical).cloned())
            .is_some();
        if !exists {
            bail!("unknown provider '{canonical}'");
        }
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            bail!("API key cannot be empty");
        }
        self.secrets.set_key(&canonical, trimmed)
    }

    async fn remove_credential(&self, provider_id: &str) -> Result<()> {
        let canonical = canonical_provider_id(provider_id);
        self.secrets.delete_credential(&canonical)
    }

    async fn set_provider_base_url(
        &self,
        provider_id: &str,
        base_url: Option<String>,
    ) -> Result<()> {
        let canonical = canonical_provider_id(provider_id);
        let clean = base_url
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty());
        {
            let mut rt = self
                .runtime
                .write()
                .map_err(|_| anyhow::anyhow!("provider runtime lock poisoned"))?;
            if let Some(ref url) = clean {
                rt.base_urls.insert(canonical.clone(), url.clone());
            } else {
                rt.base_urls.remove(&canonical);
            }
        }
        let updated = {
            let mut user = self.user_settings.lock().unwrap();
            user.set_provider_base_url(&canonical, clean);
            user.clone()
        };
        if self.config_dir.is_some() {
            self.persist_user_settings(&updated)?;
        }
        Ok(())
    }

    async fn select_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<ResolvedModelSelection> {
        let canonical = canonical_provider_id(provider_id);
        let clean_model = model_id.trim().to_string();
        if clean_model.is_empty() {
            bail!("model id cannot be empty");
        }
        let (command_model, context_window_tokens) = {
            let mut rt = self
                .runtime
                .write()
                .map_err(|_| anyhow::anyhow!("provider runtime lock poisoned"))?;
            if rt.catalog.provider(&canonical).is_none() {
                bail!("unknown provider '{canonical}'");
            }
            rt.active_provider = canonical.clone();
            let command_model = rt.catalog.format_genai_spec(&canonical, &clean_model);
            let ctx = rt
                .catalog
                .model_info(&canonical, &clean_model)
                .and_then(|info| info.context_window);
            (command_model, ctx)
        };
        let updated = {
            let mut user = self.user_settings.lock().unwrap();
            user.active_provider = Some(canonical.clone());
            user.active_model = Some(clean_model.clone());
            user.record_provider_model(&canonical, &clean_model);
            user.clone()
        };
        if self.config_dir.is_some() {
            self.persist_user_settings(&updated)?;
        }
        Ok(ResolvedModelSelection {
            provider: canonical,
            display_model: clean_model,
            command_model,
            context_window_tokens,
        })
    }

    async fn resolve_agent_application(
        &self,
        name: &str,
        current_provider: &str,
    ) -> Result<ResolvedAgentApplication> {
        let project_root = find_project_root(&self.workspace_root);
        let definitions = load_definitions_for_runtime(self.config_dir.as_deref(), &project_root);
        let agent = definitions
            .agents
            .into_iter()
            .find(|agent| agent.id == name)
            .with_context(|| format!("unknown agent: {name}"))?;
        let model_raw = match validate_agent_model(agent.model.as_deref()) {
            Ok(raw) => raw.map(str::to_string),
            Err(rejected) => {
                bail!(
                    "agent {} model '{rejected}' implies a provider change; it is not applied",
                    agent.id
                );
            }
        };
        let (model, context_window_tokens) = if let Some(raw) = model_raw {
            let rt = self
                .runtime
                .read()
                .map_err(|_| anyhow::anyhow!("provider runtime lock poisoned"))?;
            let spec = rt.catalog.format_genai_spec(current_provider, &raw);
            let ctx = rt
                .catalog
                .model_info(current_provider, &raw)
                .and_then(|info| info.context_window);
            (Some(spec), ctx)
        } else {
            (None, None)
        };
        Ok(ResolvedAgentApplication {
            name: agent.id,
            instructions: agent.prompt,
            model,
            context_window_tokens,
        })
    }

    async fn resolve_skill_application(&self, name: &str) -> Result<ResolvedSkillApplication> {
        let project_root = find_project_root(&self.workspace_root);
        let definitions = load_definitions_for_runtime(self.config_dir.as_deref(), &project_root);
        let skill = definitions
            .skills
            .into_iter()
            .find(|skill| skill.id == name)
            .with_context(|| format!("unknown skill: {name}"))?;
        Ok(ResolvedSkillApplication {
            name: skill.id,
            instructions: skill.body,
        })
    }

    async fn update_settings(&self, update: HostSettingsUpdate) -> Result<HostReloadOutcome> {
        let mut user = self
            .config_dir
            .as_deref()
            .and_then(|dir| load_settings_from(dir).ok())
            .unwrap_or_else(|| self.user_settings.lock().unwrap().clone());
        if update.reset_theme {
            user.theme = None;
        } else if let Some(theme) = update.theme {
            if theme.is_empty() {
                user.theme = None;
            } else {
                let mut current = user.theme.take().unwrap_or_default();
                current.overlay(theme);
                user.theme = (!current.is_empty()).then_some(current);
            }
        }
        if let Some(context) = update.context {
            user.context = Some(context);
        }
        if let Some(session) = update.session {
            user.sessions = Some(session);
        }
        if let Some(sandbox) = update.sandbox {
            user.sandbox = Some(sandbox);
        }
        if let Some(forget) = update.forget_model {
            user.forget_provider_model(&forget.provider, &forget.model);
        }
        self.persist_user_settings(&user)?;
        *self.user_settings.lock().unwrap() = user;
        self.reload_config().await
    }

    async fn reload_config(&self) -> Result<HostReloadOutcome> {
        let project_root = find_project_root(&self.workspace_root);
        let (user, merged, mut warnings) = self.load_merged_settings();
        *self.user_settings.lock().unwrap() = user.clone();
        let definitions = load_definitions_for_runtime(self.config_dir.as_deref(), &project_root);
        warnings.extend(definitions.warnings);
        let rules = merged
            .permissions
            .as_ref()
            .and_then(|value| rules_from_yaml(value).ok());
        let memory_block = self.memory_prompt_block(merged.context.as_ref());
        let base = meerai_agents::prompt::compose_system_prompt_with_memory(
            &self.launch_system,
            &self.workspace_root,
            memory_block.as_deref(),
        )
        .await;
        let system_prompt = format!("{base}{}", self.launch_definitions);
        let hooks_enabled = merged
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.enabled)
            .unwrap_or(true);
        let hook_entries = if hooks_enabled {
            let (entries, hook_warnings) = load_hooks_for_run(
                self.config_dir.as_deref(),
                &project_root,
                merged.hooks.as_ref(),
            );
            warnings.extend(
                hook_warnings
                    .into_iter()
                    .map(|warning| format!("hook config: {warning}")),
            );
            entries
        } else {
            Vec::new()
        };
        let host_info = self.compute_host_info_from(&merged, hook_entries.len());
        Ok(HostReloadOutcome {
            host_info,
            warnings,
            payload: HostReloadPayload {
                rules,
                context: merged.context.clone(),
                system_prompt,
                hooks: hook_entries,
            },
        })
    }

    async fn list_memories(&self, project_root: &str) -> Result<Vec<ProjectMemory>> {
        self.store()?.memories().list(project_root)
    }

    async fn search_memories(&self, project_root: &str, query: &str) -> Result<Vec<ProjectMemory>> {
        self.store()?.memories().search(project_root, query)
    }

    async fn save_memory(
        &self,
        draft: NewProjectMemory,
        id: Option<&str>,
    ) -> Result<ProjectMemory> {
        let store = self.store()?;
        match id {
            Some(id) => {
                let updated = store.memories().update(
                    &draft.project_root,
                    id,
                    Some(draft.kind),
                    Some(&draft.title),
                    Some(&draft.body),
                )?;
                let _memory = updated.context("memory not found")?;
                store
                    .memories()
                    .set_pinned(&draft.project_root, id, draft.pinned)?
                    .context("memory not found")
            }
            None => store.memories().insert(&draft),
        }
    }

    async fn set_memory_pinned(
        &self,
        project_root: &str,
        id: &str,
        pinned: bool,
    ) -> Result<Option<ProjectMemory>> {
        self.store()?
            .memories()
            .set_pinned(project_root, id, pinned)
    }

    async fn delete_memory(&self, project_root: &str, id: &str) -> Result<bool> {
        self.store()?.memories().delete(project_root, id)
    }

    async fn routes(&self) -> Result<RoutingSettings> {
        self.store()?.routes().load()
    }

    async fn upsert_route(&self, route: ModelRoute) -> Result<()> {
        self.store()?.routes().upsert(&route)
    }

    async fn delete_route(&self, task_type: &str) -> Result<bool> {
        let task = TaskType::parse(task_type)?;
        self.store()?.routes().remove(&task)
    }

    async fn list_packs(&self) -> Result<Vec<InstalledPack>> {
        self.store()?.packs().list()
    }

    async fn install_pack(&self, source: &str, rev: Option<&str>) -> Result<InstalledPack> {
        let pack = meerai_common::packs::install_pack(source, rev)?;
        self.store()?.packs().upsert(&pack)?;
        Ok(pack)
    }

    async fn set_pack_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        self.store()?.packs().set_enabled(id, enabled)
    }

    async fn set_pack_trusted(&self, id: &str, trusted: bool) -> Result<bool> {
        self.store()?.packs().set_trusted(id, trusted)
    }

    async fn pin_pack(&self, id: &str, rev: &str) -> Result<bool> {
        self.store()?.packs().pin(id, rev)
    }

    async fn remove_pack(&self, id: &str) -> Result<bool> {
        self.store()?.packs().remove(id)
    }

    async fn definitions_diagnostics(&self) -> Result<HostDefinitionsDiagnostics> {
        let catalog =
            load_definitions_for_runtime(self.config_dir.as_deref(), &self.workspace_root);
        let agents = catalog
            .agents
            .iter()
            .map(|agent| HostDefinitionInfo {
                id: agent.id.clone(),
                kind: "agent".to_string(),
                source: agent.source.label().to_string(),
                path: agent.path.display().to_string(),
                description: agent.description.clone(),
                model_invocable: None,
                token_estimate: agent.token_estimate,
                warnings: agent.warnings.clone(),
                supporting_files: agent.supporting_files.clone(),
            })
            .collect();
        let skills = catalog
            .skills
            .iter()
            .map(|skill| HostDefinitionInfo {
                id: skill.id.clone(),
                kind: "skill".to_string(),
                source: skill.source.label().to_string(),
                path: skill.path.display().to_string(),
                description: skill.description.clone(),
                model_invocable: Some(skill.model_invocable),
                token_estimate: skill.token_estimate,
                warnings: skill.warnings.clone(),
                supporting_files: skill.supporting_files.clone(),
            })
            .collect();
        let shadows = catalog
            .shadows
            .iter()
            .map(|shadow| meerai_host::HostShadowedDefinition {
                id: shadow.id.clone(),
                kind: shadow.kind.to_string(),
                source: shadow.source.label().to_string(),
                path: shadow.path.display().to_string(),
                kept_source: shadow.kept_source.label().to_string(),
                kept_path: shadow.kept_path.display().to_string(),
            })
            .collect();
        Ok(HostDefinitionsDiagnostics {
            agents,
            skills,
            warnings: catalog.warnings,
            shadows,
        })
    }

    async fn reset_vault(&self) -> Result<usize> {
        let data_dir = self
            .data_dir
            .as_deref()
            .context("no data directory; set HOME or XDG_DATA_HOME")?;
        meerai_providers::secrets::open_vault(Some(data_dir))?.reset()
    }

    async fn read_session_blob(&self, id: &SessionId, blob_key: &str) -> Result<String> {
        let config = SessionConfig {
            root: default_sessions_root(),
            workspace: self.workspace_root.clone(),
        };
        meerai_session::read_session_blob(&config, id, blob_key)
            .await?
            .with_context(|| format!("no spilled blob {blob_key} for session {id}"))
    }
}
