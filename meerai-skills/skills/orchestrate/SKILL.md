# Orchestration

You own the outcome, not the keystrokes. Delegate research and bounded work.
Keep the plan, the integration order, and the observation.

This skill is not only for code. An objective can be a report, a launch, a data
set, a migration, an event, or a repository change.

## When to use it

- The objective spans more than one area, artifact, or actor.
- The work has phases and a merge or delivery order.
- Parallel research or execution would help.
- One unreviewed pass would be risky.

For one small change, apply an implementation skill directly.

## Phase 1 — Plan

If the harness provides `loop-plan`, call it with these fields. Otherwise write
the plan in the session and keep it visible:


- **Objective** — one sentence.
- **Done when** — the observable results that prove completion.
- **Constraints** — frozen decisions, owned resources, budgets, and things you
  must not change.
- **Items** — the workstreams, each with one action and one expected
  observation.

If the request has two plausible readings, ask one question now. A subagent will
confidently deliver the wrong one.

Plan the order:

1. **Shared contract first.** Identify the schema, interface, document outline,
   data shape, or schedule that several workstreams use. Freeze it before
   parallel work.
2. **Vertical increments.** Each increment is observable on its own. Prefer
   contract, then producer, then consumer over "all upstream, then all
   downstream".
3. **Ownership.** Give each workstream disjoint paths, documents, data sets, or
   systems. Two writers never own the same artifact in one round.
4. **Integration order.** Name the exact steps and checks.
5. **Risks.** Compatibility, approvals, migrations, generated output, external
   dependencies.

## Phase 2 — Act

Take one workstream at a time. Then call `loop-act` (or record the action in the
session) with what you did and the observation you expect.

Delegate when it helps:

| Need | Preferred | Fallback |
| :--- | :--- | :--- |
| Research | `research` subagent | research inline |
| Independent work | `general` subagent or a project definition | do it inline |
| Evaluation | `review` subagent | review inline |

Every subagent gets a self-contained brief. Subagents do not see this
conversation. A missing field is a defect.

| Field | What it answers |
| :--- | :--- |
| Context | The objective, the workstream, and the decision already made |
| Read first | Rules, sources, and exact paths to start from |
| Scope | Paths, documents, or systems the subagent owns |
| Out of scope | What it must not touch |
| Done condition | The exact observation that must pass |
| Output | What to return: changes, commands, results, questions |
| Constraints | No external side effects, no silent deviations, no widening |

Rules:

- One writer per artifact. Parallel only when scopes are disjoint.
- The shared contract always lands before parallel work starts.
- Continue a subagent session when the follow-up depends on its earlier work.
- Never delegate irreversible actions: publication, payment, deletion,
  deployment, or secret handling.
- A subagent's "done" is a claim until you observe the result yourself.

### Parallel rounds

- Give each subagent a **scoped check**: judge failures by the artifact it owns;
  ignore sibling work. The full check runs once after the round lands.
- A shared contract that several agents consume gets **one owner who lands it
  first**.
- Build or resource locks are normal. Never kill another agent's work to
  "unblock" a shared resource.
- Amend a running subagent through **its existing session id**. A message sent
  without the session id starts a duplicate writer.

## Phase 3 — Observe

Run the observation named in the plan. Keep the raw result. Call
`loop-observe` with it.

Use the project's own checks: its test command, its linter, its preview, its
query, its measurement. A review by a `review` subagent is evidence only when
it cites the artifact.

## Phase 4 — Adjust

Call `loop-adjust`:

- `done` — the observation proves the workstream.
- `open` — retry it.
- `failed` — return to plan and revise the workstream.

Add newly discovered workstreams in the same call. Resolve conflicting patterns
by the owning rules, not by majority.

## Integrate

1. Apply shared contract changes first.
2. Merge the producer before the consumers.
3. Run the narrow check after each merge. Do not stack unobserved changes.
4. Checkpoint the working state when a round's full check passes.
5. Commit, publish, or deploy only when the user has explicitly authorized it.
   A rule you wrote yourself is not authorization.

## Report

Give:

- The outcome in one sentence.
- Each area with what changed and one line of reasoning.
- Every observation with its real result.
- Decisions with the reason, and open risks or follow-ups.

Follow the project's own commit, publication, or handover convention.

## Stop and ask

Stop and ask when:

- A frozen decision would change.
- An irreversible or external action is required.
- Secrets or credentials are involved.
- Two workstreams need the same artifact, or the contract cannot be frozen.
- A check fails outside the scope of the change. Report it; do not widen the
  change.

## Anti-patterns

- Five agents without a contract, then five incompatible designs.
- A vague brief accepted with a confident summary.
- Parallel edits to one shared artifact or generated output.
- Declaring done without your own observation.
- Letting an agent fix unrelated problems or change frozen decisions.
- Committing, publishing, or deploying on self-granted authority.
