#!/usr/bin/env python3
import os
import sys
from pathlib import Path


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def workspace_version(path: Path) -> str:
    in_section = False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if line.startswith("[") and line.endswith("]"):
            in_section = line == "[workspace.package]"
            continue
        if in_section and line.startswith("version") and "=" in line:
            return line.split("=", 1)[1].strip().strip('"')
    return ""


def main() -> None:
    event_name = os.environ.get("EVENT_NAME", "")
    ref_name = os.environ.get("REF_NAME", "")
    input_tag = os.environ.get("INPUT_TAG", "")
    input_prerelease = os.environ.get("INPUT_PRERELEASE", "false")
    cargo_toml = Path(os.environ.get("CARGO_TOML", "Cargo.toml"))

    if event_name == "workflow_dispatch":
        tag = input_tag
        prerelease = input_prerelease
    else:
        tag = ref_name
        prerelease = "true" if "-" in tag else "false"

    if not tag:
        fail(f"could not resolve a release tag (event {event_name})")

    if not cargo_toml.is_file():
        fail(f"cannot find {cargo_toml}")

    version = workspace_version(cargo_toml)
    if not version:
        fail(f"could not read version from [workspace.package] in {cargo_toml}")

    tag_version = tag[1:] if tag.startswith("v") else tag
    tag_base = tag_version.split("-", 1)[0].split("+", 1)[0]
    if not tag_base:
        fail(f"release tag {tag} does not contain a version")

    if tag_base != version:
        fail(
            f"release tag {tag} has version {tag_base} but {cargo_toml} has {version}. "
            "Bump [workspace.package] version or retag."
        )

    output_path = os.environ.get("GITHUB_OUTPUT")
    if output_path:
        with open(output_path, "a", encoding="utf-8") as handle:
            handle.write(f"tag={tag}\nprerelease={prerelease}\n")

    print(
        f"Tag '{tag}' matches workspace version '{version}' (prerelease: {prerelease})."
    )


if __name__ == "__main__":
    main()
