"""Two built-in analysis presets select only models and compatible hardware."""
from pathlib import Path
import json
import subprocess
import unittest

ROOT=Path(__file__).resolve().parents[1]
PRESETS=ROOT/"scripts/volleyball_ui/static/analysis-presets.js"
HTML=ROOT/"scripts/volleyball_ui/static/index.html"


class TwoAnalysisProfilesTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Evaluate the actual preset module without running DOM init.
        script=r"""
const fs=require("node:fs");
const vm=require("node:vm");
const window={};
const document={readyState:"loading",addEventListener(){}};
vm.runInNewContext(fs.readFileSync(process.argv[1],"utf8"),{window,document});
console.log(JSON.stringify(window.VolleyballAnalysisPresets.presets));
"""
        result=subprocess.run(["node","-e",script,str(PRESETS)],
                              capture_output=True,text=True,check=True)
        cls.presets=json.loads(result.stdout)

    def test_accuracy_speed_and_new_presets(self):
        self.assertEqual(set(self.presets),{"accuracy","speed","new"})
        self.assertEqual(self.presets["accuracy"]["name"],"Точность")
        self.assertEqual(self.presets["speed"]["name"],"Скорость")
        self.assertEqual(self.presets["new"]["name"],"Новый")

    def test_model_device_pairs(self):
        self.assertEqual(self.presets["accuracy"]["values"],{
            "ball_model":"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
            "player_model":"RAVEL-VB-012-9f",
            "backend":"onnx",
            "player_backend":"onnx-cuda",
            "player_device":"GPU",
            "gpu_device_id":0,
            "gpu_memory_limit_mb":0,
        })
        self.assertEqual(self.presets["speed"]["values"],{
            "ball_model":"VballNetFastV1_seq9_grayscale_233_h288_w512.onnx",
            "player_model":"RAVEL-VB-011-9f",
            "backend":"onnx",
            "player_backend":"onnx-cuda",
            "player_device":"GPU",
            "gpu_device_id":0,
            "gpu_memory_limit_mb":0,
        })

    def test_new_gpu_profile_ravel013(self):
        self.assertEqual(self.presets["new"]["values"],{
            "ball_model":"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
            "player_model":"RAVEL-VB-013-9f",
            "backend":"onnx",
            "player_backend":"onnx-cuda",
            "player_device":"GPU",
            "gpu_device_id":0,
            "gpu_memory_limit_mb":0,
            "player_action_threshold":0.5,
        })

    def test_all_other_settings_inherited_from_form(self):
        text=PRESETS.read_text(encoding="utf-8")
        self.assertIn("return {...factory,...(all[id]?.values||{})};",text)
        self.assertIn('applyProfile("new");',text)
        self.assertIn('presets.new.values.threads=Math.min(128,Math.max(1,',text)
        html=HTML.read_text(encoding="utf-8")
        for field,value in (
            ("player_hysteresis_frames","60"),
            ("player_match_center_distance","0.15"),
            ("player_frame_step","1"),
        ):
            self.assertIn('name="'+field+'"',html)
            self.assertIn('value="'+value+'"',html)
        # CPU tracking settings are no longer part of the neural analysis UI.
        self.assertNotIn('name="track_max_distance"',html)


if __name__=="__main__":
    unittest.main()
