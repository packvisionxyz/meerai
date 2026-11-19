# meerai Threat Model

This document defines the threat model for Pack Vision's `meerai` agent
harness: assets, trust boundaries, attacker-controlled inputs, implemented
controls, and explicit residual risks.

| | |
| --- | --- |
| **Status** | Verified against the tree on 2026-11-19 |
| **Audience** | Security reviewers, operators, contributors |
| **Scope** | All crates; CI and release packaging (`.github/workflows/`, `deny.toml`) |
| **Related** | [harness-spec.md](harness-spec.md) · [acp.md](acp.md) · [token-efficiency.md](token-efficiency.md) · [harness-internals.md](harness-internals.md) |

## 1. Overview

meerai runs model output and repository content against the local
workspace. The security posture is layered: deterministic policy and
approval gates, in-process path guards, best-effort OS sandboxing, secret
scrubbing and redaction, heuristic injection screening, and bounded
budgets. No single layer is a guarantee; §5 states what each layer does
not cover. Extensions that run outside the harness (hooks, ACP backends,
planned MCP servers) are explicitly outside policy enforcement and are
never described as security boundaries.

## 2. Implemented controls

### 2.1 Policy engine and approval gates

`PolicyEngine` (`meerai-policy/src/engine.rs`) decides every tool call.
The decision order is:

1. **`plan` gate.** Non-`Read` effects are denied before anything else.
   `plan` is hard read-only; no rule or grant can loosen it. In `plan` the
   request also omits non-read tool schemas, so a denied tool is not
   advertised.
2. **Grants.** A recorded grant allows the call. Grant resources are
   literal (glob-escaped), never patterns. Grants are recorded only through
   an explicit approval (`t` = Turn, `a` = Session); Turn grants clear at
   turn end. Because grants are checked before rules, an existing grant
   takes precedence over a rule that matches later for the same literal
   resource (see §5.10).
3. **Rules.** Ordered `action/resource/effect` rules, last match wins,
   evaluated over default sensitive rules → user rules → agent rules.
   Wildcards on action and resource; patterns without `/` match the
   basename at any depth, patterns with `/` match the workspace-relative
   path. Actions: `read`, `edit`, `shell`, `subagent`, `webfetch`,
   `external_directory`.
4. **Tier defaults.** `ask`: Read allows, Write/Execute/External ask.
   `auto`: every tier default allows, but explicit `ask` and `deny` rules
   still win.

`Ask` emits `ApprovalRequested` and suspends the tool item until a
decision. Denied calls return a structured refusal to the model and are
logged. Outside-workspace access forces an approval; approving adds a
literal path root (`Toolset::grant_path_root`), session-scoped and cleared
on reset/rewind. Headless runs fail fast on tool approvals (exit `2`);
Spend and external-agent approvals decline (exit `1` path for Spend).

**Default sensitive-file rules.** `PolicyEngine::default_sensitive_rules`
installs `Deny` rules active in every mode, and `is_sensitive_path`
(`meerai-tools/src/code/paths.rs`) blocks the same shapes at the tool
layer: `.env*` (exact `.env.example`/`.sample`/`.template` exempt),
key/cert extensions (`.pem`, `.key`, `.p12`, `.pfx`, `.jks`, `.keystore`),
SSH private material, `.aws/credentials`, `.docker/config.json`,
`.kube/config`, `.netrc`/`_netrc`/`.npmrc`/`.pypirc`/`auth.json`/
`credentials*`/`.htpasswd`, and non-source/non-doc `*secret*`/
`*credential*` filenames.

### 2.2 Path guards

`resolve_for_read`/`resolve_for_write` canonicalize every target (through
the nearest existing ancestor for new files) and confine reads to the
workspace plus readable roots and writes to the workspace plus writable
roots; approved extra roots widen both. These are in-process policy
checks, not a kernel boundary.

### 2.3 OS sandbox (`ExecutionWorld`)

`meerai-tools/src/world.rs`, wired by `meerai-cli`. `code-shell` runs with
a `WorkspaceWrite` policy and `sac-run_script` with `ReadOnly`; both pass
`allow_network: false`.

- **macOS:** `SeatbeltWorld` via `/usr/bin/sandbox-exec`. Read-all except
  masked sensitive paths; writes only to workspace roots, writable roots,
  and temp (`/dev/null` only in `ReadOnly`); network denied.
- **Linux:** `BubblewrapWorld` via `bwrap`: `--ro-bind / /`, `--dev`,
  `--proc`, `--tmpfs /tmp`, `--bind` per writable root, `--unshare-net`,
  `--die-with-parent`.
- **Sensitive-path masking:** both worlds deny reads (and writes in
  `WorkspaceWrite`) on `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`,
  `~/.docker`, `.netrc`, `_netrc`, `.npmrc`, `.pypirc`, `.git-credentials`,
  and the meerai config/data dirs, plus bounded workspace sensitive files
  (`is_sensitive_path`, depth ≤ 5, max 256 files, including gitignored
  `.env*`). bwrap masks with `--tmpfs`/`--ro-bind /dev/null` after `--bind`.
- **Fallback:** `HostWorld` (path guards only) with a startup stderr
  warning when the probe fails or `MEERAI_DISABLE_SANDBOX` is set.
  `sandbox.require_for_shell`/`require_for_scripts` refuse `HostWorld`
  (fail closed). The CLI always injects a detected world; a library
  embedder that constructs `CodeTools::new` without one gets a silent
  `HostWorld` (§5.3).
- **Limits:** captured stdout/stderr capped at 1 MiB per stream while
  draining; on Unix `pre_exec` sets `RLIMIT_FSIZE` (1 GiB) and
  `RLIMIT_CORE` (0).
- **Strict environment:** `sandbox.strict_env` clears the child
  environment and keeps only `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`,
  `TERM`, `TMPDIR`, `PWD`, `LANG`, and `LC_*`; the secret scrub also runs
  on the child environment. Tighten-only from a project file.
- **Interpreter hardening:** `sac-run_script` runs `python3 -I -B`; Node
  gets `--permission` (or `--experimental-permission`) with
  `--allow-fs-read=<cwd>` when the probe succeeds. Script `path` arguments
  pass `resolve_for_read`.

### 2.4 Secret handling

- **Environment scrub** (`scrub_command_env`, `meerai-tools/src/code/shell.rs`):
  before spawning `shell`, `sac-run_script`, or an LSP server, the child
  environment drops 25 named credential variables (provider keys,
  `GITHUB_TOKEN`, `GH_TOKEN`, `GITLAB_TOKEN`, `NPM_TOKEN`, `PYPI_TOKEN`,
  `CARGO_REGISTRY_TOKEN`, `HF_TOKEN`, `VAULT_TOKEN`, `SLACK_TOKEN`,
  `DISCORD_TOKEN`, AWS keys, and others) and any name ending in one of 17
  suffixes (`_API_KEY`, `_ACCESS_KEY`, `_API_TOKEN`, `_ACCESS_TOKEN`,
  `_AUTH_TOKEN`, `_SESSION_TOKEN`, `_REFRESH_TOKEN`, `_BEARER_TOKEN`,
  `_BOT_TOKEN`, `_SECRET`, `_SECRET_KEY`, `_CLIENT_SECRET`, `_PRIVATE_KEY`,
  `_PASSWORD`, `_PASSWD`, `_CREDENTIAL`, `_CREDENTIALS`),
  case-insensitively, plus caller-configured extras.
- **Redaction** (`redact_run`/`redact_text`, `meerai-agents/src/redact.rs`):
  9 static regex patterns replace matches with `[REDACTED:<kind>]` —
  Anthropic `sk-ant-`, OpenAI/OpenRouter `sk-`, Google `AIza`, GitHub
  `ghp_`/`github_pat_`, AWS `AKIA`, Slack `xox*`, `Bearer` tokens, and PEM
  private keys. Applied to observation stdout/stderr/hint, `ToolOutput`
  text, `FileChange` diffs, and dispatch errors, immediately after a tool
  run and before spill, the session log, UI events, and model text. The
  original value is discarded; there is no log-only copy. Paths are not
  redacted.
- **Credential storage** (`meerai-providers/src/secrets.rs`):
  `KeychainSecretStore` uses the macOS Keychain (`security`) or Linux
  Secret Service (`secret-tool`) under service `meerai`, deleting stale
  `auth.json` entries. Fallback is `FileSecretStore` (`auth.json`, `0600`
  on Unix, dir `0700`), also forced by `MEERAI_DISABLE_KEYCHAIN`. ChatGPT plan
  OAuth credentials (`type:"oauth"`: access token, rotating refresh token,
  issued client id, scopes) always live in `auth.json`, never in the keychain;
  every rewrite takes `auth.json.lock`, and `list_providers`/`auth list` never
  print a token. Key resolution order: stored credential (API key or OAuth
  access token) → configured `api_key` → provider env vars from the bundled
  catalog. Config secrets support `${VAR|default}` and `{env:VAR}` expansion.

### 2.5 Injection screening

`screen_run`/`screen_untrusted_text` (`meerai-agents/src/screen.rs`) scan
tool output, repository instructions, and definition text with 10 static
heuristics: five instruction-override phrases (`ignore previous
instructions`, `disregard prior instructions`, `forget previous
instructions`, `override system prompt`, `new system instructions`) and
five spoofed role markers (`<|im_start|>system`, `<|system|>`, `[INST]`,
`### System:`, `<system-prompt>`). Flagged text is wrapped in
`<untrusted_content>` with an explicit warning header. Best-effort, not a
guarantee (§5.1).

### 2.6 Untrusted-input boundaries

- **Repository instructions.** `compose_system_prompt`
  (`meerai-agents/src/prompt.rs`) reads root `AGENTS.md` and
  `.meerai/instructions.md` (32 KiB per file, 64 KiB block, truncation
  markers) and screens them before insertion. Scoped directory `AGENTS.md`
  files (16 KiB, max 8 per turn) are injected as `RuntimeContext` user
  messages when an authorized tool touches their directory.
- **Agent and skill definitions.** `load_definitions`
  (`meerai-common/src/definitions.rs`) is read-only discovery at the
  project root: `.agents/agents/*.md` and `.agents/skills/*/SKILL.md`
  before `.claude/...`; `.agents` wins duplicate ids; symlinks are skipped
  with a warning; only `SKILL.md` text is read; `.meerai/agents` and
  `.meerai/skills` are never scanned or created. Definitions are **prompt
  text only**: frontmatter `tools`, `permissions`, and `allowed-tools`
  warn and are ignored; no tool registration, permission mapping, script,
  or sandbox implication. Startup text is capped at 16 KiB per definition
  and 32 KiB total. An agent `model` containing `:` is ignored (no
  provider switch).
- **Project configuration.** `merge_project_settings`
  (`meerai-common/src/settings.rs`) is tighten-only: permissions merge as
  project `ask` → user → project `deny` (`allow` dropped with a warning;
  invalid shapes drop the block); `context` overrides field by field with
  `max_session_tokens`/`max_turn_secs` clamped (a project cannot raise a
  cap, disable the turn clock, or introduce a clock the user did not set);
  `sandbox` flags can only tighten; `active_provider`, `active_model`,
  `providers`, `approval_mode`, `sessions`, and `hooks` blocks are ignored
  with warnings. A project `recovery_model` containing `:` is dropped. A
  malformed file warns and is ignored; startup never aborts.
- **Tool arguments.** Deserialized against schemars draft-07 schemas
  before execution; the lenient parser repairs malformed JSON and coerces
  values to the schema (`meerai-core/src/lenient.rs`). Invalid arguments
  return `ToolError::WrongArguments` without invoking the tool and are not
  retried.

### 2.7 Hooks

User hooks (`~/.config/meerai/hooks.json`) and project hooks
(`<project-root>/.meerai/hooks.json`) run shell commands on 13 lifecycle
events: `SessionStart`, `UserPromptSubmit`, `PreToolUse`,
`PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `Stop`,
`PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, `SessionEnd`,
`TurnFailed`.

- **Trust gate.** Project hooks load only with
  `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, user `hooks.trust_project: true`,
  or the canonical project root in user `hooks.trusted_projects`
  (`meerai hooks trust|untrust|status`). The TUI path prompts once on a
  terminal and defaults to no. A project file cannot trust itself: its
  `hooks` settings block is discarded during merge. `hooks.enabled: false`
  disables both sources.
- **Execution.** Entries are
  `{ event, matcher, if, command, baseline_file, timeout_ms, fail_closed }`.
  Matching hooks run concurrently with a 10 s default timeout (60 s max;
  out-of-range values warn and reset). JSON on stdin carries `event`,
  `session_id`, `turn_id`, `tool_name`, `tool_input`, `tool_output`,
  `prompt`, `error`, `cwd`. Exit `0` passes (stdout JSON may carry a
  decision), exit `2` blocks with the stderr reason, other codes pass with
  a recorded `exit code N` reason. Spawn/timeout failures pass unless
  `fail_closed`. Decisions: `hookSpecificOutput.permissionDecision` for
  `PreToolUse`; `decision: "block"` for `Stop` (continues the turn once).
  `UserPromptSubmit` block fails the turn with `ErrorCode::Config`.
  `TurnFailed` is observational.
- **Tighten-only.** A hook `Allow` converts a tier-default `Ask` to
  `Allow` only when no explicit rule matched and the path is inside the
  workspace; it cannot override `plan`, an explicit `deny`/`ask` rule, or
  the outside-workspace approval. `Ask` can raise `Allow` to `Ask`;
  `Deny`/`Block` always deny, including reads in `plan`.
- **Logging.** Every firing logs `HookFired` (with `HookDetail`) and emits
  `AgentEvent::HookCompleted`; the TUI shows a System line and headless
  NDJSON carries the event. `meerai hooks verify` installs a user `Stop`
  hook with a per-project failure baseline (first failing run records, later
  runs block only new names, a pass clears).
- **Boundary.** Hooks run host commands with the user's privileges
  **outside** `ExecutionWorld`. Hook output is untrusted and truncated,
  not redacted (§5.8).

### 2.8 Subagents

`SubagentToolset` (`meerai-agents/src/subagent.rs`) enforces structural
containment:

- **Depth-1 isolation:** `ChildToolsetFilter` rejects any `subagent-*`
  tool; children cannot spawn children.
- **Read-only built-ins:** `explore` (4 steps) and `plan` (16 steps) are
  restricted to `READ_ONLY_SUBAGENT_TOOLS` (`code-read_file`,
  `code-list_dir`, `code-grep`, `code-glob`, `retrieval-repo_map`,
  `lsp-go_to_definition`, `lsp-find_references`, `lsp-get_diagnostics`)
  and forced into `ApprovalMode::Plan`. `build` (32 steps) and `general`
  (16 steps) write under the parent's mode. Step budgets clamp to the
  parent's `max_steps_per_turn`.
- **Custom definitions:** all repository-defined agents run read-only
  (`Plan`, read-only tools, 16 steps) regardless of frontmatter.
- **Forwarded approvals and inherited world:** child approvals route to
  the parent overlay; the parent session token cap propagates; child tools
  inherit the parent's `ExecutionWorld`. Usage rolls up into parent
  counters. `question-ask` and `acp-run` are not registered for children.

### 2.9 ACP external backends

`meerai --acp` spawns `agy acp`; `AcpWorker` mirrors turns into the session
log and forwards permission requests as `ApprovalKind::ExternalAgent`.
The native agent can delegate one task via `acp-run` in a child session.
The decision is **advisory**: the external process executes its own tools
under its own policies. meerai's rules, path guards, `ExecutionWorld`, and
redaction do not apply; the adapter inherits provider credentials. `plan`
refuses `--acp` and denies `acp-run`. Full boundary: [acp.md](acp.md) §5.

### 2.10 Budgets and circuit breakers

`max_steps_per_turn` (default 32; the final step strips tools and asks for
a summary), per-tool timeout (default 120 s), per-turn wall clock
(`max_turn_secs`, default 1800, `0` disables; subagent children run with
`0`), and an optional session token cap with a Spend approval pause bound
runaway execution. Breakers: the 3rd identical failing call injects a
diagnostic and the 4th fails the turn; the 3rd identical batch injects a
diagnostic and the 5th fails; `NoProgress` diagnoses at 8 write-less steps
and fails at 12 (disabled in `plan` and without a write tool). Provider
retries are status-classified, 3 attempts, jittered backoff; one bounded
recovery turn follows a retryable failure.

### 2.11 Session privacy and retention

Session directories are `0700` and files `0600` on Unix, re-tightened on
open (including legacy files and blobs). `meta.json` records cwd, git
branch/SHA, title, model, approval mode, lineage, and timestamps.
Retention is user-controlled only: manual `sessions prune`, opt-in
`sessions.auto_prune_days`/`keep` (active session always spared), and
startup cleanup of empty sessions older than six hours that are not open.
A project file cannot control retention. Windows has no mode enforcement (§5.4). Cross-process `SessionLock`
(`session.lock`, `0600` on Unix) uses `std::fs::File::try_lock` so two
processes cannot write or delete the same live session concurrently;
`Session::fork_stored` reads events without taking the write lock.

### 2.12 Browser companion server (`meerai-web`, `web` feature)

- **Loopback only:** `meerai serve` and TUI `/serve` bind strictly to
  `127.0.0.1` (default port `28860` unless overridden).
- **Mandatory launch Basic auth:** Launch surfaces require Basic auth
  (`ServeAuth::Basic`, constant-time secret compare via `subtle`), defaulting to
  local credentials (`meerai` / `12345678`) unless `--password`,
  `MEERAI_SERVE_SECRET`, `--auth`, or the `ServeDialog` field sets another
  password (never written to session logs or browser storage). Server instances configured with
  `ServeAuth::None` fail closed (`401`) on privileged parity endpoints; the
  snapshot, SSE stream, session creation, and prompt routes stay open. Both
  launch surfaces always use `ServeAuth::Basic`.
- **CSRF / cross-origin guards:** Mutating `POST`/`DELETE` `/api/*` requests
  verify loopback `Host` (`127.0.0.1:<port>` or `localhost:<port>`), a same-origin
  `Origin`, and an `application/json` content type.
- **No secret exposure in HTTP or Wasm:** API keys and OAuth tokens are never
  returned by `/api/providers` or `/api/snapshot` (`configured` + status badge
  only) and never persisted in browser storage.
- **DOM output hardening:** The Leptos client renders model, tool, file, diff,
  title, and repository text as text nodes. It does not use `inner_html` for
  untrusted content. Markdown HTML stays text, links are scheme-checked, images
  render as links, and theme values are validated before they become CSS custom
  properties. The CSP restricts scripts and network origins.

### 2.13 Supply chain

- **CI audit:** `deny.toml` + `EmbarkStudios/cargo-deny-action@v2` run
  `--all-features check advisories licenses bans sources` on every
  push/PR, denying known vulnerabilities and yanked crates and enforcing
  permissive OSI licenses.
- **Release integrity:** five targets build with per-archive `.sha256`
  files and `SHA256SUMS.txt`; installers verify checksums.
- **Conditional signing:** `release.yml` signs macOS binaries
  (`codesign` + `notarytool`) when `APPLE_CERTIFICATE` and notarization
  secrets are set, and Windows binaries (`signtool`, SHA-256 + timestamp)
  when `WINDOWS_CERTIFICATE` is set. Without those secrets, archives
  publish unsigned (§5.6).

## 3. Planned controls

| Item | Description | Gate |
| --- | --- | --- |
| MCP server policy | External MCP servers (processes or remote endpoints) must pass `PolicyEngine` approval gates; their schemas, descriptions, and outputs are untrusted | MCP client milestone |
| Windows sandbox | Job Objects / AppContainer for `ExecutionWorld`; today Windows always uses `HostWorld` | Platform work |
| Linux in-process fallback | Landlock + seccomp when `bwrap` is absent | Platform work |
| LSP sandboxing | Spawn language servers inside `ExecutionWorld`; today they are env-scrubbed only (request and write timeouts exist) | Tools work |
| Read allowlist mounts | Optional allowlist-only reads for `ReadOnly` sandbox policies | Read-prefix matrix ([token-efficiency.md](token-efficiency.md) §5) |
| Release signing secrets | Populate `APPLE_CERTIFICATE` / `WINDOWS_CERTIFICATE` in GitHub Actions | Operator configuration |
| Sandbox warning in TUI | Surface `ExecutionWorld::warning()` in the statusline | TUI design |

## 4. Reference

### 4.1 Protected assets

| Asset | Location / surface | Primary threats |
| --- | --- | --- |
| Workspace code & files | Workspace root + `writable_roots` | Destructive modification, malicious injection, exfiltration |
| Host filesystem & network | Paths outside the workspace, local services, external endpoints | Arbitrary read/write via `shell`, exfiltration, SSRF |
| Provider API keys | OS keychain, `auth.json` (`0600`), process env | Leakage via child processes, tool output, logs, model context |
| ChatGPT plan OAuth tokens | `auth.json` (`0600`), process memory | Refresh-token theft, token in an error body or log, shared-machine access |
| Repository & user secrets | `.env*`, SSH keys, `.aws/credentials`, `.kube/config`, `.docker/config.json`, `*.pem`/`*.key` | Disclosure into prompts/transcripts or exfiltration |
| Session transcripts & blobs | `$XDG_DATA_HOME/meerai/sessions/<id>/` (`0700`/`0600` on Unix) | Local multi-user disclosure, secret persistence |
| Supply chain & artifacts | `Cargo.lock`, workspace crates, release binaries | Vulnerable/yanked/unlicensed deps, tampered binaries |

### 4.2 Trust boundaries

1. **User & user configuration (trusted):** CLI flags, approval decisions,
   user config (`~/.config/meerai/`).
2. **Repository & workspace content (untrusted):** source files,
   `.meerai/settings.yml`, `AGENTS.md`, `.meerai/instructions.md`, scoped
   `AGENTS.md`, `.agents/` and `.claude/` definitions. A checked-out
   repository may be attacker-controlled.
3. **LLM responses & tool arguments (untrusted):** model text and calls
   may carry indirect prompt injection. Every call crosses the policy and
   schema boundary before execution.
4. **Child processes & tool outputs (untrusted):** `code-shell`,
   `sac-run_script`, LSP servers, and file reads cross back into the model
   context and the log through scrub → redact → screen → spill.
5. **External extensions (outside harness enforcement):** hooks, ACP
   backends, and planned MCP servers run outside `CodeTools`/
   `PolicyEngine` enforcement.

### 4.3 Tool-output pipeline

Every `ToolRun` passes a fixed pipeline in
`meerai-agents/src/multi_turn_agent/tools.rs` before spill, logging, UI
emission, or model consumption: environment scrub at spawn (§2.4) →
`redact_run` → `screen_run` → spill of large outputs
(`spill_max_inline_tokens`, default 12 500) → `ToolResult` log →
`ItemCompleted` → model text. Dispatch errors go through `redact_text`.

## 5. Residual risks and non-guarantees

1. **Best-effort redaction and screening.** Redaction uses 9 static regex
   patterns with no entropy detection: novel token formats, encoded or
   split secrets, and file paths are not redacted. Screening uses 10
   English/token-marker heuristics: paraphrased, encoded, split, or
   non-English injections do not match, and `<untrusted_content>` wrapping
   does not guarantee the model ignores embedded instructions.
2. **`auto` mode.** `auto` allows every tier default (explicit `ask`/`deny`
   rules, sensitive defaults, and sandbox confinement still apply).
   Malicious repository instructions or model hallucinations can modify or
   delete workspace files and run workspace-scoped commands without
   confirmation.
3. **Sandbox fallback.** When the probe fails (for example an unprivileged
   container without user namespaces) or `MEERAI_DISABLE_SANDBOX` is set,
   execution degrades to `HostWorld` with a warning unless
   `require_for_shell`/`require_for_scripts` fail closed. Under `HostWorld`
   there is no kernel filesystem or network isolation (Node `--permission`
   still restricts JS reads when available). A library embedder that builds
   `CodeTools::new` without injecting a world gets `HostWorld` with no
   warning; the CLI always injects one.
4. **Windows.** No OS sandbox (always `HostWorld`) and no ACL equivalent of
   the Unix `0700`/`0600` enforcement on sessions and `auth.json`.
5. **Broad read visibility; unscoped LSP.** Both sandbox worlds mask the
   listed sensitive locations but allow other host reads so toolchains
   work; files beyond the scan cap/depth remain readable. LSP servers are
   env-scrubbed but run unsandboxed; requests time out after 15 s and a
   stalled stdin write retires the server, but there is no kernel
   isolation for the server process.
6. **Unsigned releases.** Until signing secrets are configured, macOS and
   Windows archives publish unsigned; Gatekeeper/SmartScreen may warn.
   Verify `SHA256SUMS.txt`.
7. **Keychain fallback.** Without Secret Service/`secret-tool` (or with
   `MEERAI_DISABLE_KEYCHAIN`), credentials fall back to plaintext
   `auth.json` (`0600`) or environment variables.
8. **Hooks are unsandboxed host commands.** A trusted project hook or a
   user hook can read, write, or exfiltrate anything the user can; `plan`
   and policy rules do not restrict the hook itself. Hooks fail open by
   default (set `fail_closed` on `PreToolUse` to deny on failure). Hook
   stdout/stderr and reasons are untrusted and truncated, not redacted.
9. **ACP is outside local policy.** Rules, grants, path guards, redaction,
   and the sandbox do not constrain the external agent; permissions are
   advisory; the adapter inherits credentials and can reach the network.
   The badge and header are an external-engine warning, not a safety
   claim.
10. **Grants precede rules.** A grant is recorded only through an explicit
    approval, but it is checked before rules: if rules later change (for
    example `/reload` adds a `deny`), an existing Session grant still
    allows the same literal resource until it is cleared. Grants never
    widen to globs and cannot loosen `plan`.
11. **Browser credentials and DOM output.** Basic auth over loopback HTTP is
    not encrypted; a local root user or a process with packet-capture
    privileges can read the credential or the traffic. The browser caches the
    credential until it closes, and there is no logout; restarting the server
    with a new secret revokes it. The Leptos client renders a DOM path. Leptos
    escaping, the HTML-as-text rule, link scheme checks, and the CSP reduce the
    injection risk, but a bug in a custom renderer or a missed raw-HTML path
    could still inject markup.
