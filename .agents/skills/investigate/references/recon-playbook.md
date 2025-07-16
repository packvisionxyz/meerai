# Recon playbook

## First five minutes in an unknown repo

1. Read `.agents/repo/profile.md`. If absent, read `README.md` and the nearest agent rules file.
2. List the top-level tree; identify the deployable units, not every directory.
3. Read the manifests to learn the toolchain and scripts (`package.json` scripts, `Makefile` targets,
   `go.mod`, `Cargo.toml`, `pyproject.toml`).
4. Read the CI workflows to learn what "green" means and which commands are authoritative.
5. Open one end-to-end example of the layer in question before opening ten partial ones.

## Entry points by question

| Question | Start at |
| :--- | :--- |
| Where is symbol X defined? | `rg -n "X"`; prefer the definition over call sites |
| How does an HTTP/RPC request flow? | Router/handler registration, then the handler body |
| How does a background job run? | Worker entry, queue consumer, scheduler, or command registration |
| How does the UI get data? | Route loader / data hook / query module, then the client |
| How is auth enforced? | Middleware/interceptor/guard list at the transport boundary |
| Where is the rule? | Nearest `AGENTS.md` / `CONTRIBUTING.md`; otherwise the docs the repo points to |
| How is it tested? | Test file beside the source, or the test directory named in the manifest |
| How is it deployed? | CI workflow → composite action / script → IaC |
| Who owns it? | `CODEOWNERS`, package manifest, CI path filters |

## Tracing a flow

Write the hops explicitly; each hop is a boundary where a value can change shape.

```
caller
  → transport (route, RPC, CLI args, event payload)
  → validation / auth
  → domain rule
  → persistence or external call
  → response / event / file
  → client state or next consumer
```

At each boundary, note:

- What validates the input?
- What is the identity/tenant source?
- What error type crosses the boundary?
- Is the call retried, queued, or fire-and-forget?
- What is persisted, and in which transaction?

## Reading rules correctly

- Rules are scoped. A root rule applies everywhere; a package rule adds or narrows.
- Precedence: nearest owning file > root file > generic docs.
- A generated or vendored file carries no rules of its own.
- A status label in a doc is the author's claim. Verify the behavior in code before repeating it.
- If a rule and the code disagree, report the conflict; do not silently pick one.

## Blast radius

Before saying "this is a small change", find:

- Other callers of the symbol (`rg` again, excluding the definition).
- Generated clients or types that mirror the contract.
- Tests that assert the current behavior.
- Config, CI, and IaC references to the same name.
- Docs that would become false.

## Reporting

- Lead with the answer.
- Cite `path:line` for facts.
- Separate "what exists" from "what should exist".
- Name the rule that governs the area.
- End with open questions and the smallest next step.
