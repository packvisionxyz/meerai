# meerai — canonical patterns

Repository knowledge for `.agents/skills/*`. Reference implementations a reviewer will compare
against.

## Workspace & dependency direction

- **Current:** `core -> session -> agents -> tui -> cli`; `core -> policy -> agents`;
  `core -> tools -> tui/cli`; `core -> common -> providers -> tui/cli`; `macros -> (consumers)`.
  The `meerai` binary lives in `meerai-cli`.
- Shared dependency versions live in root `Cargo.toml` under `[workspace.dependencies]` and are
  inherited with `{ workspace = true }`. Workspace metadata (`license = "Apache-2.0"`,
  `repository`, `homepage`, `rust-version = "1.100"`, `publish = false`) is inherited the same way.

## Tool definitions (`meerai-core`, `meerai-macros`, `meerai-tools`)

Three canonical shapes:

1. **Struct toolset (`#[derive(Toolset)]`):**
   - Annotate the struct with `#[toolset(name = "...", tools(...))]`.
   - Implement the generated module-private `<Struct>Invoke` trait in the **same module** with
     `async fn <snake_case_tool>(&self, args: &<Params>) -> Result<ToolOutput, ToolError>` (omit
     `args` for zero-arg tools).
   - Reference test: `meerai-macros/tests/derive_toolset_pass.rs`.
2. **Free-function tool (`#[tool(...)]`):**
   - Put `#[tool(name = "...", description = "...")]` on an `async fn`; it infers the parameter type
     from the function's first argument (do not declare `params` in the attribute) and generates
     `<PascalName>Toolset`.
   - Reference test: `meerai-macros/tests/attribute_tool_pass.rs`.
3. **Hand-written `impl Toolset` (the coding tools):**
   - `meerai-tools/src/code/mod.rs` implements `name`, `definition`, `contain`, `effect_of`,
     `invoke`, and `invoke_structured` directly. No `#[derive(Toolset)]` is involved.
   - Schemas come from `schemars::generate::SchemaSettings::draft07()` with `meta_schema = None`
     (`definition_for::<T>`).

Rules for all shapes:

- Parameter structs must derive `serde::Deserialize`, `serde::Serialize`, and `JsonSchema` (from
  workspace `schemars = "1"`, locked `1.2.3`). Do not use schemars 0.8 APIs or attributes.
- Consuming crates must declare `meerai-core`, `schemars`, and `serde_json` as direct
  dependencies.
- Runtime tool names are `"<toolset_snake>-<tool_snake>"`; built-ins are `code-read_file`,
  `code-write_file`, `code-edit_file`, `code-list_dir`, `code-grep`, `code-glob`, `code-shell`.
- `meerai-macros` exports only `#[tool]` and `#[derive(Toolset)]`. Parameter types use
  `#[derive(schemars::JsonSchema)]` from schemars 1.x; there is no schema derive in the crate.
- Macro expansion changes must be checked against `insta` snapshots in
  `meerai-macros/src/tool/snapshots/`.

## LLM providers (`meerai-providers`)

- Providers are reached through `genai` 0.7: `ProviderCatalog` + `RuntimeProviderState` +
  `build_genai_client` in `meerai-providers/src/providers.rs`.
- Provider ids are canonicalized (`canonical_provider_id`); `MEERAI_PROVIDER` / `MEERAI_MODEL` and
  CLI flags select the active pair.
- API-key resolution: stored `auth.json` → configured `api_key` in `settings.yml`/`providers.yml` →
  the catalog's `env_vars`. Missing keys fail in the genai auth resolver; there is no API-key
  assert.
- `.env` is opt-in. Do not add an implicit `dotenvy::dotenv()` call.

## Configuration & secrets (`meerai-common`)

- Runtime config is `AppSettings` loaded from `settings.yml` via `load_settings_from`; keys are
  `active_provider`, `active_model`, `providers`, `approval_mode`, `permissions`, and `context`.
  Fields are additive `Option`s so old files keep loading.
- Project config is `<project-root>/.meerai/settings.yml`. `find_project_root` walks up from the
  process CWD to the nearest directory containing `.git` (a directory or a file, so linked
  worktrees count) or `.meerai/settings.yml`; with no match, the CWD is the root. Tools and
  sessions still use the CWD as the workspace. Loaded by `load_project_settings` and merged by
  `merge_project_settings`. The rules are load-bearing:
  - `permissions` merge as project `ask` → user rules → project `deny`. `allow` entries are
    dropped with a warning; a project `ask` cannot downgrade a user `deny` (user rules match
    later), and a project `deny` always adds a denial after user rules. Drop the whole block on an
    invalid shape.
  - `context` overrides field by field; `max_session_tokens` and `max_turn_secs` are clamped so a
    project can only tighten them and cannot use `0` to disable the turn clock.
  - `active_provider`, `active_model`, `providers`, and `approval_mode` are ignored with a warning.
  - `sessions` is ignored with a warning: only the user config controls retention.
  - A malformed file warns and is ignored; startup never aborts.
- `context.summarizer_model` is an optional full `provider:model` spec for compaction summaries;
  the default is the session model, and summarizer usage counts toward `max_session_tokens`.
- Environment expansion supports `${VAR}` / `${VAR|default}` and `{env:VAR}`
  (`settings::expand_config_secret`).
- Credentials use the AEAD `VaultSecretStore` (`vault_secrets`) with a per-database DEK under
  service `meerai`. macOS reads and writes the DEK through `/usr/bin/security`, so the keychain
  grant survives rebuilds and upgrades but covers the tool, not meerai: it is not a per-application
  boundary. Linux uses `secret-tool`, Windows the Credential Manager, and `MEERAI_DISABLE_KEYCHAIN`
  skips the keychain. Without a keychain the vault requires `MEERAI_VAULT_PASSPHRASE` (Argon2id)
  and fails closed; there is no plaintext fallback. Resolution: stored credential → configured
  `api_key` → provider environment variables.
- Tool-output redaction is `redact_run` in `meerai-agents/src/redact.rs`. It runs in the agent after
  the tool run and before spill, session log, UI items, or model text. Patterns: OpenAI/OpenRouter
  `sk-`, Anthropic `sk-ant-`, Google `AIza`, GitHub `ghp_`/`github_pat_`, AWS `AKIA`, Slack `xox*`,
  `Bearer` tokens, and PEM private keys; matches become `[REDACTED:<kind>]`. Observation
  stdout/stderr/hint, `ToolOutput` text, `FileChange` diffs, and dispatch error text are covered.
  The original is discarded. Paths are not redacted. This is pattern-based, not a guarantee.

## Agent & skill definitions (`meerai-common::definitions`, M12)

- `load_definitions(&project_root)` returns a `DefinitionCatalog { agents, skills, warnings }`.
  Discovery is read-only and starts at the project root, not the CWD:
  `.meerai/agents/*.md` then `.agents/agents/*.md` then `.claude/agents/*.md`, and
  `.meerai/skills/*/SKILL.md` then `.agents/skills/*/SKILL.md` then `.claude/skills/*/SKILL.md`.
- `.meerai` has priority, then `.agents`, then `.claude`. A duplicate id keeps the higher entry and
  appends a warning that names both paths. The catalog is sorted by id. Nothing is copied and there
  is no import lock. Symlinks are skipped with a warning.
- Id: frontmatter `name`, else the file stem (agent) or directory name (skill). Frontmatter fields:
  `name`, `description`, `model`, `steps`, `mode`, `permissions`, `disable-model-invocation`.
  `mode` parses to `AgentMode` (`all`/`primary`/`subagent`, unknown warns and becomes `all`).
  `permissions` parses to `PermissionRule`s; `allow` entries are dropped with a warning, and an
  invalid block is dropped. `tools` and `allowed-tools` warn and are not applied. Invalid
  frontmatter (unclosed `---`, bad YAML) warns and skips the file. Missing frontmatter uses the
  fallback name and the whole body.
- Skill loading is progressive. `compose_definition_prompt` appends a `## Skills` block with
  `<id>: <description>` for each skill where `description` is set and `disable-model-invocation`
  is not. The block is capped at 8 KiB and screened. The read-only `skill-load` tool
  (`meerai-agents/src/skill_tool.rs`) returns the `SKILL.md` body and the supporting file names;
  with `file`, it returns one listed file. Reads stay inside the skill directory, reject path
  separators and symlinks, and cap text at 16 KiB. A skill without a description is not
  advertised but still works with `/skill`/`--skill`.
- Startup: `--agent <ID>` takes one id (repeating is an error); `--skill <ID>` is repeatable. Both append
  instructions to the system prompt through `compose_definition_prompt` after
  `compose_system_prompt`. Unknown ids fail with the available ids. Model precedence is `--model`
  → `MEERAI_MODEL` → agent `model` → settings/provider default; an agent model containing `:`
  warns and is ignored. `steps` prints a note for a primary agent; `mode` filters selection.
- TUI: `/agent <id>` and `/skill <id>` send `AgentCommand::ApplyAgent`/`ApplySkill`. Typing `@`
  opens the Mentions popup (`render_completion_popup`, title `Mentions`): agents first, then
  skills, then files. Agent rows show `<id>|agent` (cyan), skill rows `<id>|skill` (magenta), each
  with source and description; files show a path with no badge; one id can appear as both kinds.
  Filtering matches `@agent/<id>`, `@skill/<id>`, the bare id, and fuzzy prefixes. Up/Down or
  ctrl+p/ctrl+n move, Tab accepts, Enter accepts without submitting, Esc dismisses. Accepting
  inserts `@agent/<id> `, `@skill/<id> `, or `@path ` with one trailing space and stays in Insert;
  an exact agent/skill id keeps the popup open, an exact file path auto-dismisses. On submit, skills
  apply before the agent (`parse_mentions`/`apply_mentions`), mentions are stripped, repeats apply
  once, and an unknown mention stays as text; a mention-only prompt applies definitions without a
  turn. The agent logs `Agent instructions (<id>): …` or `Skill instructions (<id>): …` as an
  `InputSource::System` user message at the next step boundary. The composer renders a known accepted
  mention as an `<id>` chip (bold cyan agent, bold magenta skill) while the raw
  `@agent/<id>`/`@skill/<id>` stays in the input buffer and submit detection; unknown ids and
  partial tokens render as typed, and cursor columns use the chip display width. The composer title
  says `@ mentions`. `/agents` and `/skills` open the Mentions and definitions panels; there is no
  definition editor.
- Security: definitions are untrusted repository prompt text. No tool execution, permission/tool
  mapping, scripts, sandbox, or MCP. `skill-load` only reads files inside the skill directory.

## Session log (`meerai-session`)

- The log is append-only. `Session::append` writes plain JSONL (`session.v<N>.jsonl`); rotation
  seals a generation to `.jsonl.zstd`. Committed generations are never rewritten.
- `derive_messages` is the pure projection. It skips snapshots' shadowed ranges and keeps only the
  current turn's `InputSource::RuntimeContext` messages when `strip_ephemera` is on.
- Session dirs are private (`0700`; files `0600`) on Unix. `meta.json` records cwd, git branch/SHA,
  title, model, and timestamps. Project-local sessions write a `.gitignore` (`*`, `!.gitignore`).
- `list_sessions`, `latest_session`, and `delete_session` back the `sessions` subcommand and
  `--continue`. Ordering is by `updated_at_ms` (most recently active first). `sessions prune`
  (`select_prunable`) sorts newest first, skips the newest `--keep N` (default 0), then deletes
  sessions with `updated_at_ms < now - days*86_400_000` (default 30 days) and prints
  `removed N sessions`.
- Automatic retention is a separate opt-in path: `auto_prune_config` reads
  `settings.sessions.auto_prune_days` (unset = manual) plus `keep` (default 0), and
  `auto_prune_sessions` runs at startup after the session exists. It calls the same
  `select_prunable` with `exclude = Some(active_session_id)`, so the opened session is never
  deleted, prints `Pruned N sessions (older than N days)`, and warns instead of aborting on error.
  Do not let a project `sessions` block reach this path.
- TUI resume is `transcript_from_events(&session.events())`, wired only when `--prompt` is absent.
  It replays user/steering messages, assistant text/reasoning, tool calls/results, and
  `CompactionSummary` as a system entry; runtime/system/step events are skipped, and unmatched
  running tool calls are marked `Cancelled`. `session_usage_from_events` seeds TUI totals, and
  `set_harness` seeds the agent's cumulative usage from the last logged `UsageUpdated`. A fork
  zeroes inherited usage fields in the copied prefix, so the child starts with empty counters.
- Compaction is bracketed by `CompactionStarted`/`CompactionEnded`; recovery closes open turns and
  open compactions on open.

## Agent loop & prompt (`meerai-agents`)

- `MultiTurnAgent` plus `AgentHarness` is the current loop. Requests come from
  `session.derive_messages`, not `chat_history`; `chat_history` is a mirror.
- Emit `AgentEvent` items only. No `println!`/`eprintln!` in library crates.
- Provider retries: 3 attempts total, 250/500 ms backoff, only for errors without a non-transient
  marker (`api key`, `401`, `403`, `invalid model`, `not found`, `permission denied`). Context
  overflow triggers one compaction retry.
- Tool retries use a fixed 250 ms sleep and `max_retries` (default 3). There is no
  `invoke_tool_with_retry` function.
- Consecutive read-only calls run concurrently via `join_all`; mutating calls run one at a time.
  `ToolOutput::Stop` halts the remaining batch and marks later calls `Cancelled`.
- `max_turn_secs` (default 1800, `0` disables) is a per-turn wall-clock budget.
- Approvals: `Ask` emits `ApprovalRequested`, suspends, and resumes on `AgentCommand::Approve`.
  `AcceptForSession` records a Session grant; `scope: Some(Turn)` comes from the TUI `t` key and is
  cleared at turn end. `y`/Enter sends `scope: None`.
- Session title: `new_session_title` takes the first non-empty line, collapses whitespace, caps at
  80 chars, and runs only when the session has no title and no earlier `InputSource::User` message.
  The headless CLI also sets a title from the prompt when the session has none.
- Compaction uses `harness.config.summarizer_model` when set, else the session model. Recorded
  summary usage is added to `cumulative_usage`, so it counts toward the session token cap.
- Redaction: `redact_run(&mut run)` runs before spill, `SessionEvent::ToolResult`, `ItemCompleted`,
  and `ToolResponse` model text. Never log or persist the original secret.
- Headless non-`--json` prints `tokens: prompt=… completion=… total=…` on stderr after the turn
  from the last cumulative `UsageUpdated`. Counters are session cumulative and resume seeds them
  from the log. The TUI statusline adds `session ↑… ↓… Σ…` from the same cumulative usage.
  `est_cost_usd` is always `None`.
- Prompt composition: `BASE_SYSTEM_PROMPT` + a `Workspace root:` line (file tool paths are relative
  to it; a shell `cd` does not move it) + optional `--system` + root `AGENTS.md` +
  `.meerai/instructions.md` (32 KiB/file, 64 KiB block). Scoped `AGENTS.md` files are injected once
  per turn as `RuntimeContext` when a tool touches their directory (16 KiB, max 8 files).

## Coding & retrieval toolsets & modal TUI

- `meerai-tools` exposes `CodeTools` (`code-read_file`, `code-write_file`, `code-edit_file`,
  `code-list_dir`, `code-grep`, `code-glob`, `code-shell`), `RepoMapToolset`
  (`retrieval-repo_map`), `LspToolset` (`lsp-go_to_definition`, `lsp-find_references`,
  `lsp-get_diagnostics`), and `SearchAsCodeToolset` (`sac-run_script` using `python3 -I` /
  `node -e` with env scrubbing and `EffectClass::Execute` policy gating).
- `shell` runs `sh -c` on Unix and `cmd /C` on Windows through `tokio::process`, with a pinned
  workspace cwd, 120 s default timeout, 32 KiB capped output, and no streaming. Provider secret
  variables are removed from the child environment.
- Path guards use `resolve_for_read` / `resolve_for_write`; writes are confined to the workspace
  plus writable roots, reads to the workspace plus readable roots (including the session dir).
- `meerai-tui` keeps rendering (`ratatui` + `crossterm`) decoupled from providers/tools via
  `AgentEvent` / `AgentCommand` channels. Use `tokio::select!` in the UI loop.
- Current TUI surface: `NOR`/`INS` modes, multi-session tab bar (`[ + ]`, `alt+1..9`,
  `ctrl+pageup/pagedown`, `alt+left`/`Right`, `ctrl+w`, `gt`/`gT`/`[`/`]`, mouse click), `/`
  slash commands (completion list in `completion.rs`, including `/sessions [id-or-prefix]`,
  `/new`, `/close`, `/agent`/`/skill`), `/sessions` picker modal, `@` Mentions popup (agents,
  skills, files; Tab accepts, Enter accepts without submitting), approval overlay (`y`/Enter, `t`,
  `a`, `n`, `r`, Esc), sidebar with file changes, diff/search modals, transcript replay on resume,
  `ctrl+b` sidebar, `ctrl+t` thinking, `ctrl+c` cancel-then-quit.
- Planned only (do not describe as implemented): `Space` leader menu, Select mode, `:` command
  mode, transcript search, `/rewind`, themes, OSC 52 clipboard fallback.

## Security posture (M11)

- **Landed:** path guards (`resolve_for_read`/`resolve_for_write`) plus sensitive-file path blocks
  (`is_sensitive_path`) and default policy denies (`PolicyEngine::default_sensitive_rules()`), `plan`
  hard read-only, approval gates with headless fail-fast, provider-secret env scrub in
  `code/shell.rs` + `lsp/client.rs`, OS command sandboxing (`ExecutionWorld`: `SeatbeltWorld` on
  macOS, `BubblewrapWorld` on Linux, `HostWorld` fallback with warning or `MEERAI_DISABLE_SANDBOX`),
  OS keychain credential storage (`KeychainSecretStore` with `auth.json` fallback), best-effort
  `redact_run`/`redact_text`, heuristic prompt-injection screening (`screen_run` /
  `screen_untrusted_text`), session `0700`/`0600` permissions, opt-in retention, session/turn token
  budgets, `deny.toml` + `cargo-deny-action` audit in CI, `docs/threat-model.md`, and
  conditional macOS/Windows release signing steps (unsigned until signing secrets exist).
- **M12 definitions are prompt text:** `.agents`/`.claude` agents and skills are untrusted
  repository instructions. They grant no tool, permission, script, MCP, or sandbox capability, and
  `tools`/`permissions`/`allowed-tools` are warning-only. Do not describe discovery as a security
  control.
- **Residual limits (see `docs/threat-model.md`):** Windows uses `HostWorld` and lacks Unix file
  modes; `SeatbeltWorld`/`BubblewrapWorld` allow host-wide reads outside the workspace; `LspToolset`
  is not wrapped in `ExecutionWorld`; redaction and injection screening are regex best-effort;
  release archives remain unsigned when signing secrets are not set. Source of truth:
  `docs/harness-spec.md` §9.6.

## Common review rejections

- Introducing actor frameworks, web/desktop GUIs, or unrelated bot integrations.
- Missing `meerai-core`, `schemars`, or `serde_json` direct dependency in a crate using tool macros.
- Implementing `<Struct>Invoke` in a different module from `#[derive(Toolset)]`.
- Using `schemars` 0.8 attributes/APIs instead of schemars 1.x `#[derive(schemars::JsonSchema)]`.
- Adding `println!` / `eprintln!` in `meerai-agents` or `meerai-tools` (corrupts TUI raw mode).
- Calling `std::thread::sleep` or blocking `std::fs` in hot async paths when async primitives are
  available.
- Panicking (`unwrap()` / `expect()` / `assert!`) on missing user config, missing env vars, or
  malformed model tool arguments (`ToolError::WrongArguments` should be returned instead).
- Modifying `meerai-macros` codegen without updating `insta` snapshots.
- Describing `plan`/`auto` as advisory: `plan` denies non-read effects before rules; `auto` honors
  explicit `ask` and `deny` rules.
- Treating grant resources as globs (they match literally) or removing the TUI `t` turn-grant key.
- Loading `.env` implicitly, bypassing sensitive-file blocks (`is_sensitive_path` /
  `default_sensitive_rules`), or claiming `HostWorld` / Windows runs inside an OS sandbox.
- Claiming heuristic screening/redaction are guarantees, claiming `LspToolset` runs inside
  `ExecutionWorld`, or claiming release binaries are signed when signing secrets are not configured.
  See `docs/threat-model.md`.
- Resolving the workspace to the discovered project root: `find_project_root` locates policy only;
  tools and sessions stay CWD-relative.
- Letting a project `sessions` block affect retention, or pruning the currently active session.
- Keeping project `allow` rules, ignoring the project `permissions` warning path, or letting
  `active_provider`/`active_model`/`providers`/`approval_mode` leak from a project file.
- Retaining the original tool-output secret in a log-only file, redacting paths, or claiming
  redaction is a guarantee.
- Retitling a session that already has a user message, or dropping the resume seed for token
  counters (`set_harness`, `session_usage_from_events`).
- Adding a second definition directory without updating the precedence chain, copying or importing
  definitions, or writing an import lock. Discovery is read-only at the project root and `.meerai`
  wins duplicate ids.
- Applying definition `tools`/`allowed-tools`, honouring `steps` as a primary-agent budget, letting
  a definition `permissions` block carry `allow`, or switching provider from an agent `model` that
  contains `:`.
