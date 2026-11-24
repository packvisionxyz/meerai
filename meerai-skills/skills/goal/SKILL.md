# Goal loop

You own one objective until it is done or you must stop. Do not stop early and
do not claim success with an open item.

## 1. Plan

Call `loop-plan` with:

- **Objective** — one sentence.
- **Done when** — the observable results that prove completion.
- **Constraints** — frozen decisions, owned resources, budgets, and things you
  must not change.
- **Items** — 3–8 ordered actions. Each item has one action and one expected
  observation.

If two readings of the request would produce different work, ask one question
with `question-ask` now. Do not guess.

## 2. Act

The loop starts on the first open item. Read before you write. Do one action,
then call `loop-act` with what you did and the observation you expect.

Do not start a second item.

## 3. Observe

Collect real evidence: command output, URL text, a measurement, or a document
quote. Keep the raw result. Call `loop-observe` with it.

An empty observation is a defect. A failed command is still an observation.

## 4. Adjust

Call `loop-adjust` with one status:

- `done` — the observation proves the item. The loop moves to the next item.
- `open` — the item needs another action. The loop retries the same item.
- `failed` — the item cannot pass. The loop returns to plan.

Add newly discovered items in the same `loop-adjust` call. For a wrong plan,
call `loop-plan` again and pass the `id` of every item that stays; its action,
observation, and status are preserved.

## Rules

- Call `loop-status` first after compaction or a resumed session.
- Never mark an item done without an observation.
- A failed item stays open.
- Never call `loop-act` outside the act phase, and never call `loop-adjust`
  before `loop-observe`.
- Use `subagent-run` for one bounded item when another session helps.
- Stop and ask when a decision blocks progress or a budget ends the turn.

## Report

When every item is done, report:

- What changed, with paths or resource names.
- The observations and their results.
- Open risks or follow-ups.
