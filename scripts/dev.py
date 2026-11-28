#!/usr/bin/env python3
"""Watch the workspace, rebuild, and restart `meerai serve` only after a green build.

Runs under watchexec. File changes arrive as SIGHUP; each one schedules a rebuild. A failed
build leaves the running server untouched, so the browser tab keeps its connection.
"""
from __future__ import annotations

import argparse
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "debug" / "meerai"
LOG = ROOT / "target" / "dev-serve.log"
DEFAULT_PORT = 28860
NO_SESSION_HINT = "no sessions found to continue"

WATCH_EXTENSIONS = "rs,toml,yml,css,html,js,svg,py"
WATCH_IGNORE = ("dist", ".git", "target")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="dev.py",
        description="Rebuild and restart the meerai browser server on every change.",
    )
    parser.add_argument(
        "--supervise",
        action="store_true",
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--no-web",
        action="store_true",
        help="Skip the Wasm bundle build (Rust-only loop)",
    )
    parser.add_argument(
        "--no-resume",
        action="store_true",
        help="Always start a fresh session instead of resuming the latest one",
    )
    parser.add_argument(
        "--debounce",
        default="400ms",
        help="watchexec debounce window (default: 400ms)",
    )
    args, extra = parser.parse_known_args(argv)
    args.serve_args = [arg for arg in extra if arg != "--"]
    return args


def serve_port(serve_args: list[str]) -> int:
    for index, arg in enumerate(serve_args):
        if arg.startswith("--port="):
            return int(arg.split("=", 1)[1])
        if arg == "--port" and index + 1 < len(serve_args):
            return int(serve_args[index + 1])
    return DEFAULT_PORT


def log_tail(lines: int = 40) -> str:
    try:
        text = LOG.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    tail = text.splitlines()[-lines:]
    return "\n".join(tail)


def run_step(cmd: list[str]) -> bool:
    print(f"$ {' '.join(cmd)}", flush=True)
    started = time.monotonic()
    try:
        result = subprocess.run(
            cmd,
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    except OSError as error:
        print(f"build failed: {error}", flush=True)
        return False
    elapsed = time.monotonic() - started
    if result.returncode == 0:
        print(f"  ok in {elapsed:.1f}s", flush=True)
        return True
    output = "\n".join(result.stdout.splitlines()[-60:])
    print(f"  FAILED in {elapsed:.1f}s\n{output}", flush=True)
    return False


def build(no_web: bool) -> bool:
    if not no_web:
        web = [
            sys.executable,
            str(ROOT / "scripts" / "build-web.py"),
            "--dev",
        ]
        if not run_step(web):
            return False
    cargo = shutil.which("cargo") or "cargo"
    return run_step([cargo, "build", "-p", "meerai-cli", "--features", "web"])


def wait_for_port(port: int, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.settimeout(0.2)
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.1)
    return False


class Supervisor:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.port = serve_port(args.serve_args)
        self.resume = not args.no_resume
        self.child: subprocess.Popen[bytes] | None = None
        self.pending = True
        self.running = True
        LOG.parent.mkdir(parents=True, exist_ok=True)
        LOG.write_text("", encoding="utf-8")

    def request_rebuild(self, _signum: int, _frame: object) -> None:
        self.pending = True

    def request_stop(self, _signum: int, _frame: object) -> None:
        self.running = False

    def child_command(self) -> list[str]:
        command = [str(BIN), "serve"]
        if self.resume:
            command.append("--continue")
        return command + self.args.serve_args

    def stop_child(self) -> None:
        child = self.child
        self.child = None
        if child is None or child.poll() is not None:
            return
        print("stopping server", flush=True)
        child.send_signal(signal.SIGINT)
        try:
            child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()

    def start_child(self) -> bool:
        command = self.child_command()
        for attempt in (0, 1):
            with LOG.open("ab") as log:
                log.write(f"\n=== {' '.join(command)} ===\n".encode())
                log.flush()
                self.child = subprocess.Popen(
                    command,
                    cwd=ROOT,
                    stdin=subprocess.DEVNULL,
                    stdout=log,
                    stderr=log,
                    start_new_session=True,
                )
            if wait_for_port(self.port, 25):
                print(
                    f"serving http://127.0.0.1:{self.port} (log: {LOG.relative_to(ROOT)})",
                    flush=True,
                )
                return True
            exited = self.child.poll()
            self.child = None
            if exited is None:
                self.stop_child()
                print(f"server did not listen on {self.port}", flush=True)
                print(log_tail(), flush=True)
                return False
            if attempt == 0 and self.resume and NO_SESSION_HINT in log_tail(200):
                print("no session to resume yet; starting a fresh one", flush=True)
                self.resume = False
                command = self.child_command()
                continue
            print(f"server exited with status {exited}", flush=True)
            print(log_tail(), flush=True)
            return False
        return False

    def cycle(self) -> None:
        if not build(self.args.no_web):
            state = "still serving" if self.child and self.child.poll() is None else "no server"
            print(f"build failed; {state} the previous binary", flush=True)
            return
        self.stop_child()
        self.start_child()

    def run(self) -> int:
        signal.signal(signal.SIGHUP, self.request_rebuild)
        signal.signal(signal.SIGINT, self.request_stop)
        signal.signal(signal.SIGTERM, self.request_stop)
        print(
            f"watching for changes; Ctrl-C to stop. Basic auth defaults to meerai/12345678.",
            flush=True,
        )
        while self.running:
            if self.pending:
                self.pending = False
                self.cycle()
                continue
            if self.child is not None and self.child.poll() is not None:
                status = self.child.returncode
                self.child = None
                print(f"server exited with status {status}; waiting for a change", flush=True)
                print(log_tail(), flush=True)
            time.sleep(0.15)
        self.stop_child()
        return 0


def launch_watcher(args: argparse.Namespace) -> int:
    watchexec = shutil.which("watchexec")
    if watchexec is None:
        print(
            "watchexec is not installed.\n"
            "  cargo install watchexec-cli --locked\n"
            "  brew install watchexec",
            file=sys.stderr,
        )
        return 1
    command = [
        watchexec,
        "--workdir",
        str(ROOT),
        "--exts",
        WATCH_EXTENSIONS,
    ]
    for pattern in WATCH_IGNORE:
        command += ["--ignore", pattern]
    command += [
        "--debounce",
        args.debounce,
        "--shell=none",
        "--on-busy-update",
        "signal",
        "--signal",
        "SIGHUP",
        "--stop-signal",
        "SIGTERM",
        "--stop-timeout",
        "20s",
        "--",
        sys.executable,
        str(Path(__file__).resolve()),
        "--supervise",
    ]
    if args.no_web:
        command.append("--no-web")
    if args.no_resume:
        command.append("--no-resume")
    if args.serve_args:
        command.append("--")
        command += args.serve_args
    try:
        return subprocess.run(command, cwd=ROOT).returncode
    except KeyboardInterrupt:
        return 0


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.supervise:
        return Supervisor(args).run()
    return launch_watcher(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
