# meerai Harness Specification

Unified architecture, techniques, and specifications for the `meerai` coding-agent
harness. This document supersedes the former "Optimized Coding Agent Harness
Architecture & Context Engineering Specification" and folds its context-engineering
content into a concrete design for this workspace. It complements
[architecture.md](architecture.md) (crate layout, protocol basics) and
[tui-design.md](tui-design.md) (modal terminal UI).

The design is grounded in primary-source research of four production harnesses:

| Harness | Runtime | Standout pattern | What meerai borrows |
| --- | --- | --- | --- |
| **Codex** (`openai/codex`, `codex app-server`) | Rust | Typed Thread → Turn → Item JSON-RPC protocol; OS-kernel sandboxing (Seatbelt / bubblewrap+Landlock) | Typed item lifecycle (`started` / `delta` / `completed`) for harness↔UI decoupling; sandbox-mode × approval-policy split |
| **OpenCode** (`opencode serve`) | TypeScript | OpenAPI server; Markdown-defined agents with ordered permission rules; session forking; per-step snapshots | `.agents/agents/*.md` and `.claude/agents/*.md` definitions applied as prompt instructions (M12); `.agents`/`.claude` skills; `allow/ask/deny` rule engine; message-level fork & undo |
| **DeepSeek Harness** (`deepseek-ai/deepseek-harness`, "dsh") | TypeScript (Cordis plugins) | Zero-privileged-core plugin tree; append-only `SessionEvent` log with pure message projections | **Model-visible means logged**: one append-only event log, `derive_messages()` as the single pure projection, context techniques as projection rules |
| **Claude Code** | TypeScript / Node CLI | ~30 deterministic lifecycle hooks; automatic per-prompt file checkpoints with `/rewind` | `PreToolUse`/`PostToolUse` deterministic command hooks (JSON stdin, exit-code-2 block); per-turn file snapshots + restore |

Empirical support for "grow the harness, not the context": *Growing Harness*
(arXiv:2609.26760) compiles recurring control decisions into executable harness code
and reports 76–92% fewer LLM calls and 74–99% lower inference cost. Cognition's
SWE-grep line shows RL-tuned search subagents returning only `file:start-end` spans
materially cut retrieval cost. Both patterns are adopted in §12.

Implementation status is tracked in §15.1. Landed: the session log and
projection (§3), `AgentEvent` v2 (§4), the ordered step interceptor seam (§5),
coding tools and retrieval v1–v3 (§12), policy and approvals (§8), the context
engine v1 (§7), OS sandboxing (§8.3), the subagent runtime (§10.2), CLI forking
(§13), user questions/choices (§4.1), M11 security (§9.6), M12 definitions
(§10.1), and per-model context and pricing metadata in the provider catalog
(§7.1). Checkpoints/rewind (§9.1, v1) are
landed. Still targets: hooks (§9.2, including post-edit LSP diagnostics),
progressive tool disclosure (§7.7), `meerai export` (§13), definition
tool/permission mapping, and ACP (§15.1 M13/M14). Prompt-injection screening and secret redaction ship as
best-effort heuristics (§9.5).

---

## 1. Design principles

**P1 — The event log is the single source of truth (dsh).**
Every session is an append-only log of durable events. The model transcript,
the TUI transcript, telemetry, checkpoints metadata, and exports are all
*projections* of that log. Nothing that the model sees may exist outside the log
("model-visible means logged"), so any request is exactly reconstructable and
replayable offline.

**P2 — Typed items decouple harness from UI (Codex).**
The harness emits a typed `Session → Turn → Item` event protocol. Every item has
an explicit lifecycle: `Started` (full initial payload) → zero or more typed
`Delta`s → `Completed` (authoritative final payload). The TUI, the headless
printer, and any future server/SDK consume the same protocol. UI state is a fold
over item events; no UI logic leaks into the agent loop.

**P3 — Deterministic control lives in the harness (Claude Code, arXiv:2609.26760).**
Anything that can be decided without the model is decided in code: permission
rules, hooks, budgets, circuit breakers, schema validation, spill/compaction
triggers, checkpointing, retries. The model is a stateless CPU; the harness is
the virtual-memory OS that manages context allocation, scheduling, privilege
boundaries, and persistence. Recurring model-driven routines should be migrated
into harness primitives over time.

**P4 — Context is a scarce, budgeted resource (source spec).**
Working context is *compiled* per request from the log under explicit token
budgets (spill stubs, ephemera stripping, reasoning pruning, tiered compaction,
cache-friendly prefix ordering). Storage history keeps everything; working
context keeps only what the next inference step needs.

---

## 2. Layered architecture

The diagram below is the **target** layered view. Crate boundaries exist now;
the hook runner and checkpoint service boxes do not. The sandbox and
`meerai-cli` boxes landed in M9/M11. Landing status is tracked in §15.1.

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ Clients: meerai-tui (modal TUI) │ headless printer (--prompt, --json) │ SDK │
└───────────────▲──────────────────────────────────────────────────────────┘
                │  AgentEvent stream (typed items: Started/Delta/Completed)
┌───────────────┴──────────────────────────────────────────────────────────┐
│ meerai-agents — Turn/Step loop                                           │
│   interception points (Rust "waterfalls"):                               │
│     pre_step → build_request → stream → tool pre/post-execute → turn_end │
│   compaction triggers · circuit breakers · cancellation · retries        │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-session — append-only SessionEvent log                            │
│   JSONL+zstd persistence · derive_messages() projection · fork · replay  │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-policy — permissions · sandbox modes · hooks · budgets            │
│   ordered allow/ask/deny rules · approval flow · checkpoint service      │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-tools (code) — read/write/edit/list/grep/glob/shell · diffs       │
│   toolsets declare EffectClass (Read/Write/Execute) + risk tier          │
├──────────────────────────────────────────────────────────────────────────┤
│ meerai-core — Toolset · ToolOutput · ToolObservation · FileDiff (genai)   │
│ meerai-common — settings · ProviderCatalog · SecretStore · genai client    │
└──────────────────────────────────────────────────────────────────────────┘
```

Crate deltas relative to [architecture.md](architecture.md):

| Crate | Change |
| --- | --- |
| `meerai-session` | Session event log, persistence, projections, forking, replay, blob (spill) store |
| `meerai-policy` | Permission rule engine, approval modes, grants; sandbox adapters, hook runner, and checkpoint service are target-only |
| `meerai-agents` | Turn/Step loop rebuilt around the log + interception points; `AgentEvent` v2 (item protocol) |
| `meerai-core` | `EffectClass` on tool definitions; structured tool observations; unchanged macros |
| `meerai-tui` | Consume item protocol; approval overlay; undo/rewind UI; hooks visibility |
| `meerai-cli` (planned) | Headless `--prompt`/`--print`, `--json` NDJSON item stream, `--continue`/`--resume`; `-p` stays the provider flag |

---

## 3. Session event log (`meerai-session`)

Adopted pattern: dsh `core/session` — append-only log, surface vs. log-only
events, pure projections, versioned JSONL persistence.

### 3.1 Event vocabulary

```rust
pub enum SessionEvent {
    // structure (log-only, never projected into messages[])
    TurnStarted  { turn_id: TurnId, trigger: TurnTrigger },
    TurnEnded    { turn_id: TurnId, reason: TurnEndReason },
    StepStarted  { step_id: StepId, turn_id: TurnId },
    StepEnded    { step_id: StepId, finish: StepFinish },

    // model-visible surface events (projected by derive_messages)
    SystemMessage    { sections: Vec<PromptSection>, generation: u64 },
    UserMessage      { source: InputSource, content: Vec<ContentBlock> },
    AssistantMessage { provider: String, model: String, content: Vec<ContentBlock>,
                       tool_calls: Vec<ToolCall>, usage: Usage, stream_digest: StreamDigest },
    AssistantAttempt { /* failed/retried/cancelled attempt; log-only, keeps usage */ },
    ToolCall         { call_id: CallId, name: String, args: String },
    ToolResult       { call_id: CallId, output: ToolObservation },

    // context-engine events (log-only; mutate the surface via replacements)
    SpillApplied   { call_id: CallId, bytes_omitted: u64, blob_path: PathBuf },
    CompactionStarted { compaction_id: CompactionId, trigger: CompactionTrigger },
    CompactionSummary { compaction_id: CompactionId, replaced: SeqRange, summary: String },
    CompactionEnded   { compaction_id: CompactionId, result: CompactionResult },

    // harness bookkeeping (log-only)
    UsageUpdated   { usage: Usage, cumulative: Usage },
    ApprovalRequested { request_id: RequestId, kind: ApprovalKind, preview: ApprovalPreview },
    ApprovalDecided   { request_id: RequestId, decision: ApprovalDecision, scope: GrantScope },
    HookFired      { event: HookEventName, decision: HookDecision },
    CheckpointCreated { checkpoint_id: CheckpointId, turn_id: TurnId, paths: Vec<PathBuf> },
    CircuitBreaker { kind: BreakerKind, action: BreakerAction },
    Error          { code: ErrorCode, message: String, retryable: bool },
}
```

Rules:

- **Durable vs. live.** Only facts that must survive a restart are log events.
  Per-token streaming deltas are *live* events (`AgentEvent`, §4) and are never
  logged individually; the settled `AssistantMessage` embeds a compact
  `StreamDigest` (final text, timing, usage) so replay reproduces the transcript.
- **Structured turn-end reasons** (dsh `TurnEndReasonMap`):
  `Completed | Aborted { cause } | Failed { error } | MaxTokens | Interrupted`
  (crash-orphans closed on recovery) `| Forked` (seed marker only).
- **Tool results are structured observations** (source spec §2.5):
  `ToolObservation { status, exit_code: Option<i32>, stdout, stderr, hint: Option<String> }`
  rendered deterministically into the tool-role message.
- **Every model-visible input is logged.** Injected runtime context (file trees,
  reminders, scoped `AGENTS.md` fragments) is logged as `UserMessage` with a
  typed non-`user` `InputSource` so it can be stripped or refreshed by
  projection rules (§7).

### 3.2 Persistence

```text
~/.local/share/meerai/sessions/<session-id>/     # Linux; macOS uses dirs::data_dir()
├── session.v<N>.jsonl         # active append-only log (plain, one JSON object per line)
├── session.v<N>.jsonl.zstd    # sealed older generations (zstd, write-once)
├── blobs/<hash>-<len>.txt     # spilled tool outputs (§7.2)
├── snapshots/<turn-id>/       # planned only; checkpoints (M6), not created today
└── meta.json                  # title, cwd, git branch/sha, model, created/updated
```

- The active generation is plain JSONL: crash-resilient at line granularity and
  easy to tail. Rotation seals the active file to `.jsonl.zstd`; committed
  generations are never rewritten. A format change writes a new generation
  through an adjacent `vN → vN+1` migration.
- The root is `dirs::data_dir()/meerai/sessions` (on macOS
  `~/Library/Application Support/meerai/sessions`), or `MEERAI_SESSION_DIR`.
  `SessionConfig { project_local: true }` uses `<workspace>/.meerai/sessions`;
  the current binary uses the global root.
- On Unix, session directories are `0700` and session files are `0600`
  (`meerai-session/src/perms.rs`); Windows has no equivalent enforcement.
- Crash recovery: on open, an unsealed trailing turn gets a synthesized
  `TurnEnded { Interrupted }` closer, and an open compaction bracket gets a
  recovered `CompactionEnded`; events before the crash stay intact.

### 3.3 Projection: `derive_messages()`

The single pure function from log to `Vec<ChatMessage>` (genai):

```rust
pub fn derive_messages(log: &[SessionEvent], policy: &ProjectionPolicy) -> Vec<ChatMessage>;
```

- Pure, deterministic, snapshot-tested (insta). External reconstructors
  (evals, exporters) must produce byte-identical transcripts.
- `ProjectionPolicy` carries the context-engine knobs (§7): spill budget,
  ephemera stripping, reasoning pruning, compaction replacements, verbatim
  window size `K`.
- Recomputed from the full event vector per request. Incremental caching
  (pure tail growth in O(new events), rebuild from a replacement point) is
  **not implemented**; it is a future optimization.
- Messages are cloned into the request; there is no deep-frozen/shared
  representation yet. History is never mutated because the log is append-only,
  not because of a frozen type.
- Skips empty-content assistant messages; failed attempts project as nothing
  but keep their usage in accounting.

### 3.4 Forking & branching

- `fork(source, boundary)` copies log events through an inclusive `turn-end`
  boundary into a new session id and records `forked_from` +
  `inherited_event_count` in `meta.json`. A prefix whose boundary turn is still
  open (or unknown) is rejected, not silently clipped (dsh rule). The
  `TurnEndReason::Forked` variant exists but fork does **not** write it; a fork
  always stops at a closed turn, so no closer is needed.
- Powers (landed in part): `--fork <ID>` and `sessions fork <ID> [--turn
  <ID>]` fork at a closed turn, and the TUI message menu forks a session and
  seeds the new composer. Message-level arbitrary fork, subagent seeding (§10),
  and eval branch runs remain targets. There is no `/fork` or `/rewind`
  command.
- `rewind(turn_id, scope)` — user-facing rewind: log a `TurnEnded { Aborted }` for the
  open turn and treat later turns as shadowed (kept in the log, excluded from
  the surface by a replacement range), never deleted.

---

## 4. Typed item protocol (`AgentEvent` v2)

Adopted pattern: Codex app-server `item/started` → `item/*/delta` →
`item/completed`, where `completed` is authoritative.

```rust
pub enum AgentEvent {
    TurnStarted   { turn_id: TurnId, model: String },
    ItemStarted   { item: SessionItem },                 // full initial payload
    ItemDelta     { item_id: ItemId, delta: ItemDelta }, // typed incremental updates
    ItemCompleted { item: SessionItem },                 // authoritative final state
    ApprovalRequested { request: ApprovalRequest },      // needs a client decision
    ApprovalResolved  { request_id: RequestId },
    QuestionAsked     { request: QuestionRequest },      // needs a client choice
    QuestionResolved  { request_id: RequestId },
    UsageUpdated  { usage: Usage, cumulative: Usage, est_cost_usd: Option<f64> },
    TurnCompleted { turn_id: TurnId, reason: TurnEndReason },
    Error         { code: ErrorCode, message: String },
}

pub enum SessionItem {
    UserMessage    { id: ItemId, content: Vec<ContentBlock> },
    AgentMessage   { id: ItemId, text: String },
    Reasoning      { id: ItemId, summary: String },
    ToolCall       { id: ItemId, call_id: CallId, name: String, args: String,
                     effect: EffectClass, status: ItemStatus },
    CommandRun     { id: ItemId, command: String, cwd: PathBuf,
                     aggregated_output: String, exit_code: Option<i32>,
                     duration_ms: u64, status: ItemStatus },
    FileChange     { id: ItemId, changes: Vec<FileDiff>, status: ItemStatus }, // {path, kind, unified diff}
    SubagentRun    { id: ItemId, agent: String, status: ItemStatus, report: Option<String> },
    Compaction     { id: ItemId, trigger: CompactionTrigger, freed_tokens: u64 },
    Plan           { id: ItemId, steps: Vec<PlanStep> },   // { text, pending|inProgress|completed }
}

pub enum ItemDelta {
    AgentText(String), ReasoningSummary(String), CommandOutput(String),
    PlanUpdated(Vec<PlanStep>), StatusChanged(ItemStatus),
}
```

Migration from today's events (`meerai-agents/src/events.rs`,
`meerai-tui/src/app.rs::handle_agent_event`):

| Current | v2 |
| --- | --- |
| `TextDelta(s)` | `ItemDelta::AgentText` against the open `AgentMessage` item |
| `ReasoningDelta(s)` | `ItemDelta::ReasoningSummary` |
| `ToolCallProposed` | `ItemStarted { ToolCall, status: Proposed }` |
| `ToolStarted` / `ToolFinished` | `ItemDelta::StatusChanged` / `ItemCompleted { ToolCall | CommandRun | FileChange }` |
| `TurnFinished` | `TurnCompleted { reason }` |
| `Cancelled` | `TurnCompleted { reason: Aborted { user } }` |

Contract rules:

1. Every `ItemStarted.id` is unique per turn; every delta references it; the
   final `ItemCompleted` payload — not the concatenated deltas — is
   authoritative for state (Codex rule; avoids drift on re-streams).
2. `ApprovalRequested` is a *request*: the loop suspends the item until the
   client answers via `AgentCommand::Approve { request_id, decision, scope }`.
   Decisions: `Accept | AcceptForSession | Decline | DeclineWithReason(String) | Cancel`.
   `scope: Option<GrantScope>` carries a `Turn | Session` grant. `AcceptForSession`
   always maps to a Session grant; the TUI `t` key sends `Accept` with
   `Some(Turn)`, and `y`/Enter sends `None`. Turn grants clear at turn end.
   Grant resources match literally; rule resources use globs. A
   `ToolCall` item moves `Proposed` → `Pending` (awaiting approval) →
   `InProgress` (authorized) → a terminal status.
3. Headless mode serializes the same enum as NDJSON (`--json`); the TUI folds it
   into `AppState`. One protocol, three clients (TUI, headless, future server).
4. `AgentCommand` stays the client→loop channel: `SubmitPrompt`, `Steer`
   (append input to the in-flight turn, dsh inbox / Codex `turn/steer`),
   `Approve`, `AnswerQuestion`, `Rewind`, `SetModel`, `SetPermissionMode`,
   `Compact`, `Cancel`, `Reset`, `Quit`. `Fork` is **not** a variant; fork UX is
   future work (M9). There is no `Revert` command: `/rewind` sends `Rewind`, and
   the conversation-only path is an internal `RewindScope::Conversation`.
   `spawn_worker` handles `SetPermissionMode` and `Compact` even while no turn
   is running, so idle `/mode` and `/compact` take effect.
5. `QuestionAsked` is an interaction request, not a permission. The loop
   suspends the `question-ask` tool call until the client answers via
   `AgentCommand::AnswerQuestion { request_id, answers }`, where `answers: None`
   means cancelled. A question never consults the policy engine and never
   records a grant.

A future `meerai serve` (optional milestone) wraps this protocol in JSON-RPC over
stdio like Codex app-server, or OpenAPI+SSE like OpenCode; the enum is designed
to serialize cleanly to either.

### 4.1 Questions and choices

Approvals are yes/no. A plain assistant question ends the turn and forces the
user to retype context. The `question-ask` tool adds a real choice channel:

```json
{
  "questions": [
    {
      "header": "Database",
      "question": "Which database should the change target?",
      "options": [
        { "label": "Postgres", "description": "existing production store" },
        { "label": "SQLite", "description": "local tests only" }
      ],
      "multiple": false
    }
  ]
}
```

- **Tool.** `meerai-agents::question::QuestionToolset` registers
  `question-ask` (`EffectClass::Read`). Its `invoke` is a fallback failure; the
  harness handles the call before dispatch. Up to four questions per call, two
  to eight options per question, `header` at most 24 characters. `multiple`
  allows several labels per question.
- **Protocol.** The harness emits `QuestionAsked { request }`, logs
  `SessionEvent::QuestionAsked`, and awaits `AnswerQuestion`. The answer is
  logged as `SessionEvent::QuestionAnswered` and returned to the model as a
  JSON `ToolResult`:
  `{"answers":[{"header":"Database","selected":["Postgres"]}]}`. A cancelled or
  channel-less request returns a structured refusal, so the model can ask in
  plain text or choose a default.
- **Ordering.** Question calls are executed before other tool calls in the same
  batch and never run concurrently. A question does not stop the turn.
- **TUI.** `ActiveModal::Question` shows the header, question, and options.
  Up/Down moves, Space toggles (multiple) or selects (single), Tab/Right and
  Left move between questions, Enter submits, Esc cancels. Pending questions
  queue per tab like approvals.
- **Headless.** There is no interactive channel. `--prompt` auto-cancels the
  question, prints a hint to stderr, and lets the turn continue. NDJSON emits
  the same `QuestionAsked`/`QuestionResolved` events.
- **Subagents.** Child agents do not receive `question-ask` in v1; they must
  return a report instead of blocking on the parent's user.
- **Logging.** `QuestionAsked` and `QuestionAnswered` are log-only session
  events; the authoritative model-visible record stays the `ToolCall`/`ToolResult`
  pair. Replay renders the tool call and its answer.

---

## 5. Agent loop: turns and steps

Adopted semantics (dsh): a **step** is one model request plus the tool calls it
returns; a **turn** is zero or more steps opened by user input and closed when
nothing is owed. `MultiTurnAgent::prompt_streaming` evolves into this loop:

```text
turn/start (log)
  claim inbox: pending steering + next user prompt
  checkpoint: snapshot files the turn may touch (§9)
  loop {
    pre_step waterfall      → compaction pressure check, ephemera refresh,
                               repeat-guard reset, scoped-AGENTS.md injection
    build request           → derive_messages(log, policy) + system prompt assembly
                              (static-first ordering, §7.5) + admitted tool schemas
    budget gate             → token/step/wall-clock limits (§9.4; USD deferred)
    stream                  → live ItemDeltas; settle AssistantMessage (log)
    on request error        → request_error waterfall: retry with backoff, or
                              context-overflow → compaction recovery, then retry once
    tool calls (all of them):
      classify by EffectClass + permission decision (§8)
      read-only calls: run concurrently; mutating calls: ordered, gated
      tools/pre_execute  → hooks, permission enforcement, path guards
      execute            → sandboxed where applicable; stream CommandOutput deltas
      tools/post_execute → spill policy (§7.2), hooks, checkpoint bookkeeping
      settle ToolResult (log)
    step/end (log)
  } while tools owe another request, or steering arrived
  turn_stopping serial checkpoint → Stop hook (§9.3) may continue the turn once
turn/end (log, structured reason)
```

Landed seam: `meerai_agents::intercept` defines `StepPlan`, `ToolOutcome`,
`StepAction`, `TurnStopAction`, and the `Interceptor` trait (`before_step`,
`after_tool`, `turn_stopping`). `MultiTurnAgent::add_interceptor` and
`add_shared_interceptor` register interceptors at composition time;
`before_step` can stop a turn, `after_tool` can add a model-visible follow-up,
and `turn_stopping` can continue the turn once. The other interception points
(`build_request`, `tools/pre_execute`, the full `turn_stop` waterfall) remain
targets. This keeps dsh's extensibility shape without a dynamic plugin runtime;
hot-swappable plugins are explicitly out of scope (§14).

Invariants kept from current code: `CancellationToken` per turn shared into
every await (stream, tool, hook, retry sleep); all retries via
`tokio::time::sleep`; tool failures fed back to the model as tool-role messages,
never as aborts; loop ends on a text-only response or `ToolOutput::Stop`.

---

## 6. Storage history vs. working context

(Source spec §1, retained as the framing for §7.)

- **Storage history** = the session log + blob store + snapshots: immutable,
  complete, auditable, never sent to the model raw.
- **Working context** = `derive_messages(log, policy)` output for one request:
  transient, budgeted, compiled per step.

All context-engineering techniques below are implemented as **projection rules
or post-execute policies over the log** — they never destructively edit history.
This is the key unification of the source spec with the dsh pattern: "lossless
token optimization" is lossless *because* the log is append-only.

---

## 7. Context engine

### 7.1 Budget model

Per-model context budget comes from `ProviderCatalog`. The engine targets:

| Knob | Default | Meaning |
| --- | --- | --- |
| `context_high_watermark` | 75% | trigger tiered compaction (§7.4) |
| `spill_max_inline_tokens` | 12 500 | per-tool-result retention budget (§7.2) |
| `verbatim_window_k` | 4 turns | recent turns never compacted |
| `reasoning_pruning` | on | strip pre-1-step reasoning from projections (§7.3) |

Resolution: the CLI asks `ProviderCatalog::model_info(provider, model)` for the
active model. A model `context_window` replaces the configured default only when
`context.context_window_tokens` is unset; explicit settings and project overrides
always win. `input_cost_per_mtok` and `output_cost_per_mtok` fill
`HarnessConfig::token_pricing` and make `UsageUpdated.est_cost_usd` non-`None`.
Bundled metadata carries context windows only, so no unverified price is
reported.

### 7.2 Tool-output spill (pointer swapping)

Adopted pattern: dsh `spill-policy` (refines source-spec Technique A).

- Runs in `tools/post_execute` before the result is settled.
- Results over `spill_max_inline_tokens`: keep ordered **head/tail halves** of
  the budget, store the full canonical result at
  `blobs/<16-hex-hash>-<byte-len>.txt`, and append a recovery notice:

```text
(Omitted 41 233 bytes. Full result stored at:
 ~/.local/share/meerai/sessions/<id>/blobs/9f2c…-41233.txt —
 use read_file with offset/limit, or grep this path, to inspect it.)
```

- `read_file`/`grep` work on blob paths transparently: the binary adds the
  session directory to `CodeTools` readable roots.
- The **log copy keeps only the retained content**: `ToolResult` stores the
  truncated `ToolObservation`, and `SpillApplied` stores the byte count plus the
  blob path. The full value exists only in the blob file; the current log has no
  separate full-value field.
- Best-effort: any storage failure leaves the original content visible and logs a
  warning (dsh rule). `read_file` results are exempt (already line-bounded).

### 7.3 Ephemera stripping & reasoning pruning

(Source-spec Techniques B + C, as projection rules.)

- **Ephemeral injections** (`InputSource::RuntimeContext`: file trees, lint
  output, reminders) project only into the step they were created for; later
  projections drop them. Persistent injections (safety rules, task goal,
  directory policies) carry `InputSource::System` and stay.
- **Reasoning pruning**: `Reasoning` content projects only for the current step.
  Settled steps keep reasoning in the log (and in `StreamDigest`) for audits and
  the TUI transcript, but it is excluded from `derive_messages` output.
- Provider-specific note: where a provider requires thought-signature replay
  (Gemini), pruning defers to provider correctness — the projection policy is
  per-adapter.

### 7.4 Tiered compaction

Adopted pattern: dsh `compaction` bracket + source-spec Technique D.

- **Triggers**: `Pressure` (pre-step watermark check) and `ContextOverflow`
  (provider error mapped in the `request_error` waterfall → compact → retry
  once). Manual: `:compact` / `/compact` (`AgentCommand::Compact`).
- **Edge safety**: compaction boundaries snap to tool-pairing-balanced
  positions — never cut between an assistant tool call and its result
  (dsh `toolPairingBalancedBefore/After`).
- **Procedure** (all logged, crash-safe via bracket lock):
  1. append `CompactionStarted` (lock; an unmatched start after recovery = busy signal),
  2. summarize turns `1..N-K` with a small model call into a **structured state block**
     (source-spec template: goal / completed progress / modified files / key
     decisions / open blockers),
  3. append `CompactionSummary` and a surface *replacement* that shadows the
     summarized range with one summary message,
  4. append `CompactionEnded` (release). The shadowed events stay in the log —
     replay deterministically reproduces the same condensed conversation.
- **Summarizer model:** `context.summarizer_model` (optional full
  `provider:model` spec) overrides the model used for the summary call. The
  default is the session model. Summary usage is added to cumulative usage and
  counts toward `context.max_session_tokens`.
- **Optional pre-pass**: prune old tool results to spill stubs before
  summarizing (cheap tokens back without an LLM call).
- **Clean reset** for long refactors: `Fork` a fresh session seeded with the
  state block (§3.4) — reset without losing the audit trail.
- Summary messages carry a stable marker so TUI/export/evals can recognize
  condensed history.

### 7.5 Prefix layout for KV-cache hits

(Source-spec Technique E, partially implemented.) The target order is:

```text
1. base system instructions & safety policy        (static per session)
2. tool schemas (stable ordering; §7.7 disclosure) (static per step-set)
3. AGENTS.md root + scoped directory rules         (stable; changes → SystemMessage generation bump)
4. compacted state block                           (changes only at compaction)
5. conversation history & observations             (append-only tail)
```

What the current binary does:

- `compose_system_prompt` builds **one** system string from
  `BASE_SYSTEM_PROMPT` + optional `--system` text + the root `AGENTS.md` +
  `.meerai/instructions.md` (32 KiB per file, 64 KiB block cap). It is sent with
  `ChatRequest::with_system` and logged as a `SystemMessage` with a `generation`
  counter when it changes.
- Tool schemas are attached to the request separately with stable ordering, and
  stripped on the final step so the model returns a summary.
- Scoped `AGENTS.md` files are **not** part of the system prefix. When an
  authorized tool touches a subdirectory, the nearest `AGENTS.md` is injected
  once per turn as a `RuntimeContext` user message (16 KiB cap, max 8 files).
  Projection keeps only the current turn's runtime context.
- A compaction summary is projected as a user message at the replacement
  point, not as a fixed section 4; ordering relative to history therefore
  follows the log, not a rewritten prefix.

The loop does not reorder or reformat the composed system string between steps.
Moving the remaining sections into a stabilized multi-section prefix is future
work.

### 7.6 Output contracts

(Source-spec Technique F.) Enforced in the coding system prompt + tool schemas:

- No full-file rewrites for files over ~30 lines; `edit_file` uses exact
  `old_string`/`new_string` (already the planned `meerai-tools` design).
- `write_file`/`edit_file` emit unified diffs into `FileChange` items — the diff
  is computed by the harness from git/snapshots, never trusted from model text.
- Terse JSON tool arguments (no pretty-printing in generated args).

### 7.7 Progressive tool disclosure

(Source-spec Technique G.) When the registered tool count grows (MCP, skills),
admit only a compact tool index (name + one-line description) into section 2,
plus a `load_tool_schema(tool)` meta-tool whose result is an ephemeral injection
with the full schema. v1 ships all core coding tools directly (they are few);
disclosure activates when admitted schemas exceed ~2k tokens.

---

## 8. Permissions, sandboxing, approvals

Unifies Codex (sandbox modes × approval policy × OS enforcement) with OpenCode
(ordered `action/resource/effect` rules) and the source spec's risk tiers.

### 8.1 Effect classes & risk tiers

Every tool declares `EffectClass`: `Read | Write | Execute | External`.
Mapped to source-spec tiers:

| Tier | Class/examples | Default handling |
| --- | --- | --- |
| 1 — autonomous read | `read_file`, `grep`, `glob`, `list_dir` | run, concurrent |
| 2 — sandboxed mutation | `edit_file`, `write_file`, sandboxed `shell` (build/test) | run inside workspace-write sandbox |
| 3 — human gate | destructive shell (`rm`, `git push --force`), secrets/config edits, network mutations, anything outside writable roots | approval request (TUI overlay / headless prompt or deny) |

### 8.2 Rule engine (OpenCode shape)

```yaml
# ~/.config/meerai/settings.yml — ordered, last match wins
permissions:
  - { action: read,  resource: "*",            effect: allow }
  - { action: read,  resource: "*.env*",       effect: deny }
  - { action: edit,  resource: "src/**",       effect: allow }
  - { action: shell, resource: "cargo *",      effect: allow }
  - { action: shell, resource: "git push *",   effect: ask }
  - { action: shell, resource: "rm *",         effect: deny }
```

- Two config files: the user `settings.yml` in the platform config dir, and an
  optional project `<project-root>/.meerai/settings.yml`. `providers.yml` in the
  config dir holds provider overrides.
- **Project root discovery.** `find_project_root` walks up from the process CWD
  and stops at the first directory that contains `.git` (a directory or a file,
  so linked worktrees count) or `.meerai/settings.yml`. If no ancestor matches,
  the CWD is the root. Tools and sessions still use the process CWD as the
  workspace, so running `meerai` from a subdirectory applies the repository-root
  policy without moving the workspace.
- The project file is merged over the user file by `load_project_settings` and
  `merge_project_settings`; it can tighten permissions but cannot redirect the
  session or grant a new capability:

```yaml
# <project-root>/.meerai/settings.yml
permissions:
  - { action: edit,  resource: "src/generated/**", effect: deny }
  - { action: shell, resource: "cargo publish *",  effect: ask }
context:
  high_watermark_pct: 70
  max_session_tokens: 250000
  summarizer_model: openrouter:small-model
```

  - **Rule order:** project `ask` rules → user rules → project `deny` rules.
    `allow` entries are dropped with a warning. A project `ask` cannot downgrade
    a user `deny`, even with a wildcard resource, because user rules match
    later. It tightens the mode default only where the user has no matching
    rule. A project `deny` always adds a denial after user rules. An invalid
    rule or non-list shape drops the whole project `permissions` block with a
    warning.
  - **Context:** overrides field by field. The budget knobs
    `max_session_tokens` and `max_turn_secs` may only tighten: a project value
    larger than the user value is ignored, and a project cannot use `0` to
    disable the turn clock.
  - **Ignored fields:** `active_provider`, `active_model`, `providers`,
    `approval_mode`, and `sessions` are ignored with a warning, so a repository
    cannot change provider endpoints, switch the mode to `yolo`, or shorten
    session retention.
  - A malformed project file warns and is ignored; startup never aborts.
- Actions: `read`, `edit` (write/patch tools), `shell`, `subagent`, `webfetch`,
  `external_directory`; wildcards on action and resource. Path resources expand
  `~` and are workspace-relative. Patterns without `/` match the basename at
  any depth; patterns with `/` match the workspace-relative path.
- Rules are ordered and the last match wins. The project merge order above keeps
  project `ask` tightening ahead of user rules and project `deny` behind them.
  The engine supports `with_agent_rules`, but no markdown-agent discovery exists
  yet.
- Decisions: `Allow | Ask | Deny`. `Ask` produces `ApprovalRequested`;
  `AcceptForSession` stores a Session grant (logged) so repeats don't re-prompt.
  Grant resources match **literally**; only rule resources use globs. Denied
  calls return a structured refusal `ToolObservation` to the model (it can
  adapt), and are logged.
- `approval_mode` presets: `plan` (hard read-only: non-`Read` effects are denied
  before rules/grants), `ask` (tier defaults), `auto-edit` (reads and workspace
  writes allowed, shell/external ask), `yolo` (allow-all except explicit `deny`
  rules; loud statusline badge).

### 8.3 Sandbox enforcement (Codex pattern, staged)

- **Phase 1 (shipped):** in-process path guards — `meerai-tools` resolves and
  canonicalizes every path (`resolve_for_read`/`resolve_for_write`), confines
  writes to the workspace root + configured writable roots, and confines reads
  to the workspace + readable roots (the session dir is added so spill blobs are
  readable). `shell` runs with `cwd` pinned to the workspace, `sh -c` on Unix /
  `cmd /C` on Windows, a default 120 s timeout, and provider secret variables
  removed from the child environment. There is **no** command deny list; use
  `action: shell` rules instead. This is policy, not a kernel boundary;
  documented as such.
- **Phase 2 (landed — M11):** OS-kernel sandbox for `shell` and `sac-run_script` (`meerai-tools/src/world.rs`, wired by `meerai-cli`):
  - macOS: `SeatbeltWorld` via `/usr/bin/sandbox-exec` (`sandbox_profile`) — read-all, write only workspace roots + `$TMPDIR` (`/dev/null` only in `ReadOnly`), network denied by default.
  - Linux: `BubblewrapWorld` via `bwrap` (`bwrap_args`) — read-only root bind, read-write workspace/writable roots, `--unshare-net` by default; falls back to `HostWorld` (Phase-1 guards) with a startup warning when unavailable (such as Windows or restricted hosts without user namespaces).
  - The sandbox is a **seam**: `trait ExecutionWorld { kind(), is_sandboxed(), warning(), spawn_command() }` so a remote/container world can replace host execution without changing toolsets.
- Sandbox escape requests surface as tier-3 approvals with the exact
  escalation reason (Codex `on-request` semantics).

---

## 9. Safety nets: checkpoints, hooks, budgets, guardrails

### 9.1 Checkpoints & undo (M6 v1, landed)

- The snapshot store lives at `sessions/<id>/snapshots/<turn-id>/`. A turn
  directory holds `manifest.json` plus one blob per captured file. The manifest
  maps workspace-relative paths to `{ blob, existed }`.
- Capture runs in `execute_tool_calls` before a write-class tool executes
  (`write_file`, `edit_file`). It captures the original bytes once per file per
  turn, skips symlinks and files over 2 MiB, and records `existed: false` for a
  file that did not exist. Each capture logs `CheckpointCreated`. Capture is
  best-effort: a failure logs a session error and never fails the turn.
- `AgentCommand::Rewind { turn_id, scope }` with `RewindScope::{Both, Code,
  Conversation}`. Code restore rewrites or deletes files from the manifest.
  `Session::rewind` closes open turns and appends one `RewindApplied { scope,
  restored, replaced }` event; `replaced` drives conversation shadowing. Both
  run in that order.
- The TUI `/rewind` menu targets the latest user turn and offers **Rewind code +
  conversation | Rewind code only**. The message menu's **Rewind** action sends
  `RewindScope::Both` for the selected turn. Conversation restore truncates the
  visible transcript before the turn. `Summarize from here` is not implemented.
- Documented limits: shell-only changes, external edits, gitignored files, files
  over 2 MiB, and symlinks are not tracked. Checkpoints are session recovery,
  not version control. Retention keeps the newest 100 turn directories,
  pruned best-effort on capture. Session GC still follows
  `sessions.auto_prune_days`.

### 9.2 Hooks (Claude Code pattern, Codex-compatible event names)

Deterministic, user-supplied extensions that run *in code*, not in the model.

- **Event set (v1):** `SessionStart`, `UserPromptSubmit`, `PreToolUse`,
  `PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `Stop`,
  `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, `SessionEnd`.
  (Later: `FileChanged`, `PreModelSwitch`, `PostToolBatch`.)
- **Definition:** `.meerai/hooks.json` (project) + `~/.config/meerai/hooks.json`
  (user); entries `{ event, matcher (tool glob), if (arg pattern), command, timeout_ms }`.
- **Execution:** spawn `sh -c command` with JSON on stdin
  (`{ session_id, turn_id, tool_name, tool_input | tool_output, cwd }`);
  exit `0` = pass, exit `2` = **block** (stderr becomes the model-visible
  reason), other = non-fatal warning. JSON stdout supports decisions:
  `{ "hookSpecificOutput": { "permissionDecision": "allow"|"deny"|"ask", "reason": "..." } }`
  for `PreToolUse`; `{ "decision": "block", "reason": ... }` for `Stop`
  (continue-the-turn-once semantics).
- **Guarantees:** hooks for one event run concurrently with a hard timeout
  (default 10 s); a failing hook never bricks a turn (fail-open, except
  `PreToolUse` configured `fail-closed`); every firing is logged (`HookFired`)
  and surfaced as `hook started/completed` notifications like Codex.
- **Trust:** project hooks run only after the workspace is trusted (first-run
  prompt; Codex trust-review rule).

### 9.3 Stop hook & verification

`Stop` fires when the loop is about to end a turn naturally. A project hook can
run `cargo clippy`/tests and block once with a reason — a deterministic
"verify before done" gate (Claude Code pattern; the source spec's dual-agent
verification is the LLM-driven variant, §13).

### 9.4 Budgets & circuit breakers (source spec §2.2)

Enforced by the budget gate in §5, all configurable per session:

- `max_steps` (default 32 per turn; final step strips tools and asks for a
  text summary — OpenCode `steps` semantics),
- wall-clock caps per tool invocation (default 120 s for `shell`) and per turn,
- cumulative token cap per session (`context.max_session_tokens`); hitting the
  cap emits an approval-style pause ("continue spending?") instead of a hard
  kill. `est_cost_usd` is computed from `UsageUpdated` when the provider catalog
  carries input/output pricing for the active model; otherwise it stays `None`.
  USD spend caps remain deferred.
- **circuit breakers**: 3 identical failing tool calls, repeated identical
  edits, or N consecutive steps without surface progress → log
  `CircuitBreaker`, inject a structured diagnostic user message, and either
  continue once or end the turn with `Failed { loop_detected }`
  (dsh `repeat-tool-reminder` guard + source spec).

### 9.5 Inline guardrails (source spec §2.4)

- **Secret redaction (implemented, best effort).** `redact_run`
  (`meerai-agents/src/redact.rs`) rewrites every tool run before spill, the
  session log, the UI, and model text. It covers observation
  stdout/stderr/hint, `ToolOutput` text, and `FileChange` unified diffs; tool
  dispatch errors go through `redact_text`. Patterns: OpenAI/OpenRouter `sk-`,
  Anthropic `sk-ant-`, Google `AIza`, GitHub `ghp_`/`github_pat_`, AWS `AKIA`,
  Slack `xox*`, `Bearer` tokens, and PEM private keys. Matches become
  `[REDACTED:<kind>]`. The original secret is discarded; there is no log-only
  copy. Paths are not redacted. This is regex and pattern based with no entropy
  screen, so it is not a guarantee.
- **Injection screening** (heuristic v1, implemented — `meerai-agents/src/screen.rs`): flags instruction-override phrases and spoofed chat/role delimiters in tool outputs (`screen_run` after `redact_run`) and repository/definition instructions (`screen_untrusted_text`), wrapping flagged text with a persistent `[UNTRUSTED CONTENT WARNING...]` block; documented as best-effort, not a guarantee.
- **Schema validation**: tool arguments validated against the `schemars`
  schema at the harness boundary before dispatch; invalid args return a
  structured error observation without executing (already partially enforced by
  macro-generated deserialization — formalize as `WrongArguments` observation).

### 9.6 Security operations & supply chain

Security is a staged milestone (**M11**, §15.1) because the harness runs model
output and repository content against the local workspace. This section splits
what the tree enforces today from what is backlog.

**Landed (in the tree):**

- **Path guards.** `meerai-tools` canonicalizes every path
  (`resolve_for_read`/`resolve_for_write`), confines writes to the workspace plus
  writable roots, and confines reads to the workspace plus readable roots. These
  are in-process policy checks, not a kernel boundary.
- **`plan` is hard read-only.** Non-`Read` effects are denied before rules and
  grants, so a plan-mode session cannot write, execute, or reach the network
  through a tool.
- **Approval gates.** `Ask` emits `ApprovalRequested` and suspends the tool item
  until a decision arrives; `AcceptForSession` records a logged Session grant,
  turn grants clear at turn end, and grant resources match literally. Headless
  runs fail fast (exit `2`) instead of prompting.
- **Provider-secret environment scrub.** The `shell` child process drops known
  provider key variables and every name ending in `_API_KEY`, `_API_TOKEN`,
  `_ACCESS_TOKEN`, `_SECRET`, `_SECRET_KEY`, or `_PRIVATE_KEY`.
- **Best-effort redaction.** `redact_run`/`redact_text` rewrite observation
  stdout/stderr/hint, `ToolOutput` text, `FileChange` diffs, and tool-dispatch
  errors before spill, session log, UI, and model text. The original secret is
  discarded. Patterns only, no entropy screen, no guarantee; paths are not
  redacted.
- **Definitions are prompt text only.** M12 discovers markdown agents and skills
  under `.agents/` and `.claude/` and applies them as instructions. A definition
  cannot grant a tool, map a permission, run a script, or change the sandbox.
  `tools`, `permissions`, and `allowed-tools` warn and are ignored. Treat the
  files as untrusted repository content. There is no `.meerai/agents` or
  `.meerai/skills` discovery.
- **Session file permissions.** Session directories are `0700` and session files
  `0600` on Unix, including legacy files re-tightened on open. Windows has no
  equivalent mode enforcement.
- **Configurable retention.** `sessions prune` is the manual surface; the user
  config `sessions.auto_prune_days` (with `sessions.keep`, default 0) prunes at
  startup and always spares the currently active session. Retention is never
  controlled by a project file.
- **Budgets.** Session token cap with a Spend approval pause, per-turn wall
  clock, per-tool timeouts, and repeated-failure circuit breakers bound runaway
  spend and loops.

- **OS sandbox.** `trait ExecutionWorld` (`SeatbeltWorld` on macOS, `BubblewrapWorld` on Linux, network denied by default) with `HostWorld` fallback and startup warning when unavailable (§8.3).
- **Sensitive-file blocks.** `is_sensitive_path` in `meerai-tools/src/code/paths.rs` and `PolicyEngine::default_sensitive_rules` block reading/editing `.env*`, key, secret config, and credential files by default.
- **Dependency audit in CI.** `deny.toml` + `EmbarkStudios/cargo-deny-action@v2` in `.github/workflows/ci.yml` and `release.yml`.
- **Written threat model.** Published at [docs/threat-model.md](threat-model.md) covering repository instructions, model output, planned hooks/MCP/ACP, and subagents (M8).
- **Prompt-injection screening.** Heuristic v1 (`meerai-agents/src/screen.rs`) wraps flagged tool output and repository/definition instructions; pattern-based, not a guarantee.
- **OS keychain.** `KeychainSecretStore` (`NativeKeychainBackend` using macOS `security` and Linux `secret-tool`) migrates secrets out of plaintext `auth.json` when available, falling back cleanly to `0600` `auth.json`.
- **Conditional artifact signing.** `.github/workflows/release.yml` contains secret-gated macOS `codesign`/`notarytool` and Windows `signtool` steps alongside `SHA256SUMS.txt`.

**Remaining / Operational:**

- Release signing certificates and secrets (`APPLE_CERTIFICATE` / `WINDOWS_CERTIFICATE`) are not populated in the repository by default, so published archives remain unsigned until configured.
- Windows `ExecutionWorld` runs on `HostWorld` (Phase-1 path guards).

---

## 10. Agents & subagents (OpenCode pattern)

### 10.1 Markdown-defined agents (M12, landed)

```markdown
<!-- .agents/agents/reviewer.md (default), or .claude/agents/reviewer.md -->
---
name: reviewer
description: Reviews changes for correctness and regressions
model: gemini-3.1-pro      # optional; a ':' model is ignored (no provider switch)
steps: 8                   # optional; recorded but not applied
mode: subagent             # optional; recorded but not applied
---
Review the current changes. List findings by severity with file:line references.
```

- Discovery is read-only and starts at the project root: `.agents/agents/*.md`
  (default, higher priority) then `.claude/agents/*.md`; `.agents` and
  `.claude` skills follow the same order. `.meerai/agents`,
  `.meerai/skills`, and `~/.config/meerai/agents` are not scanned or created.
  meerai copies nothing and writes no import lock. Output is sorted by id.
- Id: frontmatter `name`, else the file stem. Fields: `name`, `description`,
  `model`, `steps`, `mode`. `tools`, `permissions`, and `allowed-tools` warn and
  are not applied. Invalid frontmatter warns and skips the file. Missing
  frontmatter uses the file stem and the whole body.
- Duplicate id: `.agents` wins and the warning names both paths.
- Body = instructions. `--agent <ID>` appends them to the system prompt at
  startup; it takes one id and repeating the flag is an error. `/agent <id>` applies them as a
  System message at the next step boundary; the `@` Mentions popup inserts
  `@agent/<id>`, which applies them before the prompt is sent. `steps` and `mode`
  are not honoured.
- Model precedence: `--model` → `MEERAI_MODEL` → agent `model` →
  settings/provider default. An agent `model` containing `:` would change
  provider and is ignored with a warning.
- M12 prompt application gives no custom tool subset or custom permission set
  when applied via `--agent` or `/agent`. When executed as a child via
  `subagent-run` (M8), built-ins (`build`, `plan`, `explore`, `general`) get
  their typed tool subsets and budgets, whereas `.agents`/`.claude` custom
  agents stay read-only (`ApprovalMode::Plan`, read-only tools) regardless of
  frontmatter `tools`/`permissions`/`allowed-tools`.

### 10.2 Subagent execution

- A subagent run = **child session** (own event log, forked seed or fresh) +
  its own `MultiTurnAgent` instance with the agent's system prompt, model,
  permission set, and tool subset.
- Exposed to the model as one `subagent(agent, task)` tool (tier: read-only
  agents run concurrently; mutating agents serialize behind approvals).
- Child runs in background; parent receives `ItemStarted { SubagentRun }` →
  report on child idle → `ItemCompleted { report }`. Only the final report
  enters the parent's context — the isolation that makes search subagents
  token-efficient (source spec §5, SWE-grep evidence).
- Default budgets: `explore` = 4 steps (3 parallel-search steps + 1
  aggregation), returning only `file:start-end` spans + one-paragraph synthesis.

### 10.3 Skills & portable definitions (M12, landed)

- Skills are markdown bundles: `.agents/skills/<dir>/SKILL.md` (default) then
  `.claude/skills/<dir>/SKILL.md`. Only `SKILL.md` is read; supporting files in
  the directory are ignored.
- Id: frontmatter `name`, else the directory name. Frontmatter and duplicate
  rules match agents (§10.1); `description` feeds the listing.
- `--skill <ID>` is repeatable and appends skill text before the agent text at
  startup. `/skill <id>` applies the body as a System message at the next step
  boundary. In the TUI, typing `@` opens the Mentions popup: agents first, then
  skills, then files. An agent row shows `<id>|agent` (cyan), a skill row shows
  `<id>|skill` (magenta), each followed by its source and description; a file
  row shows its path with no badge. The row shows the bare id, not the
  `@agent/`/`@skill/` prefix. One id can appear as both an agent and a skill.
  Filtering matches `@agent/<id>`, `@skill/<id>`, the bare id, and fuzzy
  prefixes. Up/Down or Ctrl-p/Ctrl-n move; Tab accepts; Enter accepts without
  submitting; Esc dismisses. Accepting inserts `@agent/<id> `, `@skill/<id> `,
  or `@path ` with one trailing space and stays in Insert. An exact agent or
  skill id keeps the popup open; an exact file path auto-dismisses. A known
  accepted mention renders as an `<id>` chip (bold cyan for agent, bold magenta
  for skill) while the raw `@agent/<id>`/`@skill/<id>` stays in the input buffer;
  unknown ids and partial tokens render as typed. On submit,
  skills apply before the agent, mentions are stripped, repeats apply once, and
  an unknown mention stays as text; a mention-only prompt applies definitions
  without a turn. Unknown or missing `/agent`/`/skill` ids print a System error
  and send nothing. `/agents` and `/skills` do not exist.
- Portability: `.agents` and `.claude` layouts are read in place. There is no
  import command, no copy step, and no `.meerai` definition directory.
- Security: definitions are untrusted repository prompt text. No tool
  execution, no permission or tool mapping, no scripts, no sandbox implication,
  no MCP, and no supporting-file loading. Definition text passes through
  heuristic prompt-injection screening (`screen_untrusted_text`, §9.5), which is
  best-effort regex wrapping rather than a hard security guarantee.
- Runtime & future: M8 subagent execution (`subagent-run`) can launch a
  definition via `--agent`/TUI mention or tool dispatch while forcing
  repository-defined agents into `ApprovalMode::Plan`. Tool/permission mapping,
  MCP servers, and supporting skill files remain backlog.

---

## 11. Structured errors & retries (source spec §2.5)

- Provider and tool errors map to typed observations:
  `{ status, exit_code, stdout, stderr, hint }`; raw stack traces never reach
  the model unstructured.
- Transient provider failures retry **3 attempts total** with 250 ms then 500 ms
  backoff (`MAX_PROVIDER_ATTEMPTS`, `provider_backoff_after`). Errors whose
  message contains a non-transient marker (`api key`, `unauthorized`, `401`,
  `403`, `invalid model`, `not found`, `permission denied`) fail immediately.
  Tool invocation retries use a fixed 250 ms sleep and `max_retries` (default 3).
  There is no `invoke_tool_with_retry` function in the current tree.
- Provider context-overflow errors route to the compaction recovery path
  (§7.4) and retry once instead of failing the turn.
- Recurring model mistake classes (e.g., repeated invalid edits) compile into
  harness-side pre-checks over time — the §1 P3 discipline, tracked as
  "harness growth" backlog items from trace review (§13).

---

## 12. Retrieval & code intelligence (non-vector)

Source spec §4 retained in full; staged by cost/benefit for this workspace.

| Stage | Capability | Notes |
| --- | --- | --- |
| v1 | `read_file(offset,limit)`, `grep` (ripgrep-style, line-bounded), `glob`, `list_dir` | Implemented in `meerai-tools`; spill stubs keep big hits cheap |
| v1 | Hierarchical `AGENTS.md` | Root always in prefix (§7.5); per-directory `AGENTS.md` injected as ephemera only when the turn touches matching paths |
| v2 | `explore` subagent (§10.2) | landed: parallel read-only calls; returns spans only |
| v2 | AST repo map (Aider pattern) | `tree-sitter` symbol extraction + PageRank over the reference graph; render top symbols as an abbreviated tree inside a 1–2k token budget; regenerate on file-change, cache per session |
| v3 | LSP tools | `go_to_definition`, `find_references`, `get_diagnostics` via `rust-analyzer`/`tsserver`/`gopls`; post-edit diagnostics as a `PostToolUse` hook on edit tools |
| v3 | Search-as-Code | `run_script` tool: sandboxed script (Python/JS) for multi-step search/filter/AST analysis; output spilled like any tool result (Perplexity SaC pattern; source spec §5) |

Rationale (source spec §4, unchanged): vector embeddings underperform on live
code — exact-identifier confusion, chunk fragmentation, intra-turn staleness,
no structural awareness. Default retrieval is lexical + structural; embeddings
are not planned.

**Native parallel tool arrays:** the loop executes all tool calls per step.
**Consecutive read-only calls run concurrently** via `join_all` today (not a
bounded pool); a mutating call breaks the batch and runs alone. `ToolOutput::Stop`
halts the remaining batch and marks later calls `Cancelled`. That is what makes
the `explore` budget viable once subagents land.

---

## 13. Observability & evals (source spec §2.6)

- **Traces**: the log *is* the trace. `meerai export <session> --json` is
  planned, not built; `transcript_from_events` already renders the human
  projection. Per-turn rollups: token usage, wall time, `git diff` stat at
  `TurnEnded`. `est_cost_usd` is computed from the provider catalog's
  per-million prices when both are configured; otherwise it is `None`.
- **Replay/evals**: `derive_messages` purity means an offline runner can replay
  any logged session against a new model/harness version (recorded provider
  streams enable deterministic re-projection); regression suite over golden
  sessions (SWE-bench-style harness evals later).
- **Dual-agent verification** (high-risk deliverables): a `reviewer` subagent
  with a distinct prompt and read-only permissions inspects the diff against
  the task spec before the turn is declared done — the LLM-driven sibling of the
  §9.3 deterministic Stop hook.
- **Telemetry seam**: `HookFired`, `UsageUpdated`, `CircuitBreaker`, and error
  events give ops the full cost/behavior picture without extra instrumentation.

---

## 14. Non-goals

- Dynamic hot-swappable plugin runtimes (dsh's Cordis layer): Rust composition
  at binary startup + hooks + markdown agents cover the extensibility need.
- Vector databases / embedding indexes (§12 rationale).
- Web/desktop GUIs, actor frameworks, bot integrations (per architecture.md).
- Windows sandbox parity in v1 (phase-1 guards only).
- Rewriting history destructively: rewind/compaction always shadow, never delete.

---

## 15. Migration roadmap

Extends the roadmap in [architecture.md](architecture.md); each milestone keeps
`fmt → clippy → test` green and ships something user-visible.

### 15.1 Milestones

| # | Milestone | Content | Touches | Status |
| --- | --- | --- | --- | --- |
| M1 | **Session log core** | `meerai-session` crate: `SessionEvent`, JSONL+zstd persistence, `derive_messages` + snapshot tests, crash recovery, meta | new crate; `meerai-agents` logs while keeping in-memory history | landed (R1–R2) |
| M2 | **Item protocol** | `AgentEvent` v2 (§4) behind the existing channel; TUI `handle_agent_event` folds items; headless NDJSON | `meerai-agents`, `meerai-tui` | landed (R0–R3) |
| M3 | **Coding tools + effect classes** | `meerai-tools` v1 (architecture.md §coding toolsets) with `EffectClass`, path guards, unified-diff `FileChange` items | new crate, `meerai-core` | landed (R1–R3) |
| M4 | **Policy engine + approvals** | `meerai-policy`: ordered rules, approval request/response loop, TUI approval overlay wired to `ApprovalRequested`, session grants | new crate, TUI | landed (R1–R3) |
| M5 | **Context engine v1** | spill policy, ephemera/reasoning pruning, prefix ordering, watermark compaction with bracket lock + `:compact` | `meerai-session`, `meerai-agents` | landed (R1–R3); prefix ordering partial: root `AGENTS.md` + `.meerai/instructions.md` + scoped per-turn injection are implemented; full static-first multi-section prefix remains future work |
| D1 | **Distribution hardening** | Apache-2.0 license, workspace metadata, CI gate, tag-driven 5-target release, Pages installers | repo root, `.github/`, `site/` | landed |
| M6 | **Checkpoints + rewind** | per-turn snapshots, `/rewind` menu, restore-code/conversation flows | `meerai-session`, `meerai-agents`, TUI | landed v1: capture before write tools, `RewindScope::{Both,Code,Conversation}`, `/rewind` menu. Shell-only edits, gitignored files, and summarize-from-here remain backlog |
| M7 | **Hooks** | v1 event set, JSON stdin/exit-2 contract, trust gate, `HookFired` logging | `meerai-policy`, `meerai-agents` | backlog |
| M8 | **Subagent runtime** | child sessions (`parent_session_id`), depth-1 isolation, built-ins (`build`/`plan`/`explore`/`general`), `subagent-run` tool, forwarded child approvals, usage rollup, `SubagentRun` items, read-only `Plan` execution for M12 custom definitions | `meerai-agents`, `meerai-session`, `meerai-tui` | landed (M8): child sessions and `SubagentToolset` landed; M12 frontmatter `tools`/`permissions` remain prompt-only |
| M9 | **CLI + resume/fork** | `meerai-cli` crate: `--prompt`, `--json`, `--continue`, `--resume <id>`, `--fork <id>`, `sessions fork <id> [--turn <turn-id>]`, `auth`/`sessions`, config/env wiring | `meerai-cli`, `meerai-session` | landed (M9): `meerai-cli`, `sessions fork`, and `--fork` landed (`-p` stays `--provider`); `export` and TUI `/fork` are not built |
| M10 | **Retrieval v2/v3** | `RepoMapToolset` (`retrieval-repo_map` via tree-sitter + PageRank), `LspToolset` (`lsp-go_to_definition`, `lsp-find_references`, `lsp-get_diagnostics`), `SearchAsCodeToolset` (`sac-run_script`) | `meerai-tools`, `meerai-cli` | landed (M10): repo map, LSP stdio tools, and search-as-code landed; automatic post-edit diagnostics waits on M7 hooks |
| M11 | **Security & supply chain** | landed: path guards, sensitive-file path + policy blocks, `plan` hard read-only, approval gates, provider-secret env scrub, best-effort redaction, heuristic injection screening (`screen_run`/`screen_untrusted_text`), session permissions, retention, budgets, `ExecutionWorld` (`SeatbeltWorld`/`BubblewrapWorld` + `HostWorld` fallback), OS keychain (`KeychainSecretStore`) with `auth.json` fallback, `cargo-deny` CI audit (`deny.toml`), conditional release signing, and [threat-model.md](threat-model.md) | `meerai-tools`, `meerai-policy`, `meerai-providers`, `meerai-agents`, `meerai-cli`, `.github/workflows/`, `docs/threat-model.md` | landed (M11): core runtime controls, CI audit, threat model, and conditional signing landed; repository signing secrets remain operator-configured and Windows uses `HostWorld` |
| M12 | **Agents & skills discovery** | read-only markdown discovery: `.agents/agents/*.md` + `.agents/skills/*/SKILL.md` over `.claude/...`; duplicate ids resolve to `.agents`; frontmatter `name`/`description`/`model`/`steps`/`mode`; startup `--agent`/`--skill`; TUI `/agent`/`/skill`, the `@` Mentions popup, and composer chips | `meerai-common` (`definitions`), `meerai-tui`, `meerai-agents` (`ApplyAgent`/`ApplySkill`) | landed (R5): prompt instructions only. No subagent runtime (M8), no tool/permission mapping, no MCP, and no supporting skill files |
| M13 | **ACP client (external harnesses)** | adapter presets in user config only; spawn via `AcpAgent`; `session/new`/`session/prompt`/`session/cancel`; map chunks, tool calls, plans, usage, and permission requests to `AgentEvent`; `--acp`/`/acp`; `acp doctor`. Target order: Antigravity (`agy acp`), then OpenCode (`opencode acp`), then Gemini/Claude/Codex. Design in [acp.md](acp.md) | new `meerai-acp` crate, `meerai-tui`, `meerai-session` (meta fields) | backlog (P0 spike first). External agents keep their own tools and policy: meerai rules, path guards, and redaction do not apply; permission requests are advisory |
| M14 | **ACP agent (meerai in editors)** | `meerai acp` on stdio: `initialize`, `session/new`, `session/load`, `session/prompt`, `session/cancel`, `set_mode`; stream `AgentEvent` as `session/update`; approvals as `session/request_permission`; `session/load` reuses `transcript_from_events`. Design in [acp.md](acp.md) | `meerai-acp`, `meerai-agents`, `meerai-tui` | backlog. Gates on the ACP compatibility kit plus an editor smoke |

Ordering rationale: M1–M2 establish the two load-bearing patterns (log,
protocol) before features accrete; everything after is projection rules,
policies, or tools layered on the same spine.

### 15.2 Delivery model for M1–M5

Contract-first, five rounds. Shared contracts land before any parallel work;
each round has disjoint file ownership (one writer per file set); the
orchestrator runs the authoritative workspace gate after every round.

| Round | Workstream | Owned files | Delivers |
| --- | --- | --- | --- |
| R0 | Contracts (orchestrator) | `docs/harness-spec.md`, root `Cargo.toml`, `meerai-core/src/tools.rs`+`lib.rs`, crate skeletons (`meerai-session`, `meerai-policy`, `meerai-tools`), `meerai-agents/src/events.rs` | `EffectClass`, `ToolObservation`, `ToolRun`, `Toolset::{effect_of, invoke_structured}` (default methods — no proc-macro codegen change); frozen `SessionEvent` vocabulary + ids; frozen `AgentEvent` v2; frozen policy types; new workspace deps (`zstd`, `globset`, `similar`, `insta`) |
| R1a | WS-SESSION | `meerai-session/**` | M1: JSONL persistence (+zstd rotation), `meta.json`, crash recovery, `derive_messages` + `ProjectionPolicy` + insta snapshots, fork/rewind, blob store, spill primitives, compaction primitives (M5 lib half) |
| R1b | WS-TOOLS | `meerai-tools/**` | M3: `read_file`, `write_file`, `edit_file`, `list_dir`, `grep`, `glob`, `shell`; path guards; unified diffs via `similar`; `ToolObservation`/`FileDiff` results; per-tool `EffectClass` |
| R1c | WS-POLICY | `meerai-policy/**` | M4 lib half: ordered `action/resource/effect` rules (glob via `globset`, last match wins), `ApprovalMode` presets, tier defaults, session grants, YAML config parsing |
| R1d | WS-MIGRATE | `meerai-agents/src/multi_turn_agent.rs`, `meerai-tui/**` | M2 mechanical: loop emits v2 item lifecycle; TUI folds items into the existing transcript; tests updated; behavior preserved (no session/policy yet) |
| R2 | WS-LOOP | `meerai-agents/**`, `meerai-common/src/settings.rs` | M1/M4/M5 wiring: session dual-write, `derive_messages`-driven requests, policy checks + `ApprovalRequested`/`Approve` suspend-resume via in-turn command inbox, spill post-execute, watermark compaction + `ContextOverflow` recovery, budget gate, circuit breakers, `Steer`, prefix-ordered system prompt; additive settings fields (permissions, budgets, context knobs) |
| R3 | WS-CLIENT | `meerai-tui/**` | M2/M4/M5 client: approval overlay (`ActiveModal` recipe), `/compact`, mode switching, headless `--prompt`/`--print` + `--json` NDJSON printer, `CodeTools` registration in `main.rs` |
| R4 | Integration (orchestrator) | repo docs | Consolidated gate `cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace`; sync `README.md`, `AGENTS.md`, `.agents/repo/profile.md`; update status column above |

Parallel-round gate protocol (shared Cargo build): each R1 agent verifies with
`cargo fmt -p <crate>`, `cargo clippy -p <crate> --all-targets`,
`cargo test -p <crate>` and judges compile errors **by file path** — fix only
owned paths, never touch or wait on sibling crates; the orchestrator runs the
workspace-wide gate once after the whole round lands. Cargo target-dir lock
contention is normal, not a hang.

### 15.3 Acceptance criteria (M1–M5)

**M1 — Session log core**
- [x] `SessionEvent` vocabulary frozen per §3.1; ids are cheap newtypes generated without external services.
- [x] Append-only JSONL at `<sessions-root>/<id>/session.v0.jsonl`; sealed generations rotate to `.zstd`; committed generations never rewritten.
- [x] Crash recovery: unsealed trailing turn gets a synthesized `TurnEnded { Interrupted }`; earlier events intact (test).
- [x] `derive_messages(log, policy)` pure + deterministic; insta snapshots cover: plain transcript, ephemera stripping, reasoning pruning, empty-assistant skip, compaction shadowing.
- [x] `fork()` only at turn-end boundaries (mid-turn prefix rejected, test); rewind shadows, never deletes.
- [x] Agent loop dual-writes: in-memory history kept, every model-visible message logged (R2).

**M2 — Item protocol**
- [x] `AgentEvent` v2 per §4 with serde derives; every delta references an `ItemStarted.id`; `ItemCompleted` payload is authoritative.
- [x] Migration table (§4) fully applied; v1 variants gone; TUI transcript UX unchanged.
- [x] Headless: `meerai --prompt "..." [--json]` runs one turn without a TTY; `--json` prints one NDJSON object per event.
- [x] `ApprovalRequested` suspends the tool item until `AgentCommand::Approve` arrives (in-process test).

**M3 — Coding tools + effect classes**
- [x] Seven tools via `Toolset` with per-tool `EffectClass` (`effect_of`) and `invoke_structured` returning `ToolRun { output, observation, file_changes }`.
- [x] Path guards: canonicalized; writes confined to workspace + writable roots; escapes return structured refusals (tests).
- [x] `edit_file`: exact `old_string` match, unique-or-`replace_all`, `WrongArguments`-shaped refusal on ambiguity/miss (tests).
- [x] `shell`: `tokio::process`, pinned cwd, 120 s default timeout, capped output, exit code/stdout/stderr in `ToolObservation` (tests).
- [x] `write_file`/`edit_file` produce unified diffs (`similar`) surfaced as `FileChange` items by the loop.

**M4 — Policy engine + approvals**
- [x] Ordered rules from config YAML; wildcards on action+resource; last match wins (tests).
- [~] Per-agent rules refine global in the engine (`with_agent_rules`). M12 discovers markdown agents but applies prompt text only: `permissions` and `tools` warn and are not mapped, so no rule set is loaded from a definition.
- [x] `ApprovalMode` presets `plan|ask|auto-edit|yolo`; `plan` denies non-Read before rules/grants, `yolo` honors explicit `deny`.
- [x] `Ask` → `ApprovalRequested` event; `AcceptForSession` stores a logged Session grant; repeats don't re-prompt (test). The TUI `t` key sends `scope: Some(Turn)`; `y`/Enter sends `None`. Grant resources match literally.
- [x] `Deny` → structured refusal `ToolObservation` fed back to the model; turn continues.
- [x] TUI approval overlay shows tool + args preview; keys map to Accept / AcceptForTurn (`t`, `scope: Some(Turn)`) / AcceptForSession / Decline / DeclineWithReason / Cancel.

**M5 — Context engine v1**
- [x] Context knobs with §7.1 defaults (watermark 75 %, spill 12 500 tokens, verbatim window 4 turns, reasoning pruning on) in `HarnessConfig`; `ProjectionPolicy` carries the trimming flags.
- [x] Spill in post-execute: head/tail retention, blob write, recovery notice; `read_file` exempt; storage failure fails open (tests).
- [x] Ephemera stripping + reasoning pruning active in `derive_messages` (snapshot tests); only the current turn's `RuntimeContext` survives.
- [x] Compaction: pressure trigger (pre-step watermark) + manual `Compact` command; bracket events logged; boundary snaps tool-pair-balanced; structured state-block summary; shadowed range replayable deterministically (tests).
- [x] `ContextOverflow` → compact → retry once in the request-error path.
- [x] Budget gate: max steps per turn (default 32; final step strips tools and asks for a summary), per-tool wall-clock caps, per-turn wall clock (`max_turn_secs`, also bounding streams, summarization, and tool batches), session token cap → approval-style pause.
- [x] Circuit breakers: 3 identical failing tool calls → `CircuitBreaker` event + model-visible diagnostic; a 4th fails the turn with `LoopDetected`.
- [~] Prefix ordering: root `AGENTS.md` / `.meerai/instructions.md` composition and scoped per-turn injection are implemented; the full static-first multi-section prefix (§7.5) is not.

### 15.4 Recorded design decisions (M1–M5 push and later rounds)

- **`EffectClass` declaration seam.** Declared via defaulted
  `Toolset::effect_of(fn_name)` + `Toolset::invoke_structured` instead of a new
  `ToolDefinition` field, so proc-macro codegen and its insta snapshots stay
  untouched. A `#[tool(effect = "...")]` attribute can migrate this later.
- **Headless flag.** `-p` already means `--provider` in the current binary; the
  spec's `meerai -p "<prompt>"` would collide. Headless ships as
  `--prompt`/`--print` (+ `--json`); flag unification is deferred to `meerai-cli` (M9).
- **Token accounting and cost stance.** `ProviderCatalog` carries optional
  per-model metadata (`models:` in `providers.yml` or the user override):
  `context_window`, `input_cost_per_mtok`, and `output_cost_per_mtok`. The
  resolved model's context window replaces the configured default only when
  `context.context_window_tokens` is unset. Budgets still use a ~4-chars/token
  heuristic. `est_cost_usd` is computed from cumulative usage when both prices
  are configured; otherwise it stays `None`. Bundled metadata ships context
  windows only, so no wrong price is reported. Headless non-`--json` prints
  `tokens: prompt=… completion=… total=…` on stderr, `--json` emits
  `UsageUpdated`, and the TUI statusline shows turn and session totals.
- **Rewind is the only user-facing action.** `AgentCommand` has no `Revert`
  variant and the log has no `RevertApplied` event. `Session::rewind` appends one
  `RewindApplied { turn_id, scope, restored, replaced }`; `replaced` holds the
  shadow range for conversation scopes. `RewindScope::Code` restores files from
  the snapshot manifest. `Both` runs code then conversation. The TUI menu
  targets the latest user turn; the message menu's Rewind action targets the
  clicked turn.
- **Questions are not approvals.** `question-ask` uses `QuestionAsked` /
  `AnswerQuestion`, not the policy engine. Reusing approvals would conflate a
  permission decision with a product choice, pollute grants, and make headless
  exit codes wrong. A question is `EffectClass::Read`, serialized before the
  rest of the batch, and returns a JSON answer or a structured cancellation.
  Subagents do not get the tool.
- **Step interceptor seam.** `meerai-agents::intercept` is the extension point
  for M6/M7: an ordered `Vec<Arc<dyn Interceptor>>` runs `before_step`,
  `after_tool`, and `turn_stopping` without editing the loop body. `ToolOutcome`
  carries the post-redaction, post-spill observation. Hooks and checkpoints must
  register here, not add branches to `turn.rs`.
- **Provider model metadata.** `ModelInfo` lives in `ProviderCatalog`, not in
  `HarnessConfig`. The catalog supplies only defaults; user settings and the
  project file still win, and a project cannot raise a cap. Pricing is never
  invented in the bundled catalog.
- **Build surfaces.** `meerai-tui` has a default `update` feature (reqwest);
  `meerai-cli` has a default `tui` feature. `--no-default-features` builds a
  headless binary without ratatui, git2, comrak, or the TUI crate. genai still
  pulls reqwest in both builds.
- **Session root.** `dirs::data_dir()/meerai/sessions` (platform-correct),
  `MEERAI_SESSION_DIR` env override, optional project-local `.meerai/sessions`.
- **Persistence.** Active generation is plain append-only JSONL (crash-resilient
  line granularity, human-tailable); zstd applies to sealed/rotated generations.
- **No commits without user authorization.** Round checkpoints are recorded as
  green-gate states in the working tree.
- **`plan` is a hard read-only mode.** Non-`Read` effects are denied before
  rules and grants. `yolo` is allow-all *except* explicit `deny` rules. `ask`
  and `auto-edit` keep the §8.1 tier defaults.
- **Headless exit contract.** Tool-call approval: fail fast, hint (`--mode
  auto-edit|yolo` or the TUI), exit `2`. Spend approval: print the
  `context.max_session_tokens` remedy, decline, exit `1` (turn `Failed { Budget }`).
  `Completed` → `0`; other failures → `1`.
- **Approval scope.** `Approve.scope` is `Option<Turn|Session>`.
  `AcceptForSession` always records a Session grant; the TUI `t` key sends
  `Some(Turn)` and the grant is cleared at turn end. `y`/Enter sends `None`.
  Grant resources are literal; rule resources use globs.
- **`.env` is opt-in.** `MEERAI_LOAD_DOTENV=1|true|yes` or `--env-file <PATH>`;
  nothing loads implicitly.
- **Windows is supported.** `cmd /C` shell, x64 release target, no file-mode
  enforcement. Release archives are unsigned.
- **License and metadata.** Apache-2.0; `license`, `repository`, `homepage`,
  `rust-version = "1.100"`, `publish = false` in `[workspace.package]`.
- **Session privacy and management.** `0700` dirs, `0600` files on Unix;
  `meta.json` carries cwd/git/title/model/timestamps; `sessions list|rm|prune`
  and `--continue`/`--resume` are the management surface. `prune` defaults to 30
  days and keeps 0 unless `--keep N`. `--continue`/`--resume` work headless with
  `--prompt` and start the TUI with a replayed transcript without it.
  Project-local sessions are gitignored. Retention is manual by default;
  `sessions.auto_prune_days` opts into startup pruning with
  `sessions.keep` survivors and always spares the active session.
- **Project root discovery.** The project config is looked up at the repository
  root, not only at the CWD: `find_project_root` walks up to the nearest
  directory holding `.git` (directory or file) or `.meerai/settings.yml`, and
  falls back to the CWD. Tools and sessions keep the CWD as the workspace, so a
  run from a subdirectory picks up the repository-root policy.
- **Retention is opt-in and user-controlled.** Manual `sessions prune` stays the
  default. `sessions.auto_prune_days` (with `sessions.keep`, default 0) prunes at
  startup and always spares the active session; a successful prune prints
  `Pruned N sessions (older than N days)` on stderr, and a failure warns without
  aborting. A project `sessions` block is ignored with a warning, so a repository
  cannot shorten retention.
- **Dead surface removed.** `#[derive(Schema)]` is gone from `meerai-macros`
  (only `#[tool]` and `#[derive(Toolset)]` remain), the legacy `meerai-common`
  config module and `auth.yml`-era layer are gone, the orphaned `meerai-common`
  stop tools are gone, and `meerai-core`'s `json_schema`/`errors` modules are
  gone. Unused dependencies were pruned and test-only macro dependencies moved
  to `[dev-dependencies]`.
- **Security stance.** Redaction (`redact_run`) and prompt-injection screening
  (`screen_run`) are best effort, not guarantees. `ExecutionWorld` provides
  macOS Seatbelt / Linux bubblewrap isolation when available and degrades with a
  startup warning to Phase-1 guards on unsupported/nested hosts (and Windows).
  Release signing steps in `release.yml` remain inactive until repository signing
  secrets are supplied; see [threat-model.md](threat-model.md).
- **Prompt composition.** `BASE_SYSTEM_PROMPT` + `--system` + root `AGENTS.md` +
  `.meerai/instructions.md` (32 KiB/file, 64 KiB block) in the system prompt;
  scoped `AGENTS.md` injected once per turn as runtime context (16 KiB, max 8).
- **Project permissions are tighten-only; config cannot redirect.**
  `<project-root>/.meerai/settings.yml` is discovered from the CWD (see
  *Project root discovery* below) and merged
  over user settings. Project
  `permissions` merge as project `ask` → user → project `deny`; `allow` is
  dropped, a project `ask` cannot downgrade a user `deny`, and a project `deny`
  always adds a denial after user rules. `context` overrides field by field,
  with budget knobs allowed only to tighten. `active_provider`, `active_model`,
  `providers`, `approval_mode`, and `sessions` are ignored with a warning. A
  malformed file warns and is ignored.
- **Redaction is best effort.** Known secret patterns are replaced with
  `[REDACTED:<kind>]` before spill, log, UI, and model text; the original is
  discarded (no log-only copy). Diffs are redacted, paths are not. No entropy
  screen and no guarantee.
- **Session titles and summarizer model.** The agent titles a session from the
  first user prompt (single line, 80 chars) only when it has no title and no
  earlier user message. `context.summarizer_model` overrides the compaction
  model; the default is the session model, and summary usage counts toward the
  session token cap.
- **Provider retries and turn clock.** 3 provider attempts (250/500 ms,
  transient-only), tool retries at `max_retries` with fixed 250 ms, and a
  per-turn wall clock (default 1800 s, `0` disables) that also bounds in-flight
  streams, compaction summarization, and tool batches.
- **Read-only concurrency.** Consecutive `EffectClass::Read` calls run
  concurrently; `Stop` halts the remaining batch.
- **Definitions merge at runtime, with no copies (M12).** The binary calls
  `meerai_common::definitions::load_definitions(&project_root)`; it stores the
  result in `TuiLaunchConfig` and never materializes `.meerai/agents`,
  `.meerai/skills`, copies, or an import lock. `.agents` is read before
  `.claude`, so a duplicate id keeps `.agents` and the warning names both paths;
  the catalog is sorted by id. This avoids a migration step and keeps the
  repository's own `.claude` layout authoritative.
- **`.meerai` is not a definition directory (M12).** `.meerai` holds settings,
  sessions, and instructions. Adding a native agent/skill directory would create
  a third layout to maintain and would diverge from the portable `.agents`/
  `.claude` formats. Do not add one.
- **Definitions are prompt-only and untrusted (M12).** Loading applies
  instructions to the system prompt (`--agent`/`--skill`) or as a System-sourced
  message (`/agent`/`/skill`; the `@` Mentions popup inserts
  `@agent/<id>`/`@skill/<id>`, which apply the same message before the prompt is
  sent). Frontmatter `tools`, `permissions`, and `allowed-tools` warn and are
  ignored; there is no tool execution, permission mapping, script hook, MCP, or
  sandbox implication. Mentions apply skills before the agent, strip the token
  from the prompt, apply repeats once, and leave an unknown mention in the
  prompt.
- **Agent models cannot switch provider (M12).** An agent `model` containing `:`
  is ignored with a warning, because the provider is a separate axis. Model
  precedence stays `--model` → `MEERAI_MODEL` → agent `model` → settings/provider
  default. `steps` and `mode` are parsed, reported in a note, and not applied.
- **Skill files are not traversed (M12).** Only `SKILL.md` is read; supporting
  files in the skill directory are ignored until a runtime can consume them.

### 15.5 Verification protocol

Per [.agents/repo/profile.md](../.agents/repo/profile.md) §7: repo-wide gate is
`cargo fmt --all -- --check && cargo clippy --workspace --all-targets && cargo
test --workspace`. During parallel rounds, agents run crate-scoped gates only
(§15.2). A subagent's "done" is a claim until the consolidated gate passes in
the orchestrator's session.

### 15.6 Delivery log

The test counts in the R0–R4 entries below are snapshots at delivery time. The
current count is whatever `cargo test --workspace` reports (348 at the time of
the Post-M5 R5 note).

- **R0 landed.** Contracts in tree: `EffectClass`, `ToolObservation`,
  `FileDiff`, `ToolRun`, `Toolset::{effect_of, invoke_structured}` (defaulted,
  non-breaking) in `meerai-core`; `SessionEvent` vocabulary, ids, projection
  and persistence skeletons in `meerai-session`; rule/engine skeletons in
  `meerai-policy`; `CodeTools` skeleton in `meerai-tools`; `AgentEvent`
  v2 in `meerai-agents/src/events.rs` (frozen).
- **Contract amendments (orchestrator):** `SessionItem::ToolCall` gained
  `output: Option<String>` so `ItemCompleted` stays authoritative for the TUI
  output preview; `meerai_core::ToolCall` derives `PartialEq`/`Eq` (required by
  `SessionEvent`); `meerai-agents` exposes `pub mod events` re-exporting the
  session vocabulary types so clients need no direct `meerai-session` dep.
- **Concurrent user changes (preserved, not part of R0–R4 scope):** provider
  `x-opencode-session` id + user-agent headers on agent requests, `meerai auth`
  subcommand (list/login/logout), API-key settings plumbing in
  `meerai-common`, TUI auth/model UI. WS-MIGRATE and later briefs must keep
  these behaviors intact.
- **Incident:** a stray `exclude = ["meerai-session"]` appeared in the root
  manifest mid-R0, silently dropping the new crate from all workspace gates.
  Removed. Gate rule: `cargo clippy --workspace` output must name every member
  crate; an excluded member is a red flag.
- **R1 (WS-MIGRATE) done.** v1→v2 protocol migration landed in
  `multi_turn_agent.rs` (item lifecycle, cumulative usage, `Steer` queueing,
  typed `TurnError` classification) and the TUI fold (`app.rs`,
  `event_loop.rs`, `ui.rs` tests). Orchestrator-verified consolidated gate:
  fmt clean, clippy clean (pre-existing warnings only), `cargo test
  --workspace` 92 passed / 0 failed.
- **Contract amendment 2 (orchestrator):** `ContentBlock` switched to adjacent
  tagging (`tag = "type", content = "data"`) — serde cannot serialize
  internally-tagged newtype variants containing strings; this broke JSONL
  persistence until fixed.
- **R1 deep round running (parallel, disjoint crates):** WS-SESSION
  (generations+zstd rotation, crash recovery, projection completeness + insta
  snapshots, fork/rewind — incl. the `RewindApplied` event —
  blob store + spill, compaction primitives), WS-TOOLS (seven `code-*` tools,
  guards, diffs, observations), WS-POLICY (globset rule matching, presets,
  grants, YAML). Each on the crate-scoped gate protocol; consolidated gate
  re-run by the orchestrator after the round lands.
- **R1 consolidated gate (orchestrator-verified):** fmt clean; clippy clean
  except two pre-existing warnings (`meerai-core` test `assert!(true)`,
  `meerai-macros` test-util — predate this push, left untouched);
  `cargo test --workspace` **168 passed / 0 failed** (session 47, policy 45,
  tools 18, tui 31+2, agents 5+, macros 8, common 11, core 1).
  M1/M3/M4-lib/M5-lib landed; follow-ups applied in-session: spill blob write
  moved to `spawn_blocking`, `list_dir` gitignore-aware (ripgrep-style: hidden
  entries excluded), unused deps pruned without lint overrides.
- **R2a landed (orchestrator-verified: 13 tests, clippy clean, workspace check
  clean).** `meerai-agents` harness wiring: `AgentHarness`/`HarnessConfig`,
  session dual-write + derive-driven requests, turn/step/usage logging,
  `invoke_structured` dispatch with per-tool timeout, `FileChange` items,
  policy enforcement with `ApprovalRequested` suspend/resume, in-turn command
  inbox (`Approve`/`Steer`/`SetPermissionMode`/`Compact`),
  `prompt_streaming_with_commands`. Non-harness mode unchanged (incl.
  `x-opencode-session` header behavior). Rulings: channel-less `Ask`
  auto-declines (fixed in R2b); `reset()` intentionally keeps the session log
  (P1 audit trail).
- **R2b running:** spill post-execute, pressure/manual/overflow compaction
  with bracket events, final-step tool stripping, session token cap with Spend
  approval, circuit breakers.
- **R2b landed (orchestrator-verified: 25 tests, clippy clean, workspace check
  clean).** Full M5 loop wiring: spill (Success/Stopped observations,
  `code-read_file` exempt, fail-open), pressure compaction (once per turn) +
  manual `Compact` at step boundaries + `ContextOverflow` → compact → retry
  once (pre-stream failures only), final-step tool stripping (ephemera kept
  for that request via `strip_ephemera: false`), session token cap with Spend
  approval pause (auto-decline without a command channel — never hangs),
  repeated-failure circuit breaker (3× diagnostic injection, 4× `LoopDetected`
  turn failure). Ruling applied: summarizer usage counts toward the session
  token cap (real spend). Final R2 state orchestrator-verified: agents 26
  tests, workspace **189 passed / 0 failed**.
- **R3 running (WS-CLIENT):** additive `ContextSettings`/`approval_mode`/
  `permissions` settings fields, harness + `CodeTools` registration in
  `main.rs`, headless `--prompt/--print [--json]` NDJSON mode, TUI approval
  overlay (Accept/AcceptForSession/Decline/DeclineWithReason/Cancel),
  `/compact` + `/mode`, mid-turn `Steer` from the composer.
- **R3 landed.** CLI: `--prompt/--print`, `--json` NDJSON, `--mode` (`-p`
  stays provider); approval overlay with queued requests + inline
  decline-with-reason; `/compact`, `/mode` + statusline badge; composer Enter
  while running sends `Steer`; settings stay backward-compatible (all-Option
  additive fields, round-trip tested). Known gap (accepted): an approval
  arriving while another modal is open replaces that modal.
- **R4 consolidated gate (orchestrator-verified):** `cargo fmt --all --
  --check` clean; `cargo clippy --workspace --all-targets` clean except the
  two pre-existing warnings; `cargo test --workspace` **203 passed / 0
  failed**. §15.3 acceptance: all 27 boxes ticked. Caveats: headless mode is
  unit-tested (arg parsing, NDJSON serde round-trip) but not smoked against a
  live provider (no API keys in this environment); prefix ordering (§7.5)
  covers the instructions section with generation bumps — AGENTS.md scoped
  injection was deferred here and landed in Post-M5 R2.
- **R4 doc sync landed:** `README.md`, `AGENTS.md`, `.agents/repo/profile.md`
  rewritten to match the tree (claim→evidence verified by the doc workstream).
  Orchestrator rulings on its open questions: (1) `#[derive(Schema)]` stayed in
  the tree at this point, documented as unused/broken against schemars 1.x — the
  fix-or-remove follow-up resolved as **removal** in Post-M5 R4; the macOS
  data-dir split (sessions under
  `dirs::data_dir()/meerai/sessions`, `auth.json` under the XDG-style
  `default_data_dir()`) stays as shipped — both are documented; unifying on one
  helper is a follow-up.
- **Delivery complete for M1–M5.** Final state: 8 crates + `meerai` binary,
  workspace gate green (203 tests), no commits made — the full delivery sits
  in the working tree for review.
- **WS-POLICY landed (42 tests green), two orchestrator rulings:**
  (1) path rules use gitignore-style semantics — patterns without `/` match
  the file basename at any depth (`*.env*` denies `config/prod.env`), patterns
  with `/` match the workspace-relative path; (2) `yolo` short-circuited
  rules/grants per §8.2 "allow-all" at the time — superseded in Post-M5 R2:
  `yolo` now honors explicit `deny` rules. Additive API: `with_workspace_root`,
  `warnings()`, `rules_from_yaml`, `approval_preview_for`.
- **Crate restructure (post-delivery).** `meerai-tools-code` became the `code`
  submodule of the umbrella crate `meerai-tools`: `meerai_tools::code::CodeTools`
  (also re-exported as `meerai_tools::CodeTools`), module file
  `meerai-tools/src/code/mod.rs`. Runtime tool-call names stay `code-*`, so the
  harness, policy mapping, and TUI are unaffected. `meerai-tui` dependency,
  `Cargo.lock`, and all docs/agent-knowledge references updated.
- **Post-M5 R1 — distribution hardening.** Apache-2.0 `LICENSE`; workspace
  metadata (`license`, `repository`, `homepage`, `rust-version = "1.100"`,
  `publish = false`). Workflows: `ci.yml` (fmt/clippy/test on push and PR to
  `main`), `release.yml` (verify → 5-target build → tag/version gate → GitHub
  Release with archives and SHA256), `pages.yml` (deploy `site/`). Installers
  (`site/install.sh`, `site/install.ps1`) verify checksums; `Makefile` exposes
  `make verify`. `.agents/repo/infra.md` records the operational detail.
- **Post-M5 R2 — P1 behavior and docs sync.** Headless: `--prompt`/`--print`,
  `--json`, `-V`/`--version`, `--env-file`, `--continue`, `--resume <ID>`;
  tool-call approvals fail fast with exit `2`, Spend approvals decline and exit
  `1`; sessions `list`/`rm`. Sessions: `0700` dirs / `0600` files on Unix,
  cwd/git/title/model metadata, `--continue` by last activity. Policy: `plan`
  hard read-only, `yolo` honors `deny`, literal grant resources, `Approve.scope`.
  Prompt: `meerai_agents::prompt` composition plus once-per-turn scoped
  `AGENTS.md`. Loop: provider retry schedule, `max_turn_secs`, concurrent
  consecutive read-only calls, model-visible breaker diagnostic. Tools: Windows
  `cmd /C`, provider secret env scrub. Env: `.env` opt-in. Docs: `README.md`,
  `AGENTS.md`, `.agents/repo/{profile,patterns}.md`, and this specification
  corrected against the tree (including the historical claims in §2, §3, §4,
  §7, §8, §11, §12, §15).
- **Post-M5 review pass.** An independent review of `5abff6a` found three high
  and three medium defects; all are fixed with tests: executed reads keep their
  result after a sibling `Stop`; `:` is rejected in session ids (Windows
  drive-relative paths); grants are literal and cannot widen to globs or bypass
  `deny`; the turn clock also bounds in-flight streams, compaction
  summarization, and tool batches; Spend approvals decline with exit `1` and
  distinct guidance; `updated_at_ms` moves on every `TurnEnded`; legacy session
  files are re-tightened on open; secret env matching is case-insensitive.
  Final gate: fmt clean, clippy clean except the two accepted pre-existing
  warnings, `cargo test --workspace` **265 passed / 0 failed**, the same result
  from a clean checkout, and `cargo install --path meerai-tui --bin meerai`
  verified. No live-provider smoke test ran; Windows runtime behavior has no CI
  runner.
- **Post-M5 R3 — project config, TUI replay, and redaction.** Project
  `<workspace>/.meerai/settings.yml` loads and merges over user settings:
  permission rules merge as project `ask` → user → project `deny`, `allow` is
  dropped with a warning, invalid shapes drop the block, `context` overrides
  field by field with `max_session_tokens`/`max_turn_secs` clamped to tighten
  only, and provider/model/mode fields are ignored with a warning; a malformed
  file warns and is ignored. TUI `--continue`/`--resume <ID>` work without
  `--prompt` and replay user/steering, assistant, tool, and compaction entries
  while skipping runtime/system events and marking unmatched tool calls
  cancelled. `sessions prune [--days N] [--keep N]` defaults to 30 days / keep 0
  and prints `removed N sessions`. The approval overlay adds `t` for an Accept +
  Turn grant. Headless non-`--json` runs print a cumulative token summary; the
  TUI statusline adds session totals; token counters seed from the last logged
  `UsageUpdated` on resume; cost remains token-only with `est_cost_usd = None`.
  Session titles come from the first prompt (80 chars); `context.summarizer_model`
  overrides the compaction model. Tool output, diffs, and tool-dispatch errors
  are redacted best-effort before spill, log, UI, and model text, and the
  original secret is discarded. Final gate: fmt clean, clippy clean except the
  two accepted pre-existing warnings, and `cargo test --workspace` **293 passed
  / 0 failed**.
- **Post-M5 R4 — project root discovery, opt-in retention, cleanup, M11.**
  Project config is now discovered by `find_project_root`
  (`meerai-common/src/settings.rs`), which walks up from the process CWD to the
  nearest directory containing `.git` (directory or file) or
  `.meerai/settings.yml` and falls back to the CWD; tools and sessions keep the
  CWD as the workspace. The user config gains `sessions.auto_prune_days` and
  `sessions.keep` (default 0): when `auto_prune_days` is set, startup prunes
  sessions older than N days except the newest `keep` and the active session,
  prints `Pruned N sessions older than N days` on stderr, and warns without
  aborting on error. (The R5 message is `Pruned N sessions (older than N days)`.)
  Manual mode stays the default; `sessions prune` and its
  flags are unchanged; a project `sessions` block is ignored with a warning.
  Cleanup: `#[derive(Schema)]` removed from `meerai-macros` (only `#[tool]` and
  `#[derive(Toolset)]` remain), the legacy `meerai-common` config module and
  `auth.yml`-era layer removed, the orphaned
  `meerai-common/src/tools/{stop,stop_with_reason}.rs` removed,
  `meerai-core/src/{json_schema,errors}.rs` removed, unused dependencies
  (`serde_path_to_error`, `thiserror` and `anyhow` in `meerai-macros`) pruned,
  and test-only macro dependencies moved to `[dev-dependencies]`. Roadmap: M10
  is retrieval only; **M11 Security & supply chain** added with the §9.6
  landed/backlog split. Final gate: `cargo fmt --all -- --check` clean,
  `cargo clippy --workspace --all-targets` clean except the two accepted
  pre-existing warnings, and `cargo test --workspace` **300 passed / 0 failed**.
  The same count came from a clean copy of the tree with no `target/` directory.
- **Post-M5 R5 — agents & skills discovery (M12).**
  `meerai-common::definitions` loads markdown agents
  (`.agents/agents/*.md`, then `.claude/agents/*.md`) and skills
  (`.agents/skills/*/SKILL.md`, then `.claude/skills/*/SKILL.md`) from the
  project root. Loading is read-only: no copies, no `.meerai` definition
  directories, no import lock, and symlinks are skipped. `.agents` wins a
  duplicate id and the warning names both paths; output is sorted by id.
  Frontmatter is `name`, `description`, `model`, `steps`, `mode`;
  `tools`/`permissions`/`allowed-tools` warn and are ignored; invalid frontmatter
  warns and skips the file; missing frontmatter uses the file or directory name
  and the body; only `SKILL.md` is read. Startup `--agent <ID>` takes one id
  (repeating the flag is an error) and repeatable `--skill <ID>` append
  instructions to the system prompt. Model
  precedence is `--model` → `MEERAI_MODEL` → agent `model` → settings/provider
  default; a colon model is ignored with a warning; `steps`/`mode` print a note
  and are not applied. TUI `/agent <id>` and `/skill <id>` send
  `AgentCommand::ApplyAgent`/`ApplySkill`; typing `@` opens the Mentions popup
  (agents, then skills, then files; `<id>|agent` cyan, `<id>|skill` magenta,
  files with no badge). Up/Down or Ctrl-p/Ctrl-n move, Tab accepts, Enter accepts
  without submitting, Esc dismisses; an exact agent/skill id keeps the popup
  open. On submit, skills apply before the agent, mentions are stripped, repeats
  apply once, and an unknown mention stays as text; a mention-only prompt applies
  definitions without a turn. A known accepted mention renders as an `<id>` chip
  (bold cyan agent, bold magenta skill) while the raw `@agent/<id>`/`@skill/<id>`
  stays in the input buffer; unknown and partial tokens render as typed. The
  composer title says `@ mentions`. `/agents` and `/skills` do not exist.
  Automatic prune now prints `Pruned N sessions (older than N days)`. Definitions
  are untrusted prompt text and grant no tools or permissions. Scope limits: no
  subagent runtime (M8), no tool/permission mapping, no MCP, and no supporting
  skill files. Final gate: `cargo fmt --all -- --check` clean,
  `cargo clippy --workspace --all-targets` clean except the two accepted
  pre-existing warnings, and `cargo test --workspace` **348 passed / 0 failed**.
  The same count came from a clean copy of the tree with no `target/` directory.

### 15.7 Milestone M8, M9, and M10 delivery notes

- **M8 (Subagent runtime):** `meerai-agents::subagent` implements
  `SubagentToolset` (`subagent-run`) with built-ins `explore` (4 steps, read),
  `plan` (16 steps, read), `build` (32 steps, write), and `general` (16 steps,
  write), depth-1 isolation (`ChildToolsetFilter`), child sessions linked via
  `parent_session_id` (`Session::create_child`), forwarded `ApprovalRequested`
  prompts, parent `cumulative_usage` rollup, `SessionItem::SubagentRun` items,
  and TUI/headless rendering. Custom M12 definitions execute under read-only
  `Plan` defaults with prompt instructions only.
- **M9 (CLI fork):** `meerai-cli` implements `meerai sessions fork <id> [--turn <turn-id>]`
  and top-level `--fork <id>` (fork and resume immediately). `meerai-session`
  adds `latest_closed_turn` and `fork_latest`; `latest_session` skips child
  sessions when selecting `--continue`. `-p` remains `--provider`; `meerai export`
  and TUI `/fork` remain backlog.
- **M10 (Retrieval v2/v3):** `meerai-tools` adds `RepoMapToolset`
  (`retrieval-repo_map`, tree-sitter Rust/TS/TSX/JS/Python/Go + personalized
  PageRank), `LspToolset` (`lsp-go_to_definition`, `lsp-find_references`,
  `lsp-get_diagnostics` over stdio JSON-RPC), and `SearchAsCodeToolset`
  (`sac-run_script` running `python3 -I` / `node -e` with env scrubbing and
  `EffectClass::Execute` policy gating). Automatic post-edit LSP diagnostics
  remains queued behind M7 hooks.

---

## 16. References

Primary sources consulted (Nov 2026):

- Codex app-server protocol — developers.openai.com/codex/app-server (Thread/Turn/Item,
  item deltas, approvals, fork/resume/compact, JSONL rollouts, `generate-ts`/`generate-json-schema`).
- Codex sandboxing & approvals — developers.openai.com/codex/sandboxing (read-only /
  workspace-write / danger-full-access; on-request vs never; Seatbelt, bubblewrap;
  sandbox ≠ approval policy; auto-review).
- Codex hooks — developers.openai.com/codex/hooks (event set, matchers, trust review).
- OpenCode agents — opencode.ai/docs/agents (markdown agents, modes, ordered
  permission rules, built-in build/plan/general/explore, subagent child sessions).
- OpenCode server & sessions — opencode.ai/docs/server (OpenAPI 3.1, session
  fork/revert/summarize/diff endpoints); opencode.ai/docs/snapshots (per-step
  git-scoped snapshots, `/undo` staged rollback).
- DeepSeek Harness — github.com/deepseek-ai/deepseek-harness, `docs/architecture.md`,
  `docs/agent-lifecycle.md`, `docs/subsystems/session.md`, `docs/event-producer-consumer.md`,
  `packages/spill/spill-policy`, `packages/compaction/compaction` (append-only log,
  deriveMessages purity, surface replacements, spill head/tail + recovery notice,
  compaction bracket lock, capability seams, fork-at-boundary).
- Claude Code hooks — code.claude.com/docs/en/hooks (~30 lifecycle events,
  command/HTTP/MCP/prompt/agent handlers, exit-code-2 blocking, JSON decisions).
- Claude Code checkpointing — code.claude.com/docs/en/checkpointing (per-prompt
  snapshots, 100-checkpoint retention, /rewind restore & summarize options,
  documented limitations).
- Li et al., *Grow the Harness, Not the Context*, arXiv:2609.26760 (verified:
  −76.0–91.8% LLM calls, −74.4–98.6% deployed inference cost).
- Cognition, *Introducing SWE-grep and SWE-grep-mini: RL for Multi-Turn, Fast
  Context Retrieval* (cognition.com/blog); Morph WarpGrep (as reported in the
  source comparison).
- Aider repo maps — aider.chat/docs/repomap.html (tree-sitter tags + PageRank
  within a token budget).
- Superseded source: "Optimized Coding Agent Harness Architecture & Context
  Engineering Specification" (removed; VMOS framing, techniques A–G, retrieval
  tables, and governance requirements are incorporated above).
