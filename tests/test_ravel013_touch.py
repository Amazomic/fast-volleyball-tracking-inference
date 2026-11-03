"""New RAVEL-013 model, Touch decoding and pipeline configuration."""
from pathlib import Path
import json
import unittest

import numpy as np

from scripts.volleyball_studio.advanced import validated, as_cli
from src.inference_player_ball_openvino import RavelVBONNX, decode_batch, PlayerHysteresis

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / "models/RAVEL-VB-013-9f.onnx"


class Ravel013TouchTests(unittest.TestCase):
    def test_model_is_installed_with_action_head(self):
        self.assertGreater(MODEL.stat().st_size, 5_000_000)
        meta=json.loads(MODEL.with_suffix(".json").read_text(encoding="utf8"))
        self.assertEqual(meta["release_id"], "RAVEL-VB-013")
        self.assertEqual(meta["action_names"], ["Touch", "none"])
        self.assertIn("action_logits", meta["output_names"])

    def test_profile_settings_and_gpu_default_kept(self):
        options=validated({"player_model":"RAVEL-VB-013-9f",
                           "player_backend":"onnx-cuda",
                           "player_action_threshold":0.7})
        self.assertEqual(options["player_action_threshold"],0.7)
        self.assertIn("RAVEL-VB-013-9f",as_cli(options))
        with self.assertRaises(ValueError):
            validated({"player_action_threshold":1.1})
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf8")
        self.assertIn('value="RAVEL-VB-013-9f"',html)
        self.assertIn('name="player_action_threshold"',html)
        profiles=(ROOT/"scripts/volleyball_ui/static/analysis-presets.js").read_text(encoding="utf8")
        self.assertIn('player_model:"RAVEL-VB-012-9f"',profiles)
        self.assertIn('player_model:"RAVEL-VB-011-9f"',profiles)

    def test_touch_probability_is_decoded_and_thresholded(self):
        outputs={
            "logits":np.array([[[[0,5]]]],dtype=np.float32),
            "boxes":np.array([[[[.5,.5,.3,.7]]]],dtype=np.float32),
            "action_logits":np.log(np.array([[[[.87,.13]]]],dtype=np.float32))
        }
        sample=decode_batch(outputs,0.35,0,0,action_names=["Touch","none"],
                            action_threshold=0.7)[0]
        self.assertEqual(len(sample),1)
        self.assertEqual(sample[0]["action"],"Touch")
        self.assertAlmostEqual(sample[0]["action_score"],0.87,places=2)
        sample=decode_batch(outputs,0.35,0,0,action_names=["Touch","none"],
                            action_threshold=0.95)[0]
        self.assertNotIn("action",sample[0])

    def test_old_model_without_action_tensors_keeps_legacy_predictions(self):
        outputs={
            "logits":np.array([[[[0,5]]]],dtype=np.float32),
            "boxes":np.array([[[[.5,.5,.3,.7]]]],dtype=np.float32)
        }
        sample=decode_batch(outputs,0.35,0,0,action_threshold=.5)[0]
        self.assertNotIn("action",sample[0])
        self.assertNotIn("action_score",sample[0])

    def test_interpolated_track_does_not_fake_a_new_touch(self):
        tracker=PlayerHysteresis(open_threshold=.35,close_threshold=.2,hold_frames=2)
        player={"class_name":"player","score":.9,
                "bbox_xyxy_norm":[.3,.2,.6,.8],"action":"Touch",
                "action_score":.93}
        observed=tracker.update(0,[player])
        self.assertEqual(observed[0]["action"],"Touch")
        held=tracker.update(1,[])
        self.assertEqual(len(held),1)
        self.assertTrue(held[0]["interpolated"])
        self.assertNotIn("action",held[0])
        self.assertNotIn("action_score",held[0])

    def test_actual_onnx_cpu_session_exposes_action_logits(self):
        model=RavelVBONNX(MODEL,num_threads=4,device="cpu")
        self.assertEqual(model.action_names,["Touch","none"])
        c=model.config
        sample=np.zeros((1,c["clip_length"],c["input_channels"],
                         c["image_height"],c["image_width"]),dtype=np.float32)
        outputs,elapsed=model.infer(sample)
        self.assertIn("action_logits",outputs)
        self.assertEqual(outputs["action_logits"].shape[-1],2)
        self.assertGreater(elapsed,0)
        self.assertTrue(np.isfinite(outputs["action_logits"]).all())


if __name__=="__main__":
    unittest.main()
