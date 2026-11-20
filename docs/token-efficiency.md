# Token Efficiency and Self-Healing

This document records the token-cost and self-healing initiative: what is
implemented in the tree, the decisions behind it, and what remains planned.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-17 |
| **Audience** | Contributors and maintainers of the agent loop, session log, and tools |
| **Scope** | `meerai-agents`, `meerai-session`, `meerai-tools`, `meerai-core`, `meerai-common`, `meerai-cli`, `meerai-tui` |
| **Related** | [harness-spec.md](harness-spec.md) · [architecture.md](architecture.md) · [threat-model.md](threat-model.md) · [harness-internals.md](harness-internals.md) |

## 1. Overview

meerai re-sends the projected conversation on every model step. Without
cache intent, pruning, and bounded recovery, long sessions pay full prompt
cost repeatedly and failed turns need manual repair. This initiative lowers
the cost per turn and raises the share of failed turns that recover without
user input, with no loss of session-log fidelity.

The work has two tracks. The **token track** enables provider prompt caching,
reports cache hits, prunes old tool results through the projection, and
removes the compaction dead zones. The **healing track** repairs dangling
tool calls in the live process, adds a bounded recovery turn, and gates
completion on an external verifier. All changes use the existing seams:
projection rules, `HarnessConfig`, `Interceptor`, `HookEngine`, and
`AgentCommand`.

Phases 1–7 are implemented (v1 where noted). Phase 8 and the remaining
Phase 7 hardening are planned (§3).

## 2. Implemented

### 2.1 Prompt caching and cache reporting (phase 1)

Every model step sends a request-level cache intent, and provider cache
counters are reported end to end.

- `context.prompt_caching` (default `true`) sets
  `CacheControl::Ephemeral` on every step request and on the compaction
  summary request (`meerai-agents/src/harness.rs`,
  `multi_turn_agent/turn.rs`, `multi_turn_agent/compaction.rs`). Setting it
  to `false` sends no cache control.
- With the native Anthropic adapter, genai places a breakpoint on the static
  tools+system prefix. The native OpenAI adapter uses the intent for
  GPT-5.6 and later. OpenAI-compatible gateways such as OpenRouter send
  nothing extra and keep their provider-native implicit caching.
- `Usage` carries `cached_tokens` and `cache_creation_tokens`
  (`meerai-session/src/event.rs`), copied from the provider's
  `prompt_tokens_details` and summed across steps.
- The headless stderr summary prints `cached=<n>` when the provider reports
  cache reads; the TUI session line shows `↻<n>`
  (`meerai-cli/src/headless.rs`, `meerai-tui/src/ui/statusline.rs`).
- Cache counts do not change `est_cost_usd`. Pricing stays token-only
  (decision Q2).

### 2.2 Live tool-call repair (phase 1)

A cancelled or loop-detected turn never leaves an unpaired tool call for the
next prompt.

- `Session::open_inner` heals dangling calls on open, and `run_turn` heals
  again after `TurnEnded` through the same idempotent
  `Session::heal_dangling_tool_calls` helper
  (`meerai-session/src/session.rs`, `multi_turn_agent/turn.rs`). Each
  unpaired call gets one error `ToolResult` ("tool execution interrupted
  before completion").
- Healing skips rewound and compacted ranges, never repeats on later opens,
  and runs on both the success and failure paths of a turn. A fork therefore
  does not copy a repair that the source turn did not need.
- Read-only replay uses `Session::load_events` and never heals.

### 2.3 Tool-result clearing (phase 2)

Old tool results leave the working context without touching the log.

- `ProjectionPolicy::clear_old_tool_results` (default `true`) replaces tool
  results outside `context.verbatim_window_turns` (default 4) with a short
  stub. The call stays paired, the newest window stays verbatim (even with a
  zero window), and a spilled stub names its blob
  (`meerai-session/src/projection.rs`).
- The log keeps every result. Clearing is a projection rule, never a
  log-write rewrite.
- The same-model summarizer keeps the parent clearing policy so its cached
  prefix matches; a distinct `context.summarizer_model` disables clearing so
  its input stays complete.

### 2.4 Compaction without dead zones (phase 2)

A long turn can compact before it blocks.

- Pressure compaction applies at most twice per turn and at most once per
  step (`MAX_PRESSURE_COMPACTIONS_PER_TURN = 2`,
  `multi_turn_agent/{mod,turn}.rs`). `run_compaction` returns
  `Result<bool, TurnError>`; `Ok(false)` means skipped (open bracket, hook
  block, or no balanced boundary).
- When too few turns exist for a turn boundary, the planner falls back to
  the last tool-pairing-balanced `StepEnded` outside the newest window
  (`meerai-session/src/compaction.rs`). A boundary never cuts between a tool
  call and its result.
- The watermark estimate is schema-aware: the turn's tool-schema token
  estimate is summed into the pressure check.
- With no `context.summarizer_model`, the summary request reuses the parent
  system prompt, tools, and projected history and appends
  `SUMMARY_INSTRUCTION` as the last user message, so the provider cache
  stays warm. A distinct summarizer model uses a standalone transcript
  request with the instruction in the system slot and clearing disabled.
- `ContextOverflow` retries once, and only when `run_compaction` returns
  `Ok(true)`; `Ok(false)` fails the turn cleanly
  (`multi_turn_agent/turn.rs`).
- Definition text is capped at 16 KiB per definition and 32 KiB total with a
  truncation warning; the `subagent-run` schema lists agent ids only
  (`meerai-cli/src/main.rs`, `meerai-agents/src/subagent.rs`).

### 2.5 Verified recovery (phase 3)

Completion can be gated on an external verifier, and loops are bounded.

- `meerai hooks verify [--command CMD] [--reset-baseline]` installs one user
  `Stop` hook with a per-project failure baseline
  (`meerai-cli/src/hooks_cmd.rs`). The first failing run records the known
  failures and passes; later runs block only on new failure names; a passing
  run deletes the baseline. Cargo test failure blocks and pytest `FAILED`
  lines are recognized; other commands use a command-level baseline. The
  command auto-detects `cargo test` or `npm test`. `--reset-baseline` deletes
  the baseline and returns without installing a hook.
- Circuit breakers (`multi_turn_agent/{turn,tools}.rs`):
  - **RepeatedToolFailure** — the 3rd identical failing call injects a
    model-visible diagnostic; the 4th fails the turn with `LoopDetected`.
  - **Identical batches** — the 3rd identical batch injects a diagnostic;
    the 5th fails the turn. Edit/write-only batches also log
    `CircuitBreaker { RepeatedIdenticalEdit }` events; other batches log the
    diagnostic as a runtime message only.
  - **NoProgress** — 8 tool steps without a file change (after a write
    attempt) inject a diagnostic; 12 fail the turn. Disabled in `plan` mode
    and when no write tool is registered; a `subagent-run`/`acp-run`
    delegation resets the counter.
- One bounded recovery turn follows a retryable provider error or a
  `LoopDetected` failure (`multi_turn_agent/worker.rs`,
  `MAX_RECOVERY_ATTEMPTS = 1`). The user-visible prompt names the failure
  and runs before the next queued prompt. Budget, config, tool, and
  non-retryable provider failures do not recover. Headless runs wait for the
  recovery turn (`RecoveryStarted`) and exit `0` when it completes
  (`meerai-cli/src/headless.rs`).

### 2.6 Deeper resilience (phase 4)

- `context.rollback_on_verify_failure` (default `false`) restores the turn
  snapshot when a blocking `Stop` hook reports a verify failure in `auto`
  mode, and logs `RewindApplied { scope: Code, replaced: None }`
  (`multi_turn_agent/mod.rs::rollback_after_verify_failure`). In `ask`/`plan`
  it does not restore; a notice points to `/rewind`. Rollback is partial by
  design: capture excludes shell edits (decision Q7).
- `HookEvent::TurnFailed` fires after a failed turn with the error in the
  JSON `error` payload. Its decision is logged but does not change the
  failed turn (decision Q9).
- Provider retries classify by HTTP status when genai exposes one:
  408/409/425/429 and 5xx are transient; other 4xx are permanent. A message
  blocklist is the fallback. Backoff is 250/500 ms plus up to 50% jitter,
  3 attempts total (`multi_turn_agent/{mod,turn}.rs`).

### 2.7 Dispatch and projection efficiency (phase 5)

- Tool arguments are coerced to the tool schema before dispatch: string
  numbers/booleans become typed values, a scalar for an array parameter is
  wrapped, and null optional properties are removed
  (`meerai-core/src/lenient.rs::coerce_to_schema`, called from
  `multi_turn_agent/tools.rs`). The lenient parser first repairs malformed
  JSON (JSON-in-string unwrap, wrapper objects, invalid escapes). Valid
  values are never rewritten.
- The request projection is cached. `request_messages` reuses the derived
  messages while the session event count and projection policy are
  unchanged; new events or a policy change recompute it
  (`multi_turn_agent/mod.rs::ProjectionCache`). The cache is invalidated on
  `set_harness`, `reload_config`, and `reset`. It is an internal
  optimization: `Session::derive_messages` remains the single pure
  projection, and the cache key relies on the append-only log.

### 2.8 Recovery depth (phase 6)

- A rejected tool call logs a `RuntimeContext` message with the tool's
  parameter schema and asks for corrected arguments
  (`multi_turn_agent/tools.rs`).
- `context.recovery_model` selects the model for the recovery turn; the
  session model is restored afterward and the `sessions` row keeps the session
  model (`meerai-common/src/settings.rs`, `multi_turn_agent/worker.rs`). A
  project value containing `:` is dropped so it cannot redirect the
  provider.
- Edit-anchor repair: a missing `old_string` is normalized (whitespace and
  `read_file` line prefixes). A unique normalized match applies once with an
  "Anchor repaired" note; ambiguous matches report line numbers without
  editing; no match suggests up to three closest lines
  (`meerai-tools/src/code/edit_file.rs`).
- Post-phase-6 fixes: `ContextOverflow` retries only on `Ok(true)` (§2.4);
  `sac-run_script` `path` arguments pass through `CodeTools::resolve_for_read`;
  inline `sac-run_script` approvals carry a `<language>: <first-line>`
  resource preview.

### 2.9 Script and command sandbox hardening (phase 7, v1)

Implemented in `meerai-tools/src/world.rs`, `code/shell.rs`,
`search_as_code/mod.rs`, `meerai-common/src/settings.rs`, and
`meerai-cli/src/main.rs`. Security effects and residual risks are owned by
[threat-model.md](threat-model.md).

- **Sensitive-path masking.** The Seatbelt profile and bwrap arguments deny
  reads (and writes in `WorkspaceWrite`) on sensitive host paths (`~/.ssh`,
  `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.docker`, `.netrc`, `_netrc`, `.npmrc`,
  `.pypirc`, `.git-credentials`, meerai config/data dirs) and on bounded
  workspace sensitive files (`is_sensitive_path`, depth ≤ 5, max 256 files,
  including gitignored `.env*`). bwrap masks them after `--bind`.
- **Environment scrubbing and stream caps.** `scrub_command_env` strips 25
  named credential variables and 17 secret/token/password/credential
  suffixes, case-insensitively. `run_command` caps captured stdout/stderr at
  1 MiB per stream while draining the pipes.
- **Interpreter hardening.** `sac-run_script` runs `python3 -I -B` and
  probes Node `--permission` / `--experimental-permission` with
  `--allow-fs-read=<cwd>`.
- **Fail-closed fallback.** `sandbox.require_for_shell` and
  `sandbox.require_for_scripts` (tighten-only from a project file) refuse
  `HostWorld` when no OS sandbox is available.
- **Strict environment and resource limits.** `sandbox.strict_env` clears
  the child environment and keeps only `PATH`, `HOME`, `USER`, `LOGNAME`,
  `SHELL`, `TERM`, `TMPDIR`, `PWD`, `LANG`, and `LC_*`; the secret scrub also
  runs on the child environment. On Unix, `pre_exec` sets `RLIMIT_FSIZE`
  (1 GiB) and `RLIMIT_CORE` (0).

## 3. Planned

| Item | Description | Gate |
| --- | --- | --- |
| Incremental projection | Cache `derive_messages` incrementally from a tail index and rebuild from a replacement point, instead of the current full-result cache. | Projection-cache seam (§2.7); purity must hold |
| Stable prefix ordering | Keep `RuntimeContext` / scoped `AGENTS.md` insertions out of the cached prefix so directory switches and repair hints do not drop cache-hit rates. | harness-spec §2.5 multi-section prefix |
| Partial-stream recovery | Preserve already-received stream chunks or retry safely on mid-stream truncation before a tool call executes. | Stream contract review |
| Per-turn recovery budget | Replace the single recovery attempt with a configurable `RecoveryBudget` (max retries, max recovery turns, optional ordered model fallback) and feed `PostToolUseFailure`/`TurnFailed` context into the recovery prompt. | Recovery v1 (§2.5) |
| Subagent/ACP alignment | Expose the `sac-run_script` sandbox posture and per-turn spill thresholds consistently to child sessions; surface cache-hit metrics per turn in headless `--json` and TUI metrics. | Subagent runtime |
| Read allowlist mounts | Optional allowlist-only read mounts for `SandboxPolicy::ReadOnly` (today both sandbox builders allow host-wide reads so toolchains work). | Empirical read-prefix matrix (open question 3) |
| CPU rlimit | A safe `RLIMIT_CPU` bound in `pre_exec` (FSIZE and CORE are implemented). | Platform review |
| Linux in-process fallback | Landlock + seccomp when `bwrap` is absent. | Platform work |
| Windows sandbox | Job Objects / AppContainer for `ExecutionWorld`. | Platform work |
| Sandbox warning in TUI | Surface `ExecutionWorld::warning()` in the statusline, not only startup stderr. | TUI design |
| Embedded script runtime | WASM/WASI or QuickJS backed by `CodeTools::resolve_for_read` (or `deno --allow-read=<workspace>`) so `sac-run_script` needs no host interpreter. Do not adopt `bun` (no capability sandbox). | Long-term |

Non-goals: frozen/shared message representations; progressive tool
disclosure and tool-search meta-tools; a persistent memory tool; cache-aware
USD pricing; a sliding verbatim window replacing the compaction window; a
model-fallback chain or learned-fix store; web or desktop surfaces.

## 4. Reference

### 4.1 Requirements status

All twenty requirements are implemented. Evidence is the owning module;
behavioral detail is in §2.

| ID | Requirement | Phase | Evidence |
| --- | --- | --- | --- |
| R1 | Cache intent when `prompt_caching` is true | 1 | `multi_turn_agent/turn.rs` |
| R2 | `Usage` carries cache read/write counts | 1 | `meerai-session/src/event.rs` |
| R3 | Headless and TUI show cache hits | 1 | `meerai-cli/src/headless.rs`, `meerai-tui/src/ui/statusline.rs` |
| R4 | Old tool results stubbed under a keep window | 2 | `meerai-session/src/projection.rs` |
| R5 | Compaction inside a long turn at a balanced boundary | 2 | `meerai-session/src/compaction.rs`, `multi_turn_agent/turn.rs` |
| R6 | Watermark includes tool-schema size | 2 | `multi_turn_agent/{turn,compaction}.rs` |
| R7 | Dangling calls repaired at `TurnEnded` in the live process | 1 | `meerai-session/src/session.rs`, `multi_turn_agent/turn.rs` |
| R8 | One bounded recovery turn after a retryable failure | 3 | `multi_turn_agent/worker.rs` |
| R9 | Verify hook blocks on new failures only | 3 | `meerai-cli/src/hooks_cmd.rs`, `meerai-agents/src/hooks.rs` |
| R10 | `NoProgress` and `RepeatedIdenticalEdit` emit real events | 3 | `multi_turn_agent/turn.rs` |
| R11 | Opt-in auto-mode rollback on verify failure | 4 | `multi_turn_agent/mod.rs` |
| R12 | `TurnFailed` fires with the error payload (observational) | 4 | `meerai-common/src/hooks.rs`, `multi_turn_agent/turn.rs` |
| R13 | Status-classified retries with jitter | 4 | `multi_turn_agent/turn.rs` |
| R14 | Definition text capped; compact subagent catalog | 2 | `meerai-cli/src/main.rs`, `meerai-agents/src/subagent.rs` |
| R15 | Cache-safe compaction reuses the parent prefix | 2 | `multi_turn_agent/compaction.rs` |
| R16 | Arguments coerced to the schema before dispatch | 5 | `meerai-core/src/lenient.rs`, `multi_turn_agent/tools.rs` |
| R17 | Projection cached by event count and policy | 5 | `multi_turn_agent/mod.rs` |
| R18 | Rejected calls get a schema-based repair hint | 6 | `multi_turn_agent/tools.rs` |
| R19 | `recovery_model` selects and restores the model | 6 | `meerai-common/src/settings.rs`, `multi_turn_agent/worker.rs` |
| R20 | Edit-anchor repair or candidate reporting | 6 | `meerai-tools/src/code/edit_file.rs` |

### 4.2 Recorded decisions

| # | Decision | Rationale | Revisit when |
| --- | --- | --- | --- |
| Q1 | Request-level `CacheControl::Ephemeral`, default on | One option, no new dependency; per-message breakpoints need cache-aware ordering first | A provider rejects the shape, or cache reporting shows no benefit |
| Q2 | Cache hits do not change `est_cost_usd` | Cache prices differ by provider and TTL; a wrong discount is worse than none | The catalog carries per-provider cache prices |
| Q3 | Prune old results as a projection rule, log keeps everything | Lossless under the log-is-truth principle, no LLM call | Re-read cost exceeds the saved tokens |
| Q4 | Pressure compaction at a balanced step boundary, ≤1/step and ≤2/turn | A 32-step turn otherwise never compacts; the per-step guard prevents retry loops | Mid-turn summaries degrade answer quality |
| Q5 | Heal after `TurnEnded` in `run_turn`, idempotent helper | Closes the cancel-during-planning gap without a wire-contract change; the projection stays pure | A post-`TurnEnded` result confuses a consumer |
| Q6 | External signals gate automatic retry, with a budget | A model's own opinion is a weak gate | An eval shows self-critique helps a failure class |
| Q7 | Rollback is opt-in, `auto` mode only, logs `RewindApplied { Code }` | Capture excludes shell edits, so rollback is partial; a silent partial rollback hides work | Shell-edit capture lands or a rollback approval exists |
| Q8 | Verify gate is a user `Stop` hook with a per-project baseline | Reuses the hook trust gate, timeout, logging, and one-shot continuation; no second command runner | The hook preset cannot express a required check |
| Q9 | `TurnFailed` is observational in v1 | The turn already failed and the worker owns recovery; a hook rewrite would blur the error contract | Users need hook-driven recovery for an excluded class |

### 4.3 Contract changes shipped

`Usage` gained `cache_creation_tokens`; `ProjectionPolicy` gained
`clear_old_tool_results`; `run_compaction` returns `Result<bool, TurnError>`;
`meerai_core::lenient::coerce_to_schema` is public; `ContextSettings` gained
`recovery_model` and `rollback_on_verify_failure`; `Session::event_count`
exposes the cache key. `UsageUpdated` carries `Usage`, so the event shape
changed additively and the TUI fold, headless NDJSON, and replay were updated
together.

## 5. Open questions

| # | Question | What settles it |
| --- | --- | --- |
| 1 | How many tokens do the default tool schemas cost? | Serialize the schemas and count; compare with provider-reported prompt tokens |
| 2 | Do default routes cache implicitly today? | One live turn with cached-token logging enabled |
| 3 | Which system read prefixes are minimal yet sufficient for `python3`/`node` under Seatbelt and bwrap? | Empirical smoke matrix on macOS (pyenv/Homebrew/system Python, nvm/Homebrew Node) and Linux |

Resolved questions are recorded in §2 and §4.2: assistant text stays beside
tool calls in one projected assistant message; `verbatim_window_turns` is
active in the clearing rule; the live cancel gap is fixed by Q5;
`sac-run_script` warns by default with opt-in fail-closed. The dead
`BreakerAction::ContinueOnce` variant and the unconsumed
`SandboxPolicy::ReadOnly.readable_roots` field were removed; a future
allowlist design reintroduces read scoping deliberately.
