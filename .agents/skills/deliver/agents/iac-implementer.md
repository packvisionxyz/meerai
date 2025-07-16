# IaC / pipeline implementer brief

`<skills-dir>` is the installed location of this bundle (for example `.agents/skills` or
`~/.agents/skills`). Replace placeholders.

```text
Repository: <absolute path>
Change: <one-line intent>
Environment(s): <dev|staging|prod|shared>

Read first:
- .agents/repo/profile.md §9
- <skills-dir>/deliver/SKILL.md and the matching file in <skills-dir>/deliver/references/
- <existing workflow / module / environment file>

Scope — you own:
- <paths>

Out of scope — do not touch:
- Other environments' state
- Shared resources owned by another state
- Application code
- <other workstream paths>

Requirements:
- <behavior / topology change>
- Every caller of a changed reusable unit is updated.
- New inputs documented and wired in every environment that runs the job.
- No secret value written anywhere.

Checks (run and report output):
- <fmt / validate / lint / plan command>

Output:
1. Files changed and why.
2. Checks run and their real result.
3. Plan summary: create/update/replace/destroy counts and anything surprising.
4. Rollback path.

Constraints:
- Do not apply, deploy, rotate secrets, delete state, or push.
- Do not run a plan that requires credentials you do not have; report it instead.
- If a change requires a destructive action, stop and ask.
```

## Rules

- Plan output is evidence; "validate passed" is not a topology review.
- If the change touches a reusable unit, the caller list is part of the scope.
- Never widen scope to make a validation pass.
- For a secret change, the reviewer must run before any apply.
