# Impact checklist

Expanded prompts for the table in `SKILL.md`. Answer every category; "none" is a valid answer.

## Tenancy and authorization

- Which identity resolves the tenant: session, token claim, path parameter, or request body?
- Which query filters by tenant? Is the filter in the data layer, not only in code?
- What role or permission is required? Is a narrower one enough?
- Can one tenant observe another's ids, counts, or existence?
- Does the feature create a new cross-tenant surface (search, export, webhook, admin view)?

## Entitlements and quotas

- Does it create a metered or limited object (seat, device, storage byte, transfer byte)?
- Which limit applies, and where is it enforced — one place, or several?
- What does the user see at the limit? Is it recoverable?
- Are any paths exempt (local, peer-to-peer, cached)? State them.
- Does an expired plan block it, degrade it, or leave it working?

## Offline and local-first

- What is uploaded by default? The answer should be "nothing" unless explicitly chosen.
- What works with the upstream down? What queues, retries, or fails?
- Is there a local source of truth, and how is it reconciled on reconnect?
- Are timestamps and ordering safe across clock skew and reconnects?

## Data lifecycle

- Retention: who deletes, when, and how is it proven?
- Deletion: soft or hard? Does it cascade? Does it leave orphans?
- Backfill: is it needed? Reversible? Re-runnable?
- Export: is the data portable, and in what format?

## Contracts

- Which schema, API, event, or IPC contract changes?
- Is the change backward compatible? Which clients are versioned independently?
- Who mints and who verifies? Both sides must change together.
- What is the migration order for a rolling deploy?

## Surfaces

- Server, worker, web, desktop, mobile, CLI, admin, docs.
- Which surface is the source of truth for the behavior?
- Which surface can ship later without breaking the flow?

## Localization and accessibility

- Every supported locale, including the docs site.
- Right-to-left, long strings, dates, numbers, currency.
- Keyboard, screen reader, contrast, focus.

## Failure states

- Offline, slow, timeout, partial success.
- Entitlement expired, quota exhausted, region blocked.
- Upstream down, missing data, corrupt data.
- What does the operator see, and what can support do?

## Observability

- What log, event, metric, or trace proves the behavior in production?
- What would alert, and at what threshold?
- Can a failure be diagnosed without reproducing it?

## Rollout

- Feature flag, plan gate, allowlist, or percentage.
- Migration order and backward compatibility.
- Rollback: what reverts, and what data is already written?
- What is the kill switch?
