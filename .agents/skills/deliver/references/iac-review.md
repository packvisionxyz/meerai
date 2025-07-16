# IaC review

## Inventory before editing

- Which environments exist, and which one owns shared resources?
- Where is state stored, and how is it locked?
- Which modules/packages are reusable, and who calls them?
- How do variables and secrets reach the plan?
- What is the current plan for this environment (if available)?

## Checklist

### State and ownership

- [ ] State backend and key are correct for the environment.
- [ ] Locking is enabled; concurrent applies are prevented.
- [ ] State, plan, and `.terraform/`-equivalent caches are not staged.
- [ ] Shared resources are owned by exactly one environment/state.
- [ ] No cross-environment references that create hidden coupling.

### Variables and secrets

- [ ] New variable declared in the module/environment and passed by every caller.
- [ ] Example file / docs updated.
- [ ] Secrets come from the platform secret store, never defaults or committed values.
- [ ] Required values fail closed when missing.
- [ ] Environment names and key names match across CI and IaC.

### Versions and modules

- [ ] Tool version pinned by the repo's version file.
- [ ] Provider/module versions stay within existing constraints.
- [ ] Lockfile changes are intentional and committed.
- [ ] Module changes are propagated to all callers.

### Plan review

- [ ] `create` / `update` are expected.
- [ ] `replace` is understood; the downtime and data impact are acceptable.
- [ ] `destroy` is intentional and has a backup or migration path.
- [ ] No unexpected drift (resources recreated because of renamed identifiers).
- [ ] Resource names/tags keep stable identities where replacement would be destructive.

### Domains and access

- [ ] DNS record, TLS, and route config updated for every host affected.
- [ ] Access/identity gate configured and reachable (an identity source exists).
- [ ] Public/private intent is explicit.
- [ ] CORS/allowlists updated when a browser origin changes.
- [ ] Documentation that mirrors the topology is updated.

### Safety

- [ ] Production apply goes through the sanctioned pipeline with environment protection.
- [ ] Secrets and state are not printed.
- [ ] The change is reversible, or the irreversibility is stated.

## Local checks

```bash
<tool> fmt -check -recursive <iac-dir>
cd <environment-dir> && <tool> init -backend=false && <tool> validate
<tool> plan        # only with sanctioned credentials
```

If a plan requires credentials you do not have, say so and route the plan through the pipeline rather
than guessing.

## Red flags

- A shared resource added to an app environment.
- A gated host with no identity source populated.
- A renamed resource that will be destroyed and recreated.
- A new secret defaulted to a real value.
- An apply run locally against production.
- A plan with destroys nobody reviewed.
