# TUI Design: Selection-First Modal Agent Harness

This document specifies the interactive terminal UI (`meerai-tui`) for Pack
Vision's `meerai` agent harness. It focuses on modal, selection-first,
zero-memorization keyboard ergonomics tailored for inspecting multi-step tool
execution, reviewing code diffs, and steering long coding sessions in the
terminal.

**Status:** sections marked **Current** match `meerai-tui/src/app/` and
`meerai-tui/src/ui/`. Sections marked **Planned** are design, not implemented.
Today the TUI has `NOR` and `INS` modes, slash commands, a `ctrl+p` command
palette, multi-session tabs, approvals, user-choice questions, a sidebar with
file changes, diff/search modals, transcript replay for a resumed session, a `@`
mentions popup, and a config-driven color palette. It has no Select mode, `Space` leader, `:`
command mode, transcript search, or `/rewind`. Workspace file listing and content
search go through the `WorkspaceQuery` trait so a future remote client can
replace the local walker.

## Core Interaction Principles

1. **Modal separation (`NOR` vs `INS`):** Typing a prompt never collides with
   scrolling conversation history, expanding tool outputs, or reviewing diffs.
2. **Discoverable actions:** `/help` lists commands and shortcuts. A `Space`
   leader with a key-hint popup is **planned**.
3. **Completion with preview:** `@` completes workspace files; `/` completes
   slash commands; `/model` completes known models. The search modal shows a
   result list and a preview pane.
4. **Information-dense statusline:** Input mode, approval mode, provider/model,
   credential state, current-turn and session token usage, throughput, and
   active tool. While `subagent-run` is active it names the running agents;
   clicking that segment opens a `SubagentOverlay` above the composer. A row
   click expands that child session in the transcript and keeps the overlay;
   `Esc` or a click outside closes it.
5. **Paged sidebar changes:** the file list pages with a clickable
   `… and N more` footer (back to top at the end) and scrolls with the mouse
   wheel. The diff modal cycles files with `←`/`→` (or `[`/`]`).

---

## Screen Layout

Current layout (illustrative):

```text
┌─────────────────────────────────────────────────┬──────────────────────────┐
│ Conversation & tool stream                      │ Sidebar (ctrl+b)          │
│                                                 │  Overview                 │
│ > You                                           │  File changes             │
│   Refactor the agent loop                       │                           │
│ ▸ [tool] read_file  succeeded                   │                           │
│ ▾ [tool] edit_file  succeeded                   │  src/loop.rs  +18 -9      │
│                                                 │                           │
│ ◆ Assistant (streaming...)                      │                           │
│   I replaced stdout logging with events...      │                           │
├─────────────────────────────────────────────────┴──────────────────────────┤
│ Prompt (INS) — @ files, / commands, shift+alt+enter newline                │
│ > Also add a test with @meerai-agents/█                                     │
├────────────────────────────────────────────────────────────────────────────┤
│ INS ASK  openrouter / deepseek-v4.1-flash [KEY] | 18.4k tok | 42.1 tok/s   │
└────────────────────────────────────────────────────────────────────────────┘
```

The sidebar is hidden below a minimum width (`MIN_SIDEBAR_WIDTH`) and waits for
a resize. Pressing `ctrl+b` toggles it; clicking a changed file opens the diff
modal. There is no separate "Context / Diff Drawer". The palette follows the
opencode dark theme and is overridable through the `theme` block in
`settings.yml` or `.meerai/settings.yml`.

---

## Modes & Keybindings

### 0. Global shortcuts (any mode, unless a modal is open)

| Key | Action |
| --- | --- |
| `ctrl+p` / `ctrl+shift+p` | Open the command palette |
| `shift+tab` | Cycle approval mode: `plan` -> `ask` -> `auto` |
| `ctrl+n` | New session tab |
| `ctrl+s` | Session picker |
| `ctrl+w` | Close tab |
| `alt+shift+left` / `alt+shift+right` | Move the active tab left / right |
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
| `ctrl+c` | Cancel the running turn; press twice to exit |

Tabs also reorder by mouse: press a tab to switch to it, drag more than two
columns, and release over the target tab to drop it there. The reorder persists
with the tab order in `open-tabs.json`.

### 1. Normal Mode (`NOR`) — current

| Key | Action |
| --- | --- |
| `i` / `a` | Focus the prompt composer and enter **Insert Mode** |
| `j` / `k` / `Down` / `Up` | Scroll the transcript |
| `ctrl+d` / `ctrl+u` | Half-page down / up |
| `gg` / `G` | Top / bottom |
| `ctrl+p` | Open the command palette |
| `y` | Yank the latest assistant (or error/system) text to the clipboard |
| `/` or `ctrl+f` | Open the workspace file/search modal |
| `ctrl+b` | Toggle the sidebar |
| `Esc` | Cancel the running turn (when one is active) |
| `q` | Quit |
| `ctrl+c` | Cancel the running turn; otherwise press twice to exit |

Clipboard writes use the OSC 52 escape sequence plus a native helper
(`pbcopy` / `clip` / `wl-copy` / `xclip` / `xsel`) when available. Dragging with
the mouse over the transcript selects text and copies it on release; there is
no keyboard Select mode.

### 2. Insert Mode (`INS`) — current

- A multi-line prompt buffer with cursor motions, backspace/delete, and
  bracketed paste from crossterm. Long lines soft-wrap at the input width; the
  composer grows to 8 rows and scrolls beyond that.
- `Enter` submits. `shift+enter`, `alt+enter`, or `ctrl+enter` inserts a newline.
- `@` opens file completion. `/` on an empty line opens slash-command
  completion. `/model ` opens model completion.
- `Tab` accepts a completion; `Enter` accepts a completion or submits when the
  completion is a no-op. `Esc` dismisses completion, then returns to `NOR`
  without discarding the draft.
- While a turn runs, `Enter` queues the message. The next queued message is
  sent when the turn ends, in order.
- `ctrl+g` sends now: it steers the oldest queued message (or the current draft
  when the queue is empty) at the next step boundary.
- Click the thought header to expand or collapse it. There is no keyboard
  toggle.
- `ctrl+p` (or `ctrl+shift+p`) opens the command palette when the completion
  popup is closed. The palette groups actions into sections, shows each
  shortcut in a right-hand column, filters as you type, and runs the
  highlighted row with `Enter`.

### 3. Slash commands — current

Typing `/` opens completion. The implemented command set is:

| Command | Action |
| --- | --- |
| `/help` | Show commands and shortcuts |
| `/auth` | Show credential sources |
| `/connect`, `/provider` | Configure or switch provider |
| `/models`, `/model [name]` | Pick or set the model |
| `/agent <id>` | Apply a markdown agent by id |
| `/skill <id>` | Apply a markdown skill by id |
| `/mode [plan\|ask\|auto]` | Show or set the approval mode |
| `/compact` | Request context compaction |
| `/search [query]`, `/grep [query]` | Open file/content search |
| `/diff [path]` | Open the diff view |
| `/sidebar` | Toggle the sidebar |
| `/sessions [id]` | Open the session picker or switch to an id |
| `/new` | Open a new session tab |
| `/close` | Close the active session tab |
| `/rewind` | Restore code and/or conversation for the latest turn |
| `/reload` | Re-read settings, theme, definitions, policy, and prompt |
| `/update` | Print the install hint |
| `/clear` | Clear the transcript view |
| `/reset` | Reset the conversation (session log is kept) |
| `/cancel` | Cancel the running turn |
| `/quit`, `/q` | Exit |

`/agent <id>` and `/skill <id>` send `AgentCommand::ApplyAgent`/`ApplySkill`,
which append a System instruction at the next step boundary. Typing `@` opens the
**Mentions** popup: agents first, then skills, then files. Agent rows show the
id in the teal accent color, skill rows show the id in the blue interactive
color, and each shows the source and description; file rows show a path in the
normal text color. Rows show the bare id, not the `@agent/` or `@skill/` prefix.
One id can
appear as both an agent and a skill. Filtering matches `@agent/<id>`,
`@skill/<id>`, the bare id, and fuzzy prefixes. Up/Down or ctrl+p/ctrl+n move;
Tab accepts; Enter accepts without submitting; Esc dismisses. Accepting inserts
`@agent/<id> `, `@skill/<id> `, or `@path ` with one trailing space and stays in
Insert. An exact agent or skill id keeps the popup open; an exact file path
auto-dismisses. The composer title says `@ mentions`. A known accepted mention
renders as an `<id>` chip (bold teal agent, bold blue skill) while the raw
`@agent/<id>`/`@skill/<id>` stays in the input buffer and submit detection;
unknown ids and partial tokens render as typed. Cursor columns use the chip
display width: before a chip the column is normal, inside a chip it maps to the
chip end, and after a chip it adds the chip id length. Editing still operates on
the raw text. On submit, skills
apply before the agent, mentions are stripped, repeats apply once, and an unknown
mention stays as text; a mention-only prompt applies definitions without a turn.
`/agents` and `/skills` open the Mentions popup filtered to that kind.

### 4. Approval Overlay (`APR`) — current

When the policy returns `Ask` for a tool call (for example `write_file`,
`edit_file`, or `shell`):

- Shows the tool name, effect class, and the approval preview (diff or exact
  command and `cwd`). Queued requests show a count.
- Keys:
  - `y` or `Enter` — Approve once (no grant)
  - `t` — Approve for this turn (records a Turn grant, cleared at turn end)
  - `a` — Approve for the session (records a Session grant)
  - `n` — Deny (the model receives a structured refusal)
  - `r` — Deny with a reason (inline mini-prompt; `Enter` sends)
  - `Esc` — Cancel the request and the turn
- There is no `e` key and no "inspect full file context" key; open the sidebar
  or diff modal before deciding if you need context.

### 5. Question Overlay (`QST`) — current

The model can call `question-ask` when it needs a product or configuration
choice. The overlay shows the header, the question, and a mode line:
`Select one` (radio markers `( )` / `(•)`) or `Select all that apply`
(checkbox markers `[ ]` / `[x]`). An `Other (type your own)` row is always last:

- `Up` / `Down` — move the option focus, including the `Other` row.
- `Space` — toggle an option when `multiple` is true; otherwise select it.
- Typing on the `Other` row — enter free text; it is submitted as an extra
  selected label. For a single-select question, a typed answer clears the
  option selection, and selecting an option clears the typed answer.
- `Backspace` — edit the typed answer.
- `Enter` — submit on the last question (or advance to the next); on an empty
  `Other` row it starts typing.
- `Tab` / `Right` — next question when a call contains several.
- `Left` — previous question.
- `Esc` — cancel the request; the model receives a structured refusal.

Pending questions queue per tab like approvals. A question is not a permission
prompt: it never records a grant and it never changes the approval mode. While a
question is open, normal transcript input is blocked, but transcript scrolling
still works. Headless runs auto-cancel the question and print a hint on stderr.

### 6. Rewind menu (`RWD`) — current

`/rewind` opens a menu for the latest user turn:

- **Rewind code + conversation** — restore files from the turn snapshots, then
  shadow the conversation through the turn boundary. This is the default.
- **Rewind code only** — restore files and keep the conversation.

`Up` / `Down` moves, `Enter` applies, `Esc` cancels. Rewinding the conversation
truncates the visible transcript before the turn and restores the rewound user
message to the composer (cursor at end, Insert mode), ready to amend; any typed
draft is overwritten. Rewinding code only leaves the transcript and composer
untouched. The menu refuses to open while a turn is running. Shell-only edits
and files over 2 MiB are not restored; there is no `Summarize from here` option
yet.

### 7. Session replay — current

Starting `meerai --continue` or `meerai --resume <ID>` without `--prompt` opens
the session and starts the TUI with its transcript replayed. Replay covers user
and steering messages, assistant text and reasoning, tool calls and results,
questions and their selected answers, and compaction markers. A tool call with
no result shows as cancelled. Runtime and system events are skipped. Token counters seed from the last logged usage entry.
Replay reads the session log; it has no separate retention UI. Opt-in startup
pruning (`sessions.auto_prune_days`) always spares the session being replayed,
so a resumed transcript cannot disappear underneath the TUI.

### 8. Planned keybindings (not implemented)

- **Select mode (`v` / `x`)**: range/block selection, `y` yank, `q` quote into
  the composer.
- **Space leader**: `f` files, `/` grep, `b` sessions, `m` models, `p`
  permissions, `d` diff drawer, `g` git, `c` compact, `n` new session.
- **`:` command mode**: `:model`, `:provider`, `:mode`, `:compact`, `:session`,
  `:export`, `:theme`, `:q`.
- **Transcript search** with `n`/`N` (the current `/` searches the workspace).
- A per-message rewind entry and `Summarize from here`.
- An inline viewport option.

---

## Statusline Design

Current single-row status bar, left to right:

```text
[INS|NOR] [PLAN|ASK|AUTO]  provider / model [KEY] | 18.4k tok  session ↑120k ↓18k Σ138k  42.1 tok/s  | ⠋ tool:code-shell (1.2s)
```

- The input-mode badge is blue (`NOR`) or green (`INS`); the approval badge is
  magenta (`PLAN`), yellow (`ASK`), or green (`AUTO`).
- The credential badge reports whether the active provider has a key.
- Usage shows the current turn's token count and the running session totals
  (`session ↑prompt ↓completion Σtotal`). Session counters seed from the last
  logged usage entry on resume. Throughput appears after the first measured
  stream.
- While a turn runs, a braille spinner and the active tool name (or `running`)
  update in place. Short status notices replace the right side temporarily.
- The planned statusline adds workspace/branch and step counters; they do not
  exist today.

---

## Rust Implementation Architecture (`meerai-tui`)

Current dependencies: `ratatui` + `crossterm`, `tokio`, `meerai-agents`/
`meerai-session`/`meerai-policy`/`meerai-client`/`meerai-common`, `grep-*` +
`ignore` for search, `git2` for the change sidebar, `comrak` for markdown,
`similar`, `anyhow`, `serde_json`, `futures`, `regex`. `reqwest` is behind the
default `update` feature. The crate does not depend on `meerai-tools`; file
listing and content search go through `workspace::WorkspaceQuery`, implemented
locally by `LocalWorkspace` and injectable through `TuiLaunchConfig`.

- **Rendering & terminal I/O:** `ratatui` + `crossterm`. `TerminalGuard`
  restores raw mode and the alternate screen in `Drop` and installs a panic
  hook. Only fullscreen mode exists; inline viewport is planned. Rendering is
  split by panel under `src/ui/` (`transcript`, `sidebar`, `composer`,
  `modals`, `statusline`, `selection`); `AppState` lives under `src/app/` with
  key handling, modals, commands, palette, and reload in separate modules.
- **Completion:** a custom scorer in `completion.rs` (no `nucleo`). It covers
  slash commands, `@file` paths, and model ids.
- **Text buffer:** plain `String` plus cursor helpers (no `ropey`).
- **Markdown/highlighting:** `comrak` for Markdown; `meerai-syntax` for
  tree-sitter highlighting (Rust, Python, JavaScript, TypeScript/TSX, Go, Bash,
  HTML, CSS, JSON) with a lexical fallback, so no `syntect` or
  `pulldown-cmark`; grammar dependencies stay out of the TUI crate.
- **Clipboard:** OSC 52 plus native helpers (no `arboard`).
- **Search/diff:** `grep-searcher`/`grep-regex`/`ignore` for workspace search;
  the diff modal builds side-by-side working-tree diffs with `git2`, highlights
  them through `meerai-syntax`, cycles files with `←`/`→` (or `[`/`]`), and toggles a
  navigable file list with `f` (`Up`/`Down`/`j`/`k` select, `Enter` opens, `Esc`
  closes the list first). `similar` powers the transcript's inline edit diffs.

### Async Event Loop

```text
  crossterm EventStream (keys, paste, resize, mouse) ──┐
                                                       ├──▶ tokio::select! ──▶ AppState ──▶ ratatui::draw()
  mpsc::Receiver<AgentEvent> (items, deltas, usage) ───┘         │
                                                                 ▼
                                                       mpsc::Sender<AgentCommand>
                                                       (SubmitPrompt, Steer, Approve,
                                                        SetPermissionMode, Compact, Cancel, Quit)
```

1. **Non-blocking UI:** the agent turn runs in a Tokio task (`spawn_worker`);
   the UI loop updates `AppState` and redraws on a capped tick.
2. **Streaming text:** `ItemDelta::AgentText` appends to the open assistant
   entry; `ItemCompleted` settles the final text.
3. **Cancellation:** `Esc`/`ctrl+c` send `AgentCommand::Cancel` and trip the
   turn's `CancellationToken`; a second `ctrl+c` while idle exits.


---

## Implementation invariants

These rules come from the code and keep the TUI correct.

### Crate boundaries

- `meerai-tui` must not depend on `meerai-tools` or `meerai-providers` again. File listing and content search go through `workspace::WorkspaceQuery`; provider catalog, key status, and secret contracts go through `meerai-client`; highlighting goes through `meerai-syntax`, so grammar dependencies stay out of the TUI crate. A remote client can replace the workspace query through `TuiLaunchConfig::workspace_query`.

### Rendering and cache

- The transcript document is cached. `TranscriptRenderCache`/`TranscriptRenderKey` (`app/mod.rs`) reuse the built+wrapped document until a cheap fingerprint changes (tab id, width, `transcript_version`, palette, per-entry content size/state, `turn_activity`, spinner only while a tool runs). Bump `SessionTab::transcript_version` at every transcript mutation in `app/events.rs`. Hover is an overlay paint, never baked into cached lines; markdown, edit-diff parses, and tree-sitter tokens are memoized per entry. Nested subagent folds are stored capped to the newest 60 entries. Do not reintroduce per-frame full rebuilds or hover-triggered invalidation.
- Tool rows use action labels from `activity_verbs` in `ui/transcript.rs` (`Reading`/`Read`, `Running`/`Ran`, `Editing`/`Edited`, `Searching`/`Searched`), a `+`/`-` expand marker, and no click hint. Hover paints the raised band only on expandable tool, command, subagent, and error rows; the digest and thought headers highlight their text instead. Edits and writes expand to a unified diff (`diff::parse_unified_diff`, 3 context lines from the tool output) with the path and `+N -N` counts in the header, colored additions/removals, and tree-sitter highlighting from `meerai-syntax` (the whole diff is highlighted in one pass, then split by line). Assistant markdown keeps the model's own line breaks: a single newline renders as a line break (not a space), a blank line separates blocks with one empty row, and transcript entries get two empty rows between messages (one before tool rows). Fenced code blocks use the same highlighter, inline code uses the accent color on `surface_high`, and table cells keep inline code and bold styling. Finished rows use `faint`/`muted` text; a failed row keeps `error` in the badge. Collapsed non-edit runs show the last 8 output lines; expand shows the full output (capped at 200 lines).
- The diff modal (`ctrl+e`, `/diff`, sidebar click) renders side-by-side git working-tree diffs with `meerai-syntax` highlighting (a rename path uses the new side for the language). `←`/`→` (or `[`/`]`) cycle files; `f` toggles a file list (`DiffListState`): Up/Down/j/k select, Enter opens, Esc closes the list before the modal, and clicking a row opens that file. The transcript's inline edit diffs and the modal are separate render paths over the same `diff.rs` data.
- Tool rows are expandable even while running. Expanding shows pretty-printed tool arguments, then the script when present, then the output. Collapsed runs show the last 8 output lines.
- Thought blocks are click-only: no key binding toggles them. The header reads `Thinking` while the item streams and `Thought` after the turn ends, with a `+`/`-` expand marker and a text hover highlight.
- `build_transcript_lines` emits one per-turn digest under the user message (`TurnDigest` in `ui/transcript.rs`). `code-shell`/`sac-run_script` count as commands, `code-read_file` as reads, `code-write_file`/`code-edit_file` as edits, `subagent-run`/`acp-run` as agents, and all other tools (grep, glob, list_dir, lsp, repo_map, question-ask) as tools; non-empty reasoning counts as thoughts. Keep that mapping in `ui/transcript.rs` when tool names change. Active groups use gerunds (`Running 1 · Reading 2`); finished groups use noun counts (`1 command · 2 reads`). The digest is a collapse toggle: every turn hides its tool rows, thought blocks, and subagent rows behind the digest line by default, even while running. Clicking the digest expands or collapses that turn (`+`/`-` marker, `SessionTab::turn_activity`, keyed by user entry index, cleared on rewind/`/clear`/`/reset`). Response text, errors, and system notices stay visible; a thought header attached to a visible response hides with the turn.
- Subagent rows are expandable when they carry a `child_session_id`. `SessionItem::SubagentRun` carries the id additively (`#[serde(default)]`), and `ItemStarted` is emitted after the child session exists. Expanding loads the child log through `SessionOpener::read_session_events` and folds it with `transcript_from_events`; while the child runs, the nested view refreshes about once per second (`queue_child_session_refreshes`). Replay converts a `subagent-run` tool result whose hint carries `child_session_id:` into the same Subagent entry. The statusline shows running agent names instead of `subagent-run`. The nested view is display-only: its lines share the parent hit zone, so any click collapses the group.

### Composer and input

- The composer soft-wraps at the input width: a long line occupies several visual rows, the composer grows up to 8 rows, and the cursor and click mapping follow the wrapped rows (`wrap_composer`, `composer_row_start` in `ui/composer.rs`). `Enter` submits; `shift+enter`, `alt+enter`, and `ctrl+enter` insert a newline. Transcript entries are separated by two blank rows (one before tool rows) and markdown blocks by one, so long sessions stay readable.
- Composer clicks move the cursor and start a drag selection (`composer_pointer_down`, `composer_drag_anchor`). Drag (`Drag` or `Moved` while an anchor is held) sets `composer_selection`, paints it with `surface_high`, and never copies; Backspace/Delete or typing deletes the selection. Clicks anywhere in `composer_block_area` are captured so they cannot start a transcript selection. `display_col_to_char_index` and `display_pos_for_byte` map display columns through `@` mention chips; keep them in sync with `render_composer_line`.
- Pressing `!` at the start of a composer line opens a **Terminal** dialog and runs a user shell command (`!cargo test`). `submit_input` clears the input and queues `PendingSessionAction::RunUserShell`; `event_loop` calls `SessionOpener::run_user_shell`, which the CLI host implements with `CodeTools::invoke("code-shell", ...)` so the workspace, `ExecutionWorld`, env scrub, `strict_env`, and `require_for_shell` all apply. `plan` mode refuses it. The dialog shows `running…` then the output (j/k scroll). `Esc` **backgrounds** it: the dialog closes, the statusline shows `shell <command>`, the task keeps running, and the result is appended to the transcript when it finishes; `q` closes the dialog. Long commands use the shell timeout (120 s default). The TUI never spawns a process itself.
- While a turn runs, `Enter` appends to `SessionTab::queued` and sends nothing; `SessionTab::flush_queued` sends the next prompt on `TurnCompleted` or `Error`. `ctrl+g` steers the oldest queued message (or the current draft) at the next step boundary. Queue state is display-only and never enters the session log. Do not send a second `SubmitPrompt` while `is_running`.

### Modals, lists, and navigation

- `ctrl+p`/`ctrl+shift+p` opens the command palette (`ActiveModal::CommandPalette`, `app/palette.rs`) unless the completion popup is open; there it keeps its Up/Down behavior. Rows are grouped by `PaletteCommand::section` (Session, Model, Approval, View, Turn, App) and the hint column shows the shortcut text via `PaletteCommand::display_hint`; key hints and slash-command hints use different styles. Palette rows run through `handle_slash_command` (or a `PaletteAction` for tab, copy, or mode cycle); do not duplicate command logic in the palette. Menu shortcuts: `ctrl+n` new tab, `ctrl+s` sessions, `ctrl+o` models, `ctrl+a` provider connect, `ctrl+e` first file diff, `ctrl+f` search, `ctrl+shift+f` content search, `ctrl+b` sidebar, `ctrl+w` close tab, `ctrl+k` compact, `ctrl+shift+r` reset, `ctrl+q` quit, `f1` help, `alt+shift+left/right` moves the active tab, `shift+tab` cycles plan → ask → auto. Labels come from `app::ctrl_label`, which always renders `ctrl+<key>`; `ctrl+t` is not bound.
- Mouse toggles happen on mouse-up only. `pending_toggle` is captured on press and cleared on drag, so dragging over expanded content never collapses it.
- Clicking a user message opens `ActiveModal::UserMessageActions` with `Rewind`, `Copy`, and `Fork` (keys `w`/`c`/`f` or Enter). Copy uses the clipboard path. Rewind sends `AgentCommand::Rewind { turn_id, scope: Both }`, truncates the transcript before the message, and restores that message to the composer without sending it (`restore_composer_message` in `app/input.rs`, which also clears the composer selection). Fork calls `SessionOpener::fork_session` at the previous turn (or `create_session` when there is no previous turn) and puts the message in the new tab's composer without sending it. Turn ids come from `SessionTab::user_turns`, fed live on `TurnStarted` and from events by `user_turn_map`.
- Open modals block all mouse input except scroll. Do not let clicks behind a dialog start a selection or toggle an entry.
- The sidebar file list pages: the `… and N more` footer is clickable (next page, or back to top at the end) and the mouse wheel over the list scrolls it (`file_change_scroll`). Row clicks map through the scroll offset.
- The statusline shows running subagent names while `subagent-run` is active. Clicking that activity opens a `SubagentOverlay` above the composer with the running subagents (or all subagents when none run). Clicking a row expands that child session in the transcript and keeps the overlay open; `Esc` or a click outside closes it.
- Dialog widths are capped: approval `≤72`, provider `≤64`, API key `64`, model `60`, session list `≤68`, command palette `46..58`, message menu `40`. Diff and search stay wide. Keep new dialogs narrow.
- The session picker (`/sessions`) deletes with `ctrl+d` after a second `ctrl+d` confirmation (`session_delete_confirm`). Open sessions close automatically; deleting the current session when it is the last tab creates a replacement session first. It sends `PendingSessionAction::DeleteSession { session_id }`, then refreshes the list. Deletion removes the session directory through `SessionOpener::delete_session`.
- Tabs: each tab has a `×` close zone (`TabHitKind::CloseTab`). Status shows as the tab color (red approval pending, amber running, text unread/finished, muted idle); the TUI tab bar has no dot or spinner glyph because tabs are narrow. Closing a tab quits its worker but keeps the session on disk. Tabs reorder by drag (press switches immediately, dragging more than 2 columns arms `tab_drag`, mouse-up drops via `move_tab`) or `alt+shift+left/right`; `move_tab` re-anchors `active_tab` and persists the new order. `meerai-tui/src/tabs.rs` saves session ids and the active index per workspace to `<data>/open-tabs.json`; `run_tui` enables persistence (`tab_persistence`), so tests never write it. On TUI start with no `--prompt`/`--continue`/`--resume`/`--fork`, the CLI restores the first saved session as the initial tab and queues the rest as `OpenSessionId`. The saved active index is not restored.

### Chrome, layout, and lifecycle

- On TUI exit the binary prints an exit summary: the gradient M logo, `Session <title>` (session id when untitled), and `Continue meerai --resume <id>` for the active tab at exit. `run_tui` returns `TuiExit { session_id, title }`; the logo rows, gradient, and ANSI renderer live in `meerai_tui::logo` and are shared with the TUI header, so both surfaces render identical art and colors. Headless runs print nothing extra.
- The terminal window title is `M | <session title> ●` when idle, `M | <title> · working ●` during a turn, and `M | <title> · approval needed ●` while an approval waits (fallback `M | meerai`). It sets only the window title (OSC 2) and, on iTerm2, the tab color (OSC 6: green idle, amber working, red approval). An approval request additionally rings the bell and sends an OSC 9 notification. The event loop updates the title when the status or title changes and derives the aggregate status from all tabs. `MEERAI_NO_NOTIFY=1` disables title, tab color, bell, and notification; teardown restores the title and tab color.
- The layout has no rule above the composer. The composer and statusline are one surface panel with one row and one column of padding; the transcript has one row and two columns of padding. The composer keeps the input at the top and shows a metadata line at the bottom, separated by one blank row: input mode badge (`INS`/`NOR`, colored) · approval mode badge · model · provider · `<value>` temperature (two decimals, or `default`), driven by `context.temperature`. The statusline is monochrome: it shows the workspace path, the git branch (when the workspace is a git repo), usage, activity, and hints only; input mode, approval mode, and model/provider/temperature live in the composer footer. The sidebar is two panels: `surface` for the session block, a one-row gap, then `surface_high` for file changes. The session block shows `session`, `provider`, `model`, `auth`, and `dir` (home shortened to `~`); do not duplicate running state, tokens, or speed there because the statusline owns them. The transcript header is one line (`meerai provider/model [auth]`).
- Update checks are notification-only. `meerai-tui/src/update.rs` reads the latest GitHub release tag (3 s timeout) at TUI startup; `MEERAI_NO_UPDATE_CHECK=1` disables it. When a newer version exists, the statusline shows `update vX (/update)` with a click hit zone, `/update` prints the install hint, and the palette has "Check for updates". meerai never installs an update on its own.
- `sac-run_script` arguments are parsed for display: the header preview shows the language and first script line (or the file path), and the expanded run shows the script lines (`›`) before the output (`│`).
- `/reload` re-reads user and project settings, the theme palette (`theme::set`), definitions, policy rules, context budgets, and the composed system prompt. It keeps every session, transcript, and tab; it sends `AgentCommand::Reload` to each tab. Provider catalog, API keys, and the approval mode are not reloaded (use `/connect`, `/model`, `/mode`); the update check is startup-only.

### Availability

- `/rewind` is available only when no turn is running and there is a user turn. It opens the rewind menu; conversation restore truncates the visible transcript before the turn and restores the rewound user message to the composer (cursor at end, Insert mode, draft overwritten). Code-only restore keeps the transcript, `user_turns`, and composer untouched.
