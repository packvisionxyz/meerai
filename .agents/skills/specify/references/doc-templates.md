# Document templates

## Pick the bucket

Use the repo's documentation convention from profile §8. If none is defined, use:

| Bucket | Holds | Status examples |
| :--- | :--- | :--- |
| `architecture/` | True in the main branch today | Shipped, Implemented, Completed |
| `initiatives/` | Approved or in-flight work with remaining scope | In review, Approved design, In progress |
| `ideas/` | Proposed, parked, superseded, brainstormed | Brainstorm, Proposed, Parked, Superseded |
| `concepts/` | Findings, measurements, conventions | Active guide, Convention |

Ask "how much of this is true today?" Not "what is this about?".

Update the repo's document index in the same change.

## Required header

```markdown
# Title

**Date:** YYYY-MM-DD
**Author:** …
**Status:** Brainstorm | Proposed | In review | Approved design | In progress | Shipped | Implemented | Parked | Superseded
**Scope:** `paths/this/doc/touches`
**Related Documents:** [relative link](./other.md)
```

`Scope` lists the paths the document touches. `Related Documents` links prior decisions so the doc does
not re-argue them.

## Idea template

```markdown
## 1. Motivation
Who hurts, when, what they do today, what it costs.

## 2. Options
| Option | What it is | Cost | Risk | Verdict |
| :--- | :--- | :--- | :--- | :--- |

## 3. Recommendation
Direction plus reasoning. Explicitly include "do nothing".

## 4. Constraints applied
Offline, tenancy, entitlements, platforms, locales, region, frozen decisions.

## 5. Open questions
| # | Question | What settles it | Owner |
```

## Initiative template

```markdown
## 1. Summary
What changes, in 3–5 sentences.

## 2. Current state (evidence)
What exists today, with path:line or contract citations.

## 3. Decisions
| # | Decision | Chosen | Rejected | Why |

## 4. Target behavior
Tables for surfaces, states, and failure paths.

## 5. Requirements
| ID | Requirement | Priority (NEED/WANT/INSIST) | Phase |

## 6. Acceptance criteria
Per phase, observable and verifiable.

## 7. Impact
Tenancy, entitlements, offline, lifecycle, contracts, surfaces, localization, failure states,
observability, rollout.

## 8. Non-goals
What this explicitly does not do.

## 9. Phases
Each phase: scope, done condition, verification.

## 10. Open questions
| # | Question | What settles it | Owner |
```

## Requirement row

| Field | Meaning |
| :--- | :--- |
| ID | `R1`, `R2`, … stable across revisions |
| Requirement | One behavior, not a bundle |
| Priority | NEED / WANT / INSIST |
| Phase | Which shippable slice delivers it |
| Evidence | Current state, or "new" with the gap named |

## Decision log

```markdown
**Q1 — <question>**
Chosen: <answer>.
Why: <reason tied to product stance or evidence>.
Rejected: <alternative> — <why it loses>.
Revisit if: <condition>.
```

A decision without "Rejected" is usually a preference, not a decision.

## Acceptance criteria examples

Good:

```markdown
**AC-1 — Blocked region reaches a terminal state**
Given a client outside the supported region, when it calls the API, then it receives a typed
permission error with a stable reason code and the UI shows the localized unavailable state
without retrying.
```

```markdown
**AC-2 — Warm cache plays without upstream work**
Given a cached object inside its TTL, when the user opens the item, then playback starts from the
cache and no upstream build is requested.
```

Bad: "The feature is fast and reliable", "Errors are handled gracefully", "The UI is intuitive".

## Phase definition

Each phase states:

- Scope in one line.
- Done condition: the command, test, or observable behavior that proves it.
- Surfaces touched.
- Deliberately deferred items.

A phase that cannot be verified independently is not a phase.
