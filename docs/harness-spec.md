# meerai Harness Specification

Unified product specification for the `meerai` agent harness: design
principles, implemented behavior, and the planned backlog.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-17 |
| **Audience** | Contributors, harness/tool authors, reviewers |
| **Scope** | Session log, item protocol, turn loop, context engine, permissions, safety nets, agents/skills/subagents, retrieval, ACP, milestones |
| **Related** | [architecture.md](architecture.md) (crate layout) · [harness-internals.md](harness-internals.md) (implementation invariants) · [tui-design.md](tui-design.md) · [acp.md](acp.md) · [threat-model.md](threat-model.md) · [token-efficiency.md](token-efficiency.md) |

## 1. Overview

The design is grounded in primary-source research of four production
harnesses:

| Harness | Standout pattern | What meerai adopts |
| --- | --- | --- |
| **Codex** (`codex app-server`, Rust) | Typed Thread → Turn → Item protocol; OS-kernel sandboxing | Typed item lifecycle (`Started`/`Delta`/`Completed`); sandbox-mode × approval-policy split |
| **OpenCode** (TypeScript) | Markdown agents with ordered permission rules; session forking | `.agents`/`.claude` definitions as prompt instructions; `allow/ask/deny` rule engine; fork and rewind |
| **DeepSeek Harness** (TypeScript) | Append-only `SessionEvent` log with pure projections | **Model-visible means logged**: one append-only log, `derive_messages` as the single pure projection, context techniques as projection rules |
| **Claude Code** (TypeScript) | Deterministic lifecycle hooks; per-prompt checkpoints with `/rewind` | `PreToolUse`/`PostToolUse` command hooks (JSON stdin, exit-2 block); per-turn snapshots and restore |

Empirical support for "grow the harness, not the context": *Growing
Harness* (arXiv:2609.26760) reports 76–92% fewer LLM calls and 74–99% lower
inference cost when recurring control decisions compile into harness code.
Cognition's SWE-grep line shows search subagents returning only
`file:start-end` spans materially cut retrieval cost. Both patterns are
adopted (§2.5, §2.8, §2.10).

Milestone status is tracked in §4.1. M1–M13 are implemented; M14 and the
backlog in §3 are planned.

### Design principles

**P1 — The event log is the single source of truth.** Every session is an
append-only log of durable events. The model transcript, the TUI
transcript, telemetry, checkpoint metadata, and exports are projections of
that log. Nothing the model sees may exist outside the log, so any request
is exactly reconstructable and replayable offline.

**P2 — Typed items decouple harness from UI.** The harness emits a typed
`Session → Turn → Item` protocol. Every item has an explicit lifecycle:
`Started` (full initial payload) → zero or more typed `Delta`s →
`Completed` (authoritative final payload). The TUI, the headless printer,
and any future server consume the same protocol; UI state is a fold over
item events.

**P3 — Deterministic control lives in the harness.** Anything decidable
without the model is decided in code: permission rules, hooks, budgets,
circuit breakers, schema validation, spill/compaction triggers,
checkpointing, retries. Recurring model-driven routines migrate into
harness primitives over time.

**P4 — Context is a scarce, budgeted resource.** Working context is
compiled per request from the log under explicit token budgets (spill
stubs, ephemera stripping, reasoning pruning, tiered compaction,
cache-friendly ordering). Storage history keeps everything; working
context keeps only what the next step needs.

## 2. Implemented

### 2.1 Layered architecture

All boxes below exist in the tree; the SDK/server client is planned (§3).

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ Clients: meerai-tui (modal TUI) │ headless printer (--prompt, --json)    │
└───────────────▲──────────────────────────────────────────────────────────┘
                │  AgentEvent stream (typed items: Started/Delta/Completed)
┌───────────────┴──────────────────────────────────────────────────────────┐
│ meerai-agents — Turn/Step loop                                           │
│   interceptor seam · hook engine · compaction triggers · breakers        │
│   cancellation · retries · recovery turn · subagents · acp-run           │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-session — append-only SessionEvent log                            │
│   SQLite persistence · derive_messages projection · fork · replay        │
│   snapshots · blobs · spill · compaction primitives                      │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-policy — permissions · approval modes · grants                    │
│ meerai-tools — code/repo_map/lsp/sac/web toolsets · ExecutionWorld      │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-core — Toolset · ToolOutput · ToolObservation · FileDiff (genai)  │
│ meerai-common — settings · definitions · hooks config                    │
│ meerai-client/providers — ProviderCatalog · SecretStore · genai client   │
└──────────────────────────────────────────────────────────────────────────┘
```

Crate roles and dependency direction: [architecture.md](architecture.md).

### 2.2 Session event log (`meerai-session`)

#### Event vocabulary

`SessionEvent` (`meerai-session/src/event.rs`, adjacent serde tagging) has
24 variants in four groups:

```rust
// structure (log-only, never projected into messages[])
TurnStarted  { turn_id, trigger }
TurnEnded    { turn_id, reason }
StepStarted  { step_id, turn_id }
StepEnded    { step_id, finish }

// model-visible surface events (projected by derive_messages)
SystemMessage    { sections, generation }
UserMessage      { source, content }
AssistantMessage { provider, model, content, tool_calls, usage,
                   stream_digest, call_signatures }
AssistantAttempt { /* failed/retried/cancelled attempt; log-only, keeps usage */ }
ToolCall         { call_id, name, args }
ToolResult       { call_id, output }

// context-engine events (log-only; mutate the surface via replacements)
SpillApplied      { call_id, bytes_omitted, blob_path }
CompactionStarted { compaction_id, trigger }
CompactionSummary { compaction_id, replaced, summary }
CompactionEnded   { compaction_id, result }

// harness bookkeeping (log-only)
UsageUpdated      { usage, cumulative }
ApprovalRequested { request_id, kind, preview }
ApprovalDecided   { request_id, decision, scope }
QuestionAsked     { request_id, questions }
QuestionAnswered  { request_id, answers }
RewindApplied     { turn_id, scope, restored, replaced }
HookFired         { event, decision, detail }
GoalUpdated       { state }   # legacy
LoopUpdated       { state }
CheckpointCreated { checkpoint_id, turn_id, paths }
CircuitBreaker    { kind, action }
Error             { code, message, retryable }
```

Rules:

- **Durable vs. live.** Only facts that must survive a restart are log
  events. Per-token deltas are live `AgentEvent`s (§2.3) and are never
  logged individually; the settled `AssistantMessage` embeds a compact
  `StreamDigest` so replay reproduces the transcript.
- **Structured turn-end reasons:** `Completed | Aborted { cause } |
  Failed { error } | MaxTokens | Interrupted` (crash orphans closed on
  recovery). Fork always stops at a closed turn, so no fork closer is
  needed.
- **Tool results are structured observations:**
  `ToolObservation { status, exit_code, stdout, stderr, hint }`, rendered
  deterministically into the tool-role message.
- **Every model-visible input is logged.** Injected runtime context
  (scoped `AGENTS.md`, diagnostics, reminders) is logged as `UserMessage`
  with a typed non-`user` `InputSource`, so projection rules can strip or
  refresh it.
- **Reasoning signatures:** `AssistantMessage.call_signatures` is
  index-aligned with `tool_calls`, defaulted for old logs, and skipped
  when every entry is `None`. The projection restores them; a stored
  signature is never sent as `None` (Gemini 2.5/3 require them).

#### Persistence

```text
~/.local/share/meerai/meerai.db                  # $XDG_DATA_HOME/meerai or MEERAI_DB_PATH
├── sessions                   # id, project root, cwd, git, title, model, mode, lineage, timestamps
├── session_events             # append-only event rows, ordered by (session_id, seq)
├── session_events_fts         # FTS5 projection of event text
├── session_blobs              # zstd spilled tool outputs
├── session_snapshots          # checkpoint rows: turn_id, rel_path, existed, zstd content
└── meta                       # schema version and vault key envelope
~/.local/share/meerai/sessions/<session-id>/session.lock   # one worker lock per session
```

- The store is SQLite with WAL, `synchronous = NORMAL`, `foreign_keys = ON`, and
  a 5 s busy timeout. Event rows are append-only and ordered by `seq`; the FTS5
  triggers index event text for `Session::search_events`.
- The root is `<data>/meerai.db`, or `MEERAI_DB_PATH`. The sessions directory is
  `$XDG_DATA_HOME/meerai/sessions` (`MEERAI_SESSION_DIR`) and holds only the
  worker lock.
- On Unix, the database and lock files are `0600` and directories `0700`,
  re-tightened on open; Windows has no equivalent enforcement.
- **Crash recovery and healing.** On open, the session heals dangling tool
  calls (one error `ToolResult` per unpaired call in the last visible
  assistant message), closes every open turn with
  `TurnEnded { Interrupted }`, and closes an open compaction bracket with
  `CompactionEnded { Failed }`. Healing is idempotent, skips rewound and
  compacted ranges, and also runs after `TurnEnded` in the live loop, so a
  cancelled turn never sends an unpaired call. Read-only replay
  (`Session::load_events`) never heals.

#### Projection: `derive_messages`

```rust
pub fn derive_messages(events: &[LoggedEvent], policy: &ProjectionPolicy) -> Vec<ChatMessage>;
```

- Pure and deterministic, snapshot-tested (insta). External
  reconstructors must produce byte-identical transcripts.
- `ProjectionPolicy` carries the context knobs: `strip_ephemera`,
  `prune_reasoning`, `verbatim_window_turns` (default 4),
  `clear_old_tool_results` (default true), `include_system_messages`.
- The agent caches the full projection keyed by `(event_count, policy)`
  and recomputes on new events or a policy change
  (`meerai-agents/src/multi_turn_agent/mod.rs`). Incremental tail
  projection and frozen/shared messages are planned (§3).
- Empty-content assistant messages are skipped; failed attempts project
  as nothing but keep their usage in accounting. Assistant text stays
  beside its tool calls in one message.
- **Age-based clearing:** results outside the verbatim window become
  short stubs that keep the call paired and name the spill blob; the
  newest window stays verbatim (the window floors at 1). The log keeps
  every result.

#### Forking and rewind

- `fork(source, boundary)` copies events through an inclusive closed
  `turn-end` boundary into a new session id and records `forked_from` +
  `inherited_event_count` in the `sessions` row. An open or unknown boundary turn
  is rejected, not silently clipped. Surfaces: `--fork <ID>`,
  `sessions fork <ID> [--turn <ID>]`, TUI `/fork`, and the user-message
  menu's Fork action.
- `rewind(turn_id, scope)` closes open turns and appends one
  `RewindApplied { scope, restored, replaced }`; later turns are shadowed
  by a replacement range, never deleted. `RewindScope::{Both, Code,
  Conversation}`; `Both` runs code restore then conversation shadowing.
  Conversation rewind returns the rewound user message to the composer.

### 2.3 Typed item protocol (`AgentEvent`)

`meerai-agents/src/events.rs`. `ItemCompleted` is authoritative.

```rust
pub enum AgentEvent {
    TurnStarted   { turn_id, model },
    ItemStarted   { item },                  // full initial payload
    ItemDelta     { item_id, delta },        // typed incremental updates
    ItemCompleted { item },                  // authoritative final state
    ItemDiscarded { item_id },               // drop an uncommitted partial item
    ApprovalRequested { request },
    ApprovalResolved  { request_id },
    QuestionAsked     { request },
    QuestionResolved  { request_id },
    RewindCompleted   { turn_id, scope, restored, error },
    RecoveryStarted   { attempt },
    UsageUpdated  { usage, cumulative, est_cost_usd },
    TurnCompleted { turn_id, reason },
    Error         { code, message },
    HookCompleted { event, decision, reason },
    ContextUpdated { breakdown },
    LoopUpdated   { state },
    LoopContinued { turn, max_turns },
}

pub enum SessionItem {
    UserMessage { id, content },
    AgentMessage { id, text },
    Reasoning { id, summary },
    ToolCall { id, call_id, name, args, effect, status, output },
    CommandRun { id, command, cwd, aggregated_output, exit_code, duration_ms, status },
    FileChange { id, changes, status },          // Vec<FileDiff> {path, kind, unified diff}
    SubagentRun { id, agent, status, report, child_session_id },
    Compaction { id, trigger, freed_tokens },
    Plan { id, steps },                          // Vec<PlanStep> {text, status}
}

pub enum ItemDelta {
    AgentText(String), ReasoningSummary(String), CommandOutput(String),
    PlanUpdated(Vec<PlanStep>), StatusChanged(ItemStatus),
}
```

Contract rules:

1. Every `ItemStarted.id` is unique per turn; every delta references it;
   the final `ItemCompleted` payload — not the concatenated deltas — is
   authoritative (avoids drift on re-streams).
2. `ApprovalRequested` suspends the item until the client answers via
   `AgentCommand::Approve { request_id, decision, scope }`. Decisions:
   `Accept | AcceptForSession | Decline | DeclineWithReason(String) |
   Cancel`. `AcceptForSession` always records a Session grant; the TUI `t`
   key sends `Accept` with `Some(Turn)` (cleared at turn end); `y`/Enter
   sends `None`. Grant resources match literally; rule resources use
   globs. A `ToolCall` item moves `Proposed → Pending → InProgress →` a
   terminal status.
3. Headless mode serializes the same enum as NDJSON (`--json`); the TUI
   folds it into `AppState`. One protocol, three clients (TUI, headless,
   future server).
4. `AgentCommand` is the client→loop channel: `SubmitPrompt`, `Steer`
   (append input to the in-flight turn), `Approve`, `AnswerQuestion`,
   `Rewind`, `SetModel`, `SetPermissionMode`, `Compact`, `ApplyAgent`,
   `ApplySkill`, `Reload`, `SetHooks`, `Cancel`, `Reset`, `Quit`. There is
   no `Fork` or `Revert` command; fork UX lives in the CLI and the TUI
   session opener. `spawn_worker` handles `SetPermissionMode` and
   `Compact` even while idle.
5. `QuestionAsked` is an interaction request, not a permission (§2.3.1).
6. `Plan` items are produced by the ACP worker; the native loop has no
   plan producer yet (§3).
7. `ItemDiscarded` removes an item that never completed, after a mid-stream
   provider retry. The client drops the item and its deltas; the partial
   text never reaches the session log, so replay cannot restore it. A
   discarded item has no `ItemCompleted`.

#### 2.3.1 Questions and choices

Approvals are yes/no; a plain assistant question ends the turn and forces
the user to retype context. `question-ask` adds a real choice channel:

- **Tool.** `meerai-agents::question::QuestionToolset` registers
  `question-ask` (`EffectClass::Read`); the harness handles the call
  before dispatch. Up to four questions per call, two to eight options
  per question, `header` at most 24 characters; `multiple` allows several
  labels.
- **Protocol.** The harness emits `QuestionAsked`, logs
  `SessionEvent::QuestionAsked`, and awaits `AnswerQuestion`
  (`answers: None` = cancelled). The answer is logged as
  `QuestionAnswered` and returned to the model as a JSON `ToolResult`:
  `{"answers":[{"header":"Database","selected":["Postgres"]}]}`. A
  cancelled or channel-less request returns a structured refusal.
- **Ordering.** Question calls run before other calls in the same batch
  and never concurrently. A question does not stop the turn.
- **TUI.** A choice overlay with keyboard navigation, single/multiple
  markers, and an `Other` typed-answer row; pending questions queue per
  tab ([tui-design.md](tui-design.md) §2.8).
- **Headless.** Auto-cancels with a stderr hint; the turn continues.
  NDJSON emits the same events.
- **Subagents.** Children do not receive the tool; they return a report
  instead of blocking on the parent's user.
- **Logging.** `QuestionAsked`/`QuestionAnswered` are log-only; the
  model-visible record stays the `ToolCall`/`ToolResult` pair. Replay
  renders the question and its selected answers. Questions never consult
  policy and never record grants.

#### 2.3.2 Web fetch and search

`meerai-tools::web::WebTools` registers `web-fetch` and `web-search` as
`EffectClass::External` with the `webfetch` permission action
(`resource_arg` = `url` / `query`). `plan` hides both because it advertises
only `Read` tools; `ask` requests an approval; `auto` allows them.

- **`web-fetch`.** Accepts `http` and `https` only, rejects URL userinfo,
  and follows at most five redirects with the address guard applied to
  every hop. The guard resolves the host and refuses loopback, private,
  link-local, CGNAT, broadcast, multicast, and IPv4-mapped private
  addresses, and rejects `localhost` and `.local` names. DNS rebinding is
  not prevented. Bodies stop at 2 MiB and output at 60,000 characters.
  HTML becomes plain text with the page title kept; JSON, XML, and other
  textual content is returned as-is; binary content is refused.
- **`web-search`.** Sends the query to `html.duckduckgo.com`, then
  `lite.duckduckgo.com` if the first page yields no parsed result. It needs
  no account, returns up to ten numbered results with title, URL, and
  snippet, unwraps `uddg` redirect links, drops duplicate targets, and
  reports a blocked provider instead of retrying.
- Both tools are outside effects: they never write the workspace, never
  spawn a process, and use the shared tool-output spill rule. Fetched text
  reaches the model as untrusted data.

### 2.4 Agent loop: turns and steps

A **step** is one model request plus the tool calls it returns; a **turn**
is zero or more steps opened by user input and closed when nothing is
owed.

```text
turn/start (log)
  claim inbox: pending steering + next user prompt
  loop {
    before_step interceptors → compaction pressure check, repeat-guard reset,
                               scoped-AGENTS.md injection
    build request            → derive_messages (cached) + system prompt assembly
                               + admitted tool schemas (plan mode omits non-read)
    budget gate              → token/step/wall-clock limits
    stream                   → live ItemDeltas; settle AssistantMessage (log)
    on request error         → status-classified retry with jitter, or
                               context-overflow → compaction → retry once
    tool calls (all of them):
      questions first; classify by EffectClass + policy decision
      read-only calls: concurrent; mutating calls: ordered, gated
      before_tool interceptors → checkpoint capture
      hooks (PreToolUse/PermissionRequest) · path guards · approvals
      execute                → sandboxed where applicable
      redact → screen → spill → settle ToolResult (log)
      after_tool interceptors · PostToolUse hooks · batch notes
    step/end (log)
  } while tools owe another request, or steering arrived
  turn_stopping interceptors + Stop hook → may continue the turn
  heal dangling calls · turn/end (log, structured reason)
```

The extension seam is `meerai_agents::intercept`: an ordered
`Vec<Arc<dyn Interceptor>>` with `before_step` (may stop the turn), async
`before_tool` (authorized plan + context, returns session events to log),
`after_tool` (post-redaction/post-spill outcome, may add one model-visible
note), and `turn_stopping` (may continue the turn, consulted only when no
tool calls were made; a later stop can continue again, bounded by the
remaining step budget). Checkpoint capture is the built-in
`CheckpointInterceptor`. The built-in `LoopInterceptor` keeps a loop turn
alive while an item is open, at the phase the `loop-*` tools name, up to 24
rounds.
Hooks use the separate `HookEngine` because they
await host processes outside the loop. Hot-swappable plugins are out of
scope (§3, non-goals).

Loop invariants: a per-turn `CancellationToken` shared into every await;
all retries via `tokio::time::sleep`; tool failures fed back to the model
as tool-role messages, never as aborts; the loop ends on a text-only
response or `ToolOutput::Stop`; every planned call logs a `ToolResult`,
and batch notes log as one `RuntimeContext` message after the last result
so calls stay adjacent to results.

### 2.5 Context engine

Storage history (log + blobs + snapshots) is immutable and complete;
working context is the budgeted projection compiled per step. Every
technique below is a projection rule or post-execute policy — never a
destructive edit of history.

#### Budget model

Per-model context comes from `ProviderCatalog`. Knobs (`context` in
`settings.yml`; defaults in `HarnessConfig`):

| Knob | Default | Meaning |
| --- | --- | --- |
| `context_window_tokens` | 128 000 (catalog wins when unset) | model context budget |
| `high_watermark_pct` | 75 | trigger tiered compaction |
| `spill_max_inline_tokens` | 12 500 | per-tool-result retention budget |
| `verbatim_window_turns` | 4 | recent turns never compacted or cleared |
| `reasoning_pruning` | true | strip settled-step reasoning from projections |
| `prompt_caching` | true | request-level `CacheControl::Ephemeral`; cache counts reported |
| `max_steps_per_turn` | 32 | final step strips tools and asks for a summary |
| `no_progress_fail_steps` | 12 | fail the turn after this many write steps with no file change; read-only steps do not count; the diagnostic notice fires at two thirds |
| `tool_timeout_ms` | 120 000 | per-tool wall clock |
| `max_turn_secs` | 1800 | per-turn wall clock; `0` disables |
| `max_session_tokens` | unset | session token cap with a Spend approval pause |
| `summarizer_model` | session model | compaction summary model |
| `recovery_model` | unset | model for the one recovery turn |
| `rollback_on_verify_failure` | false | auto-mode snapshot restore on a verify block |
| `post_edit_diagnostics` | false | LSP diagnostics after successful edits |
| `temperature` | `0.20` (medium tier) | sampling temperature, 0.0..=2.0 |
| `effort` | `medium` | reasoning tier: `low` 0.00, `medium` 0.20, `high` 0.50, `extrahigh` 0.70, `max` 0.90 |

`effort` sets the temperature for its tier when `temperature` is unset, and
is also sent as the provider reasoning-effort hint (`extrahigh` maps to
`xhigh`; `max` maps to `max`). The active model's catalog entry carries
`effort_levels`; a tier the model does not accept clamps down to the nearest
accepted tier, and an empty set sends no reasoning hint but keeps the tier
temperature. An unknown model keeps all five tiers. An adapter that ignores the hint still receives the tier
temperature. A route's explicit `effort` is sent as-is; the model set gates only
the session tier.

The CLI asks `ProviderCatalog::model_info(provider, model)` for the active
model: a catalog `context_window` replaces the configured default only
when `context.context_window_tokens` is unset; explicit settings and
project overrides always win. A mid-session model switch (TUI `/model`,
the provider/model picker, or an applied agent model) re-derives the
window the same way: the host resolves the catalog window and sends it
with `SetModel`/`ApplyAgent`, unless an explicit settings window pins it.
`input_cost_per_mtok`/`output_cost_per_mtok`
fill `HarnessConfig::token_pricing` and make `UsageUpdated.est_cost_usd`
non-`None`. The bundled catalog ships context windows only, so no
unverified price is reported. Token estimates use a ~4-chars/token
heuristic; the watermark includes the turn's tool-schema estimate.

#### Tool-output spill (pointer swapping)

- Runs post-execute, before the result settles, for `Success`/`Stopped`
  observations over the budget. Keeps ordered head/tail halves of the
  budget, stores the full result at `blobs/<16-hex-hash>-<byte-len>.txt`,
  and appends a recovery notice:

```text
(Omitted 41233 bytes. Full result stored at:
 ~/.local/share/meerai/sessions/<id>/blobs/9f2c…-41233.txt —
 use read_file with offset/limit, or grep this path, to inspect it.)
```

- `read_file`/`grep` work on blob paths: the binary adds the session
  directory to `CodeTools` readable roots. `code-read_file`, LSP, `sac`,
  and `question-ask` results are exempt.
- The log copy keeps only the retained content; the full value exists
  only in the blob. Storage failure fails open with a warning.
- Age-based clearing (§2.2) then stubs old results in the projection; the
  same-model summarizer keeps the parent clearing policy, a distinct
  summarizer disables it.

#### Ephemera stripping and reasoning pruning

- Ephemeral injections (`InputSource::RuntimeContext`) project only into
  the turn they were created for; later projections drop them. Persistent
  injections carry `InputSource::System` and stay.
- Reasoning projects only for the current step; settled steps keep
  reasoning in the log for audits and the TUI transcript. Where a
  provider requires thought-signature replay (Gemini), correctness wins:
  signatures are always restored.

#### Tiered compaction

- **Triggers:** `Pressure` (pre-step watermark), `ContextOverflow`
  (provider error → compact → retry once, only when compaction applied),
  and manual `/compact` (`AgentCommand::Compact`).
- **Edge safety:** boundaries snap to tool-pairing-balanced positions —
  never between a call and its result. With too few turns, the planner
  falls back to the last balanced `StepEnded` outside the newest window.
  Pressure applies at most twice per turn and once per step;
  `run_compaction` returns `Result<bool, TurnError>` (`Ok(false)` =
  skipped).
- **Procedure** (logged, crash-safe via bracket): `CompactionStarted`
  (lock) → summarize the shadowed range into a structured state block
  (goal / progress / modified files / decisions / blockers) →
  `CompactionSummary` with a surface replacement → `CompactionEnded`
  (release). Shadowed events stay in the log; replay reproduces the
  condensed conversation.
- **Cache-safe summary:** with no `summarizer_model`, the summary request
  reuses the parent system prompt, tools, and history and appends
  `SUMMARY_INSTRUCTION` as the last user message, keeping the cached
  prefix warm. A distinct summarizer uses a standalone transcript request
  with clearing disabled. Summary usage counts toward the session token
  cap.
- Summary messages carry a stable marker so TUI/export/evals recognize
  condensed history. A clean reset for long refactors is a fork seeded
  with the state block, not a destructive rewrite.

#### Prefix layout for KV-cache hits

Implemented: `compose_system_prompt` builds one system string from
`BASE_SYSTEM_PROMPT` + a `Workspace root:` line + optional `--system` +
root `AGENTS.md` + `.meerai/instructions.md` (32 KiB/file, 64 KiB block),
sent with `ChatRequest::with_system` and logged as a `SystemMessage` with
a generation counter. Tool schemas attach separately with stable ordering
and strip on the final step. Scoped `AGENTS.md` files are injected once
per turn as `RuntimeContext` user messages (16 KiB, max 8), not into the
system prefix. The loop never reorders the composed string between steps;
compaction summaries project at their replacement point. The full
static-first multi-section prefix (state block as a fixed section) is
planned (§3).

#### Output contracts

Enforced in the coding system prompt and tool schemas: no full-file
rewrites for files over ~30 lines; `edit_file` uses exact
`old_string`/`new_string` (with anchor repair,
[harness-internals.md](harness-internals.md) §2.5); `write_file`/`edit_file`
emit harness-computed unified diffs into `FileChange` items, never diffs
trusted from model text; terse JSON tool arguments.

### 2.6 Permissions, sandboxing, approvals

#### Effect classes and tiers

Every tool declares `EffectClass`: `Read | Write | Execute | External`.

| Tier | Examples | Default handling |
| --- | --- | --- |
| 1 — autonomous read | `read_file`, `grep`, `glob`, `list_dir` | run, concurrent |
| 2 — sandboxed mutation | `edit_file`, `write_file`, sandboxed `shell` | run inside the workspace-write sandbox |
| 3 — human gate | destructive shell, secrets/config edits, `web-fetch`/`web-search`, outside writable roots | approval request (TUI overlay; headless fails fast) |

#### Rule engine

```yaml
# ~/.config/meerai/settings.yml — ordered, last match wins
permissions:
  - { action: read,  resource: "*",          effect: allow }
  - { action: read,  resource: "*.env*",     effect: deny }
  - { action: edit,  resource: "src/**",     effect: allow }
  - { action: shell, resource: "cargo *",    effect: allow }
  - { action: shell, resource: "git push *", effect: ask }
  - { action: shell, resource: "rm *",       effect: deny }
```

- Decision order: `plan` gate → literal grants → rules (default sensitive
  → user → agent, last match wins) → tier defaults. Actions: `read`,
  `edit`, `shell`, `subagent`, `webfetch`, `external_directory`; wildcards
  on action and resource. Patterns without `/` match the basename at any
  depth; patterns with `/` match the workspace-relative path. `~` expands.
- `approval_mode` presets: `plan` (hard read-only; non-Read denied before
  rules and grants, and non-read schemas are not advertised), `ask` (tier
  defaults), `auto` (allow-all tier defaults, explicit `ask`/`deny` still
  win).
- `Ask` produces `ApprovalRequested`; `AcceptForSession` stores a logged
  Session grant so repeats don't re-prompt. Denied calls return a
  structured refusal to the model and are logged. Outside-workspace
  access forces an approval that adds a literal path root. A
  `SandboxEscape` approval reruns a sandbox-denied shell command without the
  sandbox (§ Sandbox enforcement).
- **Project overlay.** `<project-root>/.meerai/settings.yml` (root found
  by walking up to `.git` or `.meerai/settings.yml`) merges tighten-only:
  project `ask` rules → user rules → project `deny` rules; `allow` dropped
  with a warning; invalid shapes drop the block; `context` overrides field
  by field with budget clamps; `active_provider`, `active_model`,
  `providers`, `approval_mode`, `sessions`, and `hooks` are ignored with
  warnings. A malformed file warns and is ignored. Tools and sessions keep
  the CWD as the workspace.

#### Sandbox enforcement

- **In-process guards:** `resolve_for_read`/`resolve_for_write`
  canonicalize every path and confine reads/writes to the workspace plus
  configured roots (the session dir is readable for spill blobs). `shell`
  pins cwd to the workspace, runs `sh -c` / `cmd /C`, defaults to a 120 s
  timeout, and scrubs provider secrets from the child environment. There
  is no command deny list; use `action: shell` rules. Safe read-only
  commands auto-approve in `ask` mode when no rule matched
  ([harness-internals.md](harness-internals.md) §2.5).
- **OS sandbox (`ExecutionWorld`):** `SeatbeltWorld` (macOS
  `sandbox-exec`) and `BubblewrapWorld` (Linux `bwrap`) confine
  `code-shell` (workspace-write) and `sac-run_script` (read-only), deny
  network by default, and mask sensitive host and workspace paths.
  `HostWorld` fallback carries a startup warning;
  `sandbox.require_for_shell`/`require_for_scripts` fail closed;
  `sandbox.strict_env` allowlists the child environment. A sandboxed shell
  command whose output matches a write-denial marker offers one
  `ApprovalKind::SandboxEscape`; an accepted rerun uses
  `SandboxPolicy::Unrestricted` with the same environment scrub. Turn and
  session grants cover later shell calls in that scope. `plan` mode and
  `sandbox.require_for_shell` never offer it, and the `unsandboxed` flag is
  hidden from the model schema. The trait is a
  seam: a remote/container world can replace host execution without
  changing toolsets. Security effects and residual risks:
  [threat-model.md](threat-model.md).

### 2.7 Safety nets

#### Checkpoints and rewind (v1)

- The snapshot store lives at `sessions/<id>/snapshots/<turn-id>/`
  (`manifest.json` maps workspace-relative paths to `{ blob, existed }`).
- Capture runs before write-class tools (`write_file`, `edit_file`) via
  the `CheckpointInterceptor`: original bytes once per file per turn,
  symlinks and files over 2 MiB skipped, `existed: false` for new files,
  `CheckpointCreated` logged. Capture is best-effort and never fails the
  turn. Retention keeps the newest 100 turn directories.
- Restore stages each file beside its target and renames it into place;
  a failed step rolls back applied steps and reports rollback failures.
  This is per-file atomicity, not crash durability.
- `AgentCommand::Rewind { turn_id, scope }` restores code and/or shadows
  conversation (§2.2). The TUI `/rewind` menu targets the latest user
  turn; the message menu targets the clicked turn.
- Limits: shell-only changes, external edits, gitignored files, symlinks,
  and files over 2 MiB are not tracked. Checkpoints are session recovery,
  not version control. `Summarize from here` is planned.

#### Hooks (v1)

Deterministic, user-supplied extensions that run in code, not in the
model. Full contract and trust model:
[threat-model.md](threat-model.md) §2.7; mechanics:
[harness-internals.md](harness-internals.md) §2.3.

- **13 events:** `SessionStart`, `UserPromptSubmit`, `PreToolUse`,
  `PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `Stop`,
  `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`,
  `SessionEnd`, `TurnFailed`.
- **Definition:** user `~/.config/meerai/hooks.json` and project
  `.meerai/hooks.json`; entries
  `{ event, matcher, if, command, baseline_file, timeout_ms, fail_closed }`.
- **Execution:** `sh -c` with JSON on stdin; exit 0 passes, exit 2 blocks
  (stderr becomes the model-visible reason), other codes pass with a
  recorded reason. JSON stdout decisions:
  `hookSpecificOutput.permissionDecision` for `PreToolUse`;
  `decision: "block"` for `Stop` (continue-the-turn-once). Hooks for one
  event run concurrently with a 10 s default timeout (60 s max); failures
  fail open unless `fail_closed`. Every firing logs `HookFired` and emits
  `HookCompleted`.
- **Trust:** project hooks run only after explicit trust (env flag, user
  setting, trusted-project list, or the one-time TUI prompt). A project
  file cannot trust itself.
- **Tighten-only:** hooks may tighten or ask, never loosen `plan`, an
  explicit `deny`/`ask` rule, or the outside-workspace approval.
  `SessionStart`/`SessionEnd` are log-only; `TurnFailed` is observational.
  Hooks run outside `ExecutionWorld`. `/reload` re-reads both files.

#### Stop hook and verification

`Stop` fires when the loop is about to end a turn naturally; a hook can
run tests/clippy and block once with a reason — a deterministic "verify
before done" gate. `meerai hooks verify [--command CMD] [--reset-baseline]`
installs a user Stop hook with a per-project failure baseline: the first
failing run records known failures, later runs block only new names, and
a pass clears the baseline. With `context.rollback_on_verify_failure` in
`auto` mode, a verify block restores the turn snapshot and logs
`RewindApplied { scope: Code }`.

#### Budgets and circuit breakers

- `max_steps_per_turn` (default 32; the final step strips tools and asks
  for a text summary), `no_progress_fail_steps` (default 12; the turn fails
  after that many write steps with no file change, with a diagnostic at two
  thirds; read-only steps do not count), per-tool timeouts, per-turn wall clock
  (`max_turn_secs`, also bounding streams, summarization, and tool
  batches), and a session token cap with a Spend approval pause
  (`Accept` raises the cap by one context window; `AcceptForSession`
  removes it; `Decline` fails the turn). USD spend caps are planned.
- Breakers: the 3rd identical failing call injects a diagnostic and the
  4th fails the turn (`LoopDetected`); the 3rd identical batch injects a
  diagnostic and the 5th fails (edit/write-only batches also log
  `CircuitBreaker { RepeatedIdenticalEdit }`); `NoProgress` diagnoses at 8
  write steps without a file change and fails at 12, disabled in `plan`
  and without a write tool, reset by delegation. A read-only step never
  increments the counter.

#### Inline guardrails

- **Secret redaction (best effort):** `redact_run` rewrites every tool run
  before spill, log, UI, and model text (9 patterns;
  [threat-model.md](threat-model.md) §2.4). The original is discarded;
  paths are not redacted; no guarantee.
- **Injection screening (heuristic):** `screen_run`/`screen_untrusted_text`
  flag override phrases and spoofed role markers (10 patterns) and wrap
  flagged text in an `<untrusted_content>` warning block. Best effort.
- **Schema validation:** arguments are normalized, coerced to the
  schemars schema, and deserialized before dispatch; invalid arguments
  return a structured `WrongArguments` observation without executing and
  are not retried. A rejected call logs a schema-based repair hint.

#### Security operations and supply chain

Path guards, sensitive-file blocks, `plan` hard read-only, approval gates,
headless fail-fast, env scrub, redaction, screening, session permissions,
retention, budgets, `ExecutionWorld`, keychain storage, `cargo-deny` CI
audit, conditional release signing, and the written threat model are
implemented. The landed/backlog split and residual risks are owned by
[threat-model.md](threat-model.md).

### 2.8 Agents and subagents

#### Markdown-defined agents and skills

Read-only discovery at the project root
(`meerai-common::definitions`): `.meerai/agents/*.md` then
`.agents/agents/*.md` then `.claude/agents/*.md`; `.meerai/skills/*/SKILL.md`
then `.agents/skills/*/SKILL.md` then `.claude/skills/*/SKILL.md`. `.meerai`
wins a duplicate id, then `.agents`, then `.claude` (warning names both
paths); output is sorted by id; symlinks are skipped; only `SKILL.md` text is
read during discovery; nothing is copied.

```markdown
<!-- .meerai/agents/reviewer.md -->
---
name: reviewer
description: Reviews changes for correctness and regressions
model: gemini-3.1-pro      # optional; a ':' model is ignored (no provider switch)
steps: 8                   # optional; note-only for a primary agent
mode: subagent             # all (default) | primary | subagent
permissions:               # optional; applied to a subagent, tighten-only
  - { action: edit, resource: "*", effect: deny }
---
Review the current changes. List findings by severity with file:line references.
```

- Frontmatter: `name`, `description`, `model`, `steps`, `mode`,
  `permissions`, and `disable-model-invocation`. `mode` filters selection:
  `primary` hides the agent from `--agent` and the pickers; `subagent` hides it
  from those and offers it to `subagent-run`; unknown values warn and become
  `all`. `permissions` become `PermissionRule`s on the child policy after the
  parent rules; `allow` entries are dropped with a warning and an invalid block
  is dropped. `tools` and `allowed-tools` warn and are not applied. Invalid
  frontmatter warns and skips; missing frontmatter uses the file/directory name
  and the body.
- Skills load progressively. The system prompt appends a `## Skills` block with
  `<id>: <description>` for each skill that has a description and does not set
  `disable-model-invocation`, capped at 8 KiB and screened as untrusted. The
  `skill-load` tool returns the `SKILL.md` body and the supporting file names;
  a second call with `file` returns one file. Reads stay inside the skill
  directory, reject path separators and symlinks, and cap text at 16 KiB. The
  tool is registered only when at least one skill is advertised.
- Explicit application: startup `--agent <ID>` (one id; repeating is an error)
  and repeatable `--skill <ID>` (skills before the agent) append a body to the
  system prompt, capped at 16 KiB per definition and 32 KiB total. TUI
  `/agent`/`/skill` apply a System instruction at the next step boundary; the
  `@` Mentions popup applies definitions before the prompt
  ([tui-design.md](tui-design.md) §2.6).
- Model precedence: `--model` → `MEERAI_MODEL` → agent `model` →
  settings/provider default. A `:` model is ignored with a warning;
  `steps` is note-only for a primary agent, and `mode` filters selection.
- Definitions are untrusted repository prompt text, screened before insertion.
  They grant no tools, scripts, MCP, or sandbox. An agent `permissions` block
  tightens a subagent's policy; `allow` entries are dropped. Full tool and
  permission mapping from definitions is planned (§3).
- Two skills are built in (`DefinitionSource::Builtin`): `goal` and
  `orchestrate`, merged by `meerai-skills::builtin`. A user, pack, or project
  skill with either id shadows the built-in. Both drive one four-phase loop
  with five Read-classified tools: `loop-plan` stores the objective, done
  condition, constraints, and items; `loop-act` records one action and its
  expected observation; `loop-observe` records the raw evidence; `loop-adjust`
  records done, open, or failed and may add items; `loop-status` re-reads the
  state after compaction or resume. `LoopInterceptor::turn_stopping` continues
  the turn while an item is open, up to 24 rounds. The TUI and browser accept
  `@goal` as an alias for `@skill/goal`.
- The loop spans turns. After a turn ends with an open item (`Ok` or
  `ErrorCode::Budget`), the worker logs `SessionEvent::LoopUpdated`, emits
  `AgentEvent::LoopUpdated` and `AgentEvent::LoopContinued`, resets the round
  count, and pushes one continuation prompt at the front of the queue. It caps
  the automatic turns at 4. `Cancel`, `Quit`, `Reset`, a queued recovery turn,
  and headless one-shots opt out.
- `SessionEvent::LoopUpdated` is durable but not model-visible.
  `meerai_session::last_loop` restores it in `build_agent` and in host
  `register_opened`, so the model state and the panel survive `--resume`.
  Legacy `SessionEvent::GoalUpdated` is still read and mapped to `LoopState`.
- `AgentEvent::LoopUpdated` is the live signal. The TUI stores it on the tab;
  the host stores it on the session record, exposes it in the session detail,
  and rebroadcasts it as `RegistryEventKind::LoopUpdated` /
  `ViewEvent::LoopUpdated`. `/loop` (or `/goal`) opens a read-only panel in
  both clients. The live loop state lives with the agent; the durable event is
  the only copy outside a running process.

#### Subagent execution

A subagent run is a **child session** (own event log,
`parent_session_id`) with its own `MultiTurnAgent`, system prompt, model,
permission set, and tool subset, exposed as one `subagent-run` tool.

- Built-ins: `explore` (4 steps, read-only), `plan` (16 steps, read-only),
  `build` (32 steps, write), `general` (16 steps, write). Read-only
  profiles use a fixed tool allowlist and forced `ApprovalMode::Plan`;
  step budgets clamp to the parent's. Custom definitions run write-capable
  (16 steps) and are offered only when they are `mode: all` or
  `mode: subagent`; their `permissions` join the child policy as agent rules,
  so a definition can deny or ask but never allow.
- Depth-1 isolation: `ChildToolsetFilter` rejects `subagent-*` tools;
  `question-ask` and `acp-run` are top-level only.
- The child runs in background; the parent receives
  `ItemStarted { SubagentRun }` → report on idle → `ItemCompleted`. Only
  the final report enters the parent's context — the isolation that makes
  search subagents token-efficient. The TUI opens the child's own full
  conversation in the agent overlay from its log without affecting the row
  state; the runs dialog reads the same log.
- The tool accepts an optional `name` for the run. It is carried on
  `SubagentSpec` and `SessionItem::SubagentRun`, shown in the transcript
  header, runs list, detail title, and browser card, and recovered from the
  tool-call arguments on replay. `acp-run` accepts the same argument.
- Child approvals forward to the parent overlay; usage rolls up into
  parent counters; the parent token cap propagates; children inherit the
  parent's `ExecutionWorld` and run without a turn clock. A child
  `max_tokens` request floors at 50,000, then clamps to the parent's
  remaining budget.
- The delegated `acp-run` tool sends one task to an external `agy` child
  session; it has no token/grant rollup ([acp.md](acp.md) §2.8).

### 2.9 Structured errors and retries

- Provider and tool errors map to typed observations
  `{ status, exit_code, stdout, stderr, hint }`; raw stack traces never
  reach the model unstructured.
- Transient provider failures retry 3 attempts total with 250/500 ms
  backoff plus up to 50% jitter. Classification uses the HTTP status when
  genai exposes one (408/409/425/429 and 5xx transient; other 4xx
  permanent); a message blocklist is the fallback. Tool retries use
  `max_retries` (default 3) with a fixed 250 ms sleep; `WrongArguments`
  and `InvalidFunctionName` are not retried.
- Stream requests carry a 30 s header timeout and a 120 s per-chunk read
  timeout (heartbeats reset the read timer). A silent stall or a fully
  empty stream is a transient failure and retries; a stream with usage
  but no content is a normal end. An empty stream reports a diagnostic
  block with the provider, model, stop reason, event count, elapsed time,
  and a hint. A stop reason of `content_filter`, `max_tokens`, or
  `stop_sequence` is a non-retryable provider error, and captured
  reasoning counts as content.
- Context overflow routes to compaction and retries once, only when
  compaction applied.
- The worker queues one recovery turn after a retryable provider or
  `LoopDetected` failure; the prompt names the failure and asks for a
  different approach. Budget, config, tool, and non-retryable failures do
  not recover. Headless runs wait for the recovery turn. Deeper recovery
  (budgets, escalation chains) is planned
  ([token-efficiency.md](token-efficiency.md) §3).

### 2.10 Retrieval and code intelligence (non-vector)

| Stage | Capability | Status |
| --- | --- | --- |
| v1 | `read_file(offset,limit)`, `grep`, `glob`, `list_dir`; spill stubs keep big hits cheap | Implemented |
| v1 | Hierarchical `AGENTS.md`: root in the prefix, per-directory files injected as ephemera when touched | Implemented |
| v2 | `explore` subagent: parallel read-only calls, spans-only report | Implemented |
| v2 | AST repo map: `retrieval-repo_map`, tree-sitter (Rust, C, C++, C#, Java, Kotlin, Scala, Swift, PHP, Haskell, Ruby, TS/TSX/JS/Python/Go) + personalized PageRank, ~1500-token budget (200..4000) | Implemented |
| v3 | LSP tools: `lsp-go_to_definition`, `lsp-find_references`, `lsp-get_diagnostics` over stdio JSON-RPC (`rust-analyzer`, `typescript-language-server`, `gopls`, `dart language-server --protocol=lsp`, `pyright-langserver`); opt-in post-edit diagnostics (`context.post_edit_diagnostics`) | Implemented |
| v3 | Search-as-Code: `sac-run_script` runs `python3 -I -B` / hardened Node for multi-step analysis; output spilled like any tool result | Implemented |

Rationale: vector embeddings underperform on live code — exact-identifier
confusion, chunk fragmentation, intra-turn staleness, no structural
awareness. Retrieval is lexical + structural; embeddings are not planned.

**Native parallel tool arrays:** the loop executes all tool calls per
step; consecutive read-only calls run concurrently via `join_all`, a
mutating call breaks the batch and runs alone, and `ToolOutput::Stop`
halts the remaining batch (later calls marked `Cancelled`).

### 2.11 Observability

- **The log is the trace.** `replay_from_events` produces the human
  projection; `derive_messages` purity lets an offline runner replay any
  logged session against a new model/harness version. `meerai export` is
  planned (§3).
- **Usage and cost:** `UsageUpdated` carries per-step and cumulative
  usage with cache counts; `est_cost_usd` is computed when the catalog
  has both per-million prices, otherwise `None`. Headless prints a
  cumulative token summary; the TUI shows turn and session totals.
- **Telemetry seam:** `HookFired`, `UsageUpdated`, `CircuitBreaker`, and
  error events give the full cost/behavior picture without extra
  instrumentation.
- Dual-agent verification (a read-only `reviewer` subagent inspecting the
  diff before completion) is possible with the subagent runtime today but
  is not wired as an automatic gate (§3).

### 2.12 ACP client (external harnesses)

`meerai --acp` drives `agy acp` as an external engine; `AcpWorker` maps
typed ACP events into `AgentEvent` and mirrors the session log
(`harness: acp:agy`, `remote_session_id`); `--continue`/`--resume` use
`session/load`; `acp doctor` prints the handshake; the native agent can
delegate one task via `acp-run`. Permissions are advisory: meerai policy,
path guards, redaction, and the sandbox do not apply to the external
process, and `plan` refuses ACP. Full specification and trust boundary:
[acp.md](acp.md).

## 3. Planned

| Item | Description | Gate |
| --- | --- | --- |
| Progressive tool disclosure | Admit a compact tool index plus a `load_tool_schema` meta-tool when admitted schemas exceed ~2k tokens (MCP, skills growth) | Tool count growth |
| `meerai export` | Export a session log for evals and audit (`--json`) | CLI milestone |
| TUI polish | Select mode, Space leader, `:` command mode, transcript search, `Summarize from here`, inline viewport | [tui-design.md](tui-design.md) §3 |
| Definition mapping | Map definition `tools`/`permissions` and MCP servers | Security review |
| M14 — `meerai acp` | meerai as an ACP agent for editors | [acp.md](acp.md) §3.1 |
| ACP presets | User-config preset catalog (`acp.yml`), OpenCode/Gemini/Claude/Codex adapters | [acp.md](acp.md) §3 |
| M15 — OpenAI plan usage | In-TUI sign-in modal, multi-account switching, live provider spike | [openai-chatgpt-plan-usage.md](openai-chatgpt-plan-usage.md) |
| Browser client polish | Delta coalescing, definition chips, ACP badge, browser automation, and an accessibility audit; the host, protocol, server, and Leptos client are implemented | [Browser UI](browser-ui.md) §3 |
| Incremental projection | Tail-incremental `derive_messages`, frozen/shared messages, stable multi-section prefix | Projection purity |
| Recovery depth | Per-turn recovery budget, model escalation, verifier-gated retry ladder | [token-efficiency.md](token-efficiency.md) §3 |
| Dual-agent verification gate | Automatic read-only reviewer pass before completion on high-risk deliverables | Eval evidence |
| USD spend caps | Cost-based session caps once catalog pricing is trusted | Pricing metadata |
| Native `Plan` producer | A plan item produced by the native loop (ACP already emits plans) | Loop design |
| Windows sandbox parity | Job Objects / AppContainer; today Windows uses host guards | Platform work |
| Release signing | Populate signing secrets so published binaries are signed/notarized | Operator configuration |
| Harness growth | Compile recurring model mistake classes into harness-side pre-checks from trace review | Trace review |

### Non-goals

- Dynamic hot-swappable plugin runtimes: Rust composition at startup,
  hooks, and markdown definitions cover the extensibility need.
- Vector databases / embedding indexes (§2.10 rationale).
- Desktop GUIs, remote or multi-user browser hosting, actor frameworks, and bot integrations. The loopback browser companion is scoped in [browser-ui.md](browser-ui.md).
- Destructive history rewrites: rewind and compaction shadow, never
  delete.

## 4. Reference

### 4.1 Milestones

Each milestone keeps `fmt → clippy → test` green and ships something
user-visible. M1–M2 established the two load-bearing patterns (log,
protocol) before features accreted; everything after is projection rules,
policies, or tools on the same spine.

| # | Milestone | Content | Status |
| --- | --- | --- | --- |
| M1 | Session log core | `SessionEvent`, SQLite persistence, `derive_messages` + snapshots, crash recovery, meta | Implemented |
| M2 | Item protocol | `AgentEvent` lifecycle; TUI fold; headless NDJSON | Implemented |
| M3 | Coding tools + effect classes | Seven `code-*` tools, path guards, unified-diff `FileChange` items | Implemented |
| M4 | Policy engine + approvals | Ordered rules, approval suspend/resume, TUI overlay, grants | Implemented |
| M5 | Context engine v1 | Spill, ephemera/reasoning pruning, age-based clearing, watermark compaction with bracket lock, cache-safe summaries | Implemented (multi-section prefix planned) |
| D1 | Distribution hardening | Apache-2.0, CI gate, tag-driven 5-target release, Pages installers | Implemented |
| M6 | Checkpoints + rewind | Pre-write capture, `/rewind` menu, staged restore with rollback | Implemented v1 (shell edits, gitignored files, summarize-from-here planned) |
| M7 | Hooks | 13-event set, JSON stdin/exit-2 contract, trust gate, concurrent timeouts, `HookFired`, tighten-only integration | Implemented v1 |
| M8 | Subagent runtime | Child sessions, depth-1 isolation, built-ins, forwarded approvals, usage rollup, `SubagentRun` items | Implemented |
| M9 | CLI + resume/fork | `meerai-cli`, `--prompt`/`--json`/`--continue`/`--resume`/`--fork`, `sessions fork`, auth/sessions | Implemented (`export` planned; TUI `/fork` implemented) |
| M10 | Retrieval v2/v3 | `repo_map`, LSP stdio tools, `search_as_code`, opt-in post-edit diagnostics | Implemented |
| M11 | Security & supply chain | Guards, sensitive blocks, sandbox, keychain, redaction, screening, budgets, retention, CI audit, threat model, conditional signing | Implemented (signing secrets operator-configured; Windows host guards) |
| M12 | Agents & skills discovery | Read-only `.agents`/`.claude` discovery, progressive skill loading via `skill-load`, mentions | Implemented (tool/permission mapping planned) |
| M13 | ACP client | `meerai-acp` bridge, `AcpWorker`, `--acp`/`/acp`/`acp doctor`, `session/load` resume, `acp-run` | Implemented for `agy` (other presets planned) |
| M14 | ACP agent | `meerai acp` on stdio for editors | Planned (compatibility kit + editor smoke) |
| M15 | OpenAI plan usage | Sign in with ChatGPT (`chatgpt.tokens.use.direct`), plan request profile, account model catalog | Implemented (live provider spike and multi-account planned) |

### 4.2 Recorded decisions

- **`EffectClass` declaration seam.** Declared via defaulted
  `Toolset::effect_of` + `invoke_structured`, not a `ToolDefinition`
  field, so macro codegen and its snapshots stay untouched.
- **Headless flag.** `-p` means `--provider`; headless input is
  `--prompt`/`--print` (+ `--json`).
- **Token accounting and cost.** `ProviderCatalog` carries optional
  per-model metadata; the catalog supplies defaults only, settings win,
  and pricing is never invented in the bundled catalog. Budgets use a
  ~4-chars/token heuristic.
- **Rewind is the only user-facing undo.** No `Revert` command or event;
  one `RewindApplied` per rewind; `Both` runs code then conversation.
- **Questions are not approvals.** Reusing approvals would conflate a
  permission with a product choice, pollute grants, and break headless
  exit codes.
- **Step interceptor seam.** In-loop extensions use `Interceptor`;
  checkpoints are the built-in interceptor; hooks use `HookEngine`
  because they await host processes outside the loop.
- **Build surfaces.** `meerai-tui` defaults `update`; `meerai-cli`
  defaults `tui`; `--no-default-features` builds headless without
  ratatui/git2/comrak/TUI.
- **Persistence.** Sessions, events, the FTS5 index, spilled blobs, and
  checkpoints live in SQLite (`meerai.db`); only `session.lock` remains in the
  per-session directory. `meerai sessions export` prints JSONL.
- **`plan` is hard read-only.** Non-Read denied before rules and grants;
  `auto` honors explicit `ask`/`deny`.
- **Headless exit contract.** Tool approval: fail fast, exit `2`. Spend:
  decline with the `max_session_tokens` remedy, exit `1`. Completed `0`;
  other failures `1`.
- **Approval scope.** `AcceptForSession` → Session grant; TUI `t` →
  `Some(Turn)` cleared at turn end; grants are literal.
- **`.env` is opt-in.** `MEERAI_LOAD_DOTENV` or `--env-file`; nothing
  loads implicitly.
- **Windows is supported.** `cmd /C`, x64 release target, no file-mode
  enforcement.
- **Session privacy and management.** `0700`/`0600` on Unix;
  `sessions list|rm|prune|fork` and `--continue`/`--resume` are the
  management surface; retention is manual by default and user-controlled;
  the active session is never pruned.
- **Project config is tighten-only and cannot redirect.** Root discovery
  walks up from the CWD; tools and sessions keep the CWD as the workspace.
- **Redaction and screening are best effort.** No entropy screen, no
  guarantee; the original secret is discarded; paths are not redacted.
- **Session titles and summarizer.** First prompt, single line, 80 chars,
  no retitle; `summarizer_model` overrides compaction and its usage counts
  toward the cap.
- **Provider retries and turn clock.** 3 attempts, transient-only,
  jittered; the turn clock bounds streams, summarization, and tool
  batches.
- **Read-only concurrency.** Consecutive Read calls run concurrently;
  `Stop` halts the batch.
- **Definitions merge at runtime, with no copies.** `.agents` before
  `.claude`; no `.meerai` definition directories, no import lock;
  definitions are untrusted text that cannot grant capabilities; agent models
  cannot switch provider; skill reads stay inside the skill directory.
- **Prompt composition.** Base + workspace root line + `--system` + root
  instructions (32 KiB/file, 64 KiB block); scoped `AGENTS.md` once per
  turn (16 KiB, max 8); an `Active model:` identity line at request time with
  the bare model id, not the provider adapter prefix.
- **ACP is not a safety boundary.** External agents keep their own tools
  and policy; permissions are advisory; `plan` refuses ACP.

### 4.3 Verification protocol

Per [.agents/repo/profile.md](../.agents/repo/profile.md) §7: the
repo-wide gate is `cargo fmt --all -- --check && cargo clippy
--workspace --all-targets && cargo test --workspace` (or `make verify`).
CI runs the same gate on `ubuntu-24.04` plus both reduced-feature checks
and a `cargo-deny` audit. A subagent's "done" is a claim until the
consolidated gate passes.

### 4.4 References

Primary sources consulted (Nov 2026):

- Codex app-server protocol — developers.openai.com/codex/app-server
  (Thread/Turn/Item, deltas, approvals, fork/resume/compact, SQLite
  rollouts).
- Codex sandboxing & approvals — developers.openai.com/codex/sandboxing
  (read-only / workspace-write / danger-full-access; Seatbelt,
  bubblewrap; sandbox ≠ approval policy).
- Codex hooks — developers.openai.com/codex/hooks (event set, matchers,
  trust review).
- OpenCode agents — opencode.ai/docs/agents (markdown agents, ordered
  permission rules, built-in profiles, child sessions).
- OpenCode server & sessions — opencode.ai/docs/server;
  opencode.ai/docs/snapshots.
- DeepSeek Harness — github.com/deepseek-ai/deepseek-harness
  (append-only log, deriveMessages purity, surface replacements, spill
  head/tail + recovery notice, compaction bracket lock, fork-at-boundary).
- Claude Code hooks — code.claude.com/docs/en/hooks (lifecycle events,
  exit-code-2 blocking, JSON decisions).
- Claude Code checkpointing — code.claude.com/docs/en/checkpointing
  (per-prompt snapshots, 100-checkpoint retention, /rewind).
- Li et al., *Grow the Harness, Not the Context*, arXiv:2609.26760.
- Cognition, *Introducing SWE-grep and SWE-grep-mini* (cognition.com/blog).
- Aider repo maps — aider.chat/docs/repomap.html (tree-sitter tags +
  PageRank within a token budget).
- ACP — agentclientprotocol.com (see [acp.md](acp.md) §7).
