"""Versioned, opt-in Rust CPU primitives with a strict Python fallback.

Rust is never enabled by default. Set VOLLEY_RUST_IOU=1 explicitly after
building the cdylib, then use parity tests before switching project runs.
"""
from __future__ import annotations

import ctypes
from functools import lru_cache
import os
from pathlib import Path
import sys
from typing import Sequence

ABI_VERSION = 1
ROOT = Path(__file__).resolve().parents[2]


def python_bbox_iou(a: Sequence[float], b: Sequence[float]) -> float:
    """Original CPU replay IoU semantics, including epsilon denominator."""
    left, top = max(a[0], b[0]), max(a[1], b[1])
    right, bottom = min(a[2], b[2]), min(a[3], b[3])
    intersection = max(0.0, right - left) * max(0.0, bottom - top)
    area_a = max(0.0, a[2] - a[0]) * max(0.0, a[3] - a[1])
    area_b = max(0.0, b[2] - b[0]) * max(0.0, b[3] - b[1])
    return intersection / max(1e-9, area_a + area_b - intersection)


def library_path() -> Path:
    override = os.environ.get("VOLLEY_RUST_CORE_LIBRARY")
    if override:
        return Path(override)
    return _default_library_path()


@lru_cache(maxsize=1)
def _default_library_path() -> Path:
    name = (
        "volley_core.dll" if sys.platform == "win32"
        else "libvolley_core.dylib" if sys.platform == "darwin"
        else "libvolley_core.so"
    )
    for profile in ("release", "debug"):
        path = ROOT / "rust" / "volley-core" / "target" / profile / name
        if path.is_file():
            return path
    return ROOT / "rust" / "volley-core" / "target" / "debug" / name


@lru_cache(maxsize=4)
def _load_library(path: str) -> tuple[ctypes.CDLL, ctypes._CFuncPtr]:
    dll = ctypes.CDLL(path)
    version = dll.volley_core_abi_version
    version.argtypes = []
    version.restype = ctypes.c_uint32
    actual = version()
    if actual != ABI_VERSION:
        raise RuntimeError(f"Rust ABI mismatch: expected {ABI_VERSION}, got {actual}")
    iou = dll.volley_bbox_iou_v1
    iou.argtypes = [ctypes.c_double] * 8
    iou.restype = ctypes.c_double
    return dll, iou


@lru_cache(maxsize=4)
def _rust_iou_function(path: str):
    # Perform disk/ABI validation exactly once per distinct library path.
    target = Path(path)
    if not target.is_file():
        raise FileNotFoundError(
            f"Rust kernel not built: {target}. Run cargo build --manifest-path "
            "rust/volley-core/Cargo.toml before enabling VOLLEY_RUST_IOU=1."
        )
    _library, function = _load_library(str(target.resolve()))
    return function


def rust_bbox_iou(a: Sequence[float], b: Sequence[float]) -> float:
    # The first native build targets x86_64-pc-windows-msvc. A 32-bit
    # interpreter cannot load its DLL (Windows error 193); report the
    # incompatibility explicitly instead of silently switching algorithms.
    if sys.platform == "win32" and ctypes.sizeof(ctypes.c_void_p) != 8:
        raise RuntimeError("Rust volley-core DLL requires 64-bit Python on Windows")
    fn = _rust_iou_function(str(library_path()))
    return float(fn(*(float(v) for v in (*a, *b))))


def bbox_iou(a: Sequence[float], b: Sequence[float]) -> float:
    """Use the Python oracle unless Rust was enabled explicitly."""
    if os.environ.get("VOLLEY_RUST_IOU") == "1":
        return rust_bbox_iou(a, b)  # Must fail loudly, never silently fall back.
    return python_bbox_iou(a, b)
