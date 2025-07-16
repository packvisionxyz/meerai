---
name: deliver
description: Change delivery and infrastructure — CI/CD pipelines, reusable workflow components, infrastructure-as-code, environments, secrets wiring, domains, and release configuration. Use for pipeline edits, IaC, deployment topology, secret plumbing, and rollout changes.
license: MIT
compatibility: Works with any agent supporting the Agent Skills standard. Subagent delegation is optional; run inline when the harness has no subagents.
metadata:
  version: "1.1.0"
  tags: "cicd, terraform, infrastructure, deployment"
---

# Infra

Infrastructure mistakes are expensive and often invisible until deploy. Read the topology before
editing, plan before applying, and never handle a secret value.

## Boot: load the repository contract

Read `.agents/repo/profile.md` §9, then the deeper repo infra notes it links to.

If the profile is missing, discover:

1. CI/CD: `.github/workflows/`, `.gitlab-ci.yml`, `Jenkinsfile`, `bitbucket-pipelines.yml`, `azure-pipelines.yml`.
2. Reusable units: composite actions, shared workflow files, pipeline templates, scripts they call.
3. IaC: `terraform/`, `pulumi/`, `cdk/`, `k8s/`, `helm/`, `ansible/`.
4. Environment/secret definitions: environment names, variable groups, vault references.
5. Docs: deployment or runbook pages linked from `README.md`.

## Principles

1. **Secrets are never written down.** They flow from the platform's secret store to the job to the
   deploy step. Never commit, echo, log, or hardcode a secret.
2. **One source per step.** Shared deploy/build logic belongs in a reusable unit. Workflows choose the
   environment, ref, and inputs.
3. **Change the reusable unit and every caller together.** Search for all references before editing.
4. **Environment ownership is exclusive.** Shared resources belong to one state/environment; per-app
   resources belong to their own. Never duplicate a shared resource into an app environment.
5. **Plan before apply, always.** Review creates, updates, replacements, and destroys. Never apply
   blindly to a shared environment.
6. **Least privilege.** Permissions, tokens, roles, and scopes are minimal and explicit.
7. **Rollback is part of the change.** Say how to revert a pipeline, a schema, or a deploy.

## Change types

| Change | Read | Delegate to |
| :--- | :--- | :--- |
| Pipeline, job, reusable action, trigger, permissions | [`references/pipeline-review.md`](references/pipeline-review.md) | [`agents/iac-implementer.md`](agents/iac-implementer.md) |
| Terraform/Pulumi/CDK, environment, state, module | [`references/iac-review.md`](references/iac-review.md) | [`agents/iac-implementer.md`](agents/iac-implementer.md) |
| Domain, hostname, DNS, access gate | `references/iac-review.md` § Domains | implementer + reviewer |
| Secret rotation or new credential | both references | reviewer before apply |
| Release/build artifact | `references/pipeline-review.md` § Release | implementer |

## Delegation (optional)

Default behavior: if the change spans both pipeline and IaC, or more than one environment, spawn one
recon subagent for the current topology, then one implementer per disjoint file set. A plan or a
destructive change also gets a read-only reviewer.

| Situation | Pattern |
| :--- | :--- |
| One pipeline file | Inline |
| Pipeline + IaC, or several environments | One recon subagent for topology, then one implementer per disjoint file set |
| Plan exists and needs review | One reviewer subagent, read-only |
| Secret or destructive change | Reviewer before anything applies; ask the user |

Briefs:

- [`agents/iac-implementer.md`](agents/iac-implementer.md)
- [`agents/infra-reviewer.md`](agents/infra-reviewer.md)

Never let a subagent run `apply`, deploy to a shared environment, rotate a key, or delete state.
Those require explicit user approval and the sanctioned workflow.

## Checks

Use the repo's toolchain; do not substitute a weaker check.

```bash
# CI syntax/lint, when installed
actionlint

# Terraform
terraform fmt -check -recursive <iac-dir>
cd <environment-dir> && terraform init -backend=false && terraform validate
terraform plan            # with the sanctioned credentials, or via the pipeline

# Other stacks
<kustomize/helm/docker/pulumi/cdk> <validate or diff command>
```

A credential-backed plan/apply goes through the sanctioned pipeline, not a local shell.

## Done checklist

- [ ] Reusable unit updated, and every caller updated with it.
- [ ] New inputs/secrets exist in every environment that runs the job.
- [ ] Permissions, triggers, concurrency, and branch/environment guards are correct.
- [ ] IaC state, ownership, and module boundaries respected.
- [ ] Plan reviewed; replacements and destroys are intentional.
- [ ] Format/validate/lint checks pass; output reported.
- [ ] No secret, state, plan, or credential file staged.
- [ ] Rollback path stated.
- [ ] Docs, domain tables, and app config updated where they mirror the change.

## Escalate before acting

Stop and ask when a change would:

- Apply to a production or shared environment outside the sanctioned pipeline.
- Rotate, add, or remove a signing key or root credential.
- Replace a database, delete a bucket, or change retention.
- Remove an access gate, WAF rule, or rate limit.
- Change a workflow trigger, branch guard, or environment protection.
- Delete or import state.

## Supporting files

- [`references/pipeline-review.md`](references/pipeline-review.md)
- [`references/iac-review.md`](references/iac-review.md)
- [`agents/iac-implementer.md`](agents/iac-implementer.md)
- [`agents/infra-reviewer.md`](agents/infra-reviewer.md)
