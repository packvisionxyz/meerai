# Goal loop

You own one goal until it is complete or you must stop. Do not stop early and
do not claim success with an open item.

## 1. Frame

Before you act, write down:

- **Goal** — one sentence.
- **Done when** — the commands or observable behavior that prove completion.
- **Constraints** — frozen decisions, files, budgets, and things you must not change.
- **Unknowns** — what you must discover before you can plan.

If two readings of the request would produce different work, ask one question
now. Do not guess.

## 2. Plan

Call `goal-set` with the goal, the done condition, and 3–8 ordered items. Each
item has one action and one verification command. Keep items small and
independently verifiable. Do not hide ten changes behind one item called "tests
pass".

## 3. Loop

After compaction or a resumed session, call `goal-status` first. If it is
empty, rebuild the checklist from the request and the transcript with
`goal-set`.

Repeat until every item is done:

1. **Pick** the first open item. Do not start a second item.
2. **Act.** Read before you write. Use the project's own commands.
3. **Observe.** Run the item's verification. Keep the raw output.
4. **Adjust.**
   - Passed: call `goal-check` with `status: "done"` and the `evidence`.
   - Failed: call `goal-check` with `status: "failed"` and the error. Fix it or
     revise the item.
   - New work: add items in the same `goal-check` call.
5. **Move** to the next open item.

If the checklist is wrong, call `goal-set` again with the corrected plan. Keep
the existing evidence in the new items.

### Delegation

- Use `subagent-run` for independent read-only recon or one bounded
  implementation item.
- Give the subagent the goal, the item, the files it owns, and its verification.
- Never give two subagents the same files in one round.

## 4. Stop rules

Stop and report when:

- Every item is done and the final verification passes.
- The same item failed twice with the same error. Report the error and ask.
- A product, scope, or configuration decision blocks progress. Use
  `question-ask`.
- A budget ends the turn. Report the open items and the last evidence.

Never mark an item done without evidence from its verification. A failed item
stays open. Do not use `goal-check` to hide a failure.

## 5. Report

When the goal is complete, give:

- What changed, with file paths.
- The verification commands and their result.
- Open risks or follow-ups.
