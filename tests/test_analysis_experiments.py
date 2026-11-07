"""Isolated analysis experiments, safe result versions, and reproducible comparisons."""
import csv
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import run_analysis_experiment as exp


class AnalysisExperimentTests(unittest.TestCase):
    def setUp(self):
        self.project="bac4db86497a"
        self.video="VID_20261031_190829.mp4"
        self.options={"project_id":self.project,"video":self.video,"start":24.5,
                      "seconds":30,"mode":"players","backend":"onnx-cpu",
                      "threads":6,"ball_threshold":.5,"analysis_width":"auto",
                      "profile_id":"new","profile_name":"Новый"}
        self.video_info={"duration_sec":3600,"auto_width":1080,"width":1080,
                         "height":1920}

    def test_range_model_and_stage_validation(self):
        with patch.object(exp,"assert_project_video"),patch.object(exp,"video_info",return_value=self.video_info):
            cfg=exp.validate_config(self.options)
            self.assertEqual(cfg["analysis_width"],1080)
            self.assertEqual(cfg["mode"],"players")
            self.assertEqual(cfg["seconds"],30)
            full=exp.validate_config({**self.options,"mode":"full"})
            self.assertTrue(full["advanced"]["player_output_video"])
            self.assertFalse(cfg["advanced"]["player_output_video"])
            for override in ({"seconds":3601},{"seconds":1},{"start":3599,"seconds":30},
                             {"start":-1},{"mode":"none"},{"ball_threshold":0},
                             {"ball_model":"bad_model.onnx"},{"analysis_width":77}):
                with self.subTest(override=override),self.assertRaises(ValueError):
                    exp.validate_config({**self.options,**override})

    def test_histories_are_project_scoped_and_media_is_allowlisted(self):
        with tempfile.TemporaryDirectory() as td:
            base=Path(td)
            with patch.object(exp,"experiment_root",return_value=base):
                (base/"runs").mkdir()
                (base/"variants/test").mkdir(parents=True)
                (base/"variants/test/players.mp4").write_bytes(b"mp4")
                entry={"id":"20261101_145000_test","video":self.video,"mode":"players",
                       "status":"complete","elapsed_sec":4.7,
                       "video_file":"variants/test/players.mp4"}
                (base/"runs/20261101_145000_test.json").write_text(json.dumps(entry),encoding="utf8")
                saved=exp.history(self.project)
                self.assertEqual(saved[0]["mode"],"players")
                self.assertEqual(exp.result_media(self.project,entry["id"]),
                                 base/"variants/test/players.mp4")
                with self.assertRaises(ValueError):
                    exp.result_media(self.project,"../../etc/passwd")
                entry["video_file"]="../../private.mp4"
                (base/"runs/20261101_145000_test.json").write_text(json.dumps(entry),encoding="utf8")
                with self.assertRaises(FileNotFoundError):
                    exp.result_media(self.project,entry["id"])

    def test_saved_variants_never_replace_each_other(self):
        with tempfile.TemporaryDirectory() as td:
            base=Path(td)
            source=base/"real-match.mp4"
            source.write_bytes(b"video")
            calls=[]
            def fake_run(command,log):
                calls.append([str(c) for c in command])
                if "ffmpeg" in str(command[0]):
                    Path(command[-1]).write_bytes(b"x"*24000)
                else:
                    target=Path(command[command.index("--output")+1])
                    target.parent.mkdir(parents=True,exist_ok=True)
                    target.write_text(json.dumps({
                        "fps":30,"predictions":[
                            {"class_name":"player","track_id":1,
                             "frame_index":i,"score":.95} for i in range(25)]
                    }),encoding="utf8")
                    video=Path(command[command.index("--output-video")+1])
                    video.write_bytes(b"mp4")
            from scripts.volleyball_studio.advanced import validated
            opts={**self.options,"seconds":3,"advanced":validated({}),"analysis_width":1080}
            with patch.object(exp,"validate_config",side_effect=lambda c:c),\
                 patch.object(exp,"experiment_root",return_value=base),\
                 patch.object(exp,"video_path",return_value=source),\
                 patch("scripts.volleyball_ui.prepared_videos.ready_path",return_value=source),\
                 patch.object(exp,"run_checked",side_effect=fake_run):
                exp.execute(opts,run_id="20261101_145001_a")
                exp.execute({**opts,"advanced":{**opts["advanced"],"player_hysteresis_frames":8}},
                            run_id="20261101_145002_b")
            self.assertEqual(len(calls),3) # Prepared clip reused, RAVEL rerun twice.
            with patch.object(exp,"experiment_root",return_value=base):
                runs=exp.history(self.project)
            self.assertEqual(len(runs),2,repr(runs))
            self.assertTrue(all(x["status"]=="complete" for x in runs))
            self.assertEqual([x["metrics"]["track_ids"] for x in runs],[1,1])
            self.assertEqual(len(list((base/"prepared").glob("*.mp4"))),1)
            self.assertEqual(len(list((base/"variants").iterdir())),2)


    def test_full_tests_keep_separate_work_directories(self):
        with tempfile.TemporaryDirectory() as td:
            base=Path(td)
            source=base/"match.mp4"
            source.write_bytes(b"source")
            dirs=[]
            def fake_run(command, log):
                Path(command[-1]).write_bytes(b"x"*25000)
            def fake_analyze(source_path,work,args):
                dirs.append(work)
                cache=work/"cache"
                cache.mkdir(parents=True)
                (cache/"players-annotated.mp4").write_bytes(b"mp4")
                return [],[],cache
            from scripts.volleyball_studio.advanced import validated
            opts={**self.options,"mode":"full","seconds":3,"beach":True,
                  "ball_model":"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
                  "analysis_width":1080,"advanced":validated({"player_output_video":True})}
            with patch.object(exp,"validate_config",side_effect=lambda cfg:cfg),\
                 patch.object(exp,"experiment_root",return_value=base),\
                 patch.object(exp,"video_path",return_value=source),\
                 patch("scripts.volleyball_ui.prepared_videos.ready_path",return_value=source),\
                 patch.object(exp,"run_checked",side_effect=fake_run),\
                 patch("scripts.volleyball_ui.court.court_path",return_value=base/"no_court.json"),\
                 patch("scripts.volleyball_studio.analysis.analyze",side_effect=fake_analyze):
                exp.execute(opts,run_id="20261101_145100_a")
                exp.execute({**opts,"ball_threshold":.6},run_id="20261101_145102_b")
            self.assertEqual(len(dirs),2)
            self.assertNotEqual(dirs[0],dirs[1])
            self.assertEqual(dirs[0].parent,base/"pipeline")
            self.assertTrue((base/"runs/20261101_145100_a.json").exists())
            self.assertTrue((base/"runs/20261101_145102_b.json").exists())
            self.assertNotIn("VOLLEY_EXPERIMENT",__import__("os").environ)

    def test_saved_ball_metrics(self):
        with tempfile.TemporaryDirectory() as td:
            path=Path(td)/"ball.csv"
            path.write_text("Frame,Visibility,X,Y,Radius\n0,0,-1,-1,0\n1,1,15,15,2\n2,1,17,16,2\n",encoding="utf8")
            self.assertEqual(exp.count_ball(path),
                             {"frames":3,"ball_detected":2,"ball_coverage_percent":66.7})


if __name__=="__main__":
    unittest.main()
