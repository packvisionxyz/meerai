---
name: investigate
description: Investigate a codebase and answer questions about implementation, design patterns, and project rules. Use when locating code, tracing a flow end-to-end, finding the canonical pattern for a layer, or deciding which project rule applies before a change.
license: MIT
compatibility: Works with any agent supporting the Agent Skills standard. Subagent delegation is optional; run inline when the harness has no subagents.
metadata:
  version: "1.2.0"
  tags: "exploration, architecture, patterns, rules"
---

# Explore

Read-only reconnaissance. The output is a grounded map of the code, not an edit.

## Fast path

Classify the question before reading anything. Locate, Rule, and Ownership questions are settled by
searches, not by reading contracts end to end:

| Kind | Path | Budget |
| :--- | :--- | :--- |
| Locate — "where is X" | Search the symbol; open only the hits that answer it | 2–3 searches, then answer |
| Rule — "am I allowed to X" | Read the nearest rules file; search others only if it is silent | 1–2 file reads, then answer |
| Ownership — "who owns X" | `CODEOWNERS`, manifest, CI path filter | 1–2 lookups, then answer |
| Trace / Pattern | Full workflow below | unbounded |

Skip the profile boot on the fast path. Escalate to the full workflow when the budget runs out, when
the answer must cite invariants or canonical patterns, or when the hits contradict each other — the
profile is mandatory from there on.

## Boot: load the repository contract

The repository declares its facts in `.agents/repo/profile.md` at the project root. Read it first when
running the full workflow, then the deeper files it links to. The fast path skips this step and returns
here on escalation.

If the profile is missing, discover only what the question needs:

1. Agent rules: `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `.cursor/rules/`,
   `.github/copilot-instructions.md`.
2. Orientation: `README.md`, `docs/` (index first), architecture notes.
3. Build and test: `package.json`, `Makefile`, `justfile`, `Taskfile.yml`, `go.mod`, `Cargo.toml`,
   `pyproject.toml`.
4. Execution: `.github/workflows/`, `Dockerfile`, IaC directories.

Record which parts are missing. Never infer a rule from a neighbouring package; the rule nearest the
code wins.

## Workflow

1. **Classify the question.**

   | Kind | Example | Evidence shape |
   | :--- | :--- | :--- |
   | Locate | "where is X" | path + symbol |
   | Trace | "how does X reach Y" | ordered hops with boundaries |
   | Pattern | "how should X be written" | canonical sibling implementation |
   | Rule | "am I allowed to X" | rule file + section |
   | Ownership | "who owns X" | manifest, CODEOWNERS, CI path filter |

2. **Find the entry point.** Search the symbol, route, command, event, or path the user named. Start at
   the boundary, not at a file you guess.
3. **Trace end-to-end.** Follow the real wire: caller → transport → validation → domain → persistence
   or external call → response → client state. Note every boundary the value crosses.
4. **Compare with the canonical pattern.** If the code differs from the documented pattern, say whether
   it is drift or a deliberate exception, and cite both.
5. **Label every claim** before answering.

## Claim discipline

| Label | Meaning | Evidence required |
| :--- | :--- | :--- |
| Fact | True in the tree now | `path/to/file.ts:42` |
| Inference | Follows from facts but was not directly observed | Name the facts it rests on |
| Open question | Needs running, asking, or product input | Say what would settle it |

- Never present an inference as a fact. Never cite a path you did not open.
- Documentation can lag the code. When they disagree, cite both and state which one is current.
- Do not report a check as passing unless you saw it pass.

## Traps in an unfamiliar repo

- **Generated output is not a source.** Read it to understand a contract, never as the place a change
  belongs. Check the profile's generated paths.
- **Docs can be aspirational.** Look for status labels (`proposed`, `in progress`, `shipped`) and verify
  against the tree.
- **Compiled-but-unmounted code exists.** Feature flags, gated routes, and non-workspace crates may be
  intentional. Do not call them dead without checking the owner.
- **Duplicated layers exist.** In a monorepo the same concept may be implemented twice during a
  migration. Determine which one is live before describing behavior.
- **Config drifts.** A value may be set in code, in a workflow, and in IaC. Report all occurrences.
- **Tests are evidence of intent, not behavior.** A test that mocks the boundary proves the contract,
  not the integration.

## Delegation (optional)

Use subagents when the question decomposes. Do not spawn one for something two searches answer.

Default behavior: if the request contains more than one independent question, or the repository has no
profile and is unfamiliar, spawn one read-only subagent per question before answering. Otherwise
run inline.

| Situation | Pattern | Brief |
| :--- | :--- | :--- |
| One locate/rule/ownership question | Answer inline, fast-path budget | — |
| Several independent locate/rule questions | One search subagent each, in parallel | [`agents/search.md`](agents/search.md) |
| Two or more independent trace/pattern questions | One read-only recon subagent per question, in parallel | [`agents/recon.md`](agents/recon.md) |
| Broad unfamiliar repo | One subagent for layout, one for the specific flow | [`agents/recon.md`](agents/recon.md) |
| Follow-up depends on earlier findings | Continue the same subagent session | — |

- Map the search agent to the harness's fastest explorer and its lowest thoroughness setting. The
  budget is what makes it fast; do not remove it to "be safe".
- Map "read-only recon" to the harness's exploration agent. If there is none, use a general agent and
  state **do not edit files** in the brief.
- Subagents do not see your conversation; the brief must stand alone.
- Require `path:line` evidence and labelled claims.
- Keep recon read-only. Editing is the implement skill's job.

## Output format

```markdown
## Answer
<2–4 sentences>

## Evidence
- `path:line` — what it shows

## Rules / patterns that apply
- <rule or pattern> (`path`)

## Confidence
- Fact: …
- Inference: …
- Open: …
```

Answer first. The user wants the conclusion, not a travel log.

## Handoff

- If the question becomes a change, hand off to the **implement** skill.
- If the change spans several surfaces, hand off to the **orchestrate** skill.
- If the question is about CI or IaC, hand off to the **deliver** skill.

## Supporting files

- [`references/recon-playbook.md`](references/recon-playbook.md) — entry points by question type,
  tracing method, and what to read first in an unknown stack.
- [`agents/search.md`](agents/search.md) — fast read-only search subagent brief, budgeted.
- [`agents/recon.md`](agents/recon.md) — read-only recon subagent brief.
