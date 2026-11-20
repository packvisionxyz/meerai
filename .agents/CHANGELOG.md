# Changelog

All notable changes to the skill bundle. Versions follow the `metadata.version` in each `SKILL.md`.

## 1.3.0

- Moved the role briefs out of `skills/*/agents/` into `.meerai/agents/` as native harness agent
  definitions. Each definition declares `mode: subagent` and its `permissions`, so the harness can
  select it by id and enforce the tightening rules.
- `investigate` and `orchestrate` reference the `search`, `recon`, `implementer`, `verifier`,
  `infra-implementer`, and `infra-reviewer` definitions by id.
- `implement` and `deliver` reference their role definitions by id.
- The bundle no longer ships `agents/` directories; install the definitions into
  `.meerai/agents/` at the project root.

## 1.2.0

- `investigate`: added a fast path — Locate, Rule, and Ownership questions are answered within a
  search budget without booting the repository profile; escalation to the full workflow is explicit.
- `investigate`: new budgeted search subagent brief, `agents/search.md`, for narrow parallel lookups
  where `agents/recon.md` (trace, pattern, blast radius) would be too slow.

## 1.1.0

- Renamed the skills to verb-first IDs: `explore` → `investigate`, `coding` → `implement`,
  `infra` → `deliver`, `pm-ba` → `specify`, `tech-lead` → `orchestrate`. Descriptions and behavior
  are unchanged; everything citing a skill by ID (cross-references, role-brief paths, subagent
  definitions) was updated in the same change.

## 1.0.0

- Initial public bundle: `explore`, `coding`, `infra`, `pm-ba`, `tech-lead`.
- Repository-agnostic design: every skill reads `.agents/repo/profile.md` and falls back to
  discovering rules from agent files, manifests, and CI.
- Optional subagent delegation with role briefs for recon, implementation, verification, and infra
  review.
- Template `templates/repo-profile.md` for adopting the bundle in a new repository.
- MIT license.
