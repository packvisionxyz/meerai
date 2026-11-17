# Architecture

This document mixes two views and labels them:

- **Current** — what the tree does today. Verified against the code.
- **Target** — the intended end state. Not built unless a status line says so.

The target is a **pure, provider-agnostic terminal agent harness** driven
by a **selection-first modal terminal UI** ([docs/tui-design.md](tui-design.md))
and a scriptable headless CLI mode. The protocol and context design live in
[docs/harness-spec.md](harness-spec.md).

## Current workspace

Twelve library crates plus the `meerai` binary (built by `meerai-cli`):

| Crate | Current role |
| --- | --- |
| `meerai-core` | `Toolset` trait (`effect_of`, `permission_spec`, `should_spill_output`, `invoke_structured`), `ToolCall`, `ToolDefinition`, `ToolOutput`, `ToolError`, `EffectClass`, `PermissionAction`, `ApprovalPreview`, `GrantScope`, `ToolObservation`, `ToolRun`, `FileDiff`, shared text/env/fs-perms helpers; re-exports `genai`, `async_trait`, `schemars::JsonSchema` |
| `meerai-macros` | `#[tool]` and `#[derive(Toolset)]` |
| `meerai-common` | `AppSettings` (`settings.yml`), project-root discovery (`find_project_root`) and project settings merge, markdown agent/skill discovery (`definitions`) |
| `meerai-client` | Client-facing provider surface: `ProviderCatalog` + `providers.yml`, `ModelInfo`, `RuntimeProviderState`, `KeyStatus`, and the `SecretStore` contract |
| `meerai-providers` | `genai` client factory + native secret stores (`auth.json`, OS keychain); re-exports the client surface |
| `meerai-session` | Append-only `SessionEvent` log, JSONL/zstd generations, `meta.json`, recovery, `derive_messages`, fork/rewind, snapshot store for rewind, blob store, spill, compaction primitives, session list/delete/prune support |
| `meerai-policy` | `PolicyEngine`: ordered rules, `ApprovalMode` presets, turn/session grants, YAML parsing |
| `meerai-tools` | `code` (`CodeTools` with seven `code-*` tools, path guards, unified diffs, observations, `workspace_search`), `repo_map` (`RepoMapToolset`), `lsp` (`LspToolset`), `search_as_code` (`SearchAsCodeToolset`) |
| `meerai-agents` | `MultiTurnAgent`, `AgentHarness`, `AgentEvent`, `intercept` (`Interceptor`/`StepPlan`/`ToolOutcome`), `question-ask` (`QuestionAsked`/`AnswerQuestion`), `prompt`, `spawn_worker`, `subagent` runtime, tool-output redaction |
| `meerai-syntax` | Tree-sitter highlighting for nine languages plus a lexical fallback; `highlight_block` returns per-line tokens and has no UI dependency |
| `meerai-tui` | Modal terminal UI (`ratatui` + `crossterm`, split `ui/` panels and `app/` modules) with transcript replay, tabs, subagent cards, question and rewind overlays, syntax-highlighted fences and diffs, and the `WorkspaceQuery` seam |
| `meerai-cli` | Binary crate (`meerai`): CLI flags (`--fork`), headless runner, `auth`/`sessions` (`list`/`rm`/`prune`/`fork`) subcommands, TUI launcher |

Dependency direction today:

```text
core ──▶ session ──▶ agents ──▶ cli
  ├──▶ policy ──▶ agents/common/tui
  ├──▶ tools ──▶ cli
  ├──▶ common ──▶ client ──▶ tui ──▶ cli
  │                    └──▶ providers ──▶ cli
  └──▶ syntax ──▶ tui
macros ──▶ (consumers)
```

`meerai-agents` re-exports the session vocabulary from `events.rs`, so protocol
code can name the types through `meerai_agents`; `meerai-tui` consumes events,
provider runtime state (`RuntimeProviderState`), and a `WorkspaceQuery` for file
listing and content search. It no longer links `meerai-tools`. `meerai-cli`
wires `client`, `providers`, `common`, `policy`, `session`, `tools`, `agents`, and
`tui` together.

Build surfaces: `meerai-tui` has a default `update` feature (reqwest);
`meerai-cli` has a default `tui` feature. `cargo build -p meerai-cli
--no-default-features` produces a headless binary without ratatui, git2, comrak,
or the TUI crate. genai still pulls reqwest in both builds.

## Current protocol and loop

- **`AgentEvent`** (`meerai-agents/src/events.rs`) is the harness→client
  stream: `TurnStarted`, `ItemStarted`, `ItemDelta`, `ItemCompleted`,
  `ApprovalRequested`, `ApprovalResolved`, `UsageUpdated`, `TurnCompleted`,
  `Error`. `ItemCompleted` is authoritative.
- **`SessionEvent`** is the durable log. When a harness is attached, requests are
  built with `session.derive_messages(...)`, not `chat_history`; the in-memory
  history is a mirror.
- **`AgentCommand`** is the client→loop channel: `SubmitPrompt`, `Steer`,
  `Approve`, `AnswerQuestion`, `Rewind`, `SetModel`, `SetPermissionMode`,
  `Compact`, `ApplyAgent`, `ApplySkill`, `Cancel`, `Reset`, `Quit`. There is no
  `Revert` or `Fork` command.
- **Checkpoints.** Write-class tools capture the original file into
  `snapshots/<turn-id>/` before execution. `AgentCommand::Rewind { turn_id,
  scope }` restores code and/or conversation and logs one `RewindApplied` event.
  The TUI `/rewind` menu targets the latest user turn.
- **Questions.** `question-ask` emits `QuestionAsked` and waits for
  `AnswerQuestion`; the answer is logged as `QuestionAnswered` and returned as
  JSON. Questions are not approvals: no policy decision, no grant, and headless
  runs auto-cancel with a hint. The tool is read-classified, runs before the
  rest of the batch, and is not registered for subagents.
- **Step interceptors.** `meerai_agents::intercept` is an ordered
  `Vec<Arc<dyn Interceptor>>` on the agent. `before_step` can stop a turn,
  `after_tool` receives the post-redaction/post-spill `ToolOutcome` and can add a
  model-visible follow-up, and `turn_stopping` can continue the turn once. This
  is the extension point for M6 checkpoints and M7 hooks.
- **Approvals.** `ApprovalMode` + ordered rules produce
  `Allow | Ask | Deny`. `Ask` emits `ApprovalRequested` and suspends the tool
  item; `Approve` resumes it. Without a command channel the request
  auto-declines. `AcceptForSession` records a session grant; the TUI `t` key
  sends `Accept` with `GrantScope::Turn`, cleared at turn end.
- **All tool calls per step run.** Consecutive read-only calls run concurrently;
  a mutating call serializes. `ToolOutput::Stop` halts the remaining batch.
- **Redaction.** `redact_run` rewrites observation stdout/stderr/hint,
  `ToolOutput` text, and `FileChange` diffs with `[REDACTED:<kind>]` matches
  before spill, the session log, `ItemCompleted`, and model text; dispatch
  errors go through `redact_text`. The original secret is discarded. This is
  pattern-based, best effort, and paths are not redacted.
- **Session titles and usage.** The agent titles a session from the first user
  prompt (single line, 80 chars) when the session has no title or earlier user
  message. Headless non-`--json` runs print a cumulative
  `tokens: prompt=… completion=… total=…` line on stderr; the TUI statusline adds
  running session totals. Counters are session cumulative and resume seeds them
  from the last logged `UsageUpdated`. `est_cost_usd` is computed when the
  provider catalog has `input_cost_per_mtok` and `output_cost_per_mtok` for the
  model; the bundled catalog ships context windows only, so it stays `None`
  unless a user override adds pricing.
- **Retries.** Provider calls: 3 attempts total, 250/500 ms with up to 50%
  jitter, status-classified transient-only (408/409/425/429 and 5xx; other 4xx
  are permanent; a message blocklist is the fallback). Tool calls: `max_retries`
  (default 3) with a fixed 250 ms sleep. Context overflow triggers one
  compaction retry. The worker queues one recovery turn after a retryable
  provider or `LoopDetected` failure.
- **Budgets.** Max steps per turn (default 32), per-tool timeout (default
  120 s), per-turn wall clock (`max_turn_secs`, default 1800, `0` disables),
  optional session token cap, and circuit breakers for repeated failures,
  identical edit batches, and no-progress streaks (8-step diagnostic, 12-step
  failure; disabled in `plan` mode and without a write tool).
- **Async safety.** Cancellation uses a per-turn `CancellationToken`; retries
  and timeouts use `tokio::time`. No library `println!`; the TUI and headless
  printer only consume events.

## Current configuration

Two config layers. The user file is `settings.yml` in `$XDG_CONFIG_HOME/meerai`
(default `~/.config/meerai`). The optional project file is
`<project-root>/.meerai/settings.yml`. `find_project_root` walks up from the
process CWD and stops at the first directory that holds `.git` (a directory or a
file, so linked worktrees count) or `.meerai/settings.yml`; with no match the CWD
is the root. Tools and sessions still use the CWD as the workspace, so a run
from a subdirectory applies the repository-root policy.

```yaml
# user settings.yml
active_provider: openrouter
active_model: deepseek-v4.1-flash
approval_mode: ask                        # plan | ask | auto

permissions:                              # ordered, last match wins
  - action: read
    resource: "*"
    effect: allow
  - action: read
    resource: "*.env*"
    effect: deny
  - action: shell
    resource: "git push *"
    effect: ask

context:
  context_window_tokens: 128000
  high_watermark_pct: 75
  spill_max_inline_tokens: 12500
  verbatim_window_turns: 4
  reasoning_pruning: true
  prompt_caching: true
  max_steps_per_turn: 32
  tool_timeout_ms: 120000
  max_turn_secs: 1800
  # max_session_tokens: 2000000
  # summarizer_model: openrouter:small-model

sessions:                                 # opt-in automatic retention
  auto_prune_days: 90                     # unset = manual (`sessions prune`)
  keep: 10                                # newest N spared; default 0
```

The project overlay is merged by `load_project_settings` and
`merge_project_settings`:

- `permissions` merge as project `ask` rules → user rules → project `deny`
  rules. `allow` entries are dropped with a warning, so a project cannot grant a
  capability. A project `ask` cannot downgrade a user `deny` because user rules
  match later, and a project `deny` always adds a denial after user rules. An
  invalid rule or non-list shape drops the whole project block with a warning.
- `context` overrides field by field. `max_session_tokens` and `max_turn_secs`
  are clamped so a project can only tighten them and cannot use `0` to disable
  the turn clock.
- `active_provider`, `active_model`, `providers`, and `approval_mode` are
  ignored with a warning. A project `sessions` block is also ignored with a
  warning: only the user config controls retention.
- A malformed project file warns and is ignored; startup does not abort.

Automatic session pruning is opt-in through the user `sessions` block. With
`auto_prune_days` set, startup deletes sessions older than N days except the
newest `keep` (default 0) and the currently active session, then prints
`Pruned N sessions (older than N days)` on stderr; an error prints a warning and
never aborts. Manual `sessions prune` and its `--days`/`--keep` flags are
unchanged. A project file cannot control retention.

`context.summarizer_model` is an optional full `provider:model` spec for
compaction summaries. The default is the session model. Its usage counts toward
`context.max_session_tokens`.

`context.verbatim_window_turns` also drives projection clearing:
`derive_messages` stubs tool results outside the newest N turns, keeps the call
pair, and names the spill blob. The same-model summarizer keeps the parent
clearing policy so the cached prefix matches; a distinct `summarizer_model`
disables clearing for a complete transcript. Pressure compaction can also run
inside a long turn at a balanced step boundary.

- `providers.yml` in the config directory holds user provider overrides
  (`base_url`, `model`, `api_key`, `options.apiKey`, `recent_models`).
- API keys: stored `auth.json` → configured `api_key` → provider env vars from
  the bundled `meerai-client/providers.yml`. `auth.json` lives under
  `$XDG_DATA_HOME/meerai` (default `~/.local/share/meerai`) and is `0600` on
  Unix.
- `.env` is **opt-in**: `MEERAI_LOAD_DOTENV=1|true|yes` or `--env-file <PATH>`.
  There is no implicit `dotenvy` load.
- Prompt composition: `BASE_SYSTEM_PROMPT` + a `Workspace root:` line (file
  tool paths are relative to it; a shell `cd` does not move it) + optional
  `--system` + workspace `AGENTS.md` + `.meerai/instructions.md` (32 KiB/file,
  64 KiB block). Scoped `AGENTS.md` files are injected once per turn when an
  authorized tool touches their directory (16 KiB, max 8 files).

## Current agents and skills (`meerai-common::definitions`)

M12 adds read-only markdown discovery. `load_definitions(&project_root)` reads:

| Kind | Order | Id |
| --- | --- | --- |
| Agent | `.agents/agents/*.md`, then `.claude/agents/*.md` | `name`, else the file stem |
| Skill | `.agents/skills/*/SKILL.md`, then `.claude/skills/*/SKILL.md` | `name`, else the directory name |

`.agents` wins a duplicate id and the warning names both paths. The catalog is
sorted by id. `.meerai` is config, sessions, and instructions only: there is no
`.meerai/agents` or `.meerai/skills` discovery and no copy or import step.

Frontmatter: `name`, `description`, `model`, `steps`, `mode`. `tools`,
`permissions`, and `allowed-tools` warn and are not applied. Invalid frontmatter
warns and skips the file; missing frontmatter falls back to the file or
directory name and uses the body. Only `SKILL.md` text is read.

Application is prompt-only:

- Startup `--agent <ID>` (one id; repeating is an error) and repeatable
  `--skill <ID>` append instructions to the system prompt
  (`compose_definition_prompt` in `meerai-cli/src/main.rs`).
- Model precedence is `--model` → `MEERAI_MODEL` → agent `model` →
  settings/provider default. An agent model containing `:` is ignored with a
  warning; `steps`/`mode` print a note and are not applied.
- TUI `/agent <id>` and `/skill <id>` send
  `AgentCommand::ApplyAgent`/`ApplySkill`, which the loop records as a System
  message at the next step boundary. Typing `@` opens the Mentions popup
  (agents, then skills, then files; ids in teal for agents and blue for
  skills, files in the normal text color). Up/Down or ctrl+p/ctrl+n move, Tab accepts, Enter accepts
  without submitting, Esc dismisses; an exact agent/skill id keeps the popup
  open. A known accepted mention renders as an `<id>` chip (bold teal agent,
  bold blue skill) while the raw token stays in the input buffer; unknown and
  partial tokens render as typed. On submit, skills apply first, mentions are
  stripped, repeats apply once, and an unknown mention stays as text. `/agents`
  and `/skills` open the Mentions popup filtered to that kind.

Definitions are untrusted repository prompt text. They grant no tools,
permissions, scripts, MCP, or sandbox. Under M8 (`subagent-run`), built-in
agents (`build`, `plan`, `explore`, `general`) get typed tool subsets, while
custom M12 definitions execute read-only (`ApprovalMode::Plan`, read-only tools).

## Current coding tools (`meerai-tools`)

`CodeTools` hand-writes `impl Toolset` (`name`, `definition`, `contain`,
`effect_of`, `invoke`, `invoke_structured`). It does **not** use
`#[derive(Toolset)]`. Tool schemas are generated with
`schemars::generate::SchemaSettings::draft07()` and a `None` meta-schema.

- `read_file` — UTF-8 contents with 1-based `offset` and `limit` (default 2000
  lines), rendered as `line: text`, with a truncation footer.
- `write_file` — creates parent directories, writes/overwrites, emits a unified
  diff.
- `edit_file` — exact `old_string` replacement; fails on zero or multiple
  matches unless `replace_all`; emits a unified diff.
- `list_dir` — sorted entries, directories as `d name/`, files as
  `f name (bytes)`; gitignore-aware via the `ignore` crate.
- `grep` — regex search, case-insensitive unless the pattern contains an
  uppercase letter, optional glob filter, skips ignored paths.
- `glob` — workspace-relative glob matching.
- `shell` — `sh -c` on Unix / `cmd /C` on Windows through `tokio::process`,
  workspace cwd pinned, `timeout_ms` (default 120 000), 32 KiB capped output,
  exit code/stdout/stderr in `ToolObservation`; non-zero exit returns
  `ToolOutput::Fail`. No output streaming. Provider secret variables are removed
  from the child environment.

Path guards (`code/paths.rs`) canonicalize reads and writes, confine writes to
the workspace plus writable roots, and confine reads to the workspace plus
readable roots (the binary adds the session directory so spill blobs are
readable). These are policy checks, not an OS sandbox.

## Current execution modes

- **Interactive TUI (`meerai`)** — default when no `--prompt` is given and
  stdin/stdout are terminals. There is no piped-stdin headless mode.
- **Headless (`meerai --prompt "..."`)** — one turn, assistant text to stdout;
  `--json` emits NDJSON `AgentEvent`s instead. `--mode` sets the approval mode.
- **Resume (`--continue` / `--resume <ID>`)** — selects an existing session log.
  With `--prompt` the run is headless and unchanged. Without it the TUI starts
  and replays the transcript: user/steering messages, assistant text and
  reasoning, tool calls and results, and compaction markers; runtime and system
  events are skipped.
- **Sessions (`meerai sessions list|rm|prune`)** — list sessions, delete one or
  all, or prune sessions older than `--days N` (default 30) while keeping the
  newest `--keep N` (default 0). Startup pruning is opt-in via
  `sessions.auto_prune_days`/`sessions.keep` and always spares the active
  session.
- **Exit codes** — `0` turn Completed; `1` failure (including a declined Spend
  approval); `2` tool-call approval required (fail fast with a `--mode` hint).

## Historical codebase review (pre-M1)

The original review drove M1–M5. Items 1–5 are resolved except where noted.

1. **Broken tool-call round-trip** — resolved: `genai` messages carry tool calls
   and tool responses; `ToolCall` ids survive.
2. **Non-streaming & duplicated providers** — resolved: `genai` streaming with a
   provider catalog and auth resolver; no `assert!` on API keys.
3. **TUI-incompatible agent loop** — resolved: event-driven, no `println!`, all
   tool calls per step, `tokio::time` retries, cancellation and approvals.
4. **Inflexible config loading** — resolved: `settings.yml` is optional and
   XDG-based, with a project overlay at `<project-root>/.meerai/settings.yml`
   (discovered by walking up to `.git` or `.meerai/settings.yml`) whose
   permissions are tighten-only.
5. **Manifest hygiene** — resolved: unused dependencies were pruned;
   `meerai-macros` keeps `meerai-core`, `schemars`, `serde`, `serde_json`,
   `insta`, `prettyplease`, and `futures-test` as `[dev-dependencies]`.

## Target crate layout

Not built unless marked. Current responsibilities are in the first table.

| Crate | Target role |
| --- | --- |
| `meerai-core` | Pure primitives; re-exports only stable protocol types |
| `meerai-macros` | Tool proc-macros |
| `meerai-providers` | Provider implementations and config-driven `genai` client factory split from `meerai-common` |
| `meerai-common` | Layered config (`settings.yml`), instructions and definition discovery |
| `meerai-tools` | Coding toolsets with diff preview and workspace guards |
| `meerai-agents` | Event-driven streaming loop, approvals, compaction, multi-tool execution |
| `meerai-tui` | Modal terminal UI; consumes `AgentEvent`/`AgentCommand` |
| `meerai-cli` | Dedicated binary crate: CLI, TTY launcher, headless (`--prompt`, `--json`), resume, config/env wiring. `-p` remains provider. |

Target dependency direction:

```text
core / macros
   ├──▶ providers
   ├──▶ common
   └──▶ tools
          │
          ▼
        agents ──▶ tui ──▶ cli
```

`meerai-cli` would wire `providers`, `common`, `tools`, `agents`, and `tui`
together; `meerai-tui` would consume `agents`/`core` events and not depend on
concrete providers.

## Target protocol additions

The enums are additive; these are future variants, not current code.

- `ChatCompletion`-style provider trait in `meerai-providers` (today the
  abstraction is the `genai` client plus the catalog).
- `AgentCommand::Rewind(turn_id)` and TUI `Fork` command for M6/future TUI UX (CLI `--fork` / `sessions fork` are shipped).
- `SessionItem::SubagentRun` is emitted by `SubagentToolset`; `Plan` item payloads remain reserved without a dedicated producer.
- A `meerai serve` JSON-RPC/SSE wrapper over the same event enum.

## Target harness capabilities (not built)

- **Checkpoints and rewind (M6):** per-turn snapshots, `/rewind`, restore code
  or conversation.
- **Hooks (M7):** deterministic lifecycle commands with JSON stdin and exit-2
  blocking (including post-edit LSP diagnostics hooks).
- **CLI export & TUI `/fork`:** `meerai export` and interactive `/fork` remain backlog (`sessions fork` and `--fork` CLI flags are shipped).
- **Definition mapping (M12 follow-ups):** map definition `tools`/`permissions`,
  load supporting skill files, and add MCP. Today definitions are prompt-only
  and custom subagent runs stay read-only.
- **Security & supply chain (M11):** landed — path guards, sensitive-file
  blocks (`is_sensitive_path` + `PolicyEngine::default_sensitive_rules`), `plan`
  hard read-only, approval gates, headless fail-fast, provider-secret env scrub,
  best-effort redaction, heuristic prompt-injection screening (`screen_run`),
  session file permissions, configurable retention, token/wall-clock budgets,
  `ExecutionWorld` (`SeatbeltWorld`/`BubblewrapWorld` with `HostWorld` fallback),
  OS keychain (`KeychainSecretStore` + `auth.json` fallback), `cargo-deny` CI
  audit (`deny.toml`), conditional signing in `release.yml` (inactive without
  signing secrets), and [threat-model.md](threat-model.md). See
  `docs/harness-spec.md` §9.6.
- **M6:** checkpoints/rewind v1 is current; shell-only edits, gitignored files,
  and `Summarize from here` remain backlog.
- **Context:** projection caching, frozen/shared messages, and per-model budget
  auto-tuning. Per-model context windows and optional pricing now come from
  `ProviderCatalog`; `est_cost_usd` is computed when both prices are configured.
  Progressive tool disclosure remains a target.

## Migration roadmap (status)

1. **Core protocol & provider split** — done via `genai` and `meerai-providers`.
2. **Event-driven streaming loop** — done (item protocol, cancellation,
   approvals, all-tool execution, async retries).
3. **Common cleanup** — done: `meerai-common` holds settings/definitions;
   `meerai-providers` holds the provider catalog and secret store.
4. **Coding & retrieval toolsets** — done (`code-*`, `retrieval-repo_map`,
   `lsp-*`, `sac-run_script`).
5. **CLI & headless runner** — done in `meerai-cli` (`--prompt`, `--json`,
   `--mode`, `--continue`, `--resume`, `--fork`, `auth`, `sessions list|rm|prune|fork`).
6. **Modal TUI** — partly done: `NOR`/`INS`, slash commands, `@file`
   completion, approval overlay with turn/session grants, sidebar, diff/search
   modals, subagent cards, and resume transcript replay. The `Space` leader, Select mode, `:`
   command mode, session picker, and transcript search are planned.
7. **Later milestones** — session persistence, compaction, M12 discovery, M8
   subagent runtime (`SubagentToolset` + built-ins + child sessions), M9 CLI fork,
   and M10 retrieval (`repo_map`, `lsp`, `search_as_code`), and M11 security &
   supply chain (`ExecutionWorld`, sensitive-file blocks, CI audit, threat model,
   heuristic screening, OS keychain, conditional signing) are shipped; checkpoints
   (M6), hooks (M7), MCP client support, and repository signing secrets are not.

## Non-goals

- Web/desktop GUIs, actor frameworks, or bot integrations unrelated to terminal
  coding workflows.
- Embedding-based retrieval for live code (lexical + structural only).
- Destructive history rewrites: rewind/compaction shadow, never delete.
