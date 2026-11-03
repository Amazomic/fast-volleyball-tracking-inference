"""RAVEL ONNX Runtime / OpenVINO compatibility and explicit CUDA selection."""
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from scripts.volleyball_studio.advanced import as_cli, validated
from src.inference_player_ball_openvino import (
    RavelVBONNX, RavelVBOpenVINO, decode_batch,
)

ROOT=Path(__file__).resolve().parents[1]


class RavelONNXTests(unittest.TestCase):
    def test_backend_argument_round_trip(self):
        from scripts.volleyball_studio.__main__ import parser
        for backend in ("openvino", "onnx-cpu", "onnx-cuda"):
            value=validated({"player_backend": backend})
            args=parser().parse_args(as_cli(value))
            self.assertEqual(args.player_backend, backend)
        with self.assertRaises(ValueError):
            validated({"player_backend": "unsupported"})

    def test_onnx_cpu_outputs_match_openvino_abi(self):
        path=ROOT/"models/RAVEL-VB-012-9f.onnx"
        onnx=RavelVBONNX(path, num_threads=4, device="cpu")
        openvino=RavelVBOpenVINO(ROOT/"ov/RAVEL-VB-012-9f.xml",
                                 num_threads=4, performance_hint="LATENCY",
                                 device="CPU")
        self.assertEqual(onnx.config["clip_length"],openvino.config["clip_length"])
        self.assertEqual(onnx.config["image_width"],openvino.config["image_width"])
        frame=np.zeros((720,1280,3),dtype=np.uint8)
        np.testing.assert_array_equal(onnx.preprocess(frame),openvino.preprocess(frame))
        sample=np.stack([onnx.preprocess(frame)]*9)[None]
        out_onnx,_=onnx.infer(sample)
        out_ov,_=openvino.infer(sample)
        self.assertEqual(set(out_onnx),set(out_ov))
        for name in out_onnx:
            self.assertEqual(out_onnx[name].shape,out_ov[name].shape)
            self.assertTrue(np.isfinite(out_onnx[name]).all())
        self.assertIsInstance(decode_batch(out_onnx,0.4,onnx.ball_width,onnx.ball_height),list)

    def test_cuda_provider_must_not_silently_fall_back_to_cpu(self):
        path=ROOT/"models/RAVEL-VB-012-9f.onnx"
        with (patch("onnxruntime.get_available_providers",return_value=["CUDAExecutionProvider","CPUExecutionProvider"]),
              patch("onnxruntime.preload_dlls"),
              patch("onnxruntime.InferenceSession") as session):
            session.return_value.get_providers.return_value=["CPUExecutionProvider"]
            with self.assertRaisesRegex(RuntimeError,"refusing CPU fallback"):
                RavelVBONNX(path,num_threads=1,device="cuda")


if __name__=="__main__":
    unittest.main()
