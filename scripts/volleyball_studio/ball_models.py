"""Supported ball detector models for Volley Studio analysis."""
from pathlib import Path

from .common import ROOT

DEFAULT_BALL_MODEL = "VballNetV4c_seq9_grayscale_20260908_213829.onnx"
FAST_BALL_MODEL = "VballNetFastV1_seq9_grayscale_233_h288_w512.onnx"
BALL_MODELS = (DEFAULT_BALL_MODEL, FAST_BALL_MODEL)


def resolve_ball_model(name: str) -> tuple[Path, Path]:
    """Return ONNX and OpenVINO inputs, restricted to supported model names."""
    if name not in BALL_MODELS:
        raise ValueError("Неизвестная модель мяча")
    onnx = ROOT / "models" / name
    if not onnx.is_file():
        raise FileNotFoundError(f"Модель мяча не найдена: {onnx}")
    xml = ROOT / "ov" / f"{Path(name).stem}.xml"
    return onnx, xml if xml.is_file() and xml.with_suffix(".bin").is_file() else onnx
