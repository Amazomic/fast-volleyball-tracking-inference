#!/usr/bin/env python3
"""Compatibility CLI for the current Volley Studio analysis pipelines.

The browser application is started with ``python -m scripts.volleyball_ui``.
This entry point keeps the older ``--mode`` interface working where a real
implementation exists; it must never report success without doing work.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
DEFAULT_MODEL = ROOT / "models/VballNetV4c_seq9_grayscale_20260908_213829.onnx"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Volleyball inference CLI (legacy wrapper)")
    parser.add_argument("--mode", choices=("track", "analyze", "pose"), default="track")
    parser.add_argument("--video_path", help="Source video for --mode track")
    parser.add_argument("--model_path", default=str(DEFAULT_MODEL), help="ONNX ball model")
    parser.add_argument("--output_dir", default="output")
    parser.add_argument("--visualize", action="store_true")
    parser.add_argument("--only-csv", "--only_csv", dest="only_csv", action="store_true")
    parser.add_argument("--track_file", help="Legacy pose input (pose mode is unavailable)")
    parser.add_argument("--project-id", help="Registered Studio project for --mode analyze")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args, extra = parser.parse_known_args(argv)

    if args.mode == "track":
        if not args.video_path:
            parser.error("--video_path is required for --mode track")
        if not Path(args.video_path).is_file():
            parser.error(f"Video not found: {args.video_path}")
        if not Path(args.model_path).is_file():
            parser.error(f"Model not found: {args.model_path}")
        if args.project_id or args.track_file:
            parser.error("--project-id/--track_file are not valid for --mode track")
        command = [sys.executable, str(ROOT / "src/inference_onnx_seq_gray_v2.py"),
                   "--video_path", args.video_path, "--model_path", args.model_path,
                   "--output_dir", args.output_dir]
        if args.visualize:
            command.append("--visualize")
        if args.only_csv:
            command.append("--only_csv")
    elif args.mode == "analyze":
        if not args.project_id:
            parser.error("--project-id is required for --mode analyze")
        if args.video_path or args.track_file or args.visualize or args.only_csv:
            parser.error("Track/pose options cannot be combined with --mode analyze")
        command = [sys.executable, "-m", "scripts.run_project_analysis",
                   "--project-id", args.project_id]
    else:
        parser.error("--mode pose is not implemented: the legacy pose detector "
                     "does not supply player detections. Use Studio's RAVEL "
                     "player tracking instead.")

    # Unknown mode-specific switches are forwarded to the real parser, which
    # validates them instead of silently ignoring unsupported settings.
    command.extend(extra)
    return subprocess.run(command, cwd=ROOT, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
