# Recon subagent brief

Use this as the prompt for a read-only exploration subagent. Replace placeholders. The brief must
stand alone: subagents do not see the parent conversation.

```text
Repository: <absolute path>

Question: <one narrow question>

Read first:
- .agents/repo/profile.md (sections <n>, <n>)
- <nearest package rules file>
- <specific files, if known>

If the profile is missing, discover the minimum from README, manifests, and CI.

Deliver:
1. Direct answer in 2–4 sentences.
2. Evidence: path:line for every factual claim.
3. Canonical pattern or rule that governs the area (file + section).
4. Blast radius: other callers, generated mirrors, tests, config/IaC references.
5. Open questions, explicitly labelled.

Constraints:
- Read-only. Do not edit, create, delete, or move files.
- Do not run builds, tests, formatters, or migrations unless the question requires observing output;
  if you do, report the exact command and result.
- Do not infer a rule from a neighbouring package.
- Label claims as Fact, Inference, or Open question.
```

## When to use

| Situation | Subagents |
| :--- | :--- |
| One narrow locate/rule/ownership question | 0 — use [`search.md`](search.md) |
| Two independent questions | 2, parallel |
| Broad layout + one deep flow | 2 (layout, flow) |
| Follow-up on earlier findings | 1, continue the same session |

## Mapping to harnesses

- Harness with a read-only explorer agent: use it.
- Harness with only a general agent: use it, and keep the read-only constraint in the brief.
- Harness with no subagents: run the same steps inline and keep the output contract.

## Failure signals

- The answer has no `path:line` evidence.
- It describes an idea or doc instead of the code.
- It reports a command as passing without output.
- It edits a file "to check".
