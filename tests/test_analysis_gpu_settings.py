"""CUDA/CPU selection is explicit, and GPU resource flags reach project jobs."""
import json
import tempfile
import unittest
from pathlib import Path
from contextlib import nullcontext
from unittest.mock import MagicMock, patch

from scripts.volleyball_studio.__main__ import parser
from scripts.volleyball_ui import server


BASE=Path(__file__).resolve().parents[1]
HTML=BASE/"scripts/volleyball_ui/static/index.html"
APP=BASE/"scripts/volleyball_ui/static/app.js"
PRESETS=BASE/"scripts/volleyball_ui/static/analysis-presets.js"


class AnalysisGpuSettingsTests(unittest.TestCase):
    def test_gpu_default_and_user_choice_in_players_section(self):
        page=HTML.read_text(encoding="utf-8")
        players=page.split('id="analysis-pipeline-advanced"',1)[1].split('id="analysis-stage-performance"',1)[0]
        self.assertIn('name="player_model"',players)
        self.assertIn('name="player_backend" id="analysis-player-device"',players)
        self.assertIn('<option value="onnx-cuda" selected>GPU',players)
        self.assertIn('<option value="onnx-cpu">CPU',players)
        self.assertEqual(page.count('name="player_backend"'),1)
        self.assertIn('name="gpu_device_id"',page)
        self.assertIn('name="gpu_memory_limit_mb"',page)
        app=APP.read_text(encoding="utf-8")
        self.assertIn('$("#analysis-gpu-resources").hidden=',app)

    def test_all_built_in_profiles_select_cuda(self):
        text=PRESETS.read_text(encoding="utf-8")
        # Accuracy, speed, new and amateur ReID profiles all use explicit CUDA.
        self.assertEqual(text.count('player_backend:"onnx-cuda"'),4)
        self.assertEqual(text.count('backend:"onnx",'),4)

    def test_studio_accepts_documented_gpu_flags(self):
        args=parser().parse_args(["--gpu-device-id","0","--gpu-memory-limit-mb","4096"])
        self.assertEqual((args.gpu_device_id,args.gpu_memory_limit_mb),(0,4096))
        self.assertEqual(parser().parse_args([]).gpu_memory_limit_mb,0)

    def test_api_job_passes_resources_and_explicit_cpu_player_backend(self):
        with tempfile.TemporaryDirectory() as folder:
            proc=MagicMock()
            proc.poll.return_value=None
            job={"process":None,"id":None,"kind":None,"log":None,
                 "started_at":None,"command":[]}
            with (patch.object(server,"JOB",job),
                  patch.object(server,"project_videos",return_value=[Path("match.mp4")]),
                  patch("scripts.volleyball_ui.prepared_videos.status",
                        return_value={"ready":1,"total":1,"videos":[]}),
                  patch.object(server,"project_logs_root",return_value=Path(folder)),
                  patch("scripts.volleyball_ui.project_lifecycle.job_launch_guard",
                        return_value=nullcontext()),
                  patch.object(server.subprocess,"Popen",return_value=proc) as popen):
                server.start_job("analyze",{"project_id":"project",
                         "gpu_device_id":0,"gpu_memory_limit_mb":4096,
                         "backend":"onnx","player_backend":"onnx-cpu",
                         "threads":26})
            cmd=popen.call_args.args[0]
            self.assertEqual(cmd[cmd.index("--gpu-device-id")+1],"0")
            self.assertEqual(cmd[cmd.index("--gpu-memory-limit-mb")+1],"4096")
            self.assertEqual(cmd[cmd.index("--player-backend")+1],"onnx-cpu")
            self.assertEqual(cmd[cmd.index("--threads")+1],"26")

    def test_profile_flags_reach_studio_parser_without_device_contradictions(self):
        from scripts.volleyball_studio.ball_models import DEFAULT_BALL_MODEL, FAST_BALL_MODEL
        from scripts.volleyball_studio.advanced import validated, as_cli
        cases=(
            ("new", "Новый", DEFAULT_BALL_MODEL, "RAVEL-VB-013-9f"),
            ("accuracy", "Точность", DEFAULT_BALL_MODEL, "RAVEL-VB-012-9f"),
            ("speed", "Скорость", FAST_BALL_MODEL, "RAVEL-VB-011-9f"),
        )
        generated=[]
        for profile_id, profile_name, ball_model, player_model in cases:
            with self.subTest(profile=profile_id), tempfile.TemporaryDirectory() as folder:
                proc=MagicMock()
                proc.poll.return_value=None
                job={"process":None,"id":None,"kind":None,"log":None,
                     "started_at":None,"command":[]}
                payload={"project_id":"project","profile_id":profile_id,
                         "profile_name":profile_name,"backend":"onnx",
                         "ball_model":ball_model,"player_model":player_model,
                         "player_backend":"onnx-cuda","player_device":"GPU",
                         "gpu_device_id":0,"gpu_memory_limit_mb":0,
                         "player_action_threshold":0.5,"threads":32}
                with (patch.object(server,"JOB",job),
                      patch.object(server,"project_videos",return_value=[Path("match.mp4")]),
                      patch("scripts.volleyball_ui.prepared_videos.status",
                            return_value={"ready":1,"total":1,"videos":[]}),
                      patch.object(server,"project_logs_root",return_value=Path(folder)),
                  patch("scripts.volleyball_ui.project_lifecycle.job_launch_guard",
                        return_value=nullcontext()),
                      patch.object(server.subprocess,"Popen",return_value=proc) as popen):
                    server.start_job("analyze",payload)
                cmd=popen.call_args.args[0]
                self.assertTrue(cmd[1].endswith("run_project_analysis.py"))
                self.assertEqual(cmd[cmd.index("--profile-id")+1],profile_id)
                studio_flags=cmd[cmd.index("--backend"):]
                parsed=parser().parse_args(studio_flags)
                self.assertEqual(parsed.ball_model,ball_model)
                self.assertEqual(parsed.player_model,player_model)
                self.assertEqual(parsed.backend,"onnx")
                self.assertEqual(parsed.player_backend,"onnx-cuda")
                self.assertEqual(parsed.player_device,"GPU")
                self.assertEqual(parsed.gpu_device_id,0)
                self.assertEqual(parsed.gpu_memory_limit_mb,0)
                self.assertEqual(parsed.threads,32)
                self.assertEqual(parsed.player_action_threshold,.5)
                cli=as_cli(validated(payload))
                self.assertEqual(cli[cli.index("--player-model")+1],player_model)
                generated.append(cmd)
        self.assertNotEqual(generated[0][generated[0].index("--player-model")+1],
                            generated[1][generated[1].index("--player-model")+1])
        self.assertNotEqual(generated[0][generated[0].index("--ball-model")+1],
                            generated[2][generated[2].index("--ball-model")+1])

    def test_gpu_resources_are_validated_before_launch(self):
        for key,value in (("gpu_device_id",-1),("gpu_device_id",16),
                          ("gpu_memory_limit_mb",-1),("gpu_memory_limit_mb",131073)):
            with self.subTest(field=key,value=value):
                with patch.object(server,"project_videos",return_value=[Path("match.mp4")]):
                    with self.assertRaises(ValueError):
                        server.start_job("analyze",{"project_id":"project",key:value})


if __name__=="__main__":
    unittest.main()
