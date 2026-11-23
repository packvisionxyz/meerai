# TUI Design: Selection-First Modal Agent Harness

This document specifies the interactive terminal UI (`meerai-tui`): what is
implemented, how it behaves, and what is planned.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-19 |
| **Audience** | TUI users and `meerai-tui` contributors |
| **Scope** | `meerai-tui` (layout, modes, keybindings, overlays, rendering); host seams in `meerai-cli` |
| **Related** | [browser-ui.md](browser-ui.md) · [harness-spec.md](harness-spec.md) · [architecture.md](architecture.md) · [harness-internals.md](harness-internals.md) · [acp.md](acp.md) |

## 1. Overview

The TUI is a modal, selection-first terminal interface for inspecting
multi-step tool execution, reviewing diffs, and steering long coding
sessions. Typing a prompt never collides with scrolling history or expanding
tool output: the composer has an Insert mode and the transcript a Normal
mode. Actions are discoverable through slash commands, a command palette,
and completion popups. File listing and content search go through the
`WorkspaceQuery` trait so clients can reuse host workspace services.

This document defines terminal behavior. The TUI and the [local browser companion](browser-ui.md)
must maintain two-way feature parity across user-facing workflows, slash
commands, approvals, questions, session actions, and transcript items through
`meerai-host` and `meerai-web-protocol`, without copying terminal `Normal`/`Insert`
modes or escape sequences. Client-specific conveniences (`/settings` form and
browser `Light`/`Dark` scheme in the browser; `settings.yml` editing in the
TUI) stay documented in each client's spec.

## 2. Implemented

### 2.1 Screen layout

The screen has four surfaces: the transcript, an optional sidebar, the
composer, and a one-row statusline.

```text
┌─────────────────────────────────────────────────┬──────────────────────────┐
│ Conversation & tool stream                      │ Sidebar (ctrl+b)         │
│                                                 │  session / provider /    │
│ > You                                           │  model / auth / dir      │
│   Refactor the agent loop                       │  MCP / agents / tools    │
│ ▸ 1 command · 2 reads · 1 edit                  │  ─────────────────────── │
│ ◆ Assistant                                     │  DIAGNOSE [hide]         │
│                                                 │  ─────────────────────── │
│                                                 │  File changes [hide]     │
│                                                 │  src/loop.rs  +18 -9     │
├─────────────────────────────────────────────────┴──────────────────────────┤
│ Composer (INS) — @ mentions, / commands                                    │
│ INS · ASK · model · provider · temp 0.70                                   │
├────────────────────────────────────────────────────────────────────────────┤
│ ~/code/repo  main  42 tok/s  ⠋ Ran code-shell                              │
└────────────────────────────────────────────────────────────────────────────┘
```

- The sidebar hides below `MIN_SIDEBAR_WIDTH` and returns on resize.
  `ctrl+b` toggles it. It has three panels on `surface`: the session
  overview (`session`, `provider`, `model`, `auth`, `dir` with `~`
  shortening, then MCP servers, a `main` agent row, subagent rows, and
  tools), the `DIAGNOSE` panel, and the file-changes panel. The `AGENTS`
  block lists the `main` status and up to four subagents from the
  transcript, running ones first, each with its status badge and step
  count. The block is display-only: rows have no click action. Use the
  statusline `N agents` counter to inspect a running agent. Extra
  subagents collapse into `… N more`.
  A live `subagent-run` or `acp-run` tool item and its `SubagentRun` item
  render as one row: the tool item is replaced when the subagent item
  starts, so delegation never appears in the `TOOLS` list.
- Each panel has its own height, so expanding or collapsing `DIAGNOSE` or
  File Changes never pushes the session block out. The overview keeps its
  11-content-row floor when the sidebar is tall enough. On a short terminal the
  file-changes panel gives up height first.
- `DIAGNOSE` is shown by default and takes nine rows. `/diagnose` or a click
  on the header collapses it to three rows (header only). The rows are
  last-request tokens (`↑prompt ↓completion`), last-request cache hit ratio
  (`cached_tokens / prompt_tokens` from the provider, not an estimate), last-request cache
  write tokens, context use, memory tokens, and tool schema tokens. A provider
  that reports no cache details shows `n/a`. The block does not show a
  cumulative cache ratio: the percentage is always the last request, and a
  fork or a resumed stored session shows `n/a` until a new request reports
  usage.
- File Changes is shown by default. A click on its header, or the
  `[hide]`/`[show]` action, collapses it to three rows (header only). The
  collapsed header keeps the changed-file count. Hiding either panel gives
  its rows to the overview.
- The file list pages with a clickable `… and N more` footer (next page,
  back to top at the end) and scrolls with the mouse wheel. Clicking a
  changed file opens the diff modal.
- A numbered anchor rail sits between the transcript and the sidebar, and at
  the right edge when the sidebar is hidden. It appears when the transcript
  scrolls and the tab has at least two user turns. Each user message has one
  row, labelled with its turn number (1-based; the column widens for two
  digits). The turn nearest the viewport top is accented. When not every turn
  fits, a `⋮` row marks the hidden range. A click jumps to that turn, a click
  on `⋮` jumps to the hidden end, and dragging across numbers scrolls. Hovering
  a number paints that turn's band.
- The transcript header is one line (`meerai provider/model [auth]`); an
  ACP tab shows `agy (acp)` plus the external-engine warning instead.
- The palette follows the opencode dark theme and is overridable through the
  `theme` block in user and project settings
  (`meerai-tui/src/theme.rs`). Values accept `#rgb`, `#rrggbb`, or terminal
  color names; invalid values warn and keep the default.

### 2.2 Global shortcuts

Active in any mode unless a modal is open.

| Key | Action |
| --- | --- |
| `ctrl+p` / `ctrl+shift+p` | Command palette |
| `shift+tab` | Cycle approval mode: `plan` → `ask` → `auto` |
| `ctrl+n` | New session tab (reuses an open empty tab) |
| `ctrl+s` | Session picker |
| `ctrl+w` | Close tab |
| `alt+shift+left` / `alt+shift+right` | Move the active tab |
| `alt+1..9`, `alt+left/right`, `ctrl+PageUp/Down` | Switch tabs |
| `ctrl+o` | Model picker |
| `ctrl+a` | Provider connect |
| `ctrl+f` | Search files |
| `ctrl+shift+f` | Search file contents |
| `ctrl+e` | Review changed files |
| `ctrl+b` | Toggle sidebar |
| `ctrl+k` | Compact context |
| `ctrl+shift+r` | Reset conversation |
| `ctrl+q` | Quit |
| `f1` | Help |
| `ctrl+c` | Cancel the running turn; press twice (2 s window) to exit |

Tabs also reorder by mouse: press switches immediately, dragging more than
two columns arms the drag, and release drops the tab on the target. The
order persists in the `user_config` table (`ui_tabs`).

### 2.3 Normal mode (`NOR`)

| Key | Action |
| --- | --- |
| `i` / `a` | Focus the composer, enter Insert mode |
| `j` / `k` / `Down` / `Up` | Scroll the transcript |
| `ctrl+d` / `ctrl+u` | Half-page down / up |
| `gg` / `G` | Top / bottom |
| `y` | Yank the latest assistant (or error/system) text |
| `/` or `ctrl+f` | Workspace file/content search |
| `Esc` | Cancel the running turn |
| `q` | Quit |

Clipboard writes use OSC 52 plus a native helper (`pbcopy`, `clip`,
`wl-copy`, `xclip`, `xsel`) when available. Dragging over the transcript
selects text and copies it on release. The selection band is white with black
text. There is no keyboard Select mode.

Clicking a file path in the transcript opens it in the editor, at the line
number when the text carries one (`click_path_at`, `path_reference_at`,
`app/input.rs`). The resolver maps the clicked cell to a wrapped transcript
line through `scroll_offset`, skips the gutter, expands to the surrounding
token, and accepts it only when `meerai_syntax::detect_path_reference` returns
a reference and the joined path is an existing file under the workspace root.
The click queues `HostAction::OpenInEditor`, so the `/open` guard runs before
any process starts. Absolute paths and `..` never resolve. The editor is
`MEERAI_EDITOR`, then `VISUAL`, then `EDITOR`, spawned without a shell; plan
mode refuses. Text selection still wins: a drag is not a click, so selection
and click-to-open do not conflict.

### 2.4 Insert mode (`INS`) and the composer

- A multi-line buffer with cursor motions, backspace/delete, and bracketed
  paste. Long lines soft-wrap at the input width; the composer grows to
  8 rows and scrolls beyond that (`wrap_composer`, `ui/composer.rs`).
  `Up`/`Down` move across wrapped display rows, not only explicit
  newlines, and keep the goal column when a shorter row clamps the cursor
  (`composer_vertical_move`).
- `Enter` submits. `shift+enter`, `alt+enter`, `ctrl+enter`, and `ctrl+j`
  insert a newline; a trailing `\` continues a line.
- While a turn runs, `Enter` queues the message (`Queued N message(s) ·
  ctrl+g sends now`); queued messages send in order when the turn ends.
  `ctrl+g` steers the oldest queued message (or the current draft) at the
  next step boundary. Queue state is display-only and never enters the
  session log.
- `@` opens the Mentions popup (§2.6). `/` on an empty line opens
  slash-command completion; `/model ` opens model completion. `Tab` accepts
  a completion; `Esc` dismisses completion, then returns to `NOR` without
  discarding the draft.
- Composer clicks move the cursor and start a drag selection. Drag paints
  the selection as white with black text, and
  the release copies it, like the transcript. The drag target is clamped to
  the composer, so moving the pointer past the box still selects to the line
  end or the last visible row. Backspace/Delete or typing deletes the
  selection.
- A double-click within 400 ms on the same cell selects the word under the
  cursor and copies it on release (`register_click`,
  `composer_word_bounds_at`).
- The queued band sits directly above the composer, spans the full composer
  width, and shares one background across every row. Each message wraps to at
  most two rows, oldest first, and the band caps at six rows with a
  `… N more queued` tail. Every message keeps `edit` and `✕` hit zones on its
  first row, right-aligned (`queued_band_rows`, `render_queued_band`,
  `ui/composer.rs`). The band reserves its own layout row through
  `queued_band_rows`, so the composer never covers it. `✕` drops the message;
  `edit` opens the queued-edit dialog, a wrapped multi-line editor with a
  visible cursor: arrows move it across wrapped rows (with a goal column),
  Home and End move it, Backspace and Delete edit around it,
  `alt+enter` inserts a newline, and `Enter` commits the new text in place.
- The composer footer shows the input-mode badge (`INS`/`NOR`), approval
  mode badge, model, provider, and the effective temperature with its tier
  (`temp 0.20 · medium`), driven by `context.effort` and
  `context.temperature`. An ACP tab shows `agy (acp)` in place of
  model/provider.
- `!` at the start of a line runs a user shell command (§2.14).

### 2.5 Slash commands

Typing `/` opens completion. The implemented handler set
(`meerai-tui/src/app/commands.rs`):

| Command | Action |
| --- | --- |
| `/help` | Commands and shortcuts |
| `/auth` | Credential sources |
| `/connect`, `/provider` | Configure or switch provider |
| `/models`, `/model [name]` | Pick or set the model |
| `/effort [low\|medium\|high]` | Set the reasoning tier, or open the sampling panel with no argument |
| `/sampling` | Sampling panel: reasoning effort and temperature override together |
| `/temperature` | Sampling panel focused on the temperature override. `l`/`m`/`h` and Left/Right set the tier from either field; the tier row is clickable. Type `default` to clear the override. |
| `/agent <id>`, `/skill <id>` | Apply a markdown definition |
| `/agents` | Mentions popup filtered to agents |
| `/skills` | Definitions panel filtered to skills |
| `/mode [plan\|ask\|auto]` | Show or set the approval mode |
| `/compact` | Request context compaction |
| `/context` | Context panel: token buckets, watermark, projection toggles, turn pin/unpin, summarize-from-here, blob viewer, scroll, refresh |
| `/goal` | Goal panel: goal, done condition, constraints, items with evidence, round and turn counts. Read-only; `j`/`k`/PageUp/PageDown scroll, `Esc` closes. Use `@goal <instruction>` to start one |
| `/memory [query]` | Memory panel: list, search, pin, delete |
| `/remember <note>` | Save a project memory |
| `/forget <id>`, `/pin <id> [off]` | Delete or pin a project memory |
| `/routing [set <task> <provider::model> ...]` | Routing panel or a route change |
| `/packs`, `/pack <verb> <id> [rev]` | Packs and definitions panel, or a pack change |
| `/definitions` | Definitions tab: agents, skills, token estimates, warnings, and a `model`/`manual` marker for skills |
| `/subagent` | Spawn form: agent, task, model, mode, steps, tokens, cost, tool subset |
| `/subagent <agent> <task>`, `/subagent cancel <item>` | Direct spawn or cancel |
| `/vault-reset` | Confirmation dialog; typing `reset` deletes all saved credentials and reinitializes the vault |
| `/open <path>[:line]` | Open a workspace file in `$EDITOR` at the given line. Absolute paths and `..` are rejected |
| `/search [query]` (aliases `/tv`, `/find`), `/grep [query]` | File/content search |
| `/diff [path]` | Diff review |
| `/sidebar` | Toggle the sidebar |
| `/diagnose` | Show or hide the sidebar diagnose block (cache hit ratio, cache write, context use, memory, tool schemas) |
| `/sessions [id]` | Session picker or switch |
| `/new`, `/close` | Open / close a session tab |
| `/rewind` | Rewind menu for the latest turn (§2.9) |
| `/fork` | Fork the session at the latest closed turn into a new tab; refuses while a turn runs |
| `/acp` | Open a new `agy` ACP tab |
| `/reload` | Re-read settings, theme, definitions, policy, hooks, prompt (§2.18) |
| `/serve [stop]` | Start or stop the local browser server attached to this TUI's `SessionRegistry` (`web` feature) |
| `/update` | Print the update status and install hint |
| `/clear` | Clear the transcript view |
| `/reset` | Reset the conversation (session log kept) |
| `/cancel` | Cancel the running turn |
| `/quit`, `/q` | Exit |

`/agent` and `/skill` send `AgentCommand::ApplyAgent`/`ApplySkill`, which
append a System instruction at the next step boundary. Unknown or missing
ids print a System error and send nothing. Note: `/update`, `/tv`, `/find`,
and `/q` are handled but not listed in the completion table; `/update` is
discoverable from the statusline hint.

### 2.6 Mentions popup and chips

Typing `@` opens one Mentions popup: agent and skill rows interleaved, then
files. Interleaving keeps a skill visible on the first screen even when the
project defines many agents. A typed query groups agents before skills.
The list shows up to 12 rows and scrolls with the selection.
Every row starts with a fixed-width kind column: a faint `agent`, `skill`,
or `file`. The kind column keeps the labels aligned. Agent rows show the id
in the teal accent color, skill rows in the amber warning color, and file
rows in the normal text color; each row also shows its source and
description. Rows show the bare id, not the `@agent/` or
`@skill/` prefix. One id can appear as both an agent and a skill.

- Filtering matches `@agent/<id>`, `@skill/<id>`, the bare id, and fuzzy
  prefixes. Up/Down or ctrl+p/ctrl+n move; Tab accepts; Enter accepts
  without submitting; Esc dismisses.
- Accepting inserts `@agent/<id> `, `@skill/<id> `, or `@path ` with one
  trailing space and stays in Insert. An exact agent/skill id keeps the
  popup open; an exact file path dismisses it. The built-in `goal` skill
  accepts the shorter `@goal ` alias; `@skill/goal` still works.
- A known accepted mention renders as an `<id>` chip (bold teal agent, bold
  blue skill) while the raw token stays in the input buffer and submit
  detection. Unknown ids and partial tokens render as typed. Cursor columns
  map through chip display widths; editing operates on the raw text.
  `@goal` renders as a `goal` skill chip.
- On submit, skills apply before the agent, mentions are stripped, repeats
  apply once, and an unknown mention stays as text. A mention-only prompt
  applies definitions without starting a turn. A directly submitted user message keeps a
  known mention as the same `<id>` chip, while the prompt sent to the model and
  the session log carry the stripped text; replay shows the stripped prompt.

### 2.7 Approval overlay

When policy returns `Ask`, the overlay shows the tool name, effect class,
and the approval preview (diff or exact command and `cwd`). Queued requests
show a count.

| Key | Decision |
| --- | --- |
| `y` / `Enter` | Approve once (no grant) |
| `t` | Approve for this turn (Turn grant, cleared at turn end) |
| `a` | Approve for the session (Session grant) |
| `n` | Deny (structured refusal to the model) |
| `r` | Deny with a reason (inline mini-prompt; Enter sends) |
| `Esc` | Cancel the request and the turn |

There is no "inspect full file context" key; open the sidebar or diff modal
before deciding. External-agent (ACP) permissions use the same overlay and
are labeled as such; the decision is advisory ([acp.md](acp.md)).

### 2.8 Question overlay

The model can call `question-ask` for a product or configuration choice.
The overlay shows the header, the question, and a mode line: `Select one`
(radio markers `( )`/`(•)`) or `Select all that apply` (checkbox markers
`[ ]`/`[x]`). An `Other (type your own)` row is always last.

- Up/Down move the focus, including the `Other` row. Space toggles
  (multiple) or selects (single). Typing on `Other` enters free text,
  submitted as an extra selected label; for single-select, a typed answer
  clears the option selection and vice versa. Backspace edits the text.
- Enter submits on the last question (or advances); on an empty `Other` row
  it starts typing. Tab/Right and Left move between questions. Esc cancels
  with a structured refusal.
- Questions queue per tab like approvals. A question is not a permission:
  no grant, no mode change. Transcript scrolling still works while a
  question is open. Headless runs auto-cancel with a stderr hint.
- The transcript keeps one Question block per call (`? header`, the
  question, then `✓ selected` labels, `(waiting for answer)`, or
  `(cancelled)`), live and after replay; the `question-ask` tool row is
  hidden.

### 2.9 Rewind and the user-message menu

`/rewind` opens a menu for the latest user turn (refused while a turn runs
or when no closed turn exists):

- **Rewind code + conversation** (default) — restore files from the turn
  snapshots, then shadow the conversation through the turn boundary. The
  rewound user message returns to the composer (cursor at end, Insert mode,
  draft overwritten) for amendment.
- **Rewind code only** — restore files; transcript and composer untouched.

Up/Down moves, Enter applies, Esc cancels. Clicking a user message opens
`UserMessageActions` with **Rewind** (`w`), **Copy** (`c`), and **Fork**
(`f`), or Enter on the highlighted row. Rewind sends
`AgentCommand::Rewind { turn_id, scope: Both }` for that turn. Fork calls
`SessionOpener::fork_session` at the previous turn (or creates a session
when there is none) and puts the message in the new tab's composer without
sending it.

Limits: shell-only edits, external edits, gitignored files, symlinks, and
files over 2 MiB are not restored. There is no `Summarize from here`
option. Checkpoints are session recovery, not version control
([harness-spec.md](harness-spec.md) §2.7).

### 2.10 Session replay

`meerai --continue`, `--resume <ID>`, or `--fork <ID>` without `--prompt`
starts the TUI with the stored transcript replayed
(`meerai_session::replay::replay_from_events`). Replay covers user and
steering messages, assistant text and reasoning, tool calls and results,
questions and their selected answers, and compaction markers. An unmatched
tool call shows as cancelled; system and runtime events are skipped. Token
counters seed from the last logged `UsageUpdated`. Replay reads the log; it
has no separate retention UI, and startup pruning always spares the session
being replayed.

### 2.11 Tabs and the session picker

- Each tab has a `×` close zone and a `[ + ]` new-tab button. Tab color
  encodes status: red for a pending approval, amber while running, text for
  unread/finished, muted when idle; the active tab uses the accent color.
  There is no dot or spinner glyph.
- Closing a tab quits its worker but keeps the session on disk.
- `meerai-tui/src/tabs.rs` persists session ids, the active index, and the
  reusable empty session per workspace to the `ui_tabs` row in SQLite. On a plain
  TUI start the CLI opens one empty session tab and restores the saved sessions
  in the background, so the empty tab stays active. The saved empty session is
  reused instead of creating another; closing an empty tab keeps it for the
  next start. `/new` and `[ + ]` switch to an open empty tab instead of opening
  a second one. The saved active index is not restored. Tests disable
  persistence.
- The session picker (`/sessions`, `ctrl+s`) deletes with `ctrl+d` after a
  second `ctrl+d` confirmation. Open sessions close automatically; deleting
  the current session when it is the last tab creates a replacement first.

### 2.12 Diff review and search

- The diff modal (`ctrl+e`, `/diff`, sidebar click) renders side-by-side
  git working-tree diffs (`git2`) with `meerai-syntax` highlighting; a
  rename uses the new side for the language. `←`/`→` (or `[`/`]`) cycle
  files. `f` toggles a navigable file list (Up/Down/j/k select, Enter opens,
  Esc closes the list before the modal); clicking a row opens that file.
- Search (`ctrl+f` files, `ctrl+shift+f` content) uses the `WorkspaceQuery`
  implementation: a gitignore-aware walker and in-process grep, with a
  result list and preview pane. There is no transcript search.
- The transcript's inline edit diffs are a separate render path over the
  same `diff.rs` data: edits and writes expand to a unified diff (3 context
  lines) with the path and `+N -N` counts, colored additions/removals, and
  one-pass tree-sitter highlighting split by line.

### 2.13 Command palette

`ctrl+p`/`ctrl+shift+p` opens the palette unless the completion popup is
open (there it keeps its Up/Down behavior). Rows group into sections
(Session, Model, Approval, View, Turn, App) with a right-hand shortcut
column; typing filters; Enter runs the highlighted row. Palette rows run
through the same slash-command handler; tab, copy, mode-cycle, and the File
Changes panel toggle use dedicated palette actions. Labels render as
`ctrl+<key>`; `ctrl+t` is not bound.

### 2.14 User shell (`!`)

`!` at the start of a composer line opens a Terminal dialog and runs the
command in the workspace, for example `!cargo test`. The TUI queues
`PendingSessionAction::RunUserShell`; the CLI host implements
`SessionOpener::run_user_shell` with `CodeTools::invoke("code-shell", ...)`,
so the workspace guards, `ExecutionWorld`, env scrub, `strict_env`, and
`require_for_shell` all apply. `plan` mode refuses it.

Shells run concurrently and each run gets its own id and dialog. The dialog
shows `running…` then the output (`j`/`k` scroll). `Esc` backgrounds it: the
dialog closes and the run stays listed in the statusline counter. The result
lands in the transcript and in the run's detail view. `q` closes the dialog.
Long commands use the shell timeout (120 s default). The TUI never spawns
shell or tool processes itself; the only native helpers it spawns are the
clipboard programs (§2.3).

### 2.15 Statusline

One monochrome row, left to right: workspace path, git branch (when the
workspace is a git repo), an optional sandbox warning, the `ACP:agy` badge on ACP tabs, clickable `N agents` and `N shells` counters for running work, a clickable `web :<port>` badge while `/serve` is running, a clickable `update vX (/update)` hint,
throughput after the first measured stream, and a braille spinner with the
active tool. Token and cache counters belong to the
sidebar `DIAGNOSE` panel, so the statusline stays one row of state, not a
second usage report. Short notices replace the right side temporarily.

The counters include running work only and appear only when the count is
non-zero. Each clickable statusline item (both counters, the `web` badge, and
the update hint) gets a `surface_high` background and `text` foreground while
the pointer is over it. Clicking `N agents` or `N shells` opens the `Runs` modal
for that kind. A row lists one running agent (name, preset, status, steps) or one
running shell (`!command`, elapsed). `Enter` or a row click opens the detail view.
`Esc` in the detail returns to the list; a second `Esc` closes it. The agent
detail shows the child conversation and refreshes while the agent runs, without
expanding the same subagent row in the transcript. The shell detail shows the
command, status, elapsed time, and output. The list uses the normal modal frame,
so it blocks composer input and mouse actions behind it.

A subagent run can carry an optional `name` (`subagent-run` or `acp-run`
argument, `/subagent --name`, or the spawn panel `Name` field). The transcript
header, `Runs` row, agent detail title, and web agent card show the name with the
preset in parentheses; without a name they show the preset.

The sidebar `AGENTS` block stays display-only: it lists the main agent and the
subagents with their status, but a row is not clickable.

The model picker (`/model`, `ctrl+o`) lists catalog, saved, and recent
models. `ctrl+d` removes the highlighted model, or the typed model id when
nothing is highlighted: it drops the id from `recent_models`, clears the
saved `provider.model`, and clears the active model when it matches
(`forget_provider_model`, `meerai-common/src/settings.rs`). The change is
persisted immediately.

### 2.16 Notifications and exit summary

- The window title is `M | <title> ●` idle, `M | <title> · working ●`
  during a turn, and `M | <title> · approval needed ●` while an approval
  waits (fallback `M | meerai`). Only OSC 2 is set, plus the iTerm2 tab
  color (OSC 6: green idle, amber working, red approval). An approval also
  rings the bell and sends OSC 9. `MEERAI_NO_NOTIFY=1` disables all of it;
  teardown restores the title and tab color.
- On exit the binary prints the gradient M logo, `Session <title>` (session
  id when untitled), and `Continue meerai --resume <id>` for the active
  tab. The logo art is shared with the TUI header (`meerai_tui::logo`).
  Headless runs print nothing extra.

### 2.17 Update check

`meerai-tui/src/update.rs` reads the latest GitHub release tag at TUI
startup (3 s timeout); `MEERAI_NO_UPDATE_CHECK=1` disables it. A newer
version shows the clickable statusline hint; `/update` prints the install
hint; the palette has "Check for updates". meerai never installs an update
on its own. The check is behind the default `update` feature (reqwest).

### 2.18 `/reload`

Re-reads user and project settings, the theme palette, definitions, policy
rules, context budgets, hooks, and the composed system prompt. It keeps
every session, transcript, and tab, and sends `AgentCommand::Reload` plus
`SetHooks` to each tab. Provider catalog, API keys, and the approval mode
are not reloaded (use `/connect`, `/model`, `/mode`); the update check is
startup-only.

### 2.19 ChatGPT plan sign-in

`/connect` → OpenAI opens an **Auth method** dialog with `Continue with ChatGPT`
and `Use an API key` when the provider has no credential. Choosing ChatGPT
starts the OpenAI Sign in with ChatGPT plan-usage flow through the host service;
the TUI never spawns a process.

A provider that already has a stored credential, environment key, or config key
opens a **Connection** dialog instead. It shows the status badge and offers
`Continue to model select`, `Update credential`, and, for a stored API key or
plan credential, `Remove saved credential`. Switching away and back therefore
does not start an unwanted sign-in. `Update credential` opens the auth method
dialog; `Remove saved credential` deletes the stored entry and returns to the
provider list.

- The host opens the system browser and returns the authorize URL.
- The sign-in dialog shows the provider, `Waiting for the browser callback...`,
  the URL as wrapped text, and a paste field. Use the URL on a remote host where
  the browser is on another machine.
- `Enter` sends the pasted final redirect URL; `ctrl+y` copies the URL to the
  system clipboard; `Esc` cancels and closes the loopback listener.
- On success the credential is stored, the provider status becomes `plan`, recent
  models reload from disk, and the model picker opens. On failure the dialog
  keeps the error; `Enter` retries with a fresh loopback listener, and `Esc`
  cancels.

The dialog is 66 columns. The paste field is only needed when the browser cannot
reach `127.0.0.1`; the browser callback completes the flow automatically when it
can.

### 2.20 ACP surfaces

`/acp` opens a new tab bound to the `agy` adapter. The statusline shows the
`ACP:agy` badge; the transcript header shows `agy (acp)` with
`external agent · policy not applied`. See [acp.md](acp.md) for the trust
boundary and behavior.

## 3. Planned

| Item | Description | Gate |
| --- | --- | --- |
| Select mode (`v` / `x`) | Keyboard range/block selection, `y` yank, `q` quote into the composer. Today only mouse drag selection exists. | Input-mode design |
| Space leader | Key-hint popup: `f` files, `/` grep, `b` sessions, `m` models, `p` permissions, `d` diff, `g` git, `c` compact, `n` new session | Discoverability review |
| `:` command mode | `:model`, `:provider`, `:mode`, `:compact`, `:session`, `:export`, `:theme`, `:q` | Overlaps slash commands; needs a decision |
| Transcript search | Search within the conversation with `n`/`N`; today `/` searches the workspace | Search seam |
| `Summarize from here` | Rewind-menu option that compacts from a chosen turn | Compaction UX |
| Inline viewport | Render inside the existing terminal scrollback instead of the alternate screen | Rendering architecture |
| Statusline step counters | Per-turn step index alongside usage | Statusline design |
| Completion parity | List `/update`, `/tv`, `/find`, `/q` in slash completion and `/help` | Small fix |

## 4. Reference: implementation architecture

Dependencies: `ratatui` + `crossterm`, `tokio`,
`meerai-agents`/`meerai-session`/`meerai-policy`/`meerai-client`/`meerai-common`,
`grep-*` + `ignore` for search, `git2` for the change sidebar, `comrak` for
markdown, `similar`, `anyhow`, `serde_json`, `futures`, `regex`. `reqwest`
is behind the default `update` feature. The crate does not depend on
`meerai-tools` or `meerai-providers`; highlighting goes through
`meerai-syntax` (Rust, Python, JavaScript, TypeScript/TSX, Go, Bash, HTML,
CSS, JSON, plus a lexical fallback).

- Rendering is split by panel under `src/ui/` (`transcript`, `sidebar`,
  `composer`, `modals`, `statusline`, `selection`); `AppState` lives under
  `src/app/` with key handling, modals, commands, palette, and reload in
  separate modules. `TerminalGuard` restores raw mode and the alternate
  screen in `Drop` and installs a panic hook.
- Completion uses a custom scorer in `completion.rs` (no `nucleo`); the
  text buffer is a plain `String` with cursor helpers (no `ropey`);
  clipboard is OSC 52 plus native helpers (no `arboard`).
- `WorkspaceQuery` (`workspace.rs`) supplies `list_files` and
  `search_content`; `LocalWorkspace` walks with `ignore` (depth 16, 10k
  files) and greps in process. It is injectable through
  `TuiLaunchConfig::workspace_query`.

Async event loop:

```text
  crossterm EventStream (keys, paste, resize, mouse) ──┐
                                                       ├──▶ tokio::select! ──▶ AppState ──▶ ratatui::draw()
  mpsc::Receiver<AgentEvent> (items, deltas, usage) ───┘         │
                                                                 ▼
                                                       mpsc::Sender<AgentCommand>
```

The agent turn runs in a Tokio task (`spawn_worker`); the UI loop updates
`AppState` and redraws on a capped tick. `ItemDelta::AgentText` appends to
the open assistant entry; `ItemCompleted` settles the final text.
Esc/ctrl+c send `AgentCommand::Cancel` and trip the turn's
`CancellationToken`.

### Dialog width caps

New dialogs stay narrow. Current caps (`meerai-tui/src/ui/modals.rs`):

| Dialog | Width |
| --- | --- |
| Approval | ≤ 76 |
| Provider | ≤ 68 |
| Auth method | 66 |
| ChatGPT sign-in | 66 |
| API key | 66 |
| Model | 64 |
| Session list | ≤ 72 |
| Command palette | ≤ 62 |
| Message menu | 44 |
| Rewind menu | ≤ 60 |
| Serve dialog | 64 |
| Queued edit | 64 |
| Question | ≤ 76 |
| Context | ≤ 78 |
| Memory | ≤ 86 |
| Routing | ≤ 80 |
| Packs | ≤ 84 |
| Sampling | ≤ 68 |
| Vault reset | ≤ 72 |
| Subagent spawn | ≤ 70 |
| Runs | ≤ 68 |
| Run detail | 84% of the terminal |
| Diff | 92% of the terminal |
| Search | 90% of the terminal |
| Terminal (`!`) | 80% of the terminal |

Every dismissible dialog uses the same frame: a bold one-line title band, a
close control, a key-hint footer with the primary action first and the exit
hint last, and the shared list selection and hover styles. Approval and
Question are not dismissible: they have no close control and must resolve or
cancel. Titles use sentence case and append `(/command)` when a slash command
opens the dialog.

## 5. Invariants

- **Crate boundaries.** `meerai-tui` must not depend on `meerai-tools` or
  `meerai-providers`. File listing and content search go through
  `WorkspaceQuery`; provider catalog, key status, and secret contracts go
  through `meerai-client`; highlighting goes through `meerai-syntax`.
- **Transcript cache.** The transcript document is cached
  (`TranscriptRenderCache`/`TranscriptRenderKey`, `app/mod.rs`) and rebuilt
  only when its fingerprint changes (tab id, width, `transcript_version`,
  palette, provider/model/workspace/ACP/key status, per-entry content
  size/state, `turn_activity`, spinner only while a tool runs). Bump
  `SessionTab::transcript_version` at every transcript mutation. Hover is
  an overlay paint, never baked into cached lines. Markdown, edit-diff
  parses, and tree-sitter tokens are memoized per entry.
- **Turn digests.** `build_transcript_lines` emits one digest per contiguous run
  of activity (thoughts, tools, and subagents), immediately before the visible
  message (`Assistant` text, `User`, `Question`, `System`, or `Error`) that
  follows it—or at the end of the transcript while work trails. When an
  `Assistant` entry carries both reasoning and response text, its reasoning folds
  into the digest right above that response. `code-shell`/`sac-run_script` count
  as commands, `code-read_file` as reads, `code-write_file`/`code-edit_file` as
  edits, `subagent-run`/`acp-run` as agents, other tools as tools, and non-empty
  reasoning as thoughts. Active groups use gerunds (`Running 1 · Reading 2`);
  finished groups use noun counts (`1 command · 2 reads`). Each digest is keyed
  by its first activity entry index and collapses its group's tool rows, thought
  blocks, and subagent rows by default, even while running; the `+`/`-` marker
  is a glyph, and the toggle is mouse-click only. Response text, errors, and
  system notices stay visible. Keep the tool-name mapping in `ui/transcript.rs`
  when tool names change.
- **Tool rows.** Action labels come from `activity_verbs`
  (`Reading`/`Read`, `Running`/`Ran`, `Editing`/`Edited`,
  `Searching`/`Searched`). Rows are expandable even while running:
  expanded shows pretty-printed arguments, then the script when present,
  then the output; collapsed non-edit runs show the last 8 output lines,
  expanded caps at 200. Finished rows fade to `faint`/`muted`; a failed row
  keeps `error` in the badge. Thought blocks are click-only (`Thinking`
  while streaming, `Thought` after).
- **Subagent folds.** Rows carrying a `child_session_id` expand to the
  child's own conversation, loaded through
  `SessionOpener::read_session_events` and refreshed about once per second
  while the child runs. Nested folds cap at the newest 60 entries. The
  nested view is display-only: its lines share the parent hit zone, so any
  click collapses the group. The `N agents` statusline counter opens the
  same conversation in the `Runs` detail modal instead, and that modal
  never expands the transcript row.
- **Markdown spacing.** Assistant markdown keeps the model's line breaks:
  a single newline renders as a line break, a blank line separates blocks
  with one empty row, and transcript entries get two empty rows between
  messages (one before tool rows). Fenced code uses the shared highlighter;
  inline code uses the accent color on `surface_high`.
- **Mouse discipline.** Open modals block all mouse input except scroll.
  Toggles happen on mouse-up only (`pending_toggle` is captured on press
  and cleared on drag), so dragging over expanded content never collapses
  it. Composer clicks are captured so they cannot start a transcript
  selection.
- **Process discipline.** The TUI never spawns shell or tool processes;
  `!` commands go through `SessionOpener::run_user_shell`. Clipboard
  writes may spawn the native helper programs (§2.3).
- **Queue discipline.** Do not send a second `SubmitPrompt` while a turn
  runs; `Enter` queues and `flush_queued` sends on `TurnCompleted`/`Error`.
