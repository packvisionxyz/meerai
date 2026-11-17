# Token Efficiency and Self-Healing Initiative

**Date:** 2026-11-16
**Author:** meerai maintainers
**Status:** In progress (phases 1–6 landed v1 + overflow/script-guard fixes; phase 7+ proposed)
**Scope:** `meerai-agents`, `meerai-session`, `meerai-tools`, `meerai-common`, `meerai-cli`, `meerai-tui`, `docs/`
**Related Documents:** [harness-spec.md](harness-spec.md), [architecture.md](architecture.md), [threat-model.md](threat-model.md)

## 1. Summary

meerai re-sends the full projected conversation on every step and never asks a provider to cache it.
The harness also heals only at session open, so a live process can send an unpaired tool call after a
cancel during planning. This initiative reduces token cost and makes recovery budgeted and verified.

The work has two tracks. The token track enables provider prompt caching, reports cache hits, prunes
old tool results through the projection, and removes the compaction dead zones. The healing track
repairs dangling calls in the live process, adds a bounded recovery ladder, and gates completion on
an external verifier. All changes follow the existing seams: projection rules, `HarnessConfig`,
`Interceptor`, `HookEngine`, and `AgentCommand`.

Goal: lower cost per turn and raise the share of failed turns that recover without user input, with
no loss of session-log fidelity.

Done condition: a default session reports cache-hit tokens, a long turn compacts before it blocks,
and a cancelled or loop-detected turn never sends an unpaired tool call on the next prompt.

## 2. Current state (evidence)

| Area | Observed behavior | Evidence |
| :--- | :--- | :--- |
| Request assembly | Every step rebuilds the full projection; no projection cache | `meerai-agents/src/multi_turn_agent/turn.rs:665`, `mod.rs:856-862` |
| Provider cache | No `CacheControl` or cache key is set; `Usage.cached_tokens` is hardcoded `None` | `meerai-providers/src/providers.rs:21-55`, `turn.rs:162-172`, `meerai-session/src/event.rs:113-121` |
| Provider cache (phase 1) | `context.prompt_caching` (default true) sets `CacheControl::Ephemeral`; `Usage` copies cached and cache-write counts | `turn.rs`, `meerai-session/src/event.rs` |
| Tool-result retention | Spill writes large results to blobs and keeps head+tail near the 12 500-token budget; old results stay inline until compaction | `meerai-session/src/blob.rs:62-98`, `tools.rs:1152-1160` |
| Tool-result clearing (phase 2) | `ProjectionPolicy::clear_old_tool_results` stubs results outside `verbatim_window_turns`; the newest window stays verbatim and spilled stubs name their blob | `meerai-session/src/projection.rs` |
| Compaction trigger | Pressure compacts once per turn; the 4-turn window blocks early compaction | `turn.rs:190,257-265`, `meerai-session/src/compaction.rs:32-34` |
| In-turn compaction (phase 2) | The planner falls back to balanced `StepEnded` boundaries; pressure may apply twice per turn, once per step | `meerai-session/src/compaction.rs`, `meerai-agents/src/multi_turn_agent/turn.rs` |
| Cache-safe summarizer (phase 2) | With no `summarizer_model`, the summary request reuses the parent system, tools, and history and appends the instruction as the last user message | `meerai-agents/src/multi_turn_agent/compaction.rs` |
| Context estimate | `chars/4`; tool schemas are excluded from the watermark estimate | `meerai-session/src/blob.rs:48-50`, `compaction.rs:375-378` |
| Schema-aware estimate (phase 2) | `tool_schema_tokens` is computed per turn and summed into the watermark | `meerai-agents/src/multi_turn_agent/turn.rs`, `compaction.rs` |
| Prompt size | Root `AGENTS.md` cap is 32 KiB and definition text is uncapped | `meerai-agents/src/prompt.rs:29-31`, `meerai-cli/src/main.rs:116-134` |
| Definition cap (phase 2) | 16 KiB per definition, 32 KiB total, with a warning; the subagent schema lists ids only | `meerai-cli/src/main.rs`, `meerai-agents/src/subagent.rs` |
| Verify hook (phase 3) | `meerai hooks verify [--command CMD] [--reset-baseline]` adds one user Stop hook with a per-project failure baseline; only new failure names block | `meerai-cli/src/hooks_cmd.rs`, `meerai-common/src/hooks.rs`, `meerai-agents/src/hooks.rs` |
| Edit breaker (phase 3) | Identical `code-edit_file`/`code-write_file` batches log `RepeatedIdenticalEdit` at the third and fifth repeat | `meerai-agents/src/multi_turn_agent/turn.rs` |
| No-progress breaker (phase 3) | 8 tool steps without a file change log `NoProgress`; 12 fail the turn; disabled in `plan` and read-only runs | `meerai-agents/src/multi_turn_agent/turn.rs` |
| Recovery turn (phase 3) | The worker queues one recovery prompt after a retryable provider or `LoopDetected` failure; `RecoveryStarted` keeps headless waiting for the recovery turn | `meerai-agents/src/multi_turn_agent/worker.rs`, `meerai-cli/src/headless.rs` |
| Turn-failed hook (phase 4) | `HookEvent::TurnFailed` fires on a failed turn with the error in the JSON payload; its decision is observational | `meerai-common/src/hooks.rs`, `meerai-agents/src/multi_turn_agent/turn.rs` |
| Retry classification (phase 4) | Provider retries use HTTP status when available (408/409/425/429 and 5xx transient; other 4xx permanent) plus 50% jitter; the message blocklist is the fallback | `meerai-agents/src/multi_turn_agent/turn.rs` |
| Rollback (phase 4) | `context.rollback_on_verify_failure` restores the turn snapshot on a Stop-hook verify block in `auto` mode and logs `RewindApplied { scope: Code }` | `meerai-agents/src/multi_turn_agent/mod.rs` |
| Argument coercion (phase 5) | Tool arguments are coerced to the tool schema before dispatch (string numbers/booleans, scalar arrays, null removal); the lenient parser repairs malformed JSON first | `meerai-core/src/lenient.rs`, `meerai-agents/src/multi_turn_agent/tools.rs` |
| Projection cache (phase 5) | `request_messages` caches the projection by event count and policy; new events or a policy change recompute it | `meerai-agents/src/multi_turn_agent/mod.rs` |
| Argument repair hint (phase 6) | A rejected tool call logs a RuntimeContext message with the tool schema and asks for corrected arguments | `meerai-agents/src/multi_turn_agent/tools.rs` |
| Recovery model (phase 6) | `context.recovery_model` selects the model for the recovery turn; the session model is restored afterward | `meerai-common/src/settings.rs`, `meerai-agents/src/multi_turn_agent/worker.rs` |
| Edit-anchor repair (phase 6) | A missing `old_string` is normalized (whitespace and `read_file` line prefixes); a unique match applies once, ambiguous matches report line numbers, missing anchors suggest closest lines | `meerai-tools/src/code/edit_file.rs` |
| Live healing | Healing runs in `Session::open_inner` only | `meerai-session/src/session.rs:327-329,609-638` |
| Live healing (phase 1) | `run_turn` also heals after `TurnEnded` through the same idempotent helper | `meerai-agents/src/multi_turn_agent/turn.rs` |
| Cancel gap | Cancel during planning returns with no `ToolResult` for the batch | `meerai-agents/src/multi_turn_agent/tools.rs:292-295` |
| Provider retries | 3 attempts, message-marker classification, no status codes, no partial-stream retry | `turn.rs:10-15,696-733`, `mod.rs:85-93` |
| Overflow recovery | Fixed: `ContextOverflow` only retries once when `run_compaction` returns `Ok(true)`; `Ok(false)` skips the retry and fails cleanly | `meerai-agents/src/multi_turn_agent/turn.rs` |
| Loop breakers | All three `BreakerKind` variants (`RepeatedToolFailure`, `RepeatedIdenticalEdit`, `NoProgress`) are emitted; only `BreakerAction::ContinueOnce` remains unused | `turn.rs`, `tools.rs`, `meerai-session/src/event.rs` |
| TUI statusline (phase 1) | Shows `↻<compact>` when `session_cached_tokens > 0`; headless stderr prints `cached=<n>` | `meerai-tui/src/ui/statusline.rs`, `meerai-cli/src/headless.rs` |
| Script sandboxing (`sac-run_script`) | Runs under `ExecutionWorld` `ReadOnly` (`allow_network: false`), validates `path` inputs via `CodeTools::resolve_for_read`, and populates inline `PermissionCheck.resource` previews; host-wide reads (`(allow file-read*)` / `--ro-bind / /`), resource limits, Node `--permission`, and silent `HostWorld` fallback remain open | `meerai-tools/src/search_as_code/mod.rs`, `meerai-tools/src/world.rs`, `meerai-agents/src/multi_turn_agent/tools.rs` |
| Continuations | Stop hook and `turn_stopping` share one continuation per turn | `turn.rs:522-552` |
| Hook lifecycle | `TurnFailed` fires on failed turns (observational); non-`PreToolUse`/`Stop` hook stdout is discarded | `meerai-common/src/hooks.rs`, `meerai-agents/src/hooks.rs` |

## 3. Decisions

**Q1 — Where does prompt caching get enabled?**

Chosen: a request-level `CacheControl::Ephemeral` in the turn's `ChatOptions`, controlled by a new
`context.prompt_caching` setting that defaults to on.
Why: genai 0.7.5 places an Anthropic breakpoint on the static tools+system prefix for the native
Anthropic adapter and emits an OpenAI cache intent for GPT-5.6 and later. The change is one option
and needs no new dependency. OpenAI-compatible gateways keep their provider-native implicit caching.
Rejected: per-message breakpoints — more invasive, needs cache-aware message ordering first.
Revisit if: a provider rejects the request shape, or cache-hit reporting shows no benefit.

**Q2 — Do cache hits change `est_cost_usd`?**

Chosen: no, in phase 1. Report raw `cached_tokens` and `cache_creation_tokens`; keep the current
price formula.
Why: cache-read and cache-write prices differ by provider and TTL. A wrong discount is worse than a
missing one, and the bundled catalog ships no prices.
Rejected: assume a fixed 0.1x read discount — wrong for OpenAI, Bedrock, and 1h TTLs.
Revisit if: the catalog carries a per-provider cache price field.

**Q3 — How are old tool results pruned?**

Chosen: a projection rule that replaces results older than a keep window with a one-line stub and the
blob pointer. The log keeps everything.
Why: this is lossless under P1 and needs no LLM call. Anthropic reports an 84% token cut for the same
mechanism.
Rejected: destructive log truncation — violates P1 and breaks replay.
Revisit if: re-read cost exceeds the saved tokens for typical sessions.

**Q4 — When does compaction run in a long turn?**

Chosen: allow pressure compaction at a tool-pair-balanced step boundary, at most once per step and
at most twice per turn.
Why: a 32-step turn can never compact today. The step count is the natural outer bound and the
per-step guard prevents retry loops.
Rejected: compact mid-stream — the request is in flight.
Revisit if: mid-turn summaries degrade answer quality.

**Q5 — Where does live call repair run?**

Chosen: after `TurnEnded`, in `run_turn`, before the next turn can start. Reuse the existing
`heal_dangling_tool_calls` logic and keep it idempotent.
Why: this matches the documented invariant that a turn ending with unexecuted calls is healed after
`TurnEnded`. It closes the cancel-during-planning gap without a wire-contract change.
Rejected: heal inside `derive_messages` — the projection must stay pure and the log must show the
repair.
Revisit if: a result appended after `TurnEnded` confuses a consumer.

**Q6 — What gates an automatic retry?**

Chosen: external signals only (tests, compiler, LSP, schema checks), with a budget per recovery
action.
Why: a model's own opinion of its work is a weak gate; verifier-gated loops are the reliable pattern.
Rejected: self-critique retry — no reliable stop signal.
Revisit if: an eval shows self-critique helps for a specific failure class.

**Q7 — Is automatic rollback on by default?**

Chosen: no. It is opt-in via `context.rollback_on_verify_failure`, runs only in `auto` mode, and
logs `RewindApplied { scope: Code }`. In `ask`/`plan`, the turn continues with a notice to use
`/rewind`.
Why: capture excludes shell edits, so rollback is partial. A silent partial rollback would hide work,
and v1 does not add a new approval kind.
Rejected: always roll back on verify failure — can discard valid partial progress.
Revisit if: shell-edit capture lands or a dedicated rollback approval is added.

**Q8 — Where does the built-in verify gate live?**

Chosen: a user `Stop` hook installed by `meerai hooks verify [--command CMD]`, with a per-project
failure baseline.
Why: Stop hooks already have a trust gate, timeout, logging, and a one-shot continuation. A new
`context.verify_command` would add a second command runner and a new trust rule. The baseline keeps a
pre-existing failing suite from blocking every turn.
Rejected: a `context.verify_command` setting executed by the harness — duplicates the hook engine and
lets a project file propose a command.
Revisit if: the hook preset cannot express a required check.

**Q9 — What can a TurnFailed hook change?**

Chosen: nothing in v1. It is observational; its decision is logged.
Why: the turn has already failed and the worker owns recovery. Letting a hook rewrite the failure
would blur the error contract and the exit codes.
Rejected: a hook `Block` starts a recovery turn — duplicates the recovery budget and needs a new
decision path.
Revisit if: users need hook-driven recovery for a failure class the built-in recovery excludes.

## 4. Target behavior

| Surface | Today | Target |
| :--- | :--- | :--- |
| Provider request | Full history, no cache intent | Full history, cache breakpoint on the static prefix, cache intent on the request |
| `UsageUpdated` | Prompt, completion, total | Adds `cached_tokens`, `cache_creation_tokens` |
| Headless summary | `tokens: prompt=… completion=… total=…` | Adds `cached=…` when present (landed) |
| TUI statusline | `session ↑… ↓… Σ…` | Adds `↻…` when cached tokens exist (landed) |
| Projection | All results inline | Old results become stubs plus blob pointer |
| Long turn | No compaction after the 4-turn guard | Balanced-boundary compaction inside the turn |
| Failed turn | Reusable log, unpaired calls possible | Repaired log before the next prompt |
| Verify gate | User hook only | Opt-in built-in gate with bounded continuations |
| Rollback | User action only | Opt-in, approval-gated automatic restore |

## 5. Requirements

| ID | Requirement | Priority | Phase |
| :--- | :--- | :--- | :--- |
| R1 | Request sets a prompt-cache breakpoint when `context.prompt_caching` is true | NEED | 1 |
| R2 | `Usage` carries cache read and cache write counts from genai | INSIST | 1 |
| R3 | Headless and TUI usage show cache hits when present | WANT | 1 |
| R4 | Old tool results are stubbed through the projection under a keep window | NEED | 2 |
| R5 | Compaction can run inside a long turn at a balanced boundary | NEED | 2 |
| R6 | Watermark estimate includes tool-schema size | WANT | 2 |
| R7 | Dangling tool calls are repaired at `TurnEnded` in the live process | INSIST | 1 |
| R8 | One bounded recovery turn after a retryable provider or loop failure (deeper actions follow) | WANT | 3 |
| R9 | An opt-in verify hook blocks completion on new failures only, relative to a per-project baseline | WANT | 3 |
| R10 | `NoProgress` and `RepeatedIdenticalEdit` breakers emit real events | WANT | 3 |
| R11 | Opt-in rollback restores the turn snapshot on verify failure in `auto` mode | WANT | 4 |
| R12 | `TurnFailed` hook event fires on a failed turn with the error payload (observational) | WANT | 4 |
| R13 | Provider retry classifies by HTTP status and adds jitter | WANT | 4 |
| R14 | Definition text is capped and the subagent catalog is compact | WANT | 2 |
| R15 | Cache-safe compaction reuses the parent prefix | WANT | 2 |
| R16 | Tool arguments are coerced to the tool schema before dispatch | WANT | 5 |
| R17 | The request projection is cached by event count and policy | WANT | 5 |
| R18 | A rejected tool call adds a schema-based argument-repair hint | WANT | 6 |
| R19 | `context.recovery_model` selects the recovery model and restores the session model | WANT | 6 |
| R20 | A missing `old_string` is repaired from a unique whitespace/line-prefix match, or returns candidates | WANT | 6 |

## 6. Acceptance criteria

**AC-1 — Cache intent reaches the provider**
Given `context.prompt_caching` is unset or true, when a step request is built, then it carries a cache
control and the response usage is copied into `Usage.cached_tokens`.
Failure: a build with caching disabled sends no cache control and shows no cached tokens.

**AC-2 — Cache hits are visible**
Given a provider returns `cached_tokens`, when a headless turn ends, then the stderr summary includes
`cached=…`, and the TUI session line includes cache tokens.
Failure: a provider that returns no cache details produces an unchanged summary.

**AC-3 — Live repair closes the cancel gap**
Given a turn is cancelled during planning after an assistant message with tool calls, when the turn
ends, then one error `ToolResult` exists for each unpaired call before the next prompt.
Failure: a second open adds no duplicate repair.

**AC-4 — Repair is idempotent**
Given a session that was already repaired, when it is reopened, then no `ToolResult` is added.
Failure: a fork copies a repair that was not in the source.

**AC-5 — Long-turn compaction**
Given a turn crosses the watermark after the verbatim window, when a balanced step boundary is
reached, then compaction applies at most once in that step and at most twice in the turn.
Failure: no balanced boundary exists; the attempt is logged as skipped and the turn continues.

**AC-6 — Old tool results become stubs**
Given a session has more turns than `verbatim_window_turns`, when a request is projected, then
results from older turns are stubs that keep the call pair and name the spill blob, and the newest
window stays verbatim.
Failure: setting `clear_old_tool_results: false` keeps every result verbatim.

**AC-7 — Summary reuses the cached prefix**
Given `context.summarizer_model` is unset, when compaction builds its request, then it carries the
parent system prompt and tool definitions and appends `SUMMARY_INSTRUCTION` as the last user message.
Failure: a configured summarizer model keeps the standalone transcript request.

**AC-8 — Verify hook is installed once**
Given the user runs `meerai hooks verify --command "cargo test"`, when the user hooks file has no
matching Stop hook, then one Stop hook is added; a second run adds none.
Failure: an existing unrelated hook is removed.

**AC-9 — Verify hook blocks on new failures**
Given the baseline has recorded `tests::a`, when the verify command fails with `tests::b`, then the
hook blocks with `new verify failures: tests::b`; when it fails with only `tests::a`, it passes.
Failure: the first failing run blocks before a baseline exists.

**AC-10 — One recovery turn after a retryable failure**
Given a turn fails with a retryable provider error or `LoopDetected`, when the worker handles the
failure, then it queues one recovery prompt before the next queued prompt and starts a new turn; a
second consecutive failure does not recover. A headless run consumes the `RecoveryStarted` event and
exits `0` when the recovery turn completes.
Failure: a budget, config, tool, or non-retryable provider failure queues no recovery.

**AC-11 — TurnFailed hook payload**
Given a turn fails, when the `TurnFailed` hook runs, then its JSON stdin carries
`event: "TurnFailed"` and the error text in `error`, and a `HookFired` event is logged.
Failure: a successful turn fires no `TurnFailed` hook.

**AC-12 — Permanent provider errors fail fast**
Given a provider request fails with a 4xx status other than 408/409/425/429, when the agent classifies
it, then it does not retry and reports a non-retryable provider error; a 429 or 5xx retries with
jittered backoff.
Failure: a 401 or 400 consumes all three attempts.

**AC-13 — Baseline lifecycle**
Given no baseline file, when the verify command fails, then the hook records the failures and passes.
When the command passes later, the baseline is removed; `--reset-baseline` removes it without
changing the hook.
Failure: a removed failure stays in the baseline.

**AC-14 — Auto-mode rollback on verify failure**
Given `context.rollback_on_verify_failure` is true and the mode is `auto`, when the Stop hook blocks
with a verify failure after a write, then the turn's files are restored and
`RewindApplied { scope: Code, replaced: None }` is logged; the turn continues once.
Failure: in `ask`/`plan`, files are not restored and a notice points to `/rewind`.

**AC-15 — Arguments are coerced to the schema**
Given a tool parameter expects an integer and the model sends `"3"`, when the call is dispatched,
then the tool receives `3`; a scalar for an array parameter is wrapped; a null optional property is
removed.
Failure: a valid value is rewritten.

**AC-16 — Projection is reused**
Given the session event count and projection policy are unchanged, when the request projection is
built twice, then both calls return the same messages without re-deriving them. New events or a
policy change recompute it.
Failure: a stale projection omits a newly logged message.

**AC-17 — Rejected arguments get a repair hint**
Given a tool rejects its arguments, when the result is logged, then a RuntimeContext message names the
tool, includes its parameter schema, and asks for corrected JSON.
Failure: a successful call adds the hint.

**AC-18 — Recovery uses the recovery model**
Given `context.recovery_model` is set, when a retryable failure starts a recovery turn, then that turn
uses the recovery model and the session model is restored afterward. A project `recovery_model` with
`:` is ignored so it cannot redirect the provider.
Failure: a successful normal turn uses the recovery model.

**AC-19 — Edit anchor repair**
Given `old_string` is not found exactly but matches one place after trimming whitespace or
`read_file` line prefixes, when the edit runs, then it applies once and reports “Anchor repaired”.
Given multiple normalized matches, no file changes and the line numbers are reported. Given no match,
up to three closest lines are reported.
Failure: an ambiguous normalized anchor edits the file.

## 7. Impact

- Tenancy and authorization: none.
- Entitlements: none.
- Offline and local-first: cache control is a request field; offline behavior is unchanged.
- Data lifecycle: no new durable data; stubs point at existing blobs.
- Contracts: `Usage` gains `cache_creation_tokens`; `ProjectionPolicy` gains `clear_old_tool_results`;
  `run_compaction` returns `Result<bool, TurnError>`; `meerai_core::lenient::coerce_to_schema` is a new
  public helper; `ContextSettings` gains `recovery_model`; `Session::event_count` exposes the cache
  key. `UsageUpdated` already carries `Usage`, so the event shape changes additively and must update
  the TUI fold, headless NDJSON, and replay together.
- Surfaces: `meerai-core`, `meerai-agents`, `meerai-session`, `meerai-common`, `meerai-cli`,
  `meerai-tui`.
- Localization: none.
- Failure states: a provider that rejects cache control must fail open to an uncached request.
- Observability: `UsageUpdated` and the headless summary expose cache hits.
- Rollout: `prompt_caching` defaults on; a false value restores current behavior. Argument coercion is
  always on and leaves valid values unchanged; the projection cache is internal. `recovery_model` is
  unset by default; a project value with `:` is ignored so it cannot redirect the provider.
- Support: cache fields are self-describing in the summary and the TUI.

## 8. Non-goals

- Frozen/shared message representations and incremental tail projection. The full-result
  projection cache is landed; the next step is incremental projection from a replacement point.
- Progressive tool disclosure and tool-search meta-tools.
- A persistent memory tool and cross-session notes.
- Cache-aware USD pricing.
- Replacing the compaction window with a sliding verbatim window.
- A model-fallback chain or a learned-fix store.
- Web or desktop surfaces.

## 9. Phases

1. **Phase 1 — Cache and live repair.** R1, R2, R3, R7. Contracts first (`Usage`), then the request
   option, then the consumers. Gate: `cargo test -p meerai-session -p meerai-agents -p meerai-tui -p
   meerai-cli` plus the workspace gate. Landed.
2. **Phase 2 — Context pruning.** R4, R5, R6, R14, R15. Projection and compaction only. Gate:
   projection snapshots and compaction tests. Landed.
3. **Phase 3 — Verified recovery.** R8, R9, R10. Interceptor and hook changes. Gate: loop and breaker
   tests with a real verifier command. Landed: the `meerai hooks verify` Stop-hook preset with a
   per-project failure baseline (R9), the `RepeatedIdenticalEdit` and `NoProgress` breakers (R10), and
   one bounded recovery turn (R8 v1) in the worker and headless. Deeper recovery actions remain.
4. **Phase 4 — Deeper resilience.** R11, R12, R13. Rollback, `TurnFailed`, and provider
   classification. Gate: recovery tests plus a hook round-trip. Landed v1: `TurnFailed`,
   status-classified retries, and auto-mode rollback. A dedicated rollback approval and shell-edit
   capture remain.
5. **Phase 5 — Dispatch and projection efficiency.** R16, R17. Schema-aware argument coercion and the
   full-result projection cache. Gate: coercion and cache tests. Incremental tail projection and
   frozen messages remain.
6. **Phase 6 — Recovery depth.** R18, R19, R20. A schema-based argument-repair hint, a configurable
   recovery model, and edit-anchor repair. Gate: repair-hint, recovery-model, and edit-repair tests.
   A per-turn recovery budget and model escalation chains remain. Plus post-Phase-6 fixes: `ContextOverflow`
   only retries when `run_compaction` returns `Ok(true)`, `sac-run_script` `path` arguments pass through
   `CodeTools::resolve_for_read`, and inline `sac-run_script` approvals carry a `<language>: <first-line>`
   resource preview.
7. **Phase 7 — Custom-script & command sandboxing (`sac-run_script`, `code-shell`, `ExecutionWorld`).**
   *Landed v1* in `meerai-tools/src/world.rs`, `meerai-tools/src/code/shell.rs`,
   `meerai-tools/src/search_as_code/mod.rs`, `meerai-common/src/settings.rs`, and `meerai-cli/src/main.rs`:
   - *Sensitive-path masking inside Seatbelt & Bubblewrap:* `seatbelt_profile` and `bwrap_args`
     deny reads (and writes in `WorkspaceWrite`) for sensitive user paths (`~/.ssh`, `~/.aws`,
     `~/.gnupg`, `~/.kube`, `~/.docker`, `~/.netrc`, `_netrc`, `.npmrc`, `.pypirc`, `.git-credentials`,
     `meerai` config/data dirs) and bounded workspace sensitive files (`is_sensitive_path`, depth <= 5,
     max 256 files, including gitignored `.env*`), masking them in `bwrap` after `--bind`.
   - *Expanded child env scrubbing & stream caps:* `scrub_command_env` strips 25 credential names
     and 17 secret/token/password/credential suffixes; `run_command` caps captured stdout/stderr at
     1 MiB per stream while draining pipes.
   - *Interpreter hardening:* `sac-run_script` uses `python3 -I -B` (isolated + no `.pyc` writes)
     and probes Node `--permission` / `--experimental-permission` with `--allow-fs-read=<cwd>` for
     `javascript` (do not adopt `bun`, which lacks a capability sandbox of its own).
   - *Opt-in fail-closed fallback:* `sandbox.require_for_scripts` and `sandbox.require_for_shell`
     (`SandboxSettings`, tighten-only in `.meerai/settings.yml`) refuse `HostWorld` when an OS
     sandbox is unavailable.

   *Remaining Phase 7 backlog:*
   - *Strict read/env allowlists & `RLIMIT_*`:* optional allowlist-only read mounts for
     `SandboxPolicy::ReadOnly`, minimal env allowlist mode, and safe `setrlimit` CPU/core/fsize
     bounds in `pre_exec`.
   - *Cross-platform gaps:* in-process Linux Landlock + seccomp fallback when `bwrap` is absent,
     Windows Job Objects / AppContainer, and surfacing `ExecutionWorld::warning()` in the TUI
     statusline.
   - *Capability / embedded script runtime (long-term):* an embedded WASM/WASI or QuickJS runtime
     backed directly by `CodeTools::resolve_for_read` (or `deno` with `--allow-read=<workspace>`)
     so `sac-run_script` needs zero host interpreter or host FS visibility.
8. **Phase 8 — Next-wave harness efficiency & self-healing.** Proposed improvements across the loop:
   - *Incremental projection & stable prefix:* cache `derive_messages` incrementally from a tail index
     and keep `RuntimeContext` / scoped `AGENTS.md` insertions out of the cached prefix (append after
     the stable conversation turns) so prompt-cache hit rates do not drop on `code-read_file`
     directory switches or argument-repair hints.
   - *Partial-stream & `ContinueOnce` wiring:* either emit `BreakerAction::ContinueOnce` when a
     recovery continuation is injected (closing the last unused enum variant in `meerai-session`) or
     remove the dead variant; preserve already-received stream chunks or retry safely on mid-stream
     truncation before a tool call executes.
   - *Per-turn recovery budget & model escalation:* replace the single boolean recovery flag with a
     configurable `RecoveryBudget` (max retries, max recovery turns, optional ordered fallback list)
     and wire `PostToolUseFailure` / `TurnFailed` structured context into the recovery prompt.
   - *Subagent & ACP token/sandbox alignment:* expose `sac-run_script` sandbox posture and per-turn
     tool-output spill thresholds consistently to child sessions, and surface cache-hit / cost-saved
     metrics per turn in headless `--json` and TUI session metrics.

## 10. Open questions

| # | Question | What settles it | Owner |
| :--- | :--- | :--- | :--- |
| 1 | How many tokens do the 15 default tool schemas cost? | Serialize the schemas and count; compare with provider-reported prompt tokens | Agents |
| 2 | Do default routes cache implicitly today? | One live turn with cached-token logging enabled | CLI |
| 3 | Is dropping assistant text on tool-call messages intentional? | Author intent or a regression test (`meerai-session/src/projection.rs:104-111`) | Session |
| 4 | Is the dead `verbatim_window_turns` projection field reserved for a sliding window? | **Settled:** active in `meerai-session/src/projection.rs:210` (`clear_old_tool_results` keeps the newest N turns verbatim) | Session |
| 5 | Is the live cancel gap a defect or documented behavior? | **Settled:** fixed by Q5 / AC-3 (`heal_dangling_tool_calls` at `TurnEnded` in `meerai-agents/src/multi_turn_agent/turn.rs`) | Agents |
| 6 | Should `sac-run_script` fail closed when `ExecutionWorld::is_sandboxed()` is false? | **Settled:** warn by default; opt-in fail-closed via `sandbox.require_for_scripts` and `sandbox.require_for_shell` (`SandboxSettings`) | Tools / Policy |
| 7 | Which system read prefixes are minimal yet sufficient for `python3`/`node` under Seatbelt & `bwrap`? | Empirical smoke matrix on macOS (`pyenv`/Homebrew/system Python, `nvm`/Homebrew Node) and Linux | Tools |
