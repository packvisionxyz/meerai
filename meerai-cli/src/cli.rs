use anyhow::{Result, bail};

pub(crate) const HELP_TEXT: &str = "Usage:\n  \
    meerai [-p|--provider <PROVIDER>] [-m|--model <MODEL>] [--system <INSTRUCTIONS>]\n          \
    [--mode <plan|ask|auto-edit|yolo>] [--continue | --resume <ID> | --fork <ID>] [--env-file <PATH>]\n          \
    [--agent <ID>] [--skill <ID>]... [-h|--help] [-V|--version]\n  \
    meerai --prompt <TEXT> [--json] [--continue | --resume <ID> | --fork <ID>] [--mode <MODE>]\n          \
    [--agent <ID>] [--skill <ID>]...\n  \
    meerai auth <list|login|logout>\n  \
    meerai sessions <list|rm|prune|fork>\n\
    \n\
    Commands:\n  \
    auth list                       Show credentials across auth.json, config, and env\n  \
    auth login <provider> [key]     Save an API key in ~/.local/share/meerai/auth.json\n  \
    auth logout <provider>          Remove a stored API key from auth.json\n  \
    sessions list                   List saved sessions: id, updated ms, bytes, title/model\n  \
    sessions rm <ID>                Delete one session\n  \
    sessions rm --all               Delete every saved session\n  \
    sessions prune [--days N] [--keep N]\n                                              \
    Delete sessions older than N days (default 30), keeping the newest N (default 0)\n  \
    sessions fork <ID> [--turn <TURN_ID>]\n                                              \
    Fork a session at a closed turn (default: latest closed turn)\n\
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
        --fork <ID>            Fork a session from its latest closed turn and open it\n  \
        --json                 Print one NDJSON AgentEvent per line (with --prompt)\n  \
        --mode <MODE>          Approval mode: plan, ask, auto-edit, yolo\n  \
        --agent <ID>           Apply a markdown agent by id\n  \
        --skill <ID>           Apply a markdown skill by id (repeatable)\n  \
        --env-file <PATH>         Load variables from PATH (no implicit .env loading)\n  \
    -h, --help                 Show this help message\n  \
    -V, --version              Print version and exit";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AuthCommand {
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
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliSubcommand {
    Auth(AuthCommand),
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
                Some(other) => {
                    bail!("unknown sessions subcommand: {other}. Expected: list, rm, prune, fork");
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
        fork,
        agents,
        skills,
        show_help,
        version,
    })
}
