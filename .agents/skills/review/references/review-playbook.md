# Review playbook

How to get the right diff, read it efficiently, and turn it into findings with evidence.

## 1. Get the diff

Use the repository's own forge CLI when it exists. Otherwise use git directly. Print the revisions you
used in the report.

### Pull request

```bash
gh pr view <number> --json title,body,baseRefName,headRefName,commits
gh pr diff <number> --patch
gh pr checkout <number>          # only when you need the files on disk
git merge-base origin/<base> HEAD
git diff --stat <merge-base>...HEAD
```

Without a forge CLI, fetch the base branch and diff the merge base. Never review only the PR
description.

### Local change

```bash
git status --short
git diff --stat HEAD
git diff --cached                    # staged
git diff                             # unstaged
git diff HEAD                        # both
git ls-files --others --exclude-standard   # untracked files that belong to the change
```

Read untracked files directly. `git diff` does not show them.

### Commit range

```bash
git log --oneline <old>..<new>
git diff --stat <old>..<new>
git diff <old>..<new> -- <path>
```

`A..B` compares the tips; `A...B` compares from the merge base. Choose deliberately and say which.

## 2. Read the diff efficiently

1. **Stat first.** Files, line counts, and churn tell you where the risk is.
2. **Skip noise.** Lockfiles, generated snapshots, vendored code, and formatting-only hunks are read
   only to confirm they are noise.
3. **Read the unit, not just the hunk.** Open the whole function, type, or module around each change.
4. **Follow shared units outward.** For each changed type, enum, event, DTO, schema, or config key,
   search every reader and writer before judging.
5. **Check the pair.** A producer without its consumer, a flag without its default, a permission
   without its test, or a doc claim without the code is a finding even when each half compiles.

## 3. Category checks

### Contracts

- Wire enums and event shapes have adjacent serde tagging when they carry strings.
- Every consumer moves in the same change: other clients, mappers, replay, protocol DTOs, NDJSON.
- Field order and names stay stable unless the change is intentional.
- A removed or renamed public item has no remaining caller.

### Behaviour

- Empty input, one element, the boundary value, the maximum, and a cancelled request.
- Retry and timeout paths: is the retry bounded, and is the failure surfaced with a cause?
- Read-only mode cannot be widened by configuration or by a project file.
- A new default cannot silently change existing behavior.

### Tests

- A test that fails without the change exists for each new behavior.
- The assertion checks the behavior, not the implementation detail.
- A mock at the boundary proves the contract, not the integration. Say which one the test proves.
- Deleted or weakened tests are a finding unless the change explains why.

### Errors and resources

- Errors carry context; internal causes do not reach the wire.
- Every spawned task, thread, process, socket, subscription, and temporary file has an owner and a
  release path, including on early return.
- A panic or `unwrap` on input the caller controls is a blocker.

### Security

- Provider secrets and tokens stay out of output, logs, errors, and child environments.
- Permission changes tighten only; a project-scoped config cannot grant capability.
- Path guards and the outside-workspace approval still apply to every new file or process operation.
- New shell commands keep the safety allowlist conservative.

### Data

- A migration is reversible or explicitly one-way with a backup story.
- No committed migration is edited in place.
- A renamed or retyped contract field keeps its meaning or has a backfill.

### Concurrency and performance

- No `await` while holding a lock that another task needs.
- No blocking I/O on an async runtime thread.
- No unbounded growth in a collection that lives for the process lifetime.
- A hot loop does not allocate per iteration without a reason.

### Docs and config

- The change updates the owning document for each fact it changes: README, `docs/`, the repository
  profile, and the rules file when an invariant moved.
- Defaults in docs, examples, and config match the code.
- A renamed command, flag, path, or env var has no stale mention anywhere.

## 4. Evidence rules

- Every finding cites `path:line`. A finding without a location is a guess.
- A claim about behaviour is proven by the code path you read, or by a command you ran and quoted.
- Separate what the change introduced from what already existed. Do not blame the author for the
  second.
- Do not report a check as passing unless you saw it pass.
- No finding is better than a fabricated one. Say "not verified" when you did not verify.

## 5. Report shape

```
Verdict: <ready | ready with fixes | blocked>
Blockers:  <n>   Should fix: <n>   Notes: <n>
```

Then the lists from `SKILL.md` §6. Keep each entry to one or two lines: location, failure, smallest
fix. Put the categories you checked and found clean in one line, so the reader knows the coverage.

## 6. Stop conditions

- The diff is too large to read in one pass: split by area and use the **orchestrate** skill.
- The change depends on an unmerged change: review the dependency first, or mark the review blocked.
- The toolchain cannot run a needed check: report the gap, do not weaken the check.