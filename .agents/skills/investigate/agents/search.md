# Search subagent brief

Fast, narrow, read-only lookup for a single Locate, Rule, or Ownership question. The budget is what
makes it fast — keep it. For anything needing a trace, a pattern comparison, or a blast radius, use
[`recon.md`](recon.md) instead.

Replace placeholders. The brief must stand alone: subagents do not see the parent conversation.

```text
Repository: <absolute path>

Question: <one narrow question — where is X / which rule applies to X / who owns X>

Budget:
- At most 6 tool calls total: 2–3 targeted searches, then open only the file regions that answer it.
- No profile boot, no manifests, no CI workflows, no whole-file reads — the matching region only.

Deliver:
1. The answer in 1–3 sentences.
2. Evidence: path:line for every claim.

Constraints:
- Read-only. Do not edit, create, delete, or move files.
- Do not run builds, tests, formatters, or migrations.
- Prefer the definition over call sites; prefer the nearest rules file over generic docs.
- If the budget runs out without an answer, stop and report what you found and what is missing.
  Do not guess and do not widen the search silently — the parent will escalate to recon.
```

## When to use

| Situation | Subagent |
| :--- | :--- |
| "Where is X defined / used?" | 1 search agent |
| "Which rule applies to X?" | 1 search agent (nearest rules file) |
| "Who owns X?" | 1 search agent (`CODEOWNERS`, manifest, CI filters) |
| Several independent narrow questions | One search agent each, in parallel |
| Trace / pattern / blast radius | [`recon.md`](recon.md) instead |
| Follow-up on earlier findings | Continue the same subagent session |

## Mapping to harnesses

- Harness with a fast explorer agent: use it at its lowest thoroughness setting.
- Harness with only a general agent: use it; the budget constraint keeps it fast.
- Harness with no subagents: run the same steps inline and keep the output contract.

## Failure signals

- The answer has no `path:line` evidence.
- The budget was spent opening files that did not contain the answer.
- It guessed after the budget ran out instead of reporting what was missing.
- It read the whole profile or a whole file when a region would do.
- It edited a file "to check".
