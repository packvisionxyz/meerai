---
name: orchestrate
description: Plan and orchestrate a multi-area feature or initiative using subagents — decompose work, run recon, delegate implementation, integrate contracts, verify, and report. Use when a task spans more than one package or surface, needs parallel work, or is large enough that one uninterrupted edit would be risky.
license: MIT
compatibility: Works with any agent supporting the Agent Skills standard. Requires a subagent/delegation tool for parallel execution; otherwise run the phases inline.
metadata:
  version: "1.3.0"
  tags: "orchestration, planning, subagents, delivery"
---

# Tech Lead

You own the outcome, not the keystrokes. Delegate reconnaissance and bounded implementation; keep
design, integration order, and verification.

## Boot

Read `.agents/repo/profile.md` §2 (layout), §4 (invariants), §5 (patterns), §7 (verification), §11
(frozen decisions). If it is missing, run the **investigate** skill's boot first.

## When to use this

- More than one surface changes (API + client, schema + producer + consumer).
- The work has phases and a merge order.
- Parallel exploration or implementation would help.
- An unreviewed single pass would be risky.

For a one-file fix, use the **implement** skill directly.

## Phase 0 — Frame

Write down before spawning anything:

- **Goal** — one sentence.
- **Done condition** — the command, test, or observable behavior that proves it.
- **Constraints** — frozen decisions, tenancy, entitlements, migrations, generated files.
- **Unknowns** — what recon must answer.

If the goal has two plausible readings, ask the user now. Subagents will confidently implement the
wrong one.

## Phase 1 — Recon

Use the **investigate** skill or spawn read-only recon subagents in parallel, each with one narrow
question. Agent: [`recon`](../../../.meerai/agents/recon.md).

Good recon questions:

- "Where is the current limit enforced, and which files must change to add a new reason?"
- "List every caller of X across server, clients, and tests, with path:line."
- "Which components render this state today, and what stable test hooks do they expose?"

Bad: "understand the codebase", "look around and tell me what to do".

## Phase 2 — Design

Produce a plan before implementation:

1. **Contract first.** Identify the schema/API/event/IPC shape. Land or freeze it before parallel work.
2. **Vertical increments.** Each independently verifiable. Prefer contract → producer → client over
   "all backend, then all frontend".
3. **File ownership.** Disjoint sets per workstream. Two agents never edit the same file in one round.
4. **Integration order.** Codegen → backend → clients → tests. Name the exact commands.
5. **Risks.** Migrations, compatibility, entitlements, generated files, cross-service tickets.

Template: [`references/planning-template.md`](references/planning-template.md).

## Phase 3 — Delegate

Every subagent gets a self-contained brief. Subagents do not see this conversation.

Agent definitions live in `.meerai/agents/` at the project root. Pass the id to the harness's
subagent tool; the definition carries its own model, mode, and permission rules, so do not restate
or widen them.

| Work | Agent | Notes |
| :--- | :--- | :--- |
| Recon | [`recon`](../../../.meerai/agents/recon.md) | read-only |
| Fast lookup | [`search`](../../../.meerai/agents/search.md) | read-only, budgeted |
| Code implementation | [`implementer`](../../../.meerai/agents/implementer.md) | writes the owned paths |
| Verification | [`verifier`](../../../.meerai/agents/verifier.md) | read-only, runs the gate |
| IaC / pipeline | [`infra-implementer`](../../../.meerai/agents/infra-implementer.md) | plans, never applies |
| Infra review | [`infra-reviewer`](../../../.meerai/agents/infra-reviewer.md) | read-only |

Contract: [`references/delegation-contract.md`](references/delegation-contract.md).

Rules:

- One writer per file set. Parallel only when sets are disjoint.
- Use background mode only for work you can integrate later; never background a blocking step.
- Continue a subagent session when the follow-up depends on its earlier work.
- Never delegate commits, pushes, deploys, secret rotation, or destructive git operations.
- A subagent's "done" is a claim until the gate passes in your session.

### Parallel rounds in a shared working tree

In whole-program compiled languages (Rust, Go), every agent in a parallel round shares one build.
A sibling's mid-edit file turns the whole-crate gate red for everyone, and agents block each other
retrying a gate that cannot pass until the round settles. Prevent this in the briefs:

- Give each agent a **scoped gate protocol**: judge compile errors by file path; fix only owned
  files; ignore sibling paths — never fix, revert, or wait on them. The full gate runs once at the
  end; if it is red from sibling files, the agent reports scoped-clean evidence and the failing
  paths. The orchestrator runs the authoritative consolidated gate after the whole round lands.
- Shared contracts consumed by several agents in the same round (event enums, error enums,
  generated types) get **one owner who lands them first**, before the consumers need them.
- Build-tool locks (e.g. Cargo's target dir) serialize concurrent builds. A slow build is normal,
  not a hang; agents must never kill a build or "unblock" the tree.
- Amended instructions to a running agent must target **that agent's existing session**. A message
  sent without its session id spawns a duplicate writer into the same tree.
- When isolation matters more than integration cost, checkpoint-commit the gated tree and run the
  round in per-agent worktrees instead; merge disjoint sets afterwards.

## Phase 4 — Integrate

1. Apply contract changes and regenerate.
2. Merge producer before consumers; consumers compile against generated types.
3. Run the narrow check after each merge; do not stack unverified changes.
4. Resolve conflicting patterns by the owning rules, not by majority.
5. Checkpoint the tree once a round's consolidated gate is green, so the next parallel round
   starts from a known-good, recoverable state. Commit only when the user has explicitly
   authorized commits — a rule you wrote yourself is not authorization; otherwise record the
   green state and leave the changes in the working tree.

## Phase 5 — Verify

Use profile §7. Standard shape:

| Area | Gate |
| :--- | :--- |
| Server | format, vet/lint, unit + integration tests |
| Web / app | package test, typecheck, lint |
| Native / compiled | format, clippy/compiler warnings, tests |
| Cross-cutting | repo-wide format, lint, typecheck, unit suite |
| User-visible flow | the relevant E2E suite, deliberately |
| Infra | fmt/validate/plan/actionlint |

Report real output. Run the gate yourself.

## Phase 6 — Report

```markdown
## Outcome
<what shipped, one sentence>

## Changes by area
- `<area>` — …

## Verification
- `command` — pass
- `command` — fail: <output>

## Decisions
- Q: … → chosen because …

## Risks / follow-ups
- …
```

Commit/PR title follows the repo's convention (profile §8 or the PR template). If the repo enforces a
scope allowlist, use it.

## Stop and ask

- A frozen decision would change.
- A migration, destructive data operation, or production deploy is required.
- Secrets or signing keys are involved.
- Two workstreams need the same file, or the contract cannot be frozen.
- Verification fails for a reason outside the change's scope; report it, do not widen the change.

## Anti-patterns

- Spawning five agents without a contract, then reconciling five incompatible designs.
- Delegating a vague goal and accepting a confident summary.
- Parallel edits to generated output or the same shared file.
- Running the full end-to-end suite as a smoke check.
- Declaring done from subagent output without running the gate.
- Letting a subagent "fix" generated code, frozen decisions, or unrelated lint.
- Requiring a green whole-program gate per agent while several agents share one working tree.
- Messaging a running agent without its session id and spawning a duplicate writer.
- Committing on self-granted authority; commit rights come from the user, not from this skill.

## Supporting files

- [`references/delegation-contract.md`](references/delegation-contract.md) — brief fields,
  parallelization, and harness mapping.
- [`references/planning-template.md`](references/planning-template.md) — workstreams, ownership,
  integration, risks.
