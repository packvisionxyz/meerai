# Repository Profile — meerai (Pack Vision)

> Repository knowledge consumed by the skills in `.agents/skills/`. This is **not** a skill and is
> not loaded automatically. Skills read the sections they need.
>
> When this profile conflicts with `AGENTS.md`, `AGENTS.md` wins — update this profile in the same
> commit whenever a change moves an invariant, command, or verification gate.

## 1. Snapshot

- **Product:** `meerai` (Pack Vision) — a pure, provider-agnostic terminal agent harness
  written in Rust, featuring a selection-first modal TUI and a scriptable headless CLI mode.
- **Shape:** Rust Cargo workspace (`edition = "2024"`, toolchain pinned to `1.100.0`). Sixteen
  library crates plus the `meerai` binary, built by `meerai-cli`. Milestones M1–M13 of
  `docs/harness-spec.md` have landed; the repo is distribution-hardened (Apache-2.0
  license, CI, tag-driven releases, Pages site) and includes project-root config discovery, TUI
  transcript replay, opt-in automatic session retention, best-effort tool-output redaction,
  subagent child sessions (`subagent-run`), CLI session forking, retrieval v2/v3 tools
  (`repo_map`, `lsp`, `search_as_code`), security & supply-chain controls (**M11**),
  read-only agent/skill discovery from `.agents`/`.claude` (**M12**), checkpoints/rewind (**M6**)
  v1, and hooks (**M7**) v1. Project hooks require explicit trust through a one-time TTY prompt,
  `meerai hooks trust`, or user settings. M13 is landed for `agy`: `meerai-acp`, `--acp`, `/acp`,
  `acp doctor`, the `ACP:agy` badge and `agy (acp)` header, and `session/load` resume. OpenCode/
  Claude presets and M14 remain backlog.
- **Clients:** `meerai-tui` provides the modal terminal UI library (including replay of an
  existing session selected by `--continue`/`--resume <ID>`/`--fork <ID>` and subagent items);
  `meerai-cli` builds the `meerai` binary with CLI flag parsing, headless one-shot runner
  (`--prompt`/`--print`, `--json`, `--continue`, `--resume <ID>`, `--fork <ID>`, `--agent`,
  `--skill`), and `auth` / `sessions` (`list`, `rm`, `prune`, `fork`) subcommands.
- **Data:** `providers.yml` in the config dir, an optional
  `<project-root>/.meerai/settings.yml` project overlay (root found by walking up from the CWD),
  agent/skill definitions under user-global `~/.config/meerai/{agents,skills}`, trusted packs,
  project `.agents` then `.claude`, user settings and an append-only session log in
  `$XDG_DATA_HOME/meerai/meerai.db` (default `~/.local/share/meerai`) or `MEERAI_DB_PATH`.
- **Runtime stance:** Local-first terminal binary; outbound HTTPS only to configured LLM providers
  (a `genai` 0.7 client over the provider catalog in `meerai-providers`) and an optional update
  check. `ExecutionWorld` provides best-effort Seatbelt/bubblewrap sandboxing with a host fallback.
  No telemetry and no credential service.

## 2. Layout

### Current workspace

| Path | Responsibility |
| :--- | :--- |
| `meerai-core/` | `Toolset` trait (`effect_of`, `effect_of_call`, `invoke_structured`), `ToolOutput`, `ToolError`, `EffectClass`, `ToolObservation`, `ToolRun`, `FileDiff`; re-exports `genai`, `async_trait`, `schemars::JsonSchema` |
| `meerai-macros/` | Proc-macros `#[tool]` and `#[derive(Toolset)]` (`darling` + `syn` + `quote`), `insta` snapshot tests; test-only dependencies are dev-dependencies |
| `meerai-store/` | Embedded SQLite store (`meerai.db`): WAL, FTS5, schema v1, private-file permissions, typed repositories (`config`, `memory`, `packs`, `routes`, `usage`) |
| `meerai-session/` | `Session` append-only `SessionEvent` log in SQLite, recovery, `derive_messages`, replay projection (`replay`), fork/rewind (`latest_closed_turn`, `fork_latest`, `create_child`), SQLite snapshot store (`session_snapshots`), SQLite blob store (`session_blobs`), spill, compaction primitives, session list/delete/prune support |
| `meerai-policy/` | `PolicyEngine`: ordered permission rules, `ApprovalMode`, turn/session grants, YAML parsing |
| `meerai-tools/` | `code` (`CodeTools`, seven `code-*` tools, path guards, unified diffs, `ToolRun` observations, secret env scrub), `repo_map` (`RepoMapToolset`), `lsp` (`LspToolset`; opt-in post-edit diagnostics via `context.post_edit_diagnostics`), `search_as_code` (`SearchAsCodeToolset`) |
| `meerai-common/` | SQLite-backed `AppSettings`, project-root discovery (`find_project_root`) plus project settings load/merge (`load_project_settings`, `merge_project_settings`), markdown agent/skill discovery (`definitions`: `load_definitions`, `load_definitions_for`, user/pack/project sources, diagnostics), hook configuration (`hooks`: `HookEvent`, `HookEntry`, `load_hooks`), declarative packs (`packs`) |
| `meerai-client/` | Client-facing provider surface: `ProviderCatalog` + `providers.yml`, `ModelInfo`, `RuntimeProviderState`, `KeyStatus`, `SecretStore` contract, `MemorySecretStore`, auth listing |
| `meerai-providers/` | `genai` client factory + AEAD `VaultSecretStore` (ChaCha20-Poly1305, OS keychain DEK or Argon2id passphrase); re-exports the client surface |
| `meerai-agents/` | `MultiTurnAgent` turn/step loop, `AgentHarness`/`HarnessConfig` (+ `TokenPricing`), `AgentEvent`, `intercept` (`Interceptor`/`StepPlan`/`ToolPlan`/`ToolContext`/`ToolOutcome`, async `before_tool`), hook engine (`hooks`: `HookEngine`/`HookContext`/`HookReport`, `HookFired` logging), `question-ask` (`QuestionAsked`/`AnswerQuestion`, not an approval), `skill-load` (`skill_tool`), `prompt`, `spawn_worker`, `subagent` runtime (`SubagentToolset`, built-ins + custom child runs filtered by `mode` and tightened by definition `permissions`, `child_session_id` on `SubagentRun`), delegated `acp-run` (`acp_subagent`: `AcpRunToolset`/`AcpSpawner`), `ApplyAgent`/`ApplySkill` instruction application, tool-output redaction (`redact.rs`) |
| `meerai-host/` | UI-neutral host seams (`SessionOpener`, `OpenedSession`, `WorkspaceQuery`/`LocalWorkspace`, `WebServerLauncher`), shared `diff`/`search`/`syntax`/`theme`/`update`, and live `SessionRegistry` event pump/ring |
| `meerai-web-protocol/` | Serde-only Wasm-safe wire DTOs (`SnapshotResponse`, `ViewEvent`, `HostInfoDto`, `ProvidersResponse`, `SettingsSnapshotDto`, `ApiError`) |
| `meerai-web/` | Loopback HTTP/SSE server (`hyper` + `subtle` Basic auth, `Host`/`Origin`/JSON checks, parity REST/SSE routes, embedded Wasm bundle serving) |
| `meerai-web-client/` | Leptos 0.8 CSR Wasm browser client ([browser-ui.md](../../docs/browser-ui.md)): DOM transcript, Markdown + code highlighting, left Sessions sidebar plus TUI modal surfaces, approval scopes, Settings, palette actions, `@` mentions, completions, a collapsible sidebar Diagnose block (cache hit/write, context, memory), built via `scripts/build-web.py` |
| `meerai-tui/` | Modal TUI library (`ratatui` + `crossterm`) split into `ui/` panels and `app/` modules, transcript replay, `/serve`, tabs, collapsible digests and sidebar Diagnose block, turn-anchor rail, diff/search, re-exporting `meerai-host` seams (no `meerai-tools`, `meerai-providers`, or `meerai-web` dependency) |
| `meerai-syntax/` | Tree-sitter highlighting for Rust, Python, JavaScript, TypeScript/TSX, Go, Bash, HTML, CSS, and JSON plus a lexical fallback; `highlight_block` returns per-line `Token`s; depends only on grammar crates |
| `meerai-cli/` | Binary crate (`meerai`): CLI flags (`--fork`), headless runner, `auth`/`sessions` (`list`, `rm`, `prune`, `fork`) / `serve` (feature `web`) subcommands, and TUI launcher |
| `.agents/agents/`, `.agents/skills/` | Default agent (`*.md`) and skill (`*/SKILL.md`) definitions, read at runtime |
| `.claude/agents/`, `.claude/skills/` | Lower-priority agent and skill definitions, same format |
| `docs/` | Architecture, harness specification (`docs/harness-spec.md`), and modal TUI spec |
| `.github/workflows/` | `ci.yml`, `release.yml`, `pages.yml` |
| `site/` | Landing page plus `install.sh` / `install.ps1` served by GitHub Pages |
| `LICENSE`, `Cargo.toml` | Apache-2.0; workspace metadata (`license`, `repository`, `homepage`, `rust-version = 1.100`, `publish = false`) |
| `AGENTS.md` | Working rules for coding agents (commands, contracts, loop, config, security, TUI, commit, code style); injected into the system prompt, so it stays rules-only |
| `docs/harness-internals.md` | Implementation reference: crate layout, macro conventions, configuration, loop internals, tools/policy, ACP traps, security posture |

### ACP (see `docs/acp.md`)

- `meerai-acp/` — ACP client bridge: `AGY` preset, `connect`/`connect_with`, `AcpHandshake`,
  typed `AcpEvent` stream, `AcpWorker` (`AgentCommand`/`AgentEvent` + session log), permission
  replies, cancel, `session/load`, stderr tail, ignored live tests. The CLI/TUI depend on it.
  OpenCode/Claude presets and M14 remain backlog.

## 3. Commands

| Task | Command |
| :--- | :--- |
| Bootstrap | Rustup installs toolchain `1.100.0` automatically via `rust-toolchain.toml` |
| Install / build | `cargo build --workspace` |
| Install binary | `cargo install --path meerai-cli --bin meerai` |
| Dev | `cargo check --workspace --all-targets` |
| Format | `cargo fmt --all` (check only: `cargo fmt --all -- --check`) |
| Lint | `cargo clippy --workspace --all-targets` |
| Test (workspace) | `cargo test --workspace` |
| Test (single file) | `cargo test -p meerai-macros --test derive_toolset_pass` |
| Test (single test) | `cargo test -p meerai-macros test_snapshot` |
| Update macro snapshots | `cargo insta test --review` or `INSTA_UPDATE=always cargo test -p meerai-macros` |
| Headless surface | `cargo check -p meerai-cli --no-default-features` |
| TUI without update | `cargo check -p meerai-tui --no-default-features` |
| Local gate | `make verify` (alias of `make ci`) |
| CI | `.github/workflows/ci.yml` on push/PR to `main`: fmt check, clippy, tests on `ubuntu-24.04` |
| Release | `.github/workflows/release.yml` after CI succeeds on `main`, on `v*` tags, or on dispatch: tag/version detect, build 5 targets, GitHub Release |
| Deploy site | `.github/workflows/pages.yml` on `site/**` changes: deploy `site/` to GitHub Pages |

Notes:

- Verify order is always `fmt -> clippy -> test`. CI enforces the same gate; a red CI run is
  authoritative.
- `meerai-core` sets `doctest = false` in `meerai-core/Cargo.toml`.
- Two clippy warnings are accepted pre-existing noise: `assert!(true)` in `meerai-core/src/tools.rs`
  (tests) and the redundant `&` in `meerai-macros/src/test_utils.rs`. Do not treat them as new
  breakage.
- CI has a `cargo-deny` dependency-audit job (`deny.toml`), and no Windows runner.
- Release workflows include secret-gated signing steps (`APPLE_CERTIFICATE` / `WINDOWS_CERTIFICATE`); without configured secrets, archives publish unsigned.

## 4. Invariants

1. **Scope discipline:** Keep the workspace focused on the local agent harness and local browser companion (`web` feature). Do not introduce a desktop GUI, remote browser hosting, actor frameworks, or unrelated bot integrations.
2. **Macro dependency contract:** Proc-macros emit paths to `meerai_core`, `schemars`, and
   `serde_json`; every crate using `#[tool]` or `#[derive(Toolset)]` must list all three as direct
   dependencies.
3. **Private `Invoke` trait:** `#[derive(Toolset)]` generates a module-private `<Struct>Invoke`
   trait; `impl <Struct>Invoke for <Struct>` must live in the same module. `CodeTools` is the
   exception: it hand-writes `impl Toolset`.
4. **Schema version:** Tool parameter types derive `JsonSchema` from `schemars` 1.x (workspace
   `schemars = "1"`, locked at `1.2.3`), not 0.8. `meerai-macros` exports only `#[tool]` and
   `#[derive(Toolset)]`; use `#[derive(schemars::JsonSchema)]`. Tool schemas use
   `SchemaSettings::draft07()` and a `None` meta-schema.
5. **Tool naming:** Runtime tool-call names are always `"<toolset_snake>-<tool_snake>"`. The built-in
   coding tools are `code-read_file`, `code-write_file`, `code-edit_file`, `code-list_dir`,
   `code-grep`, `code-glob`, `code-shell`.
6. **Wire contracts:** `AgentEvent` (`meerai-agents/src/events.rs`) and the `SessionEvent`
   vocabulary (`meerai-session/src/event.rs`) plus its id newtypes connect the agent loop, the TUI,
   the headless NDJSON stream, and the persisted session log. Nothing is frozen before the first
   release: change variants and fields freely, but update every consumer and replay path in the
   same change.
7. **Serde tagging:** Use adjacent tagging (`tag = "type", content = "data"`) for enums with
   string/newtype variants. Internally tagged enums (`tag` only) cannot represent bare strings.
8. **TUI-safe library output:** Do not add `println!` / `eprintln!` progress output in library
   crates; the agent loop emits `AgentEvent`. Startup diagnostics (`settings` warnings, project
   config warnings, prune reports) are printed by `meerai-cli/src/main.rs`, the binary.
9. **Async hygiene:** No blocking sleeps (`std::thread::sleep`) inside `async fn`; use `tokio::time`.
   Async file paths use `tokio::fs`; `BlobStore::put` uses blocking `std::fs` and is called through
   `tokio::task::spawn_blocking` in `apply_spill`.
10. **Secrets handling:** API keys and ChatGPT plan OAuth credentials are encrypted in
    `vault_secrets` by the AEAD vault (ChaCha20-Poly1305, fresh nonce per write, provider id as
    AAD). The 256-bit DEK lives in the OS keychain under a per-database
    `vault-master-key-v1-<id>` account; on macOS it is read and written through
    `/usr/bin/security`, so the keychain grant survives every rebuild and upgrade but covers the
    tool, not meerai: it is not a per-application boundary. Without a keychain,
    `MEERAI_VAULT_PASSPHRASE` derives the DEK with Argon2id and the vault fails closed. Never switch
    an initialized vault's key source, never write plaintext, and never log a key or token. OAuth refresh is one `BEGIN IMMEDIATE`
    transaction. `list_providers`/`auth list` never print a token. The shell tool removes provider
    secret variables from child processes. Tool output and
    diffs pass through best-effort pattern redaction (`redact_run`) before spill, log, UI, or model
    delivery; the original secret is discarded and paths are not redacted. This is not a guarantee.
11. **Terminal safety:** `TerminalGuard` restores raw mode / alternate screen in `Drop` and installs
    a panic hook; keep both paths intact when touching `meerai-tui/src/terminal.rs`.
12. **Permission modes:** `plan` is hard read-only — writes, execute, and external effects are
    denied before rules and grants. `auto` allows every tier default and still honors explicit
    `ask` and `deny` rules. `ask` keeps tier defaults.
13. **Approval scope:** `AcceptForSession` always records a Session grant; `scope: Some(Turn)` is
    sent by the TUI `t` key and cleared at turn end. `y`/Enter sends `scope: None`; `a` sends
    `AcceptForSession`. Grant resources match literally; rule resources use globs.
14. **Headless contract:** Tool-call approvals fail fast with exit `2` and a `--mode` hint. Spend
    approvals print the `context.max_session_tokens` remedy, decline, and exit `1`. Completed is
    `0`; other failures are `1`.
15. **Session privacy:** Session directories are `0700`, session files `0600` on Unix; metadata
    records cwd, git state, title, model, and timestamps. Windows has no mode enforcement.
    `sessions prune` is the manual retention surface; `sessions.auto_prune_days` (user config only)
    opts into startup pruning with `sessions.keep` (default 0), always sparing the active session.
    Startup also removes empty sessions (no user message, no title, no parent) older than six hours
    that are not the active session or an open tab. A plain TUI start opens one empty session tab,
    restores saved tabs in the background, and reuses the remembered empty session; closing an
    empty tab keeps it for reuse, and `/new` or `[ + ]` switches to an open empty tab instead of
    opening a second one.
16. **Env opt-in:** `.env` loads only with `MEERAI_LOAD_DOTENV=1|true|yes` or `--env-file <PATH>`.
17. **Project permissions are tighten-only; project config cannot redirect:**
    `<project-root>/.meerai/settings.yml` merges over user settings. `find_project_root` walks up
    from the process CWD to the nearest `.git` (directory or file) or `.meerai/settings.yml`, else
    the CWD is used; tools and sessions still use the CWD as the workspace. Project `permissions`
    merge as `ask` rules → user rules → `deny` rules. `allow` is dropped with a warning, a project
    `ask` cannot downgrade a user `deny`, and a project `deny` always adds a denial after user
    rules. Invalid shapes drop the block with a warning. Project `active_provider`, `active_model`,
    `providers`, `approval_mode`, and `sessions` are ignored with a warning. Project `context`
    overrides field by field, with `max_session_tokens` and `max_turn_secs` clamped so a project can
    only tighten them. A malformed file warns and is ignored.
18. **Redaction call site:** `redact_run` runs in `meerai-agents` immediately after a tool run and
    before spill, `SessionEvent::ToolResult`, `ItemCompleted`, or model text. It covers observation
    stdout/stderr/hint, `ToolOutput` text, and `FileChange` diffs; dispatch error text goes through
    `redact_text`. Never retain the original secret in a log-only file.
19. **Session replay:** `--continue`/`--resume <ID>` without `--prompt` open the session and replay
    `meerai_session::replay::replay_from_events` (user/steering, assistant text/reasoning, tool
    calls/results, compaction markers); runtime/system events are skipped and unmatched tool calls
    become `Cancelled`. The TUI converts `ReplayEntry` into its `TranscriptEntry` view.
    `set_harness` and `session_usage_from_events` seed token counters from the last logged
    `UsageUpdated`. A fork copies the prefix but zeroes inherited usage fields, so the child
    starts with empty token and cache counters. `AgentCommand` has `Rewind` and no
    `Revert`/`Fork` variant; conversation shadowing is internal.
20. **Security roadmap (M11):** landed — path guards, sensitive-file blocks (`is_sensitive_path` and
    `PolicyEngine::default_sensitive_rules`), `plan` hard read-only, approval gates, headless
    fail-fast, provider-secret env scrub, best-effort output/diff/error redaction, heuristic
    prompt-injection screening (`screen_run`/`screen_untrusted_text`), session file permissions,
    configurable retention, session/turn token budgets, `ExecutionWorld` OS sandbox (`SeatbeltWorld` /
    `BubblewrapWorld` with `HostWorld` fallback warning), AEAD credential vault (OS keychain DEK or
    Argon2id passphrase, fail-closed), `cargo-deny` CI audit (`deny.toml`), conditional release signing steps,
    and `docs/threat-model.md`. Caveats: signing secrets are operator-configured, and Windows uses
    Phase-1 `HostWorld` guards (see `docs/threat-model.md`).
21. **Agent/skill definitions are untrusted text (M12):** discovery is read-only and
    starts at the project root. `.meerai/agents/*.md` and `.meerai/skills/*/SKILL.md` load first,
    then `.agents/...`, then `.claude/...`; a duplicate id keeps the higher source and the warning
    names both paths; the catalog is sorted by id. Nothing is copied and there is no import lock.
    Frontmatter is `name`, `description`, `model`, `steps`, `mode`, `permissions`, and
    `disable-model-invocation`. `mode` is `all`, `primary`, or `subagent`: primary-only definitions
    are hidden from `--agent` and the pickers, subagent-only definitions are hidden from those and
    offered to `subagent-run`. `permissions` are applied to a subagent's policy after the parent
    rules, and `allow` entries are dropped with a warning so a definition tightens only.
    `tools`/`allowed-tools` warn and are not applied; invalid frontmatter warns and skips.
    Missing frontmatter uses the file or directory name and the body. Skills with a
    description and no `disable-model-invocation` join a screened 8 KiB prompt index; the read-only
    `skill-load` tool returns `SKILL.md` or one listed supporting file inside the skill directory
    (16 KiB cap), rejecting separators and symlinks. Definitions grant no tools, scripts, or
    sandbox, and a `:` in an agent `model` is ignored (no provider switch). Startup
    `--agent` takes one id (repeating is an error); `--skill` is repeatable; unknown ids fail with
    the available ids. The TUI exposes `/agent`/`/skill`, `/agents`/`/skills` pickers, and a `@`
    Mentions popup (agents, skills, files; ids colored teal for agents and blue for skills; Tab
    accepts, Enter accepts without submitting, Esc dismisses).
22. **ACP client (M13, agy):** `--acp` drives `agy acp`; `/acp` opens a new tab; `acp doctor`
    prints the handshake. `AcpWorker` maps `AcpEvent` to `AgentEvent` and `SessionEvent`, sets
    `harness: acp:agy`/`remote_session_id`, and uses `session/load` for resume. Pre-`Connected`
    replay updates are drained. `ApprovalKind::ExternalAgent` forwards a decision that is advisory.
    ACP does not consult `PolicyEngine`, grants, path guards, redaction, or `ExecutionWorld`;
    `plan` refuses `--acp`. The top-level native agent can call `acp-run` to delegate one task in a
    child session; it forwards permissions, emits `SubagentRun`, returns the report, defaults to a
    600 s wall-clock cap, and has no token/grant rollup. `acp-run` is not registered on native
    subagents or ACP children. Never describe ACP as a safety boundary. OpenCode/Claude presets and
    M14 are backlog.
23. **Provider reasoning signatures:** `SessionEvent::AssistantMessage.call_signatures` is
    index-aligned with `tool_calls`, `#[serde(default)]`, and skipped when every entry is `None`, so
    old logs stay valid and untouched messages stay clean. `turn.rs` captures the full
    `genai::chat::ToolCall.thought_signatures` vector (and fills it from the captured call when a
    streamed chunk carried none); `derive_messages` restores it; never zero a stored signature.
    Gemini 2.5/3 reject a function-call loop without them. A provider switch is safe: genai drops
    unpaired signatures instead of sending them to a provider that cannot use them.
24. **Crash healing:** `Session::open_inner` appends one error `ToolResult`
    (`tool execution interrupted before completion`) for every call in the last visible assistant
    message that has no logged result, before closing the open turn. It skips rewound and compacted
    ranges and never repeats on later opens. A turn that already ended with unexecuted calls (loop
    detection, cancel during planning) is healed after its `TurnEnded`, so a fork does not copy that
    repair. `Session::load_events` is the read-only path (TUI child refresh) and must never heal; do
    not call `Session::open` from a read path. Do not heal at projection time; the log must show the
    repair. `run_turn` also calls the same idempotent `Session::heal_dangling_tool_calls` after
    `TurnEnded`, so the live process never sends an unpaired call.
25. **Provider cache behavior:** `context.prompt_caching` (default true) sets a request-level
    `CacheControl::Ephemeral` on every model step; `Usage` copies provider cached and cache-write
    token counts, and the headless summary and TUI show cache reads. Pricing stays token-only; do not
    invent a cache discount.
26. **Projection clearing and compaction:** `ProjectionPolicy::clear_old_tool_results` (default true)
    stubs tool results outside `verbatim_window_turns`; the newest window stays verbatim, a spilled
    stub names its blob; the same-model summarizer keeps the parent clearing policy and a distinct
    summarizer model disables clearing. `run_compaction` returns
    `Result<bool, TurnError>`; pressure compaction applies at most twice per turn and once per step,
    and the planner falls back to balanced `StepEnded` boundaries. With no `summarizer_model`, the
    summary request reuses the parent system, tools, and history. Do not clear at log-write time or
    delete a result.
27. **No-progress breaker:** `NoProgress` logs `CircuitBreaker` at 8 tool steps without a file
    change and fails the turn at 12. It is disabled in `plan` mode and when no write tool is
    registered; a `subagent-run`/`acp-run` delegation resets the counter. Do not count read-only
    exploration as failure.
28. **Recovery turn:** the worker queues one recovery turn after a retryable provider or
    `LoopDetected` failure. The prompt is user-visible and names the failure. It runs before the
    next queued prompt, at most once per failure streak; budget, config, tool, and non-retryable
    provider failures do not recover. Do not recover in a read-only or headless one-shot path
    without an explicit design; the worker path is the v1 surface.
29. **Provider retry classification:** use the HTTP status from `genai::Error::status()` when
    present (408/409/425/429 and 5xx transient; other 4xx permanent) and the message blocklist only
    as a fallback. Backoff is 250/500 ms plus up to 50% jitter. Do not retry a permanent status. An
    empty stream reports provider, model, stop reason, event count, elapsed time, and a hint; a
    `content_filter`, `max_tokens`, or `stop_sequence` stop reason is non-retryable.
30. **Delegated turn budget:** a `subagent-run` child gets the parent's remaining turn deadline
    minus a 5 s margin, with a 1 s minimum (`child_turn_secs`), so the child fails its own budget
    before the parent cancels it. The parent bridge deadline stays as the backstop. Do not set the
    child clock to `0` while the parent is bounded.
31. **Step-budget continuations:** a turn gets `max_steps_per_turn` plus
    `step_limit_continuations` extra budgets (default 32 + two x 32 = 96).
    Each boundary logs `STEP_CONTINUE_NOTICE` and keeps tools; only the
    absolute final step logs `STEP_LIMIT_NOTICE` and strips tools. Never let
    a subagent inherit continuations: its preset cap is a contract, so set
    `step_limit_continuations = 0` on the child harness.
32. **Host actions:** `meerai_host::parse_host_action` and
    `execute_host_action` are the single implementation of `/context`,
    `/memory`, `/remember`, `/forget`, `/pin`, `/routing`, `/packs`, `/pack`,
    `/definitions`, `/subagent`, and `/open`. The TUI dispatches them through
    `PendingSessionAction::HostAction`; the web server dispatches
    `POST /api/actions`. Do not fork the parsing or formatting into a UI.
33. **Context telemetry:** `ContextBreakdown` is emitted after each step and
    after compaction, stored on the TUI tab and the host `SessionRecord`, and
    rebroadcast as `RegistryEventKind::ContextUpdated` /
    `ViewEvent::ContextUpdated`. Keep pinned turns in
    `ProjectionPolicy::pinned_turns` and persist them as
    `SessionEvent::ContextTurnPinned`.
34. **Project memory:** `MemoryToolset` is the only writer of `project_memories`
    from the model; `memory-search` is Read and save/delete are Write, so plan
    mode excludes them. `MemoryCapture` runs only when
    `HarnessConfig::auto_memory` is true and writes one Progress row plus one
    Decision row per key decision from a compaction summary. The prompt block
    is read through `load_memory_prompt` and appended by
    `compose_system_prompt_with_memory`; keep it bounded and screened.
35. **Vault selection:** `default_secret_store` reads the stored envelope mode
    before choosing a key source. A keychain envelope always uses the keychain
    backend; a passphrase envelope always uses `MEERAI_VAULT_PASSPHRASE`. Only
    an uninitialized vault may choose the keychain automatically. Never switch
    an initialized vault's key source. A missing key is not auto-reinitialized:
    the operator must run `meerai auth reset-vault`, which deletes all
    ciphertext in one transaction and creates a new key.
36. **Command panels:** every user-facing command gets a dedicated panel or
    modal in both clients. Do not add a transcript-only command, and do not
    require a typed confirmation argument in one client while the other uses a
    panel. A destructive flow uses the same explicit confirmation in both.
37. **UI panels:** the TUI exposes `ActiveModal::{Context, Memory, Routing,
    Packs, SubagentSpawn}`; the browser exposes one `WorkspacePanel` modal with
    tabs. Both read the same `meerai-host` data methods and send the same host
    actions. Do not add a workflow to one client alone.

## 5. Patterns

Canonical implementations live in [`patterns.md`](patterns.md). Summary:

| Layer | Canonical shape | Reference |
| :--- | :--- | :--- |
| LLM Provider | `genai::Client` builder + auth resolver + `RuntimeProviderState` | `meerai-common/src/providers.rs` |
| Toolset macro | `#[derive(Toolset)]` struct + same-module `impl <Struct>Invoke` | `meerai-macros/tests/derive_toolset_pass.rs` |
| Free-fn tool | `#[tool(...)] async fn` generating `<PascalName>Toolset` | `meerai-macros/src/tool/attribute_tool.rs` |
| Hand-written toolset | `impl Toolset` with `definition`/`contain`/`effect_of`/`invoke_structured` | `meerai-tools/src/code/mod.rs` |
| Config | `AppSettings` + `find_project_root` + `load_settings_from` + `load_project_settings`/`merge_project_settings` + `${VAR\|default}` expansion | `meerai-common/src/settings.rs` |
| Definitions | `load_definitions(&project_root)` → `DefinitionCatalog`; `.meerai` before `.agents` before `.claude`, duplicate warning, sorted by id | `meerai-common/src/definitions.rs` |
| Secrets | AEAD vault (`vault_secrets`) for API keys and ChatGPT plan OAuth, OS keychain DEK (macOS through `/usr/bin/security`, not a per-app boundary) or `MEERAI_VAULT_PASSPHRASE`, no plaintext fallback | `meerai-providers/src/secrets/vault.rs`, `meerai-store/src/v1.sql` |
| Redaction | `redact_run` patterns and `[REDACTED:<kind>]` replacement before settle | `meerai-agents/src/redact.rs`, `multi_turn_agent.rs` |
| Session log | `Session::append` event, `derive_messages` projection | `meerai-session/src/session.rs`, `projection.rs` |
| Session CLI | `sessions list`/`rm`/`prune`, `--continue`/`--resume`, `replay_from_events`, opt-in `auto_prune_sessions` | `meerai-session/src/replay.rs`, `session.rs`, `meerai-cli/src/main.rs` |
| Policy decision | `PolicyEngine::decide` + `ApprovalMode` tier defaults + grants | `meerai-policy/src/engine.rs` |
| Coding tools | `CodeTools` `Toolset` impl + `resolve_for_read`/`resolve_for_write` guards | `meerai-tools/src/code/mod.rs`, `code/paths.rs` |
| Prompt | `BASE_SYSTEM_PROMPT` + `compose_system_prompt` + scoped `AGENTS.md` injection | `meerai-agents/src/prompt.rs`, `multi_turn_agent.rs` |
| Agent loop | item protocol + `AgentHarness` (session dual-write, `derive_messages` requests, policy + approvals, spill, compaction, budgets, breakers) | `meerai-agents/src/multi_turn_agent/`, `harness.rs` |
| Loop extension | `Interceptor` chain (`before_step`, async `before_tool`, `after_tool`, `turn_stopping`) registered on the agent; `CheckpointInterceptor` is the built-in pre-tool capture | `meerai-agents/src/intercept.rs`, `checkpoint.rs` |
| Hooks | `HookEngine` + `HookContext` + `HookReport`; loader in `meerai-common::hooks`; trust gate in `meerai-cli`; live `AgentEvent::HookCompleted` to the TUI/headless | `meerai-agents/src/hooks.rs`, `meerai-agents/src/events.rs`, `meerai-common/src/hooks.rs`, `meerai-cli/src/main.rs` |
| ACP delegation | `AcpRunToolset` (`acp-run`) uses `SUBAGENT_BRIDGE`, creates a child session, forwards permissions, emits `SubagentRun`, returns the report; `AcpSpawner` is injected by the CLI | `meerai-agents/src/acp_subagent.rs`, `meerai-cli/src/main.rs` |
| User questions | `question-ask` tool + `QuestionAsked`/`QuestionResolved` events + `AnswerQuestion`; TUI overlay; headless auto-cancel | `meerai-agents/src/question.rs`, `meerai-tui/src/app/modals.rs` |
| Rewind | capture before write tools, `Rewind { turn_id, scope }`, `/rewind` menu, one `RewindApplied` event | `meerai-session/src/snapshot.rs`, `meerai-agents/src/multi_turn_agent/tools.rs`, `meerai-tui/src/app/modals.rs` |
| TUI overlay | `ActiveModal` variant + `handle_agent_event` queue + key handler | `meerai-tui/src/app.rs` (`ActiveModal::Approval`) |
| ACP bridge | `connect_with` spawns `AcpAgent`, handshake/`session/load`, typed `AcpEvent`, `AcpWorker` to `AgentCommand`/`AgentEvent`, permission oneshot replies, cancel | `meerai-acp/src/{client,events,worker}.rs`, `meerai-cli/src/{acp_cmd,main,headless}.rs`, `meerai-tui/src/{lib,app,ui}` |

## 6. Generated and vendored paths

- `meerai-macros/src/tool/snapshots/` — `insta` macro-expansion snapshots (pretty-printed with
  `prettyplease`). Regenerate via `cargo insta test --review` or `INSTA_UPDATE=always`; do not
  hand-edit snapshot expectations without verifying macro output.
- `target/` — Cargo build artifacts (gitignored).

## 7. Verification matrix

| Touched | Required gate |
| :--- | :--- |
| `meerai-core` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-core` |
| `meerai-macros` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-macros` |
| `meerai-common` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-common` |
| `meerai-session` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-session` |
| `meerai-policy` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-policy` |
| `meerai-tools` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-tools` |
| `meerai-agents` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-agents` |
| `meerai-tui` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-tui` |
| `meerai-host` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-host` |
| `meerai-web-protocol` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-web-protocol` |
| `meerai-web` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-web` |
| `meerai-web-client` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-web-client`, `cargo check -p meerai-web-client --target wasm32-unknown-unknown`, `python3 scripts/build-web.py` |
| `meerai-syntax` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-syntax` |
| `meerai-acp` | `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test -p meerai-acp`; live `cargo test -p meerai-acp --test live_agy -- --ignored` and `--test live_worker -- --ignored` |
| Repo-wide | `cargo fmt --all -- --check && cargo clippy --workspace --all-targets && cargo test --workspace` (or `make verify`) |
| Docs only | Re-run the repo-wide check when code claims changed; otherwise verify links and the stale-claim grep |

Coverage caveats:

- CI runs the repo-wide gate on `ubuntu-24.04` only. macOS and Windows code paths are compiled and
  unit-tested there, but not exercised on those hosts (including `cmd /C` and Windows session ids).
- `meerai-core` disables doctests (`doctest = false`).
- `meerai-tui` tests render with ratatui's `TestBackend`; no live-terminal test exists.
- The headless path is not smoked against a live provider in CI; provider calls need real keys.
- `meerai-macros` keeps `meerai-core`, `schemars`, `serde`, `serde_json`, `insta`, `prettyplease`,
  and `futures-test` as `[dev-dependencies]`, so they are test-only.

## 8. Documentation conventions

All `docs/` documents follow the standard in `docs/_template.md`: a header
table with a verification date, then Overview → Implemented (description,
behavior, limits, owning module) → Planned (item, description, gate) →
optional Reference / Invariants / Open questions. Every Implemented claim
is verified against the code before writing; plans never use present
tense; one owning document per fact; no stale delivery history in the
body.

- `README.md` — project overview, install, quick start, current vs. planned crates, usage, exit
  codes, security and limits, local dev commands.
- `AGENTS.md` — working rules for AI agents: commands, contracts, loop, config, security, TUI,
  commit, and code style. It is injected into the system prompt, so keep it rules-only.
- `docs/_template.md` — document standard, template, ownership map, and rewrite checklist.
- `docs/harness-internals.md` — implementation reference: crate layout, tool macro conventions,
  configuration, turn-loop internals, TUI pointers, known traps, and security posture summary.
- `docs/harness-spec.md` — unified harness specification: design principles, session log, item
  protocol, context engine, permissions, safety nets, agents/skills/subagents, retrieval, the
  M1–M14 milestone table (§4.1), and recorded decisions.
- `docs/architecture.md` — crate layout, dependency direction, build surfaces, protocol and
  configuration summaries, planned architecture items.
- `docs/tui-design.md` — modal TUI behavior: layout, modes, keybindings, overlays, rendering
  invariants, dialog width caps, and planned UX.
- `docs/browser-ui.md` — local browser companion: `meerai serve` / TUI `/serve`,
  `SessionLock`, host design, parity API, the implemented Leptos DOM client, build, and
  security rules.
- `docs/acp.md` — ACP protocol reference, the implemented `agy` client and `acp-run`, trust
  boundary, planned presets and M14.
- `docs/threat-model.md` — assets, trust boundaries, implemented controls, planned controls,
  residual risks.
- `docs/token-efficiency.md` — token-cost and self-healing initiative: implemented phases with
  evidence, decisions, planned phases.
- `docs/openai-chatgpt-plan-usage.md` — OpenAI ChatGPT plan usage (M15, planned): credential
  model, Sign in with ChatGPT flow, plan request profile, error map, phases.
- `.agents/repo/` — repository profile, patterns, and infra notes for `.agents/skills/*`. Keep in
  sync when crates, commands, or invariants change.

## 9. Infrastructure and deploy

Detail in [`infra.md`](infra.md). Summary:

- **CI:** `.github/workflows/ci.yml` runs fmt/clippy/test on push and pull request to `main`, and
  on `workflow_dispatch`.
- **Release:** `.github/workflows/release.yml` runs after CI succeeds on `main`, on `v*` tags, or
  on manual dispatch. `detect` derives the tag from `[workspace.package] version`, blocks all
  artifacts on a mismatch, and skips a version that is already released; five targets build and
  archive with SHA256, and `publish` creates the GitHub Release at the built commit.
- **Site:** `.github/workflows/pages.yml` deploys `site/` (installers and landing page) to GitHub
  Pages at `meerai.packvision.xyz`.
- **Local:** all checks are also runnable with the pinned Rust `1.100.0` toolchain via `make verify`.
- **No IaC, containers, or self-hosted runners.** Signing secrets are not pre-configured in GitHub
  Actions (release signing steps are secret-gated); CI runs `cargo-deny` (`deny.toml`).

## 10. Domain, personas, and gating

- **Personas:**
  - *Terminal developer*: runs `meerai` for the modal TUI, or headlessly
    (`meerai --prompt "..."`, optionally `--json`, `--continue`, `--resume`) in scripts/CI.
  - *Harness/tool author*: extends `meerai-core`, `meerai-tools`, and `meerai-agents` using
    `#[derive(Toolset)]` (or a hand-written `impl Toolset`) and the `Toolset` trait.
- **Glossary:**
  - `Toolset` — async trait exposing `ToolDefinition` schemas, `effect_of`, and
    `invoke`/`invoke_structured`.
  - `ToolOutput::{Text, Fail, Stop}` — tool outcome (`Fail` feeds error back to LLM; `Stop` ends
    the turn).
  - `SessionEvent` — append-only durable log event; `derive_messages` is the pure projection into
    the model transcript.
  - `AgentEvent` — structured item-protocol stream (`TurnStarted` → `ItemStarted`/`ItemDelta` →
    `ItemCompleted` → `TurnCompleted`) that decouples the agent loop from the TUI and headless printer.
  - `ApprovalMode` — tool-execution policy preset (`plan | ask | auto`).
  - `EffectClass` — tool effect (`read | write | execute | external`) used for policy tiers.
- **Entitlement gating:** None (local CLI/TUI harness; model access is governed by user-supplied
  provider API keys).

## 11. Frozen decisions

- Rust edition `2024`, pinned toolchain `1.100.0` (`rust-toolchain.toml`), `rust-version = "1.100"`,
  `max_width = 100` (`rustfmt.toml`).
- Apache-2.0 license (`LICENSE`, `[workspace.package] license`); `publish = false`; repository and
  homepage metadata point at `packvisionxyz/meerai` and `meerai.packvision.xyz`.
- `schemars` 1.x for `JsonSchema` derivation on tool arguments (`schemars = "1"`, locked `1.2.3`).
- Runtime tool-call naming `"<toolset_snake>-<tool_snake>"`; built-in coding tools use `code-*`.
- `AgentEvent` item protocol (`docs/harness-spec.md` §2.3): `TurnStarted`/`ItemStarted`/
  `ItemDelta`/`ItemCompleted`/`TurnCompleted`, with authoritative `ItemCompleted` payloads.
- Adjacent serde tagging (`tag = "type", content = "data"`) for enums that carry string/newtype
  variants; internally tagged enums cannot hold bare strings.
- Headless CLI flags `--prompt`/`--print`, `--json`, `--continue`, `--resume <ID>`, `--env-file`;
  `-p`/`--provider`, `-m`/`--model`, `-V`/`--version` keep their existing meaning. Resume works
  with or without `--prompt`; without it the TUI starts and replays the transcript.
- Headless exit codes: `0` Completed, `1` failure (including a declined Spend approval), `2`
  tool-call approval required (fail fast, no prompt).
- Approval modes: `plan` hard read-only (denies non-Read even with allow rules), `auto` allows all
  tier defaults while honoring explicit `ask` and `deny` rules, `ask` keeps tier defaults.
  `AcceptForSession` → Session grant; `Approve.scope = Some(Turn)` comes from the TUI `t` key;
  grant resources are literal.
- Project permissions are tighten-only; project config cannot redirect.
  `<project-root>/.meerai/settings.yml` merges over user settings, discovered by `find_project_root`
  walking up from the CWD to `.git`/`.meerai/settings.yml`. Project
  `permissions` merge as `ask` → user → `deny`; `allow` is dropped, project `ask` cannot downgrade
  a user `deny`, and project `deny` always adds a denial after user rules. `context` overrides
  field by field with `max_session_tokens`/`max_turn_secs` clamped to tighten only, and
  provider/model/mode/`sessions` fields are ignored with a warning. A malformed file warns and is
  ignored.
- Redaction: best-effort regex redaction of known secret shapes before spill/log/UI/model; the
  original secret is discarded, diffs are redacted, paths are not. No entropy screen and no
  guarantee. Cost tracking is token-only by default: `est_cost_usd` is computed from
  `ProviderCatalog` model pricing when both per-million prices exist; the bundled catalog ships
  context windows only.
- Rewind only: the TUI sends `RewindScope::Both` (message menu and `/rewind` default);
  `Conversation` is an internal scope; `Code` restores snapshot blobs; `Both` runs code then
  conversation. `Session::rewind` appends one `RewindApplied` event and never a `RevertApplied`
  event. Capture happens before write-class tools only; shell edits, gitignored
  files, symlinks, and files over 2 MiB are not restored.
- Questions are not approvals. `question-ask` is read-classified, runs before the rest of the
  batch, uses `QuestionAsked`/`AnswerQuestion`, records no grant, and is not registered for
  subagents. Headless auto-cancels with a hint.
- Loop extension: M6 checkpoint capture is the built-in `CheckpointInterceptor` on async
  `before_tool`; do not add branches to `turn.rs`. M7 hooks use the separate `HookEngine`, not the
  interceptor chain, because they await host processes at lifecycle points outside the loop.
  `SessionEvent::HookFired` keeps its `event`/`decision` strings and adds `detail: Option<HookDetail>`.
- Hooks: user hooks load from `<config-dir>/hooks.json`; project hooks from
  `<project-root>/.meerai/hooks.json` only with `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, user
  `hooks.trust_project: true`, or a canonical path in user `hooks.trusted_projects`.
  `meerai hooks trust|untrust|status` manages the list; `meerai hooks verify [--command CMD]
  [--reset-baseline]` appends one user `Stop` hook with a per-project failure baseline (first run
  records known failures, later runs block only new names, a pass clears it); the TUI path prompts
  once on a terminal. Hooks run concurrently with a 10 s default timeout (60 s max), fail
  open unless `fail_closed`, log `HookFired`, and may tighten policy but cannot loosen `plan`, an
  explicit `deny`/`ask` rule, or the outside-workspace approval. They run outside `ExecutionWorld`.
- Build surfaces: `meerai-tui` defaults `update` (reqwest); `meerai-cli` defaults `tui`.
  `--no-default-features` drops ratatui, git2, comrak, and the TUI crate; genai still pulls
  reqwest.
- TUI client seam: `WorkspaceQuery` supplies file listing and content search; the local
  implementation lives in `meerai-tui/src/workspace.rs` and is injectable through
  `TuiLaunchConfig::workspace_query`. Do not re-add `meerai-tools` to the TUI.
- Session titles: first user prompt, single line, 80 chars; no retitle when the session already has
  a user message. `context.summarizer_model` overrides the compaction model (default: session
  model); summarizer usage counts toward the session token cap.
- `.env` is opt-in (`MEERAI_LOAD_DOTENV`, `--env-file`); nothing loads implicitly.
- Session files/dirs are `0600`/`0700` on Unix; root is `MEERAI_SESSION_DIR` or the XDG data
  dir (`$XDG_DATA_HOME/meerai/sessions`); `sessions list|rm|prune` is the management surface. Prune defaults to 30 days and keeps 0
  unless `--keep N`; `--continue` resumes by last activity. Automatic retention is opt-in through
  user `sessions.auto_prune_days`/`sessions.keep`, spares the active session, and is never
  controlled by a project file.
- Prompt composition: `BASE_SYSTEM_PROMPT` + a `Workspace root:` line (file tool paths are relative
  to it; a shell `cd` does not move it) + optional `--system` + root `AGENTS.md` +
  `.meerai/instructions.md` (32 KiB/file, 64 KiB block); scoped `AGENTS.md` injected once per turn
  when a tool touches its directory (16 KiB, max 8 files). The agent appends an `Active model:`
  identity line at request time (refreshed by `set_model`) and logs it with the system prompt.
- Agent/skill definitions (M12): read-only discovery at the project root from `.meerai/agents` and
  `.meerai/skills` first, then `.agents/agents` and `.agents/skills`, then `.claude/agents` and
  `.claude/skills`; the higher source wins duplicate ids; no copies and no import lock. Skills with
  a description and no
  `disable-model-invocation` join an 8 KiB screened prompt index; the read-only `skill-load` tool
  returns `SKILL.md` or one listed supporting file inside the skill directory (16 KiB cap).
  Startup `--agent` takes one id (repeating is an error) and repeatable `--skill` append
  instructions to the system prompt; TUI `/agent`/`/skill` send
  `AgentCommand::ApplyAgent`/`ApplySkill` as System instructions, and the `@` Mentions popup
  (agents, then skills, then files; ids in teal for agents and blue for skills; Tab accepts, Enter
  accepts without submitting, Esc dismisses) applies the selected definitions before the prompt. A
  known accepted mention renders as an `<id>` chip in the composer while the raw
  `@agent/<id>`/`@skill/<id>` stays in the buffer; unknown and partial tokens render as typed. On
  submit the token is applied and stripped from the prompt sent to the model, and a directly
  submitted user message keeps it as the same `<id>` chip (replay shows the stripped prompt).
  Frontmatter `mode` is `all`/`primary`/`subagent` and filters `--agent`, the pickers, and
  `subagent-run`; `permissions` join a subagent's policy after the parent rules and drop `allow`
  with a warning; `tools`/`allowed-tools` are warning-only; `steps` is note-only for a primary
  agent; a `:` model is ignored. Definitions are untrusted prompt text and grant no new capability.
- OpenAI plan usage uses the official **Sign in with ChatGPT** OSS flow
  (`chatgpt.tokens.use.direct`, `api.openai.com/v1/responses`), not the Codex
  `backend-api`. One `openai` provider serves an API key or an OAuth plan credential. Plan
  requests force `store:false`, `stream:true`, the system prompt as `instructions`, and omit
  `temperature` and cache retention. Plan OAuth lives in the encrypted `vault_secrets` row (never
  plaintext), is refreshed under one `BEGIN IMMEDIATE` transaction, and is never listed or logged
  as a token. A configured
  provider opens the TUI connection screen (next/update/remove) instead of the auth method screen.
- Windows is a supported target: `cmd /C` for shell, x64 build only, no file-mode enforcement.
- Sampling tiers are a user setting: `context.effort` is `low` (0.00), `medium` (0.20, the
  default), or `high` (0.50). The tier sets the temperature when `temperature` is unset, and is
  sent as the provider reasoning-effort hint. That hint is honored by only some adapters; DeepSeek,
  Groq, xAI, and `open_router` receive the tier temperature and ignore the hint. Both clients show
  the real value (`temp 0.20 · medium`), never `temp default`. `/effort` cycles or sets the tier in
  both clients; the browser also provides a Settings selector.
- Activity digests group each contiguous run of thoughts, tool calls, and subagents immediately
  before the visible message that follows it (or at the end when work trails), folding an
  `Assistant` entry's reasoning right above its visible text in both live and replay views.
- Stale or misspelled model ids are removable in both clients: `forget_provider_model` drops the id
  from `recent_models`, clears the saved `provider.model`, and clears the active model when it
  matches. TUI uses `ctrl+d` in the model picker; the browser uses the `Forget` badge, which routes
  through `UpdateSettingsRequest.forget_model`.
- Terminal-first UX with modal interaction (`Normal` / `Insert` input modes, `ActiveModal`
  overlays, `/` slash commands) paired with the local Leptos Wasm browser companion (`web`
  feature, `meerai serve` / TUI `/serve`); see [browser-ui.md](../../docs/browser-ui.md).
  Desktop GUIs and actor frameworks remain out of scope.

## 12. Recon pointers

| Question | Start |
| :--- | :--- |
| How are tool calls and observations modeled? | `meerai-core/src/tools.rs` |
| How are providers, auth, and the genai client built? | `meerai-providers/src/providers.rs`, `secrets.rs`, `meerai-common/src/settings.rs` |
| How are agent and skill definitions discovered, deduplicated, and applied? | `meerai-common/src/definitions.rs`, `meerai-cli/src/main.rs` (`load_definitions`, `select_definition`, `compose_definition_prompt`, model precedence), `meerai-tui/src/app/` (`/agent`, `/skill`, `parse_mentions`, `apply_mentions`), `meerai-agents/src/multi_turn_agent/mod.rs` (`apply_instructions`) |
| How do subagent child sessions and built-in agent subsets work? | `meerai-agents/src/subagent.rs`, `meerai-agents/src/multi_turn_agent/tools.rs`, `meerai-session/src/session.rs` (`create_child`) |
| How are `#[tool]` and `#[derive(Toolset)]` expanded? | `meerai-macros/src/tool/` |
| Where is runtime YAML config loaded, how is the project root found, and how is project config merged? | `meerai-common/src/settings.rs` (`load_settings_from`, `find_project_root`, `load_project_settings`, `merge_project_settings`) |
| How does the session log persist and project? | `meerai-session/src/session.rs`, `projection.rs`, `storage.rs` |
| How are session dirs/files secured, listed, forked, pruned, and replayed? | `meerai-session/src/perms.rs`, `session.rs` (`list_sessions`, `delete_session`, `fork_latest`), `replay.rs` (`replay_from_events`), `meerai-cli/src/{main.rs,sessions_cmd.rs}` (`fork_session`, `select_prunable`, `auto_prune_sessions`), `meerai-tui/src/app/events.rs` (`TranscriptEntry` adapter) |
| How are session spill blobs and compaction planned? | `meerai-session/src/blob.rs`, `compaction.rs` |
| How are permission decisions made? | `meerai-policy/src/engine.rs` (`decide`, `Plan`/`Ask`/`Auto` branches, `record_grant`) |
| How are coding and retrieval tools guarded, diffed, and executed? | `meerai-tools/src/code/`, `meerai-tools/src/repo_map/`, `meerai-tools/src/lsp/`, `meerai-tools/src/search_as_code/` |
| How is the system prompt composed and scoped instructions injected? | `meerai-agents/src/prompt.rs`, `multi_turn_agent/mod.rs` (`inject_scoped_instructions`) |
| How does the agent loop and harness work? | `meerai-agents/src/multi_turn_agent/`, `harness.rs`, `events.rs` |
| How is tool output redacted, titled, and summarized? | `meerai-agents/src/redact.rs`, `multi_turn_agent/mod.rs` (`new_session_title`, `record_summary_usage`, summarizer selection) |
| How does ACP integrate external harnesses or expose meerai to editors? | `docs/acp.md`; `meerai-acp/src/{client,events,worker}.rs`; `meerai-agents/src/acp_subagent.rs` (`acp-run`); `meerai-cli/src/{acp_cmd,main,headless}.rs`; `meerai-tui/src/{lib,app,ui}`; M13 is landed for `agy`, M14 is backlog |
| How do TUI approvals, slash commands, and transcript replay work? | `meerai-tui/src/app/` (`ActiveModal::Approval`, `transcript_from_events`, `/compact`, `/mode`) |
| How are CLI flags parsed, sessions resumed/forked, pruned, and headless output printed? | `meerai-cli/src/{cli.rs,main.rs,sessions_cmd.rs,headless.rs}` (`parse_args`, `run_sessions`, `select_prunable`, `run_headless`, `format_token_summary`) |
| How do `.env` opt-in and env scrubbing work? | `meerai-cli/src/main.rs` (`load_dotenv`), `meerai-tools/src/code/shell.rs` |
| How do hooks load, trust, execute, and affect policy? | `meerai-common/src/hooks.rs` (`load_hooks`), `meerai-agents/src/hooks.rs` (`HookEngine`), `meerai-cli/src/main.rs` (trust gate), `meerai-agents/src/multi_turn_agent/{turn.rs,tools.rs,compaction.rs}`, `subagent.rs` |
| What is the target crate & migration plan? | `docs/architecture.md`, `docs/harness-spec.md` §4.1 |
