#!/usr/bin/env python3
"""Zero-dependency repository and pull-request guardrails for Volley Studio."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
GOLD = "scripts/volleyball_ui/static/cpu-defaults.json"
GOLD_DOC = "docs/CPU_GOLDEN_DEFAULTS.md"
REPORT_DIR = "docs/quality/reports/"
RUNTIME_DIRS = ("results/", "project/", "uploads/", "studio_output/", "studio_work/")
DOCKER_RELEVANT = (
    "Dockerfile", ".dockerignore", "pyproject.toml", "uv.lock", ".python-version",
    "scripts/volleyball_ui/server.py", "scripts/volleyball_ui/__main__.py",
)
REPORT_METRICS = ("touch_f1", "rally_boundary_mae_sec", "identity_switches",
                  "identity_collisions")


class PolicyError(ValueError):
    pass


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True,
                                   stderr=subprocess.PIPE).strip()


def changed_paths(base: str | None) -> set[str] | None:
    if not base or set(base) == {"0"}:
        return None
    try:
        return set(filter(None, git("diff", "--name-only", f"{base}...HEAD").splitlines()))
    except subprocess.CalledProcessError as exc:
        raise PolicyError(f"Cannot compare PR with base {base}: {exc.stderr}") from exc


def check_repository(root: Path = ROOT) -> None:
    required = (
        "scripts/volleyball_ui/server.py",
        "scripts/run_project_analysis.py",
        "scripts/run_project_reprocess.py",
        "scripts/volleyball_studio/reprocess.py",
        "scripts/volleyball_ui/static/index.html",
        "src/inference_onnx_seq_gray_v2.py",
        "src/make_reels.py",
        GOLD,
        GOLD_DOC,
    )
    missing = [path for path in required if not (root / path).is_file()]
    if missing:
        raise PolicyError("Missing required architecture components: " + ", ".join(missing))
    version = (root / ".python-version").read_text(encoding="utf-8").strip()
    spec = tomllib.loads((root / "pyproject.toml").read_text(encoding="utf-8"))
    if version != "3.14" or not spec["project"]["requires-python"].startswith(">=3.14"):
        raise PolicyError("Python version disagrees with the tested 3.14 environment")
    gold = json.loads((root / GOLD).read_text(encoding="utf-8"))
    if gold.get("schema") != "volley-cpu-golden-defaults/1":
        raise PolicyError("Unexpected golden CPU settings schema")
    if not isinstance(gold.get("revision"), int) or gold["revision"] <= 0:
        raise PolicyError("Golden CPU settings must have a positive integer revision")
    settings = gold.get("settings", {})
    if settings.get("mode") not in ("combined", "baseline", "ravel-touch", "ravel-geometry"):
        raise PolicyError("Unsupported golden CPU replay mode")
    for setting in ("ball_threshold", "touch_threshold", "player_threshold"):
        value = settings.get(setting)
        if type(value) not in (int, float) or not 0 <= value <= 1:
            raise PolicyError(f"Invalid golden setting {setting}: {value!r}")


def check_changes(paths: set[str] | None, base: str | None,
                  root: Path = ROOT) -> bool:
    """Validate PR diff and return whether an image smoke build is required."""
    if paths is None:
        return True  # manual/scheduled run: never silently skip Docker
    offenders = [p for p in paths if p.startswith(RUNTIME_DIRS)
                 or (Path(p).name.startswith(".env") and Path(p).name != ".env.example")]
    if offenders:
        raise PolicyError("Runtime media or secret-like config cannot be committed: "
                          + ", ".join(sorted(offenders)))
    if GOLD in paths:
        if not base:
            raise PolicyError("Golden settings changes require a comparable base")
        try:
            before = json.loads(git("show", f"{base}:{GOLD}"))
        except (ValueError, subprocess.CalledProcessError) as exc:
            raise PolicyError("Cannot read baseline golden revision") from exc
        after = json.loads((root / GOLD).read_text(encoding="utf-8"))
        old_revision, new_revision = before.get("revision"), after.get("revision")
        if type(old_revision) is not int or new_revision != old_revision + 1:
            raise PolicyError("Golden settings revision must increment exactly once")
        if GOLD_DOC not in paths:
            raise PolicyError(f"Update {GOLD_DOC} when changing golden settings")
        reports = sorted(p for p in paths if p.startswith(REPORT_DIR)
                         and p.endswith(".json") and (root / p).is_file())
        if not reports:
            raise PolicyError("Golden changes require a real annotated-match quality "
                              "report under docs/quality/reports/")
        valid = False
        for report_path in reports:
            payload = json.loads((root / report_path).read_text(encoding="utf-8"))
            if (payload.get("schema") != "volley-quality-evidence/1"
                    or payload.get("baseline_revision") != old_revision
                    or payload.get("candidate_revision") != new_revision):
                continue
            if not payload.get("dataset_id") or not re.fullmatch(
                    r"[a-fA-F0-9]{64}", str(payload.get("annotation_sha256", ""))):
                continue
            measurements = payload.get("metrics", {})
            if not all(k in measurements for k in REPORT_METRICS):
                continue
            # CI validates evidence structure; an independent reviewer must
            # verify the annotation provenance and evaluate the real reel.
            valid = True
        if not valid:
            raise PolicyError("No report with matching revisions, dataset and "
                              "full metric evidence was found")
    return any(p in DOCKER_RELEVANT or p.startswith("docker/") for p in paths)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="PR structure and golden baseline policy")
    parser.add_argument("--base", default="", help="Target branch commit SHA")
    parser.add_argument("--github-output", default="", help="GitHub Actions output file")
    args = parser.parse_args(argv)
    try:
        check_repository()
        paths = changed_paths(args.base)
        needs_docker = check_changes(paths, args.base)
        if args.github_output:
            with open(args.github_output, "a", encoding="utf-8") as output:
                output.write(f"docker={'true' if needs_docker else 'false'}\n")
        print(f"Repository policy: PASS; changed files="
              f"{len(paths) if paths is not None else 'unknown'}; docker={needs_docker}")
        return 0
    except (PolicyError, ValueError, OSError, json.JSONDecodeError) as exc:
        print(f"Repository policy: FAIL: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
