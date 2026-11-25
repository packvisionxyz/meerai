# Refactor Review

A crate-by-crate audit of coding standards and structure across the twenty workspace crates, with a measured baseline and a ranked cleanup backlog.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-25. The file-size and `unwrap` questions are settled in [§6.3](#63-resolved-the-file-size-ceiling) and [§6.4](#64-resolved-where-unwrap-is-acceptable). |
| **Audience** | Contributors planning a cleanup pass; maintainers who own a crate |
| **Scope** | All twenty workspace crates: size, test layout, clippy suppressions, error handling, dependency direction, and TUI/browser duplication |
| **Related** | [architecture.md](architecture.md) · [harness-internals.md](harness-internals.md) · [tui-design.md](tui-design.md) · [browser-ui.md](browser-ui.md) · [AGENTS.md](../AGENTS.md) |

## 1. Overview

The workspace is 219 Rust files and 136,908 lines across twenty crates. Growth is not evenly spread: `meerai-tui`, `meerai-web-client`, `meerai-agents`, and `meerai-host` hold 60% of the 85,441 production lines between them, and fifteen files reach 2,000 lines. The cleanup problem is therefore a small number of specific files and a small number of specific rules that the code no longer follows.

This document records the measured baseline first, then findings for each layer. Every finding cites `path:line` in the crate that owns it. The backlog in [§3](#3-planned) ranks the work by contract risk before file splitting, because splitting a file that carries a wire contract costs more than the split saves.

This is an audit. No source file changes as part of it.

## 2. Implemented

### 2.1 Baseline

The baseline is measured, not estimated. Production lines run to the first `#[cfg(test)]` item in each file. Test lines are the remainder. When that item is an external declaration such as `#[cfg(test)] mod tests;`, the declared module file counts as test code, not production: `meerai-agents/src/multi_turn_agent/tests.rs`, `meerai-cli/src/tests.rs`, `meerai-tui/src/ui/tests.rs`, `meerai-tui/src/app/tests/`, and `meerai-macros/src/test_utils.rs` are counted this way. Files under a crate's `tests/` directory are not in this table at all, which is noted below.

| Crate | Files | Prod LOC | Test LOC | Test % | Workspace deps |
| --- | --- | --- | --- | --- | --- |
| `meerai-tui` | 40 | 23,012 | 11,126 | 32.6 | 11 |
| `meerai-web-client` | 17 | 15,969 | 2,044 | 11.3 | 2 |
| `meerai-agents` | 24 | 8,706 | 14,178 | 62.0 | 6 |
| `meerai-host` | 10 | 5,452 | 1,291 | 19.1 | 7 |
| `meerai-tools` | 25 | 5,352 | 4,638 | 46.4 | 2 |
| `meerai-cli` | 11 | 5,170 | 2,917 | 36.1 | 14 |
| `meerai-web` | 7 | 3,867 | 2,486 | 39.1 | 6 |
| `meerai-session` | 14 | 3,851 | 3,444 | 47.2 | 2 |
| `meerai-providers` | 15 | 2,164 | 1,398 | 39.2 | 4 |
| `meerai-common` | 5 | 2,044 | 2,289 | 52.8 | 3 |
| `meerai-store` | 10 | 1,361 | 655 | 32.5 | 1 |
| `meerai-acp` | 6 | 1,326 | 1,095 | 45.2 | 3 |
| `meerai-syntax` | 5 | 1,325 | 485 | 26.8 | 0 |
| `meerai-web-protocol` | 1 | 1,274 | 460 | 26.5 | 0 |
| `meerai-skills` | 5 | 986 | 530 | 35.0 | 4 |
| `meerai-client` | 3 | 961 | 469 | 32.8 | 1 |
| `meerai-core` | 7 | 933 | 221 | 19.2 | 0 |
| `meerai-policy` | 4 | 703 | 1,432 | 67.1 | 1 |
| `meerai-input` | 1 | 658 | 74 | 10.1 | 0 |
| `meerai-macros` | 9 | 327 | 235 | 41.8 | 1 |

The table now counts integration tests and external test modules as test lines, so no row under-reports coverage. The rest of this document uses the same rule.

Fifteen files are at or above 2,000 lines:

| Lines | File |
| --- | --- |
| 10,018 | `meerai-agents/src/multi_turn_agent/tests.rs` |
| 4,798 | `meerai-web-client/src/state.rs` |
| 3,684 | `meerai-web-client/src/views/modals.rs` |
| 3,618 | `meerai-tui/src/ui/tests.rs` |
| 3,495 | `meerai-host/src/registry.rs` |
| 3,480 | `meerai-tui/src/ui/modals.rs` |
| 3,467 | `meerai-tui/src/app/modals.rs` |
| 2,637 | `meerai-cli/src/tests.rs` |
| 2,582 | `meerai-tui/src/app/input.rs` |
| 2,563 | `meerai-cli/src/main.rs` |
| 2,451 | `meerai-agents/src/multi_turn_agent/tools.rs` |
| 2,438 | `meerai-session/src/session.rs` |
| 2,408 | `meerai-tui/src/app/mod.rs` |
| 2,295 | `meerai-web/src/server.rs` |
| 2,058 | `meerai-tui/src/app/tests/modals.rs` |

The next four files fall just below the line, at 1,964, 1,909, 1,766, and 1,734 lines: `meerai-tui/src/ui/transcript.rs`, `meerai-common/src/settings.rs`, `meerai-policy/src/engine.rs`, and `meerai-web-protocol/src/lib.rs`. They are the same cleanup problem at a slightly smaller size.

### 2.2 Axis one: file size and `mod.rs` ownership

Twelve `mod.rs` files exist. Four of them exceed 600 lines, and three of those are declaration files that also carry real production code:

| File | Lines | Inline test start | Character |
| --- | --- | --- | --- |
| `meerai-tui/src/app/mod.rs` | 2,408 | none, `mod tests;` at `:8` | Declares the module tree, then 2,391 lines of production code. The tests are elsewhere. |
| `meerai-agents/src/multi_turn_agent/mod.rs` | 1,606 | none, `mod tests;` at `:124` | Declares six submodules, then 1,482 lines of production code including `MultiTurnAgentConfig`, `MultiTurnAgent`, and `ChatStreamer`. |
| `meerai-web-client/src/views/mod.rs` | 1,462 | none | Pure production. No test at all. |
| `meerai-tools/src/lsp/mod.rs` | 1,006 | `:47` | 46 production lines and 960 test lines. This is the one real test-dump case. |
| `meerai-tools/src/search_as_code/mod.rs` | 856 | `:458` | 457 production and 399 test lines. Balanced. |
| `meerai-tools/src/repo_map/mod.rs` | 627 | `:17` | 16 production and 611 test lines. A test file that declares a module. |

The pattern that matters is the opposite of what a pure line count suggests. `meerai-tui/src/app/mod.rs` and `meerai-agents/src/multi_turn_agent/mod.rs` are large because they hold production code, not because they hold tests. `meerai-tools/src/lsp/mod.rs` and `meerai-tools/src/repo_map/mod.rs` are large because they hold tests.

Neither shape belongs in a `mod.rs`. The first hides a module's real content behind a declaration file; the second makes a declaration file unreadable as a map.

`meerai-tui/src/app/mod.rs` is the clearest case: 2,391 production lines, including `pub(crate) use` re-exports and state handling, sit in the file whose only job is to say which modules exist.

### 2.3 Axis two: signature and struct debt

Twenty clippy and lint suppressions exist in the tree, in five kinds. Ten are `too_many_arguments`:

| File:line | Suppression |
| --- | --- |
| `meerai-agents/src/multi_turn_agent/mod.rs:1048` | `too_many_arguments` |
| `meerai-agents/src/multi_turn_agent/mod.rs:1072` | `too_many_arguments` |
| `meerai-agents/src/multi_turn_agent/tools.rs:168` | `too_many_arguments` |
| `meerai-agents/src/multi_turn_agent/tools.rs:450` | `too_many_arguments` |
| `meerai-agents/src/multi_turn_agent/tools.rs:1120` | `too_many_arguments` |
| `meerai-tui/src/app/mod.rs:804` | `too_many_arguments` |
| `meerai-tui/src/app/mod.rs:1465` | `too_many_arguments` |
| `meerai-tui/src/app/modals.rs:56` | `too_many_arguments` |
| `meerai-tui/src/markdown.rs:315` | `too_many_arguments` |
| `meerai-tui/src/ui/composer.rs:569` | `too_many_arguments` |
| `meerai-tui/src/app/mod.rs:103` | `large_enum_variant` |
| `meerai-tui/src/app/mod.rs:682` | `large_enum_variant` |
| `meerai-web/src/server.rs:547` | `result_large_err` |
| `meerai-web/src/server.rs:558` | `result_large_err` |
| `meerai-web/src/server.rs:572` | `result_large_err` |
| `meerai-core/src/tools.rs:302` | `dead_code` |
| `meerai-macros/src/tool/common.rs:21` | `dead_code` |
| `meerai-macros/src/tool/attribute_tool.rs:74` | `dead_code` |
| `meerai-macros/src/tool/attribute_tool.rs:95` | `dead_code` |
| `meerai-tools/src/lsp/client.rs:129` | `deprecated` |

Every one of these is a decision that the compiler rejected and a human overruled. Five of the ten `too_many_arguments` sit in `meerai-agents`, five in `meerai-tui`. The three `result_large_err` cases are in one file, `meerai-web/src/server.rs`, which points at one oversized error type rather than three separate problems. The four `dead_code` cases are unused fields and dead test helpers, which are removals rather than redesigns. The one `deprecated` case is an `lsp-types` API and is justified.

`meerai-macros/src/tool/common.rs` declares four overlapping metadata structs — `ToolMeta` at `:6`, `ToolsetDerive` at `:18`, `ToolsetArgs` at `:30`, `ToolArgs` at `:39` — and silences two unused fields with `#[allow(dead_code)]` at `:21`.

`meerai-core/src/lib.rs:10` re-exports the whole `genai` crate. `meerai-core/src/tools.rs:227` and `:234` convert `ToolDefinition` into `genai::chat::Tool`. This makes the provider SDK reachable from every crate that uses `meerai-core`.

### 2.4 Axis three: error handling

This table counts production regions only: test modules, external test files, and `tests/` directories are excluded. A crate-wide count is dominated by test code and hides the real distribution.

| Crate | `.unwrap()` | `.expect()` | `panic!()` | Worst production file |
| --- | --- | --- | --- | --- |
| `meerai-host` | 73 | 0 | 0 | `src/registry.rs` (73) |
| `meerai-web` | 44 | 6 | 0 | `src/server.rs` (44), `build.rs` (4) |
| `meerai-tui` | 25 | 3 | 0 | `src/app/modals.rs` (14), `src/app/mod.rs` (9) |
| `meerai-cli` | 11 | 0 | 0 | `src/main.rs` (11) |
| `meerai-client` | 9 | 2 | 0 | `src/secrets.rs` (9) |
| `meerai-providers` | 3 | 2 | 0 | `src/providers.rs` (2), `src/oauth/refresh.rs` (1) |
| `meerai-macros` | 3 | 0 | 0 | `src/tool/derive_toolset.rs` (3) |
| `meerai-tools` | 0 | 4 | 0 | `src/web/search.rs` (4) |
| `meerai-common` | 0 | 4 | 0 | `src/settings.rs` (4) |
| `meerai-agents` | 0 | 3 | 0 | three files, one each |
| `meerai-session` | 0 | 2 | 0 | `src/session.rs` (2) |
| `meerai-core` | 0 | 2 | 0 | `src/lenient.rs` (2) |
| `meerai-acp` | 0 | 2 | 0 | `src/client.rs` (2) |

Seven crates have zero `unwrap`, zero `expect`, and zero `panic!` in production: `meerai-input`, `meerai-policy`, `meerai-skills`, `meerai-store`, `meerai-syntax`, `meerai-web-client`, and `meerai-web-protocol`.

Three observations follow.

`meerai-agents` is the best-behaved large crate: zero production unwraps in 8,706 lines. The 341 crate-wide unwraps live in `multi_turn_agent/tests.rs` and the other test modules, which is exactly where they should be.

`meerai-web-client` has zero production unwraps. All 49 are in the test module of `src/lib.rs`. So the intuition that "the clients are where unwrap is acceptable" does not match the tree: the browser client already holds the stricter line, and the terminal client does not.

`meerai-host` holds 73 of the 168 production unwraps in the workspace, which is 43%, and `meerai-web` holds 44 more, for 70% between them. `meerai-host` is a seam that three other crates depend on, so its panics reach both clients.

The counting rule for this section: a `.unwrap()` inside a `#[cfg(test)]` module, an external `*_tests` file, or a crate `tests/` directory is test code and is not counted. This is why the numbers here differ from a plain `grep`.

### 2.5 Axis four: TUI and browser duplication

Both clients must stay at feature parity per `AGENTS.md`. The audit measured how much logic they already share by symbol:

- Crate-wide, `meerai-tui` and `meerai-web-client` declare 39 functions with the same name. Most are incidental (`new`, `default`, `key`, `update`, `save`), but the named workflows overlap: `open_diff_modal`, `open_model_input`, `open_provider_select`, `open_search_modal`, `open_command_palette`, `open_runs`, `handle_modal_key`, `cycle_approval_mode`, `create_session`, `fork_session`, `apply_agent`, `apply_skill`, `update_completion`, `accept_completion`, `cancel_chatgpt_login`, `set_effort_from_arg`.
- Modal code is the largest cost in both clients: `meerai-tui/src/app/modals.rs` has 23 functions in 3,467 production lines; `meerai-web-client/src/views/modals.rs` has 44 functions in 3,684 production lines. They share no function body, only intent.
- Both crates own a Markdown renderer built on the same `comrak` call, and the two option sets have drifted. `meerai-tui/src/markdown.rs:19-33` sets 14 `options.extension` flags. `meerai-web-client/src/markdown.rs:10-25` sets the same 14 but adds `options.parse.smart = false` at `:25`. This is the `AGENTS.md` rule "both clients render the same Markdown feature set" verified as close but not equal, with no test that compares the two option sets.
- Both crates own a theme layer, and only one of them should. `meerai-tui/src/theme.rs` is a consumer: it imports `meerai_host::theme::{HostColor, HostPalette, NamedColor}` at `:4` and converts to `ratatui::style::Color` at `:28`. `meerai-web-client/src/theme.rs` is also a consumer, and already correct: it imports `meerai_web_protocol::ThemeTokensDto` at `:1` and resolves from it at `:64`. The server resolves the palette in `meerai-web/src/mapper.rs:1106 map_theme_tokens` and ships the tokens over the wire.
- Both crates own key handling: `meerai-tui/src/app/input.rs` (2,498 production, 59 functions) and `meerai-web-client/src/keymap.rs` (916 production, 9 functions).
- Both crates own path-reference handling: `meerai-tui` reaches `meerai_syntax::detect_path_reference`, and `meerai-web-client/src/pathlink.rs` (47 production) implements a local heuristic, which `AGENTS.md` records as a deliberate wasm-target decision.

The theme layer is therefore not a parity finding. `meerai-tui` reads `HostPalette` directly because it is a native binary. `meerai-web-client` reads `ThemeTokensDto` because it is a wasm bundle that cannot link `meerai-host`: `meerai-host/Cargo.toml:28` depends on `git2`, which binds to a C library and does not build for `wasm32`. Routing the browser through `meerai-host` would not share code; it would break the browser build.

The one real parity finding is the Markdown option drift. `meerai-tui/src/markdown.rs:19-33` sets 14 `options.extension` flags. `meerai-web-client/src/markdown.rs:10-25` sets the same 14 and adds `options.parse.smart = false` at `:25`. `AGENTS.md` requires "the same Markdown feature set" in both, and nothing tests that the two blocks match.

The parity rule is enforced by review, not by a shared seam. The findings rank by how cheap the fix is:

1. The two Markdown option blocks are the same 14 flags expressed twice, and they have already drifted once. A shared builder needs `comrak` and nothing else, so it can live in `meerai-web-protocol` only if that crate gains a `comrak` dependency — which it should not, because it is deliberately serde-only. The cheapest correct home is a small helper in `meerai-input`, which both clients already depend on and which has no other dependencies.
2. The renderers themselves cannot merge: one emits `MarkdownLine` for a terminal grid and the other emits Leptos `AnyView`. Only the parse options and inline text handling can be shared.
3. The theme layer is already split on a sound boundary and needs no change.

### 2.6 Axis five: crate boundaries

The dependency direction holds: no foundation crate depends on a surface crate. Four crates are dependency leaves (`meerai-core`, `meerai-input`, `meerai-syntax`, `meerai-web-protocol`), and `meerai-cli` is the only crate that depends on fourteen others.

Three findings cross a layer:

1. `meerai-core/src/lib.rs:10` re-exports `genai`, and `meerai-session/src/event.rs:112` implements `From<genai::chat::Usage>` while `meerai-session/src/projection.rs:36` returns `Vec<ChatMessage>`. The session crate — a data crate — depends on the provider SDK's chat types.
2. `meerai-common` depends on `meerai-policy` and `meerai-store`, and `meerai-common/src/settings.rs:655` builds policy rules. `meerai-common` sits above `meerai-policy`, not beside it.
3. `meerai-client/src/providers.rs` and `meerai-providers/src/secrets/vault.rs:461` show the intended direction done right: `meerai-client` owns the `SecretStore` trait and `meerai-providers` implements it. The consumer owns the contract.

Every crate keeps at least one test module in `src/`. Feature flags exist in four crates: `meerai-cli` (`tui`, `web`), `meerai-core` (empty default), `meerai-host` (`update`), and `meerai-tui` (`update`, which forwards to `meerai-host/update`). `meerai-cli` and `meerai-tui` both gate `check_latest` on the same flag, which is correct.

The crate count itself is decided in [§3.2](#32-small-crate-decision): all twelve crates below 3,600 production lines are kept, and none is merged.

### 2.7 Axis six: test organization

Three idioms exist in the tree:

| Idiom | Example | Crates |
| --- | --- | --- |
| Inline `#[cfg(test)] mod tests` at the end of the source file | `meerai-policy/src/engine.rs:458` | 18 of 20 |
| Sibling `*_tests` modules beside the production file | `meerai-agents/src/multi_turn_agent/tools.rs:319`, `:2086`, `:2157`, `:2219`, `:2344`, `:2394` | `meerai-agents` |
| `tests/` integration directory | `meerai-acp/tests/live_agy.rs`, `meerai-web/tests/server_parity.rs`, `meerai-tools/tests/tools.rs` | 4 crates |

The inline idiom is the house default and is healthy. The failures are specific:

- `meerai-syntax/src/tree.rs` declares two test modules in one file, at `:357` and `:524`.
- `meerai-web-protocol/src/lib.rs` declares DTOs after the test module, at `:1614-1732`. Types below tests read as dead code.
- `meerai-agents/src/multi_turn_agent/tools.rs` carries six test modules and 2,134 test lines against 317 production lines.
- `meerai-host/src/session.rs` has 484 lines and 25 public types with no test at all.
- `meerai-web/src/mapper.rs` has 1,214 production lines and 41 functions with no test at all.
- Four `meerai-tools` files have no test in any form: `repo_map/extract.rs` (1,009), `repo_map/rank.rs` (149), `workspace_search.rs` (128), and `lsp/uri.rs` (62). That is 1,348 lines.
- Seven other `meerai-tools` files have no inline test but are covered from `meerai-tools/tests/tools.rs` (747 lines, 24 test functions): `code/glob.rs` (132), `code/grep.rs` (184), `code/list_dir.rs` (98), `code/read_file.rs` (88), `code/write_file.rs` (97), `code/diff.rs` (15), and `code/mod.rs` (315). These are not gaps; the integration file is their test.

Files with no inline `#[cfg(test)]` are therefore not automatically untested. The audit counts a file as covered when a test in the same crate exercises its public behavior, which is why the second list is separated from the first.

### 2.8 Findings by layer

**Foundation.** `meerai-providers` is the best-modularized crate in the workspace: fifteen files, the largest 707 lines, and the `SecretStore` trait owned by its consumer. `meerai-web-protocol` is a single 1,734-line file holding about ninety DTOs, and splitting it into modules changes no serde tag. `meerai-policy` is 703 production lines in one `engine.rs` of 1,766, so the size problem is its test module, not its logic. `meerai-syntax/src/lex.rs` carries a 566-line keyword table at `:9-574`.

**Data.** `meerai-session/src/goal.rs` and `meerai-session/src/loop_state.rs` are near-copies. Both set the round limit to 24 (`goal.rs:3`, `loop_state.rs:3`) and the item limit to 40 (`goal.rs:4`, `loop_state.rs:4`). Five prompt builders exist twice: `render` (`goal.rs:100`), `continuation` (`goal.rs:166`), `turn_prompt` (`goal.rs:180`), `open_summary` (`goal.rs:192`), and `cap_text` (`goal.rs:227`), mirrored in `meerai-skills/src/loop_engine.rs:76`, `:158`, `:173`, `:203`, `:248`. `meerai-session/src/session.rs` is 2,438 lines with 1,286 production lines of session lifecycle in one file. `meerai-common/src/settings.rs` is 1,909 lines mixing value types, store IO, and the tighten-only merge.

**Engine.** The interceptor seam is the healthiest structure in the tree: `meerai-agents/src/intercept.rs` is 73 lines and defines the whole extension surface, `loop_interceptor.rs` is 23 lines, and `checkpoint.rs` is 38 lines, so 134 lines own every loop extension and `turn.rs` stays free of seam branches. `meerai-host/src/registry.rs` is the worst production file: 2,809 production lines, four impl blocks, 62 functions, and 73 unwraps. `meerai-tui/src/app/mod.rs:103` and `:682` suppress `large_enum_variant` because of `meerai-host/src/registry.rs:96 LiveEntry` and `:239 RegistryEventKind`, so that suppression belongs to `meerai-host`.

**Surface.** `meerai-web/src/server.rs` is 2,245 production lines with 81 functions and three `result_large_err` suppressions at `:547`, `:558`, `:572`. `meerai-web/src/mapper.rs` is 1,214 production lines with 41 functions and no test anywhere in the crate. `meerai-cli/src/main.rs` is 2,563 production lines including `main()` at `:383`; its tests live in `meerai-cli/src/tests.rs`, declared at `:14`. `meerai-tools/src/code/mod.rs` defines 31 functions in 315 lines, which is toolset wiring rather than logic, and it is the right shape.

**Client.** `meerai-tui` is 23,012 production lines over forty files, the largest crate in the workspace. `meerai-web-client/src/state.rs` is 4,368 production lines with 131 functions, and `views/modals.rs` is 3,684 production lines with 44 functions. These two files are the browser-side mirror of `meerai-tui/src/app/modals.rs` (3,467) and `app/input.rs` (2,498, 59 functions).

## 3. Planned

The backlog orders contract work before structural work. Each item states one observable done condition.

| # | Item | Axis | Crates | Effort | Risk | Done when |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | ~~Remove the two `large_enum_variant` suppressions~~ **Done** — see [§3.4](#34-completed-work) | signature | `meerai-tui` | S | Low | Done: both allows gone, `TranscriptEntry` cut from 312 to 104 bytes, guard test added |
| 2 | ~~Replace the 73 production unwraps in the registry with typed `HostError` returns~~ **Partly done** — 15 of 34 lock sites converted, see [§3.4](#34-completed-work) | error handling | `meerai-host` | M | Medium | Remaining: 19 lock sites in methods that cannot fail today, plus 33 `get`/`get_mut` unwraps |
| 3 | Unify `goal.rs` and `loop_engine.rs` into one loop model | duplication | `meerai-session`, `meerai-skills` | M | Medium | `cap_text`, `render`, `continuation`, `turn_prompt`, and `open_summary` each exist once |
| 4 | Split `meerai-host/src/registry.rs` into `state`, `events`, and `snapshot` modules | file size | `meerai-host` | M | Medium | No file in `meerai-host/src` exceeds 900 lines and the public re-export list is unchanged |
| 5 | Split `meerai-agents/src/multi_turn_agent/tests.rs` (10,018 lines) and `tools.rs` (2,134 test lines) into per-concern test modules | test organization | `meerai-agents` | M | Low | No test file in `multi_turn_agent/` exceeds 1,500 lines and each module under test has its own test file |
| 6 | Introduce parameter structs to remove the five `too_many_arguments` allows in `meerai-agents` | signature | `meerai-agents` | M | Medium | No `#[allow(clippy::too_many_arguments)]` remains in the crate |
| 7 | Introduce parameter structs to remove the five `too_many_arguments` allows in `meerai-tui` | signature | `meerai-tui` | M | Medium | No `#[allow(clippy::too_many_arguments)]` remains in the crate |
| 8 | Split `meerai-web/src/server.rs` below 900 lines and narrow its error type | file size | `meerai-web` | M | Medium | The three `result_large_err` suppressions are gone and no file in the crate exceeds 900 lines |
| 9 | Split `meerai-session/src/session.rs` into `open`, `fork`, and `query` modules | file size | `meerai-session` | M | Medium | No file in `meerai-session/src` exceeds 900 lines |
| 10 | Split `meerai-web-protocol/src/lib.rs` into domain modules and move DTOs above the tests | file size | `meerai-web-protocol` | S | Low | No DTO is declared below a test module and no serde tag changed |
| 11 | Share the Markdown parse options between both clients | duplication | `meerai-input`, `meerai-tui`, `meerai-web-client` | S | Low | Downgraded to optional by [§3.5](#35-corrected-finding-the-markdown-option-blocks-do-not-diverge): the blocks already agree in effect |
| 12 | Split `meerai-tui/src/app/modals.rs`, `app/input.rs`, `app/mod.rs`, and `ui/transcript.rs` | file size | `meerai-tui` | L | Medium | No file in `meerai-tui/src` exceeds 900 lines, including `app/mod.rs` |
| 13 | Split `meerai-web-client/src/state.rs` and `views/modals.rs` | file size | `meerai-web-client` | L | Medium | No file in `meerai-web-client/src` exceeds 900 lines |
| 14 | ~~Write the error-handling policy into `AGENTS.md` and apply it per crate~~ **Policy written**; the per-crate conversion below remains | error handling | all | M | Low | Every library crate reaches zero production `unwrap`/`expect`, or records a line-level exception with its reason |
| 15 | ~~Move `to_genai` out of `meerai-core` so the leaf crate stops re-exporting `genai`~~ **Withdrawn** | crate boundary | — | — | — | Withdrawn after measurement, see [§6.1](#61-resolved-the-meerai-core-genai-re-export) |
| 16 | Split `meerai-common/src/settings.rs` into value types and merge logic | file size | `meerai-common` | M | Medium | No file in `meerai-common/src` exceeds 900 lines and the tighten-only tests still pass |
| 17 | Add tests for the four untested `meerai-tools` files, `meerai-host/src/session.rs`, and `meerai-web/src/mapper.rs` | test organization | `meerai-tools`, `meerai-host`, `meerai-web` | M | Low | Each named file has at least one test exercising its public behavior, and `repo_map/extract.rs` is covered per language it collects |
| 18 | ~~Collapse the four macro metadata structs and remove the four `dead_code` allows~~ **Rescoped and Done** — see [§3.4](#34-completed-work) | signature | `meerai-macros`, `meerai-core` | S | Low | Done: the three `ToolsetArgs` types are genuinely distinct and stay; the dead `attrs` field and three pointless test allows are gone |

Items 1 through 3 are contract-adjacent and should land first. Items 4 through 13 are mechanical splits that can run crate by crate with one writer per crate. Items 14 through 18 are policy and coverage work that can run in parallel with any of the above.

### 3.4 Completed work

| Item | Change | Before | After |
| --- | --- | --- | --- |
| 1 | Removed `#[allow(clippy::large_enum_variant)]` at `meerai-tui/src/app/mod.rs:103` (vestigial: `ActiveModal` never tripped the lint) and boxed `TranscriptEntry::Subagent` | `TranscriptEntry` 312 bytes, 2 suppressions | 104 bytes, 0 suppressions |
| 1 | Added `app::tests::transcript_entry_stays_small` so the footprint cannot grow back silently | no guard | fails above 112 bytes |
| 18 | Deleted the dead `attrs` field from `ToolsetDerive` at `meerai-macros/src/tool/common.rs:21` | unused field with an allow | field removed, allow removed |
| 18 | Removed the three test-only `#[allow(dead_code)]` in `meerai-macros/src/tool/attribute_tool.rs` and `meerai-core/src/tools.rs:302`; one moved to the inner struct that genuinely needs it | 4 test allows | 1 inner allow, with the struct it applies to |
| 2 | Added `HostError::Poisoned` and a `lock_state()` helper, then converted the 15 registry lock sites whose function already returned `Result` | 15 `lock().unwrap()` in `Result` functions | 15 `lock_state()?` |
| 2 | `meerai-web/src/mapper.rs:643` maps `Poisoned` to 500 `Internal` | exhaustive match panic | explicit mapping |

Total suppressions in the tree went from 20 to 15. What remains: 10 `too_many_arguments`, 3 `result_large_err`, 1 `deprecated`, and 1 justified `dead_code`.

`TranscriptEntry` did not reach 64 bytes because `ToolCallEntry` is the next-largest variant and boxing it touches 44 call sites across the crate. That is item 7 territory, not a same-pass change.

Item 2 is partly done: 15 of 34 lock sites are converted. The other 19 sit in methods that cannot fail today (`snapshot`, `has_session`, `context_for`, `session_detail`, `session_summary`, `cmd_tx`, `subscribe`, the five `note_tui_*` writers, the three `emit_*` writers, `register_opened_with_activation`, `finish_user_shell`, `append_system_entry`, `shutdown_all`). Converting them means changing their signatures, which cascades into both clients. One of them, `snapshot()`, runs on every render, so deciding what a poisoned registry renders is a product decision rather than a mechanical one.

Verification for this batch: `cargo fmt --all --check` exit 0; `cargo clippy --workspace --all-targets` reports one pre-existing warning (`meerai-skills`); `cargo test --workspace` 1282 passed, 0 failed.

### 3.5 Corrected finding: the Markdown option blocks do not diverge

[§2.5](#25-axis-four-tui-and-browser-duplication) claimed the two clients "have drifted" because the browser sets `options.parse.smart = false` and the terminal does not. Measured, that is wrong twice over:

- `comrak::Options::default()` already has `parse.smart == false`; verified by running a probe against the same `comrak = "1"` the workspace pins. The browser line restates the default.
- Comparing all 15 option keys, the two blocks are identical in effective value. The browser has one redundant assignment; the terminal relies on the default.

So there is no rendering divergence and item 11 loses most of its value. The only real difference between the clients is that the terminal sets 14 extension flags and the browser sets the same 14 plus one no-op. A shared builder would be tidier, not a correctness fix, which moves item 11 from S/Low to optional.

The finding that does survive from §2.5 is the modal/logic duplication, which is untouched.

### 3.1 Items considered and rejected

| Considered | Why rejected |
| --- | --- |
| Merge `meerai-core`, `meerai-input`, `meerai-syntax`, or `meerai-web-protocol` into one crate | Each is a dependency leaf with no workspace dependencies. Merging them would give every consumer the tree-sitter and wasm surface it does not need. |
| Merge `meerai-policy` into `meerai-common` | `meerai-policy` is the best-tested crate at 67% and is the security decision surface. Merging it into the settings crate would hide it. |
| Split `meerai-tools` per toolset | The crate already splits cleanly by toolset directory (`code`, `lsp`, `web`, `repo_map`, `search_as_code`). Only `code/mod.rs` is large, and it is wiring. |
| Keep `meerai-cli` as one crate | It is the binary. Nothing depends on it, so its size costs no consumer. |
| Split `meerai-web-client` on the wasm boundary | `meerai-web-client` must not depend on `meerai-syntax` because the tree-sitter C parsers fail the wasm target. The current split already reflects that constraint. |

### 3.2 Small-crate decision

Twelve crates are below 3,600 production lines. Each gets an explicit keep-or-merge verdict, so no later round argues the crate count from scratch.

| Crate | Prod LOC | Direct dependents | Verdict | Reason |
| --- | --- | --- | --- | --- |
| `meerai-providers` | 2,164 | 1 (`cli`) | Keep | Holds the AEAD vault, the OS keychain backend, and the OAuth flow. It is the only crate that handles plaintext secrets, and isolating that code is the point. |
| `meerai-common` | 2,044 | 8 | Keep | Owns settings, definitions, hooks, and packs. Eight dependents read it, so merging it upward would move the settings type into each of them. |
| `meerai-store` | 1,361 | 8 | Keep | Owns the `Store` handle and its `Drop` hardening. Eight dependents and one dependency make it the narrowest correct seam for the database. |
| `meerai-acp` | 1,326 | 1 (`cli`) | Keep | Wraps an external process and its protocol. One dependent is expected for a boundary crate, and merging it into `meerai-agents` would put process spawning in the turn loop crate. |
| `meerai-syntax` | 1,325 | 2 (`host`, `tui`) | Keep | Zero workspace dependencies and eighteen tree-sitter grammars. `meerai-web-client` cannot depend on it, so it must stay separable from anything the browser build links. |
| `meerai-web-protocol` | 1,274 | 2 (`web`, `web-client`) | Keep | Serde-only, zero workspace dependencies. It is the only crate both the server and the wasm client can share, so its isolation is what makes parity possible. |
| `meerai-skills` | 986 | 3 (`agents`, `tui`, `cli`) | Keep | Owns the `skill-load` tool and the loop engine. Its four dependencies make it a consumer, not a leaf, so merging it would create a cycle with `meerai-agents`. |
| `meerai-client` | 961 | 3 (`providers`, `tui`, `cli`) | Keep | Owns the `ProviderCatalog` and the `SecretStore` trait. It is the contract crate for providers, and `meerai-providers` implements its trait rather than the reverse. |
| `meerai-core` | 933 | 14 | Keep | Fourteen crates depend on it directly. Merging it anywhere would be a merge into all of them. |
| `meerai-policy` | 703 | 6 | Keep | The security decision surface, at 67% test coverage. It is small because it is narrow, not because it is thin. |
| `meerai-input` | 658 | 2 (`tui`, `web-client`) | Keep | Zero-dependency text model shared by both clients. Merging it into either client would duplicate it in the other. |
| `meerai-macros` | 327 | 1 (`tools`) | Keep | A proc-macro crate. It must stay its own compilation unit because the macro crate cannot be re-exported through the crate it generates code for. |

No crate is merged in this effort. The crate count stays at twenty, and the backlog above reduces file size inside the existing crates instead.

### 3.3 Untested-file decision

[§2.7](#27-axis-six-test-organization) names six files with no test in any form. Each gets an explicit verdict here, so the coverage question is closed rather than left as a standing gap.

| File | Lines | Verdict | Reason |
| --- | --- | --- | --- |
| `meerai-web/src/mapper.rs` | 1,214 | Backlog item 17 | 24 public `map_*` functions translate domain types into `meerai-web-protocol` DTOs. Each is a pure function over data, so each is directly testable without a server, a socket, or a DOM. This is the largest untested surface in the workspace and the cheapest to close. |
| `meerai-tools/src/repo_map/extract.rs` | 1,009 | Backlog item 17, scoped | 16 `collect_*` functions, one per language, plus `extract_tags(lang, source) -> FileTags`. The signature is pure and takes a string, so one table-driven test per language closes it. It has no test today despite 15 tree-sitter grammars behind it. |
| `meerai-tools/src/workspace_search.rs` | 128 | Backlog item 17 | Four functions over the `grep-*` crates. `build_smart_matcher` and `build_searcher` are constructors, so a test needs no filesystem. |
| `meerai-tools/src/repo_map/rank.rs` | 149 | Backlog item 17 | `rank_files`, `normalize_rel`, and `render_repo_map` are pure functions over a file graph. No test today. |
| `meerai-tools/src/lsp/uri.rs` | 62 | Accepted gap | Four conversions between `Path` and `lsp_types::Uri`. The file exists only because `lsp-types` changed its URI type, and its behavior is exercised end to end whenever an LSP tool runs. A unit test here would restate the conversion table without adding a check. |
| `meerai-host/src/session.rs` | 484 | Accepted gap | 25 public types and 3 traits (`WebServerHandle`, `WebServerLauncher`, `SessionOpener`) with one function, `definition_token_estimate`. It is a declaration hub that both clients implement against. Its correctness shows up as a compile error when a client misses a method, not as a runtime assertion, so a test module would assert that a struct has fields. |

Four files go into backlog item 17 as test work. Two are accepted gaps, and the reason for each is recorded above rather than left implicit.

The judgement rule for this decision: a file earns a unit test when it holds logic that can be wrong at runtime. A conversion table whose every branch is a total function over a closed enum, and a type hub whose members are only field declarations, do not qualify. `mapper.rs` does qualify, because a field renamed in `meerai-web-protocol` would silently produce a wrong response.

The 2,277-line figure named in this decision's original scope was measured against files with no inline `#[cfg(test)]`. Seven of those files are covered from `meerai-tools/tests/tools.rs` and are not gaps, which is why the real total here is 3,046 lines across six files, not 2,277 across eleven. Of that, 2,500 lines go into backlog item 17 and 546 are accepted gaps.

## 4. Reference

### 4.1 Severity scale used in this audit

| Level | Meaning | Example in this tree |
| --- | --- | --- |
| Critical | Breaks a stated invariant or a wire contract | A cleared reasoning signature, a project file granting a permission |
| High | Hides a defect behind a suppression, or concentrates risk in a crate others depend on | 73 production unwraps in `meerai-host/src/registry.rs` |
| Medium | Costs a reader time on every change, with no correctness impact | A 1,006-line `mod.rs` that holds 960 test lines |
| Low | Style or naming drift that a single pass can clear | Two test modules in one file |

### 4.2 Measurement commands

The baseline in [§2.1](#21-baseline) is reproducible. Production lines run to the first `#[cfg(test)]` line in each file, except when that line is an external declaration (`#[cfg(test)] mod tests;`), in which case the whole file is production and the declared module counts as test. Files under a crate's `tests/` directory count as test.

```sh
find . -name '*.rs' -not -path './target/*' | xargs wc -l | sort -rn | head -20
grep -rn '#\[allow(' --include=*.rs .
```

The browser client has its own gate. It is separate from `cargo test` because the crate compiles to `wasm32`:

```sh
python3 scripts/build-web.py --dev --out /tmp/meerai-webbuild-check
```

Verified working on 2026-11-25: the `wasm32-unknown-unknown` target and `wasm-bindgen` are installed, and the build finished in 50 seconds with exit 0. Any change to `meerai-web-client`, `meerai-input`, or `meerai-web-protocol` must run this before it is called done.

## 5. Invariants

- Production lines stop at the first inline `#[cfg(test)]` item. An external `#[cfg(test)] mod X;` declares test code elsewhere, so its file stays production and the module counts as test. A crate's `tests/` directory is test.
- One test module per file. A second module goes in its own file.
- Types are declared above tests.
- A clippy suppression is a defect with an owner, not a style choice. Adding one requires an entry in the backlog.
- No `mod.rs` exceeds 200 lines. Declaration, not content, belongs there.
- 900 production lines is a target per file, not a build gate. A split that produces two coupled halves is worse than one coherent file.
- `unwrap`, `expect`, and `panic!` are allowed in production only in `meerai-tui`, `meerai-cli`, and `meerai-web-client`. Every other crate is library surface and returns a typed error. `.expect()` counts the same as `.unwrap()`, because both panic.
- One theme serves both clients: `meerai-tui` reads `HostPalette`, `meerai-web-client` reads `ThemeTokensDto` from the wire. Neither re-implements palette resolution.
- `meerai-web-client` does not depend on `meerai-syntax` or `meerai-host`. `meerai-host` pulls `git2`, which does not build for `wasm32`.
- Any change to `meerai-web-client`, `meerai-input`, or `meerai-web-protocol` is verified with `python3 scripts/build-web.py --dev` as well as `cargo test`.
- The crate count is twenty. Each small crate is kept for the reason recorded in [§3.2](#32-small-crate-decision); a merge proposal must first replace that reason.

## 6. Open questions

None. The two that were open are resolved in [§6.3](#63-resolved-the-file-size-ceiling) and [§6.4](#64-resolved-where-unwrap-is-acceptable).

### 6.1 Resolved: the `meerai-core` `genai` re-export

The question was whether `meerai-core` keeps the `genai` re-export at `lib.rs:10`, or whether `to_genai` moves out. Measured, the re-export has one consumer and the conversions have three call sites:

- `pub use genai` at `meerai-core/src/lib.rs:10` is used once in the workspace: `meerai-cli/src/main.rs:1358`, as `meerai_core::genai::Client`.
- `ToolDefinition::to_genai` at `meerai-core/src/tools.rs:227` is called at two sites: `meerai-agents/src/multi_turn_agent/turn.rs:356` and `meerai-agents/src/multi_turn_agent/compaction.rs:206`.
- `impl From<&genai::chat::ToolCall> for ToolCall` at `meerai-core/src/tools.rs:288` is called at one site: `meerai-agents/src/multi_turn_agent/turn.rs:720`.
- `meerai-core` names `genai` in exactly two places, both in `tools.rs`: the two conversion impls at `:227` and `:288`. It has no other dependency on the provider SDK.
- Four crates declare `genai` directly: `meerai-core`, `meerai-session`, `meerai-agents`, `meerai-providers`. Eleven of the fourteen `meerai-core` dependents declare it for no reason other than the re-export.

**Decision: keep the re-export and keep both conversions in `meerai-core`.** The reason is that `ToolDefinition` and `ToolCall` are defined in `meerai-core`, so the conversions belong beside them; moving them to `meerai-providers` would make `meerai-agents` depend on `meerai-providers` for a type conversion, adding an edge where there is none today. The eleven crates that inherit `genai` through the re-export gain nothing and cost little, because `genai` is already in the build for all of them through `meerai-agents` or `meerai-session`.

Backlog item 15 was written before this measurement and is withdrawn. Its stated done condition — "`meerai-core` names no `genai` type" — is not achievable without moving the two type definitions it converts.

The narrower improvement that survives: `meerai-cli/src/main.rs:1358` can name `genai::Client` directly, since `meerai-cli` already links it transitively. That removes the only consumer of the re-export and lets a future round decide whether the re-export is worth keeping. It is not a backlog item because it changes no behavior and no boundary.

### 6.2 Resolved: how `meerai-web-client` reaches the theme

`meerai-web-client` depends only on `meerai-input` and `meerai-web-protocol`. The question was whether it should reach the `meerai-host` theme seam, since `meerai-tui` already does.

**Decision: it must not, and it does not need to.** Measured:

- `meerai-host/Cargo.toml:28` depends on `git2`, which binds to a C library. `meerai-web-client` compiles to `cdylib` for `wasm32`, so it cannot link `meerai-host` at all.
- `meerai-host/src/theme.rs:1` imports `meerai_common::settings::ThemeSettings`, and `meerai-common` depends on `meerai-store`, which is `rusqlite` with a bundled C SQLite. That is a second non-wasm dependency on the same path.
- `meerai-web-client/src/theme.rs:1` imports `meerai_web_protocol::ThemeTokensDto` and resolves from it at `:64`. The server resolves the palette once, in `meerai-web/src/mapper.rs:1106 map_theme_tokens`, and ships the tokens over the wire.
- `meerai-web-client` references `HostPalette`, `HostColor`, `NamedColor`, and `ResolvedThemeHex` zero times.

So the theme layer is already a single implementation with two transports: `HostPalette` for the native TUI, `ThemeTokensDto` for the browser. There is no duplication to remove.

Backlog item 11 was rewritten as a result. It no longer mentions the theme, and it now scopes to the Markdown parse options, which is the drift that does exist. The best home for a shared builder is `meerai-input`: both clients depend on it, it has no dependencies of its own, and it is not the serde-only protocol crate.

### 6.3 Resolved: the file-size ceiling

**Decision: 900 lines is a target, not a hard cap.** A file above it is a review prompt, not a build failure, and a split that produces two coupled 500-line files is worse than one coherent 1,100-line file. The goal is that a reviewer can hold the file in mind and that `mod.rs` files stay declaration-only.

The four files that most need attention, in order of how much the size hides: `meerai-host/src/registry.rs` (2,809 production lines, 73 unwraps), `meerai-web/src/server.rs` (2,245 production lines, 81 functions), `meerai-tui/src/app/mod.rs` (2,391 production lines in a declaration file), and `meerai-web-client/src/state.rs` (4,368 production lines, 131 functions).

Backlog items 4, 8, 9, 10, 12, 13, and 16 reference "900 lines" as their done condition. Treat that as the stopping point for the work item, not as a rule the whole tree must satisfy at once.

### 6.4 Resolved: where `unwrap` is acceptable

**Decision: `unwrap` in production is acceptable only in the two client crates, because they are application ends, not libraries. Every other crate is library surface and must return a typed error.**

The reasoning is the one you gave: this repository is open source and the non-client crates are the publishable surface. `Cargo.toml:20` currently sets `publish = false` for the whole workspace, so nothing is released yet, but the library crates carry a stable API and a caller cannot recover from a panic. `meerai-tui` and `meerai-web-client` are terminal applications; a panic there is a crash with no caller to mislead.

Measured against that rule, the tree is already closer to compliant than the counts suggest:

| Group | Crates | Current production `unwrap` / `expect` |
| --- | --- | --- |
| Application ends, `unwrap` allowed | `meerai-tui`, `meerai-cli`, `meerai-web-client` | 25/3, 11/0, 0/0 |
| Library surface, zero required, already clean | `meerai-input`, `meerai-policy`, `meerai-skills`, `meerai-store`, `meerai-syntax`, `meerai-web-protocol`, `meerai-agents` | 0/0, 0/0, 0/0, 0/0, 0/0, 0/0, 0/3 |
| Library surface, zero required, work left | `meerai-host`, `meerai-web`, `meerai-client`, `meerai-providers`, `meerai-macros`, `meerai-tools`, `meerai-common`, `meerai-session`, `meerai-core`, `meerai-acp` | 73/0, 44/6, 9/2, 3/2, 3/0, 0/4, 0/4, 0/2, 0/2, 0/2 |

The library crates that are not yet at zero, by `unwrap`: `meerai-host` (73), `meerai-web` (44), `meerai-client` (9, `src/secrets.rs`), `meerai-providers` (3), and `meerai-macros` (3). By `expect`: `meerai-web` (6), `meerai-tools` (4), `meerai-common` (4), `meerai-agents` (3), `meerai-session` (2), `meerai-core` (2), `meerai-client` (2), `meerai-providers` (2), and `meerai-acp` (2).

Two clarifications that matter for the work:

`meerai-host` and `meerai-web` are library crates by this rule even though they sit next to the applications. `meerai-host` is the seam both clients consume, and `meerai-web` is the server. Neither has a caller that can recover from a panic, so both are held to the library standard. That makes them items 2 and 8, which is why they already rank at the top of the backlog.

`.expect()` counts the same as `.unwrap()` under this rule. An `expect` with a message is still a panic; the message only helps the person reading the crash. The 30 production `.expect()` calls in library crates are part of item 14.

Item 14 is split. The policy is written into `AGENTS.md` under **File size and error handling**, so it binds new code from now on. What remains is the per-crate conversion: bringing every library crate to zero production `unwrap`/`expect`, or recording a line-level exception with its reason.
