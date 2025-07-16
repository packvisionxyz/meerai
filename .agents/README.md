# Agent Skills

A portable bundle of five role skills for coding agents. The skills are repository-agnostic: each one
reads a per-repository profile instead of hardcoding paths, commands, or rules.

```
.agents/
├── README.md
├── CHANGELOG.md
├── skills/
│   ├── investigate/  investigate code, patterns, and rules
│   ├── implement/    implement in any layer
│   ├── deliver/      CI/CD and infrastructure-as-code
│   ├── specify/      brainstorm and write requirements
│   └── orchestrate/  orchestrate a multi-area change with subagents
├── templates/
│   └── repo-profile.md
└── repo/
    ├── profile.md   ← this repository's facts
    ├── patterns.md
    └── infra.md
```

`repo/` is repository knowledge, not part of the published skill bundle.

## How it works

Every skill boots by reading `.agents/repo/profile.md` at the project root. The profile declares:

- layout and entry points
- commands (bootstrap, test, lint, format, codegen, deploy)
- invariants
- canonical patterns
- generated and vendored paths
- verification matrix
- documentation conventions
- infrastructure and deploy topology
- domain, personas, and entitlement gating
- frozen decisions
- recon pointers

Skills cite sections by number. If the profile is missing, they fall back to discovering the same facts
from `AGENTS.md`, `README.md`, manifests, and CI.

## Install

### OpenCode

Project: copy `skills/` to `<repo>/.agents/skills/`.
Global: copy `skills/` to `~/.agents/skills/`.

OpenCode discovers `.agents/skills` and `~/.agents/skills`.

### Claude Code

Claude Code reads `.claude/skills/`. Either copy the bundle there, or symlink it:

```bash
ln -s ../.agents/skills .claude/skills
```

Global: `~/.claude/skills/`.

### Other agents

Any tool that implements the Agent Skills standard reads a skills directory containing
`<skill>/SKILL.md`. Copy or symlink `skills/` into the directory that tool documents. The skill bodies
avoid tool-specific frontmatter and tool-specific tool names.

### Set up a repository

```bash
mkdir -p .agents/repo
cp <bundle>/templates/repo-profile.md .agents/repo/profile.md
# fill it in
```

Minimum viable profile: sections 2 (layout), 3 (commands), 4 (invariants), 7 (verification). Mark the
rest `Unknown` rather than guessing.

## The skills

| ID | Use when |
| :--- | :--- |
| `investigate` | Locating code, tracing a flow, finding the canonical pattern, deciding which rule applies |
| `implement` | Writing a feature, fix, endpoint, migration, command, route, query, or component |
| `deliver` | Changing pipelines, reusable workflow components, IaC, environments, secrets, domains, releases |
| `specify` | Brainstorming an idea or writing a requirement/initiative document |
| `orchestrate` | A change spans several surfaces and needs a plan, delegation, and integration |

## Subagents

Skills delegate when the harness supports it and stay runnable inline when it does not.

Role briefs live beside the skill that owns them:

| Brief | Used by |
| :--- | :--- |
| `skills/investigate/agents/search.md` | fast read-only lookup, budgeted |
| `skills/investigate/agents/recon.md` | read-only reconnaissance |
| `skills/implement/agents/implementer.md` | one implementation workstream |
| `skills/implement/agents/verifier.md` | post-write verification |
| `skills/deliver/agents/iac-implementer.md` | one IaC/pipeline workstream |
| `skills/deliver/agents/infra-reviewer.md` | read-only infra review |

Map roles to the harness's agent types; if it has no subagents, run the same steps inline. Never
delegate commits, pushes, deploys, secret rotation, or destructive git operations.

| Harness | Recon | Implement |
| :--- | :--- | :--- |
| OpenCode | `investigate` subagent | `general` subagent |
| Claude Code | Explore agent | general-purpose agent |
| No subagents | inline | inline |

## Authoring a skill

- Directory name is the skill ID; use lowercase kebab-case.
- `SKILL.md` frontmatter: `name`, `description`, optional `compatibility`, `metadata`. Avoid
  tool-specific fields so the bundle stays portable.
- The description must say **what** and **when**; it is the only text advertised before loading.
- Keep `SKILL.md` focused. Put detail in `references/`, role briefs in `agents/`, scripts in `scripts/`.
- Paths inside a skill are relative to the directory containing `SKILL.md`.
- In subagent briefs, the bundle location is written as `<skills-dir>` and the repository contract is
  always `.agents/repo/profile.md` at the project root — never hardcode where the bundle is installed.
- Do not add repository-specific paths, commands, or rules to a skill. Add them to the repo profile.

## Publishing checklist

- [x] License: MIT — `LICENSE` at the bundle root, `license: MIT` in every `SKILL.md`.
- [ ] Remove or replace `.agents/repo/` with the template; it contains private repository knowledge.
- [ ] Verify no secrets, internal hostnames, customer names, or private URLs remain.
- [ ] Bump `metadata.version` in changed skills.
- [ ] Add a changelog entry.
- [ ] Validate frontmatter parses and each skill loads by ID.
- [ ] Confirm the bundle works against a fresh repository with only the template profile.
- [ ] Add install instructions for each supported tool.

## Compatibility notes

- Frontmatter uses only fields from the Agent Skills standard: `name`, `description`, `compatibility`,
  `metadata`.
- Delegation is described in prose, not tied to a named tool, so it degrades to inline execution.
- The repository profile is a plain Markdown contract; no tool reads it automatically.
