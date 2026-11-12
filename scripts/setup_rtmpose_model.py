#!/usr/bin/env python3
"""Pin and install the official RTMPose-M Body7 ONNX model (setup helper only)."""
import hashlib
import os
from pathlib import Path
import shutil
import tempfile
from urllib.request import Request, urlopen
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "models" / "RTMPose-M-Body7.onnx"
URL = ("https://download.openmmlab.com/mmpose/v1/projects/rtmposev1/onnx_sdk/"
       "rtmpose-m_simcc-body7_pt-body7_420e-256x192-e48f03d0_20230504.zip")
ZIP_SHA = "f7fbb6c5c11a1bb70f3d445e4ddec5d144ea89ad5649c081ef976f9b24a0b741"
MODEL_SHA = "5c0a4bf67953e6d2ac43ce15e77dc9d5d354ae18430a47d2c5963a7bc5683e3c"
MEMBER = ("20230831/rtmpose_onnx/"
          "rtmpose-m_simcc-body7_pt-body7_420e-256x192-e48f03d0_20230504/end2end.onnx")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    if TARGET.is_file() and sha256(TARGET) == MODEL_SHA:
        print("RTMPose verified:", TARGET)
        return
    TARGET.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="rtmpose-download-") as folder:
        archive_path = Path(folder) / "model.zip"
        with urlopen(Request(URL, headers={"User-Agent":"VolleyStudio/1.0"}), timeout=90) as response:
            with archive_path.open("wb") as sink:
                shutil.copyfileobj(response, sink)
        if sha256(archive_path) != ZIP_SHA:
            raise RuntimeError("RTMPose archive SHA mismatch")
        with ZipFile(archive_path) as archive:
            member = archive.getinfo(MEMBER)
            if not 1024 <= member.file_size <= 256 * 1024 * 1024:
                raise RuntimeError("Unexpected RTMPose size")
            temporary_path = Path(folder) / "model.onnx"
            with archive.open(member) as source, temporary_path.open("wb") as sink:
                shutil.copyfileobj(source, sink)
        if sha256(temporary_path) != MODEL_SHA:
            raise RuntimeError("RTMPose ONNX SHA mismatch")
        # Use a temporary file in destination directory for atomic same-volume rename.
        with tempfile.NamedTemporaryFile(dir=TARGET.parent, prefix=".rtmpose-", delete=False) as temp:
            staged = Path(temp.name)
            with temporary_path.open("rb") as source:
                shutil.copyfileobj(source, temp)
            temp.flush()
            os.fsync(temp.fileno())
        try:
            os.replace(staged, TARGET)
        finally:
            staged.unlink(missing_ok=True)
    print("RTMPose installed:", TARGET)


if __name__ == "__main__":
    main()
