# Document review against the codebase

A document review answers one question: are the document's claims true of the code today. It is a
verification pass, not a rewrite and not a summary.

## 1. Build the claim inventory

Read the document once and extract every checkable claim. A claim is checkable when a file, command,
or configuration value can contradict it.

| Kind | Example |
| :--- | :--- |
| Command | `meerai sessions prune --days 7 --keep 5` |
| Path or location | `~/.local/share/meerai/meerai.db` |
| Default | approval mode defaults to `ask` |
| Limit or range | temperature accepts `0.0..=2.0` |
| Flag or option | `--json` requires `--prompt` |
| Behaviour | a declined spend approval exits `1`, not `2` |
| Invariant | a project file can tighten but never grant |
| Interface | `/diagnose` toggles the sidebar block |
| Ordering | skills apply before the agent |

Ignore marketing prose, motivation, and roadmap items marked planned. They are not claims about
current behaviour. A roadmap item that ships without its status changing **is** a finding.

## 2. Verify each claim

Verify against the strongest source available, in this order:

1. The code path that implements it.
2. The test that pins it.
3. The configuration default or schema.
4. The CI workflow or script that enforces it.
5. Another document — only when nothing above exists, and say so.

Quote the evidence as `path:line`. For a command, the claim is true when the parser accepts those
flags and the handler performs the described effect, not when the help text repeats it.

## 3. Classify

| Status | Meaning | Action |
| :--- | :--- | :--- |
| Confirmed | The code matches the claim. | None. |
| Stale | The code moved; the document did not. | Report the exact correction. |
| Missing | A user-facing behavior exists but the owning document omits it. | Report the gap and the owning section. |
| Contradicts | The document and the code disagree, and one is a defect. | Say which one is wrong and why. |

Report the document location as a heading or anchor plus the quoted claim, so the author can find it
without a line number that will drift.

## 4. Check the document set, not one file

Facts live in one owning document. Check that the fact did not get copied:

- Root `README.md` — user-facing behavior, commands, defaults.
- `docs/` — the owning topic file, plus the architecture and internal references.
- The repository profile and the rules file — invariants, commands, ownership.
- Generated site or help output — must match its source, never be edited by hand.

A fact that appears in two places and differs is a finding even when one copy is right, because the
next change will update only one.

## 5. Findings

Write findings in the same severity scale as a code review:

- **Blocker** — the document tells a user to run a command that fails, points at a path that does not
  exist, or states an invariant the code violates.
- **Should fix** — stale default, missing user-facing behavior, doc set not updated with a behavior
  change.
- **Note** — wording, ordering, or a duplicated fact that is still correct.

Each finding: document location, quoted claim, evidence path, status, and the smallest correction.

## 6. Boundaries

- Do not rewrite the document in a review. Report; the **specify** skill owns a rewrite.
- Do not treat a passing test suite as proof that the prose is true. Tests prove behavior; they do
  not prove the sentence that describes it.
- Do not accept a document as authoritative because it is recent. Check the code.
- Do not report a claim as confirmed when you only read another document.