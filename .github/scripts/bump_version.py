#!/usr/bin/env python3
"""Bump [workspace.package] version in Cargo.toml and every workspace entry in Cargo.lock."""

import argparse
import os
import re
import sys
import tomllib
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import verify_release_version as gate

CARGO_TOML = Path(os.environ.get("CARGO_TOML", "Cargo.toml"))
CARGO_LOCK = Path(os.environ.get("CARGO_LOCK", "Cargo.lock"))
RELEASE_SUBJECT = "chore(release): bump version to "
LEVELS = ("major", "minor", "patch")
VERSION_LINE = re.compile(
    r"(?P<indent>\s*)version\s*=\s*(?P<quote>['\"])(?P<value>[^'\"]*)(?P=quote)"
)


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def notice(message: str) -> None:
    print(f"::notice::{message}")


def semver_key(version: str):
    parts = gate.SEMVER.fullmatch(version)
    if parts is None:
        fail(f"{version!r} is not a valid semantic version")
    pre = parts.group("pre") or ""
    return (
        int(parts.group("major")),
        int(parts.group("minor")),
        int(parts.group("patch")),
        0 if pre else 1,
        pre,
    )


def normalize(version: str) -> str:
    version = version.strip()
    if version[:1] in ("v", "V"):
        version = version[1:]
    if gate.SEMVER.fullmatch(version) is None:
        fail(f"{version!r} is not a valid semantic version")
    return version


def bump_level(current: str, level: str) -> str:
    parts = gate.SEMVER.fullmatch(current)
    if parts.group("pre") or parts.group("build"):
        fail(
            f"[workspace.package] version {current!r} is a pre-release; "
            "pass --version to choose the next version explicitly"
        )
    major = int(parts.group("major"))
    minor = int(parts.group("minor"))
    patch = int(parts.group("patch"))
    if level == "major":
        major, minor, patch = major + 1, 0, 0
    elif level == "minor":
        minor, patch = minor + 1, 0
    else:
        patch += 1
    return f"{major}.{minor}.{patch}"


def next_patch(version: str) -> str:
    parts = gate.SEMVER.fullmatch(version)
    if parts is None:
        fail(f"{version!r} is not a valid semantic version")
    base = "{}.{}.{}".format(parts.group("major"), parts.group("minor"), parts.group("patch"))
    if parts.group("pre"):
        return base
    return bump_level(base, "patch")


def workspace_members(root: Path) -> list[str]:
    manifest = root / "Cargo.toml"
    data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    names = []
    for pattern in data.get("workspace", {}).get("members", []):
        for path in sorted(root.glob(pattern)):
            member = path / "Cargo.toml"
            if not member.is_file():
                continue
            package = tomllib.loads(member.read_text(encoding="utf-8")).get("package", {})
            name = package.get("name")
            if isinstance(name, str) and name not in names:
                names.append(name)
    if not names:
        fail(f"could not resolve workspace members from {manifest}")
    return names


def rewrite_manifest(text: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    in_section = False
    for index, raw in enumerate(lines):
        stripped = raw.split("#", 1)[0].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_section = stripped[1:-1].strip() == "workspace.package"
            continue
        if not in_section:
            continue
        match = VERSION_LINE.fullmatch(stripped)
        if match is None or not match.group("value"):
            continue
        newline = "\n" if raw.endswith("\n") else ""
        quote = match.group("quote")
        lines[index] = f"{match.group('indent')}version = {quote}{version}{quote}{newline}"
        return "".join(lines)
    fail("could not find version under [workspace.package]")


def lock_blocks(lines: list[str]) -> list[dict]:
    blocks = []
    current = None
    for index, line in enumerate(lines):
        if line.strip() == "[[package]]":
            if current is not None:
                blocks.append(current)
            current = {"name": None, "version": None}
            continue
        if current is None:
            continue
        stripped = line.strip()
        if current["name"] is None and stripped.startswith("name = "):
            current["name"] = stripped.split("=", 1)[1].strip().strip('"')
        elif current["version"] is None and stripped.startswith("version = "):
            current["version"] = index
    if current is not None:
        blocks.append(current)
    return blocks


def rewrite_lock(text: str, names: set[str], version: str) -> tuple[str, set[str]]:
    lines = text.splitlines(keepends=True)
    touched = set()
    for block in lock_blocks(lines):
        name = block["name"]
        if name not in names or block["version"] is None:
            continue
        raw = lines[block["version"]]
        newline = "\n" if raw.endswith("\n") else ""
        indent = raw[: len(raw) - len(raw.lstrip())]
        lines[block["version"]] = f'{indent}version = "{version}"{newline}'
        touched.add(name)
    return "".join(lines), touched


def verify_lock(text: str, names: set[str], version: str) -> None:
    found = {}
    lines = text.splitlines(keepends=True)
    for block in lock_blocks(lines):
        if block["name"] in names:
            index = block["version"]
            found[block["name"]] = lines[index].split("=", 1)[1].strip().strip('"') if index is not None else ""
    missing = sorted(names - set(found))
    if missing:
        fail(f"Cargo.lock has no entry for workspace member(s): {', '.join(missing)}")
    stale = sorted(name for name, value in found.items() if value != version)
    if stale:
        fail(f"Cargo.lock still records an old version for: {', '.join(stale)}")


def write_output(pairs: dict) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    with open(output_path, "a", encoding="utf-8") as handle:
        for key, value in pairs.items():
            handle.write(f"{key}={value}\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--level", choices=LEVELS, default="patch")
    parser.add_argument("--version", default="")
    parser.add_argument("--message-out", default="")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--root", default=".")
    args = parser.parse_args()

    root = Path(args.root).resolve()
    manifest = root / CARGO_TOML
    lock = root / CARGO_LOCK
    for path in (manifest, lock):
        if not path.is_file():
            fail(f"cannot find {path}")

    current = gate.workspace_version(manifest)
    if not current:
        fail(f"could not read [workspace.package] version from {manifest}")

    wanted = normalize(args.version) if args.version else bump_level(current, args.level)
    if semver_key(wanted) <= semver_key(current):
        fail(f"new version {wanted} is not greater than the current version {current}")

    names = set(workspace_members(root))
    manifest_text = manifest.read_text(encoding="utf-8")
    lock_text = lock.read_text(encoding="utf-8")

    new_manifest = rewrite_manifest(manifest_text, wanted)
    new_lock, touched = rewrite_lock(lock_text, names, wanted)
    if touched != names:
        fail(f"Cargo.lock is missing workspace members: {', '.join(sorted(names - touched))}")
    verify_lock(new_lock, names, wanted)

    subject = f"{RELEASE_SUBJECT}{wanted}"
    tag = f"v{wanted}"

    if not args.dry_run:
        manifest.write_text(new_manifest, encoding="utf-8")
        lock.write_text(new_lock, encoding="utf-8")

    if args.message_out:
        message_path = Path(args.message_out)
        message_path.parent.mkdir(parents=True, exist_ok=True)
        message_path.write_text(subject + "\n", encoding="utf-8")

    write_output(
        {
            "version": wanted,
            "previous_version": current,
            "next_version": next_patch(wanted),
            "tag": tag,
            "subject": subject,
            "members": str(len(names)),
        }
    )

    prefix = "dry run: " if args.dry_run else ""
    print(f"{prefix}{current} -> {wanted} ({len(names)} workspace packages, tag {tag})")
    print(f"commit message: {subject}")
    if args.dry_run:
        print(f"Cargo.toml and Cargo.lock would be rewritten; nothing was written")
    notice(f"Bumped {current} to {wanted}")


if __name__ == "__main__":
    main()