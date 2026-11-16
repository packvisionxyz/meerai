# ACP integration

**Status: M13 is landed for `agy`.** `meerai-acp` is a client bridge; the CLI
and TUI expose `--acp`, `/acp`, `acp doctor`, an `ACP:agy` badge, an
`agy (acp)` transcript header, and `session/load` resume. OpenCode/Claude
presets and M14 (`meerai acp` for editors) remain backlog. The work is tracked
as **M13** (meerai as an ACP client) and **M14** (meerai as an ACP agent) in
[harness-spec.md](harness-spec.md) §15.1. Every protocol statement below was
checked against the ACP specification and the Rust SDK. Sections that describe
surfaces beyond the crate are a plan, not a description of the tree; §10.2
records what M13 actually ships.

## 1. Why

Two problems share one protocol.

1. **Use other harnesses from meerai.** Antigravity (`agy`), OpenCode, Claude
   Code, Codex, and Gemini CLI already do autonomous coding. Re-implementing
   each vendor integration is wasteful. ACP is the vendor-neutral seam.
2. **Put meerai inside editors.** Zed, Neovim, and JetBrains speak ACP. One
   `meerai acp` subcommand makes meerai usable there without an editor plugin.

ACP is a transport and a vocabulary, not a sandbox and not a policy engine. It
does not make an external agent safe, and it does not let meerai enforce its
rules on one. See §8 before reading any feature claim.

## 2. Scope

In scope:

- Spawn and drive an external ACP agent from the TUI and from headless mode.
- Map external tool calls, messages, thoughts, plans, and permission requests
  into the existing `AgentEvent` v2 vocabulary and TUI surfaces.
- Cancel a turn.
- Keep meerai's session log as the local record, with a replayable transcript.
- Expose meerai as an ACP agent for editors.

Out of scope:

- Proxying or rewriting an external agent's requests (the SDK `Proxy` role).
- MCP-over-ACP.
- Forking remote sessions.
- USD cost accounting in either direction.
- Any claim that meerai policy, path guards, redaction, or the approval overlay
  govern an external process.

## 3. Protocol in one page

| Item | Value |
| --- | --- |
| Encoding | JSON-RPC 2.0, UTF-8 |
| Transport | stdio: client spawns the agent; newline-delimited JSON on stdin/stdout |
| Agent logs | stderr only; stdout must carry protocol messages only |
| Paths | absolute, always |
| Lines | 1-based |
| Keys | `camelCase`; discriminator values `snake_case` |
| Extensibility | `_meta` fields; methods prefixed with `_` |

Client → agent:

| Method | Purpose |
| --- | --- |
| `initialize` | version and capability negotiation |
| `session/new` | create a session |
| `session/load` | resume a session (needs the agent's `loadSession` capability) |
| `session/prompt` | send user content; returns a stop reason |
| `session/set_mode` | switch agent operating mode |
| `session/cancel` | notification; cancel the turn |

Agent → client:

| Method | Purpose |
| --- | --- |
| `session/update` | stream content and tool events (notification) |
| `session/request_permission` | ask the user before a tool call |
| `fs/read_text_file`, `fs/write_text_file` | optional client file access |
| `terminal/*` | optional client command execution |
| `elicitation/create` | optional structured question |

`session/update` variants relevant to meerai: `agent_message_chunk`,
`agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`,
`current_mode_update`, `usage_update`. The rest are either UI-only
(`available_commands_update`, `session_info_update`, `notice`) or capability
gated (`compaction_update`, `compaction_summary_chunk`).

Permission options carry a kind: `allow_once`, `allow_always`, `reject_once`,
`reject_always`. The protocol states that permission requests are **advisory**:
the agent performs its own work and is responsible for honoring the answer.

Prompt stop reasons: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`,
`cancelled`.

## 4. Rust SDK

| Item | Value |
| --- | --- |
| Crate | `agent-client-protocol` 5.3.0 (14 Nov 2026) |
| License | Apache-2.0 |
| MSRV | 1.88; meerai pins 1.100 |
| Features used | `process` (spawn via `AcpAgent`), `stdio` (`Stdio` for M14) |
| Roles | `Client.v1()`, `Agent.v1()`, `Proxy.v1()`, `Conductor` |
| Test support | `agent-client-protocol-test`, plus the protocol compatibility kit |

Two integration facts drive the plan:

- The engine is **runtime-neutral** and built on `futures`; the native
  `process` and `stdio` transports use `async-io` / `async-process`, not tokio.
  The connection future is driven by the embedder. P0 proved that it runs under
  meerai's tokio runtime with no dedicated thread (§10.1), so the crate spawns
  the connection as a normal `tokio` task.
- `ByteStreams` needs `futures::io::{AsyncRead, AsyncWrite}`. If we ever drive
  our own transport, adapt tokio pipes with `tokio-util` compat types.

The SDK follows semver on the stable surface, but every `unstable_*` feature may
change in a minor release. meerai will not enable `unstable_*` in M13 or M14.

## 5. Harness presets

A preset is a command plus arguments. It is data, not code, so adapter churn
does not force a meerai release.

The landed crate carries exactly one preset, `agy`. The table below is the
adoption order for later phases; the user-config catalog in this section is
still planned.

| Order | Harness | Command | Notes |
| --- | --- | --- | --- |
| 1 | Antigravity | `agy acp` | Official CLI; native; no Node. First target; shipped as the `AGY` preset. |
| 2 | OpenCode | `opencode acp` | Official; ACP v1; ndjson over stdio; starts a private server |
| 3 | Gemini CLI | `gemini --experimental-acp` (`--acp` on newer builds) | Flag spelling to verify at adoption |
| 4 | Claude Code | `claude-agent-acp` (npm) | Adapter over the Claude Agent SDK; needs Node |
| 5 | Codex | `codex-acp` | Official adapter |
| later | Cursor, Copilot, Qwen, Kimi, Kiro, Goose, Augment, OpenHands | per vendor | One preset each; no code change |

Proposed user configuration, `~/.config/meerai/acp.yml` (not implemented):

```yaml
presets:
  agy:
    command: agy
    args: ["acp"]
    label: Antigravity
  opencode:
    command: opencode
    args: ["acp"]
    label: OpenCode
```

Rules:

- User config only. A project `acp` block is ignored with a warning. A
  repository must not be able to launch an arbitrary process when meerai
  starts.
- A preset without `command` is invalid and is skipped with a warning.
- `env` may be set per preset. The value is expanded with the existing
  `${VAR|default}` helper.
- Unknown presets fail with the list of known names.

## 6. M13 — meerai as an ACP client

### 6.1 Crate layout

Crate `meerai-acp`, depended on by `meerai-tui` only in P2; today no crate
depends on it.

```text
meerai-acp/src/
  lib.rs        preset catalog (`agy`), public re-exports, crate docs
  client.rs     spawn, handshake, command loop, permission bridge
  events.rs     ACP -> `AcpEvent` mapping, typed event structs
```

The planned split into `preset.rs` (config parsing), `bridge.rs` (`AgentEvent`
mapping), `permission.rs`, and `logger.rs` arrives with the P2 surfaces.

The crate owns every `agent_client_protocol` type. No SDK type appears in
`meerai-agents`, `meerai-session`, or `meerai-tui` signatures.

### 6.2 Lifecycle

P1 implements steps 1–6 below in crate form: no session log, no stderr file,
no surface wiring. Process teardown is the SDK transport guard; the stderr log
file is P2.

1. Resolve the preset.
2. Spawn the adapter with `AcpAgent`, with the inherited environment. The child
   inherits meerai's process working directory; the requested workspace goes
   only into `session/new` (see §10.2 for the limit this creates).
3. `initialize`; request protocol v1 and advertise **no** `fs` or `terminal`
   capability in phase 1.
4. `session/new`, or `session/load` when the agent advertises `loadSession`.
5. Run `session/prompt` on submit; stream updates.
6. `session/cancel` on Esc or ctrl+c; mark unfinished tool calls cancelled.
7. Kill the process on exit (`kill_on_drop`); flush the stderr log.

One adapter process per meerai session. Adapter stderr is captured through the
SDK debug hook into a bounded in-memory tail (`AcpSession::stderr_tail`); P1
writes no log file and does not redact it. P2 moves the tail to
`<session-dir>/acp-stderr.log`, passed through `redact_text` before it is
written. Adapter stdout is protocol traffic only and is never displayed.

### 6.3 Event mapping (ACP → meerai)

P1 exposes `AcpEvent` (see §10.2). The `AgentEvent` v2 mapping below is P2 and
is not implemented.

| ACP | meerai |
| --- | --- |
| `agent_message_chunk` | `ItemDelta` text on an `AgentMessage` item |
| `agent_thought_chunk` | `ItemDelta` reasoning |
| `tool_call` | `ItemStarted { ToolCall }` with the status below |
| `tool_call_update` | `ItemDelta::StatusChanged`, then `ItemCompleted` |
| `tool_call.content` blocks | observation text |
| `tool_call.content` diff | `SessionItem::FileChange` |
| `tool_call.locations` | sidebar paths (display only) |
| `plan` | `System` transcript note in phase 1 |
| `current_mode_update` | statusline mode badge |
| `usage_update.used` / `.size` | context usage in the statusline |
| `usage_update.cost` | ignored |
| `session/request_permission` | `ApprovalRequested` |
| permission answer | `ApprovalResolved` |
| `PromptResponse.stop_reason` | `TurnCompleted` |

Tool status mapping: `pending` → `Pending`, `in_progress` → `InProgress`,
`completed` → `Succeeded`, `failed` → `Failed`.

Tool kind mapping to `EffectClass`:

| ACP `kind` | `EffectClass` |
| --- | --- |
| `read`, `search`, `think`, `other` | `Read` |
| `edit`, `delete`, `move` | `Write` |
| `execute` | `Execute` |
| `fetch` | `External` |
| `switch_mode` | no tool item; render a system note |

Stop reason mapping:

| `stop_reason` | `TurnCompleted` |
| --- | --- |
| `end_turn` | `Completed` |
| `cancelled` | `Aborted` |
| `max_tokens`, `max_turn_requests` | `Failed { Budget }` |
| `refusal` | `Failed` |

### 6.4 Permissions

`session/request_permission` becomes an ordinary approval request. The overlay
shows the agent-authored `title`, the tool `kind`, every `location`, and the
adapter name. The user picks one of the **agent's** options.

Mapping rules:

- Pick by option **kind**, never by index or label.
- `y` / Enter → the narrowest `allow_once`; fall back to `allow_always` only
  when `allow_once` is absent.
- `a` → `allow_always` when present; otherwise `allow_once` plus a note.
- `n` / `r` → `reject_once`; `reject_always` only when the agent asks for it.
- Esc, cancel, or `plan` mode → `cancelled`.
- No matching option → `cancelled`. Never guess an allow.
- meerai's own rules and grants are **not** consulted. The external agent's
  request is the whole decision surface; adding a local deny on top would be
  theater, because the agent can ignore it.

In `yolo` mode the same rule applies, limited to `allow_once` when the agent
offers it. That is a deliberate, documented reduction of the local guarantee.

### 6.5 Session record and resume

- `SessionMeta` gains two additive fields: `harness: Option<String>` and
  `remote_session_id: Option<String>`.
- meerai logs the turn as `SessionEvent`s, so transcript replay, titles, usage
  counters, and pruning keep working.
- The remote session lives in the adapter's storage. If the agent advertises
  `loadSession`, `--continue` reconnects to it. Otherwise meerai starts a new
  remote session and sends a short recap built from the last turns. The recap
  is a prompt, not a replay, and is labeled as such in the transcript.
- Switching presets mid-session starts a new session. A single log must not mix
  two agents.

### 6.6 Usage and cost

- `usage_update` gives context `used` and `size`. That feeds the statusline.
- Prompt and completion token counts are not part of ACP. The headless
  `tokens: prompt=… completion=…` line is therefore **not printed** for an ACP
  backend. The line is replaced with `usage: context=<used>/<size> (reported by
  <harness>)` when the agent reports it.
- `usage_update.cost` exists in the protocol and is ignored. meerai does not
  report USD.

### 6.7 Surfaces

| Surface | Behavior |
| --- | --- |
| `meerai --acp agy --prompt "..."` | headless turn through the adapter |
| `meerai --acp agy` | TUI bound to the adapter |
| `meerai acp doctor [--preset agy]` | spawn, `initialize`, print capabilities, exit |
| TUI `/acp <preset>` | bind the next turn to a preset; unknown names list the presets |
| Statusline | `ACP:agy` badge next to the approval mode |
| `/acp` with no argument | list presets; do not switch |

The default backend stays the built-in engine. ACP is opt-in per run.

## 7. M14 — meerai as an ACP agent

`meerai acp` runs meerai as an agent on stdio. Editors launch it.

| Method | meerai behavior |
| --- | --- |
| `initialize` | advertise `loadSession`; no image, audio, or embedded context; no MCP |
| `session/new` | create a meerai session with the client's `cwd` as the workspace |
| `session/load` | open the session and replay `meerai_session::replay::replay_from_events` as updates |
| `session/prompt` | run `MultiTurnAgent`; stream deltas as `session/update` |
| `session/cancel` | cancel the turn token |
| `session/set_mode` | map ACP modes to `ApprovalMode` |

Mapping (meerai → ACP):

| meerai | ACP |
| --- | --- |
| `ItemDelta` text | `agent_message_chunk` |
| `ItemDelta` reasoning | `agent_thought_chunk` |
| `ItemStarted { ToolCall }` | `tool_call` with `kind` and `locations` |
| `StatusChanged` | `tool_call_update` status |
| `ItemCompleted { ToolCall }` | `tool_call_update` with content |
| `FileChange` | `tool_call_update` content `diff` |
| `ApprovalRequested` | `session/request_permission` with `allow_once` / `allow_always` / `reject_once` |
| `UsageUpdated` | `usage_update` with context `used` / `size`; no cost |
| `TurnCompleted` | prompt response with the matching stop reason |
| `Error` | JSON-RPC error or `notice` |

Mode mapping: `plan` → ACP plan mode, `ask` → ask, `auto-edit` → auto-edit,
`yolo` → yolo. A client that sends an unknown mode keeps the current mode and
receives a notice.

Agent-side approval reasons come from `PolicyEngine`. `AcceptForSession`
becomes `allow_always`; a turn grant becomes `allow_once` for that call only,
because ACP has no turn-scoped grant concept.

Gate for M14: an editor smoke (Zed) plus the ACP compatibility kit.

## 8. Trust boundary

This is the part that must appear in the README and the release notes before
M13 ships.

What ACP guarantees:

- A well-formed message stream and a version handshake.
- A UI-visible signal when the agent asks for permission.
- Cancellation.

What ACP does **not** guarantee:

- That the agent obeys a denial. Permission requests are advisory.
- That meerai's `permissions` rules, `plan` mode, path guards, or redaction
  apply to the external process. They do not.
- That the agent stays inside the workspace. It has its own path handling.
- That the agent's tools are the ones it reports.
- Any OS sandbox. ACP says so in its own security page.

Rules for M13:

- The statusline and the transcript header name the external agent and say
  `external agent: meerai policy does not apply`.
- meerai does not advertise `fs` or `terminal` in phase 1, so no extra
  capability is granted. When `fs` is added later, it must go through
  `CodeTools` path guards and produce `FileChange` diffs.
- Adapter commands come from user config only.
- Adapter stderr is redacted before it is written.
- Preset `env` values are not logged.
- The adapter inherits the environment, including provider credentials. The
  shell tool's `is_sensitive_env` scrub must **not** be applied to the spawn,
  or adapters cannot authenticate. This is intentional and must be documented.

`plan` mode with an ACP backend is not a read-only guarantee. The honest
behavior is to refuse to start an ACP backend in `plan` mode, with a message
that says why.

## 9. Non-goals

- MCP-over-ACP, proxies, and the `Conductor`.
- Agent-to-agent delegation through ACP (M8 stays a native subagent runtime).
- Streaming terminal widgets for adapter tool calls.
- Replaying a remote session that the adapter cannot load.
- Cost normalization across vendors.

## 10. Phases

| Phase | Work | Gate |
| --- | --- | --- |
| M13-P0 | Throwaway spike: client against `agy acp`, one prompt, one permission, one cancel. Decide the runtime question. | **Landed.** Connection future runs under tokio without a dedicated thread (§10.1) |
| M13-P1 | `meerai-acp`: `agy` preset, spawn, handshake, typed event bridge, permission bridge, cancel, stderr tail | **Landed.** 14 unit tests over the preset catalog and mappings; one `#[ignore = "live agy"]` test (§10.2) |
| M13-P2 | `--acp`, `/acp`, statusline badge, transcript header, resume semantics | **Landed for `agy`.** TUI tests; live `agy` headless smoke and `session/load` resume |
| M13-P3 | OpenCode preset, `acp doctor`, docs, M13 row | **Partial.** `acp doctor`, docs, and the M13 row are landed; the OpenCode preset remains backlog |
| M14-P1 | `meerai acp`: initialize, session new/load, prompt, cancel, modes | Compatibility kit |
| M14-P2 | Editor smoke, docs, M14 row | Zed manual test |

P0 answered one question: does the SDK connection future run under tokio
without a dedicated thread? It does, so P1 is a small bridge.

### 10.1 P0 result

The `connect_with` future was driven as a `tokio::spawn` task on a multi-thread
runtime. It is `Send + 'static`, every ACP call (`initialize`, `session/new`,
`session/prompt`, `session/request_permission`, `session/cancel`) completed on
that runtime, and a second `tokio::spawn` cancelled a turn through a cloned
`ConnectionTo<Agent>`. Handshakes and prompts ran against `agy acp` 1.5.9 and
`opencode acp` 2.3.0; one `agy` permission request was answered and one
`opencode` turn was cancelled. No dedicated thread or foreign-runtime bridge is
needed. The transport still starts its own `async-io` and `async-process`
threads; those are transport internals that appear on any embedder runtime.

The spike also fixed three implementation choices: `AcpAgent::from_args` instead
of `from_str` so preset arguments are not re-split, `with_debug` for adapter
stderr, and a wall-clock timeout around the connection.

### 10.2 P1 result: typed bridge

`meerai-acp` now holds a long-lived client. Public shape:

- `AcpPreset { id, command, args }` with the `AGY` constant and a `presets()`
  catalog. `AGY` is `agy acp`; there is no `opencode` and no `claude` preset.
- `connect(preset, cwd)` builds the transport with `AcpAgent::from_args`, spawns
  the SDK `connect_with` future as a `tokio` task, runs `initialize` with
  `ProtocolVersion::V1`, then `session/new` for `cwd`, and returns an
  `AcpSession` once both succeed. The handshake is bounded by a 120 s timeout;
  a prompt has no fixed timeout and is cancelled through `cancel`.
- `AcpSession` exposes `prompt`, `cancel`, `reply_permission`,
  `cancel_permission`, `shutdown`, and an `AcpEvent` stream: `Connected`,
  `TextDelta`, `ThoughtDelta`, `ToolCall`, `ToolCallUpdate`, `Plan`, `Usage`,
  `PermissionRequest`, `Finished`, `Error`. A prompt emits `Finished` with the
  stop reason (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`,
  `cancelled`).
- Permission requests are forwarded to the caller as `PermissionRequest` with
  the agent's options mapped by kind; the reply comes back through
  `reply_permission` / `cancel_permission`. The library never auto-approves.
- Adapter stderr goes through the SDK debug hook into a bounded in-memory tail;
  protocol stdout is consumed only by the transport and never displayed.
- All `agent_client_protocol` types stay inside the crate; the public API uses
  plain strings, structs, and `tokio` channels.

Not included in P1: no CLI flag, no TUI surface, no `/acp`, no `acp.yml` or
settings loading, no session-log or `AgentEvent` mapping, no provider login or
configuration change, and no policy, sandbox, path-guard, or redaction claim
over the external agent process.

Two limits observed on a live `agy` run:

- The spawned adapter inherits meerai's process working directory. `cwd` is
  sent only in `session/new`, and `agy acp` 1.5.9 resolved one relative write
  against its own process directory. P2 must set the child working directory
  (or require the caller to run from the workspace) before the surface can
  claim the writes land in the session workspace.
- One write-prompt run produced no session updates for 120 s, then the same
  prompt completed in 18 s with two permission requests answered as
  `allow_once` and tool calls `pending` → `in_progress` → `completed`. Treat
  adapter stalls as agent-side, and keep the caller able to cancel.

Tests: unit tests cover the preset catalog and every pure mapping (text chunk,
thought chunk, tool call, tool call update, plan, usage, permission kind, stop
reason). `meerai-acp/tests/live_agy.rs` is a single
`#[ignore = "live agy"]` test that connects to `agy acp`, waits for
`Connected`, sends `Reply with OK`, answers a permission request with
`allow_once` if one arrives, and requires `Finished` within 60 s. It compiles
without running; `cargo test -p meerai-acp --test live_agy -- --ignored` runs it
and needs `agy` installed and authenticated.

## 11. Open questions

1. Should `meerai --acp` in `plan` mode fail, or run with a warning? **Resolved:
   fail.** The external agent cannot be made read-only.
2. Should the transcript header repeat the policy warning on every turn, or
   once per session? Proposal: once per session, plus the statusline badge.
3. Advertise `fs.read_text_file` in phase 1 so reads get path guards? Proposal:
   no. Most adapters do their own I/O; a partial guarantee is worse than an
   honest none.
4. Should `acp doctor` print adapter capabilities and the negotiated version?
   Proposal: yes; it is the cheapest debugging tool.
5. Is one adapter per meerai session enough, or do we need parallel adapters?
   Proposal: one, until M8 exists.

## 12. References

- ACP overview: <https://agentclientprotocol.com/protocol/v1/overview>
- ACP prompt turn: <https://agentclientprotocol.com/protocol/v1/prompt-turn>
- ACP tool calls: <https://agentclientprotocol.com/protocol/v1/tool-calls>
- ACP security: <https://agentclientprotocol.com/protocol/v1/security>
- ACP transports: <https://agentclientprotocol.com/protocol/v1/transports>
- ACP agents: <https://agentclientprotocol.com/get-started/agents>
- Rust SDK: <https://github.com/agentclientprotocol/rust-sdk>
- Antigravity CLI ACP: <https://antigravity.google/docs/cli/acp>
- OpenCode ACP: <https://opencode.ai/docs/cli/acp/>
