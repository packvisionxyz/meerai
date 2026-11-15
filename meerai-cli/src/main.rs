mod auth_cmd;
mod cli;
mod headless;
mod sessions_cmd;
#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::{Result, bail};
use meerai_agents::{AgentHarness, HarnessConfig, MultiTurnAgent, spawn_worker};
use meerai_common::definitions::{AgentDefinition, SkillDefinition, load_definitions};
use meerai_common::settings::{
    default_config_dir, default_data_dir, find_project_root, load_project_settings,
    load_settings_from, merge_project_settings,
};
use meerai_policy::{ApprovalMode, PolicyEngine, parse_approval_mode, rules_from_yaml};
use meerai_providers::providers::{
    ProviderCatalog, RuntimeProviderState, build_genai_client, canonical_provider_id,
    first_configured_provider,
};
use meerai_providers::secrets::{auth_file_path, default_secret_store};
use meerai_session::{Session, SessionConfig, SessionId, default_sessions_root};
use meerai_tools::CodeTools;
use meerai_tui::{TuiLaunchConfig, transcript_from_events};

use auth_cmd::run_auth;
use cli::{CliSubcommand, HELP_TEXT, parse_args};
use headless::run_headless;
use sessions_cmd::{auto_prune_sessions, continue_session_id, run_sessions};

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

    let model_spec = if active_model.trim().is_empty() {
        String::new()
    } else {
        catalog.format_genai_spec(&active_provider, &active_model)
    };
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
        .with_readable_roots(readable_roots)
        .with_sensitive_env_vars(sensitive_env_vars);

    let system = compose_definition_prompt(
        meerai_agents::compose_system_prompt(&cli.system, &workspace_root).await,
        &selected_skills,
        selected_agent,
    );
    let mut agent = MultiTurnAgent::new(client, model_spec, vec![Box::new(code_tools)], system);
    agent.set_harness(AgentHarness {
        session,
        policy,
        config: HarnessConfig::from_context_settings(settings.context.as_ref()),
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
