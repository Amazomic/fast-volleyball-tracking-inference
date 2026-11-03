"""Montage must retain original source pixels, FPS and chosen analysis timing."""
from __future__ import annotations
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import montage_source, montage_project, montage_runner, montage_encoding


class OriginalQualityTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory(prefix="montage-source-test-")
        self.addCleanup(tmp.cleanup)
        self.root=Path(tmp.name)
        self.source=self.root/"source.mp4"
        subprocess.run([
            "ffmpeg","-nostdin","-loglevel","error","-y","-f","lavfi",
            "-i","testsrc2=size=480x270:rate=60","-t","4",
            "-c:v","libx264","-preset","ultrafast","-crf","20",
            str(self.source)],check=True,timeout=25)

    def probe(self,path):
        response=subprocess.run([
            "ffprobe","-v","error","-select_streams","v:0",
            "-show_entries","stream=width,height,r_frame_rate,nb_frames,codec_name",
            "-of","json",str(path)],capture_output=True,text=True,
            timeout=20,check=True)
        return json.loads(response.stdout)["streams"][0]

    def test_original_clip_keeps_native_size_and_60_fps(self):
        output=self.root/"clip.mp4"
        meta=montage_source.clip_from_original(
            self.source,output,start_frame=30,end_frame=89,offset=.5)
        info=self.probe(output)
        self.assertEqual((info["width"],info["height"]),(480,270))
        self.assertEqual(info["r_frame_rate"],"60/1")
        self.assertEqual(int(info["nb_frames"]),120)
        self.assertEqual(meta["start_sec"],1.5)
        self.assertEqual(meta["fps"],60)
        self.assertEqual(meta["frames"],120)
        self.assertEqual(meta["source"],str(self.source.resolve()))

    def test_track_indices_remap_to_real_source_frames(self):
        path=self.root/"track_0000.json"
        path.write_text(json.dumps({
            "start_frame":30,"last_frame":89,"track_id":7,
            "positions":[[[10,20],30],[[14,25],45],[[20,40],89]],
            "tracking_scale":{"frame_width_px":480,"frame_height_px":270},
        }))
        result=montage_source.relative_track(
            path,self.root/"rebased",start_frame=30,
            native_fps=60,native_count=120)
        track=json.loads(result.read_text())
        self.assertEqual(track["start_frame"],0)
        self.assertEqual(track["last_frame"],119)
        self.assertEqual(track["fps"],60)
        self.assertEqual([row[1] for row in track["positions"]],[0,30,118])

    def test_single_clip_concat_keeps_source_resolution_and_fps(self):
        clip=self.root/"clip.mp4"
        montage_source.clip_from_original(
            self.source,clip,start_frame=0,end_frame=29,offset=0)
        final=self.root/"final.mp4"
        montage_project.compose_clips([clip],final,1.0,"tracks")
        self.assertEqual(self.probe(final)["width"],480)
        self.assertEqual(self.probe(final)["height"],270)
        self.assertEqual(self.probe(final)["r_frame_rate"],"60/1")

    def test_multiple_clips_stream_copy_when_uniform(self):
        cuts=[]
        for index in (0,1):
            clip=self.root/f"clip-{index}.mp4"
            montage_source.clip_from_original(
                self.source,clip,start_frame=0,end_frame=29,offset=float(index))
            cuts.append(clip)
        output=self.root/"joined.mp4"
        montage_project.compose_clips(cuts,output,2.0,"tracks")
        self.assertEqual(self.probe(output)["width"],480)
        self.assertEqual(self.probe(output)["r_frame_rate"],"60/1")
        self.assertAlmostEqual(float(self.probe(output)["nb_frames"]),120,delta=2)

    def test_original_source_is_used_even_when_analysis_proxy_exists(self):
        from scripts.volleyball_ui import data
        cache=self.root/"analysis-cache"
        (cache/"analysis").mkdir(parents=True)
        (cache/"analysis"/"events.json").write_text(
            json.dumps({"analysis_offset_sec":1.25}))
        (cache/"analysis.mp4").write_bytes(b"low resolution proxy")
        config={"project_id":"project","videos":["sample.mp4"],
                "cache_ids":{"sample.mp4":"selected-id"}}
        with patch("scripts.volleyball_ui.analysis_runs.cache_for",return_value=cache),\
             patch.object(data,"video_path",return_value=self.source):
            items=montage_runner.sources_for(config)
        self.assertEqual(items[0]["video_path"],self.source)
        self.assertEqual(items[0]["analysis_offset_sec"],1.25)
        self.assertEqual(items[0]["cache"],cache)

    def test_crop_command_generates_h264_directly_without_mp4v(self):
        from src.make_reels import crop_and_save_track_payload
        result=self.root/"reel.mp4"
        payload={"start_frame":0,"last_frame":59,
                 "positions":[[[230,135],i] for i in range(0,60,10)],
                 "tracking_scale":{"frame_width_px":480,"frame_height_px":270}}
        crop_and_save_track_payload(
            str(self.source),payload,str(result),
            padding=0,quality_crf=16,quality_encoder="cpu")
        info=self.probe(result)
        self.assertEqual(info["codec_name"],"h264")
        self.assertEqual(info["r_frame_rate"],"60/1")
        self.assertEqual(info["height"],270)
        self.assertLess(info["width"],480)

    def test_custom_export_also_uses_original_not_analysis_proxy(self):
        import sys
        root=Path(__file__).resolve().parents[1]
        sys.path.insert(0,str(root/"scripts"))
        try:
            from build_match_highlights import render
        finally:
            sys.path.remove(str(root/"scripts"))
        source_frames=[{"sequence":1,"source_start_frame":30,
                        "source_end_frame":89}]
        # The proxy path deliberately does not exist; custom export must not use it.
        output=self.root/"custom_full.mp4"
        with patch("build_match_highlights.RESULTS",self.root):
            render(self.source,self.root/"missing_low_resolution_proxy.mp4",
                   source_frames,output_path=output,
                   segments_folder=self.root/"fragments",
                   original_quality=True,analysis_offset=.5)
        info=self.probe(output)
        self.assertEqual((info["width"],info["height"]),(480,270))
        self.assertEqual(info["r_frame_rate"],"60/1")
        self.assertEqual(info["codec_name"],"h264")
        final=self.root/"custom_final.mp4"
        montage_runner._finalize_mp4(output,final,2.0)
        self.assertEqual(self.probe(final)["r_frame_rate"],"60/1")
        self.assertEqual(self.probe(final)["width"],480)

    def test_marked_track_renderer_uses_original_h264_quality(self):
        import sys
        folder=self.root/"marked_tracks"
        folder.mkdir()
        (folder/"track_0000.json").write_text(json.dumps({
            "track_id":4,"start_frame":0,"last_frame":59,
            "prediction":[],"ball_sizes":[],
            "positions":[[[120,80],i] for i in range(0,60,5)],
            "rally_classification":{"label":"rally","is_rally":True},
        }),encoding="utf8")
        video=self.root/"marked_quality.mp4"
        root=Path(__file__).resolve().parents[1]
        subprocess.run([
            sys.executable,str(root/"src/track_processor.py"),
            "--video_path",str(self.source),
            "--json_dir",str(folder),"--output_path",str(video),
            "--padding","0","--quality-crf","16",
            "--quality-encoder","cpu",
        ],cwd=root,check=True,stdout=subprocess.DEVNULL,timeout=30)
        info=self.probe(video)
        self.assertEqual(info["codec_name"],"h264")
        self.assertEqual(info["r_frame_rate"],"60/1")
        self.assertEqual((info["width"],info["height"]),(480,270))

    def test_encoder_flags_auto_respects_short_cpu_long_gpu_override(self):
        with patch.object(montage_encoding,"nvenc_available",return_value=True):
            self.assertIn("libx264",montage_encoding.encoder_flags(2,prefer="auto"))
            self.assertIn("h264_nvenc",montage_encoding.encoder_flags(10,prefer="auto"))
        self.assertIn("libx264",montage_encoding.encoder_flags(10,prefer="cpu"))


if __name__ == "__main__":
    unittest.main()
