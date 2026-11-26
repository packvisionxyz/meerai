#!/usr/bin/env python3
"""Build a release body from the commits since the previous tag."""

import os
import re
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import verify_release_version as gate

MAX_COMMITS = int(os.environ.get("NOTES_MAX_COMMITS", "100"))
TAG_PATTERN = re.compile(r"^v\d+\.\d+\.\d+.*$")
CONVENTIONAL = re.compile(
    r"^(?P<type>[a-zA-Z]+)(?:\((?P<scope>[^)]*)\))?(?P<breaking>!)?:\s*(?P<summary>.+)$"
)
BREAKING_FOOTER = re.compile(r"^BREAKING[ -]CHANGE:", re.MULTILINE)
RELEASE_SUBJECT = "chore(release):"
FIELD = "\x1f"
RECORD = "\x1e"

TYPE_TITLES = {
    "feat": "Features",
    "fix": "Fixes",
    "perf": "Performance",
    "refactor": "Refactors",
    "docs": "Documentation",
    "test": "Tests",
    "build": "Build",
    "ci": "CI",
    "chore": "Chores",
    "style": "Style",
}
TYPE_ORDER = list(TYPE_TITLES.values()) + ["Other changes"]


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def git(*args: str) -> str:
    result = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        fail(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def git_optional(*args: str) -> str:
    result = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    return result.stdout.strip() if result.returncode == 0 else ""


def is_ancestor(commit: str, ref: str) -> bool:
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", commit, ref],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.returncode == 0


def previous_tag(current_tag: str) -> str:
    explicit = os.environ.get("PREVIOUS_TAG", "").strip()
    candidates = [explicit] if explicit else []
    for tag in git("tag", "--sort=-v:refname").split():
        if TAG_PATTERN.match(tag) and tag != current_tag:
            candidates.append(tag)
    for candidate in candidates:
        if candidate and is_ancestor(candidate, "HEAD"):
            return candidate
    return ""


def resolve_previous_version(current_version: str, base_tag: str) -> str:
    explicit = os.environ.get("PREVIOUS_VERSION", "").strip()
    if explicit:
        return explicit

    for ref in (base_tag, "HEAD^"):
        if not ref:
            continue
        manifest = git_optional("show", f"{ref}:Cargo.toml")
        if not manifest:
            continue
        found = gate.workspace_version_from_lines(manifest)
        if found and found != current_version:
            return found

    if base_tag:
        return base_tag[1:] if base_tag[:1] in ("v", "V") else base_tag
    return ""


def read_commits(revision_range: str) -> list[dict]:
    raw = git(
        "log",
        f"--max-count={MAX_COMMITS}",
        f"--format=%H{FIELD}%an{FIELD}%s{FIELD}%b{RECORD}",
        revision_range,
    )
    commits = []
    for record in raw.split(RECORD):
        record = record.strip("\n")
        if not record:
            continue
        fields = record.split(FIELD)
        if len(fields) != 4:
            continue
        commits.append({"sha": fields[0], "author": fields[1], "subject": fields[2], "body": fields[3]})
    return commits


def classify(commit: dict) -> dict:
    subject = commit["subject"].strip()
    match = CONVENTIONAL.match(subject)
    breaking = bool(BREAKING_FOOTER.search(commit["body"])) or bool(match and match.group("breaking"))
    if match is None:
        return {
            "sha": commit["sha"],
            "group": "Other changes",
            "subject": subject,
            "summary": subject,
            "scope": "",
            "breaking": breaking,
            "release": False,
        }
    kind = match.group("type").lower()
    return {
        "sha": commit["sha"],
        "group": TYPE_TITLES.get(kind, "Other changes"),
        "subject": subject,
        "summary": match.group("summary").strip(),
        "scope": (match.group("scope") or "").strip(),
        "breaking": breaking,
        "release": kind == "chore" and subject.startswith(RELEASE_SUBJECT),
    }


def group_commits(commits: list[dict]) -> dict[str, list[dict]]:
    grouped: dict[str, list[dict]] = {}
    for commit in commits:
        grouped.setdefault(commit["group"], []).append(commit)
    return {title: grouped[title] for title in TYPE_ORDER if title in grouped}


def render_body(context: dict) -> str:
    version = context["version"]
    lines = [f"meerai {version}", ""]
    if context["previous_version"] and context["previous_version"] != version:
        lines.append(f"Workspace version `{context['previous_version']}` -> `{version}`.")
    else:
        lines.append(f"Workspace version `{version}`.")
    lines.append("")
    if context["previous_tag"]:
        lines.append(f"Changes since `{context['previous_tag']}`.")
    else:
        lines.append("First tagged release; the notes cover the whole history.")

    release_commits = [commit for commit in context["commits"] if commit["release"]]
    if release_commits:
        lines.append("")
        lines.append("## Release commit")
        lines.append("")
        for commit in release_commits:
            lines.append(f"- `{commit['subject']}` ({commit['sha'][:7]})")

    breaking = [commit for commit in context["commits"] if commit["breaking"]]
    if breaking:
        lines.append("")
        lines.append("## Breaking changes")
        lines.append("")
        for commit in breaking:
            lines.append(f"- **{commit['summary']}** ({commit['sha'][:7]})")

    grouped = group_commits([commit for commit in context["commits"] if not commit["release"]])
    if grouped:
        lines.append("")
        lines.append(f"## Changes ({len(context['commits']) - len(release_commits)} commits)")
        for title, items in grouped.items():
            lines.append("")
            lines.append(f"### {title}")
            lines.append("")
            for commit in items:
                scope = f"**{commit['scope']}**: " if commit["scope"] else ""
                lines.append(f"- {scope}{commit['summary']} ({commit['sha'][:7]})")

    if not grouped and not release_commits:
        lines.append("")
        lines.append("No commits are recorded for this release.")

    lines.append("")
    lines.append(f"**Full changelog**: {context['compare_url']}")
    lines.append("")
    lines.append("## Install")
    lines.append("")
    lines.append("**macOS and Linux**")
    lines.append("```sh")
    lines.append("curl -LsSf https://meerai.packvision.xyz/install.sh | sh")
    lines.append("```")
    lines.append("")
    lines.append("**Windows (PowerShell)**")
    lines.append("```powershell")
    lines.append('powershell -ExecutionPolicy ByPass -c "irm https://meerai.packvision.xyz/install.ps1 | iex"')
    lines.append("```")
    return "\n".join(lines) + "\n"


def main() -> None:
    repository = os.environ.get("GITHUB_REPOSITORY", "packvisionxyz/meerai").strip()
    version = os.environ.get("VERSION", "").strip()
    tag = os.environ.get("TAG", "").strip() or (f"v{version}" if version else "")
    if not version or not tag:
        fail("VERSION and TAG are required")

    root = git_optional("rev-parse", "--show-toplevel")
    if not root:
        fail("not inside a git repository")
    os.chdir(root)

    base_tag = previous_tag(tag)
    revision_range = f"{base_tag}..HEAD" if base_tag else "HEAD"
    head_sha = git("rev-parse", "HEAD").strip()
    base_sha = git("rev-parse", base_tag).strip() if base_tag else ""

    commits = [classify(commit) for commit in read_commits(revision_range)]

    context = {
        "version": version,
        "previous_version": resolve_previous_version(version, base_tag),
        "previous_tag": base_tag,
        "commits": commits,
        "compare_url": (
            f"https://github.com/{repository}/compare/{base_tag}...{head_sha}"
            if base_tag
            else f"https://github.com/{repository}/commits/{head_sha}"
        ),
    }
    body = render_body(context)

    output_path = os.environ.get("NOTES_OUT", "")
    if output_path:
        Path(output_path).write_text(body, encoding="utf-8")
    tag_path = os.environ.get("TAG_OUT", "")
    if tag_path:
        Path(tag_path).write_text(tag + "\n", encoding="utf-8")

    output_env = os.environ.get("GITHUB_OUTPUT")
    if output_env:
        with open(output_env, "a", encoding="utf-8") as handle:
            handle.write(f"previous_tag={base_tag}\n")
            handle.write(f"base_sha={base_sha}\n")
            handle.write(f"commit_count={len(commits)}\n")

    summary_env = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_env:
        with open(summary_env, "a", encoding="utf-8") as handle:
            handle.write(body)

    if not base_tag:
        print("::warning::No previous release tag found; notes cover the whole history.")
    print(f"Notes for {tag}: {len(commits)} commit(s), base {base_tag or 'none'}")
    if not output_path:
        sys.stdout.write(body)


if __name__ == "__main__":
    main()