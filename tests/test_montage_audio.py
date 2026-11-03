"""Soundtrack selection and MP4 overlay without copying the music source."""
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import montage_audio, montage_runner, data, projects


class MontageAudioTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root=Path(tmp.name)
        source=self.root/"originals"
        source.mkdir()
        self.audio=source/"music.wav"
        self.video=self.root/"montage.mp4"
        subprocess.run(["ffmpeg","-nostdin","-hide_banner","-loglevel","error","-y",
                        "-f","lavfi","-i","testsrc2=size=160x90:rate=30",
                        "-t","2","-c:v","libx264","-pix_fmt","yuv420p",
                        str(self.video)],check=True,timeout=25)
        subprocess.run(["ffmpeg","-nostdin","-hide_banner","-loglevel","error","-y",
                        "-f","lavfi","-i","sine=frequency=320:sample_rate=44100",
                        "-t","0.45",str(self.audio)],check=True,timeout=20)

    @staticmethod
    def ffprobe(path):
        proc=subprocess.run(["ffprobe","-v","error",
                            "-show_entries","stream=codec_type:format=duration",
                            "-of","json",str(path)],capture_output=True,
                            text=True,check=True,timeout=10)
        return json.loads(proc.stdout)

    def test_soundtrack_loop_added_without_reencoding_video(self):
        video_before=self.video.read_bytes()
        source_before=self.audio.read_bytes()
        before=self.ffprobe(self.video)
        self.assertEqual([row["codec_type"] for row in before["streams"]],["video"])
        result=montage_audio.overlay_music(self.video,self.audio,40)
        after=self.ffprobe(self.video)
        self.assertEqual(set(row["codec_type"] for row in after["streams"]),
                         {"video","audio"})
        self.assertAlmostEqual(float(after["format"]["duration"]),2,delta=.14)
        self.assertEqual(result["volume_percent"],40)
        self.assertFalse(result["mixed_with_original"])
        self.assertEqual(self.audio.read_bytes(),source_before)
        self.assertNotEqual(self.video.read_bytes(),video_before)
        self.assertFalse((self.root/"montage.music-tmp.mp4").exists())

    def test_original_audio_is_mixed_when_present(self):
        with_audio=self.root/"original.mp4"
        subprocess.run(["ffmpeg","-nostdin","-hide_banner","-loglevel","error","-y",
                        "-i",str(self.video),"-f","lavfi","-i",
                        "sine=frequency=200:sample_rate=44100",
                        "-t","2","-map","0:v:0","-map","1:a:0",
                        "-c:v","copy","-c:a","aac",str(with_audio)],
                       check=True,timeout=20)
        result=montage_audio.overlay_music(with_audio,self.audio,25)
        self.assertTrue(result["mixed_with_original"])
        self.assertEqual(set(row["codec_type"]
                             for row in self.ffprobe(with_audio)["streams"]),
                         {"video","audio"})

    def test_audio_validation(self):
        self.assertIsNone(montage_audio.resolve_audio(""))
        self.assertEqual(montage_audio.resolve_audio(str(self.audio)),self.audio)
        for name in ("invalid.txt","missing.mp3","./track.mp3"):
            with self.subTest(name=name),self.assertRaises(ValueError):
                montage_audio.resolve_audio(name)
        for value in (None, -1, 101, "NaN"):
            with self.subTest(volume=value),self.assertRaises(ValueError):
                montage_audio.validate_volume(value)

    def test_generated_rally_track_is_compatible_with_actual_track_processor(self):
        import sys
        from scripts.volleyball_ui.montage_project import build_rally_payload
        track_folder=self.root/"source-tracks"
        track_folder.mkdir()
        sample={
            "track_id":11,"start_frame":0,"last_frame":59,
            "positions":[[[60,35],frame] for frame in range(0,60,5)],
            "prediction":[60,35],"ball_sizes":[],
            "rally_classification":{"label":"rally","is_rally":True},
        }
        (track_folder/"track_0011.json").write_text(json.dumps(sample),encoding="utf8")
        grouped={
            "source_start_frame":0,"source_end_frame":59,
            "detected_start_frame":0,"detected_end_frame":59,
            "track_ids":[11],"confidence":.9,
        }
        payload=build_rally_payload(track_folder,grouped)
        self.assertIn("prediction",payload)
        self.assertEqual(payload["prediction"],[60,35])
        output_tracks=self.root/"selected-tracks"
        output_tracks.mkdir()
        (output_tracks/"track_0000.json").write_text(json.dumps(payload),encoding="utf8")
        rendered=self.root/"track_clip.mp4"
        root=Path(__file__).resolve().parents[1]
        subprocess.run([
            sys.executable,str(root/"src/track_processor.py"),
            "--video_path",str(self.video),"--json_dir",str(output_tracks),
            "--output_path",str(rendered),"--padding","0","--no-mark",
        ],cwd=root,check=True,timeout=35,stdout=subprocess.DEVNULL)
        self.assertTrue(rendered.is_file() and rendered.stat().st_size>0)
        self.assertTrue(self.ffprobe(rendered)["streams"])

    def test_montage_request_uses_existing_local_audio_path(self):
        fake_project={"id":"testproject","videos":["one.mp4"]}
        with patch("scripts.volleyball_ui.projects.get_project",
                   return_value=fake_project),\
             patch.object(data,"find_cache",return_value=self.root):
            config=montage_runner.validate_request({
                "project_id":"testproject","mode":"montage",
                "audio_path":str(self.audio),"audio_volume":"55",
            })
            self.assertEqual(config["audio_path"],str(self.audio))
            self.assertEqual(config["audio_volume"],55)
            self.assertEqual(config["follow_ball"],True)
            with self.assertRaises(ValueError):
                montage_runner.validate_request({
                    "project_id":"testproject","mode":"montage",
                    "audio_path":str(self.audio),"audio_volume":"150",
                })

    def test_runner_finishes_montage_with_soundtrack_in_both_framing_modes(self):
        from scripts.volleyball_ui import montage_project
        with tempfile.TemporaryDirectory() as scratch:
            base=Path(scratch)
            source_dir=base/"project"
            source_dir.mkdir()
            (source_dir/"one.mp4").write_bytes(self.video.read_bytes())
            result_root=base/"results"
            result_root.mkdir()
            cache=base/"cache"
            (cache/"tracks/analysis/tracks").mkdir(parents=True)
            for obj,key,value in (
                (data,"PROJECT",source_dir),
                (data,"RESULTS",result_root),
                (projects,"PROJECTS_DIR",result_root/"projects"),
                (projects,"REGISTER",result_root/"projects"/"projects.json"),
            ):
                patcher=patch.object(obj,key,value)
                patcher.start()
                self.addCleanup(patcher.stop)
            with patch("scripts.volleyball_ui.preview.video_info",
                       return_value={"duration_sec":2.0}):
                project=projects.create_project("Mix",["one.mp4"])
            source={"video":"one.mp4","cache":cache,
                    "video_path":self.video,"fps":30.0}
            template={
                "track_id":11,"track_ids":[11],
                "start_frame":0,"last_frame":59,
                "positions":[[[75,40],frame] for frame in range(0,60,4)],
                "prediction":[75,40],"ball_sizes":[],
                "rally_classification":{"label":"rally","is_rally":True},
                "tracking_scale":{"frame_width_px":160,"frame_height_px":90},
            }
            def plan(sources, selected_dir, seconds, **kwargs):
                selected_dir.mkdir(parents=True,exist_ok=True)
                folder=selected_dir/"000"
                folder.mkdir()
                (folder/"track_0000.json").write_text(
                    json.dumps(template),encoding="utf8")
                (selected_dir.parent/"selection.json").write_text(
                    json.dumps({"selected":[{"index":0,"video":"one.mp4"}]}),
                    encoding="utf8")
                return [{"index":0,"video":"one.mp4","track_id":11,
                         "track_dir":folder,"track_json":folder/"track_0000.json",
                         "start_frame":0,"end_frame":59,
                         "analysis_offset_sec":0.0,
                         "video_path":self.video,"source_duration":2.0}]
            for follow in (False,True):
                with self.subTest(follow_ball=follow),\
                     patch.object(data,"find_cache",return_value=cache),\
                     patch.object(montage_runner,"sources_for",return_value=[source]),\
                     patch.object(montage_runner,"plan_clips",side_effect=plan):
                    identifier="montage_with_tracking" if follow else "montage_no_tracking"
                    montage_runner.execute({
                        "project_id":project["id"],"mode":"montage",
                        "follow_ball":follow,"duration":2.5,
                        "audio_path":str(self.audio),"audio_volume":45,
                        "request_id":identifier,
                    })
                    name="highlights_reel.mp4" if follow else "highlights_full.mp4"
                    output=result_root/"projects"/project["id"]/"montages"/identifier/name
                    self.assertTrue(output.exists())
                    info=self.ffprobe(output)
                    types={entry["codec_type"] for entry in info["streams"]}
                    self.assertEqual(types,{"video","audio"})
                    self.assertLessEqual(float(info["format"]["duration"]),2.65)

    def test_montage_form_has_music_without_file_upload(self):
        root=Path(__file__).resolve().parents[1]
        html=(root/"scripts/volleyball_ui/static/index.html").read_text("utf8")
        app=(root/"scripts/volleyball_ui/static/app.js").read_text("utf8")
        self.assertIn('name="audio_path"',html)
        self.assertIn('name="audio_volume"',html)
        self.assertIn('id="montage-audio-choose"',html)
        self.assertIn('kind:"audio"',app)
        self.assertNotIn('id="project-existing-videos"',html)
        self.assertNotIn('project-existing-videos',app)


if __name__=="__main__":
    unittest.main()
