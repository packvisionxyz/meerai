# Verification

## Find the real gate

In order:

1. Profile §7 verification matrix.
2. The repo's agent rules (`AGENTS.md`, `CONTRIBUTING.md`).
3. CI workflows — the command CI runs is the authoritative gate.
4. Manifest scripts (`package.json` scripts, `Makefile` targets, `justfile`, `Taskfile.yml`).
5. If none exists: compile the touched unit and run the nearest test file. Say that the repo has no
   documented gate.

## Order

1. Formatter on touched files.
2. Compiler / type checker.
3. Linter.
4. Unit tests for the touched unit.
5. Integration tests.
6. Repo-wide unit suite.
7. E2E or end-to-end flow — only when the change is user-visible and the suite is meant to run
   deliberately.

Do not run the slowest suite first. Do not run a full environment suite as a smoke check.

## Reporting

```markdown
- `command` — pass
- `command` — fail: <first meaningful lines of output>
- Not run: `command` — <why>
```

- Report the command exactly as run.
- A subagent's claim is not evidence; re-run the gate in your session.
- If a check fails for a reason that predates the change, separate it from failures you introduced.

## Failure handling

- Fix the cause, not the assertion.
- Never skip, weaken, delete, or quarantine a check to pass.
- Never widen the change to hide a failure.
- If the goal cannot be met, report the failing evidence and the smallest next step.

## Environment traps

- Database-backed suites may need a container or a disposable database. Never point them at data you
  want to keep.
- Browser-mode component tests need a browser and driver.
- Codegen must run before compilation when a contract changed.
- Some repos exclude a language or package from the main lint/test target; check the profile rather
  than assuming coverage.
- A green CI run on a stale branch is not evidence for the current tree.
