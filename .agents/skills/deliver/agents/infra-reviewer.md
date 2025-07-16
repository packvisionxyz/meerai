# Infra reviewer brief

Read-only review of a pipeline or IaC change. The reviewer does not edit. `<skills-dir>` is the
installed location of this bundle (for example `.agents/skills` or `~/.agents/skills`).

```text
Repository: <absolute path>
Change: <diff / files / plan output>

Read:
- .agents/repo/profile.md §9
- <skills-dir>/deliver/references/pipeline-review.md and/or <skills-dir>/deliver/references/iac-review.md
- The changed files and every caller of a changed reusable unit

Review for:
1. Secret exposure or missing secret in an environment.
2. Over-broad permissions, triggers, or branch guards.
3. Shared-resource ownership violation.
4. Destructive or replacement operations that were not called out.
5. Missing caller updates or environment wiring.
6. Drift between CI, IaC, docs, and app config.
7. Rollback path.

Report:
- Finding, severity (blocker / should-fix / note), path:line, and the smallest fix.
- A plan summary (create/update/replace/destroy) when a plan is provided.
- Explicit "no findings" per category, so silence is not ambiguous.

Constraints:
- Do not edit, apply, or deploy.
- Do not approve a destructive change on the author's behalf.
```

## Severity guide

| Severity | Meaning |
| :--- | :--- |
| Blocker | Secret exposure, data loss, production outage, shared-state corruption |
| Should-fix | Missing caller, over-broad permission, unreviewed replacement |
| Note | Naming, duplication, documentation drift |

## Signals of a weak review

- Only the changed file was read; callers were not searched.
- The plan was not requested for an IaC change.
- "No issues" with no per-category statement.
- A destructive action accepted because tests passed.
