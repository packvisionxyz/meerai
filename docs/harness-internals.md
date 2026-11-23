# Harness Internals

Implementation reference for the meerai workspace: crate layout, macro
conventions, configuration mechanics, loop internals, and the traps that
break the application when changed.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-17 |
| **Audience** | Contributors and coding agents changing the harness |
| **Scope** | All crates; implementation-level invariants (not user-facing behavior) |
| **Related** | [AGENTS.md](../AGENTS.md) (working rules) · [harness-spec.md](harness-spec.md) (behavior, milestones) · [architecture.md](architecture.md) (layout) · [tui-design.md](tui-design.md) · [acp.md](acp.md) · [threat-model.md](threat-model.md) |

This file is not injected into the model prompt.

## 1. Overview

Rust workspace for Pack Vision's terminal agent harness: twelve library
crates plus the `meerai` binary (built by `meerai-cli`). Milestones M1–M13
are implemented: session log, item protocol, coding and retrieval tools,
policy engine, context engine, prompt composition, subagent runtime, CLI
forking, project-root config discovery, TUI replay, configurable retention,
redaction, OS sandbox, injection screening, keychain storage, CI audit,
conditional signing, definitions discovery, checkpoints/rewind v1, hooks
v1 with an explicit trust gate, and the `agy` ACP client. M14 and the
remaining backlog are tracked in [harness-spec.md](harness-spec.md) §4.1.

## 2. Implemented

### 2.1 Crate layout

Dependency spine: `core -> macros`, `core -> session -> agents -> cli`,
`core -> policy -> agents/common/tui`, `core -> tools -> cli`,
`common -> client -> tui/cli`, `client -> providers -> cli`,
`agents -> acp -> cli`. The diagram and build surfaces are in
[architecture.md](architecture.md).

- `meerai-core` — `Toolset` trait (`effect_of`, `effect_of_call`,
  `permission_spec`, `should_spill_output`, `grant_path_root`,
  `invoke_structured`), `ToolOutput::{Text,Fail,Stop}`, `ToolError`,
  `EffectClass`, `PermissionAction`, `ApprovalPreview`, `GrantScope`,
  `ToolObservation`, `ToolRun`, `FileDiff`, lenient argument helpers,
  shared `text`/`env`/`fs_perms` helpers; re-exports `genai`,
  `async_trait`, `schemars::JsonSchema`. `doctest = false`.
- `meerai-macros` — `#[tool]` and `#[derive(Toolset)]` (darling + syn +
  quote; test-only deps are dev-dependencies).
- `meerai-session` — append-only `SessionEvent` log in SQLite
  (`session_events`, FTS5 `session_events_fts`), crash recovery, idempotent healing,
  `derive_messages`, replay, fork/rewind (`latest_closed_turn`,
  `fork_latest`, `create_child`), SQLite snapshot store
  (`session_snapshots`), SQLite blob store (`session_blobs`), spill,
  compaction primitives, listing/deletion/pruning.
- `meerai-policy` — `PolicyEngine`: ordered action/resource/effect rules
  (globset, last match wins over default → user → agent), `ApprovalMode`,
  literal grants, YAML parsing.
- `meerai-tools` — `code` (`CodeTools`, seven `code-*` tools, path guards,
  unified diffs, `ToolRun` observations, secret env scrub, shared
  `workspace_search`), `repo_map` (`retrieval-repo_map`, tree-sitter +
  PageRank, `#[derive(Toolset)]`), `lsp` (`lsp-*` stdio JSON-RPC),
  `search_as_code` (`sac-run_script`), `world` (`ExecutionWorld`).
- `meerai-store` — embedded SQLite (`meerai.db`): WAL, FTS5, schema v1,
  typed repositories (`ConfigRepo`, `MemoryRepo`, `PackRepo`, `RoutesRepo`,
  `UsageRepo`), and private-file permissions.
- `meerai-common` — SQLite-backed `AppSettings`, `find_project_root`,
  `load_project_settings`/`merge_project_settings`, `definitions`
  (`load_definitions`, `load_definitions_for`, `AgentDefinition`,
  `SkillDefinition`, `DefinitionSource`, shadow diagnostics), `packs`
  (`meerai-pack.yml`, install/pin/trust state), `hooks` (`HookEvent`,
  `HookEntry`, `load_hooks`).
- `meerai-client` — `ProviderCatalog` + bundled `providers.yml`,
  per-model metadata (`models:` entries: `context_window`,
  `input_cost_per_mtok`, `output_cost_per_mtok`; `model_info`),
  `RuntimeProviderState`, `KeyStatus`, `SecretStore` contract,
  `MemorySecretStore`, auth listing.
- `meerai-providers` — genai client factory plus the AEAD `VaultSecretStore`
  (OS keychain DEK or `MEERAI_VAULT_PASSPHRASE` Argon2id fallback); re-exports
  the client surface.
- `meerai-agents` — `MultiTurnAgent`, `AgentHarness`/`HarnessConfig`
  (including `TokenPricing`), `AgentEvent`/`AgentCommand`, `intercept`
  (`Interceptor`, `StepPlan`, `ToolPlan`, `ToolContext`, `ToolOutcome`,
  `StepAction`, `TurnStopAction`, async `before_tool`), hook engine
  (`HookEngine`, `HookContext`, `HookReport`), `question-ask`, `skill-load`,
  `prompt::{BASE_SYSTEM_PROMPT, compose_system_prompt}`, `spawn_worker`,
  `subagent` runtime, delegated `acp-run` (`acp_subagent`), `redact`,
  `screen`, `shell_safety`.
- `meerai-syntax` — tree-sitter highlighting for nine languages plus a
  lexical fallback; `highlight_block` returns per-line `Token`s; no UI
  dependency.
- `meerai-tui` — modal TUI split into `ui/` panels and `app/` modules;
  `workspace::WorkspaceQuery` seam (no `meerai-tools`/`meerai-providers`
  dependency). Behavior and invariants: [tui-design.md](tui-design.md).
- `meerai-acp` — `AGY` preset, `connect`/`connect_with`, `AcpHandshake`,
  typed `AcpEvent`, `AcpWorker`, permission replies, cancel,
  `session/load`, stderr tail. Only `meerai-cli` depends on it.
- `meerai-cli` — `meerai` binary: flags, headless runner,
  `auth`/`sessions`/`hooks`/`acp doctor` subcommands, TUI launcher, host
  wiring.

### 2.2 Tool macro conventions

- `#[derive(Toolset)]` on a struct requires a hand-written
  `impl <Struct>Invoke for <Struct>` with
  `async fn <snake_case_tool>(&self, args: &<Params>) -> Result<ToolOutput, ToolError>`
  per declared tool. The generated `Invoke` trait is private to the
  expansion site: impl it in the same module. The trait is named from the
  toolset name, not the struct (`RepoMapToolset` → `RetrievalInvoke`).
- `CodeTools` does **not** use the derive; it hand-writes `impl Toolset`
  (`definition`, `contain`, `effect_of`, `invoke`, `invoke_structured`).
- Generated code references `meerai_core`, `schemars`, and `serde_json`
  by absolute path: consuming crates need all three as direct
  dependencies.
- Runtime names are `"<toolset_snake>-<tool_snake>"`. A free
  `#[tool(name = "Get Weather")] async fn get_weather` generates
  `GetWeatherToolset` (runtime `get_weather-get_weather`); params are
  inferred from the fn's first argument type.
- Params derive `JsonSchema` from schemars 1.x (workspace `schemars = "1"`,
  locked 1.2.3), not 0.8; there is no schema derive in `meerai-macros`.
  Tool schemas use `SchemaSettings::draft07()` with a `None` meta-schema.
- Expansion is snapshot-tested with insta
  (`meerai-macros/src/tool/snapshots/`, prettyplease). Update with
  `cargo insta review` or `INSTA_UPDATE=always cargo test -p meerai-macros`.

### 2.3 Configuration and environment

- Runtime config: `settings.yml` + `providers.yml` in
  `$XDG_CONFIG_HOME/meerai` (default `~/.config/meerai`). `providers.yml`
  accepts a `models:` list; the resolved `context_window` replaces the
  default only when `context.context_window_tokens` is unset, and pricing
  fills `est_cost_usd` only when both per-million prices exist.
- `ContextSettings` fields (all `Option`; defaults live in
  `HarnessConfig`): `context_window_tokens` (128 000),
  `high_watermark_pct` (75), `spill_max_inline_tokens` (12 500),
  `verbatim_window_turns` (4), `reasoning_pruning` (true),
  `prompt_caching` (true), `rollback_on_verify_failure` (false),
  `recovery_model`, `max_steps_per_turn` (32), `no_progress_fail_steps` (12),
  `tool_timeout_ms`
  (120 000), `max_session_tokens` (unset), `max_turn_secs` (1800),
  `summarizer_model`, `post_edit_diagnostics` (false), `temperature`
  (0.0..=2.0; `Temperature::effective` drops out-of-range values, so the
  provider default applies and the composer shows `temp default`). The
  composer displays the effective **merged** (user + project) value.
- Project config: `<project-root>/.meerai/settings.yml`;
  `find_project_root` walks up to `.git` (directory or file) or
  `.meerai/settings.yml`, else the CWD. Merge is tighten-only:
  permissions `ask` → user → `deny` (`allow` dropped with a warning);
  `context` field by field with budget clamps; `sandbox` tighten-only;
  `theme` field by field. `active_provider`, `active_model`, `providers`,
  `approval_mode`, `sessions`, and `hooks` blocks are ignored with
  warnings. A malformed file warns and is ignored.
- `sessions.auto_prune_days`/`keep` (user only) opt into startup pruning;
  the active session is always spared; output is
  `Pruned N sessions (older than N days)`. Startup also deletes empty
  sessions (no user message, no title, no parent) that are older than six
  hours and not open, via `prune_empty_sessions` (`Removed N empty
  sessions`). A plain TUI start opens one empty session tab, restores the
  saved tabs in the background, and reuses the remembered empty session.
  Manual retention: `sessions prune` (30 days, keep 0 defaults).
- Definitions: read-only discovery at the project root, `.meerai` before
  `.agents` before `.claude`, duplicate ids keep `.meerai` (warning names both
  paths), sorted by id, symlinks skipped. Frontmatter
  `name/description/model/steps/mode/permissions/disable-model-invocation`;
  `mode` parses to `AgentMode` and filters `--agent`/pickers/`subagent-run`;
  `permissions` become agent rules on a child policy and drop `allow` with a
  warning; `tools`/`allowed-tools` warn and are ignored; invalid frontmatter
  warns and skips; missing frontmatter uses the file/directory name. Skills
  advertise one prompt line each (`id: description`, 8 KiB cap,
  screened); `skill-load` reads `SKILL.md` or one supporting file inside the
  skill directory, 16 KiB cap. One skill is built in (`Builtin` source): `goal`.
  It is appended after user, pack, and project skills, so a `goal` skill in any
  of those sources shadows it. Explicit `--skill` bodies keep the 16 KiB /
  32 KiB caps. `--agent` takes one id (repeat is a parse error); `--skill` is
  repeatable; unknown ids fail with the available list. Model precedence:
  `--model` → `MEERAI_MODEL` → agent `model` → settings/provider default;
  a `:` model is ignored with a warning; `steps` is note-only for a primary
  agent, and `mode` filters selection.
- Hooks: user `<config-dir>/hooks.json`; project
  `<project-root>/.meerai/hooks.json` only with
  `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, user `hooks.trust_project: true`,
  or a canonical path in `hooks.trusted_projects`
  (`meerai hooks trust|untrust|status`; TUI prompts once on a terminal,
  default no). Entries:
  `{ event, matcher, if, command, baseline_file, timeout_ms, fail_closed }`.
  13 events (see [threat-model.md](threat-model.md) §2.7). Default timeout
  10 s, max 60 s; out-of-range warns and resets. Exit 0 passes (stdout JSON
  decisions), exit 2 blocks, other codes pass with an `exit code N`
  reason. `hooks.enabled: false` disables both sources. Files accept a
  JSON list or `{ "hooks": [...] }`. `meerai hooks verify` installs one
  user `Stop` hook with a per-project failure baseline
  (`<config>/verify-baselines/<hash>.txt`); `--reset-baseline` deletes the
  baseline and returns without installing. `/reload` re-reads both files
  and sends `AgentCommand::SetHooks`.
- ACP: `--acp` is a bare flag (no preset argument), conflicts with
  `--agent`/`--skill`, and ignores `--mode` with a warning; `plan` refuses
  it. `acp doctor` prints the handshake. Implementation invariants: §2.6.
- CLI help is generated from the `COMMANDS`, `TUI_COMMANDS`, and
  `OPTIONS` tables in `meerai-cli/src/cli.rs`; add a row there, not in a
  format string. `help_tests` asserts coverage and unique usages.
- Build surfaces: `meerai-tui` defaults `update` (reqwest); `meerai-cli`
  defaults `tui`. `--no-default-features` drops ratatui, git2, comrak, and
  the TUI crate; genai still pulls reqwest.
- `context.summarizer_model` selects the compaction model (default:
  session model); its usage counts toward `max_session_tokens`.
  `context.post_edit_diagnostics` runs `lsp-get_diagnostics` after a
  successful `code-edit_file`/`code-write_file` (5 s timeout, 4 KiB cap,
  screened, policy-gated as read, logged as one `RuntimeContext` message
  per edit; never prompts, never recourses into an edit). `/reload`
  re-applies it with reset-to-default semantics: removing the key turns
  the feature off. `/reload` also preserves the resolved
  `context_window_tokens` and `token_pricing` when the incoming settings
  do not set a window; an explicit settings window still wins. A model
  switch (`SetModel`/`ApplyAgent`) carries a host-resolved catalog window
  and applies it unless an explicit settings window pins it; the agent
  loop stays provider-agnostic (resolution lives in the TUI/catalog).
- `context.prompt_caching` (default true) sets request-level
  `CacheControl::Ephemeral` on every model step and the summary request.
  Cached/cache-write counts land in `Usage`; headless prints `cached=<n>`,
  the TUI shows `↻<n>`.
- `theme` overrides palette tokens field by field (user and project
  `.meerai/settings.yml`).
  Values: `#rgb`, `#rrggbb`, or terminal color names; invalid values warn
  and keep the default. `theme::install` sets the process palette once;
  render reads `theme::p()`. There is no separate working-directory
  overlay: a CWD that holds `.meerai/settings.yml` is itself the project
  root, so its theme applies through the project merge.
- Keys: the AEAD vault encrypts API keys and ChatGPT plan OAuth credentials in
  `vault_secrets`. The 256-bit DEK lives in the OS keychain (`/usr/bin/security`
  on macOS, `secret-tool` on Linux, Credential Manager on Windows) under a
  per-database `vault-master-key-v1-<id>` account; without a keychain,
  `MEERAI_VAULT_PASSPHRASE` derives the key with Argon2id and the vault fails
  closed. The macOS CLI keeps the keychain grant across rebuilds, but the grant
  covers `/usr/bin/security` rather than meerai, so it is not a per-application
  boundary. OAuth refresh uses one `BEGIN IMMEDIATE` transaction. Resolution:
  stored credential → configured `api_key` → provider `env_vars` from the
  bundled catalog. `${VAR|default}` and `{env:VAR}` expansion in config
  secrets.
- Sessions live in `$XDG_DATA_HOME/meerai/meerai.db` (`MEERAI_DB_PATH` overrides
  the file). `sessions`, `session_events`, `session_events_fts`,
  `session_blobs`, and `session_snapshots` hold metadata, the append-only event
  log, spilled output, and checkpoints. Only `session.lock` remains in
  `sessions/<id>/`. Dirs `0700`, files `0600` on Unix. `--continue` picks the
  most recently updated non-child session (falls back to the newest overall
  when only children exist).
- `sac-run_script` registers only when `ScriptRuntime::detect()` finds
  `python3` (preferred) or `node`. Inline `script` or `path` (validated
  via `resolve_for_read`); 15 s default timeout clamped to 60 s; 64 KiB
  output cap. `prompt::append_script_guidance` tells the model to prefer
  one analysis script and derive paths from the workspace root.
- Headless non-`--json` prints `tokens: prompt=… completion=… total=…`
  (plus `cached=…`) on stderr; counters are session-cumulative and seed
  from the last logged `UsageUpdated` on resume. Session titles: first
  non-empty line of the first user prompt, 80 chars, no retitle once a
  title or user message exists.
- `MEERAI_PROVIDER`/`MEERAI_MODEL` set defaults; CLI flags win. `.env`
  loads only with `MEERAI_LOAD_DOTENV=1|true|yes` or `--env-file <PATH>`.

#### Configuration traps

- Project config is found at the **repository root**, not the CWD; tools,
  sessions, and prompt injection keep the CWD as the workspace. Do not
  "fix" this by resolving the workspace to the project root.
- Definitions are found at the project root too: `.meerai`, then `.agents`,
  then `.claude`. Never copy definitions or add an import lock.
- A project `ask` cannot downgrade a user `deny` (user rules match later);
  a project `deny` always adds a denial after user rules. Do not keep
  `allow` or append project `ask` after user rules.
  `merge_max_session_tokens`/`merge_max_turn_secs` clamp budgets; a
  project `0` cannot disable the turn clock, and a project alone cannot
  introduce a clock the user did not set. A project `recovery_model` with
  `:` is dropped silently (no warning).
- Hook trust is caller-side: `load_hooks` loads a project file only when
  the CLI passes `Some(path)`; a project file cannot trust itself. Do not
  add an implicit trust prompt.
- Hook output is untrusted. `HookReport::reason()` also returns a reason
  for `Pass` (for example a non-zero exit); callers must not treat a
  `Pass` reason as a block. Keep the reason caps.
- `SessionEvent::HookFired` carries an additive `detail: Option<HookDetail>`;
  keep new hook data inside `HookDetail` or add `#[serde(default)]`
  fields; do not retype `event`/`decision` in place.
- Provider model metadata belongs in `ProviderCatalog` (`ModelInfo`), not
  `HarnessConfig`; the catalog supplies defaults only, and pricing is
  never invented in the bundled catalog.
- `-p` is **provider**, not prompt; headless input is `--prompt`/`--print`,
  and `--json` requires it.
- Approval mode is stored per session in the `sessions` row. Precedence:
  `--mode` > stored session mode > `settings.approval_mode` > `ask`.
  `/mode` persists through `SetPermissionMode`; forks inherit the source
  mode; restored tabs load each session's own mode. A model switch
  persists through `SetModel`/applied agent model (`persist_model`).
- Headless approvals: tool-call approval fails fast with exit `2` and a
  `--mode` hint; a Spend approval prints the preview title with the
  `max_session_tokens` and subagent `max_tokens` remedies, declines, and
  exits `1` (`Failed { Budget }`); external-agent (ACP) permissions decline.
  Completed is `0`; other failures `1`.

### 2.4 Turn loop and context engine

Specified in [harness-spec.md](harness-spec.md) §§5–7; these are the
implementation invariants.

#### Projection and prompt

- `events.rs` (`AgentEvent`, `SessionItem`, `ItemDelta`) and the
  `SessionEvent` vocabulary plus id newtypes are wire contracts shared by
  the loop, the TUI fold, headless NDJSON, and the log. Nothing is frozen
  before the first release: change variants freely, but update every
  consumer and replay path in the same change.
- Serde tagging: internally tagged enums cannot hold newtype variants
  with bare strings. `ItemDelta`/`AgentCommand`/`ContentBlock` use
  adjacent tagging (`tag = "type", content = "data"`); keep that shape.
- With a harness attached, requests come from `session.derive_messages`;
  `chat_history` is a mirror, never the source of truth.
  `request_messages` caches the full projection keyed by
  `(event_count, policy)` and invalidates on `set_harness`,
  `reload_config`, and `reset`. The key relies on the append-only log:
  any future in-place log mutation would silently serve stale messages.
- System prompt composition is not the projection:
  `BASE_SYSTEM_PROMPT` + `Workspace root:` line + `--system` + root
  `AGENTS.md` + `.meerai/instructions.md` (32 KiB/file, 64 KiB block) via
  `ChatRequest::with_system`; scoped `AGENTS.md` (16 KiB, max 8/turn) is
  injected as `RuntimeContext` user messages once per turn. An
  `Active model:` identity line is appended at request time and refreshed
  by `set_model`. It carries the bare model id, not the provider adapter
  prefix: an Alibaba model on the `openai` adapter reports the model id, not
  `openai::<id>`. Startup `--skill`/`--agent` append to the system prompt;
  TUI apply commands use System-sourced messages at the next step
  boundary.
- Projection clearing is a rule, not a rewrite:
  `ProjectionPolicy::clear_old_tool_results` (default true) stubs results
  outside `verbatim_window_turns` (the window floors at 1); the call stays
  paired and a spilled stub names its blob. The same-model summarizer
  keeps the parent clearing policy; a distinct summarizer disables
  clearing. Never clear at log-write time or delete a result.
  `ProjectionPolicy` fields: `strip_ephemera`, `prune_reasoning`,
  `verbatim_window_turns`, `clear_old_tool_results`,
  `include_system_messages`, and `pinned_turns` (the harness sets
  `include_system_messages` to false; the config knob is
  `reasoning_pruning`). `SessionEvent::ContextTurnPinned` persists pins;
  `derive_messages` keeps a pinned turn's tool results verbatim even
  outside the window. `SetProjectionOverrides` also carries in-memory
  overrides and logs the pin change.
- Compaction is cache-aware: with no `summarizer_model`, the summary
  request reuses the parent system, tools, and projected history with
  `SUMMARY_INSTRUCTION` as the last user message. A distinct summarizer
  uses a standalone transcript request with the instruction in the system
  slot and clearing disabled. Pressure compaction applies at most twice
  per turn and once per step; the planner falls back to balanced
  `StepEnded` boundaries. `run_compaction` returns
  `Result<bool, TurnError>`; `Ok(false)` means skipped (open bracket, hook
  block, no boundary). `ContextOverflow` retries once, only on `Ok(true)`.
- Reasoning signatures travel per tool call:
  `AssistantMessage.call_signatures` is index-aligned with `tool_calls`
  (`#[serde(default)]`, skipped when all `None`); `turn.rs` captures the
  full vector and back-fills from captured calls; `derive_messages`
  restores `thought_signatures`. Gemini 2.5/3 reject a function-call loop
  without them. Never send `None` for a stored signature.

#### Tool batch, breakers, and recovery

- The loop logs `AssistantMessage` before executing tools; every planned
  call logs a `ToolResult` even when refused, cancelled, or failed.
  Post-tool notes (interceptor follow-ups, hook block reasons, argument
  repair hints, post-edit diagnostics, scoped `AGENTS.md`, breaker
  diagnostics) buffer per batch and log as one `RuntimeContext` user
  message after the last `ToolResult`, keeping calls adjacent to results.
- `collect_stream` merges streamed tool-call chunks with captured calls;
  a later chunk replaces an earlier partial for the same id, and a
  captured call fills a streamed call that carried no thought signature.
- Breakers: the 3rd identical failing call injects a diagnostic, the 4th
  fails with `LoopDetected` (key: name + args + normalized failure text;
  success resets). The 3rd identical batch injects a diagnostic, the 5th
  fails; edit/write-only batches also log
  `CircuitBreaker { RepeatedIdenticalEdit }` (other batches log the
  runtime message only). `NoProgress` diagnoses at 8 write steps without a
  file change and fails at 12; a read-only step never increments it; disabled
  in `plan` and without a write tool; a `subagent-run`/`acp-run` delegation
  resets it.
- Healing: `Session::open_inner` runs `heal_dangling_tool_calls`, then
  closes open turns (`TurnEnded { Interrupted }`) and open compactions
  (`CompactionEnded { Failed }`). `run_turn` heals again after `TurnEnded`
  on both success and failure paths. The helper is idempotent, skips
  rewound/compacted ranges, and appends one error `ToolResult` ("tool
  execution interrupted before completion") per unpaired call.
  Read-only replay uses `Session::load_events` and never heals; never
  call `Session::open` from a read path, and never heal at projection
  time.
- `max_turn_secs` defaults to 1800 with a harness (`0` disables; without
  a harness the deadline is 0; subagent children get the parent's remaining
  deadline minus a 5 s margin, minimum 1 s, and normally fail their own turn
  first). Provider
  retries: 3 attempts, 250/500 ms + up to 50% jitter, status-classified
  (408/409/425/429 and 5xx transient; other 4xx permanent; message
  blocklist fallback). Stream requests set a 30 s header timeout and a
  120 s per-chunk read timeout; heartbeats reset the read timer. A silent
  stall or a fully empty stream is transient and retried; a stream that
  fails after content discards the started `AgentMessage`/`Reasoning` items
  (`AgentEvent::ItemDiscarded`) and retries under the same classification;
  no partial text reaches the log. A stream with
  usage but no content is a normal end. An empty stream carries a
  diagnostic block (`provider`, `model`, `stop_reason`, `events`,
  `elapsed`, and a hint) into the turn error. A provider stop reason of
  `content_filter`, `max_tokens`, or `stop_sequence` is reported as a
  non-retryable provider error instead of an empty transient stream, and
  captured reasoning counts as content. When the endpoint itself is
  blocked, route around it (proxy, VPN, another provider, `base_url`
  override); meerai uses plain HTTPS.
- The worker queues one recovery turn after a retryable provider or
  `LoopDetected` failure (`MAX_RECOVERY_ATTEMPTS = 1`, reset on any
  successful turn). The user-visible prompt names the failure and runs
  before the next queued prompt; `context.recovery_model` overrides the
  model for that turn only and the session model is restored (meta keeps
  the session model). Budget, config, tool, and non-retryable provider
  failures do not recover. Headless one-shot runs go through
  `spawn_worker`, wait for the recovery turn (`RecoveryStarted`), and
  exit `0` when it completes.
- Spend approval at the session token cap: `Accept` raises the cap by one
  context window, `AcceptForSession` removes it, `Decline` fails the turn
  with `Budget`.
- Step budget: `max_steps_per_turn` (default 32) is the first budget.
  `step_limit_continuations` (default 2) grants that many automatic
  continuations, so a turn gets 96 steps by default. At each continuation
  boundary the loop logs `STEP_CONTINUE_NOTICE` ("Continue now and finish
  the task... Do not summarize or list next steps") and keeps the tools.
  Only the absolute final step logs `STEP_LIMIT_NOTICE` and strips tools.
  Subagents set `step_limit_continuations = 0`, so their preset caps stay
  exact.
- No-progress breaker: `no_progress_fail_steps` (default 12) fails the turn
  when that many write steps make no file change and delegate no
  subagent. A read-only step never increments the counter. At two thirds of
  the threshold the loop logs a `CircuitBreaker` diagnostic and injects
  `NO_PROGRESS_NOTICE`. The value is clamped to at least 3 so the notice
  always stays reachable. Read-only turns never trip it.
- Context telemetry: `ContextBreakdown` is computed from the projected
  event log plus `tool_schema_tokens` and the system/memory prompt split.
  `MultiTurnAgent` emits `AgentEvent::ContextUpdated` after each step,
  after compaction, and on `InspectContext`. `ContextBreakdown` includes
  per-bucket tokens, watermark/headroom, spilled blobs, cleared results,
  pinned turns, verbatim turns, and cached prompt tokens. The TUI stores
  the latest breakdown per tab; the host registry stores it per live
  session and rebroadcasts `RegistryEventKind::ContextUpdated`.
- Goal telemetry: `AgentEvent::GoalUpdated` carries the full `GoalState`
  after each step with an active goal. The TUI stores it per tab; the host
  stores it per record, adds it to the session detail, and rebroadcasts
  `RegistryEventKind::GoalUpdated`. The worker logs
  `SessionEvent::GoalUpdated` at turn end; `meerai_session::goal::last_goal`
  restores it in `build_agent` and host `register_opened`.
  `AgentEvent::GoalContinued` announces an automatic follow-up turn and
  renders as a system row.
- Routing and spend: `HarnessConfig::routing` holds `ModelRoute` entries.
  `RoutingCoordinator` owns a global and per-provider semaphore from
  `max_concurrency`, plus a USD ledger from `note_cost` and a cap from the
  default route's `max_cost_usd`. `collect_stream` holds a permit for one
  provider stream; compaction does the same. A spent cap raises
  `ApprovalKind::Spend` before the next step. Route `effort` and
  `max_tokens_per_turn` override the request options; an explicit
  `summarizer_model` wins over the `compact` route.
- Project memory: `MemoryToolset` exposes `memory-search` (Read),
  `memory-save`, and `memory-delete` (Write). `MemoryCapture` writes a
  `Progress` row and one `Decision` row per key decision after a successful
  compaction when `HarnessConfig::auto_memory` is true. `compose_system_prompt_with_memory`
  appends the store's bounded `prompt_block` under `## Project memory` and
  screens it with `screen_untrusted_text`. The block is stable for the turn,
  so the prompt-cache prefix stays warm.
- Manual subagents: `SpawnSubagent` and `CancelSubagent` run through the
  same `SubagentToolset` runtime as `subagent-run`, with an explicit
  `item_id`. Approval modes are clamped to the parent (`plan` forces
  read-only), tools can only narrow the preset, and step/token/cost caps
  are bounded by the parent. `SubagentRun` carries model, mode, steps,
  usage, cost, and active tool; `ItemDelta::SubagentProgress` streams
  changes to both UIs.

#### Extension points

- Extend the loop through `meerai_agents::intercept`, not by adding
  branches to `turn.rs`. `before_step` may stop a turn
  (`StepAction::Stop`), async `before_tool` sees an authorized
  `ToolPlan`/`ToolContext` and returns `SessionEvent`s to log,
  `after_tool` sees the post-redaction/post-spill `ToolOutcome` and may
  return one model-visible note, and `turn_stopping` may continue the
  turn (consulted only when no tool calls were made). It can continue
  more than once, bounded by the remaining step budget; the `Stop` hook
  keeps running at each stop until it blocks once. Checkpoint
  capture is the built-in `CheckpointInterceptor` on `before_tool`
  (Write-effect tools only). The built-in `GoalInterceptor` continues
  the turn while the `goal` tool holds an open checklist item, up to
  `MAX_GOAL_ROUNDS` (24). When the turn still ends with an open goal
  (`Ok` or `Budget`), the worker logs `SessionEvent::GoalUpdated`, emits
  `AgentEvent::GoalUpdated` and `GoalContinued`, and pushes one
  continuation turn (max 4). The `goal-set`, `goal-check`, and
  `goal-status` tools are Read-classified: they change session state only,
  never a workspace file. Hooks do **not** use the interceptor chain:
  they use `HookEngine`, because they await host processes at lifecycle
  points outside the loop.

#### Session, approvals, and commands

- `AgentCommand` has `Rewind { turn_id, scope }` and **no** `Fork` or
  `Revert` variant; the log has no `RevertApplied` event. The worker
  answers `Rewind` with `AgentEvent::RewindCompleted`. `Session::rewind`
  closes open turns and appends one
  `RewindApplied { scope, restored, replaced }`; `replaced` drives
  conversation shadowing. The TUI sends `Both` from the message menu and
  the `/rewind` menu (second option: code only). Capture happens before
  write-class tools only; shell edits, external edits, gitignored files,
  symlinks, and files over 2 MiB are not restored.
- Questions are not approvals: `question-ask` uses
  `QuestionAsked`/`AnswerQuestion` (live `QuestionResolved`), consults no
  policy, records no grant, runs before the rest of the batch, and is not
  registered for subagents. The log records
  `QuestionAsked`/`QuestionAnswered`; headless auto-cancels with a hint.
- `Approve { request_id, decision, scope }`: `AcceptForSession` always
  records a Session grant; the TUI `t` sends `Accept` with `Some(Turn)`
  (cleared at turn end), `y`/Enter sends `None`, `a` maps to
  `AcceptForSession`.
- Replay (`meerai_session::replay`) keeps user/steering messages,
  assistant text/reasoning, tool calls/results, questions and answers,
  and compaction markers; system/runtime events are skipped and unmatched
  calls become `Cancelled`. Replay shadows rewound ranges but still draws
  compacted ranges plus a "Compacted history" marker — the model
  projection drops them. `session_usage_from_events` seeds counters from
  the last `UsageUpdated`.
- With a harness but no command channel (`prompt`/`prompt_streaming`),
  every `Ask` auto-declines. The TUI and headless use `spawn_worker`,
  which supplies the inbox.

### 2.5 Tools, policy, and execution

#### Policy and workspace bounds

- Decision order: `plan` gate (non-Read denied before everything) →
  literal grants → rules (default sensitive → user → agent, last match
  wins) → tier defaults. A grant therefore takes precedence over a rule
  that matches later for the same literal resource; grants are recorded
  only through an explicit approval. In `plan`, the request omits
  non-read tool schemas.
- Outside-workspace access forces `Ask` unless a session grant covers the
  path; approval calls `Toolset::grant_path_root`, widening read/write
  resolution and the shell sandbox. `outside_grants` are session-scoped
  and cleared on reset/rewind. Do not bypass the guard for unapproved
  paths.

#### Tool names and arguments

- Runtime names are `code-<tool>`; permission checks strip `code-`/`code_`
  before mapping. Arguments are normalized before parsing
  (`meerai_core::lenient::normalize_tool_arguments`: JSON-in-string
  unwrap, wrapper objects, up to 64 levels, escape repair) and coerced to
  the schema (`coerce_to_schema`: string numbers/booleans, scalar→array,
  null removal). Field aliases: `file`/`file_path`→`path`,
  `cmd`→`command`, `old_text`/`new_text`, `column`→`character`.
  `WrongArguments` and `InvalidFunctionName` are not retried; a rejected
  call logs a schema-based repair hint.
- `edit_file` repairs a missing `old_string` by normalizing whitespace
  and `read_file` line prefixes: a unique match applies once ("Anchor
  repaired"), ambiguous matches report line numbers without editing, and
  no match suggests up to three closest lines.

#### Output handling

- `redact_run` runs immediately after a tool run and before `screen_run`,
  spill, `ToolResult`, `ItemCompleted`, and model text; it covers
  observation stdout/stderr/hint, `ToolOutput` text, and `FileChange`
  diffs. Dispatch errors go through `redact_text`. The original secret is
  discarded; never add a log-only copy. Paths are not redacted.
- Spill applies to `Success`/`Stopped` observations over the budget;
  `code-read_file`, LSP, `sac`, and `question-ask` opt out. Storage
  failure fails open. The log keeps only the retained content; the full
  value lives in the blob.

#### Shell and environment

- Safe shell commands auto-approve: `shell_safety::command_is_safe`
  splits quote-aware on `&&`, `;`, `|` and requires every segment to be a
  read-only program (with per-program restrictions for
  `find`/`sort`/`date`/`sed`/`awk`/`git`/`rustup`/`command`). It rejects
  `||`, background `&`, redirects, `$()`/`${}`, backticks, env prefixes,
  absolute paths, `..`, `~`, and pipes into interpreters. `rustup` allows
  only `show`, `toolchain list`, `toolchain show`, and `target list`;
  `command` allows only `-v` followed by program names. The agent converts
  `Ask` to `Allow` only when the mode is not `plan`, the decision is `Ask`,
  and no rule matched (`matched_effect` is `None`); an explicit `ask` or
  `deny` rule always wins, so a broad `resource: '*;*'` ask rule in
  `settings.yml` disables auto-approval for every chain. Keep the
  allowlist conservative. Chaining is preferred over one command per call
  (each extra call re-sends the conversation).
- Unix `sh -c`, Windows `cmd /C`. `scrub_command_env` drops 25 named
  credential variables and 17 secret suffixes (case-insensitive) for
  shell, `sac`, and LSP children. `sandbox.strict_env` clears the child
  environment to the allowlist (`PATH`, `HOME`, `USER`, `LOGNAME`,
  `SHELL`, `TERM`, `TMPDIR`, `PWD`, `LANG`, `LC_*`), then the scrub runs.
  `run_command` caps streams at 1 MiB and sets `RLIMIT_FSIZE` (1 GiB) /
  `RLIMIT_CORE` (0) on Unix. `require_for_shell`/`require_for_scripts`
  fail closed on `HostWorld`.
- A sandboxed `code-shell` run whose output matches a write-denial marker
  carries `SANDBOX_DENIED_HINT`. The agent asks for
  `ApprovalKind::SandboxEscape` and then reruns with the hidden
  `unsandboxed` argument (`SandboxPolicy::Unrestricted`, same env scrub).
  Turn/session grants apply to later shell calls until the turn ends or the
  session resets. `plan` mode and `sandbox.require_for_shell` never offer
  the escape, and the model schema does not expose the flag.
- LSP servers (`rust-analyzer`, `typescript-language-server --stdio`,
  `gopls`, `dart language-server --protocol=lsp`, `pyright-langserver
  --stdio`, overridable) are env-scrubbed but run outside `ExecutionWorld`.
  Requests time out after 15 s; a stalled stdin write times out and retires
  the cached server so the next call respawns it.

### 2.6 ACP invariants

Specified in [acp.md](acp.md). The implementation traps:

- `session/load` replays history as text notifications **before**
  `Connected`; `AcpWorker` drains up to `Connected` so the local
  transcript is the source. Never process pre-`Connected` updates as a
  new turn.
- `Session::fork` does not copy `harness`/`remote_session_id`; the CLI
  fork path starts a new ACP session (`AcpStart::New`).
- A fork copies the event prefix but zeroes inherited `Usage` fields in
  `AssistantMessage`, `AssistantAttempt`, and `UsageUpdated` events. The child
  starts with empty token and cache counters while keeping the transcript.
- The SDK cannot set the child working directory: the adapter runs in the
  process CWD. Do not change the process CWD globally.
- `plan` refuses `--acp`; other `--mode` values are ignored with a
  warning. The worker ignores `SetPermissionMode`, so `/mode` has no
  effect on an ACP tab.
- The worker handles `SubmitPrompt`, `Cancel`, `Approve`, `Quit` and
  ignores every other command. ACP tool calls are `EffectClass::External`
  regardless of the reported kind; permissions are advisory and local
  policy is never consulted.

### 2.7 Security posture

Controls and residual risks are owned by
[threat-model.md](threat-model.md). Summary: path guards, sensitive-file
blocks, `plan` hard read-only, approval gates, headless fail-fast, env
scrub, best-effort redaction and screening, session permissions,
user-controlled retention, budgets, `ExecutionWorld` with `HostWorld`
fallback, AEAD credential vault (keychain DEK or passphrase, fail-closed),
`cargo-deny` CI audit, and
conditional release signing are implemented. Hooks run unsandboxed host
commands behind an explicit trust gate. ACP backends are outside local
policy. Remaining gaps: signing secrets are operator-configured, Windows
uses host guards, and LSP servers are unsandboxed.

## 3. Planned

Implementation-level targets are owned by their documents: incremental
projection and stable prefix ordering
([token-efficiency.md](token-efficiency.md) §3), `meerai serve` and the
provider trait ([architecture.md](architecture.md) §3), M14 and ACP
preset catalog ([acp.md](acp.md) §3), MCP and definition mapping
([harness-spec.md](harness-spec.md) §4.1).

## 4. Reference: documentation map

| Topic | File |
| --- | --- |
| Document standard and template | [_template.md](_template.md) |
| Product behavior, milestones, backlog | [harness-spec.md](harness-spec.md) |
| Crate architecture and build surfaces | [architecture.md](architecture.md) |
| TUI design and keybindings | [tui-design.md](tui-design.md) |
| Security controls and residual risk | [threat-model.md](threat-model.md) |
| ACP client and trust boundary | [acp.md](acp.md) |
| Token-efficiency initiative | [token-efficiency.md](token-efficiency.md) |
| Working rules for agents | [../AGENTS.md](../AGENTS.md) |
