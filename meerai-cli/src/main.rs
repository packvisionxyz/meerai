#![cfg_attr(
    not(feature = "tui"),
    allow(dead_code, unused_imports, unused_variables)
)]

mod acp_cmd;
mod auth_cmd;
mod cli;
mod headless;
mod hooks_cmd;
mod sessions_cmd;
#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use anyhow::{Result, bail};
use meerai_acp::{AGY, AcpStart, AcpWorkerConfig, spawn_acp_worker};
#[cfg(feature = "tui")]
use meerai_agents::spawn_worker;
use meerai_agents::{
    AcpChild, AcpRunToolset, AcpSpawner, AgentHarness, HarnessConfig, HookEngine, MultiTurnAgent,
    QuestionToolset, SubagentToolFactory, SubagentToolset, TokenPricing,
};
use meerai_client::providers::{
    ProviderCatalog, RuntimeProviderState, canonical_provider_id, first_configured_provider,
};
use meerai_client::secrets::auth_file_path;
use meerai_common::definitions::{AgentDefinition, SkillDefinition, load_definitions};
use meerai_common::hooks::{add_trusted_project, load_hooks_for_run, project_is_trusted};
use meerai_common::settings::{
    AppSettings, HooksSettings, default_config_dir, default_data_dir, find_project_root,
    load_project_settings, load_settings_from, merge_project_settings, save_settings_to,
};
use meerai_core::{Toolset, async_trait};
use meerai_policy::{ApprovalMode, PolicyEngine, parse_approval_mode, rules_from_yaml};
use meerai_providers::providers::build_genai_client;
use meerai_providers::secrets::default_secret_store;
use meerai_session::{
    LoggedEvent, Session, SessionConfig, SessionId, SessionSummary, TurnId, default_sessions_root,
    delete_session as delete_session_dir, list_sessions,
};
use meerai_tools::search_as_code::ScriptRuntime;
use meerai_tools::{
    CodeTools, ExecutionWorld, LspToolset, RepoMapToolset, SearchAsCodeToolset,
    detect_execution_world,
};
#[cfg(feature = "tui")]
use meerai_tui::{Backend, OpenedSession, SessionOpener, TuiLaunchConfig, transcript_from_events};

use auth_cmd::run_auth;
use cli::{CliSubcommand, help_text, parse_args};
use headless::{run_headless, run_headless_channels};
use hooks_cmd::run_hooks;
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
            anyhow::anyhow!(
                "invalid --mode value '{raw}': expected plan, ask, auto-edit, auto-approve, yolo"
            )
        });
    }
    match configured {
        Some(raw) => parse_approval_mode(raw).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid approval_mode '{raw}' in settings.yml: expected plan, ask, auto-edit, auto-approve, yolo"
            )
        }),
        None => Ok(ApprovalMode::Ask),
    }
}

#[cfg(feature = "tui")]
fn apply_local_theme(workspace_root: &Path, project_root: &Path, settings: &mut AppSettings) {
    if workspace_root == project_root {
        return;
    }
    match load_project_settings(workspace_root) {
        Ok(Some(local)) => {
            if let Some(local_theme) = local.theme {
                let mut theme = settings.theme.take().unwrap_or_default();
                theme.overlay(local_theme);
                settings.theme = Some(theme);
            }
        }
        Ok(None) => {}
        Err(err) => {
            eprintln!(
                "Warning: ignoring working-directory theme ({}): {err:#}",
                workspace_root
                    .join(".meerai")
                    .join("settings.yml")
                    .display()
            );
        }
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

    if let Some(CliSubcommand::Sessions(command)) = cli.subcommand.as_ref() {
        return run_sessions(command).await;
    }

    let workspace_root = std::env::current_dir()?;
    let project_root = find_project_root(&workspace_root);
    if let Some(CliSubcommand::Acp(command)) = cli.subcommand.as_ref() {
        return acp_cmd::run(*command, &workspace_root).await;
    }
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
        apply_local_theme(&workspace_root, &project_root, &mut settings);
        let (palette, theme_warnings) =
            meerai_tui::theme::Palette::from_settings(&settings.theme.clone().unwrap_or_default());
        for warning in theme_warnings {
            eprintln!("Warning: {warning}");
        }
        meerai_tui::theme::install(palette);
    }
    let secrets = default_secret_store(data_dir.as_deref());
    let auth_path = data_dir.as_deref().map(auth_file_path);

    if let Some(CliSubcommand::Auth(auth_cmd)) = cli.subcommand {
        return run_auth(
            auth_cmd,
            &catalog,
            secrets.as_ref(),
            auth_path.as_deref(),
            config_dir.as_deref(),
        );
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
    #[cfg(not(feature = "tui"))]
    if cli.prompt.is_none() {
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
    let restore_ids = if cli.prompt.is_none()
        && !cli.continue_session
        && cli.resume.is_none()
        && cli.fork.is_none()
    {
        meerai_tui::tabs::saved_sessions_for(&workspace_root)
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "tui"))]
    let restore_ids: Vec<SessionId> = Vec::new();
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
    } else if let Some(first) = restore_ids.first() {
        match Session::open(&session_config, first.clone(), workspace_root.clone()).await {
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
    prune_empty_sessions(session.id()).await;
    auto_prune_sessions(&settings, session.id()).await;

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
    let base_system = meerai_agents::compose_system_prompt(&cli.system, &workspace_root).await;
    let base_system = meerai_agents::prompt::append_script_guidance(
        base_system,
        script_runtime.map(ScriptRuntime::program),
    );
    let initial_system =
        compose_definition_prompt(base_system.clone(), &selected_skills, selected_agent);
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

    let host = Arc::new(CliSessionHost {
        workspace_root: workspace_root.clone(),
        session_config,
        runtime: runtime.clone(),
        settings: settings.clone(),
        client,
        approval_mode,
        execution_world,
        sensitive_env_vars,
        hooks,
        custom_agents: definitions.agents.clone(),
        base_system,
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
                resumed: Arc::new(AtomicBool::new(false)),
            });
            let exit_code = run_headless_channels(cmd_tx, event_rx, prompt, cli.json).await?;
            std::process::exit(exit_code);
        }
        let agent = host.build_agent(session, &active_provider, &active_model, initial_system)?;
        let exit_code = run_headless(agent, prompt, cli.json).await?;
        std::process::exit(exit_code);
    }

    #[cfg(feature = "tui")]
    let opened = if use_acp {
        host.build_acp_opened_session(session, acp_start)?
    } else {
        let meta = session.meta();
        let events = session.events();
        let transcript = transcript_from_events(&events);
        let agent = host.build_agent(session, &active_provider, &active_model, initial_system)?;
        let (cmd_tx, event_rx) = spawn_worker(agent);
        OpenedSession {
            session_id: meta.id,
            title: meta.title,
            backend: Backend::Native,
            provider: active_provider.clone(),
            model: active_model.clone(),
            approval_mode,
            transcript,
            events,
            cmd_tx,
            event_rx,
        }
    };
    #[cfg(feature = "tui")]
    let update_available = meerai_tui::update::check_latest().await;

    #[cfg(feature = "tui")]
    return meerai_tui::run_tui(
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
            config_dir,
            approval_mode: opened.approval_mode,
            initial_transcript: opened.transcript,
            initial_events: opened.events,
            agents: definitions.agents,
            skills: definitions.skills,
            session_opener: Some(host),
            update_available,
            restore_sessions: restore_ids.iter().skip(1).cloned().collect(),
            launch_system: cli.system,
            launch_definitions,
            workspace_query: None,
        },
    )
    .await;

    #[cfg(not(feature = "tui"))]
    {
        let _ = session;
        let _ = initial_system;
        Ok(())
    }
}

struct CliAcpSpawner;

impl AcpSpawner for CliAcpSpawner {
    fn spawn(&self, session: Session, cwd: PathBuf) -> AcpChild {
        let (cmd_tx, event_rx) = spawn_acp_worker(AcpWorkerConfig {
            session,
            preset: AGY,
            cwd,
            start: AcpStart::New,
            resumed: Arc::new(AtomicBool::new(false)),
        });
        AcpChild { cmd_tx, event_rx }
    }
}

struct CliSessionHost {
    workspace_root: PathBuf,
    session_config: SessionConfig,
    runtime: Arc<RwLock<RuntimeProviderState>>,
    settings: AppSettings,
    client: meerai_core::genai::Client,
    approval_mode: ApprovalMode,
    execution_world: Arc<dyn ExecutionWorld>,
    sensitive_env_vars: Vec<String>,
    hooks: Arc<HookEngine>,
    custom_agents: Vec<AgentDefinition>,
    base_system: String,
    script_runtime: Option<ScriptRuntime>,
    default_provider: String,
    default_model: String,
}

impl CliSessionHost {
    fn resolve_session_provider_model(&self, meta_model: Option<&str>) -> (String, String) {
        let current_provider = self
            .runtime
            .read()
            .map(|rt| rt.active_provider.clone())
            .unwrap_or_else(|_| self.default_provider.clone());
        if let Some(raw) = meta_model.map(str::trim).filter(|m| !m.is_empty()) {
            if let Some((provider, model)) = raw.split_once("::") {
                return (canonical_provider_id(provider), model.trim().to_string());
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

        let code_tools = CodeTools::new(self.workspace_root.clone())
            .with_writable_roots(vec![self.workspace_root.clone()])
            .with_readable_roots(readable_roots.clone())
            .with_sensitive_env_vars(self.sensitive_env_vars.clone())
            .with_execution_world(self.execution_world.clone());
        let repo_map = RepoMapToolset::new(self.workspace_root.clone())
            .with_readable_roots(readable_roots.clone());
        let lsp = LspToolset::new(self.workspace_root.clone()).with_readable_roots(readable_roots);
        let mut sac = SearchAsCodeToolset::new(self.workspace_root.clone())
            .with_sensitive_env_vars(self.sensitive_env_vars.clone())
            .with_execution_world(self.execution_world.clone());
        if let Some(runtime) = self.script_runtime {
            sac = sac.with_runtime(runtime);
        }
        let mut initial_tools = build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, false);
        initial_tools.push(Box::new(QuestionToolset));
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
        agent.set_post_edit_diagnostics(
            self.settings
                .context
                .as_ref()
                .and_then(|context| context.post_edit_diagnostics)
                .unwrap_or(false),
        );
        let subagent_tool = SubagentToolset::from_agent(&agent, subagent_factory)
            .with_hooks(self.hooks.clone())
            .with_custom_agents(self.custom_agents.clone());
        agent.add_tool(subagent_tool);
        agent.set_harness(AgentHarness {
            session,
            policy,
            config: self.harness_config(provider, model),
        });
        Ok(agent)
    }

    fn harness_config(&self, provider: &str, model: &str) -> HarnessConfig {
        let mut config = HarnessConfig::from_context_settings(self.settings.context.as_ref());
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

    #[cfg(feature = "tui")]
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
        let transcript = transcript_from_events(&events);
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
            transcript,
            events,
            cmd_tx,
            event_rx,
        })
    }

    #[cfg(feature = "tui")]
    fn build_acp_opened_session(&self, session: Session, start: AcpStart) -> Result<OpenedSession> {
        let meta = session.meta();
        let events = session.events();
        let transcript = transcript_from_events(&events);
        let (cmd_tx, event_rx) = spawn_acp_worker(AcpWorkerConfig {
            session,
            preset: AGY,
            cwd: self.workspace_root.clone(),
            start,
            resumed: Arc::new(AtomicBool::new(false)),
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
            transcript,
            events,
            cmd_tx,
            event_rx,
        })
    }
}

#[cfg(feature = "tui")]
#[async_trait]
impl SessionOpener for CliSessionHost {
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

    async fn delete_session(&self, id: &SessionId) -> Result<bool> {
        let root = self.session_config.root.clone();
        let id = id.clone();
        sessions_cmd::run_blocking(move || delete_session_dir(&root, &id)).await
    }

    async fn fork_session(&self, id: &SessionId, turn: &TurnId) -> Result<OpenedSession> {
        let source = Session::open(
            &self.session_config,
            id.clone(),
            self.workspace_root.clone(),
        )
        .await?;
        let source_is_acp = source.meta().harness.as_deref() == Some("acp:agy");
        let forked = Session::fork(&self.session_config, &source, turn).await?;
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
        let session = Session::open(
            &self.session_config,
            id.clone(),
            self.workspace_root.clone(),
        )
        .await?;
        Ok(session.events())
    }
}
