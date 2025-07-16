# Implementer subagent brief

One brief per workstream. Replace placeholders. The brief must stand alone. `<skills-dir>` is the
installed location of this bundle (for example `.agents/skills` or `~/.agents/skills`).

```text
Repository: <absolute path>
Feature: <name>

Decision already made: <contract/approach that must not change>

Read first:
- .agents/repo/profile.md (§3, §4, §5, §6, §7)
- <nearest package rules file>
- <skills-dir>/implement/SKILL.md and <skills-dir>/implement/references/change-recipes.md
- <files to extend>

Scope — you own:
- <paths>

Out of scope — do not touch:
- Generated output: <profile §6 paths>
- <other workstream paths>
- Unrelated formatting, renames, or refactors

Requirements:
- <behavior 1>
- <behavior 2>
- Tests at the level the layer requires.
- All supported locales updated together for new copy.

Done condition (run and report output):
- <exact commands from profile §7>

Output:
1. Files changed, one line of reasoning each.
2. Commands run and their real result.
3. Deviations or open questions; do not apply a deviation silently.

Constraints:
- Do not commit, push, deploy, rotate secrets, or run destructive git commands.
- Do not edit a frozen decision; report it instead.
- If the scope is wrong, stop and say so rather than widening it.
```

## Decomposition rules

- One writer per file set per round.
- The contract lands before parallel implementers start.
- Shared generated output is written only by the codegen command.
- If two workstreams need the same file, merge them into one workstream.
- Prefer a vertical slice that can be verified over "all backend, then all frontend".

## Reviewing the result

- Diff scope matches the brief.
- No generated file modified.
- Tests exist at the right level.
- Commands were actually run.
- Deviations are reported, not hidden.
