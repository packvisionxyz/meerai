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
| `.github/workflows/release.yml` | Release on CI success (`main`), `v*` tags, or dispatch: tag/version detect, 5-target build, GitHub Release |
| `.github/workflows/pages.yml` | Deploys `site/` to GitHub Pages (`meerai.packvision.xyz`) |
| `.github/scripts/compose_installers.py` | Splices `site/requirements.json` into the generated blocks of both installers |
| `.github/scripts/verify_release_version.py` | Resolves the release tag, compares it with `[workspace.package] version`, emits `tag`/`version`/`prerelease` outputs |
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

CI runs on `ubuntu-24.04` only, executing `verify` (fmt check, clippy, workspace tests) and `audit`
(`EmbarkStudios/cargo-deny-action@v2` checking advisories, licenses, bans, and sources via `deny.toml`).
The Windows `cmd /C` shell path and Windows session-id rules are unit-tested but are not exercised
on a Windows runner, so Windows-only behavior is not covered by CI.

## Release (`release.yml`)

Triggers: push of a `v*` tag, completion of the CI workflow on `main`, or `workflow_dispatch` with
`tag` and `prerelease` inputs. The `workflow_run` path starts only when CI concludes successfully
for a commit of this repository, so release reuses the CI gate and never repeats fmt, clippy,
tests, or `cargo-deny`. Permissions are `contents: read` at workflow level; `publish` raises
`contents: write`.

Jobs run `detect -> build -> publish`:

| Job | Runner | Content |
| :--- | :--- | :--- |
| `detect` | `ubuntu-24.04` | Checks out the built commit (`github.event.workflow_run.head_sha`, else `github.sha`) and runs `.github/scripts/verify_release_version.py`. On `workflow_run` it sets `TAG_FROM_VERSION=1`, so the tag is `v<workspace version>`. Then `gh release view` sets `skip=true` when that release already exists. Runs before the matrix, so a skipped push or a tag/version mismatch costs one short job instead of five builds. |
| `build` | matrix | `needs: detect`, skipped when `skip` is true. `cargo build --release --locked -p meerai-cli --target <triple>`, conditional macOS `codesign`/`notarytool` (when `APPLE_CERTIFICATE` + Apple credentials exist) and Windows `signtool` (when `WINDOWS_CERTIFICATE` exists), archive plus `.sha256`, `actions/upload-artifact` (1-day retention) |
| `publish` | `ubuntu-24.04` | `needs: [detect, build]`, skipped when `skip` is true. Checkout, download artifacts, build `SHA256SUMS.txt`, create the GitHub Release with `tag_name`/`prerelease` from `detect` and `target_commitish` set to the built commit |

Build matrix — five published targets:

| Target | Runner | Archive |
| :--- | :--- | :--- |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | `meerai-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | `meerai-aarch64-unknown-linux-gnu.tar.gz` |
| `aarch64-apple-darwin` | `macos-15` | `meerai-aarch64-apple-darwin.tar.gz` |
| `x86_64-apple-darwin` | `macos-15` | `meerai-x86_64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `meerai-x86_64-pc-windows-msvc.zip` |

Tag/version gate: `.github/scripts/verify_release_version.py` resolves the tag (from `REF_NAME` on a
tag push, or `INPUT_TAG` on `workflow_dispatch`), or derives `v<workspace version>` when
`TAG_FROM_VERSION` is set on the CI-success path. It strips a leading `v` and any `refs/tags/`
prefix, then compares the result with the `version` inside `[workspace.package]` of the root
`Cargo.toml`. A tag passes when its full version equals the workspace version (so `0.1.0-rc.1`
matches `0.1.0-rc.1`), or when the tag is `vX.Y.Z[-pre][+build]` and the workspace version is that
same stable `X.Y.Z`. The script writes `tag`/`version`/`prerelease` to `$GITHUB_OUTPUT` and fails
on a mismatch or a malformed tag, so bump the workspace version before tagging. `detect` runs it
before the build matrix; `publish` reuses the `detect` outputs.

Release flow on `main`: bump `[workspace.package] version`, run `cargo update -w`, commit
`Cargo.toml` and `Cargo.lock` together, and push. CI runs the gate, then `release.yml` derives
`v<version>`, skips when that release exists, builds, and publishes with the tag created at the
pushed commit. No tag is pushed by hand and no commit is made by CI.

A manual `v*` tag or `workflow_dispatch` does not re-run fmt, clippy, tests, or `cargo-deny`: the
CI-success path is the normal release and the only one that proves the code passed the gate. Treat
a manual tag as an operator override for a commit CI already validated.

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

- **Signing secrets not yet provisioned.** `.github/workflows/release.yml` includes conditional macOS `codesign` + `xcrun notarytool` (`APPLE_CERTIFICATE`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`) and Windows `signtool.exe` (`WINDOWS_CERTIFICATE`) steps, but archives remain unsigned until those repository secrets are added.
- No Terraform, Dockerfiles, or self-hosted runners. All jobs use GitHub-hosted runners.
- Remaining security and host-coverage limits: `ExecutionWorld` runs `SeatbeltWorld` (macOS) and `BubblewrapWorld` (Linux), falling back to `HostWorld` on Windows or when sandboxing is disabled/unavailable (`MEERAI_DISABLE_SANDBOX`); `LspToolset` scrubs env vars without OS sandboxing; injection screening (`screen_run` / `screen_untrusted_text`) and secret redaction (`redact_run`) are heuristic/regex best-effort controls (documented in `docs/threat-model.md`).
- Product gaps: checkpoints/rewind (M6), hooks (M7), ACP external backends (M13/M14), USD pricing, and per-model context metadata. Landed in M8–M12: subagent runtime (`subagent-run`), TUI session replay, headless `--continue`/`--resume`/`--fork`, project-root config discovery, `sessions prune` + `sessions.auto_prune_days`, `.agents`/`.claude` definition discovery, OS sandbox (`ExecutionWorld`), CI `cargo-deny` audit (`deny.toml`), `docs/threat-model.md`, heuristic injection screening, and OS keychain credential storage (`KeychainSecretStore` with `auth.json` fallback). Tracked in `README.md` and `docs/harness-spec.md`.

## Environments & secrets

CI, release, and Pages work without repository secrets by default (release signing steps are conditional on `APPLE_CERTIFICATE` and `WINDOWS_CERTIFICATE`).

Provider API keys are runtime configuration, not CI secrets. Resolution order per provider is
credential store (native OS keychain via `KeychainSecretStore`, falling back to `auth.json`) → configured `api_key` → provider environment variables.

- Native OS keychain (macOS `/usr/bin/security`, Linux `secret-tool`, disabled via `MEERAI_DISABLE_KEYCHAIN`), or `auth.json` (`0600` on Unix) in the app data dir as fallback.
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
GITHUB_OUTPUT=/dev/null EVENT_NAME=push REF_NAME=v0.0.1 python3 .github/scripts/verify_release_version.py
GITHUB_OUTPUT=/dev/null EVENT_NAME=workflow_run REF_NAME=main TAG_FROM_VERSION=1 \
  python3 .github/scripts/verify_release_version.py
GITHUB_OUTPUT=/dev/null EVENT_NAME=workflow_dispatch INPUT_TAG=v0.0.1 INPUT_PRERELEASE=true \
  python3 .github/scripts/verify_release_version.py
```

## Escalate

Stop and ask the user before:

- Changing the pinned Rust version in `rust-toolchain.toml` or workspace edition in `Cargo.toml`.
- Changing release targets, release triggers, job permissions, or crate publishing metadata
  (`publish = false` in `[workspace.package]`).
- Adding codesigning/notarization secrets or any other repository secret.
- Adding new outbound network hosts, telemetry, or credential storage mechanisms.
- Modifying `.agents/skills/` (shared skill bundle) when a change only belongs in `.agents/repo/`.
