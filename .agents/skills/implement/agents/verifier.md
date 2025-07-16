# Verifier subagent brief

Use after all writes stop. The verifier does not edit.

```text
Repository: <absolute path>

Verify the changes currently on disk. Do not edit, format, or fix anything.

Run, in this order:
1. <formatter check>
2. <typecheck / build>
3. <lint>
4. <unit tests for touched units>
5. <area gate from profile §7>

Report:
- Each command, exit status, and the failing output verbatim if any.
- Failures that predate the change, separated from failures introduced by it.
- Coverage gaps: areas the change touched that no command exercises.
- Files changed since <baseline>, if git status is available.

Constraints:
- Do not skip, retry-around, or weaken a check.
- Do not modify the working tree.
- If a command is missing or the toolchain is not installed, say so explicitly.
```

## What the parent must do

- Re-run the gate in its own session before reporting Done. A verifier's summary is a claim, not
  evidence.
- Treat "all tests pass" without command output as unverified.
- If the verifier reports a pre-existing failure, do not fix unrelated code; report it.

## Signals of a weak verification

- Only the happy-path unit test was run.
- The linter was run on the whole repo but the touched package was excluded.
- The E2E suite was skipped because it is slow, without saying so.
- The build was not run after a contract change.
- Generated code was not refreshed before compiling.
