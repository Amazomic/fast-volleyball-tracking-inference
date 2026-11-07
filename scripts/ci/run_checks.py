"""Launch PR tests on the local machine; GitHub Actions is not required."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def check(label: str, command: list[str], env: dict[str, str] | None = None) -> bool:
    print(f"\n== {label}: {' '.join(command)}", flush=True)
    result = subprocess.run(command, cwd=ROOT, env=env, check=False)
    if result.returncode:
        print(f"FAIL: {label} (exit {result.returncode})", file=sys.stderr)
    return result.returncode == 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Local PR checks (no cloud runner)")
    parser.add_argument("--full", action="store_true", help="Sync and run the entire pytest suite")
    parser.add_argument("--skip-ui", action="store_true", help="Skip Node tests explicitly")
    args = parser.parse_args(argv)
    if sys.version_info < (3, 14):
        parser.error("Python 3.14 or newer is required")

    basic = [
        ("Python syntax", [sys.executable, "-m", "compileall", "-q", "main.py", "src", "scripts"]),
        ("Architecture and golden defaults", [sys.executable, "scripts/ci/repo_policy.py"]),
        ("Policy and quality tests", [sys.executable, "-m", "unittest", "discover",
                                      "-s", "tests", "-p", "test_ci_quality.py", "-v"]),
    ]
    for label, command in basic:
        if not check(label, command):
            return 1
    if args.skip_ui:
        print("NOT TESTED: JavaScript checks skipped explicitly", flush=True)
    else:
        node = shutil.which("node")
        if not node:
            parser.error("Node.js not found; install Node.js 22+ or use --skip-ui")
        for source in sorted((ROOT / "scripts/volleyball_ui/static").glob("*.js")):
            if not check("JavaScript syntax", [node, "--check", str(source)]):
                return 1
        tests = sorted((ROOT / "tests").glob("*.cjs"))
        if not tests:
            parser.error("No JS tests found")
        if not check("UI contracts", [node, "--test", *(str(p) for p in tests)]):
            return 1
    if args.full:
        uv = shutil.which("uv")
        if not uv:
            parser.error("uv not found; install uv for --full")
        if not check("Locked dev dependencies", [uv, "sync", "--frozen", "--extra", "dev"]):
            return 1
        env = dict(os.environ)
        env["PYTHONPATH"] = os.pathsep.join((".", "src", env.get("PYTHONPATH", "")))
        if not check("Full Python regressions",
                     [uv, "run", "--no-sync", "pytest", "-q", "tests"], env):
            return 1
    print("LOCAL CHECKS PASSED" + (" (UI NOT TESTED)" if args.skip_ui else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
