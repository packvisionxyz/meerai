# AGENTS.md

Rust workspace for Pack Vision's terminal agent harness: sixteen library crates plus the `meerai`
binary (built by `meerai-cli`). This file holds working rules only.

It is injected into the system prompt with a 32 KiB cap. Keep it rules-only and short. Put
implementation detail in `docs/`, not here.

## Read first

| Topic | File |
| --- | --- |
| Crate layout, configuration, loop internals, traps | [docs/harness-internals.md](docs/harness-internals.md) |
| Product behavior, milestones, backlog | [docs/harness-spec.md](docs/harness-spec.md) |
| Target architecture and migration | [docs/architecture.md](docs/architecture.md) |
| TUI behavior and keybindings | [docs/tui-design.md](docs/tui-design.md) |
| Local browser UI and parity | [docs/browser-ui.md](docs/browser-ui.md) |
| Security controls and residual risk | [docs/threat-model.md](docs/threat-model.md) |
| ACP client and trust boundary | [docs/acp.md](docs/acp.md) |

## Commands

```sh
cargo fmt --all                                # rustfmt: edition 2024, max_width 100
cargo clippy --workspace --all-targets         # rust-analyzer checks with clippy
cargo test --workspace
cargo test -p meerai-macros --test derive_toolset_pass   # single integration test file
cargo test -p meerai-macros test_snapshot                # single test by name
cargo check -p meerai-cli --no-default-features          # headless-only build surface
cargo check -p meerai-tui --no-default-features          # TUI without reqwest/update
make verify                                    # fmt --check + clippy + test (alias: make ci)
```

- Verify order is `fmt -> clippy -> test`. CI runs the same gate on `ubuntu-24.04`.
- CI has no Windows runner. Windows shell and session-id paths are unit-tested only.
- The toolchain is pinned to Rust 1.100.0 (`rust-toolchain.toml`), edition 2024.
- `release.yml` runs only after CI passes, and only for an unreleased version that matches the tag.
- Update macro snapshots with `cargo insta review` or `INSTA_UPDATE=always cargo test -p meerai-macros`.

## Rules

### Contracts

- `AgentEvent`, `SessionItem`, `ItemDelta`, and the `SessionEvent` vocabulary are wire contracts.
  Change them and the TUI fold, host registry/web mapper, headless NDJSON, and session replay in the same change.
- Use adjacent serde tagging (`tag = "type", content = "data"`) for enums with string-carrying
  variants. Internally tagged enums cannot hold them.
- `meerai-tui` must not depend on `meerai-tools`, `meerai-providers`, or `meerai-web`. Use
  `meerai_host::WorkspaceQuery`, `meerai-client`, and `meerai-syntax` instead.
- Provider model metadata belongs in `ProviderCatalog`, not `HarnessConfig`.
- Keep one `impl Toolset` per toolset. Macro-generated code needs `meerai_core`, `schemars`, and
  `serde_json` as direct dependencies of the consuming crate.

### Turn loop

- Extend the loop through `meerai_agents::intercept`, not by adding branches to `turn.rs`. Hooks are
  not interceptors: they await host processes outside the loop.
- Requests come from `Session::derive_messages`. `chat_history` is only a mirror.
- Keep every assistant tool call adjacent to its results. Batch post-tool notes and log them after
  the last `ToolResult` of the batch.
- Never send `None` for a stored reasoning signature.
- Do not clear tool results at log-write time. Clearing is a projection rule.
- Heal dangling tool calls only through the idempotent helper. Never call `Session::open` from a
  read path.
- Keep the cache-aware summarizer default. Do not switch to a standalone summary request without
  accounting for the prompt cache.
- Do not send a second `SubmitPrompt` while a turn runs.

### Config and security

- A project file can tighten, never grant: drop `allow`, ignore `approval_mode`, `providers`, and
  `sessions`, and clamp project budgets.
- Hooks tighten or ask, never loosen. Project hooks need explicit trust. Do not add an implicit
  trust prompt, and never let a project file trust itself.
- `plan` mode is hard read-only.
- Do not bypass the outside-workspace approval. Grant resources are literal; rule resources are
  globs.
- Redaction keeps no log-only copy of a secret.
- Keep the shell-safety allowlist conservative.
- Do not let a project file control session retention.
- Agent and skill definitions are read-only inputs. They supply prompt text, and an agent
  definition adds `permissions` that tighten only. `skill-load` may read files inside a skill
  directory; nothing grants tools or permissions. Discovery order is `.meerai`, then `.agents`,
  then `.claude`: never copy or import definitions.
- Never describe agent or skill definitions, hooks, or an ACP backend as a security boundary.
  Hooks run unsandboxed host commands; an ACP backend keeps its own tools and policy.

### TUI and Web

- Always keep `meerai-tui` and `meerai-web` / `meerai-web-client` at feature parity. Every
  user-facing workflow, slash command, approval/question flow, session action, and live event
  must work in both clients via `meerai-host` and `meerai-web-protocol`, without duplicating
  business logic. Update `docs/tui-design.md` and `docs/browser-ui.md` in the same change.
- Rebuild the TUI transcript only when its fingerprint changes. Hover is an overlay, never a
  rebuild. Bump `transcript_version` at every transcript mutation.
- The turn-anchor rail projects the wrapped transcript: draw it after the transcript, and never
  rebuild for rail hover.
- Open modals block all mouse input except scroll.
- Keep new TUI dialogs narrow; use the width caps in `docs/tui-design.md`.
- Neither UI spawns tool processes: `!` commands go through `SessionOpener::run_user_shell`.
- `meerai-web-client` reducers carry no DOM types; UI side effects become explicit `UiEffect` values.
- Every user-facing command and workflow gets a dedicated panel or modal in both
  `meerai-tui` and `meerai-web-client`. Do not ship a transcript-only flow in
  one client, or require a typed confirmation argument in one client and a
  panel in the other. Add the command, modal state, renderer, key handling, and
  tests in both clients in the same change. Destructive flows use the same
  explicit confirmation in both.

### Docs

- Update `README.md`, the matching `docs/` file, and `.agents/repo/profile.md` in the same change
  when behavior or a documented invariant changes.

### Commit

- One commit, one line. Squash every change set into a single commit whose message is a single line:
  no body, no bullet list, no paragraphs.
- Never add a second commit above the base, and never merge.

### Code style

- Default to **no comments** (including doc comments). Only add a comment when it is truly crucial:
  changing that code without knowing the invariant would break the application.
- Treat every struct you touch as a design review. Choose the narrowest correct type for each field:
  an enum over a `bool` or a stringly-typed flag, a newtype over a bare primitive when the value has
  invariants, and `Option<T>` only when absence is a real state. Group related fields, order them by
  identity, then state, then metadata, and refactor the struct instead of appending fields to its
  end. Keep serde names and order stable unless the change is intentional.

### Communication

- Respond to the user in ASD-STE100 Simplified Technical English: short sentences, one idea per
  sentence, active voice, no idioms.
- Use BLUF (Bottom Line Up Front): state the result or decision first, then the support.
- Use SBAR (Situation, Background, Assessment, Recommendation) for handovers and status updates.
- These rules do not apply to code, generated files, commit/PR messages, or when the user asks
  otherwise.
