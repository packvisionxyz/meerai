# meerai

Pack Vision's terminal agent harness — written in Rust, designed for
autonomous and interactive software engineering in the terminal with a
selection-first, modal TUI and a scriptable headless CLI.

**Status:** early-stage, distribution-hardened. The workspace has sixteen library
crates plus the `meerai` binary (built by `meerai-cli`). Milestones **M1–M13**
(session log, harness, checkpoints, hooks, subagent runtime, `meerai-cli` +
session fork, retrieval v2/v3, security & supply chain, agents/skills discovery,
and the `agy` ACP client) of the [harness specification](docs/harness-spec.md)
have landed. CI (`cargo-deny` + test gate), CI-driven release builds, installers,
[the threat model](docs/threat-model.md), and an Apache-2.0 license are in
place. Checkpoints (M6) v1 and hooks (M7) v1 have landed with an explicit
project-trust gate. `meerai export` does not exist yet (see
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
meerai auth login openai --chatgpt    # use a ChatGPT Plus/Pro plan
meerai                                # full-screen modal TUI
meerai --prompt "explain this repository" --mode plan
meerai --prompt "fix the failing test" --mode auto --json
meerai --agent reviewer --prompt "review this change"
meerai --skill orchestrate
```

Short form: `-p` is the provider and `-m` is the model. `--prompt` (or `--print`) is the headless input.

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
| `meerai sessions list\|rm\|prune\|fork` | Manage sessions |
| `meerai hooks trust\|untrust\|status\|verify` | Manage project hooks |
| `meerai acp doctor` | Print the ACP handshake |
| `meerai serve` (`--features web`) | Start the password-protected local browser companion |

In the TUI, type `!` before a command to run it in the workspace, for example `!cargo test`. The command runs through the same sandbox, env scrub, and `strict_env` policy as agent shell calls; `plan` mode refuses it. Its output opens in a scrollable Terminal dialog (`j`/`k`). `Esc` backgrounds it — the dialog closes, the statusline shows `shell <command>`, and the result lands in the transcript when it finishes; `q` closes the dialog. Long commands use the shell timeout (120 s).

In the TUI: `/sessions`, `/new`, `/close`, `/connect`, `/models`, `/mode`,
`/agent`, `/skill`, `/compact`, `/diff`, `/rewind`, `/fork`, `/help`.

Headless runs print the assistant text to stdout. `--json` prints one NDJSON
`AgentEvent` per line and requires `--prompt`. The default approval mode is
`ask`.

## What It Provides

### Available today

- **Agent harness (`meerai-agents`):** `MultiTurnAgent` turn/step loop plus
  `AgentHarness`; the `AgentEvent` item protocol (`TurnStarted`,
  `ItemStarted`, `ItemDelta`, `ItemCompleted`, `ApprovalRequested`,
  `ApprovalResolved`, `UsageUpdated`, `TurnCompleted`, `Error`); session
  dual-write, `derive_messages`-driven requests, approval suspend/resume, spill,
  compaction, provider prompt caching with cache-token reporting, age-based
  tool-result clearing, in-turn step-boundary compaction, token and
  wall-clock budgets, circuit breakers, cancellation,
  steering, status-classified provider retries with jitter (including empty
  provider streams retried as transient failures), one bounded
  recovery turn after a retryable provider or loop failure, consecutive
  read-only tool batches, first-prompt
  session titles, summarizer-model selection, optional sampling `temperature`,
  an ordered step interceptor seam
  (`before_step`, async `before_tool`, `after_tool`, `turn_stopping`) for
  checkpoints, and best-effort tool-output redaction before spill, logging,
  display, and model delivery.
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
  `derive_messages` projection, turn-boundary `fork`, shadowing rewind, a blob
  store, and spill plus compaction primitives.
- **Policy engine (`meerai-policy`):** ordered `action/resource/effect` rules
  (globset, last match wins), `ApprovalMode` presets (`plan`, `ask`, `auto`),
  turn and session grants, and YAML rule parsing.
- **Hooks (`meerai-common::hooks`, `meerai-agents::hooks`):** user and project
  shell hooks for thirteen lifecycle events. User hooks load from the config dir
  (`~/.config/meerai/hooks.json`); project hooks load from `.meerai/hooks.json`
  only after explicit trust. Hooks run concurrently with a timeout, receive JSON
  on stdin, use exit `0`/`2` plus JSON decisions, log `HookFired`, emit live
  `HookCompleted` events with TUI system lines, and can tighten policy but not
  loosen it.
- **ACP client (`meerai-acp`):** `--acp` drives `agy acp` as an external agent;
  `acp doctor` prints the handshake; `/acp` opens an ACP tab; the statusline and
  transcript header mark the external engine; `--continue`/`--resume` use
  `session/load`. The native agent can call the `acp-run` tool to delegate one
  task to `agy` in a child session; permissions appear in the approval overlay
  and the child appears under the parent session. ACP permissions are advisory.
  meerai policy, path guards, redaction, and the sandbox do not apply to the
  external process. OpenCode/Claude presets and `meerai acp` (M14) are backlog.
- **Subagent runtime (`meerai-agents::subagent`):** `SubagentToolset`
  (`subagent-run`) launches isolated depth-1 child sessions (`parent_session_id`),
  supporting built-in profiles (`explore`, `plan`, `build`, `general`) with
  typed tool subsets and read-only execution for M12 custom markdown definitions,
  forwarded parent approval prompts, usage rollup into parent counters, and
  `SubagentRun` TUI/headless items. In the TUI, each subagent row expands to
  show the child's own conversation, refreshed about once per second while it
  runs; the statusline names the running agents.
- **Checkpoints and rewind (M6 v1):** write-class tools capture the original
  file before a change. `/rewind` restores code, conversation, or both for the
  latest turn; a conversation rewind puts the message back in the composer for
  editing. Shell-only edits, external edits, and files over 2 MiB are not
  tracked.
- **User questions (`question-ask`):** the model can ask one to four
  single- or multiple-choice questions through `QuestionAsked` /
  `AnswerQuestion`. The TUI shows a choice overlay with keyboard navigation;
  headless runs auto-cancel with a stderr hint and let the turn continue.
  Each question and the selected answers stay in the transcript, live and after
  resume. The overlay marks single-choice questions (`Select one`) and
  multi-choice questions (`Select all that apply`), and always offers an
  `Other` row for a typed answer. Questions are not approvals: they never
  consult policy or record grants.
- **Coding and retrieval tools (`meerai-tools`):** `CodeTools` with the seven
  `code-*` tools (`read_file`, `write_file`, `edit_file`, `list_dir`, `grep`,
  `glob`, `shell`), workspace path guards, unified diffs, `ToolObservation`/`ToolRun`
  results, per-tool `EffectClass`, and provider-secret environment scrubbing; plus
  `RepoMapToolset` (`retrieval-repo_map` via tree-sitter + PageRank),
  `LspToolset` (`lsp-go_to_definition`, `lsp-find_references`,
  `lsp-get_diagnostics` over stdio JSON-RPC), opt-in post-edit diagnostics
  (`context.post_edit_diagnostics`), and `SearchAsCodeToolset`
  (`sac-run_script` running inline `python3 -I -c` / `node -e` scripts or
  existing script files through `path`, with env scrubbing and
  `EffectClass::Execute` policy gating). At startup meerai probes `python3`,
  then `node`; it registers the script tool only for the detected runtime and
  tells the model to prefer one analysis script over long chains of small
  lookups.
- **Modal TUI (`meerai-tui`) and CLI/headless runner (`meerai-cli`):** modal
  transcript (including sessions replayed by `--continue`/`--resume`/`--fork`),
  multi-session tabs (`/new`, `/close`, `[ + ]`, `alt+1..9`, `gt`/`gT`), `/sessions`
  switcher modal, provider and model pickers, sidebar with file changes, subagent
  items, diff and search views, approval overlay, `/compact`, `/mode`,
  `/agent`/`/skill`, a `@` Mentions popup for agents, skills, and files,
  collapsible per-turn activity summaries (`+`/`-`) with inline
  syntax-highlighted edit diffs, a numbered anchor rail that jumps between
  user turns, menu shortcuts (`ctrl+s` sessions, `ctrl+o` models, `ctrl+a`
  provider, `ctrl+e` diff), tab reordering
  (`alt+shift+arrows` or drag), a syntax-highlighted diff review with an `f`
  file list, a clickable running-subagent overlay from the statusline, a
  paged sidebar file list, a composer footer (INS/NOR · mode · model · provider ·
  temperature value), color-coded session tabs, and a `ctrl+p` command palette
  with sectioned actions and a right-hand shortcut column;
  on exit, the terminal shows the session title and a `meerai --resume <id>`
  continue line under the same gradient logo as the TUI header;
  one-shot headless turns with a token summary on stderr, NDJSON event stream,
  `sessions list|rm|prune|fork` management, `--fork <ID>`, startup reuse and
  cleanup of unused sessions, and opt-in startup retention
  (`sessions.auto_prune_days`).
  While a turn runs, `Enter` queues the message for the next turn and `ctrl+g`
  steers it now.
- **Config (`meerai-common`) and providers (`meerai-providers`):** a `genai`
  0.7 client factory over a provider catalog (bundled `providers.yml` plus user
  overrides), `auth.json` storage for API keys and ChatGPT plan OAuth
  credentials, `settings.yml` plus a project `.meerai/settings.yml` found by
  walking up to the repository root and merged with tighten-only permissions,
  and `${VAR|default}` env expansion. The bundled `openai` provider uses the
  Responses API and accepts either an API key or a ChatGPT plan sign-in.
- **Tool primitives (`meerai-core`):** `Toolset`, `ToolCall`, `ToolDefinition`,
  `ToolOutput`, `ToolError`, `EffectClass`, `ToolObservation`, `ToolRun`, and
  `FileDiff`.
- **Tool macros (`meerai-macros`):** `#[tool]` and `#[derive(Toolset)]`.

### Not implemented yet

- OS sandbox coverage on Windows hosts (Phase-1 host guards) and on hosts
  without Seatbelt or bubblewrap. `ExecutionWorld` sandboxes when available and
  warns on the fallback path.
- `meerai export`. TUI `/fork` is implemented (fork at the latest closed
  turn); a richer fork UX remains backlog.
- Tool or permission mapping from agent definitions, MCP servers, and skill
  supporting files. M12 reads only `SKILL.md` and custom subagent runs stay
  read-only.
- Frozen/shared message representations and incremental tail projection. The
  full-result projection cache (keyed by event count and policy) is implemented.
- USD cost and pricing: token counts are always reported; `est_cost_usd` is
  computed only when the provider catalog carries input/output prices for the
  model. The bundled catalog ships context windows only, so cost stays `None`
  unless a user `providers.yml` adds pricing.
- Progressive tool disclosure.
- ACP OpenCode/Claude presets and M14 (`meerai acp` for editors). The `agy`
  client is landed. An ACP backend runs an external agent with its own tools;
  meerai policy does not apply to it.
- Multi-account ChatGPT plan switching. One registration is supported per
  installation.
- Repository signing secrets (`APPLE_CERTIFICATE` / `WINDOWS_CERTIFICATE`) for
  non-empty signed/notarized GitHub Release binaries (conditional steps exist in
  `release.yml`).

## Crates

### Current workspace

| Crate | Role |
| --- | --- |
| `meerai-core` | `Toolset` trait (`effect_of`, `invoke_structured`), `ToolOutput`, `ToolError`, `EffectClass`, `ToolObservation`, `ToolRun`, `FileDiff`; re-exports `genai`, `async_trait`, and `schemars::JsonSchema` |
| `meerai-macros` | Proc-macros `#[tool]` and `#[derive(Toolset)]` |
| `meerai-common` | `AppSettings` (`settings.yml`), project-root discovery plus project settings load/merge (`find_project_root`, `load_project_settings`, `merge_project_settings`), agent and skill definition discovery (`definitions`) |
| `meerai-client` | Client-facing provider surface: `ProviderCatalog` + `providers.yml` metadata, `RuntimeProviderState`, key status, `SecretStore` contract, `MemorySecretStore`, and auth listing |
| `meerai-providers` | `genai` client factory plus native secret stores (`auth.json`, OS keychain); re-exports the `meerai-client` surface |
| `meerai-session` | `Session` append-only event log: JSONL/zstd generations, `derive_messages`, fork/rewind, blob + spill, compaction primitives, listing, deletion, and pruning support |
| `meerai-policy` | `PolicyEngine`: ordered permission rules, approval modes, turn/session grants |
| `meerai-tools` | `code` (`CodeTools`, seven `code-*` tools with path guards, unified diffs, observations), `repo_map` (`RepoMapToolset`), `lsp` (`LspToolset`), `search_as_code` (`SearchAsCodeToolset`) |
| `meerai-agents` | `MultiTurnAgent`, `AgentHarness`, `AgentEvent`, `prompt::{BASE_SYSTEM_PROMPT, compose_system_prompt}`, `spawn_worker`, `subagent` (`SubagentToolset` + child sessions), and best-effort tool-output redaction |
| `meerai-syntax` | Tree-sitter highlighting for Rust, Python, JavaScript, TypeScript/TSX, Go, Bash, HTML, CSS, and JSON, with a lexical fallback for other languages; returns per-line tokens with no UI dependency |
| `meerai-tui` | Modal terminal UI library (`ratatui` + `crossterm`) with transcript replay, subagent items, syntax-highlighted code fences and edit diffs, and approval overlay |
| `meerai-cli` | Binary crate (`meerai`): CLI flags (`--fork`), headless runner, `auth`/`sessions`/`hooks` subcommands, TUI launcher |
| `meerai-acp` | ACP client bridge: `agy acp` preset, handshake, typed events, `AcpWorker`, permission replies, cancel, `session/load`; used by the CLI and TUI |

## Usage

```sh
meerai                                          # full-screen modal TUI
meerai --continue                               # TUI with the last session replayed
meerai --resume <ID>                            # TUI with a session replayed
meerai --fork <ID>                              # fork a session and resume it
meerai --prompt "explain this repository"       # one headless turn
meerai --prompt "..." --json                    # one NDJSON AgentEvent per line
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
```

The credential is stored as a `type:"oauth"` entry in `auth.json` (`0600`). The
access token lasts one hour; meerai refreshes it three minutes before expiry
under a cross-process lock, and rotates the 30-day refresh token. On a headless
or remote host, paste the final redirect URL when the browser cannot reach the
loopback listener.

Plan requests follow OpenAI's preview contract: `store:false`, `stream:true`,
the system prompt as `instructions`, full history in `input`, and no
`temperature`, cache retention, or unsupported fields. Those restrictions apply
only to plan credentials; an API key keeps the normal request shape.

Implicit prompt caching still applies: meerai omits `prompt_cache_options` and
`prompt_cache_retention` because the plan route rejects them, and OpenAI caches
eligible prefixes automatically. Explicit breakpoints and cache-lifetime control
are unavailable on this route.

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

`cached=<n>` appears only when the provider reports prompt-cache reads. Prompt caching is on by
default; set `context.prompt_caching: false` to send no cache control. Cache-write counts are kept in
the session `Usage` but are not printed.

Old tool results also leave the working context. `context.verbatim_window_turns` (default 4) keeps
the newest N turns verbatim; an older result becomes a short stub that keeps the call paired and
points at the spill blob when one exists. The session log keeps every result. When a long turn
crosses the high watermark, compaction can run at a tool-pairing-balanced step boundary, at most
twice per turn. The summarizer reuses the parent system prompt, tools, and history when
`context.summarizer_model` is unset, so its prefix stays cache-warm.

Missing counters are omitted. `--json` reports the same data as `UsageUpdated`
events. The TUI statusline shows the current turn (`↑… ↓… Σ…`) and running
session totals (`session ↑… ↓… Σ…`). The counters are cumulative for the
session: a resumed run seeds from the last logged `UsageUpdated` entry. USD cost
is reported when the provider catalog has pricing for the active model;
otherwise `est_cost_usd` is `None`. The bundled catalog ships context windows
only, so the default is token-only reporting.

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

### Browser companion

`meerai serve` (or `/serve` in the TUI) serves the Leptos Wasm client on loopback with
mandatory HTTP Basic auth. The browser uses the same host, registry, session log, policy,
and approvals as the TUI. Text, controls, and diffs are DOM nodes, so selection, find, zoom,
and screen readers work. See [docs/browser-ui.md](docs/browser-ui.md).

Parity highlights:

- The approval dialog offers approve once, allow for the turn, allow for the session,
  decline, decline with reason, and cancel request. Each control maps to the TUI key with
  the same scope.
- `/clear` clears only the browser transcript view. `/reset` resets the conversation on the
  host. `/close` shuts down the live worker; the tab `×` only detaches the view.
- The per-turn activity digest uses the TUI category counts and phrasing, and folds tool
  rows by default.
- The command palette lists slash commands and non-command actions: copy last reply,
  next/previous tab, move tab, and cycle approval mode.
- `Settings` groups Connections, Session, Theme, Context, Sessions, Sandbox, and
  Permissions. Provider keys are never read back.

### Sampling temperature

Set the temperature in the user `settings.yml` (or the project `.meerai/settings.yml`, which overrides it field by field):

```yaml
# ~/.config/meerai/settings.yml
context:
  temperature: 0.7
```

The range is `0.0..=2.0`. An out-of-range value is ignored and the provider default applies. The value is shown in the composer header (`temp 0.70`, or `temp default`). Run `/reload` in the TUI to apply a change to the running session (`reload_config` rebuilds the harness config); otherwise it applies at the next start.

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
the conversation. The failure shows a diagnostic block with the provider,
model, stop reason, stream event count, elapsed time, and a hint. A provider
stop reason of `content_filter`, `max_tokens`, or `stop_sequence` is reported
as a non-retryable provider error instead of an empty stream.

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
- `.meerai` holds configuration and instructions only. meerai does not scan or
  create `.meerai/agents` or `.meerai/skills`.
- Discovery is read-only. meerai copies nothing, writes no import lock, and
  never edits the definition files. Symlinks are skipped with a warning.
- Frontmatter fields: `name`, `description`, `model`, `steps`, and `mode`.
  `tools`, `permissions`, and `allowed-tools` print a warning and are not
  applied. Invalid frontmatter prints a warning and skips the file. Without
  frontmatter, the id comes from the file or directory name and the body is the
  prompt.
- A skill uses only the text of its `SKILL.md`. Supporting files in the skill
  directory are not read. Agent and skill instructions are capped at 16 KiB per
  definition and 32 KiB total; excess text is truncated with a warning.
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
| `/agents` | Open the Mentions popup filtered to agents |
| `/skill <id>` | Apply a skill by id |
| `/skills` | Open the Mentions popup filtered to skills |
| `@` | Open the Mentions popup: agents, then skills, then files |
| `@agent/<id>` | Apply an agent, then send the rest of the prompt |
| `@skill/<id>` | Apply a skill, then send the rest of the prompt |

Typing `@` opens one **Mentions** popup. Agents are listed first, then skills,
then files. Agent rows show the id in the teal accent color, skill rows show
the id in the blue interactive color, and both show the source and description
after the id. File rows show the path in the normal text color. The row shows the
bare `<id>`, not the `@agent/` or `@skill/` prefix. The same id can appear once as
an agent and once as a skill.

After you accept a known `@agent/<id>` or `@skill/<id>` mention, the composer
shows only `<id>`: a bold teal agent chip or a bold blue skill chip. The raw
`@agent/<id>` or `@skill/<id>` text stays in the input buffer, and the prefix is
hidden only in the display. Unknown ids and partial tokens such as
`@skill/doesnotexist` or `@skill/del` render exactly as typed. Several mentions
on one line render as separate chips, and the surrounding text stays in place.

Filtering matches `@agent/<id>`, `@skill/<id>`, the bare `<id>`, and fuzzy id
prefixes. Up/Down or ctrl+p/ctrl+n move the selection; Tab accepts; Enter accepts
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
provider. `/agents` and `/skills` open the Mentions popup filtered to agents or
skills.

### Headless exit codes

| Code | Meaning |
| --- | --- |
| `0` | The turn ended `Completed`. |
| `1` | Failure: turn `Failed`/`Aborted`, `Error` event, task error, or a declined Spend approval. |
| `2` | A tool-call approval was required. The run fails fast, prints a hint to use `--mode auto`, or the TUI, and cancels the turn. |

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
`MEERAI_SESSION_DIR` can override. At startup meerai also deletes unused
sessions: sessions with no user message, no title, no parent session, and no
activity for six hours. The active session and every open tab are never
deleted. Automatic retention pruning is opt-in; see
[Session retention](#session-retention).

`--continue` opens the most recently active session (`updated_at_ms` moves on
every `TurnEnded`). With `--prompt` the run is headless; without it the TUI
starts and replays the stored transcript. Replay covers user and steering
messages, assistant text and reasoning, tool calls and results, questions and
their selected answers, and compaction markers. An unmatched tool call shows as
cancelled. System and runtime events are skipped. Session metadata records the workspace `cwd`, git branch and SHA,
title, model, approval mode, lineage, optional harness and remote session id,
and created/updated timestamps. See [Data locations](#data-locations).

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
| Sessions | `<data>/sessions` (default `~/.local/share/meerai/sessions`) | `MEERAI_SESSION_DIR` |

macOS and Linux use the same paths. If `HOME` is unset, config and data lookup
returns nothing.

### Configuration (`~/.config/meerai`)

- `settings.yml` — `active_provider`, `active_model`, `providers`,
  `approval_mode`, `permissions`, `context`, `sessions`, and `theme`.
- `providers.yml` — provider overrides and an optional `models:` list with
  `context_window`, `input_cost_per_mtok`, and `output_cost_per_mtok`.
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

`strict_env` clears the child environment and keeps only `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `TMPDIR`, `PWD`, `LANG`, and `LC_*`; the provider-secret scrub also runs on the child environment. Shell and script children also get `RLIMIT_FSIZE` = 1 GiB and `RLIMIT_CORE` = 0 on Unix. A project `.meerai/settings.yml` can enable these but cannot disable them.

### Auth and app data (`~/.local/share/meerai`)

- `auth.json` — API keys and ChatGPT plan OAuth credentials, mode `0600`. The OS
  keychain (`security` on macOS, `secret-tool` on Linux) is tried first for API
  keys; OAuth credentials stay in this file so a refresh can rotate them
  atomically. Every entry is rewritten through one lock, and an API-key write
  never drops a plan credential.
- `open-tabs.json` — TUI tab session ids, active index, and the reusable empty
  session.
- `sessions/<session-id>/` — one directory per session:
  - `session.v<N>.jsonl` — active append-only log.
  - `session.v<N>.jsonl.zstd` — sealed generation.
  - `meta.json` — cwd, git branch/SHA, title, model, approval mode, lineage,
    timestamps.
  - `blobs/` — spilled tool output.
  - `snapshots/<turn-id>/` — checkpoint manifest and captured files.

Session directories are `0700` and session files are `0600` on Unix.

### Read-only project inputs

- `<project-root>/.meerai/settings.yml` — project settings. See
  [Project config](#project-config).
- `<project-root>/.meerai/hooks.json` — project hooks, loaded only after trust.
- `<project-root>/.meerai/instructions.md` and `<project-root>/AGENTS.md` —
  prompt instructions. See
  [Prompt and repository instructions](#prompt-and-repository-instructions).
- `<project-root>/.agents/...` then `<project-root>/.claude/...` — agent and
  skill definitions. See [Agents and skills](#agents-and-skills).

meerai never writes project files, `.meerai/agents`, `.meerai/skills`, or
project-local sessions.

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
  `reasoning_pruning`, `prompt_caching`, `max_steps_per_turn`, `tool_timeout_ms`,
  `max_session_tokens`, `max_turn_secs`, `summarizer_model`, `temperature`). The budget caps
  `max_session_tokens` and `max_turn_secs` are clamped against user values: a
  project value cannot raise a user cap, and a project cannot use `0` to disable
  the turn clock.
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
background so only running work stands out. Override
any token in `settings.yml` (user) or `<project-root>/.meerai/settings.yml`
(project):

```yaml
# ~/.config/meerai/settings.yml
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
appends a `Workspace root:` line, the `--system` text (if any), the workspace
`AGENTS.md`, and `.meerai/instructions.md`. The root line states that file tool
paths are relative to it and that a shell `cd` does not move it. These files are
read per run and capped at 32 KiB each with a 64 KiB block cap. The agent also
appends an `Active model:` line
with the current model spec, so it answers model-identity questions without
reading your config. When an authorized tool touches a subdirectory,
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

`make verify` (alias `make ci`) runs the three commands. `cargo check -p
meerai-cli --no-default-features` builds the headless surface without ratatui,
git2, comrak, `meerai-host`, `meerai-web`, or `meerai-tui`; `cargo check -p meerai-tui --no-default-features`
builds the TUI without the reqwest update check. To build and run the opt-in
browser companion (`meerai serve` or `/serve` in TUI), build the Leptos Wasm bundle with
`python3 scripts/build-web.py` and run `cargo run -p meerai-cli --features web -- serve`. It binds `127.0.0.1:28860` with Basic auth (`meerai` / `12345678`) by default unless overridden (`--port`, `--user`, `--password`, `MEERAI_SERVE_SECRET`, or `--auth`). CI runs both reduced-feature
checks. `.github/workflows/smoke.yml` runs one manual live-provider headless
turn when `OPENROUTER_API_KEY` is set. `.github/workflows/release.yml` starts when
CI succeeds on `main`, on a `v*` tag push, or on manual dispatch. It reuses the
CI gate instead of re-running it, builds the five targets when
`v<workspace version>` is not yet released, and publishes archives plus SHA256
checksums. `.github/workflows/pages.yml` deploys `site/` to GitHub Pages.

Macro expansion is snapshot-tested with insta: update with `cargo insta review`
or `INSTA_UPDATE=always cargo test -p meerai-macros`.

## Environment variables

Key resolution order per provider is stored `auth.json` → `api_key` in
`providers.yml`/`settings.yml` → the provider's environment variables. The
bundled [providers.yml](meerai-client/providers.yml) declares those variables,
for example `OPENROUTER_API_KEY` (also `OPEN_ROUTER_API_KEY`) and
`GEMINI_API_KEY`.

- `MEERAI_PROVIDER`, `MEERAI_MODEL` — default provider and model; CLI flags win.
- `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` — standard proxy variables honored by the HTTP layer. Use them when the network blocks a provider endpoint. See [Network access](#network-access).
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME` — move the config and data roots. See
  [Data locations](#data-locations).
- `MEERAI_SESSION_DIR` — move only the session root.
- `MEERAI_DISABLE_KEYCHAIN=1` — store API keys in `auth.json` instead of the OS
  keychain.
- `MEERAI_NO_NOTIFY=1` — disable the window title, tab color, bell, and desktop
  notification.
- `MEERAI_NO_UPDATE_CHECK=1` — skip the startup release check.
- `MEERAI_LOAD_DOTENV=1|true|yes` — load `./.env` with `dotenvy`. Nothing loads
  implicitly.
- `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes` — load project `.meerai/hooks.json`.
  Without it, project hooks are ignored.
- `--env-file <PATH>` — load variables from an explicit file. A missing file is
  an error.

`.env` is gitignored but not read unless one of those opt-ins is used.

## Security and limits

- **OS sandbox is best-effort.** `ExecutionWorld` runs macOS Seatbelt or Linux
  bubblewrap when available and falls back to in-process path guards with a
  startup warning. Path guards are policy checks, not a kernel boundary on the
  fallback path. `plan` mode denies writes and shell, but it cannot contain a
  command you approve in another mode.
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
- **Hooks run arbitrary host commands.** `hooks.json` entries execute with your
  privileges outside `ExecutionWorld`. Project hooks stay disabled until you run
  `meerai hooks trust`, accept the one-time TUI prompt, set
  `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, or set `hooks.trust_project: true`.
  Hooks fail open by default; set `fail_closed` on a `PreToolUse` hook to deny on
  failure. Hooks can tighten policy, not loosen `plan`, an explicit
  `deny`/`ask` rule, or the outside-workspace approval.
- **Treat model and tool output as untrusted.** `BASE_SYSTEM_PROMPT` instructs
  this and heuristic injection screening (`screen_run`) wraps flagged content,
  but screening is pattern-based and not a guarantee.
- **Release binaries remain unsigned unless signing secrets are set.** macOS
  Gatekeeper and Windows SmartScreen may warn when publishing without
  certificates; verify the published `SHA256SUMS.txt`.

### Security roadmap

Security is tracked as M11 in
[docs/harness-spec.md](docs/harness-spec.md) and
[docs/threat-model.md](docs/threat-model.md). Landed in the tree: path guards,
sensitive-file blocks (`is_sensitive_path` and `PolicyEngine::default_sensitive_rules`),
`plan` hard read-only, approval gates, headless fail-fast, provider-secret
environment scrubbing, best-effort output/diff/error redaction, heuristic
prompt-injection screening (`screen_run`), session file permissions, configurable
retention, session/turn token budgets, `ExecutionWorld` OS sandbox (macOS
Seatbelt and Linux bubblewrap with Phase-1 fallback warning), OS keychain
storage (`KeychainSecretStore`) with `auth.json` fallback, `cargo-deny` CI
audit (`deny.toml`), conditional signing steps in `release.yml`, M7 hooks with
an explicit project-trust gate and tighten-only policy integration, and the
written threat model. Pending operational configuration: GitHub repository
signing certificates/secrets for macOS notarization and Windows signing, plus
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
- [AGENTS.md](AGENTS.md) — working rules for coding agents: commands, contracts, loop, config, security, TUI, commit, and code style

## License

Apache-2.0. See [LICENSE](LICENSE).
