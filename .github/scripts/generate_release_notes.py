#!/usr/bin/env python3
"""Build a release body from the commits and the changed files since the previous tag."""

import os
import re
import subprocess
import sys
from pathlib import Path

MAX_COMMITS = int(os.environ.get("NOTES_MAX_COMMITS", "100"))
MAX_FILES = int(os.environ.get("NOTES_MAX_FILES", "30"))
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

GROUP_WORKFLOWS = "Pipelines and scripts"
GROUP_SCRIPTS = "Local build scripts"
GROUP_SITE = "Installers and landing page"
GROUP_DOCS = "Documentation"
DOC_EXTENSIONS = {".md", ".mdx", ".rst", ".txt"}
PREFERRED_GROUPS = [
    "Version and workspace manifest",
    "Lockfile",
    GROUP_WORKFLOWS,
    GROUP_SCRIPTS,
    GROUP_SITE,
]
FILE_KINDS = {"A": "Added", "M": "Modified", "D": "Removed", "R": "Renamed", "C": "Copied"}


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


def changed_files(revision_range: str) -> list[dict]:
    return parse_files(git("diff", "--name-status", "--find-renames", revision_range))


def history_files() -> list[dict]:
    head = git("rev-parse", "HEAD").strip()
    roots = git_optional("rev-list", "--max-parents=0", "HEAD").splitlines()
    if roots and roots[-1] != head:
        return changed_files(f"{roots[-1]}..{head}")
    return parse_files(git("show", "--name-status", "--format=", head))


def parse_files(raw: str) -> list[dict]:
    seen = {}
    for line in raw.splitlines():
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        status = fields[0][:1]
        path = fields[-1]
        kind = FILE_KINDS.get(status, "Changed")
        if seen.get(path) == "Added" and kind == "Modified":
            continue
        seen[path] = kind
    files = []
    for path, kind in seen.items():
        parts = Path(path).parts
        files.append(
            {
                "kind": kind,
                "path": path,
                "directory": parts[0] if parts else "",
            }
        )
    files.sort(key=lambda entry: entry["path"])
    return files


def file_group(entry: dict) -> str:
    path = entry["path"]
    if path == "Cargo.toml":
        return "Version and workspace manifest"
    if path == "Cargo.lock":
        return "Lockfile"
    if path.startswith(".github/"):
        return GROUP_WORKFLOWS
    if path.startswith("scripts/"):
        return GROUP_SCRIPTS
    if path.startswith("site/"):
        return GROUP_SITE
    if path.startswith("meerai-") and entry["directory"]:
        return f"Crate `{entry['directory']}`"
    if Path(path).suffix.lower() in DOC_EXTENSIONS or path.startswith("docs/"):
        return GROUP_DOCS
    return "Other files"


def group_commits(commits: list[dict]) -> dict[str, list[dict]]:
    grouped: dict[str, list[dict]] = {}
    for commit in commits:
        grouped.setdefault(commit["group"], []).append(commit)
    return {title: grouped[title] for title in TYPE_ORDER if title in grouped}


def group_files(files: list[dict]) -> dict[str, list[dict]]:
    grouped: dict[str, list[dict]] = {}
    for entry in files:
        grouped.setdefault(file_group(entry), []).append(entry)
    result = {title: grouped[title] for title in PREFERRED_GROUPS if title in grouped}
    for title in sorted(grouped):
        if title not in result:
            result[title] = grouped[title]
    return result


def render_body(context: dict) -> str:
    version = context["version"]
    lines = [f"meerai {version}", ""]
    lines.append(f"Workspace version `{context['previous_version']}` -> `{version}`.")
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

    grouped_files = group_files(context["files"])
    if grouped_files:
        lines.append("")
        lines.append(f"## Files changed ({len(context['files'])})")
        shown = 0
        for title, items in grouped_files.items():
            if shown >= MAX_FILES:
                break
            budget = items[: MAX_FILES - shown]
            shown += len(budget)
            lines.append("")
            lines.append(f"### {title}")
            lines.append("")
            for entry in budget:
                lines.append(f"- {entry['kind']} `{entry['path']}`")
            hidden = len(items) - len(budget)
            if hidden > 0:
                lines.append(f"- and {hidden} more file(s)")
        remaining = len(context["files"]) - shown
        if remaining > 0:
            lines.append("")
            lines.append(f"Full list: {context['compare_url']}")

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
    files = changed_files(revision_range) if base_tag else history_files()

    context = {
        "version": version,
        "previous_version": os.environ.get("PREVIOUS_VERSION", "").strip() or version,
        "previous_tag": base_tag,
        "commits": commits,
        "files": files,
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
            handle.write(f"file_count={len(files)}\n")

    summary_env = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_env:
        with open(summary_env, "a", encoding="utf-8") as handle:
            handle.write(body)

    if not base_tag:
        print("::warning::No previous release tag found; notes cover the whole history.")
    print(f"Notes for {tag}: {len(commits)} commit(s), {len(files)} changed file(s), base {base_tag or 'none'}")
    if not output_path:
        sys.stdout.write(body)


if __name__ == "__main__":
    main()