"""Diagnostic for the supported Studio inference/runtime installation.

Unlike the old YOLO demo checker, this verifies files used by current entry
points. The check does not load models or invoke the GPU.
"""

from __future__ import annotations

from importlib.util import find_spec
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]
REQUIRED_FILES = (
    "models/VballNetV4c_seq9_grayscale_20260908_213829.onnx",
    "models/RAVEL-VB-013-9f.onnx",
    "scripts/volleyball_ui/server.py",
    "scripts/run_project_analysis.py",
    "src/inference_onnx_seq_gray_v2.py",
)
REQUIRED_COMMANDS = ("ffmpeg", "ffprobe")
REQUIRED_IMPORTS = ("cv2", "numpy", "onnxruntime", "openvino", "pandas", "scipy")


def missing_requirements(root: Path = ROOT) -> list[str]:
    """Report absent *current* requirements, without importing heavy libraries."""
    missing = [str(root / path) for path in REQUIRED_FILES
               if not (root / path).is_file()]
    missing.extend(command for command in REQUIRED_COMMANDS
                   if shutil.which(command) is None)
    missing.extend(module for module in REQUIRED_IMPORTS
                   if find_spec(module) is None)
    return missing


def check_files(root: Path = ROOT) -> bool:
    """Backwards-compatible boolean probe suitable for smoke checks."""
    missing = missing_requirements(root)
    if missing:
        print("Missing Studio requirements:")
        for item in missing:
            print(f"  - {item}")
        print("Install project dependencies with `uv sync`, install FFmpeg, "
              "and check the model files under models/.")
        return False
    print("Studio requirements are present (GPU and inference not tested).")
    return True


if __name__ == "__main__":
    raise SystemExit(0 if check_files() else 1)
