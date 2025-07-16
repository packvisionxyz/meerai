# meerai — infrastructure

Repository knowledge for `.agents/skills/*`.

## Layout

| Path | Owns |
| :--- | :--- |
| `rust-toolchain.toml` | Pinned Rust toolchain (`1.100.0`) with `clippy` and `rustfmt` components |
| `rustfmt.toml` | Workspace formatting rules (`edition = "2024"`, `max_width = 100`, `tab_spaces = 4`) |
| `.editorconfig` | Line endings (`lf`), indentation (`4` for `.rs`/`.md`/`.json`, `2` default) |
| `.vscode/settings.json` | Format-on-save and `rust-analyzer.check.command = "clippy"` |
| `.github/workflows/ci.yml` | Push/PR verification on `ubuntu-24.04` |
| `.github/workflows/release.yml` | Tag-driven (`v*`) release: verify, tag/version gate, 5-target build, GitHub Release |
| `.github/workflows/pages.yml` | Deploys `site/` to GitHub Pages (`meerai.packvision.xyz`) |
| `.github/scripts/compose_installers.py` | Splices `site/requirements.json` into the generated blocks of both installers |
| `.github/scripts/verify_release_version.sh` | Resolves the release tag, compares it with `[workspace.package] version`, emits `tag`/`prerelease` outputs |
| `site/` | Landing page, `install.sh`, `install.ps1`, `requirements.json`, `CNAME`, `.nojekyll` |
| `Makefile` | Local task entry points (`make help`) |
| `LICENSE` | Apache-2.0 |

## CI (`ci.yml`)

Triggers: push to `main`, pull request to `main`, `workflow_dispatch`. Permissions are
`contents: read`; concurrency group `ci-${{ github.ref }}` cancels superseded runs.

One job, `verify`, on `ubuntu-24.04`:

1. `actions/checkout`
2. `dtolnay/rust-toolchain` — `1.100.0` with `rustfmt` and `clippy`
3. `swatinem/rust-cache`
4. `cargo fmt --all -- --check`
5. `cargo clippy --workspace --all-targets`
6. `cargo test --workspace`

`make verify` (alias of `make ci`) runs the same three commands locally.

CI runs on `ubuntu-24.04` only. The Windows `cmd /C` shell path and the Windows session-id rules
are unit-tested but are not exercised on a Windows runner, so Windows-only behavior is not covered
by CI. There is **no dependency-audit step** (`cargo audit` or `cargo deny`) anywhere in CI or
release; adding one is an M11 backlog item.

## Release (`release.yml`)

Triggers: push of a `v*` tag, or `workflow_dispatch` with `tag` and `prerelease` inputs.
Permissions are `contents: read` at workflow level; `publish` raises `contents: write`.

Jobs run in order `verify -> version -> build -> publish`:

| Job | Runner | Content |
| :--- | :--- | :--- |
| `verify` | `ubuntu-24.04` | Same toolchain/cache setup as CI: fmt check, clippy, workspace tests. |
| `version` | `ubuntu-24.04` | `needs: verify`. Runs `.github/scripts/verify_release_version.sh` to resolve the release tag and compare it with the workspace version. Runs before the matrix, so a tag/version mismatch costs one short job instead of five builds. |
| `build` | matrix | `needs: [verify, version]`. `cargo build --release --locked -p meerai-tui --target <triple>`, archive plus `.sha256`, `actions/upload-artifact` (1-day retention) |
| `publish` | `ubuntu-24.04` | `needs: build`. Checkout, download artifacts, build `SHA256SUMS.txt`, re-run the same tag/version script (defense in depth, also resolves `tag`/`prerelease` outputs), create the GitHub Release |

Build matrix — five published targets:

| Target | Runner | Archive |
| :--- | :--- | :--- |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | `meerai-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | `meerai-aarch64-unknown-linux-gnu.tar.gz` |
| `aarch64-apple-darwin` | `macos-15` | `meerai-aarch64-apple-darwin.tar.gz` |
| `x86_64-apple-darwin` | `macos-15` | `meerai-x86_64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `meerai-x86_64-pc-windows-msvc.zip` |

Tag/version gate: `.github/scripts/verify_release_version.sh` resolves the tag (from `REF_NAME` on a
tag push, or `INPUT_TAG` on `workflow_dispatch`), strips a leading `v` and any `-pre`/`+build`
suffix, then compares it with the `version` inside `[workspace.package]` of the root `Cargo.toml`.
It writes `tag`/`prerelease` to `$GITHUB_OUTPUT` and fails on a mismatch, so bump the workspace
version before tagging. The `version` job runs it before the build matrix; `publish` runs it again
before creating the release.

## Pages (`pages.yml`)

Triggers: push to `main` touching `site/**`, `.github/scripts/compose_installers.py`, or the
workflow itself; plus `workflow_dispatch`. It runs `python3 .github/scripts/compose_installers.py
--out-dir _site`, uploads the directory as a Pages artifact, and deploys it. Permissions:
`contents: read`, `pages: write`, `id-token: write`; concurrency group `pages`.

## Installers

`site/install.sh` (POSIX `sh`) and `site/install.ps1` (PowerShell) both resolve a target triple
from the host, download `meerai-<triple>.<ext>` plus its `.sha256` from GitHub Releases, verify the
checksum, and install into a user bin directory.

- `site/requirements.json` is the single source for the "missing requirement" checks; the block
  between `# BEGIN GENERATED REQUIREMENTS` and `# END GENERATED REQUIREMENTS` in each installer is
  generated. Regenerate with
  `python3 .github/scripts/compose_installers.py --in-place` after editing the JSON, never by hand.
- Windows publishes the x64 build only. On an ARM64 host the installer downloads
  `meerai-x86_64-pc-windows-msvc.zip` and prints a warning that the binary runs under emulation;
  unsupported architectures fail with an actionable message.
- The shell tool uses `cmd /C` on Windows, so the x64 build is the supported Windows path.

## Gaps

- **No codesigning or notarization.** macOS archives are unsigned and un-notarized; Windows
  archives are unsigned. No signing secrets exist and no workflow step performs signing, so users
  may hit Gatekeeper or SmartScreen warnings. Do not add signing secrets without an owner decision.
- **No dependency audit.** Neither `ci.yml` nor `release.yml` runs `cargo audit` or `cargo deny`.
  Supply-chain scanning is an M11 backlog item.
- **No written threat model.** Repository instructions (`AGENTS.md`,
  `.meerai/instructions.md`), model output, hooks, MCP servers, and subagents are treated as
  untrusted input by policy but are not modelled or screened. Prompt-injection screening does not
  exist.
- No Terraform, Dockerfiles, or self-hosted runners. All jobs use GitHub-hosted runners.
- Product gaps: checkpoints/rewind, hooks, the subagent runtime (M8), an OS sandbox, USD pricing,
  and per-model context metadata. TUI session resume/transcript replay, headless
  `--continue`/`--resume <ID>`, project-root config discovery, `sessions prune`, opt-in
  `sessions.auto_prune_days`, and M12 `.agents`/`.claude` agent and skill discovery already ship.
  M12 applies definitions as untrusted prompt text only: there is no subagent runtime, no
  MCP/tool/permission mapping, and no supporting-skill-file loading. Tracked in `README.md` and
  `docs/harness-spec.md` §15.1 (security items in §9.6).

## Environments & secrets

CI, release, and Pages need no repository secrets; they use the default `GITHUB_TOKEN` with the
per-workflow permissions listed above.

Provider API keys are runtime configuration, not CI secrets. Resolution order per provider is
stored `auth.json` → configured `api_key` → provider environment variables.

- `auth.json` in the app data dir, written by `meerai auth login`; owner-only permissions (`0600`
  on Unix). No OS keychain or keyring integration exists.
- Environment variables from the bundled `meerai-common/providers.yml`, for example
  `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, `OPENAI_API_KEY`.
- `MEERAI_PROVIDER` / `MEERAI_MODEL` select defaults; `MEERAI_SESSION_DIR` overrides the session
  root.
- `.env` is gitignored and is **not** loaded implicitly. Only `MEERAI_LOAD_DOTENV=1|true|yes` loads
  `./.env`, and `--env-file <PATH>` loads an explicit file (a missing file is an error).

Never commit `.env`, API keys, or personal config files.

## Local checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

Run in order (`fmt -> clippy -> test`) before concluding any code change, or use `make verify`.

For infrastructure edits, also validate syntax:

```sh
ruby -ryaml -e 'ARGV.each{|f| YAML.load_file(f)}' .github/workflows/*.yml
sh -n site/install.sh
sh -n .github/scripts/verify_release_version.sh
actionlint .github/workflows/*.yml
```

PyYAML is not installed in the dev environment, so use `ruby -ryaml` for the YAML parse. `actionlint`
is available at `~/go/bin/actionlint`; it checks expressions, action inputs, and job wiring.

The tag/version script can be exercised locally without GitHub:

```sh
GITHUB_OUTPUT=/dev/null EVENT_NAME=push REF_NAME=v0.0.1 sh .github/scripts/verify_release_version.sh
GITHUB_OUTPUT=/dev/null EVENT_NAME=workflow_dispatch INPUT_TAG=v0.0.1 INPUT_PRERELEASE=true \
  sh .github/scripts/verify_release_version.sh
```

## Escalate

Stop and ask the user before:

- Changing the pinned Rust version in `rust-toolchain.toml` or workspace edition in `Cargo.toml`.
- Changing release targets, release triggers, job permissions, or crate publishing metadata
  (`publish = false` in `[workspace.package]`).
- Adding codesigning/notarization secrets or any other repository secret.
- Adding new outbound network hosts, telemetry, or credential storage mechanisms.
- Modifying `.agents/skills/` (shared skill bundle) when a change only belongs in `.agents/repo/`.
