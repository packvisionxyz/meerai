---
name: review
description: Review a pull request, a local change, or a document against the codebase and its rules. Finds contract breaks, missing tests, security regressions, and documentation drift. Reports findings by severity with path:line and never edits.
license: MIT
compatibility: Works with any agent supporting the Agent Skills standard. Read-only by design; no edits, no commits, no approvals. Pull-request mode uses the repository's own forge CLI when present.
metadata:
  version: "1.0.0"
  tags: "review, pull-request, diff, documentation, verification"
---

# Reviewer

Judge the change that exists, not the plan for it and not the author's account of it. The output is a
verdict and a list of findings with evidence, not a rewrite.

## Boot: load the review contract

Read `.agents/repo/profile.md`:

- Invariants and frozen decisions — what a change may never break.
- Canonical patterns — the sibling implementation every layer must follow.
- Verification matrix — which command proves which area.
- Documentation conventions — where a fact lives and which files must move together.

Then read the rules nearest the code: root `AGENTS.md` or `CLAUDE.md`, and the package rules for
every touched path. Rules nearest the code win. If the profile is missing, discover the same facts
from the root rules, manifests, and CI.

## 1. Establish the target and the base

State the review target before reading code:

| Target | Base |
| :--- | :--- |
| Pull request | The PR base branch or merge base, fetched, never the author's summary |
| Local change | `HEAD` for staged and unstaged work, plus untracked files that belong to the change |
| Commit range | The exact old and new revisions |
| Document | The document, plus the code and config it makes claims about |

Record how you obtained the diff and the revisions you reviewed. Never review a range you guessed.
See [`references/review-playbook.md`](references/review-playbook.md) for the commands.

## 2. Read in this order

1. The diff, once, for intent and scope.
2. The surrounding unit for every hunk, not only the hunk.
3. Every caller and consumer of a changed shared unit: types, wire enums, event shapes, DTOs,
   schemas, routes, CLI flags, config keys.
4. The tests that should cover the new behavior, if they exist.
5. The rules and patterns the touched layer must follow.

The review is read-only. Do not edit, format, stage, commit, or push. Run a check only when it is
read-only and it settles a claim; prefer the narrow test for the touched units over the full gate.

## 3. What to inspect

| Category | Question that finds real defects |
| :--- | :--- |
| Intent and scope | Does the diff do what it claims, and only that? Any opportunistic rename, reformat, or refactor? |
| Contracts | Is a wire enum, event, DTO, schema, or replay shape changed without every consumer in the same change? |
| Behaviour | Which branch, empty input, boundary, retry, timeout, or cancellation path is unhandled? |
| Tests | Is new behavior covered by a test that would fail without the change? Does a test assert the wrong thing? |
| Errors and resources | Is every new effect, thread, task, socket, file handle, or subscription released? Is an internal cause exposed on the wire? |
| Security | Does a secret reach output, logs, or a child environment? Do permissions tighten only? Does a read-only mode stay read-only? |
| Data | Is a migration reversible, a field renumbered, or an existing row rewritten without a backfill? |
| Concurrency | Is an await held across a lock, or a shared value read without the same ordering as its writer? |
| Docs and config | Do the docs, defaults, examples, and profile still describe the new behavior? |

Generated and vendored paths are read to understand a contract, never as the place a fix belongs.

## 4. Review a document against the codebase

Use when the ask is "is this doc still true". Do not summarize the document; verify it.

1. Build a claim inventory: every command, path, default, limit, flag, permission, and behavior the
   document asserts.
2. Verify each claim against code, config, schema, or CI. Cite the evidence.
3. Classify each claim: confirmed, stale, missing, or contradicts the code. A confirmed claim needs no
   action; the other three do.
4. Report the document location and the smallest correction, or the code defect the document exposed.

A document that is accurate but omits a user-facing behavior is a finding, not a pass. See
[`references/doc-drift.md`](references/doc-drift.md).

## 5. Severity

| Level | Meaning |
| :--- | :--- |
| Blocker | Breaks a contract or invariant, leaks a secret, corrupts or loses data, or fails a check the change itself touches. |
| Should fix | Unhandled error or resource, missing test for new behavior, documentation drift in the same change, scope creep. |
| Note | Naming, structure, or optional improvement that does not block the change. |

Rank by user or data impact, not by how easy the fix is.

## 6. Report

Report every finding with `path:line`, the failure it causes, and the smallest fix. Add an explicit
"no findings" for each category you inspected. Separate defects the change introduced from ones that
already existed.

```markdown
## Verdict
<one line: ready / ready with fixes / blocked>

## Blockers
- `path:line` — what breaks. Fix: smallest change.

## Should fix
- `path:line` — what breaks. Fix: smallest change.

## Notes
- `path:line` — observation.

## Docs
- `path` — claim, evidence, status.

## Inspected, no findings
- contracts, tests, security, ...

## Pre-existing
- `path:line` — defect that predates this change.
```

## Delegation

- `reviewer` — a bounded area or an independent second pass.
- `recon` — trace a flow or find the canonical sibling before judging a pattern.
- `search` — one narrow locate, rule, or ownership question.
- `verifier` — when a gate must actually run; it reports real output and never edits.

Run independent areas in parallel. Each brief must stand alone: target, base, area, and the evidence
shape you require. Keep the review read-only in every delegate.

## Handoff

- Fixes needed → the **implement** skill.
- A change too large to review in one pass → the **orchestrate** skill.
- A document only → the **specify** skill for the rewrite, after your findings.
- Green and complete → say so once, with the evidence. Never approve on the author's behalf, post a
  review, or push unless the user asks.