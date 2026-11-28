use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clients {
    Tui,
    Web,
    Both,
}

impl Clients {
    pub fn has_tui(self) -> bool {
        matches!(self, Clients::Tui | Clients::Both)
    }

    pub fn has_web(self) -> bool {
        matches!(self, Clients::Web | Clients::Both)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub args: &'static str,
    pub desc: &'static str,
    pub takes_arg: bool,
    pub clients: Clients,
}

impl CommandSpec {
    pub fn label(self) -> String {
        if self.args.is_empty() {
            self.name.to_owned()
        } else {
            format!("{} {}", self.name, self.args)
        }
    }

    pub fn insert(self) -> String {
        if !self.takes_arg {
            self.name.to_owned()
        } else {
            format!("{} ", self.name)
        }
    }
}

impl fmt::Display for CommandSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.label())
    }
}

const fn both(
    name: &'static str,
    args: &'static str,
    desc: &'static str,
    takes_arg: bool,
) -> CommandSpec {
    CommandSpec {
        name,
        args,
        desc,
        takes_arg,
        clients: Clients::Both,
    }
}

const fn web(
    name: &'static str,
    args: &'static str,
    desc: &'static str,
    takes_arg: bool,
) -> CommandSpec {
    CommandSpec {
        name,
        args,
        desc,
        takes_arg,
        clients: Clients::Web,
    }
}

const fn tui(
    name: &'static str,
    args: &'static str,
    desc: &'static str,
    takes_arg: bool,
) -> CommandSpec {
    CommandSpec {
        name,
        args,
        desc,
        takes_arg,
        clients: Clients::Tui,
    }
}

pub const COMMANDS: &[CommandSpec] = &[
    both("/help", "", "Show commands and keybindings", false),
    both(
        "/sessions",
        "[id|filter]",
        "Browse or switch sessions",
        false,
    ),
    both("/new", "", "Open a new session tab", false),
    both(
        "/fork",
        "",
        "Fork the session at the latest closed turn into a new tab",
        false,
    ),
    both(
        "/btw",
        "[question]",
        "Open a read-only side session seeded from this history",
        true,
    ),
    both("/acp", "[preset]", "Start an ACP session (agy)", false),
    both("/close", "", "Close the current session tab", false),
    tui("/auth", "", "Show active credential sources", false),
    both(
        "/connect",
        "",
        "Configure provider API key and model",
        false,
    ),
    both("/provider", "", "Switch or configure AI provider", false),
    both("/models", "[name]", "Choose or set the active model", false),
    both("/model", "[name]", "Switch active LLM model", true),
    both(
        "/effort",
        "[low|medium|high|xhigh|max]",
        "Set reasoning effort, or open the sampling panel with no argument",
        true,
    ),
    both(
        "/sampling",
        "",
        "Set reasoning effort and temperature together",
        false,
    ),
    both(
        "/temperature",
        "[0.0..2.0|default]",
        "Set the sampling temperature override",
        true,
    ),
    both(
        "/agent",
        "<id>",
        "Apply an agent by id (or @agent/<id>)",
        true,
    ),
    both(
        "/agents",
        "",
        "Choose an agent from the Mentions popup",
        false,
    ),
    both(
        "/skill",
        "<id>",
        "Apply a skill by id (or @skill/<id>)",
        true,
    ),
    both(
        "/skills",
        "",
        "Show discovered skills and token estimates",
        false,
    ),
    both(
        "/loop",
        "",
        "Show the active loop checklist (or use @goal / @orchestrate <instruction>)",
        false,
    ),
    both(
        "/context",
        "",
        "Inspect the context breakdown, watermark, and headroom",
        false,
    ),
    both(
        "/memory",
        "[query]",
        "List or search project memories",
        true,
    ),
    both("/remember", "<note>", "Save a project memory", true),
    both("/forget", "<id>", "Delete a project memory", true),
    both("/pin", "<id> [off]", "Pin or unpin a project memory", true),
    both(
        "/routing",
        "[set <task> <provider::model>]",
        "List or edit task model routes",
        true,
    ),
    both("/packs", "", "List installed declarative packs", false),
    both(
        "/pack",
        "<add|enable|disable|trust|untrust|pin|remove> <id>",
        "Install, enable, trust, pin, or remove a pack",
        true,
    ),
    both(
        "/definitions",
        "",
        "Show discovered agents and skills",
        false,
    ),
    both(
        "/mcp",
        "[list|trust|untrust|reload]",
        "List MCP servers, or trust and reload this project",
        false,
    ),
    both(
        "/subagent",
        "<agent> <task>",
        "Spawn a manual subagent, or cancel one",
        true,
    ),
    both(
        "/open",
        "<path>[:line]",
        "Open a workspace file in $EDITOR",
        true,
    ),
    both(
        "/vault-reset",
        "",
        "Delete every saved credential and reinitialize the vault",
        false,
    ),
    both(
        "/search",
        "[query]",
        "Search files by name or content",
        false,
    ),
    both(
        "/grep",
        "[query]",
        "Search file contents with ripgrep",
        false,
    ),
    both("/diff", "[path]", "View a diff of changed files", false),
    both(
        "/sidebar",
        "",
        "Toggle the status and file changes sidebar",
        false,
    ),
    both(
        "/diagnose",
        "",
        "Show or hide cache, context, and memory diagnostics",
        false,
    ),
    both("/clear", "", "Clear the visible chat transcript", false),
    both(
        "/reset",
        "",
        "Clear the transcript and reset agent history",
        false,
    ),
    both("/cancel", "", "Cancel the active agent turn", false),
    both(
        "/compact",
        "",
        "Compact older turns to reclaim context",
        false,
    ),
    both(
        "/mode",
        "[plan|ask|auto]",
        "Show or set the approval mode",
        false,
    ),
    both(
        "/rewind",
        "",
        "Restore code and conversation for a turn",
        false,
    ),
    both(
        "/reload",
        "",
        "Reload settings, theme, definitions, and prompt",
        false,
    ),
    both(
        "/serve",
        "[stop]",
        "Start or stop the local browser UI server",
        false,
    ),
    both("/update", "", "Show version and update instructions", false),
    web(
        "/settings",
        "",
        "Configure theme, context, sessions, and sandbox",
        false,
    ),
    both("/quit", "[q]", "Exit or close the client", false),
];
