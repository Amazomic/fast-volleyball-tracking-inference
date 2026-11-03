"""Quick Reel: bounded music uploads and project -> analysis -> montage chain."""
from __future__ import annotations

import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import quick_reel_audio, quick_reel_runner


class MusicUploadTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.folder=Path(self.tmp.name)
        self.patch=patch.object(quick_reel_audio,"ROOT",self.folder/"audio")
        self.patch.start()
        self.addCleanup(self.patch.stop)
        quick_reel_audio._UPLOADS.clear()
        self.addCleanup(quick_reel_audio._UPLOADS.clear)
        self.music=self.folder/"track.mp3"
        subprocess.run([
            "ffmpeg","-nostdin","-hide_banner","-loglevel","error","-y",
            "-f","lavfi","-i","sine=frequency=440:sample_rate=24000",
            "-t","12","-c:a","libmp3lame","-q:a","8",str(self.music)
        ],check=True,timeout=25)

    def test_upload_and_audio_length(self):
        data=self.music.read_bytes()
        reply=quick_reel_audio.start("track.mp3",len(data))
        token=reply["upload_id"]
        midpoint=len(data)//2
        quick_reel_audio.append(token,0,io.BytesIO(data[:midpoint]),midpoint)
        quick_reel_audio.append(token,midpoint,io.BytesIO(data[midpoint:]),len(data)-midpoint)
        recorded=quick_reel_audio.finish(token)
        self.assertTrue(Path(recorded["path"]).is_file())
        self.assertAlmostEqual(recorded["duration"],12,delta=.2)
        self.assertAlmostEqual(quick_reel_audio.inspect_audio(recorded["path"])["duration"],12,delta=.2)

    def test_reject_bad_and_incomplete_audio(self):
        with self.assertRaises(ValueError):
            quick_reel_audio.start("../bad.mp3",30)
        with self.assertRaises(ValueError):
            quick_reel_audio.start("track.exe",30)
        token=quick_reel_audio.start("track.mp3",100)["upload_id"]
        with self.assertRaises(ValueError):
            quick_reel_audio.finish(token)
        quick_reel_audio.abort(token)


class QuickReelStandaloneTests(unittest.TestCase):
    def setUp(self):
        from scripts.volleyball_ui import quick_reel_jobs
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.source=self.root/"game.mp4"
        self.source.write_bytes(b"source-original")
        self.audio=self.root/"song.mp3"
        self.audio.write_bytes(b"song")
        patch_root=patch.object(quick_reel_jobs,"QUICK_ROOT",self.root/"quick_reels")
        patch_source=patch("scripts.volleyball_ui.data.video_path",
                           side_effect=lambda name:self.source)
        patch_root.start();patch_source.start()
        self.addCleanup(patch_root.stop);self.addCleanup(patch_source.stop)
        self.jobs=quick_reel_jobs

    def create(self):
        from scripts.volleyball_ui.quick_reel_selection import save_selections
        token=save_selections([str(self.source)])[0]["id"]
        return self.jobs.create_quick_reel([token],{
            "path":str(self.audio),"duration":34.5,
        })

    def test_each_run_reanalyzes_despite_cached_preparation(self):
        from scripts.volleyball_ui import quick_reel_render
        seen=[]
        def prepare(name,progress):
            progress(100,True)
            return self.root/"shared.mp4",True
        def analyze(reel_id,rows):
            seen.append(reel_id)
            rows[0]["state"]="analyzed"
        def render(reel_id,audio,duration,progress):
            progress(2,True);progress(4,True)
            path=self.jobs.quick_dir(reel_id)/"montage"/"highlights_reel.mp4"
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_bytes(b"mp4")
            return path
        with (patch.object(quick_reel_runner,"prepare_source",side_effect=prepare),
              patch.object(quick_reel_runner,"_analyze",side_effect=analyze),
              patch.object(quick_reel_render,"render_reel",side_effect=render),
              patch("scripts.volleyball_ui.quick_reel_audio.inspect_audio",
                    return_value={"path":str(self.audio),"duration":34.5}),
              patch("scripts.volleyball_ui.montage_audio._streams",
                    return_value={"format":{"duration":"34.4"}})):
            first,_=self.create()
            quick_reel_runner.execute(self.jobs.request_for(first))
            second,_=self.create()
            quick_reel_runner.execute(self.jobs.request_for(second))
        self.assertNotEqual(first,second)
        self.assertEqual(seen,[first,second],"Fresh model inference is mandatory")
        for reel_id in (first,second):
            folder=self.jobs.quick_dir(reel_id)
            self.assertTrue((folder/"result.json").is_file())
            self.assertTrue((folder/"status.json").is_file())
            self.assertFalse((folder/"project.json").exists())
            result=json.loads((folder/"result.json").read_text())
            self.assertEqual(result["video"],"montage/highlights_reel.mp4")
            self.assertTrue(result["default_analysis"])
            self.assertEqual(result["duration"],34.4)
        self.assertFalse((self.root/"projects").exists())

    def test_changed_music_is_rejected_before_analysis(self):
        identifier,_=self.create()
        with (patch("scripts.volleyball_ui.quick_reel_audio.inspect_audio",
                    return_value={"path":str(self.audio),"duration":75}),
              patch.object(quick_reel_runner,"_analyze") as analyze):
            with self.assertRaisesRegex(ValueError,"Музыка изменилась"):
                quick_reel_runner.execute(self.jobs.request_for(identifier))
            analyze.assert_not_called()

    def test_fresh_analysis_uses_default_model_and_gpu(self):
        from scripts.volleyball_ui.quick_reel_analyze import default_arguments
        from scripts.volleyball_ui.quick_reel_render import quick_render_defaults
        args=default_arguments()
        self.assertEqual(args.backend,"onnx")
        self.assertEqual(args.ball_threshold,.5)
        self.assertIn("VballNetV4c",args.ball_model)
        self.assertEqual(args.player_model,"RAVEL-VB-013-9f")
        self.assertEqual(args.player_backend,"onnx-cuda")
        self.assertEqual(args.player_device,"GPU")
        self.assertEqual(args.gpu_memory_limit_mb,0)
        self.assertEqual(args.player_score_threshold,.42)
        self.assertEqual(args.player_action_threshold,.5)
        montage=quick_render_defaults(self.audio,34.5)
        self.assertTrue(montage["follow_ball"])
        self.assertEqual(montage["selection_strategy"],"highlights")
        self.assertEqual(montage["min_confidence"],.5)


if __name__=="__main__":
    unittest.main()
