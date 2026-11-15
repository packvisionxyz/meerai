mod auth_cmd;
mod cli;
mod headless;
mod sessions_cmd;
#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Result, bail};
use meerai_agents::{
    AgentHarness, HarnessConfig, MultiTurnAgent, SubagentToolFactory, SubagentToolset, spawn_worker,
};
use meerai_common::definitions::{AgentDefinition, SkillDefinition, load_definitions};
use meerai_common::settings::{
    AppSettings, default_config_dir, default_data_dir, find_project_root, load_project_settings,
    load_settings_from, merge_project_settings,
};
use meerai_core::{Toolset, async_trait};
use meerai_policy::{ApprovalMode, PolicyEngine, parse_approval_mode, rules_from_yaml};
use meerai_providers::providers::{
    ProviderCatalog, RuntimeProviderState, build_genai_client, canonical_provider_id,
    first_configured_provider,
};
use meerai_providers::secrets::{auth_file_path, default_secret_store};
use meerai_session::{
    Session, SessionConfig, SessionId, SessionSummary, TurnId, default_sessions_root, list_sessions,
};
use meerai_tools::search_as_code::ScriptRuntime;
use meerai_tools::{
    CodeTools, ExecutionWorld, LspToolset, RepoMapToolset, SearchAsCodeToolset,
    detect_execution_world,
};
use meerai_tui::{OpenedSession, SessionOpener, TuiLaunchConfig, transcript_from_events};

use auth_cmd::run_auth;
use cli::{CliSubcommand, HELP_TEXT, parse_args};
use headless::run_headless;
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
    apply_local_theme(&workspace_root, &project_root, &mut settings);
    let (palette, theme_warnings) =
        meerai_tui::theme::Palette::from_settings(&settings.theme.clone().unwrap_or_default());
    for warning in theme_warnings {
        eprintln!("Warning: {warning}");
    }
    meerai_tui::theme::install(palette);
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
    } else {
        Session::open_or_create(&session_config, SessionId::new(), workspace_root.clone()).await?
    };
    if !active_model.trim().is_empty() {
        session.set_model(&active_model).await?;
    }
    prune_empty_sessions(session.id()).await;
    auto_prune_sessions(&settings, session.id()).await;

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

    let host = Arc::new(CliSessionHost {
        workspace_root: workspace_root.clone(),
        session_config,
        runtime: runtime.clone(),
        settings: settings.clone(),
        client,
        approval_mode,
        execution_world,
        sensitive_env_vars,
        custom_agents: definitions.agents.clone(),
        base_system,
        script_runtime,
        default_provider: active_provider.clone(),
        default_model: active_model.clone(),
    });

    if let Some(prompt) = cli.prompt {
        let agent = host.build_agent(session, &active_provider, &active_model, initial_system)?;
        let exit_code = run_headless(agent, prompt, cli.json).await?;
        std::process::exit(exit_code);
    }

    let initial_meta = session.meta();
    let initial_events = session.events();
    let initial_transcript = transcript_from_events(&initial_events);
    let agent = host.build_agent(session, &active_provider, &active_model, initial_system)?;
    let (cmd_tx, event_rx) = spawn_worker(agent);

    meerai_tui::run_tui(
        cmd_tx,
        event_rx,
        TuiLaunchConfig {
            initial_session_id: initial_meta.id,
            initial_session_title: initial_meta.title,
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
            session_opener: Some(host),
        },
    )
    .await
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
        let initial_tools = build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, false);
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
        let subagent_tool = SubagentToolset::from_agent(&agent, subagent_factory)
            .with_custom_agents(self.custom_agents.clone());
        agent.add_tool(subagent_tool);
        agent.set_harness(AgentHarness {
            session,
            policy,
            config: HarnessConfig::from_context_settings(self.settings.context.as_ref()),
        });
        Ok(agent)
    }

    fn build_opened_session(
        &self,
        session: Session,
        provider: String,
        model: String,
    ) -> Result<OpenedSession> {
        let meta = session.meta();
        let events = session.events();
        let transcript = transcript_from_events(&events);
        let agent = self.build_agent(session, &provider, &model, self.base_system.clone())?;
        let (cmd_tx, event_rx) = spawn_worker(agent);
        Ok(OpenedSession {
            session_id: meta.id,
            title: meta.title,
            provider,
            model,
            approval_mode: self.approval_mode,
            transcript,
            events,
            cmd_tx,
            event_rx,
        })
    }
}

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

    async fn fork_session(&self, id: &SessionId, turn: &TurnId) -> Result<OpenedSession> {
        let source = Session::open(
            &self.session_config,
            id.clone(),
            self.workspace_root.clone(),
        )
        .await?;
        let forked = Session::fork(&self.session_config, &source, turn).await?;
        let meta = forked.meta();
        let (provider, model) = self.resolve_session_provider_model(meta.model.as_deref());
        self.build_opened_session(forked, provider, model)
    }
}
