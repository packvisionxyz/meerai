# meerai

Pack Vision's terminal agent harness — written in Rust, designed for
autonomous and interactive software engineering in the terminal with a
selection-first, modal TUI and a scriptable headless CLI.

**Status:** early-stage, distribution-hardened. The harness, checkpoints, hooks,
subagents, retrieval, security controls, agents and skills, and the `agy` ACP
client are implemented; the [harness specification](docs/harness-spec.md)
tracks the milestones. CI, release builds, installers,
[the threat model](docs/threat-model.md), and an Apache-2.0 license are in
place. `meerai export` does not exist yet (see
[Security roadmap](#security-roadmap)).

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
make install
```

`make install` builds the Wasm bundle, then installs `meerai` with the `web`
feature compiled in, so `meerai serve` and TUI `/serve` work. Without `make`,
run the two steps yourself:

```sh
python3 scripts/build-web.py
cargo install --path meerai-cli --bin meerai --features web
```

Or build without installing:

```sh
make build-web
cargo build --release -p meerai-cli --features web    # target/release/meerai
```

`make package` builds the same web-enabled binary into a `dist/` archive plus
`.sha256`, matching the release artifacts.

Rust `1.100.0` (pinned in [rust-toolchain.toml](rust-toolchain.toml)) is
required; `rustup` installs it automatically. The bundle step also needs the
`wasm32-unknown-unknown` target and a matching `wasm-bindgen` CLI
(`cargo install wasm-bindgen-cli --version 0.2.130 --locked`; the version comes
from `Cargo.lock`).

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
meerai auth login openai --chatgpt    # use a ChatGPT Plus/Pro plan
meerai                                # full-screen modal TUI
meerai --prompt "explain this repository" --mode plan
meerai --prompt "fix the failing test" --mode auto --json
meerai --agent reviewer --prompt "review this change"
meerai --skill orchestrate
```

Short form: `-p` is the provider and `-m` is the model. `--prompt` (or `--print`)
is the headless input.

```sh
meerai -p openrouter -m google/gemini-3.8-flash --prompt "explain this repository"
meerai -p openrouter -m google/gemini-3.8-flash --prompt "fix the test" --mode auto --json
```

Run `meerai --help` for the generated command and option list. Common commands:

| Command | Action |
| --- | --- |
| `meerai` | Open the TUI |
| `meerai --prompt "..."` | One headless turn |
| `meerai --continue` / `--resume <ID>` | Reopen a session (TUI or headless) |
| `meerai --fork <ID>` | Fork and resume a session |
| `meerai auth login <provider>` | Save an API key, or sign in with a ChatGPT plan (`--chatgpt`) |
| `meerai sessions list\|rm\|prune\|fork\|export` | Manage sessions and export events as JSONL |
| `meerai memory list\|save\|delete\|pin` | Manage cross-session project memories |
| `meerai routes list\|set\|clear` | Manage task-type model routes |
| `meerai pack list\|add\|enable\|trust\|pin\|remove` | Manage declarative packs |
| `meerai hooks trust\|untrust\|status\|verify` | Manage project hooks |
| `meerai acp doctor` | Print the ACP handshake |
| `meerai serve` (`--features web`) | Start the password-protected local browser companion |

In the TUI, type `!` before a command to run it in the workspace, for example
`!cargo test`. It runs through the same sandbox, environment scrub, and
`strict_env` policy as agent shell calls; `plan` mode refuses it. Output opens in
a scrollable Terminal dialog (`j`/`k`). `Esc` backgrounds it: the dialog closes and
the run stays listed. Several shells can run at once. The statusline shows
clickable `N agents` and `N shells` counters for running work; a counter opens a
list of runs, and a row opens the agent conversation in a full chat overlay or
the shell output. `Esc` closes the overlay. The
result also lands in the transcript when it finishes; `q` closes the dialog.
Long commands use the shell timeout (120 s).

In the TUI: `/sessions`, `/new`, `/close`, `/connect`, `/models`, `/mode`,
`/agent`, `/skill`, `/loop`, `/compact`, `/context`, `/diagnose`, `/memory`,
`/remember`, `/routing`, `/subagent`, `/packs`, `/definitions`, `/effort`,
`/sampling`, `/temperature`, `/vault-reset`, `/open`, `/diff`, `/rewind`, `/fork`,
`/btw`, `/help`.

`/btw [question]` opens a transient, read-only tab seeded from the current
session. It reads files and answers follow-up prompts in its own history, but it
cannot write, run shell commands, or fetch the web. Nothing it says enters the
current session log or the session list, and closing the tab discards it. The
same command exists in the browser.

Headless runs print the assistant text to stdout. `--json` prints one NDJSON
`AgentEvent` per line and requires `--prompt`. The default approval mode is
`ask`.

## What It Provides

### Available today

- **Agent loop:** multi-turn steps with streaming output, approval
  suspend/resume, steering, cancellation, provider retries with one bounded
  recovery turn, token and wall-clock budgets, optional step caps, compaction
  with a cache-aware summarizer, and live context
  and usage telemetry.
- **Tools and sandbox:** read, write, edit, list, grep, glob, and shell tools
  with workspace path guards and unified diffs; a tree-sitter repository map;
  LSP definition, references, and diagnostics; script analysis through
  `python3` or `node`; and `web-fetch` / `web-search` for URLs and keyless web
  search. Shell and script children run in the OS sandbox when one is
  available, with provider secrets removed from the environment.
- **Agents, skills, and packs:** read-only markdown definitions from user
  config, trusted packs, `.meerai`, `.agents`, then `.claude`, plus the built-in
  `goal` and `orchestrate` skills, which run a bounded plan/act/observe/adjust
  loop. The model sees a
  skill list
  with one line per described skill and loads a skill on demand. An applied
  definition adds prompt text; agent `permissions` can tighten a subagent's
  policy but never grant. See
  [Agents and skills](#agents-and-skills).
- **Subagents and routing:** isolated child sessions with `explore`, `plan`,
  `build`, `general`, `research`, and `review` profiles, plus custom
  `mode: subagent` definitions,
  an optional run `name` shown in the transcript, runs list, and browser
  agent card, forwarded approvals, and usage rollup; a child `max_tokens` cap
  floors at 50,000 and clamps to the parent budget; task
  routes for `default`, `explore`, `plan`, `build`, `review`, `compact`,
  `recovery`, and `memory` with their own model, caps, and concurrency.
- **Sessions and checkpoints:** an append-only event log, `--continue`,
  `--resume`, `--fork`, session tabs, replay on resume, retention and prune;
  `/rewind` restores code, conversation, or both for the latest turn. Shell
  edits and files over 2 MiB are not tracked.
- **Memory and hooks:** project-scoped memory with search and automatic
  capture; user and trusted project hooks on thirteen lifecycle events.
- **Interfaces:** a modal TUI with tabs, pickers, a mentions popup, diff
  review, a command palette, and a numbered turn rail; headless one-shot runs
  with NDJSON output; and an opt-in browser companion on loopback. See
  [Usage](#usage) and [Browser companion](#browser-companion).
- **Providers, credentials, and config:** a bundled provider catalog with user
  overrides, an encrypted credential vault in the OS keychain, user settings
  and routes in SQLite, a project `.meerai/settings.yml` with tighten-only
  permissions, and `${VAR|default}` expansion.
- **External agent (ACP):** `--acp` drives `agy acp`, `acp doctor` prints the
  handshake, and `acp-run` delegates one task to a child session. The external
  agent keeps its own tools and policy.

### Not implemented yet

- OS sandbox on Windows hosts and on hosts without Seatbelt or bubblewrap.
- `meerai export`.
- Tool and permission mapping from agent definitions and MCP servers.
- USD cost, unless the provider catalog carries model prices.
- ACP presets for OpenCode and Claude, and `meerai acp` for editors.
- Multi-account ChatGPT plan switching.
- Signed and notarized release binaries.

## Crates

### Current workspace

| Crate | Role |
| --- | --- |
| `meerai-core` | Tool primitives and shared types |
| `meerai-input` | Shared multiline text input model for the TUI and browser |
| `meerai-macros` | `#[tool]` and `#[derive(Toolset)]` macros |
| `meerai-store` | Embedded SQLite store |
| `meerai-common` | Settings, project config, definitions, and packs |
| `meerai-client` | Provider catalog and the secret-store contract |
| `meerai-providers` | Provider clients and the encrypted credential vault |
| `meerai-session` | Session event log, replay, fork, and rewind |
| `meerai-policy` | Permission rules and approval modes |
| `meerai-tools` | Coding, repository map, LSP, and script tools |
| `meerai-agents` | Turn loop, prompt, subagents, and redaction |
| `meerai-skills` | Built-in skills, the loop engine, and skill tools |
| `meerai-syntax` | Tree-sitter highlighting |
| `meerai-tui` | Terminal UI |
| `meerai-host` | UI-neutral host seams |
| `meerai-web-protocol` | Browser companion DTOs |
| `meerai-web` | Loopback server for the browser companion |
| `meerai-web-client` | Browser client |
| `meerai-cli` | The `meerai` binary: CLI, headless runner, and TUI launcher |
| `meerai-acp` | ACP client bridge |

## Usage

```sh
meerai                                          # full-screen modal TUI
meerai --continue                               # TUI with the last session replayed
meerai --resume <ID>                            # TUI with a session replayed
meerai --fork <ID>                              # fork a session and resume it
meerai --prompt "explain this repository"       # one headless turn
meerai --prompt "..." --json                    # one JSON event per line
meerai --prompt "..." --continue                # resume the last session headless
meerai --prompt "..." --resume <ID>             # resume a session by id headless
meerai --mode plan|ask|auto                     # approval mode (default: ask)
meerai -p <provider> -m <model> --system "..."  # provider, model, system prompt
meerai --agent reviewer --prompt "review this change"  # apply an agent
meerai --skill orchestrate                      # apply a skill (repeatable)
meerai --acp                                    # TUI with an agy ACP session
meerai --acp --prompt "fix the test"            # headless ACP turn
meerai acp doctor                               # print the ACP handshake
meerai --mode auto --prompt 'Use acp-run with agent "agy" and task "Reply with OK".'
meerai --env-file .env.local                    # load variables from a file
meerai auth list|login|logout                   # inspect, save, remove API keys
meerai sessions list|rm|prune|fork              # list, delete, prune, or fork sessions
meerai sessions export <ID>                     # print session events as JSONL
meerai memory list|save|delete|pin              # manage project memories
meerai routes list|set|clear                    # manage task-type model routes
meerai pack list|add|enable|trust|pin|remove    # manage declarative packs
meerai config export|import <PATH>              # export or replace user settings
meerai hooks trust|untrust|status               # manage project hook trust
meerai hooks verify [--command CMD]             # install a Stop verify hook with a baseline
meerai sessions fork <ID> --turn <TURN_ID>      # fork from a specific closed turn
meerai sessions prune --days 30 --keep 5        # delete old sessions, keep 5 newest
meerai --version                                # print version (-V)
```

`-p` is the provider flag; use `--prompt` (or `--print`) for headless input.
`--json` requires `--prompt`. `--continue` and `--resume <ID>` select an
existing session for either mode: with `--prompt` they run headless, without it
they start the TUI with the stored transcript replayed. The two flags cannot be
combined. Without `--prompt`, the TUI starts (unless a subcommand or `--version`
is given), and a non-terminal stdin/stdout is an error.

### ChatGPT plan (Sign in with ChatGPT)

meerai can use a ChatGPT Plus/Pro plan instead of an API key. This uses
OpenAI's official **Sign in with ChatGPT** plan-usage flow for open-source and
locally hosted apps, with the `chatgpt.tokens.use.direct` scope and
`api.openai.com/v1/responses`.

```sh
meerai auth login openai --chatgpt   # browser sign-in; paste the callback URL if needed
meerai auth list                     # shows plan + expiry, never the token
meerai auth logout openai            # revokes best-effort and deletes the credential
meerai auth reset-vault              # delete all saved credentials, reinitialize the vault
```

The access token lasts one hour; meerai refreshes it three minutes before expiry
and rotates the 30-day refresh token. On a headless or remote host, paste the
final redirect URL when the browser cannot reach the loopback listener.

Plan requests use OpenAI's plan-usage contract, so explicit prompt-cache
breakpoints are unavailable on this route. An API key keeps the normal request
shape.

A plan usage limit is terminal. meerai does not retry it and does not silently
fall back to an API key. It tells you the plan limit was reached and points at
ChatGPT's usage settings and `/connect`.

In the TUI, `/connect` → OpenAI offers `Continue with ChatGPT` and `Use an API
key` when no credential is stored. A provider that already has a credential
opens a connection screen with `Continue to model select`, `Update credential`,
and `Remove saved credential`, so going back to a plan does not start a new
sign-in. The ChatGPT row opens the browser, shows the URL as text, accepts a
pasted redirect URL for remote hosts, and copies the URL with `ctrl+y`. `Esc`
cancels; on success the model picker opens.

Account model choices come from `GET /v1/models` at login and seed the recent
model list. Enter a model id manually if your account lists none. One ChatGPT
registration is supported per installation.

### Token usage

Headless runs without `--json` print one cumulative summary line to stderr after
the turn:

```text
tokens: prompt=1234 completion=567 total=1801 cached=800
```

`cached=<n>` appears only when the provider reports prompt-cache reads. Prompt
caching is on by default; set `context.prompt_caching: false` to disable it.

The provider reports token counters, never a cache percentage. meerai computes
the hit ratio as `cached_tokens / prompt_tokens`. That ratio is exact because
the provider adds cached tokens to `prompt_tokens` on every supported route.
Cache writes come from `cache_creation_tokens`. The sidebar `DIAGNOSE` block
(`/diagnose`) is shown by default; a click on its header collapses it. It shows
the last request's hit ratio and cache writes, the context
use, and the memory and tool-schema tokens. The sidebar File Changes block is
also shown by default and has its own `hide`/`show` toggle. Neither block shows a cumulative cache
ratio. A fork or a resumed stored session shows `n/a` until a new request
reports usage.

On Anthropic routes meerai sets one ephemeral breakpoint on the system block,
which ends with the project-memory section. The cache therefore covers the
system prompt and the tool schemas, not the conversation history. OpenAI and
Gemini cache the prefix automatically, so their session ratio rises with the
transcript.

The TUI statusline shows the workspace, branch, approval mode, model,
provider, temperature, throughput, the active tool, and clickable counters for
running agents and shells. Token counters live
in the sidebar `DIAGNOSE` block, not the statusline. `--json` reports the raw
counters. Counters are cumulative for the session, so a resumed run continues
from the last value. USD cost appears only when the provider catalog has
pricing for the model.

Old tool results leave the working context but stay in the session log.
`context.verbatim_window_turns` (default 4) keeps the newest N turns verbatim.
A long turn is compacted at a safe step boundary. A turn has no step cap by
default; set `context.max_steps_per_turn` to cap it. Both limits are
configurable; see [Project config](#project-config).

### Approval modes

| Mode | Behavior |
| --- | --- |
| `plan` | Hard read-only. Reads allowed; writes, shell, and external effects are denied even when a rule would allow them. |
| `ask` | Reads allowed; writes, shell, and external effects ask. Default. |
| `auto` | Allows every tier default, but still honors explicit `ask` and `deny` rules. |

`shift+tab` cycles `plan` -> `ask` -> `auto` in the TUI.

Grants skip later prompts for the same action and exact resource. In the
approval overlay, `y`/`Enter` approves once, `t` approves and records a turn
grant, `a` approves and records a session grant, `n` denies, `r` denies with a
reason, and `Esc` cancels. Turn grants are cleared at turn end. Grant resources
are literal, not glob patterns; permission rules still use globs.

When the OS sandbox blocks a shell command from writing outside the workspace,
meerai adds a `sandbox escape` approval. `y` reruns that command once without
the sandbox; `t`/`a` also run later shell commands without the sandbox for the
turn or the session. `plan` mode and `sandbox.require_for_shell` never offer
the escape.

### Browser companion

`meerai serve` (or `/serve` in the TUI) serves the browser client on loopback
with mandatory HTTP Basic auth. The browser uses the same host, registry,
session log, policy, and approvals as the TUI. Text, controls, and diffs are DOM
nodes, so selection, find, zoom, and screen readers work. See
[docs/browser-ui.md](docs/browser-ui.md).

Parity highlights:

- The approval dialog offers approve once, allow for the turn, allow for the session,
  decline, decline with reason, and cancel request. Each control maps to the TUI key with
  the same scope.
- `/clear` clears only the browser transcript view. `/reset` resets the conversation on the
  host. `/close` shuts down the live worker; the tab `×` only detaches the view.
- Activity digests group each contiguous run of thoughts, tools, and subagents in
  chronological order before the visible message that follows it, using the TUI category
  counts and phrasing and folding activity rows by default.
- The command palette lists slash commands and non-command actions: copy last reply,
  next/previous tab, move tab, and cycle approval mode.
- `Settings` groups Connections, Session, Theme, Context, Sessions, Sandbox, and
  Permissions. Provider keys are never read back.
- Assistant markdown renders with the same `comrak` feature set as the TUI: tables with
  alignment, task lists, footnotes, description lists, alerts, math, and preserved line
  breaks. Wide tables scroll in the browser and fit the width in the TUI.

### Sampling temperature

Set `context.temperature` in the user settings, or in the project
`.meerai/settings.yml` (which overrides it field by field). In the TUI, use
`/sampling` or `/temperature`; the browser uses Settings → Context. In the
sampling panel, Left/Right cycle the effort tier from either field, `l`/`m`/`h`
pick it directly, `x` picks extra high, the tier row is clickable, and typing
`default` clears the override.

Effort has five tiers: `low` (temp 0.00), `medium` (0.20), `high` (0.50),
`extra high` (0.70), and `max` (0.90). The tier list follows the active model:
the bundled catalog records the reasoning levels each model accepts, so
`extra high` appears only for models that accept `xhigh`, and `max` only for
models that accept `max`. A tier the model does not accept clamps down to the
nearest accepted tier. A model with no recorded reasoning support still
receives the tier temperature and no reasoning hint. An unknown model keeps all
five tiers.

In the TUI, `/effort xhigh` sets the extra-high tier; a three-tier model clamps
it to `high` and the status line reports the tier it applied. `/effort max`
sets the top tier where the model declares it.

Edit user settings with `meerai config export`, an edit, then
`meerai config import <PATH>`:

```yaml
context:
  temperature: 0.7
```

The range is `0.0..=2.0`. An out-of-range value is ignored and the provider
default applies. The value appears in the composer header (`temp 0.70`, or
`temp default`). Run `/reload` in the TUI to apply a change to the running
session; otherwise it applies at the next start.

### Network access

meerai talks to providers over plain HTTPS. There is no WebRTC or
peer-to-peer fallback transport, so a network that blocks a provider endpoint
must be routed around. Use one of these:

- **HTTP(S) proxy** — the HTTP layer honors the standard `HTTPS_PROXY`,
  `HTTP_PROXY`, `ALL_PROXY`, and `NO_PROXY` environment variables:

  ```sh
  HTTPS_PROXY=http://127.0.0.1:7890 meerai --continue
  ```

  To keep the setting, put it in `.env` and load it explicitly with
  `MEERAI_LOAD_DOTENV=1` or `--env-file .env`. Nothing loads implicitly.
- **VPN** — a system VPN changes the route for all traffic. No meerai setting
  is needed.
- **Same model, different route** — switch the transport, for example
  `meerai -p openrouter -m google/gemini-4-hydrogen`, when that endpoint is
  reachable from your network.
- **Base URL override** — point the provider at a reachable gateway with
  `base_url` in `providers.yml`.

A firewall that silently drops the connection can end a provider stream with
no text, no tool calls, and no usage. meerai treats that as a transient
provider failure and retries it with the standard provider backoff (3
attempts) before it fails the turn, so an empty answer does not silently end
the conversation. A stream that breaks after some text is discarded and
retried the same way; the partial answer never enters the session log. The
failure shows a diagnostic block with the provider,
model, stop reason, stream event count, elapsed time, and a hint. A provider
stop reason of `content_filter`, `max_tokens`, or `stop_sequence` is reported
as a non-retryable provider error instead of an empty stream.

### Agents and skills

meerai reads markdown agent and skill definitions from the project root, which is
the same root used for project config. Agents apply as prompt text. Skills load
on demand. Agent definitions marked `mode: subagent` are the only ones available
to the `subagent-run` tool.

| Kind | Paths, in priority order | Id |
| --- | --- | --- |
| Agent | user `~/.config/meerai/agents/*.md`, trusted pack agents, `.meerai/agents/*.md`, `.agents/agents/*.md`, then `.claude/agents/*.md` | frontmatter `name`, else the file name |
| Skill | user `~/.config/meerai/skills/*/SKILL.md`, trusted pack skills, `.meerai/skills/*/SKILL.md`, `.agents/skills/*/SKILL.md`, then `.claude/skills/*/SKILL.md` | frontmatter `name`, else the directory name |

- Priority is user-global, then enabled and trusted installed packs, then
  `.meerai`, then `.agents`, then `.claude`. If two sources define the same id,
  the higher source wins and a warning plus a shadow entry names both paths. The
  output is sorted by id.
- Pack definitions load only when the pack is enabled and trusted
  (`meerai pack enable|trust`). Pack hooks and `mcp_servers` declarations are
  validated and listed in diagnostics, but they are not executed or connected.
- `.meerai/agents` and `.meerai/skills` are repository-owned definition
  directories. Use them for this repository's native agents and skills.
- Discovery is read-only. meerai copies nothing, writes no import lock, and
  never edits the definition files. Symlinks are skipped with a warning.
- Frontmatter fields: `name`, `description`, `model`, `steps`, `mode`,
  `permissions`, and `disable-model-invocation`. `mode` is `all` (default),
  `primary`, or `subagent`: a primary-only agent is hidden from `--agent` and
  the pickers, and a subagent-only agent is offered only to `subagent-run`.
  `permissions` are applied to a subagent's policy after the parent rules;
  `allow` entries are dropped with a warning, so a definition tightens only.
  `tools` and `allowed-tools` print a per-definition warning and are not applied.
  Invalid frontmatter prints a warning and skips the file. Without frontmatter,
  the id comes from the file or directory name and the body is the prompt.
- Skill loading is progressive. The system prompt lists each skill that has a
  description as `<id>: <description>`. The model calls `skill-load` to read
  `SKILL.md`, and calls it again with a file name to read one supporting file.
  A skill without a description is not listed; apply it with `/skill` or
  `--skill`.
- `disable-model-invocation: true` keeps a skill out of the prompt list. It still
  works when you apply it directly.
- `skill-load` reads only inside the skill directory. It never runs scripts or
  reads files outside that directory.
- Definitions are repository-controlled prompt text. They cannot run tools,
  grant permissions, or imply a sandbox; agent `permissions` can only tighten a
  subagent. Treat them as untrusted content.
- Two skills are built in: `goal` and `orchestrate`. They run the same loop with
  the `loop-plan`, `loop-act`, `loop-observe`, `loop-adjust`, and `loop-status`
  tools. `@goal <instruction>` (or `@skill/goal`, `/skill goal`, `--skill goal`)
  starts the goal preset; `orchestrate` is the multi-area preset. While an item
  is open, the harness continues the turn with the current phase, up to 24
  rounds and the step cap when set, the time cap, and the token cap. A user, pack, or project
  skill with either id shadows the built-in. In headless mode, use
  `--mode auto`: the first write asks for approval in `ask` mode and exits `2`.

Startup flags apply definitions to the system prompt:

```sh
meerai --agent reviewer --prompt "review this change"
meerai --skill orchestrate
```

- `--agent <ID>` selects one agent. Repeating the flag is an error. The id must
  exist, and the definition must not be `mode: subagent`.
- `--skill <ID>` selects a skill and is repeatable. Skill text is appended
  before the agent text.
- An unknown id fails with the list of available ids.
- Model precedence is `--model` -> `MEERAI_MODEL` -> the agent `model` -> the
  configured model, then the provider default. An agent model that contains `:`
  is ignored with a warning because it would switch provider. `steps` prints a
  note and is not applied to a primary agent.

TUI commands and mentions:

| Surface | Action |
| --- | --- |
| `/agent <id>` | Apply an agent by id |
| `/agents` | Open the Mentions popup filtered to agents |
| `/skill <id>` | Apply a skill by id |
| `/skills` | Show discovered skills, warnings, token estimates, and whether the model can load the skill |
| `@` | Open the Mentions popup: agents and skills interleaved, then files |
| `@agent/<id>` | Apply an agent, then send the rest of the prompt |
| `@skill/<id>` | Apply a skill, then send the rest of the prompt |
| `@goal` | Apply the built-in goal skill, then send the rest of the prompt |
| `@orchestrate` | Apply the built-in orchestrate skill, then send the rest of the prompt |

Typing `@` opens one **Mentions** popup: agent and skill rows interleaved, then
files, so a skill stays visible when the project has many agents. A typed
query groups agents before skills. Up/Down or ctrl+p/ctrl+n move the selection;
Tab accepts; Enter accepts the highlighted row without sending; Esc dismisses.
Accepting inserts the mention and one trailing space, so a second Enter sends
the prompt.

A known `@agent/<id>`, `@skill/<id>`, `@goal`, or `@orchestrate` shows as a compact chip in the
composer. The model and the session log receive the stripped prompt. Unknown
ids stay as typed. On submit, skills apply before the agent, and repeats apply
once; a prompt that is only mentions applies the definitions without starting a
turn.

`/agent` and `/skill` are shortcuts for the same thing. An unknown or missing id
prints an error and sends nothing. `/agents` opens the Mentions popup filtered to
agents; `/skills` shows the discovered skills.

#### Loop

Two skills are built in: `goal` and `orchestrate`. Both drive one loop with four
explicit phases:

1. **Plan** — `loop-plan` stores the objective, done condition, constraints, and
   items.
2. **Act** — `loop-act` records one action and its expected observation.
3. **Observe** — `loop-observe` records the raw evidence.
4. **Adjust** — `loop-adjust` marks the item done, open, or failed, and may add
   items.

`@goal <instruction>` applies `goal` and sends the rest of the prompt.
`@orchestrate` applies the multi-area preset, which delegates research and
review to the `research` and `review` subagents. The loop is not code-specific:
an item can be a document, a data set, a launch, or a repository change.

An item can require human sign-off. The model asks with `question-ask`, records
the exact answer as the observation, and keeps the item open until the answer
approves it. A `research` or `review` subagent answer is not human approval.

The harness enforces the loop. When the model stops with an open item,
`LoopInterceptor` re-enters the turn with the current phase, up to 24 rounds. If
the turn still ends with open items, the worker starts a follow-up turn, up to 4
turns; each turn resets the round count. The step cap (when set), token cap,
approvals, cancellation, and `quit` still apply.

`/loop` opens a read-only Loop panel in the TUI and the browser. It
shows the objective, done condition, constraints, phase, each item with its
action and observation, and the round and turn counts. The last state is saved
as a `LoopUpdated` session event, so the panel and the model state come back
after `--resume`.

`loop-status` re-reads the in-process state after compaction. A headless
one-shot runs one turn only: use `--mode auto`. Steps are unlimited by default;
set `context.max_steps_per_turn` only when you need a cap. In `ask` mode the
first write stops the run with exit `2`.

### Headless exit codes

| Code | Meaning |
| --- | --- |
| `0` | The turn completed. |
| `1` | Failure: the turn failed or was aborted, an error occurred, or a Spend approval was declined. |
| `2` | A tool-call approval was required. The run fails fast, prints a hint to use `--mode auto` or the TUI, and cancels the turn. |

A **Spend** approval (session token cap reached) is not a tool approval. In
headless mode meerai prints how to raise or unset `context.max_session_tokens`,
declines the request, and exits `1` because the turn fails on budget. Do not
treat every approval as exit `2`.

### Sessions

```sh
meerai sessions list          # id, updated ms, bytes, title or model
meerai sessions rm <ID>       # delete one session
meerai sessions rm --all      # delete every session
meerai sessions prune         # delete sessions older than 30 days, keep none
meerai sessions prune --days 7 --keep 5
meerai sessions prune --days=7 --keep=5
```

`prune` deletes sessions older than the cutoff (`--days N`, default 30), except
the newest `--keep N` sessions (default 0). Those new sessions stay even when
they are older than the cutoff. The command prints `removed N sessions`. The
cutoff uses the global session root, which `MEERAI_SESSION_DIR` can override. At
startup meerai also deletes unused sessions: sessions with no user message, no
title, no parent session, and no activity for six hours. The active session and
every open tab are never deleted. Automatic retention pruning is opt-in; see
[Session retention](#session-retention).

`--continue` opens the most recently active session. With `--prompt` the run is
headless; without it the TUI starts and replays the stored transcript. Replay
covers user and steering messages, assistant text and reasoning, tool calls and
results, questions and their answers, and compaction markers. An unmatched tool
call shows as cancelled. Session metadata records the workspace, git branch and
SHA, title, provider, model, approval mode, lineage, and timestamps. See
[Data locations](#data-locations). Provider, model, and approval mode belong to
the session: changing the model or provider in one tab does not change another
session, and reopening a session restores its own values. An explicit
`--provider`, `--model`, or `--mode` overrides the stored value for that run.

A plain TUI start adds one empty session tab and restores the saved tabs in
the background, so the empty tab stays active. The empty session is reused on
the next start when it is still empty; closing it keeps it for that reuse.
`/new` and `[ + ]` switch to an open empty tab instead of opening a second
one.

The agent sets the session title from the first user prompt: first non-empty
line, at most 80 characters. A session that already has a
user message is not retitled by the agent.

## Data locations

meerai writes two XDG roots. Sessions live under the data root. Project files
are read-only inputs.

| Purpose | Path | Override |
| --- | --- | --- |
| Configuration | `$XDG_CONFIG_HOME/meerai` (default `~/.config/meerai`) | `XDG_CONFIG_HOME` |
| Auth and app data | `$XDG_DATA_HOME/meerai` (default `~/.local/share/meerai`) | `XDG_DATA_HOME` |
| SQLite database | `<data>/meerai.db` | `MEERAI_DB_PATH` |
| Session locks | `<data>/sessions` | `MEERAI_SESSION_DIR` |

macOS and Linux use the same paths. If `HOME` is unset, config and data lookup
returns nothing.

User state lives in one SQLite database. Use `meerai config export|import` for
YAML. Do not edit the database by hand.

### Configuration (`~/.config/meerai`)

- `providers.yml` — provider overrides and an optional `models:` list with
  `context_window`, `max_output_tokens`, `input_cost_per_mtok`,
  `output_cost_per_mtok`, and `effort_levels` (`low`, `medium`, `high`,
  `extrahigh`, `max`). The bundled `meerai-client/models.yml` carries
  the shipped model table; `make sync-models` (`scripts/sync-models.py`)
  regenerates it from [models.dev](https://models.dev/api.json), and
  `make models-check` fails when the committed table is stale. A provider with
  more than 50 text models keeps the 50 newest plus one flagship per major
  model family.
- The user settings (`active_provider`, `active_model`, `providers`,
  `approval_mode`, `permissions`, `context`, `routing`, `sessions`, and
  `theme`) are stored in the database. Change them with TUI Settings or
  `meerai config import <PATH>` after `meerai config export`.
- `hooks.json` — user hooks. `meerai hooks verify [--command CMD] [--reset-baseline]`
  installs a Stop hook with a per-project failure baseline. The first failing
  run records the known failures and does not block; later runs block only on
  new failure names, and a passing run clears the baseline. Cargo test names and
  pytest `FAILED` lines are recognized; other commands fall back to a
  command-level baseline. `--reset-baseline` clears it. It auto-detects
  `cargo test` or `npm test`; pass `--command` for anything else. The hook is a
  user hook, so it needs no project trust.

Sandbox hardening in `settings.yml`:

```yaml
sandbox:
  require_for_shell: true      # fail if no OS sandbox is available
  require_for_scripts: true    # same for sac-run_script
  strict_env: true             # allowlisted child environment
```

`strict_env` clears the child environment and keeps only `PATH`, `HOME`, `USER`,
`LOGNAME`, `SHELL`, `TERM`, `TMPDIR`, `PWD`, `LANG`, and `LC_*`. Provider
secrets are removed as well. A project `.meerai/settings.yml` can enable these
settings but cannot disable them.

### Auth and app data (`~/.local/share/meerai`)

- `meerai.db` — the SQLite database above, mode `0600`, opened with WAL.
  - `vault_secrets` — API keys and ChatGPT plan OAuth credentials, encrypted
    with ChaCha20-Poly1305. The 256-bit data key is generated on the first
    credential write and kept in the OS keychain under service `meerai`
    (account `vault-master-key-v1-<id>`). On macOS, a new version does not ask
    for keychain access again. Without a keychain it requires
    `MEERAI_VAULT_PASSPHRASE` and fails closed instead of writing plaintext.
    If the key is missing (for example, the keychain entry was deleted),
    meerai refuses to decrypt and tells you to restore the key or run
    `meerai auth reset-vault`. A reset deletes every saved credential; it
    never runs automatically.
  - Sessions and their event log. `meerai sessions export <ID>` prints JSONL.
  - Settings and open tabs, project memories, model routes, installed packs,
    usage rows, and spilled output with checkpoints.
- `sessions/<session-id>/` — per-session runtime files:
  - `session.lock` — worker lock.

Session directories are `0700` and database/session files are `0600` on Unix.

### Read-only project inputs

- `<project-root>/.meerai/settings.yml` — project settings. See
  [Project config](#project-config).
- `<project-root>/.meerai/hooks.json` — project hooks, loaded only after trust.
- `<project-root>/.meerai/instructions.md` and `<project-root>/AGENTS.md` —
  prompt instructions. See
  [Prompt and repository instructions](#prompt-and-repository-instructions).
- `<project-root>/.meerai/agents/...` and `<project-root>/.meerai/skills/...`, then
  `<project-root>/.agents/...`, then `<project-root>/.claude/...` — agent and
  skill definitions. See [Agents and skills](#agents-and-skills).

meerai never writes project files, definition files, or project-local sessions.

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
  `reasoning_pruning`, `prompt_caching`, `max_steps_per_turn`,
  `no_progress_fail_steps`, `step_limit_continuations`, `auto_memory`,
  `memory_prompt_bytes`,
  `tool_timeout_ms`, `max_session_tokens`, `max_turn_secs`, `summarizer_model`,
  `temperature`). The budget caps
  `max_session_tokens` and `max_turn_secs` are clamped against user values: a
  project value cannot raise a user cap, and a project cannot use `0` to disable
  the turn clock. `max_steps_per_turn` has no cap by default (`0` means
  unlimited); a project value can add a cap or lower a finite user cap.
- **`theme`** — reviewed like any other color choice and merged field by field
  over the user theme. It changes only the palette, never a capability.
- **Ignored fields** — `active_provider`, `active_model`, `providers`, and
  `approval_mode` are ignored with a warning, so a repository cannot change
  provider endpoints or switch the approval mode to `auto`. A project `sessions`
  block is also ignored with a warning: only the user config controls session
  retention.
- A malformed project file prints a warning and is ignored. Startup never
  aborts because of a bad project file.

`context.summarizer_model` is optional. It is a full `provider:model` spec used
for compaction summaries; the default is the session model. Summarizer usage
counts toward `context.max_session_tokens`.

### Theme

The TUI uses the opencode dark palette by default: near-black background,
raised surfaces for the sidebar, composer, and the user band, muted borders only
on dialogs, a teal accent, and a cool blue interactive color. Responses keep
the model's line breaks, paragraphs get a blank row, and composer rows are
spaced so long sessions stay readable. Active tool
rows keep the raised band; finished rows fade to muted text and recede into the
background so only running work stands out. Override any token in the user
settings (export, edit `theme`, import) or in
`<project-root>/.meerai/settings.yml` (project):

```yaml
theme:
  background: "#0a0a0a"
  surface: "#141414"
  surface_high: "#1e1e1e"
  text: "#eeeeee"
  muted: "#808080"
  faint: "#4c4c4c"
  accent: "#2dd4bf"
  interactive: "#7abaff"
  border: "#484848"
  error: "#e06c75"
  warning: "#ff9e64"
  success: "#7fd88f"
  info: "#56b6c2"
  diff_added: "#4fd6be"
  diff_removed: "#c53b53"
  diff_context: "#7d8590"
  code_function: "#c678dd"
```

Values accept `#rgb`, `#rrggbb`, or a terminal color name (`black`, `darkgray`,
`lightblue`, ...). An invalid value prints a warning and keeps the default.
The project theme overrides the user theme field by field.

### Session retention

Retention is manual by default: use `meerai sessions prune`. Automatic pruning is
opt-in in the **user** settings (export, edit `sessions`, import):

```yaml
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

Every request starts from the built-in system prompt. meerai then appends a
workspace-root line, the `--system` text (if any), the workspace `AGENTS.md`,
and `.meerai/instructions.md`. File tool paths are relative to the workspace
root, and a shell `cd` does not move it. The active model name is included, so
meerai can answer model-identity questions.

When a tool touches a subdirectory, that directory's `AGENTS.md` is added once
for the turn. Treat repository instructions as untrusted content from the
repository.

## Development

CI runs on every push and pull request to `main` that changes code: a push or
pull request that touches only Markdown, `docs/`, `site/`, `.agents/`, or
`LICENSE` is skipped, and so is the push that lands a version bump (its only
change is version lines). The run is `cargo fmt --all -- --check`, `cargo
clippy --workspace --all-targets`, then `cargo test --workspace` on
`ubuntu-24.04` with the pinned toolchain, plus the two reduced-feature checks
and a Wasm check of the browser client. The `cargo-deny` audit runs on every
push to `main` and on a pull request only when it changes `Cargo.toml` or
`Cargo.lock`. CI has no Windows runner: the Windows `cmd /C` shell path and
Windows session-id rules are unit-tested, but not exercised on a Windows host.
Run the same order locally:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

`make verify` (alias `make ci`) runs the three commands. `cargo check -p
meerai-cli --no-default-features` builds the headless surface without ratatui,
git2, comrak, `meerai-host`, `meerai-web`, or `meerai-tui`; `cargo check -p
meerai-tui --no-default-features` builds the TUI without the reqwest update
check. Every released binary and every `make install` binary already contains
the bundle and the `web` feature, so `meerai serve` and TUI `/serve` work out
of the box. To run the browser companion from source, build the bundle with
`python3 scripts/build-web.py` and run `cargo run -p meerai-cli --features web
-- serve` (or use `make build-web`, `make run-web`, and `make serve`). It binds
`127.0.0.1:28860` with
Basic auth (`meerai` / `12345678`) by default unless overridden (`--port`,
`--user`, `--password`, `MEERAI_SERVE_SECRET`, or `--auth`). CI runs both
reduced-feature checks. `.github/workflows/smoke.yml` runs one manual
live-provider headless turn when `OPENROUTER_API_KEY` is set.
`.github/workflows/release.yml` starts when a push to `main` changes
`Cargo.toml` and the pushed commit message is
`chore(release): bump version to <version>`, or on manual dispatch. A `run-name` cannot read the
version on the push path, so the run title stays `Release from version bump` and the job names carry
the tag instead (`Build <triple> (v0.0.2)`, `Publish v0.0.2`). It verifies the commit is a pure
version bump, builds the five targets when `v<workspace version>` is not yet released, and publishes
web-enabled archives plus SHA256 checksums. The release body lists the release commit and the commits
grouped by type since the previous tag, with a link to the full changelog; it does not list changed
files. `.github/workflows/bump.yml` is the only writer
of `[workspace.package] version`: run it manually with the target `version` and it rewrites
`Cargo.toml` and `Cargo.lock`, commits with that subject, and opens the bump pull request against
`main` with the job's own `GITHUB_TOKEN`. The input is required, and the run title is exactly
`Bump version <version>`. Squash-merge the PR and keep its title as the commit message, so the
release can match it.
`.github/workflows/pages.yml` deploys `site/` to GitHub Pages.

Macro expansion is snapshot-tested with insta: update with `cargo insta review`
or `INSTA_UPDATE=always cargo test -p meerai-macros`.

## Environment variables

Key resolution order per provider is the stored vault credential → `api_key`
in `providers.yml` → the provider's environment variables. The
bundled [providers.yml](meerai-client/providers.yml) declares those variables,
for example `OPENROUTER_API_KEY` (also `OPEN_ROUTER_API_KEY`) and
`GEMINI_API_KEY`. Shipped providers are `opencode-go`, `opencode`,
`openrouter`, `qwen-cloud` (`ALIBABA_TOKEN_PLAN_API_KEY`, also
`QWEN_CLOUD_API_KEY`), `gemini`, `openai`, `anthropic`, `deepseek`, `xai`,
`zai-coding` (`ZAI_API_KEY`), `moonshot` (`MOONSHOT_API_KEY`), `mistral`
(`MISTRAL_API_KEY`), and `ollama`.

- `MEERAI_PROVIDER`, `MEERAI_MODEL` — default provider and model; CLI flags win.
- `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` — standard proxy
  variables honored by the HTTP layer. Use them when the network blocks a
  provider endpoint. See [Network access](#network-access).
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME` — move the config and data roots. See
  [Data locations](#data-locations).
- `MEERAI_SESSION_DIR` — move only the session lock directory.
- `MEERAI_DISABLE_KEYCHAIN=1` — skip the OS keychain and derive the vault key
  from `MEERAI_VAULT_PASSPHRASE`.
- `MEERAI_NO_NOTIFY=1` — disable the window title, tab color, bell, and desktop
  notification.
- `MEERAI_NO_UPDATE_CHECK=1` — skip the startup release check.
- `MEERAI_LOAD_DOTENV=1|true|yes` — load `./.env`. Nothing loads implicitly.
- `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes` — load project `.meerai/hooks.json`.
  Without it, project hooks are ignored.
- `--env-file <PATH>` — load variables from an explicit file. A missing file is
  an error.

`.env` is gitignored but not read unless one of those opt-ins is used.

## Security and limits

- **OS sandbox is best-effort.** meerai uses macOS Seatbelt or Linux bubblewrap
  when available and falls back to in-process path guards with a startup
  warning. Path guards are policy checks, not a kernel boundary on the fallback
  path. `plan` mode denies writes and shell, but it cannot contain a command you
  approve in another mode. A sandbox escape rerun needs your explicit approval
  and runs with your user privileges; `plan` mode and `sandbox.require_for_shell`
  never offer it.
- **Secret handling is partial.** The shell tool removes known provider key
  variables and any variable ending in `_API_KEY`, `_API_TOKEN`,
  `_ACCESS_TOKEN`, `_SECRET`, `_SECRET_KEY`, or `_PRIVATE_KEY` from child
  processes. Tool output and diffs are also redacted on a best-effort basis
  before they are spilled, logged, shown in the UI, or sent to the model.
  Tool-dispatch errors are redacted before they are logged or sent to the
  model. The patterns cover OpenAI/OpenRouter `sk-`, Anthropic `sk-ant-`,
  Google `AIza`, GitHub `ghp_` and `github_pat_`, AWS `AKIA`, Slack `xox*`,
  `Bearer` tokens, and PEM private keys. Matches become `[REDACTED:<kind>]`.
  The original text is discarded; there is no log-only copy. Paths are not
  redacted. Redaction is pattern-based, not a guarantee, and a secret that does
  not match a pattern is not redacted.
- **Web tools are approval-gated external effects.** `web-fetch` and
  `web-search` declare `EffectClass::External` under the `webfetch` action, so
  `plan` mode hides them, `ask` mode requests an approval, and `auto` mode
  allows them. `web-fetch` accepts http and https only, rejects URL userinfo,
  follows at most five redirects, caps the body at 2 MiB and the output at
  60,000 characters, and refuses loopback, private, link-local, CGNAT,
  broadcast, multicast, and IPv4-mapped private addresses after DNS
  resolution. DNS rebinding is not prevented. `web-search` uses DuckDuckGo
  without an account and is best-effort.
- **Local files are private by default.** `meerai.db` is `0600`; session
  directories are `0700` on Unix. Windows has no equivalent mode enforcement.
- **Repository input is opt-in.** `.env` is not loaded implicitly. `AGENTS.md`
  and `.meerai/instructions.md` are injected into the prompt, so review them
  like code.
- **Agent and skill definitions are prompt text.** `.meerai/agents`,
  `.meerai/skills`, `.agents/agents`, `.agents/skills`, `.claude/agents`, and
  `.claude/skills` are repository-controlled content that reaches the model as
  instructions. Definition `permissions` can only tighten a subagent's policy:
  `allow` entries are dropped. Definitions cannot run tools, add capability, or
  change the sandbox. Review them like code.
- **Hooks run arbitrary host commands.** Hook entries execute with your
  privileges outside the sandbox. Project hooks stay disabled until you run
  `meerai hooks trust`, accept the one-time TUI prompt, set
  `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, or set `hooks.trust_project: true`.
  Hooks fail open by default; set `fail_closed` on a `PreToolUse` hook to deny on
  failure. Hooks can tighten policy, not loosen `plan`, an explicit
  `deny`/`ask` rule, or the outside-workspace approval.
- **Treat model and tool output as untrusted.** The system prompt says so, and
  heuristic injection screening wraps flagged content, but screening is
  pattern-based and not a guarantee.
- **Release binaries remain unsigned unless signing secrets are set.** macOS
  Gatekeeper and Windows SmartScreen may warn when publishing without
  certificates; verify the published `SHA256SUMS.txt`.

### Security roadmap

Security work is tracked in [docs/harness-spec.md](docs/harness-spec.md) and
[docs/threat-model.md](docs/threat-model.md). Path guards, sensitive-file
blocks, `plan` read-only, approval gates, secret scrubbing and redaction,
injection screening, file permissions, retention, token budgets, the OS
sandbox, the encrypted credential vault, hooks with project trust, and the
dependency audit are in place.

Pending: signing certificates for macOS notarization and Windows signing, and
OS sandboxing on Windows hosts.

## Documentation

- [docs/harness-spec.md](docs/harness-spec.md) — unified harness specification: event-sourced session log, typed item protocol, context engine, permissions/sandboxing, hooks, checkpoints, subagents, and retrieval strategy
- [docs/threat-model.md](docs/threat-model.md) — M11 security threat model, trust boundaries, controls, and residual risks
- [docs/architecture.md](docs/architecture.md) — current implementation notes, target harness architecture, protocol/loop design, and migration plan
- [docs/harness-internals.md](docs/harness-internals.md) — implementation reference: crate layout, configuration, turn-loop internals, TUI internals, and known traps
- [docs/tui-design.md](docs/tui-design.md) — modal TUI design, keybindings, pickers, diff review, and async Rust rendering architecture
- [docs/browser-ui.md](docs/browser-ui.md) — local browser companion: `meerai serve` / TUI `/serve`, host registry, parity HTTP/SSE API, and the implemented Leptos DOM client behind `--features web`
- [docs/acp.md](docs/acp.md) — Agent Client Protocol: the landed `agy` client (M13), the planned `meerai acp` editor surface (M14), and the trust boundary
- [docs/token-efficiency.md](docs/token-efficiency.md) — initiative for prompt caching, tool-result pruning, live call repair, and verified recovery
- [docs/openai-chatgpt-plan-usage.md](docs/openai-chatgpt-plan-usage.md) — initiative for using a ChatGPT Plus/Pro subscription through OpenAI's Sign in with ChatGPT plan-usage flow
- [docs/refactor-review.md](docs/refactor-review.md) — crate-by-crate audit of coding standards and structure: measured baseline, six findings axes, and a ranked cleanup backlog
- [AGENTS.md](AGENTS.md) — working rules for coding agents: commands, contracts, loop, config, security, TUI, commit, and code style

## License

Apache-2.0. See [LICENSE](LICENSE).
