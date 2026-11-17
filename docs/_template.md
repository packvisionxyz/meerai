# Document Standard and Template

This file defines the structure and writing rules for every document in
`docs/`. It is a meta document: it describes how documentation is written,
not how the product behaves. New documents copy the template in §3. Existing
documents are rewritten to match it.

## 1. Writing rules

1. **Two states only.** Every capability is either **Implemented** (in the
   tree today) or **Planned** (not built). A capability that is partly built
   is split: the built part goes to Implemented, the rest goes to Planned
   with an explicit boundary. Never describe a plan in present tense.
2. **Verify before writing.** Every Implemented claim must be checked
   against the code before it is written. Cite the owning crate and module
   (for example `meerai-session/src/projection.rs`) at least once per
   capability. Do not cite a file you did not open. Do not report a test or
   check as passing unless you saw it pass.
3. **Description before detail.** Each capability starts with one or two
   sentences that say what it does and why it exists. Configuration,
   defaults, limits, and edge cases follow. A reader must be able to stop
   after the first sentence and still know what the feature is.
4. **No stale history in the body.** Delivery-round narratives, old test
   counts, and superseded designs do not belong in the body. Milestone
   status lives in one table (harness-spec.md). Superseded decisions are
   either removed or recorded as a decision with its resolution.
5. **One source of truth per fact.** A fact is documented in depth in exactly
   one document. Other documents link to it instead of repeating it. The
   owner map is in §4.
6. **Explicit limits.** Every security-relevant or best-effort mechanism
   states what it does not guarantee. Never describe hooks, definitions, or
   an ACP backend as a security boundary.
7. **Stable vocabulary.** Use the product terms as defined: `Toolset`,
   `EffectClass`, `SessionEvent`, `AgentEvent`, `AgentCommand`,
   `ApprovalMode` (`plan | ask | auto`), `ExecutionWorld`, turn, step, item,
   projection, spill, compaction, rewind, grant. Tool names use the runtime
   form (`code-read_file`, `subagent-run`, `acp-run`, `question-ask`).
8. **Planned items are actionable.** Each Planned item states what it is,
   why it is wanted, and what gates it (dependency, milestone, or decision).
   No dates and no promises unless a milestone table commits them.
9. **Links are relative.** Link sibling documents as `name.md`, repository
   files as `../path`. Every link must resolve.
10. **Status header on every document.** The header table (§3) records the
    verification date. Re-verify and update the date when the document is
    changed after a code change.

## 2. Status labels

| Label | Meaning |
| --- | --- |
| Implemented | In the tree and verified against the code. |
| Implemented (v1) | In the tree with documented limits; the limits are listed. |
| Planned | Not built. Design intent only. |
| Operational | Built, but inactive until an operator configures it (for example release signing secrets). |

Do not use "landed", "shipped", "target", "current", or "future" as status
labels in prose; they drift. The milestone table in harness-spec.md may keep
`landed` / `backlog` as compact table values.

## 3. Template

Copy everything below this line into a new document. Remove optional
sections that do not apply. Keep the section order.

---

# <Title>

<One sentence: what this document covers and what the reader gets from it.>

| | |
| --- | --- |
| **Status** | Verified against the tree on YYYY-MM-DD |
| **Audience** | <who reads this: users, contributors, operators, security reviewers> |
| **Scope** | <crates, surfaces, or behaviors covered> |
| **Related** | [sibling.md](sibling.md) · [AGENTS.md](../AGENTS.md) |

## 1. Overview

<2–5 sentences. What this area is, why it exists, and how it fits the rest
of the harness. Link the owning documents for depth.>

## 2. Implemented

<One short paragraph: what is in the tree today, at a glance.>

### 2.1 <Capability>

<Description: what it does, why. Then behavior, configuration, defaults,
and limits. Cite the owning module once.>

### 2.2 <Capability>

<...>

## 3. Planned

<One short paragraph: what is not built and the general direction.>

| Item | Description | Gate |
| --- | --- | --- |
| <name> | <what and why, 1–2 sentences> | <dependency, milestone, or decision> |

<Expand an item into its own subsection only when the design needs more
than the table row. Label every design detail as planned.>

## 4. Reference <optional>

<Tables that a reader looks up rather than reads end to end: protocol
variants, keybindings, configuration fields, event vocabularies, exit
codes. Reference material describes Implemented behavior unless a row is
explicitly marked Planned.>

## 5. Invariants <optional>

<Rules that must hold when the code changes. One bullet per invariant,
with the reason it exists. Working rules for agents live in AGENTS.md;
this section records design invariants, not contributor instructions.>

## 6. Open questions <optional>

| # | Question | What settles it |
| --- | --- | --- |
| 1 | <question> | <experiment, decision, or measurement> |

<Resolved questions are removed or moved into the relevant Implemented
subsection as a recorded decision.>

---

## 4. Document ownership map

Each fact has one owning document. Others link to it.

| Document | Owns |
| --- | --- |
| [harness-spec.md](harness-spec.md) | Product behavior: design principles, session log, item protocol, turn loop, context engine, permissions, safety nets, agents/skills/subagents, retrieval, milestone status table, recorded decisions. |
| [architecture.md](architecture.md) | Crate layout, dependency direction, build surfaces, protocol and configuration summary (links to spec/internals for depth), target layout. |
| [harness-internals.md](harness-internals.md) | Implementation reference: macro conventions, configuration mechanics, loop internals, traps and invariants, security posture summary. |
| [tui-design.md](tui-design.md) | TUI behavior: layout, modes, keybindings, overlays, rendering invariants, planned UX. |
| [acp.md](acp.md) | ACP: protocol reference, the implemented `agy` client, `acp-run`, trust boundary, planned presets and M14. |
| [threat-model.md](threat-model.md) | Security: assets, trust boundaries, untrusted inputs, implemented controls, residual risks. |
| [token-efficiency.md](token-efficiency.md) | Token-cost and self-healing initiative: implemented phases with evidence, decisions, planned phases. |

Cross-cutting facts:

- Milestone status (M1–M14): harness-spec.md only. Other documents say
  "Implemented" or "Planned" and link the milestone table.
- Security guarantees and residual risk: threat-model.md only. Other
  documents state the mechanism and link the risk.
- Keybindings and TUI invariants: tui-design.md only.
- Working rules for coding agents: ../AGENTS.md only.

## 5. Rewrite checklist

Apply to every document before calling it done:

- [ ] Header table present with a real verification date.
- [ ] Every capability is under Implemented or Planned, never mixed.
- [ ] Every Implemented claim was verified against the code in this pass.
- [ ] No claim contradicts another document or the README.
- [ ] No stale status ("not built" for a shipped feature, or the reverse).
- [ ] Best-effort mechanisms state their non-guarantees.
- [ ] All relative links resolve.
- [ ] Duplicated depth replaced with links to the owning document.
- [ ] Historical delivery narrative removed or compressed into the
      milestone table.
