# Local browser UI for meerai

This document defines the loopback browser companion: the implemented host, protocol, server, and Leptos DOM client, plus the remaining parity work.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-19. Host, protocol, server, launch surfaces, and the Leptos client are implemented. Rune is removed. |
| **Audience** | Terminal developers, tech lead, security reviewers, contributors |
| **Scope** | `meerai serve` (CLI) and `/serve` (TUI); `meerai-host`; `meerai-web-protocol`; `meerai-web`; `meerai-web-client`; `scripts/build-web.py` |
| **Related** | [Architecture](architecture.md) · [Harness spec](harness-spec.md) · [TUI design](tui-design.md) · [Threat model](threat-model.md) · [AGENTS.md](../AGENTS.md) |

## 1. Overview

The browser companion lets a developer start, supervise, review, and resume a coding task from a browser on the same machine. It uses the same agent, session log, policy engine, approvals, and workspace as the TUI. The native process keeps credentials, tool execution, and the authoritative live state. The browser is a second client of the same live session registry.

The client is a Leptos 0.8 client-side-rendered DOM application. It replaces the earlier Rune canvas client. Text, controls, dialogs, and diffs are real DOM nodes, so browser selection, find-in-page, zoom, screen readers, IME, and native form controls work. The client does not need WebGPU and runs in Chromium, Firefox, and Safari.

**Done condition for the first full release:** the browser covers the implemented TUI workflows in [TUI design](tui-design.md#2-implemented), including the [transcript invariants](tui-design.md#5-invariants). The browser authenticates with a launch secret. Reloading neither repeats a turn nor bypasses policy. The remaining gaps are listed in §3.

## 2. Implemented

The companion is behind the opt-in `web` Cargo feature on `meerai-cli`. The TUI starts the same server through `meerai_host::WebServerLauncher`, so `meerai-tui` does not depend on `meerai-web`.

### 2.1 Host seams and live registry

`meerai-host` owns `SessionOpener`, `OpenedSession`, `WorkspaceQuery`, `WebServerLauncher`, and `SessionRegistry` ([`meerai-host/src/session.rs`](../meerai-host/src/session.rs), [`meerai-host/src/registry.rs`](../meerai-host/src/registry.rs)). The registry keeps every live worker's command sender, folds `AgentEvent`s into a live view, appends view events to a bounded monotonic ring, and broadcasts them to attached clients. The TUI and the browser attach to the same registry. A decision in either client resolves the request for both.

`CliSessionHost` implements the host traits in [`meerai-cli/src/main.rs`](../meerai-cli/src/main.rs). Diff review, syntax highlighting, theme resolution, update status, and workspace search live in `meerai-host`, not in the TUI.

### 2.2 Session ownership

`SessionLock` uses an advisory `session.lock` with `try_lock`, mode `0600` on Unix, and a PID holder hint. It prevents two processes from writing one session. `Session::fork_stored` reads a stored log without the exclusive lock, so a live or stored session forks cleanly. The lock lives in [`meerai-session/src/session.rs`](../meerai-session/src/session.rs).

### 2.3 Server and authorization

`meerai-web` runs a `hyper` HTTP/1 server on `127.0.0.1` ([`meerai-web/src/server.rs`](../meerai-web/src/server.rs)). It refuses a non-loopback bind. It serves the embedded Wasm bundle, JSON intents, and SSE.

Authentication and request checks are in [`meerai-web/src/auth.rs`](../meerai-web/src/auth.rs):

- HTTP Basic with a constant-time secret compare (`subtle`).
- A secret must have at least 8 characters and no control characters.
- `Host` must be `127.0.0.1:<port>` or `localhost:<port>`.
- State-changing requests must use `POST`, `PUT`, or `DELETE`, send `Content-Type: application/json`, and carry an `Origin` equal to the served origin.
- Rejected requests are logged by reason, never with the secret.
- Failed authentication backs off from 50 ms to 1 s.
- When auth is enabled, every route requires Basic, including the snapshot, the SSE stream, and the prompt route. When auth is disabled, the snapshot, SSE stream, session creation, and prompt stay open, and privileged routes return `401`. Both launch surfaces always construct `ServeAuth::Basic`; `--user` and `MEERAI_SERVE_USER` default to `meerai`.

The static handler rejects traversal and serves `.wasm` as `application/wasm`. The CSP is `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self'`.

### 2.4 Wire protocol

`meerai-web-protocol` is serde-only and builds for `wasm32-unknown-unknown` ([`meerai-web-protocol/src/lib.rs`](../meerai-web-protocol/src/lib.rs)). It defines the snapshot, view events, intents, and DTOs for sessions, turns, approvals, questions, workspace, providers, settings, and ACP. The server maps `AgentEvent` and host results into this vocabulary. The client never receives provider secrets, raw session files, or direct tool invocation.

The protocol crate exists because `AgentEvent` lives in `meerai-agents`, which depends on `genai`, `tokio`, and `tokio-util`. Its ID and request types come from `meerai-session`, and `FileDiff` comes from `meerai-core`. Those crates are not Wasm clients.

### 2.5 Leptos client

`meerai-web-client` is a Leptos 0.8 CSR application mounted into `#app` by `start()` ([`meerai-web-client/src/lib.rs`](../meerai-web-client/src/lib.rs)). It uses `leptos`, `wasm-bindgen`, `web-sys`, `js-sys`, `comrak`, and the wire protocol. It does not depend on `meerai-agents`, `meerai-session`, `meerai-tools`, `meerai-providers`, or `meerai-syntax`.

| Module | Role |
| --- | --- |
| `store.rs` | `StoreHandle`: an `RwSignal<AppState>` plus an effect queue. `UiEffect` covers copy, focus, scroll, title, and external URL. |
| `state.rs` | Pure reducers: snapshot, view events, tabs, queue, slash commands, mentions, settings form, transcript grouping, history. |
| `text.rs` | `TextBuffer` replaces the Rune `TextEdit`; no renderer type remains in state. |
| `transport.rs`, `transport/wasm.rs` | `fetch` for intents and session detail hydration, `EventSource` with `requestAnimationFrame` delta coalescing, snapshot-first attach, resync on an expired cursor, and reconnect. |
| `markdown.rs` | `comrak` AST to Leptos views with the same feature set as the TUI: tables with alignment, task lists, footnotes, description lists, GitHub alerts, math, and front matter. Soft breaks render as line breaks. HTML blocks and inline HTML render as text. Links are scheme-checked. Images render as links. |
| `highlight.rs` | Lexical highlighting returns semantic spans; CSS owns colour. |
| `theme.rs` | Validates `ThemeTokensDto` (hex and CSS named colors), computes `color-scheme` and contrast foregrounds, and manages browser-only `BrowserTheme` (`Host`, `System`, `Light`, `Dark`). |
| `keymap.rs` | Normalizes `KeyboardEvent.code` to app actions, handles modal navigation and `Tab` focus trapping, and keeps text-field handling separate. |
| `views/` | `App`, header, tabs, workspace inspector sidebar, transcript, composer, modals, and Settings. |

The client renders:

- Snapshot-first state, SSE cursor resume, resync, a manual Reconnect control, and dismissible notice/error banners.
- Draggable tabs with `+` session creation, status dots, `ACP` tags, unread markers, a workspace inspector sidebar (MCP servers, tools, cost/metrics, a Diagnose block shown by default, collapsible changed files, subagents), a mobile drawer, and a numbered turn rail with a jump-to-latest pill.
- Collapsed activity digests per contiguous activity run (with thoughts, tools, and subagents in chronological order, including reasoning preceding a visible reply), using the TUI category counts, gerund phrasing, and structured tool summaries.
- Markdown, highlighted code blocks, per-reply and tool-output copy buttons, and inline unified edit diffs.
- Streaming assistant text and reasoning, system lines, errors, subagent rows, and question results.
- Header model/mode pills, `ACP:<preset>` and update status badges, and a pending request reopen pill.
- FIFO composer queue, steering (queued items or the active draft), cancel, compact, reset, and resolved `@agent`/`@skill` definition chips in the composer and in directly submitted user messages. The built-in `goal` and `orchestrate` skills also accept `@goal` and `@orchestrate`. Submitting strips known mention tokens from the prompt sent to the model and keeps the chip in the user message; replay shows the stripped prompt.
- Approval decisions for once, turn, session, decline, decline with reason, and cancel.
- Question answers with single or multiple choice, a multiline free-text field, Back/Next, keyboard navigation, submit, and cancel.
- Session picker, history, fork, rewind, delete, workspace diff, debounced search with highlighted preview lines, and workspace inspector actions.
- User shell through `!command`, with a scrollable shell dialog, concurrent runs, and transcript result. Running shells and subagents appear as clickable `N agents` and `N shells` chips under the composer; a chip opens a `Runs` list, and a row opens the agent conversation or the shell detail. Opening the agent conversation does not expand the transcript card. Rows and cards show an optional run name with its preset. The agent conversation is a full-height overlay that renders the child transcript with the same transcript components as the chat, and its text is selectable with native browser selection. `Esc` closes the agent overlay to the chat and returns from a shell detail to the list.
- Provider and model pickers (cross-provider or scoped by provider, recent and custom models, active/configured sorting, type-to-filter with prefix matches ranked first, `Tab` completion of the highlighted model, removal of recent or misspelled model ids via the `Forget` badge), API key and base URL entry (including independent Base URL edits for keyless or already-configured providers), clickable ChatGPT sign-in, removal of stored credentials, and Settings (`Providers & Models`, `Current session`, grouped `Context & Limits` with model spec suggestions and a reasoning-effort tier selector (`low` 0.00, `medium` 0.20, `high` 0.50), `Theme` with color swatches, `Retention`, `Sandbox`, `Permissions`) with `config_writable` guards.
- Command palette, slash commands, `@` mentions, completion, Help, an auto-refreshing subagent overlay, and the running-work panels.

The interface is flat and DOM-native: one surface scale, 1px borders, a single teal accent, rounded cards, no gradients, and a shared transcript and composer column capped at 880 px alongside the left Sessions panel (`Sessions`) and the right inspector sidebar (toggled from the far-right utility cluster). Every dialog uses the same shell: a title band with a close control, an optional footer action row, `pick-row` lists for selectable rows, and `Esc` closes the dialog or returns to the previous panel. Titles use sentence case; TUI slash-command hints are omitted because the same dialog opens from the palette, the sidebar, and a shortcut. Transcript bubbles, error/system banners, markdown blocks, and subagent cards wrap long content inside the conversation column; subagent entries keep both their Markdown `Report` and child `Messages` collapsible inside `<details>` blocks, the child `Messages` body uses the same transcript renderer as the chat, and optimistic in-flight prompts render at the bottom of the transcript until committed, while queued prompts render only in the composer queue strip as stacked rows with `Steer`, `Edit`, and `Remove` actions and one shared strip background. Hover and focus states are visible, dialogs trap `Tab` focus and restore focus to the opener on close, and the composer is a focused input card with an inline pending-request reopen bar when needed, anchored completion, an embedded stateful send/queue/stop action, working status animations, and hint keys below the card. The empty state offers three starter prompts that fill the composer. Light, dark, or custom colors come from the browser color scheme selector (`Host theme`, `System`, `Light`, `Dark`) or the host theme tokens; the client validates host tokens, sets `color-scheme` and contrast foregrounds, and writes CSS custom properties.

The base stylesheet is [`meerai-web-client/styles/app.css`](../meerai-web-client/styles/app.css). Theme values become `--m-*` custom properties on the app shell. The layout uses CSS media queries instead of a manual breakpoint constant, and `env(safe-area-inset-*)` for insets.

### 2.6 Launch and build

`meerai serve` and TUI `/serve` bind to `127.0.0.1:28860` unless `--port` or a port in `ServeDialog` is specified. When unspecified, Basic auth defaults to username `meerai` and password `12345678`, overridden by `--password`, `MEERAI_SERVE_SECRET`, `--auth` prompt, or the masked field in `ServeDialog`.

`scripts/generate_favicon.py` generates the SVG brand logo and browser favicons (`logo.svg`, `favicon.svg`, `favicon.ico`, PNG icons, and `apple-touch-icon.png`) under `meerai-web-client/assets/`. `scripts/build-web.py` builds `meerai-web-client` for `wasm32-unknown-unknown`, runs the workspace `meerai-web-bindgen` helper (`wasm-bindgen-cli-support`), optionally runs `wasm-opt -Oz`, copies `styles/app.css` to `styles.css` and copies the SVG/favicon assets, writes `index.html` and `boot.js`, and reports the Wasm size. It has no Rune font step and no WebGPU check. `meerai-web/build.rs` embeds every file under `target/web` (or `MEERAI_WEB_DIR`) into the binary.

`index.html` contains `#app`, the loading overlay, favicon `<link>` tags, a `styles.css` link, and a module `boot.js` that calls the Wasm `init()`. If initialization fails, the overlay names the error and points at the TUI.

### 2.7 Worker and item protocol

`spawn_worker` returns one `AgentCommand` sender and one `AgentEvent` receiver ([`meerai-agents/src/multi_turn_agent/worker.rs`](../meerai-agents/src/multi_turn_agent/worker.rs)). The registry is the only consumer and fans events out to clients.

Two constraints shape the browser API:

- An `Approve` for a request ID that is not pending is silently ignored by the worker. The registry rejects stale decisions before forwarding.
- A second `SubmitPrompt` while a turn runs is not supported. The registry rejects it as busy; the client queues the prompt locally and submits it when the turn ends, or steers it explicitly.

### 2.8 Replay and live state

The replay projection reconstructs transcript entries from logged events ([`meerai-session/src/replay.rs`](../meerai-session/src/replay.rs)). `Session::load_events` is the read-only history path. Neither includes in-flight deltas or pending requests: deltas are not logged. The registry folds live events and serves a snapshot plus a stream. The TUI folds the same events for its own view.

### 2.9 Implemented browser parity

| TUI workflow | Browser status |
| --- | --- |
| Layout, theme, transcript | Implemented: responsive shell, theme tokens, sidebar, turn rail, activity digests, Markdown, highlighted code. |
| Keyboard, navigation, copy | Implemented for the shortcuts in §4.3. Browser find, selection, zoom, and copy work on the DOM. |
| Composer and active turns | Implemented: multiline editing, IME, queue, steer, cancel, compact, reset. Queued prompts show `Steer`, `Edit`, and `Remove`; `Edit` opens the narrow queued-edit dialog and saves the new text in place. |
| Slash commands and palette | Implemented for the dispatcher's current command set plus `/settings` for in-browser configuration. `/effort` with no argument and `/sampling` open Settings → Context, where effort and temperature are edited together; the effort choices follow the active model's catalog levels and `/effort xhigh` or `/effort max` clamps to the nearest accepted tier. `/diagnose` toggles the inspector's Diagnose block. `/btw` opens a transient, read-only tab seeded from the active session; its summary has `transient: true`, so it stays out of the Sessions panel and never persists. Unknown commands stay errors, never prompts. |
| Mentions and definitions | Implemented: completion, apply, and resolved agent/skill chips with remove controls; the raw text stays in the textarea. The built-in `goal` and `orchestrate` skills accept `@goal` and `@orchestrate` as aliases for `@skill/goal` and `@skill/orchestrate`. |
| Approvals | Implemented: once, turn, session, decline, reason, cancel. A resolved request closes in every client. |
| Questions | Implemented: single and multiple choice, a multiline `Other` textarea, Back/Next, keyboard selection, submit, cancel. |
| Message actions, rewind, fork | Implemented: copy, rewind (`Both`, `Code`, or `Conversation`), fork before a message, and the draft restore on rewind. |
| Replay | Implemented through the shared replay projection. Historical items are not marked live. |
| Tabs and sessions | Implemented: open, resume, HTML5 drag or shortcut reorder, `+` create, detach, fork, confirmed delete. The left Sessions sidebar lists live and stored sessions; row actions appear on hover or keyboard focus. Tab state stays local. Provider, model, and approval mode are per session: a model or mode change applies to the active session and is stored in its row, and reopening the session restores those values. |
| Workspace review and search | Implemented: changed files with per-file Stage and Unstage actions, unified diff with a split toggle, debounced file/content search, highlighted preview. |
| User shell | Implemented through `SessionOpener::run_user_shell`; concurrent runs are tracked by id, the dialog and transcript receive the output, and the composer chips open the run list. |
| Status and usage | Implemented: workspace, branch, connection, mode, model, effective temperature with its tier (`temp 0.20 · medium`), live turn timer, active tool, cost, throughput, update status badge, and clickable `N agents` / `N shells` counters for running work. The composer hint keeps cost and throughput only; token counters live in the inspector. The inspector's Diagnose block shows last-request tokens, last-request cache hit ratio, last-request cache write tokens, context use, memory tokens, and tool schema tokens, and is shown by default. The Changed files block is also shown by default and has a Show/Hide control. It does not show a cumulative cache ratio: the percentage is always the last request, and a fork or a resumed stored session shows `n/a` until a new request reports usage. Browser notifications are not shown yet. |
| Reload | Implemented through `/reload`; it reloads config and reports warnings. |
| Context, loop, memory, routing, packs, subagents | Implemented as one tabbed Control panel: `/context`, `/loop`, `/memory`, `/routing`, `/packs`, `/definitions`, and `/subagent` open the matching tab. The Context tab shows the token breakdown, projection toggles, turn pin/unpin, summarize-from-here, the summary-focus input, and a blob viewer. The Loop tab shows the objective, done condition, constraints, phase, items with action and observation, and round and turn counts; use `@goal <instruction>` for the goal preset or `@orchestrate <instruction>` for the multi-area preset. The Subagent tab exposes name (one word, optional), agent, task, model, mode, steps, tokens, cost, and tool subset. The Packs tab installs, pins, enables, trusts, and removes packs and lists definition diagnostics with a `model`/`manual` marker for skills. The panel fetches `GET /api/memories`, `/api/routes`, `/api/packs`, and `/api/definitions/diagnostics`; mutations and spawns call `POST /api/actions`, which runs the same `meerai-host` executor as the TUI. Context and loop state arrive through `ContextUpdated` and `LoopUpdated`; blobs use `GET /api/sessions/{id}/blobs/{key}`. `/vault-reset` opens a confirmation panel that calls the shared reset action. |
| Providers and sign-in | Implemented: provider status, model choice, API key, ChatGPT sign-in, paste, cancel, removal. |
| ACP | Implemented: `/acp` creates an ACP session; tabs and header show `ACP` badges with the external-engine warning banner. |
| Reconnect | Implemented: snapshot, cursor, resync, manual Reconnect action, and no automatic replay of unknown outcomes. |

## 3. Planned

The remaining work is automation, bundle gating, and manual accessibility sign-off, not a renderer migration.

| Item | Description | Gate |
| --- | --- | --- |
| Browser notifications | Optional notifications for turn completion when the tab is hidden. | Permission and privacy review |
| Search request ids | Add explicit client sequence tokens on preview/search requests beyond query/mode matching. | Workspace search tests |
| Accessibility audit | Run VoiceOver and NVDA across nested dialogs and document screen-reader verification. | Audit record |
| Browser automation | Add a headless browser suite for mount, snapshot, prompt, approval, and settings flows. | CI browser runner |
| Bundle budget | The release bundle is about 1.9 MB raw and 547 KB gzip. Enforce a budget in CI. | Release check |
| CI bundle build | Build the Wasm bundle in CI when `web` is exercised, not only the Wasm check. | CI time budget |
| Transcript click-to-open | The TUI opens a clicked transcript path through `HostAction::OpenInEditor`. The browser has the action (`/open`, `POST /api/actions`) but no click handler on rendered paths. | Parity with `docs/tui-design.md` §2.3 |

## 4. Reference

### 4.1 HTTP and SSE routes

| Method | Path | Purpose | Auth rule |
| --- | --- | --- | --- |
| GET | `/api/snapshot` | Registry and active-session snapshot with `event_id` | Basic when enabled; open only if auth is disabled |
| GET | `/api/events` | SSE stream; `?after=` or `Last-Event-ID` | Basic when enabled; open only if auth is disabled |
| POST | `/api/sessions` | Create a native or ACP session | Basic when enabled; open only if auth is disabled |
| POST | `/api/sessions/{id}/prompt` | Submit a prompt | Basic when enabled; open only if auth is disabled |
| GET | `/api/sessions/history` | Stored session list | Basic; 401 if auth is disabled |
| GET | `/api/sessions/{id}` | Session detail | Basic; 401 if auth is disabled |
| GET | `/api/sessions/{id}/events` | Session event history | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/open` | Open a stored session | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/activate` | Make a session active | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/close` | Close the worker | Basic; 401 if auth is disabled |
| DELETE | `/api/sessions/{id}` | Delete stored history | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/fork` | Fork a session | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/btw` | Open a transient, read-only side session seeded from this one. Body: `{ "text": "optional first question" }`. The summary has `transient: true`. | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/steer` | Steer the running turn | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/cancel` | Cancel the turn | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/compact` | Compact context | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/reset` | Reset conversation context | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/rewind` | Rewind code, conversation, or both | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/model` | Set the model | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/mode` | Set the approval mode | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/apply-agent` | Apply an agent definition | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/apply-skill` | Apply a skill definition | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/shell` | Run a user shell command. Body: `{ "command": "...", "runId": "optional" }`. The summary's `shellRuns` lists the runs that are still active. | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/approvals/{request}` | Decide an approval | Basic; 401 if auth is disabled |
| POST | `/api/sessions/{id}/questions/{request}` | Answer a question | Basic; 401 if auth is disabled |
| GET | `/api/workspace/files` | List workspace files | Basic; 401 if auth is disabled |
| GET | `/api/workspace/search` | Search names or content | Basic; 401 if auth is disabled |
| GET | `/api/workspace/preview` | Preview a file | Basic; 401 if auth is disabled |
| GET | `/api/workspace/changes` | List working-tree changes | Basic; 401 if auth is disabled |
| GET | `/api/workspace/diff` | Working-tree diff | Basic; 401 if auth is disabled |
| POST | `/api/workspace/stage` | Stage or unstage one path (`path`, `stage`) | Basic; 401 if auth is disabled |
| GET | `/api/providers` | Provider catalog and credential status | Basic; 401 if auth is disabled |
| POST | `/api/providers/{id}/api-key` | Store an API key | Basic; 401 if auth is disabled |
| DELETE | `/api/providers/{id}/credential` | Remove a stored credential | Basic; 401 if auth is disabled |
| POST | `/api/providers/{id}/base-url` | Set or clear a base URL | Basic; 401 if auth is disabled |
| POST | `/api/providers/{id}/chatgpt/start` | Start a ChatGPT sign-in | Basic; 401 if auth is disabled |
| POST | `/api/providers/{id}/chatgpt/{attempt}/paste` | Paste a redirect URL | Basic; 401 if auth is disabled |
| POST | `/api/providers/{id}/chatgpt/{attempt}/cancel` | Cancel a sign-in attempt | Basic; 401 if auth is disabled |
| POST | `/api/actions` | Run a host action such as `/memory`, `/routing`, `/packs`, `/context`, `/pin-turn`, `/context-policy`, `/summarize`, `/subagent`, or `/open` | Basic; 401 if auth is disabled |
| GET | `/api/memories` | List or search project memories (`?query=`) | Basic; 401 if auth is disabled |
| GET | `/api/routes` | List task model routes | Basic; 401 if auth is disabled |
| GET | `/api/packs` | List installed packs | Basic; 401 if auth is disabled |
| GET | `/api/definitions/diagnostics` | Agent and skill diagnostics | Basic; 401 if auth is disabled |
| GET | `/api/sessions/{id}/context` | Latest context breakdown | Basic; 401 if auth is disabled |
| GET | `/api/sessions/{id}/blobs/{key}` | Read a spilled tool-output blob | Basic; 401 if auth is disabled |
| POST | `/api/settings` | Update settings | Basic; 401 if auth is disabled |
| POST | `/api/reload` | Reload configuration | Basic; 401 if auth is disabled |

The launch surfaces always configure Basic auth. The four open rows are a library-state caveat, not a supported launch mode.

### 4.2 Approval decisions

| Control | Key | Decision | Grant |
| --- | --- | --- | --- |
| Approve once | `y` | `Accept` | none |
| Allow turn | `t` | `Accept` with `Turn` | Turn grant, cleared at turn end |
| Allow session | `a` | `AcceptForSession` | Session grant |
| Decline | `n` | `Decline` | none |
| With reason | `r` | `DeclineWithReason` | none |
| Cancel request | `c` | `Cancel` | none |

`Esc` leaves reason editing back to the action row without a decision; closing the dialog with `Esc` leaves the request pending so it can be reopened from the pending pill (`c` cancels the request). Any attached client can resolve it; the first valid decision wins and later decisions are rejected as stale.

### 4.3 Browser shortcuts

| Action | Shortcut | Notes |
| --- | --- | --- |
| Command palette | `Alt+K` | Does not use `Ctrl+P` (print); `Ctrl+K` is reserved by some browsers. |
| Focus composer | `Alt+A` | Does not use `Ctrl+L` (address bar). |
| Next / previous tab | `Alt+]` / `Alt+[` | Does not use `Alt+Left` / `Alt+Right` (history). |
| Select tab 1–9 | `Alt+1` … `Alt+9` | Mirrors the TUI. |
| Move tab left / right | `Alt+Shift+[` / `Alt+Shift+]` | Drag also works. |
| Close tab view | `Alt+W` | Detaches the view; it does not close the worker. Guard against text focus. |
| Toggle sidebar | `Ctrl+B` | Safe because the app has no `contenteditable` region. |
| Cycle approval mode | `Alt+M` | `plan` → `ask` → `auto`. |
| Sessions sidebar | `Alt+S` | Reveals and focuses the left Sessions panel, the same surface as `/sessions`. |
| Model picker | `Alt+L` | Opens the same modal as `/models`. |
| Workspace diff | `Alt+Shift+D` | Opens the same view as `/diff`. |
| Workspace search | `Alt+Shift+F` | Opens the same view as `/search`. |
| Steer queued prompt or draft | `Alt+G` | Steers the oldest queued prompt, or the active draft when the queue is empty; does not use `Ctrl+G`. |
| Cancel turn | `Alt+X` | Also available as a visible button. |
| Copy latest answer | `Alt+C` | Requires a user gesture for the Clipboard API. |
| Help | `?` | Only when focus is not in a text field. |
| Approval decisions | `y` `t` `a` `n` `r` `c` | Only inside the approval dialog, and not while the reason editor has focus. |
| Close dialog | `Esc` | Except the approval dialog while reason editing is active. |

`Alt` means `Option` on macOS. Handlers do not run when focus is in a text field, except approval and question decisions. Every shortcut also exists as a visible control.

### 4.4 Theme tokens

`ThemeTokensDto` maps one-to-one to CSS custom properties:

| DTO field | CSS variable |
| --- | --- |
| `background` | `--m-bg` |
| `surface` | `--m-surface` |
| `surface_high` | `--m-surface-high` |
| `text` | `--m-text` |
| `muted` | `--m-muted` |
| `faint` | `--m-faint` |
| `accent` | `--m-accent` |
| `interactive` | `--m-interactive` |
| `border` | `--m-border` |
| `error` | `--m-error` |
| `warning` | `--m-warning` |
| `success` | `--m-success` |
| `info` | `--m-info` |
| `diff_added` | `--m-diff-added` |
| `diff_removed` | `--m-diff-removed` |
| `diff_context` | `--m-diff-context` |
| `code_function` | `--m-code-function` |

The host sends effective theme strings. The client accepts `#rgb`, `#rrggbb`, or a CSS color name and otherwise keeps the default. Theme values are written as custom properties, never as arbitrary CSS. When `Host theme` is selected, `UiTheme::resolve` also computes `--m-on-accent` and `--m-on-warning` contrast foregrounds from `accent` and `warning`. Selecting `Host theme`, `System`, `Light`, or `Dark` updates `meerai.browser-theme` in `localStorage` and persists `theme.color_scheme` in user `settings.yml` when writable so restarts keep the chosen mode.

### 4.5 Build commands

```sh
cargo check -p meerai-web-client --target wasm32-unknown-unknown
cargo test -p meerai-web-client
python3 scripts/build-web.py --dev
python3 scripts/build-web.py
cargo run -p meerai-cli --features web -- serve
make build-web
make run-web
make serve
make install
```

`MEERAI_WEB_DIR` points the server at an unembedded bundle during development. The release build embeds `target/web`, and every release archive and `make install` binary ships with `--features web`.

## 5. Invariants

- The host is the source of truth for workers, turns, history, policy, grants, and pending requests. The browser never invokes a tool directly.
- State reducers carry no DOM types. UI actions become explicit `UiEffect` values.
- Untrusted content is text. No `inner_html`, no raw Markdown HTML, no remote image load.
- Transcript text stays in the DOM. Do not virtualize it or move it into a canvas.
- A disconnect, reload, modal close, or focus change never decides a request or cancels a shared turn.
- The first valid decision for a pending request wins. Later decisions are stale.
- A direct prompt submit while a turn runs is rejected by the host. Queueing is a client behavior.
- Dialogs trap focus and restore it to the opener.
- Browser storage holds no transcript, draft, queue, or secret.
- The server stays loopback-only with mandatory Basic auth on both launch surfaces.
- `plan` mode stays hard read-only through every browser and TUI surface.

## 6. Open questions

| # | Question | What settles it |
| --- | --- | --- |
| 1 | Does per-frame delta coalescing materially improve long-turn typing latency? | Measure a 10k-token fixture turn. |
| 2 | Does `content-visibility: auto` preserve find-in-page and screen-reader order? | Test in Chromium, Firefox, and Safari. |
| 3 | Which browser and OS combinations are release blockers, and which are best effort? | A written matrix and smoke results. |
| 4 | Should the Settings panel expose raw theme values or only reviewed presets? | Security review of the CSS value grammar. |
| 5 | Can CI run a real browser without a long setup step? | A headless browser spike. |
| 6 | Should the current 547 KB gzip bundle become a hard CI gate? | Add the gate to CI. |