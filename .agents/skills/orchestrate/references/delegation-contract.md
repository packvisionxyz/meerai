# Delegation contract

## Required fields

Every subagent brief must contain all seven. A missing field is a defect.

| Field | What it answers |
| :--- | :--- |
| Context | Repo, feature, and the decision already made |
| Read first | Rules, profile sections, and exact files |
| Scope | Paths the subagent owns |
| Out of scope | Paths and categories it must not touch |
| Done condition | Exact commands that must pass |
| Output | What to return: files, commands, results, questions |
| Constraints | No commits/pushes/deploys; no generated edits; no silent deviations |

## Agent types

Map to what the harness offers:

| Role | Preferred | Fallback |
| :--- | :--- | :--- |
| Recon | read-only exploration agent | general agent + "do not edit" |
| Implementation | general implementation agent | same session, inline |
| Verification | general agent, no edits | run gates inline |
| Review | general or exploration agent | inspect the diff inline |

If the harness has no subagents, run the phases inline and keep the same output contract.

## Parallelization

Parallel is safe when all are true:

- File sets are disjoint.
- No shared generated output.
- No shared contract file.
- No dependency between the outputs.

Otherwise sequential. The contract always lands before parallel implementers start.

| Situation | Pattern |
| :--- | :--- |
| Two independent questions | Two recon agents, parallel |
| Contract + producer + client | Contract, then sequential implementers |
| Two disjoint areas | Two implementers, parallel |
| Follow-up depends on earlier work | Continue the same session |
| All writes finished | One verifier |

## Sizing

- A workstream should be reviewable in one pass: roughly one layer, or one vertical slice.
- If a brief needs more than a screen, split it.
- If two workstreams need the same file, merge them.

## Handoff quality

A good result includes:

- Files changed with one line of reasoning each.
- Commands run and real results.
- Deviations reported, not applied silently.
- Open questions with what would settle them.

A bad result: "implemented the feature, all tests pass" with no files, commands, or output.

## Parent responsibilities

- Freeze the contract before delegating.
- Run the gate after integration.
- Keep the diff reviewable.
- Do not let subagents widen scope.
- Do not accept a summary as evidence.
