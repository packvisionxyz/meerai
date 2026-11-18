# OpenAI ChatGPT Plan Usage

This document specifies how meerai can use a user's ChatGPT Plus/Pro subscription
as a model source through OpenAI's official "Sign in with ChatGPT" plan-usage
flow, instead of requiring a Platform API key.

| | |
| --- | --- |
| **Status** | Implemented (phases 1–4; phase 0 live provider spike pending) — tree checked 2026-11-18 |
| **Audience** | Maintainers of the provider layer, CLI, TUI, and security review |
| **Scope** | `meerai-client`, `meerai-providers`, `meerai-cli`, `meerai-tui`, `meerai-agents` request shaping, `providers.yml`, docs |
| **Related** | [harness-spec.md](harness-spec.md) · [threat-model.md](threat-model.md) · [harness-internals.md](harness-internals.md) · [architecture.md](architecture.md) |

## 1. Overview

Today meerai reaches OpenAI only with a Platform API key. Users who hold a
ChatGPT Plus/Pro subscription pay twice: once for the plan and again per token
for the same models. OpenAI's **Sign in with ChatGPT** plan-usage flow lets a
locally running open-source tool spend the user's plan allowance on eligible
Responses API requests.

The outcome this document targets: **a terminal developer signs in once with a
ChatGPT account and runs meerai turns on that plan, with the same tools, policy,
session log, and TUI behavior as an API-key run, and with no credential ever
logged.**

This is an authentication and request-shaping change. It adds no new agent
capability. It must not weaken policy, redaction, or sandbox behavior.

**Done condition:** phases 0–4 pass their acceptance criteria, `make verify` is
green, and a plan-credential headless run completes on macOS and Linux without a
credential value appearing in any log, event, or error.

## 2. Current state (evidence)

### 2.1 Connect flow is API-key only

- `/connect` selects a provider, then shows one masked **API key** field and an
  optional **Base URL** field (`meerai-tui/src/ui/modals.rs:117-230`).
- On Enter it calls `SecretStore::set_key`, saves the base URL into
  `settings.yml`, prints `Connected <provider>`, and opens the model input
  (`meerai-tui/src/app/modals.rs:232-277`).
- `meerai auth login <provider>` reads one line and stores it as a key
  (`meerai-cli/src/auth_cmd.rs:24-41`). `AuthCommand` has only
  `List`/`Login { provider, key }`/`Logout` (`meerai-cli/src/cli.rs:177-186`).
- Providers without a key go straight to model selection
  (`meerai-tui/src/app/modals.rs:116-119`), driven by `requires_api_key`
  (`meerai-client/src/providers.rs:17-26`).

### 2.2 Credentials are one string per provider

- `SecretStore` is `get_key`/`set_key`/`delete_key`/`list_providers`, all
  `Option<String>` (`meerai-client/src/secrets.rs:13-18`).
- `FileSecretStore` parses `auth.json` and **drops every entry whose `type` is
  not `api`**; only the `key` field is read (`meerai-providers/src/secrets.rs:35-59`,
  test at `:656-676`).
- `KeychainSecretStore` tries the OS keychain, then the file fallback
  (`meerai-providers/src/secrets.rs:522-566`).
- Nothing in the tree models expiry, refresh, scopes, or a client id.

### 2.3 The OpenAI provider forces chat completions

- `providers.yml:38-43` declares `openai` with `adapter: openai` and no base URL.
- `parse_adapter` maps unknown adapter names to `openai`
  (`meerai-client/src/providers.rs:116-137`); `openai_resp` is not in the list.
- `genai_model_spec` always prefixes the adapter (`:29-34`), and
  `format_genai_spec` returns a model unchanged only when it already contains
  `::` (`:337-349`). A user can therefore type `openai_resp::<model>` today, but
  the documented provider entry cannot express it.

### 2.4 genai 0.7.5 Responses behavior

Citations to `genai-0.7.5/…` below refer to the crates.io source of the pinned
dependency, not to a path in this repository.

The pinned `genai` 0.7.5 adapter is closer to the plan contract than the provider
entry suggests:

- `ChatRequest.system` becomes the top-level `instructions` field; it is not
  emitted as a system input item (`genai-0.7.5/src/adapter/adapters/openai_resp/adapter_impl.rs:125-173`).
- `store` defaults to `false` and is never set implicitly
  (`:137-158`). meerai always streams.
- `response.failed` becomes a stream error, but only `error.message` is kept;
  the structured `error.code` is dropped
  (`genai-0.7.5/src/adapter/adapters/openai_resp/streamer.rs:421-433`).
- Setting `CacheControl` maps to `prompt_cache_retention: "in_memory"` for
  `"https://api.openai.com"` endpoints on models that do not require explicit
  cache, and to `prompt_cache_options` on models that do
  (`genai-0.7.5/src/adapter/adapters/openai_resp/adapter_impl.rs:154-168,290-305`,
  `.../openai/cache_policy.rs:26-34`).
- `temperature` is sent when configured (`.../openai_resp/adapter_impl.rs:261-265`).

meerai sets `CacheControl::Ephemeral` on every step unless
`context.prompt_caching` is false, and sets `temperature` when configured
(`meerai-agents/src/multi_turn_agent/turn.rs:225-249`). Both fields are rejected
by the plan route (§4.4).

### 2.5 No dynamic model catalog and no browser seam

- The model list is static (`meerai-client/providers.yml:73-91`); no code calls
  `/v1/models`.
- The TUI never spawns a process; `!` commands go through
  `SessionOpener::run_user_shell` (`AGENTS.md:91`, `docs/tui-design.md:498-499`,
  `meerai-tui/src/lib.rs:67-83`).
- No browser-launch helper exists in the tree.

## 3. Decisions

**Q1 — Which OpenAI route do we integrate?**
Chosen: the official **Sign in with ChatGPT plan-usage** flow with
`chatgpt.tokens.use.direct` against `https://api.openai.com/v1/responses`.
Why: it is documented for open-source and locally hosted apps, needs no client
secret or partner key, and OpenAI publishes the request contract, scopes, and
error codes. meerai is an Apache-2.0 local CLI, which matches the OSS path.
Rejected: the Codex backend (`chatgpt.com/backend-api/codex`) — undocumented for
third parties, requires `chatgpt-account-id`/`originator`/beta headers, zstd
bodies, and a WebSocket transport; the official docs explicitly say not to point
plan requests at `backend-api`. Pi supports it, but that is reverse engineering,
not a contract.
Rejected: delegating to `codex app-server` — it is documented, but the external
process owns tools and policy, which contradicts meerai's harness value.
Revisit if: OpenAI withdraws the OSS self-serve path, or the app-server route
becomes the only supported one.
Resolved (2026-11-18): meerai is free to use and has no commercial or hosted
distribution, so the open-source self-serve path applies and the partner interest
form is not required.

**Q2 — One provider or two?**
Chosen: keep **one `openai` provider** with two credential kinds. `/connect`
offers "Sign in with ChatGPT" and "Use an API key".
Why: the same model ids, base URL, and model catalog serve both; a second id
would duplicate model rows and make `openai/gpt-*` ambiguous. Pi uses one
provider with two auth kinds.
Rejected: `openai-chatgpt` as a separate provider id — duplicate catalog,
confusing picker, and a model name that resolves differently by id.
Revisit if: plan models diverge into a disjoint catalog that cannot share a
picker.

**Q3 — How are OAuth credentials stored?**
Chosen: a structured credential model in `meerai-client`
(`Credential::{ApiKey, OAuth}`) with credential methods added to the existing
`SecretStore` trait (defaulted, so every store compiles), and OAuth entries
serialized as `{"type":"oauth", ...}` in the existing `auth.json`. API keys stay
where they are (keychain first, file fallback).
Why: one trait keeps one handle and one writer; a separate store trait would
thread a second `Arc` through the CLI and TUI and risk two caches over one file.
Rejected: a second `CredentialStore` trait — dual-writer risk and wide plumbing.
Rejected: JSON-in-a-string through `set_key` — the keychain and file caches can
clobber each other, and `auth list`/masking would need to parse opaque text.
Rejected: keychain-only OAuth — every refresh rewrites the item, and each access
may spawn `security`, with no cross-process lock.
INSIST: atomic write, `0600`, one exclusive lock, and one writer for both entry
types (see Q10).

**Q4 — Which transport builds the request?**
Chosen: `genai` 0.7.5 `openai_resp` for the `openai` provider, with a
plan-usage request profile layered on top.
Why: it already maps the system prompt to `instructions`, defaults `store` to
false, streams, captures tool calls and usage, and surfaces `response.failed`.
Rejected: a meerai-owned HTTP/SSE transport — duplicates streaming, tool-call
capture, usage mapping, and retry policy.
Revisit if: the Phase 0 spike shows that lost `error.code` values prevent
classification of plan-limit failures; then either patch genai upstream or add a
narrow meerai transport for this provider.

**Q5 — How does the UI learn that a provider supports sign-in?**
Chosen: replace `ProviderInfo::requires_api_key` with
`ProviderAuthKind { ApiKey, OAuth, OAuthOrApiKey, None }` and update the call
sites that read the boolean (`meerai-client/src/providers.rs:352-416`,
`meerai-tui/src/app/modals.rs:116,1613`).
Why: the connect dialog must show two actions, and `first_configured_provider`
must count an OAuth credential as configured.
Rejected: a second boolean `supports_oauth` — the two flags can contradict.
Revisit if: a provider needs a third method (device code, ambient cloud auth).

**Q6 — What does the plan request profile enforce?**
Chosen: for a plan credential, force `store:false`, `stream:true`, no
`temperature`, no cache control, and no unsupported field; keep the full history
in `input`.
Why: the published preview limitations reject `temperature`,
`prompt_cache_retention`, and system input items; genai would send the first two
today.
Rejected: relying on the server to reject the body — a failed turn is not a
usable error for a local user, and the docs require the client to omit fields.
Revisit if: OpenAI removes a restriction from the preview.

**Q7 — Where does the model list come from?**
Chosen: fetch `GET https://api.openai.com/v1/models` with the plan access token
at connect and on model-picker refresh, keep entries with `visibility == "list"`,
use `slug` as the model id, and cache the result per account with a TTL. Manual
model entry stays available.
Why: eligibility is account-scoped; the docs say to request the account catalog
before showing a picker.
Rejected: the bundled static catalog — it cannot know plan eligibility.
Revisit if: the endpoint is unavailable to plan tokens.

**Q8 — What happens when plan usage fails?**
Chosen: no silent billing fallback. Map the documented codes to typed errors.
`subscription_sharing_usage_limit_exceeded` is terminal (no retry);
`subscription_sharing_usage_unavailable` and 503 use bounded backoff; 401/403
tell the user to sign in again or check the account and region.
Why: OpenAI documents that it does not switch billing paths, and a silent API-key
retry would spend money the user did not authorize for that run.
Rejected: automatic fallback to the stored API key — surprising spend and hides
the plan state.
Revisit if: the product adds an explicit per-run "fallback allowed" setting.

**Q9 — Who opens the browser?**
Chosen: `meerai-cli` owns browser launch through a `SessionOpener` method; the
TUI only calls the opener. Headless prints the URL and reads a pasted callback
URL.
Why: the TUI never spawns a process (`AGENTS.md:91`).
Rejected: launching a browser inside `meerai-tui` or `meerai-client` — violates
the process invariant and adds a platform dependency to a library.

**Q10 — How is refresh serialized?**
Chosen: refresh when less than three minutes remain, under an exclusive lock
file next to `auth.json`; re-read the entry inside the lock; persist the rotated
access token, refresh token, expiry, and scopes together.
Why: refresh tokens rotate and are single-use; two sessions refreshing
concurrently can invalidate each other. Pi and the OpenAI docs both require
serialization.
Rejected: refresh only on a 401 — access tokens live one hour, so a turn can
fail mid-stream.
Rejected: an in-process mutex only — separate `meerai` processes share the file.
INSIST: a terminal refresh error marks the credential "needs sign-in" but keeps
it, so a transient network failure does not destroy a usable refresh token.

**Q11 — What if the user declines plan usage?**
Chosen: complete the OAuth exchange, then require
`chatgpt.tokens.use.direct` in the granted scopes. If it is absent, store
nothing and exit with a typed error that names plan usage and the API-key
alternative.
Why: meerai has no identity feature, so an identity-only grant has no use.
Rejected: storing an identity-only credential — dead weight and confusing state.
Revisit if: meerai adds a hosted account feature.

**Q12 — Does the session record the credential kind?**
Chosen: no. Sessions keep the provider and model. The active credential kind is
runtime state and appears as a TUI badge.
Why: a user can sign in or out between runs, and resume should not pin a billing
method. Adding a persisted field would change `meta.json` for little value.
Rejected: `credential: plan|api` in session metadata — a contract change that
does not change replay.

**Q13 — What does logout do?**
Chosen: best-effort revoke at the documented revocation endpoint, then delete
the entry. Keep the `ext_agent_host_id`.
Why: the grant belongs to the user; leaving a live refresh token after logout is
a security defect.
Rejected: delete-only — leaves the grant active in ChatGPT settings.
Rejected: also deleting the host id — a new id would look like a new host and
loses plan-usage association.

**Q14 — Multi-account support in v1?**
Chosen: no. One `openai` registration per installation. Account switching is
explicitly out of scope.
Why: it needs an account picker, per-registration client ids, and a credential
selector in every surface.
Rejected: Pi-style snapshot switching — a follow-on feature, not a dependency.

## 4. Target behavior

### 4.1 Credential model

| Kind | Fields | Storage |
| --- | --- | --- |
| `ApiKey` | `key` | keychain first, `auth.json` fallback (current behavior) |
| `OAuth` | `access`, `refresh`, `expires` (ms), `client_id`, `scopes[]`, `account`, `needs_relogin` | `auth.json` entry `{"type":"oauth", ...}`, `0600` |

`account` is an optional display value from the ID token (email or subject). It
is never used as an identity key. `needs_relogin` marks a credential that must be
replaced by a new sign-in; it is kept so a transient failure does not destroy a
usable refresh token.

### 4.2 Surfaces

| Surface | Behavior |
| --- | --- |
| TUI `/connect` → OpenAI | No credential: `Continue with ChatGPT` and `Use an API key`. Configured: `Continue to model select`, `Update credential`, and `Remove saved credential`. |
| TUI statusline | `plan` badge when the active credential is OAuth; no temperature value (the field is ignored). |
| TUI sign-in row | Opens the system browser through the host, shows the URL as text, accepts a pasted redirect URL, and cancels with `Esc`. |
| `meerai auth login openai --chatgpt` | Loopback flow; falls back to paste when the port is busy or the browser cannot reach the host. |
| `meerai auth login openai --key` | Current key prompt. |
| `meerai auth list` | Shows `plan` with expiry, or `api` with a masked key; never the token. |
| `meerai auth logout openai` | Revokes best-effort, deletes the entry. |
| Headless run | Uses the stored credential; a missing or expired-plan credential fails with the `/connect` remedy. |

### 4.3 Precedence

Stored `OAuth` → stored `ApiKey` → config `api_key` → provider env var.

A stored credential owns the provider. If it is unusable, meerai reports the
error; it does not silently fall through to env vars or another credential.
This mirrors Pi's resolution rule and matches the "no silent billing fallback"
decision (Q8).

### 4.4 Plan request profile

| Field | Plan value | Reason |
| --- | --- | --- |
| `store` | `false` | Required by the plan route. |
| `stream` | `true` | Required by the plan route. |
| `instructions` | composed system prompt | System input items are rejected. |
| `input` | full history | `previous_response_id` is not supported over HTTP. |
| `temperature` | omitted | Unsupported; ignore a configured value with one warning. |
| `prompt_cache_retention` | omitted | Unsupported; force `context.prompt_caching = false` for this provider. |
| `max_output_tokens`, `top_p`, `truncation`, `metadata`, `user`, `safety_identifier`, `background`, `conversation`, `multi_agent`, `prompt` | omitted | Listed as unsupported for plan usage. |
| Tools | function tools only | Hosted tools, `tool_search`, and Connectors are unavailable. |

Implicit prompt caching remains active: omitting `prompt_cache_options` selects
OpenAI's implicit mode. Explicit breakpoints, `prompt_cache_options.ttl`, and
`prompt_cache_retention` are unavailable on the plan route. The provider reports
cache reads through `usage.input_tokens_details.cached_tokens`, which meerai
shows as `cached=` and `↻`.

### 4.5 Error map

| Code or status | Retry? | User-visible action |
| --- | --- | --- |
| `subscription_sharing_usage_limit_exceeded` (429) | No | Plan limit reached; open ChatGPT usage settings, or connect an API key. |
| `subscription_sharing_usage_unavailable` (503) | Bounded backoff | Temporary; credentials preserved. |
| `subscription_sharing_user_not_eligible` (403) | No | Plan usage is unavailable for this account, workspace, or region. |
| `subscription_sharing_unsupported_capability` (400) | No | meerai sent an unsupported field; report a bug with the request id. |
| `subscription_sharing_invalid_user` (401) | No | Sign in again. |
| `invalid_grant` / refresh-token family | No | Sign in again; the stored credential is marked unusable but kept. |
| Direct-admission `401`/`403`/`503` before streaming | Per status | Sign in again / show the region restriction / bounded retry. |
| Network failure | Bounded backoff | Current provider retry policy. |

## 5. Requirements

| ID | Requirement | Priority | Phase | Evidence |
| --- | --- | --- | --- | --- |
| R1 | Add `Credential`/`OAuthCredential` and credential methods on `SecretStore` in `meerai-client`; API keys keep their current surface. | NEED | 1 | `meerai-client/src/secrets.rs:13-18` |
| R2 | Read and write `{"type":"oauth"}` entries in `auth.json` with `0600` and atomic replace. | INSIST | 1 | `meerai-providers/src/secrets.rs:35-59` |
| R3 | Serialize every credential mutation, including refresh, under one cross-process lock. | INSIST | 1 | no lock exists today |
| R4 | Browser login: loopback `127.0.0.1`, path `/auth/callback`, PKCE S256, `state`, `nonce`, dynamic registration, persisted `ext_agent_host_id`. | NEED | 1 | no OAuth code |
| R5 | Paste fallback accepts the full redirect URL when the loopback callback is unreachable. | NEED | 1 | no browser seam |
| R6 | Require `chatgpt.tokens.use.direct` after exchange; store nothing when it is absent. | INSIST | 1 | `auth.json` skips OAuth today |
| R7 | Refresh at `< 3 min` remaining, persist rotation atomically, mark unusable credentials instead of deleting them. | INSIST | 1 | token lifetime is one hour |
| R8 | CLI: `auth login --chatgpt`, `auth list` with kind and expiry, `auth logout` with best-effort revoke. | NEED | 1 | `meerai-cli/src/cli.rs:177-186` |
| R9 | Add `ProviderAuthKind` and drive `/connect`/`first_configured_provider` from it. | NEED | 2 | `meerai-client/src/providers.rs:17-26` |
| R10 | Add `openai_resp` to `parse_adapter` and set the bundled `openai` provider to it. | NEED | 2 | `meerai-client/src/providers.rs:116-137` |
| R11 | Enforce the plan request profile and refuse unsupported fields with a typed error. | INSIST | 2 | `turn.rs:225-249` |
| R12 | Force prompt caching off and temperature unset for plan credentials, with one warning. | INSIST | 2 | `openai_resp/adapter_impl.rs:154-168,290-305` |
| R13 | Fetch and cache the account model catalog; keep manual entry as a fallback. | NEED | 2 | static `providers.yml:73-91` |
| R14 | Map plan error codes to typed errors; make the usage-limit code terminal. | INSIST | 2 | retry classifier is status-based |
| R15 | TUI: two connect actions, login modal, `plan` badge, usage-limit notice with the `/connect` remedy; the sign-in label follows OpenAI's UI/UX guidelines. | NEED | 3 | `modals.rs:232-277` |
| R16 | Browser launch lives in `meerai-cli`; the TUI only calls `SessionOpener`. | INSIST | 3 | `AGENTS.md:91` |
| R17 | Redaction covers JWT-shaped tokens, and no credential field reaches a log, event, hook payload, or error text. | INSIST | 4 | `meerai-agents/src/redact.rs` |
| R18 | Windows: loopback flow works with `start`; unit tests cover URL and command construction. | NEED | 4 | no Windows browser path |
| R19 | Update README, threat model, harness internals, and this document in the same change. | NEED | 4 | repo doc rule |

## 6. Acceptance criteria

### Phase 0 — Spike

**AC-0.1 — Plan request completes**
Given a ChatGPT Plus/Pro account and a manually obtained plan-scoped token,
when the operator sends `POST https://api.openai.com/v1/responses` with
`store:false` and `stream:true`, then the stream ends with `response.completed`.
Failure: an admission error is recorded with its exact status, body, and code.

**AC-0.2 — Tool loop works**
Given the same token, when the model emits a function tool call and the caller
returns a tool result, then the second request completes.
Failure: the request is rejected as `subscription_sharing_unsupported_capability`
and the exact `error.param` is recorded.

**AC-0.3 — Error codes survive**
Given an exhausted plan allowance, when a request fails mid-stream, then the
recorded evidence contains `subscription_sharing_usage_limit_exceeded` or the
exact alternative code.
Failure: only a human-readable message is available; this reopens Q4.

### Phase 1 — Credential and login

**AC-1.1 — Login stores a plan credential**
Given a valid account, when `meerai auth login openai --chatgpt` completes, then
`auth.json` contains one `type:"oauth"` entry with mode `0600`, and
`meerai auth list` prints `plan` with an expiry and no token text.
Failure: the command exits `1` and `auth.json` is unchanged.

**AC-1.2 — Rotation is serialized**
Given two processes refreshing the same expiring credential, when both refresh,
then exactly one token exchange occurs and both read the same new credential.
Failure: one process reports `invalid_grant` and the other turn still succeeds.

**AC-1.3 — Declined plan usage**
Given a consent result without `chatgpt.tokens.use.direct`, when the exchange
finishes, then no credential is stored and stderr names plan usage plus the
API-key alternative.
Failure: any partial credential is written.

**AC-1.4 — Callback unavailable**
Given port 1455 in use or a remote shell, when login starts, then the URL and a
paste prompt are shown, and a pasted redirect URL completes the login.
Failure: the flow hangs with no prompt.

### Phase 2 — Provider and request profile

**AC-2.1 — Headless plan run**
Given a stored plan credential, when `meerai --prompt "say hi" -p openai` runs,
then the turn completes and the stderr token summary prints.
Failure: the process exits `1` with the mapped plan error.

**AC-2.2 — Request shape**
Given a plan credential, when the request body is captured in a test, then it
contains `store:false`, `stream:true`, `instructions`, and no `temperature`,
`prompt_cache_retention`, `max_output_tokens`, or system input item.
Failure: any listed field is present.

**AC-2.3 — Temperature is ignored**
Given `context.temperature: 0.7`, when a plan turn runs, then no `temperature`
is sent and one warning is printed; an API-key turn still sends it.
Failure: the plan request fails with an unsupported-field error.

**AC-2.4 — Usage limit is terminal**
Given a mid-stream `subscription_sharing_usage_limit_exceeded`, when the error
arrives, then the turn fails once with the plan-limit message and no retry.
Failure: more than one request is observed.

**AC-2.5 — Account model list**
Given a plan credential, when the model picker opens, then it lists models from
`/v1/models` with `visibility == "list"`, and a manually typed id is still
accepted.
Failure: the picker is empty and manual entry is blocked.

### Phase 3 — TUI

**AC-3.1 — Two connect actions**
Given `/connect` and the OpenAI row with no credential, when Enter is pressed,
then the dialog offers "Sign in with ChatGPT" and "Use an API key".
Failure: the API-key field appears first for an OAuth choice.

**AC-3.1a — Configured provider skips reconnect**
Given `/connect` and a provider with a stored API key or plan credential, when
Enter is pressed, then the dialog offers "Continue to model select",
"Update credential", and "Remove saved credential", and no sign-in starts.
Failure: a new browser sign-in starts or the auth method dialog appears.

**AC-3.2 — No process from the TUI**
Given a TUI login, when the browser launch happens, then it goes through
`SessionOpener` implemented by `meerai-cli`, and `meerai-tui` contains no process
spawn.
Failure: a `Command::new` appears in the TUI crate.

**AC-3.3 — Cancel is safe**
Given a login in progress, when the user cancels, then the previous credential
and active provider are unchanged and no partial entry exists.
Failure: the active provider switches or a token is stored.

### Phase 4 — Hardening

**AC-4.1 — Token redaction**
Given a JWT-shaped string in tool output or an error body, when it is settled,
then the log, UI, and model input contain `[REDACTED:jwt]`.
Failure: any segment of the token survives.

**AC-4.2 — Docs match**
When the change is complete, then README, threat model, harness internals, and
this document agree, and the stale-claim grep is clean.
Failure: a doc says API-key-only or claims plan usage is implemented before it
is.

**AC-4.3 — Windows path**
Given Windows, when login builds the callback URL and browser command, then the
unit tests cover `127.0.0.1` and `start`, and the docs state that CI does not run
a live Windows login.
Failure: the Windows path is untested and undocumented.

## 7. Impact

| Area | Impact |
| --- | --- |
| Tenancy / authorization | None. Single-user local tool; no tenant data and no new role. The credential is user-local. |
| Entitlements | The allowance belongs to the user's ChatGPT plan, with per-app caps set in ChatGPT settings. meerai adds no limit. Session and turn token budgets still apply. |
| Offline / local-first | Login needs the network and a browser (or a pasted callback URL). Inference needs the network. Nothing extra is uploaded; the request body is the normal conversation. |
| Data lifecycle | Refresh rotates tokens; logout revokes best-effort and deletes the entry; the host id is retained. No session schema change (`Q12`). |
| Contracts | New `Credential`/`CredentialStore` in `meerai-client`; `ProviderInfo.auth` replaces the boolean; the `openai` adapter changes to `openai_resp`. `AgentEvent`, `SessionEvent`, and `meta.json` are unchanged. |
| Surfaces | `meerai-client`, `meerai-providers`, `meerai-cli` (auth, headless), `meerai-tui` (connect, statusline, errors), `providers.yml`, docs. |
| Localization | None. The CLI and TUI are English-only today. |
| Failure states | Declined consent, missing scope, port busy, remote shell, expired/invalid refresh token, plan limit, usage unavailable, ineligible account, region block, revoked app, upstream 5xx. |
| Observability | No telemetry. Typed errors carry status, code, and request id where available. Tokens never appear in logs or events. |
| Rollout | Opt-in through login. The `openai` adapter change affects every OpenAI user, so it needs the full provider test matrix and a smoke run. Rollback reverts the adapter; stored credentials remain usable later. |
| Support | `meerai auth list` shows kind and expiry. Error text names the remedy: sign in again, open ChatGPT usage settings, or connect an API key. |

## 8. Non-goals

- The Codex backend (`chatgpt.com/backend-api/codex`), WebSocket transport, and
  zstd request bodies.
- Identity sign-in, meerai accounts, or any profile data use.
- A commercial or remotely hosted partner integration.
- Multi-account switching in v1.
- USD cost for plan usage; token counts remain the report.
- Automatic billing fallback between plan and API key.
- Delegating turns to `codex app-server`.
- Importing credentials from Pi, Codex CLI, or `~/.codex/auth.json`.
- Retaining identity-only sign-in when the user declines plan usage. meerai does
  not use identity claims, so a grant without `chatgpt.tokens.use.direct` fails
  and stores nothing.
- A consent re-prompt (`prompt=consent` / `force_reconsent`) after a decline.

## 9. Phases

### Implementation result (2026-11-18)

Phases 1–4 are implemented in the tree. `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets`, and `cargo test --workspace` pass;
`meerai-providers` has 50 unit tests and `meerai-tui` 204. Phase 0 was not run:
it needs a real ChatGPT Plus/Pro account and a browser, which the implementation
session did not have. The live sign-in, one streamed plan request, one tool
round-trip, and one forced usage-limit error remain the gate before release.

### Phase 0 — Spike (not shippable)

- Scope: manual token, one streamed request, one tool round-trip, one forced
  usage-limit error.
- Done: AC-0.1 through AC-0.3 recorded in this document's open questions.
- Surfaces: none in the tree.
- Deferred: everything else.

### Phase 1 — Credential and login

- Scope: credential model, store, lock, loopback login, paste fallback, refresh,
  CLI login/list/logout.
- Done: AC-1.1 through AC-1.4.
- Surfaces: `meerai-client`, `meerai-providers`, `meerai-cli`.
- Deferred: provider request profile, TUI, model catalog.

### Phase 2 — Provider and request profile

- Scope: `openai_resp`, plan request profile, model catalog, error map.
- Done: AC-2.1 through AC-2.5, plus the repo gate.
- Surfaces: `meerai-client`, `meerai-providers`, `meerai-agents` request shaping.
- Deferred: TUI polish, redaction, Windows.

### Phase 3 — TUI

- Scope: connect actions, login modal, plan badge, usage-limit notice, browser
  seam.
- Done: AC-3.1 through AC-3.3.
- Surfaces: `meerai-tui`, `meerai-cli`.
- Deferred: multi-account, account switcher.

### Phase 4 — Hardening and docs

- Scope: JWT redaction, Windows path, docs, support surface.
- Done: AC-4.1 through AC-4.3, plus `make verify`.
- Surfaces: `meerai-agents`, `meerai-cli`, docs.
- Deferred: upstream genai error-code contribution, if still needed.

## 10. Open questions

| # | Question | What settles it | Owner |
| --- | --- | --- | --- |
| 1 | Which models are eligible per account, and does `visibility: "list"` match them? | Phase 0 live spike plus `/v1/models` on two accounts | Provider owner |
| 2 | Does the plan route accept genai's top-level function tool schema, or does it require namespaces or `additional_tools`? | Phase 0 AC-0.2 | Provider owner |
| 3 | Does the live sign-in return the issued `client_id` in the callback, the token body, or both? | Phase 0; both paths are parsed today | Provider owner |
| 4 | Does the live ID token verify against the published JWKS with the `kid` we receive? | Phase 0; the code caches the JWKS for one hour and refetches once on an unknown `kid` | Provider owner |
| 5 | Is a Windows loopback login acceptable for support, given CI has no Windows runner? | AC-4.3 and a manual Windows pass | Maintainers |
| 6 | Does a mid-stream plan-limit error carry a readable code through genai 0.7.5? | Phase 0 AC-0.3; if not, Q4's revisit condition applies | Provider owner |
| 7 | On GPT-6.2 and later, top-level `instructions` are appended after `input` and billed uncached. Should plan requests move the stable system prompt into a developer message? | Cache-hit measurement on a 6.2 account | Provider owner |

## 11. Reference

### 11.1 Endpoints and scopes

| Item | Value |
| --- | --- |
| Authorization | `https://auth.openai.com/api/accounts/authorize` |
| Token | `https://auth.openai.com/api/accounts/oauth/token` |
| Revocation | `https://auth.openai.com/api/accounts/oauth/revoke` |
| JWKS | `https://auth.openai.com/.well-known/jwks.json` |
| Discovery | `https://auth.openai.com/.well-known/openid-configuration` |
| Resource | `https://api.openai.com/v1` (the guide's table shows the bare audience form; the live authorize endpoint rejects it, so the full URI is used in the authorize, exchange, and refresh requests) |
| Identity scopes | `openid profile email` |
| Plan scopes | `offline_access resource.invoke chatgpt.tokens.use.direct` |
| Dynamic client id | `dynamic_agent_client` on registration; store the issued `client_id` returned by the callback |
| Callback | `http://127.0.0.1:<port>/auth/callback`; scheme, host, and path are fixed, port may vary |
| Access token | One hour |
| Refresh token | 30 days, rotated on every refresh |

### 11.2 Prior art

- OpenAI, *Sign in with ChatGPT*: <https://developers.openai.com/siwc/token-sharing-open-source>
- OpenAI, *Preview limitations*: <https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations>
- OpenAI, *Errors and recovery*: <https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery>
- Pi, `packages/ai/src/auth/oauth/openai-chatgpt.ts` and
  `packages/ai/src/api/openai-responses.ts`: a working Rust-independent client
  for the same flow.
- Pi, `openai-codex.ts`: the rejected Codex-backend route.

### 11.3 Implementation notes for the tech lead

- Reuse the workspace `reqwest` (rustls) for token and model requests; do not add
  a second TLS stack.
- The loopback listener can be a small `tokio::net::TcpListener` handler for one
  route; a full HTTP framework is not required.
- PKCE needs SHA-256 plus base64url; `state`/`nonce` need a CSPRNG; the host id
  needs a UUIDv4 formatter. Choose crates under the existing supply-chain policy
  (`deny.toml`).
- Validate the ID token with the published JWKS (RS256), plus `iss`, `aud`,
  `exp`, and `nonce`. Pi checks presence only; meerai verifies the signature.
- genai 0.7.5 adds `prompt_cache_options` for gpt-6-family models on
  `api.openai.com` even when no cache intent is set; the plan route rejects that
  field. Plan requests keep the same endpoint with an explicit `:443`, which
  bypasses the adapter's string check, so the field is omitted.
  `RuntimeProviderState.plan_usage` selects that endpoint. Omitting the field
  leaves OpenAI's implicit caching on; genai's `mode: "explicit"` with no
  breakpoints would disable caching on GPT-5.6 and later.
- The running agent receives `AgentCommand::SetPlanUsage` when the credential
  kind or provider changes, so an in-TUI login takes effect without a restart.
- ID token signatures use `jsonwebtoken 12` with the `aws_lc_rs` crypto
  provider. Its `rust_crypto` feature is not usable because its `p256`
  dependency has no published stable release.
- Keep the callback handler free of blocking I/O and close the listener on every
  exit path, including cancel.
- Unit-test the request profile by capturing the serialized payload; do not rely
  on a live provider for CI.

### 11.4 Compliance with the OpenAI guide

Audited against `developers.openai.com/siwc/token-sharing-open-source` on
2026-11-18.

| Guide area | Status | Note |
| --- | --- | --- |
| Registration and sign-in | Compliant | Dynamic registration, saved issued `client_id`, `agent_name_hint` only on new registration. |
| PKCE, state, nonce | Compliant | S256; state checked on the callback; nonce checked in the ID token. |
| Loopback callback | Compliant | `127.0.0.1`, path `/auth/callback`, port 1455 or ephemeral, same URI in exchange. |
| ID token | Compliant | RS256 verified with the published JWKS (cached one hour), plus `iss`, `aud`, `exp`, `nonce`. |
| Scopes and resource | Compliant | Requires `chatgpt.tokens.use.direct`; `resource=https://api.openai.com/v1` (the full URI, verified against the live authorize endpoint). |
| Token exchange and refresh | Compliant | Form POST, no client secret, issued client id, rotating refresh token, serialized under a lock. |
| Revocation | Compliant | Best-effort revoke before delete; skipped when no issued client id exists. |
| Credential storage | Compliant | `auth.json`, `0600`, atomic replace, token never listed or logged. |
| Accounts and sessions | Partial | One registration per installation; no account picker. |
| Models and inference | Partial | CLI lists `/v1/models` and seeds recent models; the TUI picker has no dedicated catalog view. |
| Request profile | Compliant | `store:false`, `stream:true`, `instructions`, full history, no temperature, no cache retention. |
| Errors and recovery | Partial | Documented codes are terminal or retried correctly; request ids are not surfaced, and genai can drop a mid-stream `error.code`. |
| Preview limitations | Compliant | Unsupported fields are omitted; function tools only. |
| UI/UX guidelines | Partial | Sign-in label is `Continue with ChatGPT`; the TUI sign-in modal shows the URL and paste field, but there is no connection card or usage link. |
| Codex app-server | Not used | meerai calls the Responses API directly. |
| Declined plan scope | Deviation | meerai stores nothing because it consumes no identity claims. |
| Consent re-prompt | Not implemented | A new sign-in repeats the full scope request; no `force_reconsent`. |

## 12. Invariants

- The plan credential is user-local. A project `.meerai/settings.yml` cannot
  select it, switch the billing path, or override `providers` (already ignored).
- No credential field (access token, refresh token, ID token, client id secret)
  reaches a log, `AgentEvent`, `SessionEvent`, hook payload, or error text.
- The plan request profile is enforced in one place. No call site may add
  `temperature`, cache control, or an unsupported field for a plan credential.
- `auth.json` has one writer. API-key edits and OAuth refresh share one lock and
  one read-modify-write path.
- The TUI never spawns a process. Browser launch belongs to `meerai-cli`.
- A refresh failure never deletes a possibly valid refresh token; it marks the
  credential unusable and asks for sign-in.
