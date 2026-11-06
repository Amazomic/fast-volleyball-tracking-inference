"""Finished Quick Reel MP4 history survives each fresh analysis launch."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import quick_reel_jobs as jobs


class QuickReelHistoryTests(unittest.TestCase):
    def setUp(self):
        tmp=tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root=Path(tmp.name)/"quick_reels"
        patch_root=patch.object(jobs,"QUICK_ROOT",self.root)
        patch_root.start()
        self.addCleanup(patch_root.stop)
        self.audio=Path(tmp.name)/"song.mp3"
        self.audio.write_bytes(b"soundtrack")
        self.ids=["a"*32,"b"*32,"c"*32]

    def fake_run(self,identifier,ready):
        folder=jobs.quick_dir(identifier)
        folder.mkdir(parents=True,exist_ok=True)
        (folder/"request.json").write_text(json.dumps({
            "id":identifier,"created_at":100+len(list(self.root.iterdir())),
            "videos":[{"video":"match.mp4","selection_id":"d"*32}]}),encoding="utf8")
        (folder/"analysis_cache").mkdir()
        (folder/"analysis_cache"/"raw-vectors.bin").write_bytes(b"scratch")
        (folder/"status.json").write_text(json.dumps({
            "id":identifier,"status":"complete" if ready else "queued"
        }),encoding="utf8")
        if ready:
            path=folder/"montage"/"highlights_reel.mp4"
            path.parent.mkdir()
            path.write_bytes(b"mp4 content "+identifier.encode())
            (folder/"result.json").write_text(json.dumps({
                "id":identifier,"video":"montage/highlights_reel.mp4",
                "duration":22.5,"music_duration":30,
                "videos":["match.mp4"]}),encoding="utf8")
        return folder

    def test_finished_exports_survive_next_run_without_scratch(self):
        first,second,third=self.ids
        old=self.fake_run(first,True)
        latest=self.fake_run(second,False)
        jobs.publish_latest(second,str(self.audio))
        self.assertEqual(jobs.latest_reel_id(),second)
        self.assertFalse(old.exists())
        self.assertTrue(latest.exists())
        archived=jobs.result_for(first)
        self.assertIsNotNone(archived)
        self.assertIn(b"mp4 content",archived[1].read_bytes())
        self.assertFalse((self.root/"completed"/"analysis_cache").exists())
        self.assertEqual([entry["id"] for entry in jobs.completed_history()],[first])
        self.assertTrue(jobs.completed_history()[0]["download_url"].endswith(first))
        self.assertEqual(
            sorted(p.suffix for p in (self.root/"completed").iterdir()),
            [".json", ".mp4"],
            "The completed archive is a single folder with only MP4+metadata")
        # Second reel is finished. On the third run both completed exports are available.
        path=latest/"montage"/"highlights_reel.mp4"
        path.parent.mkdir()
        path.write_bytes(b"second mp4")
        (latest/"result.json").write_text(json.dumps({
            "id":second,"video":"montage/highlights_reel.mp4",
            "duration":32,"videos":["training.mp4"]}),encoding="utf8")
        self.fake_run(third,False)
        jobs.publish_latest(third,str(self.audio))
        self.assertFalse(latest.exists())
        history={item["id"]:item for item in jobs.completed_history()}
        self.assertEqual(set(history),{first,second})
        self.assertEqual(jobs.result_for(second)[1].read_bytes(),b"second mp4")
        self.assertTrue(jobs.result_for(first)[1].exists())
        self.assertFalse((self.root/"completed"/second/"analysis_cache").exists())
        self.assertEqual(len(list((self.root/"completed").glob("*.mp4"))),2)

    def test_delete_only_one_completed_reel_and_never_original(self):
        a,b,_=self.ids
        self.fake_run(a,True)
        self.fake_run(b,True)
        self.assertEqual(len(jobs.completed_history()),2)
        original=self.audio.read_bytes()
        deleted=jobs.delete_completed(a)
        self.assertEqual(deleted,{"deleted":a})
        self.assertIsNone(jobs.result_for(a))
        self.assertIsNotNone(jobs.result_for(b))
        self.assertEqual({item["id"] for item in jobs.completed_history()},{b})
        self.assertEqual(self.audio.read_bytes(),original)
        self.assertFalse((self.root/"completed"/(a+".mp4")).exists())
        self.assertFalse((self.root/"completed"/(a+".json")).exists())
        with self.assertRaisesRegex(ValueError,"не найден"):
            jobs.delete_completed(a)

    def test_cannot_delete_running_reel_even_if_export_exists(self):
        a,_,_=self.ids
        folder=self.fake_run(a,True)
        (folder/"status.json").write_text(json.dumps({"status":"running"}),encoding="utf8")
        (self.root/"latest.json").write_text(json.dumps({"id":a}),encoding="utf8")
        jobs.archive_completed(a)
        with self.assertRaisesRegex(ValueError,"до завершения"):
            jobs.delete_completed(a)
        self.assertIsNotNone(jobs.archived_result_for(a))

    def test_incomplete_reel_has_no_result_link(self):
        a,b,_=self.ids
        self.fake_run(a,False)
        self.fake_run(b,False)
        jobs.publish_latest(b,str(self.audio))
        self.assertEqual(jobs.completed_history(),[])
        self.assertIsNone(jobs.result_for(a))

    def test_active_completed_reel_is_also_listed(self):
        a,_,_=self.ids
        self.fake_run(a,True)
        history=jobs.completed_history()
        self.assertEqual(len(history),1)
        self.assertTrue(history[0]["url"].endswith(a))

    def test_untrusted_or_missing_ids_do_not_expose_arbitrary_paths(self):
        with self.assertRaises(ValueError):
            jobs.result_for("../../secret")
        with self.assertRaises(ValueError):
            jobs.result_for("invalid")
        with self.assertRaises(ValueError):
            jobs.delete_completed("../../secret")
        self.assertEqual(jobs.completed_history(),[])


if __name__=="__main__":
    unittest.main()
