# meerai Threat Model (M11)

This document defines the threat model for Pack Vision's `meerai` agent harness. It documents assets, trust boundaries, attacker-controlled input surfaces, landed M11 mitigations, planned integrations, and explicit residual risks. Statements about shipped behavior reflect the current repository implementation; planned integrations are marked explicitly.

---

## 1. Scope, Assets, and Trust Boundaries

### 1.1 Scope

In scope:
- Interactive TUI (`meerai-tui`) and headless CLI (`meerai-cli`) execution modes.
- Multi-turn agent loop, context/prompt composition, redaction, injection screening, and subagent orchestration (`meerai-agents`).
- Policy engine and approval gating (`meerai-policy`).
- Workspace coding, retrieval, LSP, and `search_as_code` tools plus OS sandbox wrappers (`meerai-tools`).
- Provider credential storage (`meerai-providers`), session persistence (`meerai-session`), and settings/definition loading (`meerai-common`).
- CI dependency auditing and release artifact packaging/signing (`.github/workflows/ci.yml`, `.github/workflows/release.yml`, `deny.toml`).

### 1.2 Protected Assets

| Asset | Location / Surface | Primary Threats |
| --- | --- | --- |
| Workspace code & files | Workspace root and configured `writable_roots` | Unintended/destructive modification, malicious code injection, exfiltration |
| Host filesystem & network | Paths outside workspace, local services, external endpoints | Arbitrary file read/write via `shell`, data exfiltration, SSRF |
| Provider API keys | OS keychain (`KeychainSecretStore`), `auth.json` (`0600`), process env vars | Leakage via child processes, tool output, session logs, or model context |
| Repository & user secrets | `.env*`, SSH private keys, `.aws/credentials`, `.kube/config`, `.docker/config.json`, `*.pem`/`*.key` | Disclosure into LLM prompts/transcripts or exfiltration via `shell` |
| Session transcripts & blobs | `$XDG_DATA_HOME/meerai/sessions/<id>/` (default `~/.local/share/meerai/sessions`; `0700` dirs, `0600` files on Unix) | Local multi-user disclosure, secret persistence in logs or spill blobs |
| Supply chain & release artifacts | `Cargo.lock`, workspace crates, release binaries (`meerai`, `meerai.exe`) | Vulnerable/yanked/unlicensed dependencies, tampered release binaries |

### 1.3 Trust Boundaries

1. **User & User Configuration (`Trusted`)**: CLI flags, interactive approval decisions (`AgentCommand::Approve`), and user config (`~/.config/meerai/settings.yml`).
2. **Repository & Workspace Content (`Untrusted`)**: Source files, `.meerai/settings.yml`, `AGENTS.md`, `.meerai/instructions.md`, scoped `AGENTS.md`, and `.agents/` / `.claude/` agent and skill definitions. A cloned or checked-out repository may be attacker-controlled.
3. **LLM Responses & Tool Arguments (`Untrusted`)**: Model-emitted text and tool calls may be influenced by indirect prompt injection from repository content, command output, or external data. All tool calls cross a policy and schema boundary before execution.
4. **Child Processes & Tool Outputs (`Untrusted`)**: Output from `code-shell`, `sac-run_script`, LSP servers, and file reads is untrusted data crossing back into the model context and session log.
5. **External Extensions (`Outside Harness Enforcement`)**: Hooks (M7), planned MCP servers, and ACP external backends (M13/M14) run outside `CodeTools` / `PolicyEngine` enforcement unless explicitly sandboxed.

---

## 2. Attacker-Controlled and Untrusted Inputs

### 2.1 Repository Instructions and Definitions

- **Root & scoped instructions**: `compose_system_prompt` (`meerai-agents/src/prompt.rs`) reads root `AGENTS.md` and `.meerai/instructions.md` (capped at 32 KiB per file, 64 KiB total block). Scoped directory `AGENTS.md` files (16 KiB cap, max 8 per turn) are injected as `RuntimeContext` user messages.
- **Agent and skill definitions (M12)**: `load_definitions` (`meerai-common/src/definitions.rs`) performs read-only discovery under `.agents/agents/*.md`, `.claude/agents/*.md`, `.agents/skills/*/SKILL.md`, and `.claude/skills/*/SKILL.md` at the project root.
  - Definitions are **prompt text only**: they cannot execute scripts, register tools, or alter sandbox/policy settings.
  - Frontmatter fields `tools`, `permissions`, and `allowed-tools` emit a warning and are ignored.
  - Symlinks are skipped with a warning; supporting files inside skill directories are not read; `.meerai/agents` and `.meerai/skills` are never scanned.
- **Injection screening**: Root instruction files (`prompt.rs`), startup/TUI definition instructions (`multi_turn_agent/instructions.rs`, `multi_turn_agent/tools.rs`), and tool outputs pass through `screen_untrusted_text` (`meerai-agents/src/screen.rs`).

### 2.2 Project Configuration (`.meerai/settings.yml`)

- `merge_project_settings` (`meerai-common/src/settings.rs`) enforces a **tighten-only** merge over user settings:
  - `active_provider`, `active_model`, `providers`, `approval_mode`, and `sessions` (retention) are ignored with warnings so a repository cannot redirect traffic, weaken approvals, or alter pruning.
  - Project `permissions` entries with `allow` are dropped with a warning. Project `ask` rules are prepended *before* user rules (so a project `ask` cannot override a user `deny`), and project `deny` rules are appended *after* user rules (last-match-wins).
  - `context.max_session_tokens` and `context.max_turn_secs` are clamped against user caps; a project config cannot raise token limits or set `0` to disable the turn wall clock.

### 2.3 Model Output and Tool Dispatch

- **Schema validation**: Tool parameters are deserialized via `serde_json` against `schemars` draft-07 schemas before execution; invalid arguments return `ToolError::WrongArguments` without invoking the tool.
- **Policy gate (`PolicyEngine`, `meerai-policy/src/engine.rs`)**:
  - `ApprovalMode::Plan` is hard read-only: non-`Read` effects are denied before rules and grants are checked.
  - Ordered rules match `(action, glob resource, effect)` with last-match-wins semantics.
  - `Ask` suspends execution and emits `ApprovalRequested`; interactive TUI prompts the user (`None` / `Turn` / `Session` grant scopes, with literal resource matching for grants), while headless runs fail fast with exit code `2` (or exit `1` on declined `Spend` cap).
- **Budgets & circuit breakers**: `max_steps_per_turn`, per-tool timeouts (`120_000` ms default for `shell`), `max_turn_secs` (`1800` s default with harness), `max_session_tokens` spend pause, and repeated-failure/no-progress circuit breakers limit runaway execution.

### 2.4 Tool and Command Output

Every `ToolRun` passes through a fixed pipeline in `meerai-agents/src/multi_turn_agent/tools.rs` before spill, session logging, UI event emission, or model consumption:

1. **Environment scrubbing (`scrub_command_env`, `meerai-tools/src/code/shell.rs`)**: Before spawning `shell`, `SearchAsCodeToolset` (`meerai-tools/src/search_as_code/mod.rs`), or `LspClient` (`meerai-tools/src/lsp/client.rs`), the child environment strips 11 known provider variables (`MEERAI_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `OPEN_ROUTER_API_KEY`, `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPSEEK_API_KEY`, `XAI_API_KEY`, `MISTRAL_API_KEY`, `GROQ_API_KEY`, `AZURE_OPENAI_API_KEY`) and any variable ending in `_API_KEY`, `_API_TOKEN`, `_ACCESS_TOKEN`, `_SECRET`, `_SECRET_KEY`, or `_PRIVATE_KEY`, plus any caller-configured `removed_env_vars`.
2. **Secret redaction (`redact_run` / `redact_text`, `meerai-agents/src/redact.rs`)**: Best-effort regex replacement (`[REDACTED:<kind>]`) on observation `stdout`/`stderr`/`hint`, `ToolOutput`, `FileDiff::unified_diff`, and dispatch failure messages. Matches OpenAI/OpenRouter (`sk-`), Anthropic (`sk-ant-`), Google (`AIza`), GitHub (`ghp_`, `github_pat_`), AWS (`AKIA`), Slack (`xox[baprs]-`), `Bearer` tokens, and PEM private keys. The unredacted value is discarded.
3. **Heuristic injection screening (`screen_run` / `screen_untrusted_text`, `meerai-agents/src/screen.rs`)**: `detect_injection_signals` scans for override phrases (e.g., `ignore_previous_instructions`, `disregard_prior_instructions`, `forget_previous_instructions`, `override_system_prompt`, `new_system_instructions`) and spoofed role markers (`<|im_start|>system`, `<|system|>`, `[INST]`, `### System:`, `<system-prompt>`). Flagged text is wrapped in `<untrusted_content>` tags with an explicit warning header.
4. **Disk spill (`spill_observation`, `meerai-agents/src/multi_turn_agent/tools.rs`, `meerai-session/src/blob.rs`)**: Large outputs from spill-enabled tools exceeding `spill_max_inline_tokens` (default `12_500`) are written to session blob storage after redaction and screening.

### 2.5 Subagents (M8)

`SubagentToolset` (`meerai-agents/src/subagent.rs`) enforces structural containment on child agents:

- **Depth-1 isolation**: `ChildToolsetFilter` rejects any `subagent-*` tool, preventing recursive subagent spawning.
- **Read-only tool whitelist**: Built-in `explore` and `plan` subagents are restricted to `READ_ONLY_SUBAGENT_TOOLS` (`code-read_file`, `code-list_dir`, `code-grep`, `code-glob`, `retrieval-repo_map`, `lsp-go_to_definition`, `lsp-find_references`, `lsp-get_diagnostics`) and `EffectClass::Read`.
- **Forced `Plan` mode**: Read-only built-ins and **all** repository-defined custom subagents are forced into `ApprovalMode::Plan` regardless of the parent's approval mode.
- **Forwarded approvals & inherited world**: Child approval requests route to the parent via `SubagentBridge` (`ForwardedApproval`), parent session token caps propagate, and child tools inherit the parent's `ExecutionWorld` via `SubagentToolFactory` in `meerai-cli/src/main.rs`.

### 2.6 Hooks (M7), Planned MCP Servers, and ACP Backends (M13/M14)

- **Hooks (M7, landed v1 — `docs/harness-spec.md` §9.2)**: `~/.config/meerai/hooks.json` and `.meerai/hooks.json` define shell commands triggered by twelve lifecycle events. Project hooks load only with `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, user `hooks.trust_project: true`, or the canonical project root in user `hooks.trusted_projects` (`meerai hooks trust|untrust|status`); the TUI path prompts once on a terminal and defaults to no. The explicit opt-in remains the boundary. Hooks run concurrently with a 10 s default timeout (60 s max), receive JSON on stdin, log every firing as `HookFired`, and fail open unless `fail_closed` is set. Project hooks settings in `.meerai/settings.yml` are ignored. Hooks may tighten policy but cannot loosen `plan`, an explicit `deny`/`ask` rule, or the outside-workspace approval.
- **MCP servers (planned)**: External MCP servers run as separate processes or remote endpoints; their tool schemas, descriptions, and outputs are untrusted and must not bypass `PolicyEngine` approval gates.
- **ACP external backends (M13 landed for `agy`; M14 planned — `docs/acp.md`)**: `meerai --acp` spawns `agy acp`; `AcpWorker` maps typed events, mirrors turns into the session log, and forwards permission requests through the approval overlay as `ApprovalKind::ExternalAgent`. The native agent can call `acp-run` to delegate one task to `agy` in a child session; the same advisory permission path applies. The decision is advisory. The external process executes its own tools under its own policies; `meerai`'s `PolicyEngine` rules, `CodeTools` path guards, `ExecutionWorld`, and `redact_run` do **not** apply. `plan` refuses `--acp`. `session/load` replays remote history before `Connected`; the worker drops it and relies on the local transcript. OpenCode/Claude presets and M14 remain backlog.

---

## 3. Controls & Mitigations Landed (M11)

| Control | Implementation | Security Effect |
| --- | --- | --- |
| OS command sandbox (`ExecutionWorld`) | `meerai-tools/src/world.rs`, wired in `meerai-cli/src/main.rs` | `detect_execution_world(true)` probes `/usr/bin/sandbox-exec` (`SeatbeltWorld` on macOS) and `bwrap` (`BubblewrapWorld` on Linux). Both `code-shell` (`WorkspaceWrite`) and `sac-run_script` (`ReadOnly`) run with `allow_network: false`. Seatbelt denies default/network and restricts writes to `/dev/null`, `temp_dir`, `workspace_root`, and `writable_roots`. Bubblewrap mounts `/` read-only (`--ro-bind / /`), isolates network (`--unshare-net`), binds only `workspace_root` + `writable_roots`, and sets `--die-with-parent`. Falls back to `HostWorld` with a startup stderr warning if unavailable or disabled via `MEERAI_DISABLE_SANDBOX`. |
| In-process path guards | `meerai-tools/src/code/paths.rs` | `resolve_for_read` and `resolve_for_write` canonicalize targets (and nearest existing ancestor for new files) and reject escapes outside `workspace_root` + configured `readable_roots` / `writable_roots`. |
| Sensitive-file blocks | `is_sensitive_path` (`paths.rs`), `PolicyEngine::default_sensitive_rules()` (`engine.rs`) | Denies tool reads/writes on `.env*` (allowing `.env.example`/`.sample`/`.template`), key/cert extensions (`.pem`, `.key`, `.p12`, `.pfx`, `.jks`, `.keystore`), `.ssh/*` private material, `.aws/credentials`, `.docker/config.json`, `.kube/config`, `.netrc`/`_netrc`/`.npmrc`/`.pypirc`/`auth.json`/`credentials*`, and non-source/non-doc `*secret*`/`*credential*` filenames. `default_sensitive_rules()` also installs default `Deny` rules active even in `yolo` mode unless explicitly overridden by user config. |
| OS keychain credential store | `KeychainSecretStore` (`meerai-providers/src/secrets.rs`) | Stores provider API keys in the macOS Keychain (`/usr/bin/security`) or Linux Secret Service (`secret-tool`) under service `meerai`, deleting any stale entry from `auth.json`. Falls back to `FileSecretStore` (`auth.json`, `0600` on Unix) when native keychain is unavailable or `MEERAI_DISABLE_KEYCHAIN` is set. |
| Session filesystem permissions & retention | `meerai-core::fs_perms`, `meerai-session` | Session directories are `0700` and files are `0600` on Unix (re-tightened on open). Retention is user-controlled only (`sessions prune`, `sessions.auto_prune_days`). |
| Hooks (M7) | `meerai-common/src/hooks.rs`, `meerai-agents/src/hooks.rs`, `meerai-cli/src/main.rs` | User hooks are opt-in by file. Project hooks require `MEERAI_TRUST_PROJECT_HOOKS=1|true|yes`, user `hooks.trust_project: true`, or a canonical path in user `hooks.trusted_projects`; the TUI prompt is terminal-only and defaults to no. A project file cannot trust itself. Execution uses a 10 s default timeout (60 s max), fail-open defaults, `fail_closed` opt-in, stream-capped stdout/stderr, and `HookFired` logging. Hook decisions can tighten policy but cannot loosen `plan`, explicit `deny`/`ask` rules, or the outside-workspace approval. |
| CI dependency & license audit | `deny.toml`, `.github/workflows/ci.yml`, `.github/workflows/release.yml` | `EmbarkStudios/cargo-deny-action@v2` runs `--all-features check advisories licenses bans sources` on CI pushes/PRs and as an `audit` dependency before release builds, denying known vulnerabilities and yanked crates and enforcing permissive OSI licenses. |
| Conditional release signing & checksums | `.github/workflows/release.yml` | Conditionally signs macOS binaries with `codesign --force --options runtime --timestamp` + `xcrun notarytool submit` when `APPLE_CERTIFICATE` (and Apple notarization secrets) are set, and signs Windows binaries with `signtool.exe sign` (SHA-256 + timestamp) when `WINDOWS_CERTIFICATE` is set. Always emits per-archive `.sha256` files and `SHA256SUMS.txt`. |

---

## 4. Explicit Residual Risks & Non-Guarantees

1. **Best-effort regex redaction and screening**:
   - `redact.rs` uses 9 static regex patterns with no entropy detection. Novel token formats, base64/hex-encoded secrets, multi-line split secrets, and file paths are not redacted.
   - `screen.rs` uses 10 English/token-marker regex heuristics. Paraphrased, encoded, split, or non-English prompt injections will not match, and `<untrusted_content>` wrapping does not guarantee the LLM will ignore embedded instructions.
2. **`ApprovalMode::Yolo` risks**:
   - `yolo` auto-approves `Ask` actions (while still honoring explicit `Deny` rules, including `default_sensitive_rules()`, and `ExecutionWorld` confinement). Malicious repository instructions or model hallucinations in `yolo` mode can still modify or delete workspace files and run arbitrary workspace-scoped commands without user confirmation.
3. **Nested sandbox fallback to Phase-1 guards**:
   - When `sandbox-exec` or `bwrap` fails the startup probe (for example inside an unprivileged container lacking user namespaces) or `MEERAI_DISABLE_SANDBOX` is set, `detect_execution_world` degrades to `HostWorld` with a warning. Under `HostWorld`, `shell` and `sac-run_script` have **no kernel filesystem or network isolation**; only `CodeTools` in-process path checks and env scrubbing remain.
4. **Unsandboxed Windows host & file mode gaps**:
   - Neither `SeatbeltWorld` nor `BubblewrapWorld` supports Windows. On Windows, `ExecutionWorld` always resolves to `HostWorld`, and Unix `0700`/`0600` file permission enforcement on sessions and `auth.json` has no Windows ACL equivalent.
5. **Global read visibility under OS sandboxes & unscoped LSP**:
   - Both `SeatbeltWorld` (`(allow file-read*)`) and `BubblewrapWorld` (`--ro-bind / /`) allow child processes to read host-readable files outside the workspace so system toolchains and shared libraries function. A shell command running inside the sandbox can therefore read non-scrubbed readable files on disk (though network egress is blocked when `allow_network: false`).
   - `LspToolset` (`meerai-tools/src/lsp/client.rs`) scrubs sensitive environment variables via `scrub_command_env` but does not currently spawn language servers inside `ExecutionWorld`.
6. **Unsigned release binaries when signing secrets are unconfigured**:
   - Release signing in `.github/workflows/release.yml` is conditional on repository secrets (`APPLE_CERTIFICATE`, `APPLE_ID`/`APPLE_TEAM_ID`/`APPLE_APP_PASSWORD`, `WINDOWS_CERTIFICATE`). When those secrets are not configured, the release workflow publishes **unsigned** macOS and Windows archives alongside `SHA256SUMS.txt`.
7. **Keychain fallback to `auth.json`**:
   - On headless Linux or environments without Secret Service / `secret-tool`, credentials fall back to `auth.json` (`0600` plaintext on disk) or environment variables.
8. **Hooks execute unsandboxed host commands**:
   - Hook commands run with the user's privileges outside `ExecutionWorld`; `plan` mode and policy rules do not restrict what a hook itself does. A trusted project hook or a user hook can read, write, or exfiltrate anything the user can. Project hooks are off until explicit trust, and `hooks.enabled: false` disables both sources. Hooks fail open by default, so a broken security hook does not block work; set `fail_closed` on `PreToolUse` to deny on failure. Hook stdout/stderr and reasons are untrusted and best-effort truncated, not redacted.
9. **ACP external agents are outside local policy**:
   - `--acp` drives `agy acp` as an external process. Local rules, grants, path guards, redaction, and `ExecutionWorld` do not constrain its tools. `session/request_permission` is advisory; the approval overlay forwards a decision but cannot force the agent to obey it. `plan` refuses `--acp` and denies `acp-run`. A remote agent can act with its own credentials and reach the network. `acp-run` delegates one task in a child session but does not make the external agent local-policy-bound. Treat the `ACP:agy` badge and `agy (acp)` header as an external-engine warning, not a safety claim.
