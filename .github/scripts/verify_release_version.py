#!/usr/bin/env python3
import os
import re
import sys
from pathlib import Path

TAG_PREFIX = "refs/tags/"
SEMVER = re.compile(
    r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)"
    r"(?P<pre>-(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?P<build>\+(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
TRUTHY = {"1", "true", "yes", "on"}
FALSY = {"0", "false", "no", "off"}


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def resolve_tag() -> str:
    event_name = os.environ.get("EVENT_NAME", "").strip()
    ref_name = os.environ.get("REF_NAME", "").strip()
    input_tag = os.environ.get("INPUT_TAG", "").strip()

    tag = input_tag if event_name == "workflow_dispatch" else ref_name
    if tag.startswith(TAG_PREFIX):
        tag = tag[len(TAG_PREFIX) :]
    tag = tag.strip()

    if not tag:
        fail(f"could not resolve a release tag (event {event_name or 'unknown'})")
    return tag


def workspace_version_from_lines(text: str) -> str:
    in_section = False
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            in_section = line[1:-1].strip() == "workspace.package"
            continue
        if not in_section:
            continue
        match = re.fullmatch(r"version\s*=\s*(['\"])(?P<value>[^'\"]*)\1", line)
        if match:
            return match.group("value").strip()
    return ""


def workspace_version(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    try:
        import tomllib
    except ModuleNotFoundError:
        return workspace_version_from_lines(text)

    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        fail(f"cannot parse {path}: {error}")

    package = data.get("workspace", {}).get("package", {})
    if not isinstance(package, dict):
        return ""
    version = package.get("version")
    return version.strip() if isinstance(version, str) else ""


def split_version(version: str):
    match = SEMVER.fullmatch(version)
    if match is None:
        return None
    base = "{}.{}.{}".format(
        match.group("major"), match.group("minor"), match.group("patch")
    )
    return base, match.group("pre") or ""


def resolve_prerelease(tag_pre: str) -> str:
    event_name = os.environ.get("EVENT_NAME", "").strip()
    if event_name != "workflow_dispatch":
        return "true" if tag_pre else "false"

    raw = os.environ.get("INPUT_PRERELEASE", "").strip().lower()
    if not raw:
        return "true" if tag_pre else "false"
    if raw in TRUTHY:
        return "true"
    if raw in FALSY:
        return "false"
    fail(f"invalid prerelease input {raw!r}; use true or false")


def tag_from_version() -> bool:
    return os.environ.get("TAG_FROM_VERSION", "").strip().lower() in TRUTHY


def main() -> None:
    cargo_toml = Path(os.environ.get("CARGO_TOML", "Cargo.toml"))

    if not cargo_toml.is_file():
        fail(f"cannot find {cargo_toml}")

    version = workspace_version(cargo_toml)
    if not version:
        fail(f"could not read version from [workspace.package] in {cargo_toml}")

    tag = f"v{version}" if tag_from_version() else resolve_tag()

    tag_version = tag[1:] if tag[:1] in ("v", "V") else tag
    tag_parts = split_version(tag_version)
    if tag_parts is None:
        fail(f"release tag {tag} is not a valid vX.Y.Z[-pre][+build] version")
    tag_base, tag_pre = tag_parts

    version_parts = split_version(version)
    if version_parts is None:
        fail(
            f"[workspace.package] version {version!r} in {cargo_toml} is not a valid version"
        )
    version_base, version_pre = version_parts

    exact_match = tag_version == version
    base_match = tag_base == version_base and not version_pre
    if not (exact_match or base_match):
        fail(
            f"release tag {tag} has version {tag_version} but {cargo_toml} has {version}. "
            "Bump [workspace.package] version or retag."
        )

    prerelease = resolve_prerelease(tag_pre)

    output_path = os.environ.get("GITHUB_OUTPUT")
    if output_path:
        with open(output_path, "a", encoding="utf-8") as handle:
            handle.write(f"tag={tag}\nversion={version}\nprerelease={prerelease}\n")

    print(
        f"Tag '{tag}' matches workspace version '{version}' (prerelease: {prerelease})."
    )


if __name__ == "__main__":
    main()
