"""Normalized video ranges must survive changes to models or profile settings."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.volleyball_studio.video_inputs import prepare_proxy, cached_source_identity


class VideoInputReuseTests(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.folder=Path(temp.name)
        self.original=self.folder/"original.mp4"
        self.original.write_bytes(b"original file")
        self.prepared=self.folder/"prepared"/"video_30fps.mp4"
        self.prepared.parent.mkdir()
        self.prepared.write_bytes(b"already prepared")

    def test_original_source_sha_is_cached_between_profiles(self):
        result=[str(self.original.resolve()),self.original.stat().st_size,
                self.original.stat().st_mtime_ns,"a"*64]
        with patch("scripts.volleyball_studio.video_inputs.identity",return_value=result) as hash_file:
            first=cached_source_identity(self.original,self.prepared)
            second=cached_source_identity(self.original,self.prepared)
        self.assertEqual(first,second)
        hash_file.assert_called_once()

    def test_full_video_never_reencodes_and_reuses_source(self):
        first=self.folder/"analysis"/"run1"/"analysis.mp4"
        second=self.folder/"analysis"/"run2"/"analysis.mp4"
        with (patch("scripts.volleyball_studio.video_inputs.run") as ffmpeg,
              patch("scripts.volleyball_studio.common.probe",return_value={
                  "streams":[{"codec_type":"video","width":1280}]})):
            for output in (first, second):
                result=prepare_proxy(source=self.original,prepared=self.prepared,
                     proxy=output,start=0,seconds=None,width=1280,threads=8)
                self.assertEqual(result,"reused")
                self.assertEqual(output.read_bytes(),self.prepared.read_bytes())
            ffmpeg.assert_not_called()

    def test_same_clip_is_encoded_only_once_across_distinct_profiles(self):
        calls=[]
        def fake_encode(args, log):
            calls.append(args)
            Path(args[-1]).write_bytes(b"normalized range")
        a=self.folder/"cache"/"profile_a"/"analysis.mp4"
        b=self.folder/"cache"/"profile_b"/"analysis.mp4"
        with (patch("scripts.volleyball_studio.video_inputs.run",side_effect=fake_encode),
              patch("scripts.volleyball_studio.common.probe",return_value={
                  "streams":[{"codec_type":"video","width":1280}]})):
            out1=prepare_proxy(source=self.original,prepared=self.prepared,
                 proxy=a,start=13.5,seconds=77.2,width=1280,threads=8)
            out2=prepare_proxy(source=self.original,prepared=self.prepared,
                 proxy=b,start=13.5,seconds=77.2,width=1280,threads=16)
            out3=prepare_proxy(source=self.original,prepared=self.prepared,
                 proxy=a,start=13.5,seconds=77.2,width=1280,threads=4)
        self.assertEqual([out1,out2,out3],["created","reused","reused"])
        self.assertEqual(len(calls),1)
        self.assertEqual(a.read_bytes(),b.read_bytes())
        self.assertEqual(len(list((self.prepared.parent/"segments").glob("*.mp4"))),1)

    def test_different_profile_width_is_derived_from_prepared_source(self):
        commands=[]
        def fake_encode(args, log):
            commands.append(args)
            Path(args[-1]).write_bytes(b"transcoded from normalized cache")
        with (patch("scripts.volleyball_studio.video_inputs.run",side_effect=fake_encode),
              patch("scripts.volleyball_studio.common.probe",return_value={
                  "streams":[{"codec_type":"video","width":640}]})):
            output=self.folder/"analysis"/"different_width"/"analysis.mp4"
            result=prepare_proxy(source=self.original,prepared=self.prepared,
                                 proxy=output,start=0,seconds=None,
                                 width=1280,threads=4)
        self.assertEqual(result,"created")
        self.assertEqual(len(commands),1)
        args=commands[0]
        self.assertEqual(args[args.index("-i")+1],str(self.prepared.resolve()))
        self.assertNotIn(str(self.original),[str(value) for value in args])
        self.assertIn("scale=1280:-2",args[args.index("-vf")+1])

    def test_standalone_raw_video_also_reuses_same_segment(self):
        calls=[]
        def fake_encode(args, log):
            calls.append(args)
            Path(args[-1]).write_bytes(b"original encoded once")
        with (patch("scripts.volleyball_studio.video_inputs.run",side_effect=fake_encode),
              patch("scripts.volleyball_studio.common.probe",return_value={
                  "streams":[{"codec_type":"video","width":1280}]})):
            for name in ("profile_a", "profile_b"):
                result=prepare_proxy(source=self.original,prepared=None,
                     proxy=self.folder/"output"/name/"analysis.mp4",
                     start=12,seconds=20,width=640,threads=4)
                self.assertEqual(result,"created" if name=="profile_a" else "reused")
        self.assertEqual(len(calls),1)

    def test_changed_clip_or_video_source_invalidates_cache(self):
        calls=[]
        def fake_encode(args, log):
            calls.append(args)
            Path(args[-1]).write_bytes(("encoding"+str(len(calls))).encode())
        with (patch("scripts.volleyball_studio.video_inputs.run",side_effect=fake_encode),
              patch("scripts.volleyball_studio.common.probe",return_value={
                  "streams":[{"codec_type":"video","width":1280}]})):
            for i, seconds in enumerate((77.2,77.2,70.0)):
                prepare_proxy(source=self.original,prepared=self.prepared,
                     proxy=self.folder/"cache"/f"r{i}"/"analysis.mp4",
                     start=13.5,seconds=seconds,width=1280,threads=8)
            self.prepared.write_bytes(b"changed prepared content")
            prepare_proxy(source=self.original,prepared=self.prepared,
                 proxy=self.folder/"cache"/"r3"/"analysis.mp4",
                 start=13.5,seconds=77.2,width=1280,threads=8)
        self.assertEqual(len(calls),3)


if __name__=="__main__":
    unittest.main()
