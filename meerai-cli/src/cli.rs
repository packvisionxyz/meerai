use std::path::PathBuf;

use anyhow::{Result, bail};

struct HelpRow {
    usage: &'static str,
    description: &'static str,
}

const COMMANDS: &[HelpRow] = &[
    HelpRow {
        usage: "acp doctor",
        description: "Connect to the agy ACP adapter, print capabilities, exit",
    },
    HelpRow {
        usage: "auth list",
        description: "Show credentials across the encrypted vault, config, and env",
    },
    HelpRow {
        usage: "auth login <provider> [key] [--chatgpt]",
        description: "Save an API key, or sign in with a ChatGPT plan, in {auth_path}",
    },
    HelpRow {
        usage: "auth logout <provider>",
        description: "Remove stored credentials for one provider",
    },
    HelpRow {
        usage: "auth reset-vault [--yes]",
        description: "Delete every saved credential, reinitialize the vault, and require login",
    },
    HelpRow {
        usage: "config export",
        description: "Print stored user settings as YAML without resolving environment references",
    },
    HelpRow {
        usage: "config import <PATH>",
        description: "Replace stored user settings from YAML; API keys must be environment references",
    },
    HelpRow {
        usage: "sessions list",
        description: "List saved sessions: id, updated ms, bytes, title/model",
    },
    HelpRow {
        usage: "sessions rm <ID>",
        description: "Delete one session",
    },
    HelpRow {
        usage: "sessions rm --all",
        description: "Delete every saved session",
    },
    HelpRow {
        usage: "sessions prune [--days N] [--keep N]",
        description: "Delete sessions older than N days (default 30), keeping the newest N (default 0)",
    },
    HelpRow {
        usage: "sessions fork <ID> [--turn <TURN_ID>]",
        description: "Fork a session at a closed turn (default: latest closed turn)",
    },
    HelpRow {
        usage: "sessions export <ID> [--jsonl]",
        description: "Print every stored session event as JSONL",
    },
    HelpRow {
        usage: "memory [list] [query] [--project PATH]",
        description: "List project memories or search them with FTS5",
    },
    HelpRow {
        usage: "memory save <kind> <title> [body] [--pin]",
        description: "Store a project memory (decision, progress, fact, convention)",
    },
    HelpRow {
        usage: "memory delete <id> | memory pin <id> [--off]",
        description: "Delete or pin a project memory",
    },
    HelpRow {
        usage: "routes [list] | routes set <task> <provider::model> [max_cost] [max_tokens] [concurrency]",
        description: "List or edit task-type model routes",
    },
    HelpRow {
        usage: "routes clear <task>",
        description: "Remove a task-type model route",
    },
    HelpRow {
        usage: "pack [list] | pack add <path|git-url> [--rev <rev>]",
        description: "List packs or install one from a local path or Git repository",
    },
    HelpRow {
        usage: "pack enable|disable|trust|untrust <id>",
        description: "Change a pack's enabled and trusted state",
    },
    HelpRow {
        usage: "pack pin <id> <rev> | pack remove <id>",
        description: "Pin a pack revision or remove a pack",
    },
    HelpRow {
        usage: "hooks status",
        description: "Show hook settings and project trust state",
    },
    HelpRow {
        usage: "hooks trust",
        description: "Trust project hooks for this repository",
    },
    HelpRow {
        usage: "hooks untrust",
        description: "Revoke project hook trust for this repository",
    },
    HelpRow {
        usage: "hooks verify [--command CMD] [--reset-baseline]",
        description: "Install a Stop hook with a failure baseline",
    },
    HelpRow {
        usage: "serve [--port N] [--user NAME] [--password PASS] [--auth]",
        description: "Start the local browser UI server on 127.0.0.1:28860 (Basic auth required)",
    },
];

const TUI_COMMANDS: &[HelpRow] = &[
    HelpRow {
        usage: "/context",
        description: "Inspect the context breakdown and token headroom",
    },
    HelpRow {
        usage: "/memory [query] | /remember <note>",
        description: "List, search, or save project memories",
    },
    HelpRow {
        usage: "/routing [set <task> <provider::model>]",
        description: "List or edit task-type model routes",
    },
    HelpRow {
        usage: "/packs | /pack <verb> <id>",
        description: "Inspect packs or change their enabled and trusted state",
    },
    HelpRow {
        usage: "/subagent <agent> <task> | /subagent cancel <item>",
        description: "Spawn or cancel a manual subagent",
    },
    HelpRow {
        usage: "/definitions",
        description: "Show discovered agents and skills",
    },
    HelpRow {
        usage: "/agent <id>",
        description: "Apply an agent by id (or mention @agent/<id>)",
    },
    HelpRow {
        usage: "/skill <id>",
        description: "Apply a skill by id (or mention @skill/<id>)",
    },
    HelpRow {
        usage: "/serve [stop]",
        description: "Start or stop the local browser UI server",
    },
];

const OPTIONS: &[HelpRow] = &[
    HelpRow {
        usage: "-p, --provider <PROVIDER>",
        description: "Provider id (env: MEERAI_PROVIDER)",
    },
    HelpRow {
        usage: "-m, --model <MODEL>",
        description: "Model id (env: MEERAI_MODEL)",
    },
    HelpRow {
        usage: "--system <PROMPT>",
        description: "Additional system instructions",
    },
    HelpRow {
        usage: "--prompt, --print <TEXT>",
        description: "Run one headless turn (no TUI)",
    },
    HelpRow {
        usage: "--continue",
        description: "Resume the most recent session (TUI or headless)",
    },
    HelpRow {
        usage: "--resume <ID>",
        description: "Resume a session by id (TUI or headless)",
    },
    HelpRow {
        usage: "--fork <ID>",
        description: "Fork a session from its latest closed turn and open it",
    },
    HelpRow {
        usage: "--json",
        description: "Print one NDJSON AgentEvent per line (with --prompt)",
    },
    HelpRow {
        usage: "--mode <MODE>",
        description: "Approval mode: plan, ask, auto",
    },
    HelpRow {
        usage: "--acp",
        description: "Use the external ACP agent (agy) instead of the built-in harness",
    },
    HelpRow {
        usage: "--agent <ID>",
        description: "Apply a markdown agent by id",
    },
    HelpRow {
        usage: "--skill <ID>",
        description: "Apply a markdown skill by id (repeatable)",
    },
    HelpRow {
        usage: "--env-file <PATH>",
        description: "Load variables from PATH (no implicit .env loading)",
    },
    HelpRow {
        usage: "-h, --help",
        description: "Show this help message",
    },
    HelpRow {
        usage: "-V, --version",
        description: "Print version and exit",
    },
];

fn help_section(title: &str, rows: &[HelpRow], auth_path: &str) -> String {
    let width = rows
        .iter()
        .map(|row| row.usage.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = format!("{title}:\n");
    for row in rows {
        let description = row.description.replace("{auth_path}", auth_path);
        out.push_str(&format!("  {:<width$}  {description}\n", row.usage));
    }
    out
}

pub(crate) fn help_text() -> String {
    let auth_path = meerai_common::settings::default_data_dir()
        .map(|dir| dir.join("meerai.db"))
        .map(|path| meerai_core::env::display_path(&path))
        .unwrap_or_else(|| "meerai.db".to_string());
    let mut text = String::from(
        "Usage:\n  \
    meerai [-p|--provider <PROVIDER>] [-m|--model <MODEL>] [--system <INSTRUCTIONS>]\n          \
    [--mode <plan|ask|auto>] [--continue | --resume <ID> | --fork <ID>] [--env-file <PATH>]\n          \
    [--acp] [--agent <ID>] [--skill <ID>]... [-h|--help] [-V|--version]\n  \
    meerai --prompt <TEXT> [--json] [--acp] [--continue | --resume <ID> | --fork <ID>] [--mode <MODE>]\n          \
    [--agent <ID>] [--skill <ID>]...\n  \
    meerai acp doctor\n  \
    meerai auth <list|login|logout>\n  \
    meerai config export\n  \
    meerai config import <PATH>\n  \
    meerai sessions <list|rm|prune|fork>\n  \
    meerai hooks <trust|untrust|status|verify>\n  \
    meerai serve [--port <PORT>] [--user <NAME>] [--password <PASS>] [--auth] [--continue | --resume <ID> | --fork <ID>]\n\n",
    );
    text.push_str(&help_section("Commands", COMMANDS, &auth_path));
    text.push('\n');
    text.push_str(&help_section("TUI commands", TUI_COMMANDS, &auth_path));
    text.push('\n');
    text.push_str(&help_section("Options", OPTIONS, &auth_path));
    text.trim_end().to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AuthCommand {
    List,
    Login {
        provider: String,
        key: Option<String>,
        chatgpt: bool,
    },
    Logout {
        provider: String,
    },
    ResetVault {
        yes: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigCommand {
    Export,
    Import(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SessionTarget {
    Id(String),
    All,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SessionsCommand {
    List,
    Remove { target: SessionTarget },
    Prune { days: u64, keep: usize },
    Fork { id: String, turn: Option<String> },
    Export { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemoryCommand {
    List {
        project: Option<String>,
        query: Option<String>,
    },
    Save {
        project: Option<String>,
        kind: String,
        title: String,
        body: Option<String>,
        pinned: bool,
    },
    Delete {
        project: Option<String>,
        id: String,
    },
    Pin {
        project: Option<String>,
        id: String,
        pinned: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RoutesCommand {
    List,
    Set {
        task: String,
        spec: String,
        effort: Option<String>,
        max_cost_usd: Option<f64>,
        max_tokens_per_turn: Option<u64>,
        max_concurrency: Option<usize>,
    },
    Clear {
        task: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackCommand {
    List,
    Add { source: String, rev: Option<String> },
    SetEnabled { id: String, enabled: bool },
    SetTrusted { id: String, trusted: bool },
    Pin { id: String, rev: String },
    Remove { id: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HooksCommand {
    Trust,
    Untrust,
    Status,
    Verify {
        command: Option<String>,
        reset_baseline: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcpCommand {
    Doctor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServeCommand {
    pub(crate) port: Option<u16>,
    pub(crate) username: Option<String>,
    pub(crate) password: Option<String>,
    pub(crate) auth: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum CliSubcommand {
    Acp(AcpCommand),
    Auth(AuthCommand),
    Config(ConfigCommand),
    Hooks(HooksCommand),
    Memory(MemoryCommand),
    Pack(PackCommand),
    Routes(RoutesCommand),
    Serve(ServeCommand),
    Sessions(SessionsCommand),
}

#[derive(Debug)]
pub(crate) struct CliArgs {
    pub(crate) subcommand: Option<CliSubcommand>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) system: String,
    pub(crate) prompt: Option<String>,
    pub(crate) json: bool,
    pub(crate) acp: bool,
    pub(crate) mode: Option<String>,
    pub(crate) env_file: Option<String>,
    pub(crate) continue_session: bool,
    pub(crate) resume: Option<String>,
    pub(crate) fork: Option<String>,
    pub(crate) agents: Vec<String>,
    pub(crate) skills: Vec<String>,
    pub(crate) show_help: bool,
    pub(crate) version: bool,
}

pub(crate) fn parse_count(flag: &str, raw: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("invalid value for {flag}: {raw} (expected a number)"))
}

pub(crate) fn parse_keep(raw: &str) -> Result<usize> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("invalid value for --keep: {raw} (expected a number)"))
}

pub(crate) fn parse_port(raw: &str) -> Result<u16> {
    raw.trim()
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("invalid value for --port: {raw} (expected 0..=65535)"))
}

pub(crate) fn parse_args<I>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut subcommand = None;
    let mut provider = None;
    let mut model = None;
    let mut system = String::new();
    let mut prompt = None;
    let mut json = false;
    let mut acp = false;
    let mut mode = None;
    let mut env_file = None;
    let mut continue_session = false;
    let mut resume = None;
    let mut fork = None;
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
            "acp" if subcommand.is_none() => match iter.next().as_deref() {
                None | Some("doctor") => {
                    if let Some(extra) = iter.next() {
                        match extra.as_str() {
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for acp doctor: {extra}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Acp(AcpCommand::Doctor));
                }
                Some("-h" | "--help") => {
                    show_help = true;
                }
                Some(other) => {
                    bail!("unknown acp subcommand: {other}. Expected: doctor");
                }
            },
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
                        anyhow::anyhow!("usage: meerai auth login <provider> [api-key] [--chatgpt]")
                    })?;
                    if target_provider == "-h" || target_provider == "--help" {
                        show_help = true;
                        continue;
                    }
                    let mut key = None;
                    let mut chatgpt = false;
                    for extra in iter.by_ref() {
                        match extra.as_str() {
                            "--chatgpt" => chatgpt = true,
                            "-h" | "--help" => show_help = true,
                            other if key.is_none() && !other.starts_with('-') => {
                                key = Some(other.to_string());
                            }
                            other => {
                                bail!("unexpected argument for auth login: {other}");
                            }
                        }
                    }
                    if chatgpt && key.is_some() {
                        bail!("auth login takes either an API key or --chatgpt, not both");
                    }
                    subcommand = Some(CliSubcommand::Auth(AuthCommand::Login {
                        provider: target_provider,
                        key,
                        chatgpt,
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
                Some("reset-vault") => {
                    let mut yes = false;
                    for extra in iter.by_ref() {
                        match extra.as_str() {
                            "--yes" => yes = true,
                            "-h" | "--help" => show_help = true,
                            other => bail!("unexpected argument for auth reset-vault: {other}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Auth(AuthCommand::ResetVault { yes }));
                }
                Some(other) => {
                    bail!(
                        "unknown auth subcommand: {other}. Expected: list, login, logout, reset-vault"
                    );
                }
            },
            "config" if subcommand.is_none() => match iter.next().as_deref() {
                Some("-h" | "--help") => show_help = true,
                Some("export") => match iter.next().as_deref() {
                    None => subcommand = Some(CliSubcommand::Config(ConfigCommand::Export)),
                    Some("-h" | "--help") => show_help = true,
                    Some(_) => bail!("usage: meerai config export"),
                },
                Some("import") => match iter.next().as_deref() {
                    Some("-h" | "--help") => show_help = true,
                    Some(path) if !path.starts_with('-') && iter.next().is_none() => {
                        subcommand =
                            Some(CliSubcommand::Config(ConfigCommand::Import(path.into())));
                    }
                    _ => bail!("usage: meerai config import <PATH>"),
                },
                _ => bail!("usage: meerai config export | meerai config import <PATH>"),
            },
            "memory" if subcommand.is_none() => {
                match iter.next().as_deref() {
                    Some("-h" | "--help") | None => {
                        if iter.next().is_some() {
                            bail!("usage: meerai memory [list|save|delete|pin] ...");
                        }
                        subcommand = Some(CliSubcommand::Memory(MemoryCommand::List {
                            project: None,
                            query: None,
                        }));
                    }
                    Some("list") | Some("ls") => {
                        let mut project = None;
                        let mut query = None;
                        while let Some(arg) = iter.next() {
                            match arg.as_str() {
                                "--project" => {
                                    project = Some(iter.next().ok_or_else(|| {
                                        anyhow::anyhow!("--project needs a path")
                                    })?);
                                }
                                "-h" | "--help" => show_help = true,
                                other if other.starts_with('-') => {
                                    bail!("unknown option for memory list: {other}")
                                }
                                other => {
                                    query = Some(query.map_or_else(
                                        || other.to_string(),
                                        |q: String| format!("{q} {other}"),
                                    ));
                                }
                            }
                        }
                        subcommand = Some(CliSubcommand::Memory(MemoryCommand::List {
                            project,
                            query,
                        }));
                    }
                    Some("save") => {
                        let mut project = None;
                        let mut pinned = false;
                        let mut positional = Vec::new();
                        while let Some(arg) = iter.next() {
                            match arg.as_str() {
                                "--project" => {
                                    project = Some(iter.next().ok_or_else(|| {
                                        anyhow::anyhow!("--project needs a path")
                                    })?);
                                }
                                "--pin" => pinned = true,
                                "-h" | "--help" => show_help = true,
                                other if other.starts_with('-') => {
                                    bail!("unknown option for memory save: {other}")
                                }
                                other => positional.push(other.to_string()),
                            }
                        }
                        let [kind, title, rest @ ..] = positional.as_slice() else {
                            bail!(
                                "usage: meerai memory save <decision|progress|fact|convention> <title> [body]"
                            );
                        };
                        subcommand = Some(CliSubcommand::Memory(MemoryCommand::Save {
                            project,
                            kind: kind.clone(),
                            title: title.clone(),
                            body: (!rest.is_empty()).then(|| rest.join(" ")),
                            pinned,
                        }));
                    }
                    Some("delete") | Some("rm") => {
                        let mut project = None;
                        let mut id = None;
                        while let Some(arg) = iter.next() {
                            match arg.as_str() {
                                "--project" => {
                                    project = Some(iter.next().ok_or_else(|| {
                                        anyhow::anyhow!("--project needs a path")
                                    })?);
                                }
                                "-h" | "--help" => show_help = true,
                                other if other.starts_with('-') => {
                                    bail!("unknown option for memory delete: {other}")
                                }
                                other => id = Some(other.to_string()),
                            }
                        }
                        let id =
                            id.ok_or_else(|| anyhow::anyhow!("usage: meerai memory delete <id>"))?;
                        subcommand =
                            Some(CliSubcommand::Memory(MemoryCommand::Delete { project, id }));
                    }
                    Some(verb @ ("pin" | "unpin")) => {
                        let mut pinned = verb == "pin";
                        let mut project = None;
                        let mut id = None;
                        while let Some(arg) = iter.next() {
                            match arg.as_str() {
                                "--project" => {
                                    project = Some(iter.next().ok_or_else(|| {
                                        anyhow::anyhow!("--project needs a path")
                                    })?);
                                }
                                "--off" => pinned = false,
                                "-h" | "--help" => show_help = true,
                                other if other.starts_with('-') => {
                                    bail!("unknown option for memory pin: {other}")
                                }
                                other => id = Some(other.to_string()),
                            }
                        }
                        let id = id.ok_or_else(|| {
                            anyhow::anyhow!("usage: meerai memory pin <id> [--off]")
                        })?;
                        subcommand = Some(CliSubcommand::Memory(MemoryCommand::Pin {
                            project,
                            id,
                            pinned,
                        }));
                    }
                    Some(other) => bail!("unknown memory subcommand: {other}"),
                }
            }
            "routes" if subcommand.is_none() => match iter.next().as_deref() {
                Some("-h" | "--help") | None => {
                    subcommand = Some(CliSubcommand::Routes(RoutesCommand::List));
                }
                Some("set") => {
                    let task = iter.next().ok_or_else(|| anyhow::anyhow!("usage: meerai routes set <task> <provider::model> [max_cost] [max_tokens] [max_concurrency] [--effort <level>]"))?;
                    let spec = iter.next().ok_or_else(|| anyhow::anyhow!("usage: meerai routes set <task> <provider::model> [max_cost] [max_tokens] [max_concurrency] [--effort <level>]"))?;
                    let mut effort = None;
                    let mut max_cost_usd = None;
                    let mut max_tokens_per_turn = None;
                    let mut max_concurrency = None;
                    while let Some(token) = iter.next() {
                        match token.as_str() {
                            "--effort" => {
                                effort = Some(iter.next().ok_or_else(|| {
                                    anyhow::anyhow!("--effort requires low, medium, or high")
                                })?);
                            }
                            other if max_cost_usd.is_none() => {
                                max_cost_usd =
                                    Some(other.parse::<f64>().map_err(|_| {
                                        anyhow::anyhow!("max_cost must be a number")
                                    })?);
                            }
                            other if max_tokens_per_turn.is_none() => {
                                max_tokens_per_turn =
                                    Some(other.parse::<u64>().map_err(|_| {
                                        anyhow::anyhow!("max_tokens must be a number")
                                    })?);
                            }
                            other if max_concurrency.is_none() => {
                                max_concurrency = Some(other.parse::<usize>().map_err(|_| {
                                    anyhow::anyhow!("max_concurrency must be a number")
                                })?);
                            }
                            other => bail!("unexpected argument for routes set: {other}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Routes(RoutesCommand::Set {
                        task,
                        spec,
                        effort,
                        max_cost_usd,
                        max_tokens_per_turn,
                        max_concurrency,
                    }));
                }
                Some("clear") | Some("remove") => {
                    let task = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai routes clear <task>"))?;
                    subcommand = Some(CliSubcommand::Routes(RoutesCommand::Clear { task }));
                }
                Some(other) => bail!("unknown routes subcommand: {other}"),
            },
            "pack" if subcommand.is_none() => match iter.next().as_deref() {
                Some("-h" | "--help") | None => {
                    subcommand = Some(CliSubcommand::Pack(PackCommand::List));
                }
                Some("add" | "install") => {
                    let source = iter.next().ok_or_else(|| {
                        anyhow::anyhow!("usage: meerai pack add <path|git-url> [--rev <rev>]")
                    })?;
                    let rev = match iter.next().as_deref() {
                        None => None,
                        Some("--rev") => Some(iter.next().ok_or_else(|| {
                            anyhow::anyhow!("usage: meerai pack add <path|git-url> [--rev <rev>]")
                        })?),
                        Some(other) => bail!("unknown option for pack add: {other}"),
                    };
                    subcommand = Some(CliSubcommand::Pack(PackCommand::Add { source, rev }));
                }
                Some(verb @ ("enable" | "disable")) => {
                    let enabled = verb == "enable";
                    let id = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai pack enable|disable <id>"))?;
                    subcommand = Some(CliSubcommand::Pack(PackCommand::SetEnabled { id, enabled }));
                }
                Some(verb @ ("trust" | "untrust")) => {
                    let trusted = verb == "trust";
                    let id = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai pack trust|untrust <id>"))?;
                    subcommand = Some(CliSubcommand::Pack(PackCommand::SetTrusted { id, trusted }));
                }
                Some("pin") => {
                    let id = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai pack pin <id> <rev>"))?;
                    let rev = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai pack pin <id> <rev>"))?;
                    subcommand = Some(CliSubcommand::Pack(PackCommand::Pin { id, rev }));
                }
                Some("remove") | Some("rm") => {
                    let id = iter
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("usage: meerai pack remove <id>"))?;
                    subcommand = Some(CliSubcommand::Pack(PackCommand::Remove { id }));
                }
                Some(other) => bail!("unknown pack subcommand: {other}"),
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
                Some("fork") => {
                    let mut id = None;
                    let mut turn = None;
                    let mut help = false;
                    while let Some(arg) = iter.next() {
                        match arg.as_str() {
                            "-h" | "--help" => help = true,
                            "--turn" => {
                                let value = iter
                                    .next()
                                    .ok_or_else(|| anyhow::anyhow!("missing value for --turn"))?;
                                turn = Some(value);
                            }
                            flag if flag.starts_with("--turn=") => {
                                turn = Some(flag["--turn=".len()..].to_string());
                            }
                            flag if flag.starts_with('-') => {
                                bail!("unknown option for sessions fork: {flag}")
                            }
                            _ => {
                                if id.is_some() {
                                    bail!("unexpected argument for sessions fork: {arg}");
                                }
                                id = Some(arg);
                            }
                        }
                    }
                    if help {
                        show_help = true;
                        continue;
                    }
                    let id = id.ok_or_else(|| {
                        anyhow::anyhow!(
                            "sessions fork needs a session id\n\
                             usage: meerai sessions fork <ID> [--turn <TURN_ID>]"
                        )
                    })?;
                    subcommand = Some(CliSubcommand::Sessions(SessionsCommand::Fork { id, turn }));
                }
                Some("export") => {
                    let mut id = None;
                    let mut help = false;
                    for arg in iter.by_ref() {
                        match arg.as_str() {
                            "-h" | "--help" => help = true,
                            "--jsonl" => {}
                            flag if flag.starts_with('-') => {
                                bail!("unknown option for sessions export: {flag}")
                            }
                            other => id = Some(other.to_string()),
                        }
                    }
                    if help {
                        show_help = true;
                        continue;
                    }
                    let id = id.ok_or_else(|| {
                        anyhow::anyhow!("usage: meerai sessions export <ID> [--jsonl]")
                    })?;
                    subcommand = Some(CliSubcommand::Sessions(SessionsCommand::Export { id }));
                }
                Some(other) => {
                    bail!(
                        "unknown sessions subcommand: {other}. Expected: list, rm, prune, fork, export"
                    );
                }
            },
            "hooks" if subcommand.is_none() => match iter.next().as_deref() {
                None | Some("status") => {
                    if let Some(extra) = iter.next() {
                        match extra.as_str() {
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for hooks status: {extra}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Hooks(HooksCommand::Status));
                }
                Some("-h" | "--help") => {
                    show_help = true;
                }
                Some("trust") => {
                    if let Some(extra) = iter.next() {
                        match extra.as_str() {
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for hooks trust: {extra}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Hooks(HooksCommand::Trust));
                }
                Some("untrust") => {
                    if let Some(extra) = iter.next() {
                        match extra.as_str() {
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for hooks untrust: {extra}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Hooks(HooksCommand::Untrust));
                }
                Some("verify") => {
                    let mut command = None;
                    let mut reset_baseline = false;
                    while let Some(arg) = iter.next() {
                        match arg.as_str() {
                            "--command" => {
                                let value = iter.next().ok_or_else(|| {
                                    anyhow::anyhow!("missing value for --command")
                                })?;
                                command = Some(value);
                            }
                            "--reset-baseline" => reset_baseline = true,
                            "-h" | "--help" => show_help = true,
                            _ => bail!("unexpected argument for hooks verify: {arg}"),
                        }
                    }
                    subcommand = Some(CliSubcommand::Hooks(HooksCommand::Verify {
                        command,
                        reset_baseline,
                    }));
                }
                Some(other) => {
                    bail!(
                        "unknown hooks subcommand: {other}. Expected: trust, untrust, status, verify"
                    );
                }
            },
            "serve" if subcommand.is_none() => {
                subcommand = Some(CliSubcommand::Serve(ServeCommand {
                    port: None,
                    username: None,
                    password: None,
                    auth: false,
                }));
            }
            "--port" => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--port can only be used with `serve`");
                };
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --port"))?;
                cmd.port = Some(parse_port(&value)?);
            }
            _ if arg.starts_with("--port=") => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--port can only be used with `serve`");
                };
                cmd.port = Some(parse_port(&arg["--port=".len()..])?);
            }
            "--user" | "--username" => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("{arg} can only be used with `serve`");
                };
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
                cmd.username = Some(value);
            }
            _ if arg.starts_with("--user=") => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--user can only be used with `serve`");
                };
                cmd.username = Some(arg["--user=".len()..].to_string());
            }
            _ if arg.starts_with("--username=") => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--username can only be used with `serve`");
                };
                cmd.username = Some(arg["--username=".len()..].to_string());
            }
            "--password" | "--secret" => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("{arg} can only be used with `serve`");
                };
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
                cmd.password = Some(value);
            }
            _ if arg.starts_with("--password=") => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--password can only be used with `serve`");
                };
                cmd.password = Some(arg["--password=".len()..].to_string());
            }
            _ if arg.starts_with("--secret=") => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--secret can only be used with `serve`");
                };
                cmd.password = Some(arg["--secret=".len()..].to_string());
            }
            "--auth" => {
                let Some(CliSubcommand::Serve(cmd)) = subcommand.as_mut() else {
                    bail!("--auth can only be used with `serve`");
                };
                cmd.auth = true;
            }
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
            "--acp" => {
                acp = true;
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
            "--fork" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --fork"))?;
                fork = Some(value);
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
            _ if arg.starts_with("--fork=") => {
                fork = Some(arg["--fork=".len()..].to_string());
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
    let session_selectors =
        usize::from(continue_session) + usize::from(resume.is_some()) + usize::from(fork.is_some());
    if session_selectors > 1 {
        bail!("--continue, --resume, and --fork cannot be combined");
    }
    if prompt.is_some() && subcommand.is_some() {
        bail!("--prompt/--print cannot be combined with a subcommand");
    }
    if agents.len() > 1 {
        bail!("--agent can be given only once");
    }
    if acp && (!agents.is_empty() || !skills.is_empty()) {
        bail!(
            "--acp cannot be combined with --agent or --skill; the external agent controls its own instructions"
        );
    }
    if acp && mode.is_some() {
        eprintln!(
            "Warning: --mode is ignored with --acp; the external agent controls its own modes"
        );
    }
    #[cfg(not(feature = "tui"))]
    if acp && prompt.is_none() && !matches!(subcommand, Some(CliSubcommand::Serve(_))) {
        bail!("ACP without --prompt requires a TUI build");
    }

    Ok(CliArgs {
        subcommand,
        provider,
        model,
        system,
        prompt,
        json,
        acp,
        mode,
        env_file,
        continue_session,
        resume,
        fork,
        agents,
        skills,
        show_help,
        version,
    })
}

#[cfg(test)]
mod help_tests {
    use super::*;

    #[test]
    fn help_text_is_generated_from_the_spec() {
        let help = help_text();
        for row in COMMANDS {
            assert!(help.contains(row.usage), "missing command {}", row.usage);
        }
        for row in TUI_COMMANDS {
            assert!(
                help.contains(row.usage),
                "missing TUI command {}",
                row.usage
            );
        }
        for row in OPTIONS {
            assert!(help.contains(row.usage), "missing option {}", row.usage);
        }
    }

    #[test]
    fn spec_usages_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for row in COMMANDS.iter().chain(TUI_COMMANDS).chain(OPTIONS) {
            assert!(seen.insert(row.usage), "duplicate usage {}", row.usage);
        }
    }
}
