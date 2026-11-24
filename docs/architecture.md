# Architecture

This document describes the crate architecture of the meerai workspace:
what is implemented today, how the pieces connect, and what remains
planned.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-18 |
| **Audience** | Contributors navigating the workspace; embedders choosing a build surface |
| **Scope** | All seventeen workspace crates, dependency direction, build surfaces, protocol and configuration summary |
| **Related** | [harness-spec.md](harness-spec.md) (behavior and milestones) · [harness-internals.md](harness-internals.md) (implementation invariants) · [tui-design.md](tui-design.md) · [browser-ui.md](browser-ui.md) · [acp.md](acp.md) |

## 1. Overview

meerai is a pure, provider-agnostic terminal agent harness: a Rust
workspace with sixteen library crates plus the `meerai` binary (built by
`meerai-cli`). It offers a selection-first modal TUI, a scriptable
headless CLI, and an opt-in local browser baseline (`meerai serve`) over one agent loop. The loop is driven by an append-only
session log and a typed item protocol; every client consumes the same
events. Depth lives in the owning documents: product behavior in
[harness-spec.md](harness-spec.md), implementation traps in
[harness-internals.md](harness-internals.md).

## 2. Implemented

### 2.1 Workspace crates

| Crate | Role |
| --- | --- |
| `meerai-core` | `Toolset` trait (`effect_of`, `effect_of_call`, `permission_spec`, `should_spill_output`, `grant_path_root`, `invoke_structured`), `ToolCall`, `ToolDefinition`, `ToolOutput::{Text,Fail,Stop}`, `ToolError`, `EffectClass`, `PermissionAction`, `ApprovalPreview`, `GrantScope`, `ToolObservation`, `ToolRun`, `FileDiff`, lenient argument parsing/coercion, shared text/env/fs-perms helpers; re-exports `genai`, `async_trait`, `schemars::JsonSchema` |
| `meerai-macros` | Proc-macros `#[tool]` and `#[derive(Toolset)]`; insta snapshot tests |
| `meerai-store` | Embedded SQLite store (`meerai.db`): WAL, FTS5, schema v1, private-file permissions, and typed repositories for config, memories, packs, routes, and usage |
| `meerai-common` | SQLite-backed `AppSettings`, project-root discovery and tighten-only project merge, markdown agent/skill discovery (`definitions`) across user, trusted packs, and project sources, hook configuration (`hooks`) |
| `meerai-client` | Client-facing provider surface: `ProviderCatalog` + bundled `providers.yml` and the generated `models.yml` table (context windows, prices, per-model effort levels), `RuntimeProviderState`, `KeyStatus`, `SecretStore` contract, `MemorySecretStore`, auth listing |
| `meerai-providers` | `genai` client factory plus the AEAD `VaultSecretStore` (ChaCha20-Poly1305, OS keychain DEK, Argon2id passphrase fallback) and ChatGPT plan OAuth |
| `meerai-session` | Append-only `SessionEvent` log in SQLite, crash recovery and idempotent healing, `derive_messages` projection, replay, fork/rewind, SQLite blob and snapshot stores, spill and compaction primitives, listing/deletion/pruning |
| `meerai-policy` | `PolicyEngine`: ordered rules (default sensitive → user → agent, last match wins), `ApprovalMode` presets, literal turn/session grants, YAML parsing |
| `meerai-tools` | `code` (`CodeTools`, seven `code-*` tools, path guards, unified diffs, observations, shell safety inputs, env scrub), `repo_map` (`retrieval-repo_map`, tree-sitter + PageRank), `lsp` (`lsp-*` over stdio JSON-RPC), `search_as_code` (`sac-run_script`), `web` (`web-fetch`, `web-search`), `ExecutionWorld` sandbox seam |
| `meerai-agents` | `MultiTurnAgent` turn/step loop, `AgentHarness`/`HarnessConfig`, `AgentEvent`/`AgentCommand`, interceptor seam, hook engine, `question-ask`, `skill-load`, prompt composition, `spawn_worker`, subagent runtime, delegated `acp-run`, redaction and injection screening, shell-safety allowlist |
| `meerai-syntax` | Tree-sitter highlighting (Rust, C, C++, C#, Java, Kotlin, Scala, Swift, PHP, Haskell, Ruby, Python, JavaScript, TypeScript/TSX, Go, Bash, HTML, CSS, JSON) plus a lexical fallback; per-line tokens, no UI dependency |
| `meerai-host` | UI-neutral host seams (`SessionOpener`, `OpenedSession`, `WorkspaceQuery`/`LocalWorkspace`) and `SessionRegistry` live event pump/ring |
| `meerai-web-protocol` | Serde-only Wasm-safe wire DTOs (`SnapshotResponse`, `ViewEvent`, `SubmitPromptRequest`, `ApiError`) with adjacent serde tagging |
| `meerai-web` | Loopback HTTP/SSE server (`hyper` + `subtle` Basic auth, `Host`/`Origin`/JSON checks, embedded Wasm bundle serving) |
| `meerai-web-client` | Leptos 0.8 CSR browser client crate (`scripts/build-web.py`), DOM transcript, modals, and theme ([browser-ui.md](browser-ui.md)) |
| `meerai-tui` | Modal TUI library (`ratatui` + `crossterm`): transcript replay and cached rendering, tabs, approvals, questions, rewind/fork menus, diff/search modals, mentions, palette, theme, re-exporting `meerai-host` seams |
| `meerai-acp` | ACP client bridge: `agy` preset, handshake, typed `AcpEvent` stream, `AcpWorker` (event mapping + session log), permission replies, cancel, `session/load` |
| `meerai-cli` | Binary crate (`meerai`): CLI flags, headless runner, `auth`/`sessions`/`memory`/`routes`/`pack`/`config`/`hooks`/`acp doctor`/`serve` (feature `web`) subcommands, TUI launcher, host wiring |

### 2.2 Dependency direction

```text
core ──▶ session ──▶ agents ──▶ cli
  ├──▶ policy ──▶ agents / common / tui
  ├──▶ tools ──▶ cli
  ├──▶ common ──▶ client ──▶ tui ──▶ cli
  │                    └──▶ providers ──▶ cli
  ├──▶ syntax ──▶ tui
  └──▶ agents ──▶ acp ──▶ cli
macros ──▶ (consumers)
```

- `meerai-agents` re-exports the session vocabulary from `events.rs`, so
  protocol code can name the types through `meerai_agents`.
- `meerai-tui` consumes events, `RuntimeProviderState`, and a
  `WorkspaceQuery`; it does not link `meerai-tools` or `meerai-providers`.
- Only `meerai-cli` depends on `meerai-acp`; the TUI reaches ACP through
  `Backend::Acp` and `SessionOpener::create_acp_session`. No SDK type
  escapes `meerai-acp`.
- `meerai-cli` wires client, providers, common, policy, session, tools,
  agents, acp, and tui together.

Build surfaces: `meerai-tui` has a default `update` feature (reqwest);
`meerai-cli` has a default `tui` feature. `cargo check -p meerai-cli
--no-default-features` builds the headless surface without ratatui, git2,
comrak, or the TUI crate; `cargo check -p meerai-tui --no-default-features`
builds the TUI without the update check. genai still pulls reqwest in both
builds. CI runs both reduced-feature checks.

### 2.3 Protocol and loop (summary)

Behavior is specified in [harness-spec.md](harness-spec.md); invariants
live in [harness-internals.md](harness-internals.md). The load-bearing
facts:

- **`AgentEvent`** (`meerai-agents/src/events.rs`) is the harness→client
  stream: `TurnStarted`, `ItemStarted`, `ItemDelta`, `ItemCompleted`,
  `ApprovalRequested`, `ApprovalResolved`, `QuestionAsked`,
  `QuestionResolved`, `RewindCompleted`, `RecoveryStarted`,
  `UsageUpdated` (with `est_cost_usd`), `TurnCompleted`, `Error`,
  `HookCompleted`. `ItemCompleted` is authoritative.
- **`AgentCommand`** is the client→loop channel: `SubmitPrompt`, `Steer`,
  `Approve`, `AnswerQuestion`, `Rewind`, `SetModel`, `SetPermissionMode`,
  `Compact`, `ApplyAgent`, `ApplySkill`, `Reload`, `SetHooks`, `Cancel`,
  `Reset`, `Quit`. There is no `Fork` or `Revert` command; fork UX lives in
  the CLI and the TUI session opener.
- **`SessionEvent`** (`meerai-session/src/event.rs`, 24 variants) is the
  durable log. With a harness attached, requests come from
  `Session::derive_messages`; `chat_history` is only a mirror. A
  full-result projection cache keyed by event count and policy avoids
  re-deriving unchanged logs (`multi_turn_agent/mod.rs`).
- **Approvals.** `ApprovalMode` + ordered rules produce `Allow | Ask |
  Deny`; `Ask` suspends the tool item until `Approve` resumes it. Without
  a command channel, requests auto-decline. Grants are literal; Turn
  grants clear at turn end.
- **Execution.** All tool calls per step run: consecutive read-only calls
  concurrently, mutating calls serialized, questions first.
  `ToolOutput::Stop` halts the remaining batch. Redaction and screening
  run before spill, log, UI, and model text.
- **Resilience.** Status-classified provider retries (3 attempts,
  250/500 ms + ≤50% jitter), stream header/read timeouts (30 s / 120 s),
  empty-stream and partial-stream retry, one compaction retry on context
  overflow, circuit breakers, budgets, and one bounded recovery turn after a
  retryable failure.
- **Extension.** In-loop extensions use the ordered `Interceptor` chain
  (`before_step`, async `before_tool`, `after_tool`, `turn_stopping`);
  checkpoint capture is the built-in interceptor. Hooks use the separate
  `HookEngine` because they await host processes outside the loop.

### 2.4 Configuration (summary)

Two layers: user `settings.yml` in `$XDG_CONFIG_HOME/meerai` and an
optional project `<project-root>/.meerai/settings.yml`, discovered by
walking up from the CWD to `.git` or `.meerai/settings.yml`. The project
merge is tighten-only (permissions, budgets, sandbox); provider, model,
approval mode, sessions, and hooks blocks are ignored with warnings.
Tools and sessions keep the CWD as the workspace. `providers.yml` holds
provider overrides and per-model metadata; `models.yml` is the generated
shipped model table, including each model's reasoning effort levels; keys
resolve stored secret → configured `api_key` → provider env vars; `.env`
loads only by opt-in.
Field-level detail: [harness-internals.md](harness-internals.md).

### 2.5 Agents, skills, and tools (summary)

- Markdown definitions load read-only from `.meerai/`, then `.agents/`, then
  `.claude/` at the project root and apply as prompt text (`--agent`/`--skill`,
  TUI commands and mentions). They grant no new capability. Agent `permissions`
  tighten a subagent's policy; `allow` entries are dropped. Custom definitions
  run write-capable under the parent's mode unless a rule denies it.
- `CodeTools` hand-writes `impl Toolset`; the retrieval toolsets use
  `#[derive(Toolset)]`. Runtime names are `<toolset>-<tool>` (`code-*`,
  `retrieval-repo_map`, `lsp-*`, `sac-run_script`). `sac-run_script`
  registers only when `python3` or `node` is detected at startup.
- Execution modes: interactive TUI (default on a terminal), headless
  `--prompt` (text or `--json` NDJSON), resume (`--continue`/`--resume`/
  `--fork`, with or without `--prompt`), and `sessions list|rm|prune|fork`
  management. Exit codes: `0` completed, `1` failure (including a declined
  Spend approval), `2` tool-call approval required.

## 3. Planned

| Item | Description | Gate |
| --- | --- | --- |
| Browser client polish | Delta coalescing, definition chips, ACP badge, browser automation, and an accessibility audit; the host, protocol, server, and Leptos client are implemented | [Browser UI](browser-ui.md) §3 |
| Provider trait | A `ChatCompletion`-style abstraction in `meerai-providers`; today the abstraction is the genai client plus the catalog | Demand for a non-genai backend |
| Incremental projection | Tail-incremental `derive_messages` and frozen/shared message representations; the full-result cache is implemented | Projection purity |
| Progressive tool disclosure | Compact tool index plus a `load_tool_schema` meta-tool when admitted schemas grow (MCP, skills) | Tool count growth |
| `meerai export` | Session export for evals and audit; replay already produces the human projection | CLI milestone |
| Definition mapping | Map definition `tools`/`permissions` and MCP servers | Security review |
| M14 ACP agent | `meerai acp` on stdio for editors | [acp.md](acp.md) §3.1 |
| Per-model budget auto-tuning | Budgets derive from catalog metadata today; automatic tuning is not built | Usage data |

The former "target crate layout" (provider split, dedicated CLI crate,
TUI without concrete providers) is implemented and described in §2.

## 4. Reference

### 4.1 Migration status

The migration is complete through M13; the authoritative milestone table
is [harness-spec.md](harness-spec.md) §4.1. In brief: session log and
projection (M1), item protocol (M2), coding tools (M3), policy and
approvals (M4), context engine (M5), distribution hardening (D1),
checkpoints/rewind v1 (M6), hooks v1 (M7), subagent runtime (M8), CLI and
fork (M9), retrieval v2/v3 (M10), security and supply chain (M11),
definitions discovery (M12), and the `agy` ACP client (M13) are
implemented. M14 and the items in §3 are planned.

### 4.2 Non-goals

- Desktop GUIs, remote or multi-user browser hosting, actor frameworks, and bot integrations. See [browser-ui.md](browser-ui.md) for the local browser proposal.
- Embedding-based retrieval for live code (lexical + structural only).
- Destructive history rewrites: rewind and compaction shadow, never
  delete.
- Dynamic hot-swappable plugin runtimes: Rust composition at startup,
  hooks, and markdown definitions cover the extensibility need.
