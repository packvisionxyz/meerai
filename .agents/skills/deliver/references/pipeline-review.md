# Pipeline review

## Inventory before editing

- Which workflows run on push, PR, schedule, or dispatch?
- Which jobs gate merges (required checks)?
- Which workflows deploy, and to which environment?
- Which logic is duplicated across workflows and should be a reusable unit?
- Which secrets and permissions does each job receive?
- Which artifacts cross jobs, and how are they named/retained?

## Checklist

### Structure

- [ ] Shared logic lives in one reusable unit, not copy-pasted across workflows.
- [ ] Every caller of a changed reusable unit is updated in the same change.
- [ ] Job names, step names, and inputs describe intent.
- [ ] The workflow's `run-name` / summary shows what was deployed and from which ref.

### Triggers and guards

- [ ] Trigger types and branches are intentional.
- [ ] Deploy jobs guard the ref (branch or tag), not only the event.
- [ ] Manual dispatch inputs have types, defaults, and descriptions.
- [ ] Concurrency prevents overlapping applies/deploys; `cancel-in-progress` is deliberate.
- [ ] Fork/PR workflows do not receive secrets.

### Permissions and secrets

- [ ] `permissions:` is least-privilege; no `write-all`.
- [ ] Each secret exists in the target environment and is passed explicitly.
- [ ] Secrets are not echoed, written to artifacts, or interpolated into shell strings that could log.
- [ ] OIDC/short-lived credentials are preferred over static keys when supported.
- [ ] Environment protection rules still apply to production.

### Actions and dependencies

- [ ] Third-party actions are pinned (SHA preferred where the repo already pins).
- [ ] Action versions are consistent with neighbouring workflows.
- [ ] Caches are keyed by lockfile/version and do not leak secrets.
- [ ] Artifacts have explicit names, paths, and retention.

### Release and rollback

- [ ] Version/tag resolution is deterministic.
- [ ] The deployed ref is the one that was built and tested.
- [ ] Artifacts are produced once and promoted, not rebuilt per environment.
- [ ] Rollback is documented: previous tag, previous image, or revert path.
- [ ] A failed deploy leaves the previous version serving.

## Common mistakes

- Editing one caller and not the other.
- Adding a secret to the workflow but not to the environment.
- Removing a branch guard to make a manual test pass.
- Using `pull_request_target` with checkout of untrusted code.
- Pinning an action to a floating tag while neighbours use a SHA.
- Adding `permissions: write-all`.
- Running deploy logic inline in a workflow instead of the reusable unit.
- Caching a directory that contains credentials.

## Local checks

```bash
actionlint                                   # workflow/action syntax
<repo-specific pipeline linter, if any>
```

If the tool is missing, say so. Do not claim the pipeline is valid because it "looks right".
