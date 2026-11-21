#!/usr/bin/env python3
"""Open or refresh the bump pull request on top of main."""

import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

API_ROOT = os.environ.get("GITHUB_API_URL", "https://api.github.com")
BASE_BRANCH = os.environ.get("BASE_BRANCH", "main")
BRANCH_PREFIX = os.environ.get("BUMP_BRANCH_PREFIX", "release/bump-v")


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def notice(message: str) -> None:
    print(f"::notice::{message}")


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True, check=False)


def git(*args: str) -> str:
    result = run("git", *args)
    if result.returncode != 0:
        fail(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def token() -> str:
    value = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not value:
        fail("GH_TOKEN is not set")
    return value


def api(method: str, path: str, payload: dict | None = None):
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    request = urllib.request.Request(
        f"{API_ROOT}{path}",
        data=data,
        method=method,
        headers={
            "Authorization": f"Bearer {token()}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read().decode("utf-8")
            return json.loads(body) if body else None
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", "replace")
        if error.code == 403 and "not permitted to create or approve pull requests" in detail:
            fail(
                "GitHub Actions may not create pull requests in this repository. "
                "Enable Settings > Actions > General > Workflow permissions > "
                "'Allow GitHub Actions to create and approve pull requests', "
                "or set BUMP_TOKEN to a fine-grained token with pull-requests: write "
                "and contents: write."
            )
        fail(f"GitHub API {method} {path} failed with HTTP {error.code}: {detail}")
    except urllib.error.URLError as error:
        fail(f"GitHub API {method} {path} is unreachable: {error.reason}")


def existing_pull_request(repository: str, branch: str) -> dict | None:
    owner = repository.split("/")[0]
    payload = api("GET", f"/repos/{repository}/pulls?state=open&head={owner}:{branch}&per_page=100")
    if not isinstance(payload, list):
        fail("unexpected pull request list payload")
    return payload[0] if payload else None


def write_output(pairs: dict) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    with open(output_path, "a", encoding="utf-8") as handle:
        for key, value in pairs.items():
            handle.write(f"{key}={value}\n")


def pull_request_body(context: dict) -> str:
    members = context["members"]
    lines = [
        f"Bumps `[workspace.package] version` from `{context['previous']}` to `{context['version']}`.",
        "",
        f"- `Cargo.toml` version becomes `{context['version']}`.",
        f"- All {members} workspace entries in `Cargo.lock` are updated with it, "
        "so `cargo build --locked` keeps working.",
        f"- After the merge, the next patch version is `{context['next']}`.",
        "",
        "### Release step",
        "",
        "Squash-merge this pull request and keep the PR title as the commit message:",
        "",
        f"`{context['title']}`",
        "",
        f"The `Release` workflow matches that subject on the push to `{BASE_BRANCH}`, waits for CI "
        f"on the squashed commit, and publishes `{context['tag']}`.",
        "If you edit the title, the release does not start.",
    ]
    return "\n".join(lines) + "\n"


def main() -> None:
    repository = os.environ.get("GITHUB_REPOSITORY", "").strip()
    context = {
        "version": os.environ.get("VERSION", "").strip(),
        "previous": os.environ.get("PREVIOUS_VERSION", "").strip(),
        "members": os.environ.get("MEMBERS", "").strip() or "workspace",
        "next": os.environ.get("NEXT_VERSION", "").strip(),
    }
    context["tag"] = os.environ.get("TAG", "").strip() or f"v{context['version']}"
    context["title"] = os.environ.get("PR_TITLE", "").strip() or (
        f"chore(release): bump version to {context['version']}"
    )
    if not repository or not context["version"]:
        fail("GITHUB_REPOSITORY and VERSION are required")

    branch = f"{BRANCH_PREFIX}{context['version']}"

    git("add", "Cargo.toml", "Cargo.lock")
    if run("git", "diff", "--cached", "--quiet").returncode == 0:
        notice("The workspace version is already current; no pull request was created.")
        write_output({"created": "false", "branch": "", "number": "", "url": "",
                      "title": context["title"], "next_version": context["next"]})
        return

    token()

    git("checkout", "-B", branch)
    git("commit", "--message", context["title"])
    git("push", "--force-with-lease", "origin", f"HEAD:refs/heads/{branch}")
    head_sha = git("rev-parse", "HEAD").strip()
    body = pull_request_body(context)

    existing = existing_pull_request(repository, branch)
    if existing:
        api("PATCH", f"/repos/{repository}/pulls/{existing['number']}",
            {"title": context["title"], "body": body})
        number = existing["number"]
        url = existing.get("html_url", "")
        notice(f"Updated pull request #{number}: {url}")
        created = "false"
    else:
        payload = api("POST", f"/repos/{repository}/pulls", {
            "title": context["title"],
            "head": branch,
            "base": BASE_BRANCH,
            "body": body,
        })
        if not isinstance(payload, dict) or "number" not in payload:
            fail("the pull request was not created")
        number = payload["number"]
        url = payload.get("html_url", "")
        notice(f"Opened pull request #{number}: {url}")
        created = "true"

    write_output({
        "created": created,
        "branch": branch,
        "number": str(number),
        "url": url,
        "sha": head_sha,
        "title": context["title"],
        "next_version": context["next"],
    })

    summary_env = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_env:
        with open(summary_env, "a", encoding="utf-8") as handle:
            handle.write("## Bump pull request\n\n")
            handle.write(f"- Title: `{context['title']}`\n")
            handle.write(f"- Branch: `{branch}`\n")
            handle.write(f"- Pull request: {url}\n")
            handle.write("- Squash-merge it with the PR title as the commit message.\n")

    print(f"Pull request for {context['tag']}: {url}")


if __name__ == "__main__":
    main()