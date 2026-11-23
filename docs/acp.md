# ACP Integration

This document covers the Agent Client Protocol integration: the implemented
`agy` client (M13), the delegated `acp-run` tool, the trust boundary, and
the planned presets and editor surface (M14).

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-17 |
| **Audience** | Users of external agents, `meerai-acp` contributors, security reviewers |
| **Scope** | `meerai-acp`, ACP wiring in `meerai-cli` and `meerai-tui`, `meerai-agents::acp_subagent` |
| **Related** | [harness-spec.md](harness-spec.md) (§4.1 M13/M14) · [threat-model.md](threat-model.md) · [harness-internals.md](harness-internals.md) |

## 1. Overview

Two problems share one protocol. First, other harnesses (Antigravity,
OpenCode, Claude Code, Codex, Gemini CLI) already do autonomous coding;
ACP is the vendor-neutral seam that lets meerai drive them instead of
re-implementing each integration. Second, editors (Zed, Neovim, JetBrains)
speak ACP; one `meerai acp` subcommand would put meerai inside them without
an editor plugin. The first problem is implemented for `agy`; the second
is planned (M14).

**ACP is a transport and a vocabulary, not a sandbox and not a policy
engine.** It does not make an external agent safe, and it does not let
meerai enforce its rules on one. Read §5 before relying on any feature
claim.

## 2. Implemented

### 2.1 Crate and dependency shape

`meerai-acp` is the client bridge. Only `meerai-cli` depends on it
directly; the TUI reaches ACP through `Backend::Acp` and
`SessionOpener::create_acp_session`, so no SDK type appears in
`meerai-tui`, `meerai-agents`, or `meerai-session` signatures. The crate
depends on `meerai-agents` (for `AgentEvent`/`AgentCommand`) and owns every
`agent_client_protocol` type; its public API uses plain strings, structs,
and tokio channels.

Public surface (`meerai-acp/src/{lib,client,events,worker}.rs`):

- `AcpPreset { id, command, args }` with the `AGY` constant (`agy acp`) and
  a `presets()` catalog. `agy` is the only preset.
- `connect(preset, cwd)` / `connect_with`: spawn via `AcpAgent::from_args`,
  drive the SDK connection future as a normal tokio task, run `initialize`
  with `ProtocolVersion::V1`, then `session/new` (or `session/load`). The
  handshake is bounded by a 120 s timeout; a prompt has no fixed timeout
  and is cancelled through `cancel`. Returns `AcpSession` and an
  `AcpHandshake` (agent name, version, capabilities, auth, modes).
- `AcpSession`: `prompt`, `cancel`, `reply_permission`, `cancel_permission`,
  `shutdown`, `handshake`, `stderr_tail`, and an `AcpEvent` stream:
  `Connected`, `TextDelta`, `ThoughtDelta`, `ToolCall`, `ToolCallUpdate`,
  `Plan`, `Usage`, `PermissionRequest`, `Finished`, `Error`.
- `AcpWorker` (`spawn_acp_worker`): the long-lived bridge that maps
  `AcpEvent` to `AgentEvent` and mirrors the session log (§2.3).

The SDK engine is runtime-neutral (`futures`, `async-io`/`async-process`
transports). A spike proved the connection future runs under meerai's
tokio runtime with no dedicated thread; the transport's own threads are
internal to it. No `unstable_*` SDK feature is enabled.

### 2.2 Adapter spawn and environment

- The adapter inherits meerai's environment, **including provider
  credentials**. The shell tool's secret scrub is deliberately not applied
  to the spawn, or adapters cannot authenticate.
- The SDK cannot set the child working directory: the adapter runs in
  meerai's process CWD, and the requested workspace goes only into
  `session/new`/`session/load`. Run meerai from the workspace.
- `initialize` advertises **no** `fs` or `terminal` capability, so no extra
  client surface is granted.
- Adapter stderr goes through the SDK debug hook into a bounded 200-line
  in-memory tail (`AcpSession::stderr_tail`), printed by `acp doctor`.
  There is no stderr log file and no redaction of the tail today (§3).
  Adapter stdout is protocol traffic only and is never displayed.
- One adapter process per meerai session; the process is killed on exit.

### 2.3 Worker and session record

`AcpWorker` dual-writes every turn into the local session log, so transcript
replay, titles, and pruning keep working:

- `SessionMeta` carries `harness: "acp:agy"` and `remote_session_id`
  (`Session::set_remote_session`).
- Turn lifecycle maps to `TurnStarted`/`UserMessage`/`AssistantMessage`/
  `TurnEnded`; tool calls and results are logged; approvals are logged as
  `ApprovalRequested`/`ApprovalDecided`.
- `session/load` replays remote history as text notifications **before**
  `Connected`; the worker drains events up to `Connected` so the local
  transcript stays the source. Pre-`Connected` updates never start a turn.
- Commands handled: `SubmitPrompt`, `Cancel`, `Approve`, `Quit`. All other
  `AgentCommand` variants (`Steer`, `SetModel`, `SetPermissionMode`,
  `Compact`, `Rewind`, `ApplyAgent`/`ApplySkill`, `Reload`, `SetHooks`,
  `AnswerQuestion`, `Reset`) are ignored on an ACP tab.

### 2.4 Event mapping (ACP → meerai)

The implemented mapping (`meerai-acp/src/{events,worker}.rs`):

| ACP | meerai |
| --- | --- |
| `agent_message_chunk` | `ItemStarted`/`ItemDelta` text on an `AgentMessage` item |
| `agent_thought_chunk` | reasoning item deltas |
| `tool_call` | `ItemStarted { ToolCall }` |
| `tool_call_update` | `ItemCompleted { ToolCall }` (no intermediate `StatusChanged` delta) |
| `tool_call.content` blocks | observation text |
| `plan` | `SessionItem::Plan` item (started + completed) |
| `session/request_permission` | `ApprovalRequested` with `ApprovalKind::ExternalAgent` |
| permission answer | `ApprovalResolved` |
| `usage_update` | mapped to `AcpEvent::Usage`, then discarded by the worker (§3) |
| `PromptResponse.stop_reason` | `TurnCompleted` |

Stop reasons: `end_turn` → `Completed`; `cancelled` → `Aborted { User }`;
`max_tokens` → `MaxTokens`; `max_turn_requests` → `Completed`; `refusal` →
`Failed` ("agent refused").

Every ACP tool call is classified `EffectClass::External` regardless of the
agent-reported kind. The kind is display information; it does not drive
local policy, because local policy does not apply (§5).

### 2.5 Permissions

`session/request_permission` becomes an approval request in the ordinary
overlay, labeled "external agent permission". The overlay shows the
agent-authored title and the adapter name; the user picks one of the
**agent's** options. Decisions map by option **kind**, never by index or
label (`worker.rs`):

| Decision | Option chosen |
| --- | --- |
| `Accept` (`y`/Enter) | narrowest `allow_once`; fallback `allow_always` |
| `AcceptForSession` (`a`) | `allow_always`; fallback `allow_once` |
| `Decline` / `DeclineWithReason` (`n`/`r`) | `reject_once`; fallback `reject_always` |
| `Cancel` (Esc) | no option (cancelled) |

- The overlay appears in every approval mode; `/mode` has no effect on an
  ACP tab because the worker ignores `SetPermissionMode`.
- Headless runs decline external-agent permission requests; the adapter
  receives the rejection and decides what to do with it.
- meerai's rules and grants are **not** consulted (the crate has no
  `meerai-policy` dependency). The external agent's request is the whole
  decision surface; a local deny on top would be theater, because the
  agent can ignore it. The decision is advisory in both directions.

### 2.6 CLI and TUI surfaces

| Surface | Behavior |
| --- | --- |
| `meerai --acp` | TUI bound to the `agy` adapter. Bare flag; there is no preset argument. Conflicts with `--agent`/`--skill`; `--mode` is ignored with a warning |
| `meerai --acp --prompt "..."` | headless turn through the adapter |
| `meerai acp doctor` | spawn, `initialize`, print preset, command, protocol, agent, `loadSession`, auth, modes, session id, and the stderr tail; exit |
| TUI `/acp` | open a new ACP tab (argument ignored; `agy` is the only preset) |
| Statusline | `ACP:agy` badge |
| Transcript header | `agy (acp)` plus `external agent · policy not applied` |
| `plan` mode | refuses to start: "ACP cannot run in plan mode; the external agent cannot be made read-only" |

The default backend stays the built-in engine; ACP is opt-in per run.
Headless ACP runs print no token summary: ACP has no prompt/completion
token counts, and the worker does not emit `UsageUpdated`. A context-usage
line is planned (§3).

### 2.7 Resume and fork

- `--continue`/`--resume <ID>` of an ACP session uses `session/load` when
  the agent advertises `loadSession`. Otherwise meerai silently starts a
  new remote session; the local transcript still replays, but the external
  agent loses its history. A recap prompt is planned (§3).
- Resuming a non-ACP session with `--acp` fails with a clear message.
- Forking an ACP session (`--fork`, TUI fork) always starts a **new** remote
  session (`AcpStart::New`); `Session::fork` does not copy
  `harness`/`remote_session_id`. A single log never mixes two agents.

### 2.8 Delegated `acp-run`

The top-level native agent can call `acp-run` with
`{"agent":"agy","task":"..."}` (`meerai-agents/src/acp_subagent.rs`). An
optional `name` gives the run a display label and names the child session.
The tool:

- creates a child session (`parent_session_id`, `harness: acp:agy`,
  `remote_session_id`) that appears under the parent; the nested TUI view
  loads from the child log;
- sends one prompt, forwards `session/request_permission` to the parent
  approval overlay as `ApprovalKind::ExternalAgent`, emits `SubagentRun`
  items, and returns the report with a `child_session_id` hint;
- enforces a wall-clock cap: default 600 s, clamped to 30..=3600. On
  timeout it cancels, drains for 2 s, and quits the adapter;
- performs **no** token, grant, or budget rollup (unlike `subagent-run`).

Registration is top-level only: native subagents and ACP children never
see the tool. Policy tier is `External`: `plan` denies it, `ask` asks,
`auto` allows. The external agent still has its own tools and policy.

### 2.9 Tests

37 in-crate unit tests cover the preset catalog and every pure mapping
(text, thought, tool call, tool call update, plan, usage, permission kind,
stop reason) plus the worker. Two ignored live tests need `agy` installed
and authenticated:

```sh
cargo test -p meerai-acp --test live_agy -- --ignored
cargo test -p meerai-acp --test live_worker -- --ignored
```

`live_agy` connects, sends `Reply with OK`, answers a permission with
`allow_once` if one arrives, and requires `Finished` within 60 s.
`live_worker` asserts the session log carries the user message,
`provider == "acp:agy"`, and `model == "agy"`. The TUI has ACP-focused tests
for the tab command, badge, and header.

## 3. Planned

| Item | Description | Gate |
| --- | --- | --- |
| User preset catalog (`acp.yml`) | `~/.config/meerai/acp.yml` presets (`command`, `args`, `label`, `env` with `${VAR\|default}` expansion). User config only; a project `acp` block must be ignored so a repository cannot launch a process. Unknown presets fail with the known list | Config design |
| OpenCode preset | `opencode acp` (official, ACP v1, ndjson over stdio). Proven reachable in the P0 spike | Preset catalog |
| Gemini / Claude / Codex presets | `gemini --experimental-acp` (flag spelling to verify), `claude-agent-acp` (npm adapter), `codex-acp`; later Cursor, Copilot, Qwen, Kimi, Kiro, Goose, Augment, OpenHands | Preset catalog; one preset each, no code change |
| Usage surfacing | Feed `usage_update.used`/`.size` to the statusline and print `usage: context=<used>/<size> (reported by <harness>)` in headless ACP runs. `usage_update.cost` stays ignored; meerai does not report USD for ACP | Worker wiring (`AcpEvent::Usage` exists and is discarded today) |
| Resume recap | When the agent cannot `session/load`, send a short labeled recap built from the last turns instead of silently starting fresh | Prompt design |
| Stderr log file | Move the tail to `<session-dir>/acp-stderr.log`, passed through `redact_text` before writing | Redaction seam |
| Tool status deltas | Emit `ItemDelta::StatusChanged` on `tool_call_update` before completion, and map the agent-reported kind to an `EffectClass` for display | Wire-contract review |
| `fs` capability | If `fs/read_text_file` is ever advertised, route it through `CodeTools` path guards and produce `FileChange` diffs. Today no `fs`/`terminal` capability is advertised, deliberately: a partial guarantee is worse than an honest none | Security review |
| M14 — `meerai acp` | meerai as an ACP agent on stdio for editors (§3.1) | ACP compatibility kit + Zed smoke |

Out of scope (non-goals): proxying or rewriting an external agent's
requests (the SDK `Proxy` role), MCP-over-ACP, the `Conductor`, forking
remote sessions, USD cost accounting in either direction, streaming
terminal widgets for adapter tool calls, replaying a remote session the
adapter cannot load, and any claim that meerai policy governs an external
process.

### 3.1 M14 design (planned)

`meerai acp` would run meerai as an agent on stdio; editors launch it.

| Method | Planned behavior |
| --- | --- |
| `initialize` | advertise `loadSession`; no image, audio, or embedded context; no MCP |
| `session/new` | create a meerai session with the client's `cwd` as the workspace |
| `session/load` | open the session and replay `replay_from_events` as updates |
| `session/prompt` | run `MultiTurnAgent`; stream deltas as `session/update` |
| `session/cancel` | cancel the turn token |
| `session/set_mode` | map ACP modes to `ApprovalMode`; an unknown mode keeps the current mode and receives a notice |

Mapping (meerai → ACP): text deltas → `agent_message_chunk`; reasoning →
`agent_thought_chunk`; `ToolCall` items → `tool_call` with kind and
locations; status changes and completed content → `tool_call_update`;
`FileChange` → content `diff`; `ApprovalRequested` →
`session/request_permission` (`AcceptForSession` → `allow_always`, a turn
grant → `allow_once` for that call, because ACP has no turn-scoped grant);
`UsageUpdated` → `usage_update` with context used/size, no cost;
`TurnCompleted` → the matching stop reason; `Error` → JSON-RPC error or
notice.

## 4. Reference: protocol in one page

Checked against the ACP specification and the Rust SDK
(`agent-client-protocol` 5.3.0, Apache-2.0, MSRV 1.88; meerai pins 1.100).

| Item | Value |
| --- | --- |
| Encoding | JSON-RPC 2.0, UTF-8 |
| Transport | stdio: client spawns the agent; newline-delimited JSON on stdin/stdout |
| Agent logs | stderr only; stdout carries protocol messages only |
| Paths | absolute, always |
| Lines | 1-based |
| Keys | `camelCase`; discriminator values `snake_case` |
| Extensibility | `_meta` fields; methods prefixed with `_` |

Client → agent: `initialize`, `session/new`, `session/load` (needs the
agent's `loadSession` capability), `session/prompt`, `session/set_mode`,
`session/cancel` (notification).

Agent → client: `session/update` (notification),
`session/request_permission`, optional `fs/read_text_file` /
`fs/write_text_file`, optional `terminal/*`, optional
`elicitation/create`.

`session/update` variants relevant to meerai: `agent_message_chunk`,
`agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`,
`current_mode_update`, `usage_update`. The rest are UI-only
(`available_commands_update`, `session_info_update`, `notice`) or
capability-gated (`compaction_update`, `compaction_summary_chunk`).

Permission options carry a kind: `allow_once`, `allow_always`,
`reject_once`, `reject_always`. Permission requests are **advisory**: the
agent performs its own work and is responsible for honoring the answer.
Prompt stop reasons: `end_turn`, `max_tokens`, `max_turn_requests`,
`refusal`, `cancelled`.

The SDK follows semver on the stable surface; every `unstable_*` feature
may change in a minor release. meerai does not enable `unstable_*`.

## 5. Trust boundary

What ACP guarantees:

- A well-formed message stream and a version handshake.
- A UI-visible signal when the agent asks for permission.
- Cancellation.

What ACP does **not** guarantee:

- That the agent obeys a denial. Permission requests are advisory.
- That meerai's `permissions` rules, `plan` mode, path guards, redaction,
  or `ExecutionWorld` apply to the external process. They do not.
- That the agent stays inside the workspace. It has its own path handling
  and inherits meerai's working directory and credentials.
- That the agent's tools are the ones it reports.
- Any OS sandbox. The ACP security page says so itself.

Rules the implementation follows:

- The statusline badge and transcript header name the external agent and
  warn that meerai policy does not apply.
- No `fs` or `terminal` capability is advertised, so no extra client
  surface is granted.
- `plan` mode refuses to start an ACP backend, with a message that says
  why: the external agent cannot be made read-only.
- The adapter inherits provider credentials by design (§2.2); this is
  documented, not hidden.
- `acp-run` delegates one task; it does not make the external agent
  local-policy-bound. Treat the badge and header as an external-engine
  warning, not a safety claim.

Residual risks are owned by [threat-model.md](threat-model.md) §5.

## 6. Open questions

| # | Question | What settles it |
| --- | --- | --- |
| 1 | Should the worker surface `AcpEvent::Usage` (statusline + headless line) or drop the variant? | Product decision; the event is produced and discarded today |
| 2 | Is the silent fresh remote session on non-`loadSession` resume acceptable, or should the recap land first? | User feedback on lost external history |
| 3 | One adapter per session is enough today; parallel adapters would need a session-to-process map | Demand |

Resolved decisions: `plan` mode fails rather than warns (§2.6); the
external-engine warning appears once per session in the header plus the
persistent badge; `acp doctor` prints capabilities and the negotiated
version; no `fs` capability in the current phase. The worker's unread
`resumed` flag was removed; `AcpHandshake.resumed` (the `session/load`
report) stays.

## 7. References

- ACP overview: <https://agentclientprotocol.com/protocol/v1/overview>
- ACP prompt turn: <https://agentclientprotocol.com/protocol/v1/prompt-turn>
- ACP tool calls: <https://agentclientprotocol.com/protocol/v1/tool-calls>
- ACP security: <https://agentclientprotocol.com/protocol/v1/security>
- ACP transports: <https://agentclientprotocol.com/protocol/v1/transports>
- ACP agents: <https://agentclientprotocol.com/get-started/agents>
- Rust SDK: <https://github.com/agentclientprotocol/rust-sdk>
- Antigravity CLI ACP: <https://antigravity.google/docs/cli/acp>
- OpenCode ACP: <https://opencode.ai/docs/cli/acp/>
