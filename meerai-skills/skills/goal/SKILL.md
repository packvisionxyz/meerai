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
  observation. Add a review item when a human must approve the result.

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

## 5. Human review

Some items need a human decision, not more work. Add a review item when:

- The item is an irreversible or external action.
- The item is a customer-facing or published artifact.
- The item changes a frozen decision or a shared contract.
- The request names an approver.

The action is to ask the human. Call `question-ask` with a clear approve or
revise choice. The observation is the exact answer. Record it with
`loop-observe`. Do not mark the item done until the answer approves it. When the
answer asks for changes, keep the item open and add the requested work.

A review from a subagent is not a human review. If no human is available, stop
and report the open review item.

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
