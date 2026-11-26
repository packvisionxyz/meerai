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
| `.github/workflows/release.yml` | Release on a `main` push that changes `Cargo.toml` with the bump commit subject, or on dispatch: tag/version detect, pure-bump check, notes, 5-target build, GitHub Release |
| `.github/workflows/bump.yml` | Manual `workflow_dispatch` version bump: rewrites `Cargo.toml` and `Cargo.lock`, commits, opens the bump PR against `main` |
| `.github/scripts/compose_installers.py` | Splices `site/requirements.json` into the generated blocks of both installers |
| `.github/scripts/verify_release_version.py` | Resolves the release tag, compares it with `[workspace.package] version`, emits `tag`/`version`/`prerelease` outputs |
| `.github/scripts/bump_version.py` | Writes the target version, rewrites `[workspace.package] version` and every workspace entry in `Cargo.lock`, writes the commit subject |
| `.github/scripts/verify_bump_commit.py` | Proves the release commit changes only the version lines of `Cargo.toml` and `Cargo.lock` |
| `.github/scripts/generate_release_notes.py` | Builds the release body from the commits since the previous tag |
| `.github/workflows/pages.yml` | Deploys `site/` to GitHub Pages (`meerai.packvision.xyz`) |
| `site/` | Landing page, `install.sh`, `install.ps1`, `requirements.json`, `CNAME`, `.nojekyll` |
| `Makefile` | Local task entry points (`make help`); `install` and `package` build the Wasm bundle and enable the `web` feature |
| `LICENSE` | Apache-2.0 |

## CI (`ci.yml`)

Triggers: push to `main`, pull request to `main`, `workflow_dispatch`. Push and pull request ignore
`**/*.md`, `docs/**`, `site/**`, `.agents/**`, and `LICENSE`, so a docs-only or site-only change runs
nothing. Concurrency group `ci-${{ github.ref }}` cancels superseded runs. Permissions are
`contents: read`.

Both jobs also skip a `main` push whose subject starts with `chore(release): bump version to `,
because that commit changes only version lines.

`verify` on `ubuntu-24.04`:

1. `actions/checkout`
2. `dtolnay/rust-toolchain` — `1.100.0` with `rustfmt` and `clippy`
3. `swatinem/rust-cache`
4. `cargo fmt --all -- --check`
5. `cargo clippy --workspace --all-targets`
6. `cargo check -p meerai-cli --no-default-features`
7. `cargo check -p meerai-tui --no-default-features`
8. `rustup target add wasm32-unknown-unknown`
9. `cargo check -p meerai-web-client --target wasm32-unknown-unknown`
10. `cargo test --workspace`

`audit` runs `EmbarkStudios/cargo-deny-action@v2` (`--all-features`, `deny.toml`) on every push to
`main` and on `workflow_dispatch`. On a pull request it first lists the PR files with
`gh api .../pulls/<n>/files` and skips the audit unless `Cargo.toml` or `Cargo.lock` changed.

`make verify` (alias of `make ci`) runs fmt, clippy, and tests locally.

CI runs on `ubuntu-24.04` only. The Windows `cmd /C` shell path and Windows session-id rules are
unit-tested but are not exercised on a Windows runner, so Windows-only behavior is not covered by CI.

## Version bump (`bump.yml`)

Trigger: `workflow_dispatch` only, with a required `version` input and an optional `dry_run` flag.
`version` is the target workspace version, so the run title is exactly `Bump version <version>`.
There is no level input: the number is always chosen by hand, which removes the ambiguity between a
level and an explicit version. Permissions are `contents: write` on the job so the push uses the
workflow's own `GITHUB_TOKEN`; no personal token and no extra secret are needed. Concurrency group
`version-bump` serializes runs.

Steps: checkout `main` with `fetch-depth: 0` and tags, set a bot git identity, run
`.github/scripts/bump_version.py --version <version>`, refuse a version whose `v<version>` tag
already exists, prove `Cargo.lock` is consistent with `cargo metadata --locked --no-deps`, show the
diff and the commit message, then `.github/scripts/open_bump_pr.py` stages `Cargo.toml` and
`Cargo.lock`, commits with the PR title, force-with-lease pushes `release/bump-v<version>`, and
opens or updates the pull request against `main`.

`bump.yml` needs one repository setting and one optional secret:

- Settings > Actions > General > Workflow permissions must allow
  `Allow GitHub Actions to create and approve pull requests`. Without it the `POST /pulls` call
  returns HTTP 403 `GitHub Actions is not permitted to create or approve pull requests`.
- Set the `BUMP_TOKEN` secret to a fine-grained token with `contents: write` and
  `pull-requests: write` to bypass that setting, and to make the bump branch push start CI on its
  own. `BUMP_TOKEN` falls back to `GITHUB_TOKEN`.

`bump_version.py` is the only writer of `[workspace.package] version`. It requires `--version`,
reads the current version through `verify_release_version.workspace_version`, normalizes the target,
and refuses a version that is not strictly greater. It rewrites the `version` line under
`[workspace.package]`, then rewrites the `version` line of every `[[package]]` block in `Cargo.lock`
whose `name` is a workspace member, and fails when a member is missing or still stale. It emits the
outputs `version`, `previous_version`, `tag`, `subject`, and `members`.

The commit subject is exactly `chore(release): bump version to <version>`; `release.yml` matches
that string. Squash-merge the PR and keep its title as the commit message, or the release does not
start. Read `open_bump_pr.py` outputs `created`, `branch`, `number`, `url`, and `sha` to follow the
PR in the run summary.

When `BUMP_TOKEN` is not set, `open_bump_pr.py` uses `GITHUB_TOKEN`, so GitHub opens the PR as
`github-actions[bot]` and holds the `pull_request` run of `ci.yml` until a maintainer clicks
**Approve workflows to run**. With `BUMP_TOKEN`, the PR starts `ci.yml` automatically without
approval.

## Release (`release.yml`)

Triggers: a push to `main` that changes `Cargo.toml` **and** whose commit subject starts with
`chore(release): bump version to `, plus `workflow_dispatch` with an optional `tag` input. Any other
`main` push that edits `Cargo.toml` stops at the `detect` job guard. A `dry_run` bump never
commits, so it never triggers a release.

Jobs run `detect -> release-notes -> build -> publish`:

| Job | Runner | Content |
| :--- | :--- | :--- |
| `detect` | `ubuntu-24.04` | Full-history checkout of `github.sha`, then `.github/scripts/verify_release_version.py` with `TAG_FROM_VERSION=1`, so the tag is `v<workspace version>`. The step `Require the bump commit message` compares `git log -1 --format=%s` with `chore(release): bump version to <version>` and fails on a mismatch (a warning on `workflow_dispatch`). `verify_bump_commit.py` then proves the commit is a pure version bump. Finally `gh release view` sets `skip=true` when the release already exists. |
| `release-notes` | `ubuntu-24.04` | `needs: detect`. Full-history checkout, then `.github/scripts/generate_release_notes.py` writes `release-notes.md` and uploads it as the `release-notes` artifact (1 day). |
| `build` | matrix | `needs: [detect, release-notes]`, skipped when `skip` is true. Adds `wasm32-unknown-unknown`, installs the `wasm-bindgen-cli` version pinned in `Cargo.lock`, runs `python3 scripts/build-web.py`, then `cargo build --release --locked -p meerai-cli --features web --target <triple>`, so the binary embeds the browser bundle. Conditional macOS `codesign`/`notarytool` (when `APPLE_CERTIFICATE` + Apple credentials exist) and Windows `signtool` (when `WINDOWS_CERTIFICATE` exists), archive plus `.sha256`, `actions/upload-artifact` (1-day retention) |
| `publish` | `ubuntu-24.04` | `needs: [detect, release-notes, build]`, skipped when `skip` is true. Checkout the release commit, download the notes and the artifacts, build `SHA256SUMS.txt`, then `gh release create <tag> --target <release sha> --title "meerai <tag>" --notes-file notes/release-notes.md` with `--prerelease` when the version has a pre-release part and `--notes-start-tag <previous tag>` when one exists. |

The run title is `Release from version bump` for the push trigger and `Release <tag>` (or `Release
from workspace version`) for a dispatch. A `run-name` is evaluated before any job starts, so it
cannot read the `detect` outputs and cannot show the version on a push. Instead, the job names carry
it: `Build release notes (<tag>)`, `Build <triple> (<tag>)`, and `Publish <tag>`. Do not put
`github.event.head_commit.message` in `run-name`: a squash-merge message carries the PR body, and
`run-name` is limited to 256 characters.

`verify_bump_commit.py` diffs `HEAD^..HEAD` with `--unified=0`. It requires the change set to
contain `Cargo.toml` and `Cargo.lock` and nothing else. Every added and removed line in both files
must match `version = "<v>"`, the added lines must carry the release version, and `Cargo.toml` must
contribute exactly one of them. A manual dispatch downgrades every problem to a warning, because a
dispatched release may point at any commit. The release therefore needs no CI run of its own: the
bump commit cannot change code, and the `build` job already compiles with `--locked`.

`generate_release_notes.py` picks the base tag from `PREVIOUS_TAG` or the highest `vX.Y.Z*` tag that
is an ancestor of `HEAD`, then builds the body from `git log`. The body contains the workspace
version transition, the release commit, a Breaking changes block when a `type!:` subject or a
`BREAKING CHANGE:` footer exists, the commits grouped by conventional type (feat, fix, perf,
refactor, docs, test, build, ci, chore, style), a link to the full compare, and the install snippet.
It does not list changed files. It caps the commit list at 100 (`NOTES_MAX_COMMITS`) and emits
`previous_tag`, `base_sha`, and `commit_count`.

Build matrix — five published targets, each with the `web` feature and the embedded bundle:

| Target | Runner | Archive |
| :--- | :--- | :--- |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | `meerai-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | `meerai-aarch64-unknown-linux-gnu.tar.gz` |
| `aarch64-apple-darwin` | `macos-15` | `meerai-aarch64-apple-darwin.tar.gz` |
| `x86_64-apple-darwin` | `macos-15` | `meerai-x86_64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `meerai-x86_64-pc-windows-msvc.zip` |

Tag/version gate: `.github/scripts/verify_release_version.py` derives
`v<workspace version>` because the release path sets `TAG_FROM_VERSION=1`; a manual dispatch may pass
`INPUT_TAG` instead. It strips a leading `v` and any `refs/tags/` prefix, then compares the result
with the `version` inside `[workspace.package]` of the root `Cargo.toml`. A tag passes when its full
version equals the workspace version (so `0.1.0-rc.1` matches `0.1.0-rc.1`), or when the tag is
`vX.Y.Z[-pre][+build]` and the workspace version is that same stable `X.Y.Z`. The script writes
`tag`/`version`/`prerelease` to `$GITHUB_OUTPUT` and fails on a mismatch or a malformed tag.

Release flow: run the `Bump version` workflow. It rewrites `Cargo.toml` and `Cargo.lock`, commits
`chore(release): bump version to <version>` on `release/bump-v<version>`, and opens a PR against
`main`. Squash-merge that PR with the title as the commit message. The resulting push changes
`Cargo.toml` and matches the subject, so `release.yml` starts, verifies the commit is a pure version
bump, builds the five targets, and publishes `v<version>` with generated notes. No tag is pushed by
hand; `gh release create` creates it at the release commit.

A `workflow_dispatch` release uses the checked-out `main` commit. The pure-bump check becomes a
warning there, so verify the commit before you dispatch.

## Pages (`pages.yml`)

Triggers: push to `main` touching `site/**`, `.github/scripts/compose_installers.py`, or the
workflow itself; plus `workflow_dispatch`. It runs `python3 .github/scripts/compose_installers.py
--out-dir _site`, uploads the directory as a Pages artifact, and deploys it. Permissions:
`contents: read`, `pages: write`, `id-token: write`; concurrency group `pages`.

## Installers

`site/install.sh` (POSIX `sh`) and `site/install.ps1` (PowerShell) both resolve a target triple
from the host, download `meerai-<triple>.<ext>` plus its `.sha256` from GitHub Releases, verify the
checksum, and install into a user bin directory. Both install the web-enabled binary, so `meerai
serve` works after install; no separate web archive is published.

- `site/requirements.json` is the single source for the "missing requirement" checks; the block
  between `# BEGIN GENERATED REQUIREMENTS` and `# END GENERATED REQUIREMENTS` in each installer is
  generated. Regenerate with
  `python3 .github/scripts/compose_installers.py --in-place` after editing the JSON, never by hand.
- Windows publishes the x64 build only. On an ARM64 host the installer downloads
  `meerai-x86_64-pc-windows-msvc.zip` and prints a warning that the binary runs under emulation;
  unsupported architectures fail with an actionable message.
- The shell tool uses `cmd /C` on Windows, so the x64 build is the supported Windows path.

## Gaps

- **Signing secrets not yet provisioned.** `.github/workflows/release.yml` includes conditional macOS `codesign` + `xcrun notarytool` (`APPLE_CERTIFICATE`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`) and Windows `signtool.exe` (`WINDOWS_CERTIFICATE`) steps, but archives remain unsigned until those repository secrets are added.
- No Terraform, Dockerfiles, or self-hosted runners. All jobs use GitHub-hosted runners.
- Remaining security and host-coverage limits: `ExecutionWorld` runs `SeatbeltWorld` (macOS) and `BubblewrapWorld` (Linux), falling back to `HostWorld` on Windows or when sandboxing is disabled/unavailable (`MEERAI_DISABLE_SANDBOX`); `LspToolset` scrubs env vars without OS sandboxing; injection screening (`screen_run` / `screen_untrusted_text`) and secret redaction (`redact_run`) are heuristic/regex best-effort controls (documented in `docs/threat-model.md`).
- Product gaps: checkpoints/rewind (M6), hooks (M7), ACP external backends (M13/M14), USD pricing, and per-model context metadata. Landed in M8–M12: subagent runtime (`subagent-run`), TUI session replay, headless `--continue`/`--resume`/`--fork`, project-root config discovery, `sessions prune` + `sessions.auto_prune_days`, definition discovery, OS sandbox (`ExecutionWorld`), CI `cargo-deny` audit (`deny.toml`), `docs/threat-model.md`, heuristic injection screening, and OS keychain credential storage (`KeychainSecretStore` with `auth.json` fallback). Tracked in `README.md` and `docs/harness-spec.md`.

## Environments & secrets

CI, release, and Pages work without repository secrets by default (release signing steps are conditional on `APPLE_CERTIFICATE` and `WINDOWS_CERTIFICATE`).

Provider API keys are runtime configuration, not CI secrets. Resolution order per provider is
credential store (AEAD `VaultSecretStore` with an OS keychain DEK) → configured `api_key` → provider environment variables.

- Native OS keychain (macOS through `/usr/bin/security`, Linux `secret-tool`, Windows Credential
  Manager, disabled via `MEERAI_DISABLE_KEYCHAIN`). The macOS grant covers Apple's tool, not meerai,
  so it survives rebuilds but is not a per-application boundary. Without a keychain the vault
  requires `MEERAI_VAULT_PASSPHRASE` and fails closed; there is no `auth.json` fallback.
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
python3 -m py_compile .github/scripts/verify_release_version.py
actionlint .github/workflows/*.yml
```

PyYAML is not installed in the dev environment, so use `ruby -ryaml` for the YAML parse. `actionlint`
is available at `~/go/bin/actionlint`; it checks expressions, action inputs, and job wiring.

The tag/version script can be exercised locally without GitHub:

```sh
GITHUB_OUTPUT=/dev/null EVENT_NAME=push REF_NAME=main TAG_FROM_VERSION=1 \
  python3 .github/scripts/verify_release_version.py
GITHUB_OUTPUT=/dev/null EVENT_NAME=workflow_dispatch INPUT_TAG=v0.0.1 \
  python3 .github/scripts/verify_release_version.py
```

The bump and notes scripts run against a scratch copy of the repository:

```sh
python3 .github/scripts/bump_version.py --version 0.1.0 --dry-run
VERSION=0.0.2 TAG=v0.0.2 PREVIOUS_VERSION=0.0.1 NOTES_OUT=target/notes.md \
  python3 .github/scripts/generate_release_notes.py
```

`verify_bump_commit.py` needs `VERSION` and reads the two commits from `git`. Run it on a scratch
clone after a dry bump:

```sh
VERSION=0.0.2 TAG=v0.0.2 EVENT=push python3 .github/scripts/verify_bump_commit.py
```

## Escalate

Stop and ask the user before:

- Changing the pinned Rust version in `rust-toolchain.toml` or workspace edition in `Cargo.toml`.
- Changing release targets, release triggers, job permissions, or crate publishing metadata
  (`publish = false` in `[workspace.package]`).
- Adding codesigning/notarization secrets or any other repository secret.
- Adding new outbound network hosts, telemetry, or credential storage mechanisms.
- Modifying `.agents/skills/` (shared skill bundle) when a change only belongs in `.agents/repo/`.
