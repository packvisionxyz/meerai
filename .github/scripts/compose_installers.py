#!/usr/bin/env python3
import argparse
import json
import re
import shutil
from pathlib import Path

BEGIN_MARKER = "# BEGIN GENERATED REQUIREMENTS"
END_MARKER = "# END GENERATED REQUIREMENTS"


def load_requirements(path: Path) -> list[dict[str, str]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, list):
        raise ValueError(f"{path} must contain a JSON array")
    reqs: list[dict[str, str]] = []
    for idx, item in enumerate(data):
        if not isinstance(item, dict):
            raise ValueError(f"requirement[{idx}] must be an object")
        req: dict[str, str] = {}
        for key in ("command", "reason", "setup"):
            val = item.get(key)
            if not isinstance(val, str) or not val.strip():
                raise ValueError(f"requirement[{idx}].{key} must be a non-empty string")
            if "\n" in val or "\r" in val:
                raise ValueError(f"requirement[{idx}].{key} must be single-line")
            req[key] = val
        reqs.append(req)
    return reqs


def quote_sh(value: str) -> str:
    return "'" + value.replace("'", "'\\''") + "'"


def quote_ps1(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def render_sh_block(reqs: list[dict[str, str]]) -> str:
    if not reqs:
        return "    :"
    lines = [
        f"    check_requirement {quote_sh(r['command'])} {quote_sh(r['reason'])} {quote_sh(r['setup'])}"
        for r in reqs
    ]
    return "\n".join(lines)


def render_ps1_block(reqs: list[dict[str, str]]) -> str:
    lines = [
        f"    Test-Requirement -Command {quote_ps1(r['command'])} -Reason {quote_ps1(r['reason'])} -Setup {quote_ps1(r['setup'])}"
        for r in reqs
    ]
    return "\n".join(lines)


def replace_block(content: str, body: str, path: Path) -> str:
    pattern = re.compile(
        rf"(?P<indent>[ \t]*){re.escape(BEGIN_MARKER)}\r?\n.*?^[ \t]*{re.escape(END_MARKER)}",
        re.MULTILINE | re.DOTALL,
    )
    matches = list(pattern.finditer(content))
    if len(matches) != 1:
        raise ValueError(f"{path} must contain exactly one {BEGIN_MARKER}..{END_MARKER} block")
    indent = matches[0].group("indent")
    replacement_lines = [f"{indent}{BEGIN_MARKER}"]
    if body:
        replacement_lines.append(body)
    replacement_lines.append(f"{indent}{END_MARKER}")
    replacement = "\n".join(replacement_lines)
    return content[: matches[0].start()] + replacement + content[matches[0].end() :]


def compose(site_dir: Path, out_dir: Path | None, in_place: bool) -> None:
    reqs = load_requirements(site_dir / "requirements.json")

    if in_place:
        target_dir = site_dir
    elif out_dir is not None:
        if out_dir.exists():
            shutil.rmtree(out_dir)
        shutil.copytree(site_dir, out_dir)
        target_dir = out_dir
    else:
        raise ValueError("specify --out-dir or --in-place")

    sh_path = target_dir / "install.sh"
    sh_updated = replace_block(
        sh_path.read_text(encoding="utf-8"),
        render_sh_block(reqs),
        sh_path,
    )
    sh_path.write_text(sh_updated, encoding="utf-8", newline="\n")

    ps1_path = target_dir / "install.ps1"
    ps1_updated = replace_block(
        ps1_path.read_text(encoding="utf-8"),
        render_ps1_block(reqs),
        ps1_path,
    )
    ps1_path.write_text(ps1_updated, encoding="utf-8", newline="\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--site-dir", type=Path, default=Path("site"))
    parser.add_argument("--out-dir", type=Path)
    parser.add_argument("--in-place", action="store_true")
    args = parser.parse_args()
    compose(args.site_dir, args.out_dir, args.in_place)


if __name__ == "__main__":
    main()
