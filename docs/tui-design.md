# TUI Design: Selection-First Modal Agent Harness

This document specifies the interactive terminal UI (`meerai-tui`) for Pack
Vision's `meerai` coding-agent harness. It focuses on modal, selection-first,
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
   active tool.

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
  bracketed paste from crossterm.
- `Enter` submits. `shift+enter` or `alt+enter` inserts a newline.
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
| `/mode [plan\|ask\|auto-edit\|yolo]` | Show or set the approval mode |
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
**Mentions** popup: agents first, then skills, then files. Agent rows show
`<id>|agent` with a cyan badge, skill rows show `<id>|skill` with a magenta
badge, and each shows its source and description; file rows show a path and no
badge. Rows show the bare id, not the `@agent/` or `@skill/` prefix. One id can
appear as both an agent and a skill. Filtering matches `@agent/<id>`,
`@skill/<id>`, the bare id, and fuzzy prefixes. Up/Down or ctrl+p/ctrl+n move;
Tab accepts; Enter accepts without submitting; Esc dismisses. Accepting inserts
`@agent/<id> `, `@skill/<id> `, or `@path ` with one trailing space and stays in
Insert. An exact agent or skill id keeps the popup open; an exact file path
auto-dismisses. The composer title says `@ mentions`. A known accepted mention
renders as an `<id>` chip (bold cyan agent, bold magenta skill) while the raw
`@agent/<id>`/`@skill/<id>` stays in the input buffer and submit detection;
unknown ids and partial tokens render as typed. Cursor columns use the chip
display width: before a chip the column is normal, inside a chip it maps to the
chip end, and after a chip it adds the chip id length. Editing still operates on
the raw text. On submit, skills
apply before the agent, mentions are stripped, repeats apply once, and an unknown
mention stays as text; a mention-only prompt applies definitions without a turn.
`/agents` and `/skills` do not exist.

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
choice. The overlay shows the header, the question, and the options:

- `Up` / `Down` — move the option focus.
- `Space` — toggle an option when `multiple` is true; otherwise select it.
- `Tab` / `Right` — next question when a call contains several.
- `Left` — previous question.
- `Enter` — submit on the last question (or advance to the next).
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
truncates the visible transcript before the turn. The menu refuses to open while
a turn is running. Shell-only edits and files over 2 MiB are not restored; there
is no `Summarize from here` option yet.

### 7. Session replay — current

Starting `meerai --continue` or `meerai --resume <ID>` without `--prompt` opens
the session and starts the TUI with its transcript replayed. Replay covers user
and steering messages, assistant text and reasoning, tool calls and results, and
compaction markers. A tool call with no result shows as cancelled. Runtime and
system events are skipped. Token counters seed from the last logged usage entry.
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
[INS|NOR] [PLAN|ASK|EDIT|YOLO]  provider / model [KEY] | 18.4k tok  session ↑120k ↓18k Σ138k  42.1 tok/s  | ⠋ tool:code-shell (1.2s)
```

- The input-mode badge is blue (`NOR`) or green (`INS`); the approval badge is
  magenta (`PLAN`), yellow (`ASK`), cyan (`EDIT`), or light red (`YOLO`).
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
- **Markdown/highlighting:** plain text plus manual styling (no
  `pulldown-cmark`, `syntect`, or `tree-sitter`).
- **Clipboard:** OSC 52 plus native helpers (no `arboard`).
- **Search/diff:** `grep-searcher`/`grep-regex`/`ignore` for workspace search;
  `similar` diffs arrive in `FileChange` items and render in the diff modal.

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
