# meerai

Pack Vision's terminal coding-agent harness — written in Rust, designed for
autonomous and interactive software engineering in the terminal with a
selection-first, modal TUI and a scriptable headless CLI.

**Status:** early-stage, distribution-hardened. The workspace has nine library
crates plus the `meerai` binary (built by `meerai-cli`). Milestones M1–M5 of the
[harness specification](docs/harness-spec.md) are landed: session log, `AgentEvent`
v2 item protocol, coding tools, policy engine, context engine, and prompt
composition. Project-root config discovery, TUI session replay, configurable
session retention, tool-output redaction, and agents and skills discovery
(**M12**) have also landed. CI, tag-driven release builds, installers, and an
Apache-2.0 license are in place. Checkpoints, hooks, the subagent runtime, and
an OS sandbox do not exist yet; the security backlog is tracked as **M11** in
the spec (see [Security roadmap](#security-roadmap)).

## Install

### Release binary

macOS and Linux:

```sh
curl -LsSf https://meerai.packvision.xyz/install.sh | sh
```

Windows (PowerShell, x64):

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://meerai.packvision.xyz/install.ps1 | iex"
```

The installers download `meerai-<target>` plus its `.sha256` from GitHub
Releases, verify the checksum, and install into a user bin directory
(`MEERAI_INSTALL_DIR`, else `XDG_BIN_HOME`, else `~/.local/bin`). They add that
directory to your `PATH` unless `MEERAI_NO_MODIFY_PATH` is set. `-v <TAG>` picks
a version.

### From source

```sh
cargo install --path meerai-cli --bin meerai
```

Or build without installing:

```sh
cargo build --release -p meerai-cli    # target/release/meerai
```

Rust `1.100.0` (pinned in [rust-toolchain.toml](rust-toolchain.toml)) is
required; `rustup` installs it automatically.

### Platform support

| Platform | Status | Notes |
| --- | --- | --- |
| macOS arm64 / x64 | Release target | Unsigned and not notarized; Gatekeeper may warn |
| Linux x86_64 / aarch64 (GNU) | Release target | No static musl build |
| Windows x64 (MSVC) | Release target | The shell tool uses `cmd /C`; unsigned; x64 only |

The release workflow builds exactly these five targets. Windows on Arm runs the
x64 build under emulation; other architectures have no prebuilt binary.

## Quick start

```sh
meerai auth login openrouter          # or export OPENROUTER_API_KEY
meerai                                # full-screen modal TUI
meerai --prompt "explain this repository" --mode plan
meerai --prompt "fix the failing test" --mode auto-edit --json
meerai --agent reviewer --prompt "review this change"
meerai --skill orchestrate
```

Headless runs print the assistant text to stdout. `--json` prints one NDJSON
`AgentEvent` per line and requires `--prompt`. The default approval mode is
`ask`.

## What It Provides

### Available today

- **Agent harness (`meerai-agents`):** `MultiTurnAgent` turn/step loop plus
  `AgentHarness`; the `AgentEvent` v2 item protocol (`TurnStarted`,
  `ItemStarted`, `ItemDelta`, `ItemCompleted`, `ApprovalRequested`,
  `ApprovalResolved`, `UsageUpdated`, `TurnCompleted`, `Error`); session
  dual-write, `derive_messages`-driven requests, approval suspend/resume, spill,
  compaction, token and wall-clock budgets, circuit breakers, cancellation,
  steering, provider retries, consecutive read-only tool batches, first-prompt
  session titles, summarizer-model selection, and best-effort tool-output
  redaction before spill, logging, display, and model delivery.
- **Prompt composition (`meerai-agents::prompt`):** `BASE_SYSTEM_PROMPT` plus
  `compose_system_prompt`, which appends `--system` text, the workspace
  `AGENTS.md`, and `.meerai/instructions.md` under byte caps. Scoped `AGENTS.md`
  files are injected once per turn when a tool touches their directory.
- **Agents and skills (`meerai-common::definitions`):** read-only discovery of
  markdown agents and skills from `.agents/` (default) then `.claude/`, applied
  as prompt instructions through `--agent`/`--skill` or the TUI. See
  [Agents and skills](#agents-and-skills).
- **Session log (`meerai-session`):** append-only `SessionEvent` log in JSONL
  generations sealed to zstd, `meta.json`, crash recovery, the pure
  `derive_messages` projection, turn-boundary `fork`, shadowing `revert`, a blob
  store, and spill plus compaction primitives.
- **Policy engine (`meerai-policy`):** ordered `action/resource/effect` rules
  (globset, last match wins), `ApprovalMode` presets (`plan`, `ask`, `auto-edit`,
  `yolo`), turn and session grants, and YAML rule parsing.
- **Coding tools (`meerai-tools`):** the `code` module's `CodeTools` with the seven `code-*`
  tools (`read_file`, `write_file`, `edit_file`, `list_dir`, `grep`, `glob`,
  `shell`), workspace path guards, unified diffs, `ToolObservation`/`ToolRun`
  results, per-tool `EffectClass`, and provider-secret environment scrubbing
  from child processes.
- **Modal TUI (`meerai-tui`) and CLI/headless runner (`meerai-cli`):** modal
  transcript (including a session replayed by `--continue`/`--resume`), provider
  and model pickers, sidebar with file changes, diff and search views, approval
  overlay, `/compact`, `/mode`, `/agent`/`/skill`, and a `@` Mentions popup for
  agents, skills, and files; one-shot headless turns with a token summary on
  stderr, NDJSON event stream, `sessions list|rm|prune` management, and opt-in
  startup retention (`sessions.auto_prune_days`).
- **Config (`meerai-common`) and providers (`meerai-providers`):** a `genai`
  0.7 client factory over a provider catalog (bundled `providers.yml` plus user
  overrides), `auth.json` storage, `settings.yml` plus a project
  `.meerai/settings.yml` found by walking up to the repository root and merged
  with tighten-only permissions, and `${VAR|default}` env expansion.
- **Tool primitives (`meerai-core`):** `Toolset`, `ToolCall`, `ToolDefinition`,
  `ToolOutput`, `ToolError`, `EffectClass`, `ToolObservation`, `ToolRun`, and
  `FileDiff`.
- **Tool macros (`meerai-macros`):** `#[tool]` and `#[derive(Toolset)]`.

### Not implemented yet

- OS sandbox (macOS Seatbelt / Linux bubblewrap) — shell runs with your
  privileges after approval.
- Checkpoints and rewind (M6), hooks (M7), and the subagent runtime (M8):
  child sessions, isolation, and the `subagent` tool. M12 loads markdown agent
  and skill definitions, but applies them as instructions only.
- Tool or permission mapping from agent definitions, MCP servers, and skill
  supporting files. M12 reads only `SKILL.md`.
- Projection caching and frozen messages; per-model context windows.
- USD cost and pricing: the harness reports token counts only, and
  `est_cost_usd` is always `None`. Pricing-data collection is the blocker.
- Progressive tool disclosure, LSP tools, repository maps, and an OS keychain.
- ACP integration (M13 client, M14 agent). Design in [docs/acp.md](docs/acp.md).
  An ACP backend runs an external agent with its own tools; meerai policy does
  not apply to it.
- Dependency audit in CI, signed/notarized release artifacts, and a written
  threat model (M11).

## Crates

### Current workspace

| Crate | Role |
| --- | --- |
| `meerai-core` | `Toolset` trait (`effect_of`, `invoke_structured`), `ToolOutput`, `ToolError`, `EffectClass`, `ToolObservation`, `ToolRun`, `FileDiff`; re-exports `genai`, `async_trait`, and `schemars::JsonSchema` |
| `meerai-macros` | Proc-macros `#[tool]` and `#[derive(Toolset)]` |
| `meerai-common` | `AppSettings` (`settings.yml`), project-root discovery plus project settings load/merge (`find_project_root`, `load_project_settings`, `merge_project_settings`), agent and skill definition discovery (`definitions`) |
| `meerai-providers` | Provider catalog plus `genai` client factory (`providers.yml`), `auth.json` secret store |
| `meerai-session` | `Session` append-only event log: JSONL/zstd generations, `derive_messages`, fork/revert, blob + spill, compaction primitives, listing, deletion, and pruning support |
| `meerai-policy` | `PolicyEngine`: ordered permission rules, approval modes, turn/session grants |
| `meerai-tools` | `code` module: `CodeTools`, the seven `code-*` tools with path guards, unified diffs, and observations |
| `meerai-agents` | `MultiTurnAgent`, `AgentHarness`, `AgentEvent` v2, `prompt::{BASE_SYSTEM_PROMPT, compose_system_prompt}`, `spawn_worker`, and best-effort tool-output redaction |
| `meerai-tui` | Modal terminal UI library (`ratatui` + `crossterm`) with transcript replay and approval overlay |
| `meerai-cli` | Binary crate (`meerai`): CLI flags, headless runner, `auth` and `sessions` subcommands, TUI launcher |

## Usage

```sh
meerai                                          # full-screen modal TUI
meerai --continue                               # TUI with the last session replayed
meerai --resume <ID>                            # TUI with a session replayed
meerai --prompt "explain this repository"       # one headless turn
meerai --prompt "..." --json                    # one NDJSON AgentEvent per line
meerai --prompt "..." --continue                # resume the last session headless
meerai --prompt "..." --resume <ID>             # resume a session by id headless
meerai --mode plan|ask|auto-edit|yolo           # approval mode (default: ask)
meerai -p <provider> -m <model> --system "..."  # provider, model, system prompt
meerai --agent reviewer --prompt "review this change"  # apply an agent
meerai --skill orchestrate                      # apply a skill (repeatable)
meerai --env-file .env.local                    # load variables from a file
meerai auth list|login|logout                   # inspect, save, remove API keys
meerai sessions list|rm|prune                   # list, delete, or prune sessions
meerai sessions prune --days 30 --keep 5        # delete old sessions, keep 5 newest
meerai --version                                # print version (-V)
```

`-p` is the provider flag; use `--prompt` (or `--print`) for headless input.
`--json` requires `--prompt`. `--continue` and `--resume <ID>` select an
existing session for either mode: with `--prompt` they run headless, without it
they start the TUI with the stored transcript replayed. The two flags cannot be
combined. Without `--prompt`, the TUI starts (unless a subcommand or `--version`
is given), and a non-terminal stdin/stdout is an error.

### Token usage

Headless runs without `--json` print one cumulative summary line to stderr after
the turn:

```text
tokens: prompt=1234 completion=567 total=1801
```

Missing counters are omitted. `--json` reports the same data as `UsageUpdated`
events. The TUI statusline shows the current turn (`↑… ↓… Σ…`) and running
session totals (`session ↑… ↓… Σ…`). The counters are cumulative for the
session: a resumed run seeds from the last logged `UsageUpdated` entry. USD cost
is not tracked; `est_cost_usd` is always `None`.

### Approval modes

| Mode | Behavior |
| --- | --- |
| `plan` | Hard read-only. Reads allowed; writes, shell, and external effects are denied even when a rule would allow them. |
| `ask` | Reads allowed; writes, shell, and external effects ask. Default. |
| `auto-edit` | Reads and workspace writes allowed; shell and external effects ask. |
| `yolo` | Allow everything except explicit `deny` rules. The statusline shows a loud `YOLO` badge. |

Grants skip later prompts for the same action and exact resource. In the
approval overlay, `y`/`Enter` approves once, `t` approves and records a turn
grant, `a` approves and records a session grant, `n` denies, `r` denies with a
reason, and `Esc` cancels. Turn grants are cleared at turn end. Grant resources
are literal, not glob patterns; permission rules still use globs.

### Agents and skills

meerai reads markdown agent and skill definitions from the project root, which is
the same root used for project config. It loads them at startup and applies them
as prompt text.

| Kind | Paths, in priority order | Id |
| --- | --- | --- |
| Agent | `.agents/agents/*.md`, then `.claude/agents/*.md` | frontmatter `name`, else the file name |
| Skill | `.agents/skills/*/SKILL.md`, then `.claude/skills/*/SKILL.md` | frontmatter `name`, else the directory name |

- `.agents` has higher priority than `.claude`. If both define the same id,
  `.agents` wins and a warning names both paths. The output is sorted by id.
- `.meerai` holds configuration, sessions, and instructions only. meerai does
  not scan or create `.meerai/agents` or `.meerai/skills`.
- Discovery is read-only. meerai copies nothing, writes no import lock, and
  never edits the definition files. Symlinks are skipped with a warning.
- Frontmatter fields: `name`, `description`, `model`, `steps`, and `mode`.
  `tools`, `permissions`, and `allowed-tools` print a warning and are not
  applied. Invalid frontmatter prints a warning and skips the file. Without
  frontmatter, the id comes from the file or directory name and the body is the
  prompt.
- A skill uses only the text of its `SKILL.md`. Supporting files in the skill
  directory are not read.
- Definitions are repository-controlled prompt text. They cannot run tools,
  grant permissions, or imply a sandbox. Treat them as untrusted content.

Startup flags apply definitions to the system prompt:

```sh
meerai --agent reviewer --prompt "review this change"
meerai --skill orchestrate
```

- `--agent <ID>` selects one agent. Repeating the flag is an error. The id must
  exist.
- `--skill <ID>` selects a skill and is repeatable. Skill text is appended
  before the agent text.
- An unknown id fails with the list of available ids.
- Model precedence is `--model` -> `MEERAI_MODEL` -> the agent `model` -> the
  configured model, then the provider default. An agent model that contains `:`
  is ignored with a warning because it would switch provider. `steps` and
  `mode` print a note and are not applied.

TUI commands and mentions:

| Surface | Action |
| --- | --- |
| `/agent <id>` | Apply an agent by id |
| `/skill <id>` | Apply a skill by id |
| `@` | Open the Mentions popup: agents, then skills, then files |
| `@agent/<id>` | Apply an agent, then send the rest of the prompt |
| `@skill/<id>` | Apply a skill, then send the rest of the prompt |

Typing `@` opens one **Mentions** popup. Agents are listed first, then skills,
then files. Agent rows show `<id>|agent` with a cyan badge, skill rows show
`<id>|skill` with a magenta badge, and both show the source and description after
the badge. File rows show the path and no badge. The row shows the bare `<id>`,
not the `@agent/` or `@skill/` prefix. The same id can appear once as an agent and
once as a skill.

After you accept a known `@agent/<id>` or `@skill/<id>` mention, the composer
shows only `<id>`: a bold cyan agent chip or a bold magenta skill chip. The raw
`@agent/<id>` or `@skill/<id>` text stays in the input buffer, and the prefix is
hidden only in the display. Unknown ids and partial tokens such as
`@skill/doesnotexist` or `@skill/del` render exactly as typed. Several mentions
on one line render as separate chips, and the surrounding text stays in place.

Filtering matches `@agent/<id>`, `@skill/<id>`, the bare `<id>`, and fuzzy id
prefixes. Up/Down or Ctrl-p/Ctrl-n move the selection; Tab accepts; Enter accepts
the highlighted row without submitting; Esc dismisses. Accepting inserts
`@agent/<id> `, `@skill/<id> `, or `@path ` with one trailing space and stays in
Insert mode, so a second Enter sends the prompt. An exact agent or skill id keeps
the popup open so you can pick; an exact file path dismisses it as before.

On submit, skills apply before the agent, the mention is removed from the prompt,
and repeats apply once. An unknown mention stays as text. A prompt that is only
mentions applies the definitions without starting a turn.

`/agent` and `/skill` remain explicit shortcuts and send
`AgentCommand::ApplyAgent` or `AgentCommand::ApplySkill`, which appends a System
instruction at the next step boundary. An unknown or missing id prints a System
error and sends nothing. A colon-free agent `model` is applied within the active
provider. `/agents` and `/skills` do not exist.

### Headless exit codes

| Code | Meaning |
| --- | --- |
| `0` | The turn ended `Completed`. |
| `1` | Failure: turn `Failed`/`Aborted`, `Error` event, task error, or a declined Spend approval. |
| `2` | A tool-call approval was required. The run fails fast, prints a hint to use `--mode auto-edit`, `--mode yolo`, or the TUI, and cancels the turn. |

A **Spend** approval (session token cap reached) is not a tool approval. In
headless mode it prints how to raise or unset `context.max_session_tokens`,
declines the request, and exits `1` because the turn ends `Failed { Budget }`.
Do not treat every approval as exit `2`.

Library callers without a command channel auto-decline requests; the CLI
headless path observes the request first and exits as described above.

### Sessions

```sh
meerai sessions list          # id, updated ms, bytes, title or model
meerai sessions rm <ID>       # delete one session
meerai sessions rm --all      # delete every session
meerai sessions prune         # delete sessions older than 30 days, keep none
meerai sessions prune --days 7 --keep 5
meerai sessions prune --days=7 --keep=5
```

`prune` deletes sessions whose `updated_at_ms` is older than the cutoff
(`--days N`, default 30), except the newest `--keep N` sessions (default 0).
Those new sessions stay even when they are older than the cutoff. The command
prints `removed N sessions`. The cutoff uses the global session root, which
`MEERAI_SESSION_DIR` can override. Automatic pruning at startup is opt-in; see
[Session retention](#session-retention).

`--continue` opens the most recently active session (`updated_at_ms` moves on
every `TurnEnded`). With `--prompt` the run is headless; without it the TUI
starts and replays the stored transcript. Replay covers user and steering
messages, assistant text and reasoning, tool calls and results, and compaction
markers. An unmatched tool call shows as cancelled. System and runtime events
are skipped. Session metadata records the workspace `cwd`, git branch and SHA,
title, model, and created/updated timestamps. On Unix, session directories are
`0700` and files are `0600`.
The agent sets the session title from the first user prompt: first non-empty
line, at most 80 characters. A session that already has a
user message is not retitled by the agent.

## Config and data locations

- `$XDG_CONFIG_HOME/meerai` (default `~/.config/meerai`): `settings.yml`
  (`active_provider`, `active_model`, `providers`, `approval_mode`,
  `permissions`, `context`, `sessions`) and `providers.yml` (user provider
  overrides).
- `<project-root>/.meerai/settings.yml`: optional project settings, merged over
  the user `settings.yml`. The project root is found by walking up from the
  process CWD. See [Project config](#project-config).
- `<project-root>/.agents/` (default) and `<project-root>/.claude/`: optional
  markdown agent and skill definitions. See [Agents and skills](#agents-and-skills).
- `$XDG_DATA_HOME/meerai` (default `~/.local/share/meerai`): `auth.json` written
  by `meerai auth login` (mode `0600`); legacy `auth.yml` is still read.
- Sessions: `<dirs::data_dir()>/meerai/sessions/<session-id>/` (on macOS
  `~/Library/Application Support/meerai/sessions`; on Linux
  `$XDG_DATA_HOME/meerai/sessions`), or `$MEERAI_SESSION_DIR/<session-id>/`. A
  directory holds `session.v<N>.jsonl` (active), `session.v<N>.jsonl.zstd`
  (sealed), `meta.json`, and `blobs/`. `SessionConfig { project_local: true }`
  switches to `<workspace>/.meerai/sessions` and writes a `.gitignore`; the
  current binary uses the global root.

### Project config

The binary discovers the project root by walking up from the process working
directory. It stops at the first directory that contains `.git` (a directory or
a file, so linked worktrees count) or `.meerai/settings.yml`. If no ancestor
matches, the working directory is the root. It then reads
`<project-root>/.meerai/settings.yml` and merges it over the user
`settings.yml`. Tools and sessions still use the working directory as the
workspace, so running `meerai` from a subdirectory applies the repository-root
policy. A project file can tighten permissions, but it cannot redirect the
session or grant a new capability.

```yaml
# <project-root>/.meerai/settings.yml
permissions:
  - { action: edit,  resource: "src/generated/**", effect: deny }
  - { action: shell, resource: "cargo publish *",  effect: ask }
context:
  high_watermark_pct: 70
  summarizer_model: openrouter:small-model
```

- **`permissions`** — project rules are merged as **project `ask` rules →
  user rules → project `deny` rules**. `allow` entries are dropped with a
  warning, so a project cannot grant a new capability. A project `ask` cannot
  downgrade a user `deny`, even with a wildcard resource, because the user rules
  match later. It tightens the mode default only where the user has no matching
  rule. A project `deny` always adds a denial after the user rules. An invalid
  rule or a non-list shape drops the whole project block with a warning.
- **`context`** — overrides field by field (`context_window_tokens`,
  `high_watermark_pct`, `spill_max_inline_tokens`, `verbatim_window_turns`,
  `reasoning_pruning`, `max_steps_per_turn`, `tool_timeout_ms`,
  `max_session_tokens`, `max_turn_secs`, `summarizer_model`). The budget caps
  `max_session_tokens` and `max_turn_secs` are clamped against user values: a
  project value cannot raise a user cap, and a project cannot use `0` to disable
  the turn clock.
- **Ignored fields** — `active_provider`, `active_model`, `providers`, and
  `approval_mode` are ignored with a warning, so a repository cannot change
  provider endpoints or switch the approval mode to `yolo`. A project `sessions`
  block is also ignored with a warning: only the user config controls session
  retention.
- A malformed project file prints a warning and is ignored. Startup never
  aborts because of a bad project file.

`context.summarizer_model` is optional. It is a full `provider:model` spec used
for compaction summaries; the default is the session model. Summarizer usage
counts toward `context.max_session_tokens`.

### Session retention

Retention is manual by default: use `meerai sessions prune`. Automatic pruning is
opt-in in the **user** `settings.yml`:

```yaml
# ~/.config/meerai/settings.yml
sessions:
  auto_prune_days: 90
  keep: 10
```

- `auto_prune_days` enables pruning at startup. Sessions older than N days
  (`updated_at_ms`) are deleted.
- `keep` spares the newest N sessions (default 0). Those sessions stay even when
  they are older than the cutoff.
- The currently active session is never pruned, so `--continue` cannot delete
  the session it just opened.
- A prune prints `Pruned N sessions (older than N days)` to stderr. An error
  prints a warning; startup never aborts.
- Only the user config controls retention. A `sessions` block in a project
  `.meerai/settings.yml` is ignored with a warning.
- `keep` alone does nothing: without `auto_prune_days`, no automatic prune runs.

The `sessions prune` command keeps its own `--days` and `--keep` flags and is
unaffected by this block.

### Prompt and repository instructions

Every request starts from the built-in `BASE_SYSTEM_PROMPT`. The binary then
appends, in order, the `--system` text (if any), the workspace `AGENTS.md`, and
`.meerai/instructions.md`. These files are read per run and capped at 32 KiB
each with a 64 KiB block cap. When an authorized tool touches a subdirectory,
that directory's `AGENTS.md` is injected once for the turn, capped at 16 KiB,
with at most eight files per turn. Treat repository instructions as untrusted
content from the repository.

## Development

CI runs on every push and pull request to `main`
([.github/workflows/ci.yml](.github/workflows/ci.yml)): `cargo fmt --all --
--check`, `cargo clippy --workspace --all-targets`, then `cargo test
--workspace` on `ubuntu-24.04` with the pinned toolchain. CI has no Windows
runner: the Windows `cmd /C` shell path and Windows session-id rules are
unit-tested, but not exercised on a Windows host. Run the same order locally:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

`make verify` (alias `make ci`) runs the three commands. `.github/workflows/release.yml`
rebuilds the same gate on a `v*` tag, builds the five targets, verifies that the
tag matches `[workspace.package] version`, and publishes archives plus SHA256
checksums. `.github/workflows/pages.yml` deploys `site/` to GitHub Pages.

Macro expansion is snapshot-tested with insta: update with `cargo insta review`
or `INSTA_UPDATE=always cargo test -p meerai-macros`.

## Environment variables

Key resolution order per provider is stored `auth.json` → `api_key` in
`providers.yml`/`settings.yml` → the provider's environment variables. The
bundled [providers.yml](meerai-providers/providers.yml) declares those variables,
for example `OPENROUTER_API_KEY` (also `OPEN_ROUTER_API_KEY`) and
`GEMINI_API_KEY`.

- `MEERAI_PROVIDER`, `MEERAI_MODEL` — default provider and model; CLI flags win.
- `MEERAI_SESSION_DIR` — session root override.
- `MEERAI_LOAD_DOTENV=1|true|yes` — load `./.env` with `dotenvy`. Nothing loads
  implicitly.
- `--env-file <PATH>` — load variables from an explicit file. A missing file is
  an error.

`.env` is gitignored but not read unless one of those opt-ins is used.

## Security and limits

- **No OS sandbox.** Path guards are in-process checks, not a kernel boundary.
  `shell` runs with your user privileges after approval. `plan` mode denies
  writes and shell, but it cannot contain a command you approve in another mode.
- **Secret handling is partial.** The shell tool removes known provider key
  variables and any variable ending in `_API_KEY`, `_API_TOKEN`,
  `_ACCESS_TOKEN`, `_SECRET`, `_SECRET_KEY`, or `_PRIVATE_KEY` from child
  processes. Tool output and diffs are also redacted on a best-effort basis
  before they are spilled, logged, shown in the UI, or sent to the model.
  Tool-dispatch errors are redacted before they are logged or sent to the
  model. Patterns cover
  OpenAI/OpenRouter `sk-`, Anthropic `sk-ant-`, Google `AIza`, GitHub `ghp_` and
  `github_pat_`, AWS `AKIA`, Slack `xox*`, `Bearer` tokens, and PEM private
  keys. Matches become `[REDACTED:<kind>]`. The original text is discarded;
  there is no log-only copy. Paths are not redacted. Redaction is pattern-based,
  not a guarantee, and a secret that does not match a pattern is
  not redacted.
- **Local files are private by default.** `auth.json` is `0600`; session
  directories are `0700` and session files are `0600` on Unix. Windows has no
  equivalent mode enforcement.
- **Repository input is opt-in.** `.env` is not loaded implicitly. `AGENTS.md`
  and `.meerai/instructions.md` are injected into the prompt, so review them
  like code.
- **Agent and skill definitions are prompt text.** `.agents/agents`,
  `.agents/skills`, `.claude/agents`, and `.claude/skills` are
  repository-controlled content that reaches the model as instructions. They
  cannot run tools, map permissions, or change the sandbox. Review them like
  code.
- **Treat model and tool output as untrusted.** The base prompt says so, but
  the harness does not screen prompt injection.
- **Release binaries are unsigned.** macOS Gatekeeper and Windows SmartScreen
  may warn. Verify the published SHA256 checksum.

### Security roadmap

Security is a staged milestone (M11 in
[docs/harness-spec.md](docs/harness-spec.md)). In the tree now: path guards,
`plan` as hard read-only, approval gates, headless fail-fast, provider-secret
environment scrubbing, best-effort output/diff/error redaction, session file
permissions, configurable retention, and session/turn token budgets. The
backlog is:

1. **OS sandbox** for `shell` through an `ExecutionWorld` seam: macOS Seatbelt,
   Linux bubblewrap/Landlock, network denied by default.
2. **Dependency audit in CI** (`cargo audit` or `cargo deny`). No audit step
   exists today.
3. **Signed and notarized** macOS and Windows artifacts. No signing secret or
   workflow step exists today.
4. **Written threat model** for repository instructions, model output, hooks,
   MCP servers, and subagents.
5. **Prompt-injection screening.** The harness states the risk but does not
   screen content.
6. **OS keychain** for `auth.json`, which is a plain `0600` file today.

## Documentation

- [docs/harness-spec.md](docs/harness-spec.md) — unified harness specification: event-sourced session log, typed item protocol, context engine, permissions/sandboxing, hooks, checkpoints, subagents, and retrieval strategy
- [docs/architecture.md](docs/architecture.md) — current implementation notes, target harness architecture, protocol/loop design, and migration plan
- [docs/tui-design.md](docs/tui-design.md) — modal TUI design, keybindings, pickers, diff review, and async Rust rendering architecture
- [docs/acp.md](docs/acp.md) — planned Agent Client Protocol integration: external harnesses (M13) and `meerai acp` for editors (M14), with the trust boundary
- [AGENTS.md](AGENTS.md) — repository guide for AI coding agents (commands, macro conventions, known traps)

## License

Apache-2.0. See [LICENSE](LICENSE).
