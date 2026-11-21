#!/usr/bin/env python3
"""Prove the release commit changes only the version lines of the workspace manifests."""

import os
import re
import subprocess
import sys

ALLOWED_PATHS = ("Cargo.toml", "Cargo.lock")
VERSION_LINE = re.compile(r'^version = "([^"]+)"\s*$')


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def notice(message: str) -> None:
    print(f"::notice::{message}")


def warning(message: str) -> None:
    print(f"::warning::{message}")


def git(*args: str) -> str:
    result = subprocess.run(("git",) + args, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        fail(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def changed_paths(parent: str, head: str) -> list[str]:
    output = git("diff", "--name-only", "--no-renames", parent, head)
    return [line.strip() for line in output.splitlines() if line.strip()]


def version_lines(parent: str, head: str, path: str) -> tuple[list[str], list[str]]:
    output = git("diff", "--unified=0", "--no-renames", parent, head, "--", path)
    removed: list[str] = []
    added: list[str] = []
    for line in output.splitlines():
        if line.startswith(("+++", "---")):
            continue
        if line.startswith("+"):
            added.append(line[1:])
        elif line.startswith("-"):
            removed.append(line[1:])
    return removed, added


def collect_version_problems(
    path: str, removed: list[str], added: list[str], version: str
) -> list[str]:
    problems: list[str] = []
    for side, lines in (("removes", removed), ("adds", added)):
        for line in lines:
            if not VERSION_LINE.match(line):
                problems.append(f"{path} {side} a line that is not a version line: {line.strip()}")
    if len(removed) != len(added):
        problems.append(f"{path} removes {len(removed)} version lines but adds {len(added)}")
    if not added:
        problems.append(f"{path} has no version line change")
    for line in added:
        match = VERSION_LINE.match(line)
        if match and match.group(1) != version:
            problems.append(f"{path} sets version {match.group(1)}, but the release is {version}")
    return problems


def report(problems: list[str], event: str, tag: str, head: str) -> None:
    if not problems:
        return
    if event == "workflow_dispatch":
        warning(f"Manual dispatch on {head[:7]}, which is not a pure bump commit:")
        for problem in problems:
            warning(f"- {problem}")
        return
    fail(f"{tag} is not a pure version bump commit.")
    for problem in problems:
        print(f"::error::{problem}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    version = os.environ.get("VERSION", "").strip()
    event = os.environ.get("EVENT", "").strip()
    tag = os.environ.get("TAG", "").strip() or f"v{version}"
    if not version:
        fail("VERSION is required")

    head = git("rev-parse", "HEAD").strip()
    parent = git("rev-parse", "-q", "--verify", "HEAD^").strip()
    if not parent:
        notice(f"{head[:7]} is the first commit; nothing to compare.")
        return

    paths = changed_paths(parent, head)
    problems: list[str] = []
    unexpected = [path for path in paths if path not in ALLOWED_PATHS]
    if unexpected:
        problems.append("the release commit also changes: " + ", ".join(unexpected))
    if not paths:
        problems.append("the release commit changes no files")

    lock_versions = 0
    for path in ALLOWED_PATHS:
        if path not in paths:
            problems.append(f"the release commit does not change {path}")
            continue
        removed, added = version_lines(parent, head, path)
        problems.extend(collect_version_problems(path, removed, added, version))
        if path == "Cargo.toml" and len(added) != 1:
            problems.append(f"Cargo.toml must change exactly one version line, not {len(added)}")
        if path == "Cargo.lock":
            lock_versions = len(added)

    report(problems, event, tag, head)
    if problems:
        return

    notice(
        f"{tag} is a pure version bump: {head[:7]} changes only the workspace version "
        f"in Cargo.toml and {lock_versions} Cargo.lock entries."
    )
    print(f"Verified {tag} at {head}")


if __name__ == "__main__":
    main()
