---
name: implement
description: Implement or change code in any layer of a repository — transport, domain, persistence, background workers, client data, UI, or configuration. Use when writing a feature, fixing a bug, adding an endpoint, migration, command, route, query, or component.
license: MIT
compatibility: Works with any agent supporting the Agent Skills standard. Subagent delegation is optional; run inline when the harness has no subagents.
metadata:
  version: "1.1.0"
  tags: "implementation, backend, frontend, testing"
---

# Coding

Implement the smallest correct change that satisfies the request, in the layer that owns it.

## Boot: load the repository contract

Read `.agents/repo/profile.md` first:

- §3 Commands — what to run.
- §4 Invariants — what must not break.
- §5 Patterns — the canonical implementation to mirror.
- §6 Generated paths — what never to edit.
- §7 Verification — the gate for each area.

If the profile is missing, derive the same facts from the nearest agent rules, manifests, and CI
before editing. Do not invent a command.

## Non-negotiables

1. Read the owning rules before editing, not after.
2. Find the existing helper, pattern, or query first. Duplicate once before generalizing.
3. Never touch generated or vendored output; regenerate it.
4. Every changed line must be explainable by the task. No opportunistic refactors or reformatting.
5. Prove Done with a command, not a claim.

## Step 0 — frame the change

- What is the done condition? Which command or observable behavior proves it?
- Which module owns it? Is a contract changing (schema, API, event, IPC, config)?
- Is there an existing implementation to extend? Search before writing.
- What is the blast radius? Other callers, generated mirrors, tests, config, docs.
- If two interpretations are plausible, ask. A wrong assumption costs more than a question.

## Step 1 — choose the recipe

| Change | Read |
| :--- | :--- |
| Transport endpoint, request validation, auth wiring | [`references/change-recipes.md`](references/change-recipes.md) § Transport |
| Business rule, state machine, transaction boundary | § Domain |
| Schema, migration, query, generated data layer | § Persistence |
| Background worker, queue consumer, child process | § Background |
| Client cache, route loader, local state | § Client data |
| Component, form, styling, copy, accessibility | § UI |
| Configuration, feature flag, environment variable | § Configuration |

The profile's §5 tells you which existing implementation is canonical for each layer. Mirror it.

## Step 2 — contract first

When a change crosses a boundary, land the contract before the implementations:

1. Schema / proto / message / event / IPC shape.
2. Regenerate the derived code.
3. Server or producer.
4. Clients and consumers.
5. Tests at each level.

Never renumber or retype an existing contract field; mark removals deprecated/reserved. Never edit a
committed migration. See the profile §4.

## Step 3 — delegate when it helps

Small change: implement inline. Multi-area change: decompose by **disjoint file sets**, then delegate.

Default behavior: if the change touches more than one layer, or more than roughly three files, split
it into workstreams and use implementer subagents.

| Situation | Pattern |
| :--- | :--- |
| One file or one layer | Inline |
| Two disjoint areas | Two implementer subagents in parallel |
| Contract + server + client | Contract first, then sequential implementers |
| Several writes finished | One verifier subagent after all writes stop |

Briefs:

- [`agents/implementer.md`](agents/implementer.md)
- [`agents/verifier.md`](agents/verifier.md)

Rules:

- Never run two writers on the same file, group, generated output, or shared contract file.
- The contract is frozen before parallel work starts.
- A subagent's "done" is a claim; run the gate yourself before reporting.
- Do not delegate commits, pushes, deploys, or destructive git operations.

## Step 4 — verify

Run the narrowest check first, then the area gate from the profile §7. If the profile has no matrix,
find the authoritative commands in the manifests and CI.

Report real output: command, exit status, and failing text when it fails. Never skip, weaken, or delete
a check to pass.

See [`references/verification.md`](references/verification.md).

## Step 5 — done checklist

- [ ] Owning rules followed; no rule widened to make the change fit.
- [ ] Existing pattern reused; no speculative abstraction.
- [ ] No generated file edited; codegen re-run if a contract changed.
- [ ] Tests added at the level the layer requires.
- [ ] Every user-facing string localized in every supported locale.
- [ ] Accessibility and stable test hooks added for new UI.
- [ ] Every new effect, loop, socket, process, or subscription has cleanup.
- [ ] Errors mapped and wrapped; no raw internal cause on the wire.
- [ ] The area gate passes; output reported.
- [ ] No unrelated edits left in the diff.

## Stop and ask when

- The change would alter a decision the profile §11 marks as frozen.
- The change touches production infrastructure, secrets, or a deploy pipeline (hand to **deliver**).
- Two packages need different shapes of the same contract and neither is canonical.
- A failure exists before your change; report it rather than masking it.

## Supporting files

- [`references/change-recipes.md`](references/change-recipes.md) — what to check in each layer.
- [`references/verification.md`](references/verification.md) — finding and reporting the real gate.
- [`agents/implementer.md`](agents/implementer.md) — implementation subagent brief.
- [`agents/verifier.md`](agents/verifier.md) — verification subagent brief.
