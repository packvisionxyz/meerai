---
name: specify
description: Brainstorm product ideas and turn them into requirements — problem framing, personas, user stories, acceptance criteria, scope and non-goals, decision logs, and initiative documents. Use when shaping a feature before code, reviewing a proposal, or writing a requirement or initiative doc.
license: MIT
compatibility: Works with any agent supporting the Agent Skills standard. No code changes; documents only.
metadata:
  version: "1.1.0"
  tags: "product, requirements, brainstorming, acceptance-criteria"
---

# PM / BA

Produce decisions and documents, not code. The output is a doc a tech lead can plan from and an
engineer can implement without re-litigating scope.

## Boot: load the product contract

Read `.agents/repo/profile.md`:

- §1 Snapshot — what the product is and the outcome that matters.
- §8 Documentation conventions — where docs go and what header they need.
- §10 Domain, personas, and gating.
- §11 Frozen decisions.

Then read the repo's product docs index (usually under `docs/`) before proposing something already
settled. If the profile lacks personas or terms, ask the user; do not invent a market.

## Product lens

Every requirement is judged by whether it strengthens the product's core outcome or reduces the
friction of delivering it. State that outcome in one sentence at the top of the doc; if the repo
profile does not name one, ask for it.

## Two modes

### A. Brainstorm an idea

Use when the ask is exploratory ("what if…", "should we…", "how would we…").

1. **Restate the problem** in one paragraph: who hurts, when, what they do today, what it costs.
2. **Diverge** — 3–5 distinct options, including "do nothing" and "manual workaround". No filtering.
3. **Constrain** — apply the repo's product stance and invariants from the profile (§4, §10, §11):
   offline behavior, tenancy, entitlements, supported platforms, locales, region.
4. **Converge** — pick a direction and say why; list what was rejected and the reason.
5. **Write** to the repo's idea bucket (profile §8) with a status such as `Brainstorm` or `Proposed`.
6. **Index it** wherever the repo indexes documents.

A brainstorm doc contains a direction and open questions, not an implementation plan.

### B. Write requirements

Use when the direction is chosen and the work needs to be scoped.

1. **Evidence first.** Trace current behavior in code or docs and cite it. "There is no entity for X;
   only aggregate counts exist" beats "X support is missing".
2. **Decide the decisions.** Record each as Q1…Qn with chosen answer, rejected alternatives, and why.
   Unresolved decisions stay open, not silently assumed.
3. **Write the requirement set** — see [`references/doc-templates.md`](references/doc-templates.md).
4. **Impact analysis** — run the checklist below; write "none" explicitly.
5. **Phase it** — each phase independently shippable and verifiable.
6. **Write** to the repo's initiative/design bucket with a status such as `In review` or
   `Approved design`.
7. **Index it** and update related docs.
8. **Hand off** using the contract at the end.

## Impact checklist

Run every row; write "none" explicitly when it does not apply.

| Area | Question |
| :--- | :--- |
| Tenancy / authorization | Does it read or write tenant-scoped data? Where is the tenant resolved? Any new role or permission? |
| Entitlements | Does it create a billable/resource-limited object? Which limit applies, and what happens at the limit? |
| Offline / local-first | Can it work offline? What happens when the edge/agent is offline? What is uploaded by default? |
| Data lifecycle | Retention, deletion, expiry, backfill, migration of existing rows. |
| Contracts | Which API/event/schema changes are required? Backward compatibility? |
| Surfaces | Which clients, workers, docs, and admin surfaces must change? |
| Localization | Every supported locale; docs site included. |
| Failure states | Offline, expired entitlement, quota exhausted, region blocked, upstream down, missing data. |
| Observability | What log, event, or metric proves it works in production? |
| Rollout | Flag, plan gate, migration order, rollback. |
| Support | What does the operator see, and what can support explain without engineering? |

## Acceptance criteria

A criterion is done only if an engineer can verify it without asking you.

```markdown
**AC-1 — <observable behavior>**
Given <state>, when <action>, then <observable result>.
Failure: <the specific state or error the user sees>.
```

Avoid "works well", "is fast", "handles errors", "scales". Prefer a specific status, state, header,
or visible outcome.

## Scope discipline

- One problem per document. Split when two problems need two owners.
- Every requirement has a non-goal beside it. Unbounded scope ships nothing.
- Classify requirements **NEED** (table stakes), **WANT** (differentiator), **INSIST** (security, audit,
  trust — cannot ship without).
- Do not invent a data model to satisfy a UI sketch. State the model the product needs; let the tech
  lead choose the schema.
- When code and doc disagree: the code wins for "what is true"; the doc wins for "what was decided".

## Handoff to tech lead

The document is ready when it contains:

- Goal and done condition in one sentence each.
- Decisions with Q-numbers.
- Phased scope, each phase independently shippable.
- Acceptance criteria per phase.
- Impact analysis with explicit "none" where applicable.
- Open questions with an owner and what settles them.

Then use the **orchestrate** skill, or hand the document over directly.

## Escalate before writing

Ask when the idea conflicts with a frozen decision (profile §11) or when it needs a commercial,
pricing, or legal decision rather than a product one.

## Supporting files

- [`references/doc-templates.md`](references/doc-templates.md) — headers, buckets, requirement tables,
  decision log, acceptance criteria, phases.
- [`references/impact-checklist.md`](references/impact-checklist.md) — expanded impact prompts.
