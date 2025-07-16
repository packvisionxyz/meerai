# Planning template

Copy into the session or the initiative document. Keep it short enough to hold in your head.

## Frame

| Field | Value |
| :--- | :--- |
| Goal | |
| Done condition | |
| Surfaces | |
| Frozen decisions that apply (profile §11) | |
| Open questions | |

## Contracts

| Contract | Change | Owner / mint | Consumer | Codegen |
| :--- | :--- | :--- | :--- | :--- |
| Schema / migration | | | | |
| API / RPC / event | | | | |
| Shared types / client | | | | |
| Infra / config | | | | |

## Workstreams

| # | Workstream | Owner (agent) | Files owned | Depends on | Verification |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | | | | | |
| 2 | | | | | |

Rules:

- File sets are disjoint within a parallel round.
- A workstream that changes a contract lands first.
- Every row has an exact verification command.

## Integration order

1. Freeze/land contract.
2. Run codegen.
3. Producer (server/worker).
4. Shared client/types.
5. Consumers (web/mobile/desktop).
6. Tests and E2E.

## Risks

| Risk | Likelihood | Impact | Mitigation |
| :--- | :--- | :--- | :--- |
| Migration not backward compatible | | | |
| Entitlement/gating path missed | | | |
| Cross-service contract lands one-sided | | | |
| Generated files edited by hand | | | |
| Two agents touch the same file | | | |

## Phase gate

For each phase:

- Scope in one line.
- Done condition (command or observable).
- Surfaces touched.
- Deferred items.

## Final verification

| Gate | Command | Result |
| :--- | :--- | :--- |
| Format | | |
| Lint | | |
| Typecheck / build | | |
| Unit | | |
| Integration | | |
| E2E (if user-visible) | | |
| Infra (if touched) | | |
