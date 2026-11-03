"""Montage library and three modes preserve existing video exports."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from scripts.volleyball_ui import data, projects, montage_library, montage_runner, montage_project


class MontageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.sources = self.root / "project"
        self.sources.mkdir()
        (self.sources / "match.mp4").write_bytes(b"video")
        self.results = self.root / "results"
        self.results.mkdir()
        for p in [
            patch.object(data, "PROJECT", self.sources),
            patch.object(data, "RESULTS", self.results),
            patch.object(projects, "PROJECTS_DIR", self.results / "projects"),
            patch.object(projects, "REGISTER", self.results / "projects" / "projects.json"),
            patch.object(montage_library, "RESULTS", self.results),
            patch("scripts.volleyball_ui.preview.video_info", return_value={"duration_sec": 1000}),
        ]:
            p.start()
            self.addCleanup(p.stop)
        self.project = projects.create_project("Матч", ["match.mp4"])
        self.pid = self.project["id"]

    def test_library_lists_all_outputs_and_can_delete_without_touching_sources(self):
        root = projects.project_dir(self.pid)
        (root / "highlights.mp4").write_bytes(b"highlight")
        reel = root / "vertical_reels" / "match" / "reel_test.mp4"
        reel.parent.mkdir(parents=True)
        reel.write_bytes(b"reel")
        analysis = root / "analysis" / "cache" / "analysis.mp4"
        analysis.parent.mkdir(parents=True)
        analysis.write_bytes(b"internal")
        (self.results / "highlights_old.mp4").write_bytes(b"old")
        with patch.object(montage_library, "codec_for", return_value="h264"):
            files = montage_library.listing(self.pid)
        self.assertEqual(len(files), 3)
        self.assertEqual({row["name"] for row in files},
                         {"highlights.mp4", "reel_test.mp4", "highlights_old.mp4"})
        self.assertTrue(all(row["url"].startswith("/api/montage/media") for row in files))
        self.assertTrue(all(row["download_url"].startswith("/api/montage/download") for row in files))
        montage_library.remove(self.pid, "project:vertical_reels/match/reel_test.mp4")
        self.assertFalse(reel.exists())
        self.assertTrue(analysis.exists())
        self.assertTrue((self.sources / "match.mp4").exists())
        with self.assertRaises(FileNotFoundError):
            montage_library.output_path(self.pid, "project:../../project/match.mp4")

    def test_server_launches_validated_montage_without_shell(self):
        from scripts.volleyball_ui import server
        cache = self.root / "cache"
        cache.mkdir()
        run = {"id": "run_valid", "profile": "Точность",
               "videos": {"match.mp4": "a" * 16}}
        with patch("scripts.volleyball_ui.analysis_runs.list_runs", return_value=[run]), \
             patch("scripts.volleyball_ui.analysis_runs.cache_for", return_value=cache), \
             patch("scripts.volleyball_ui.prepared_videos.status", return_value={"videos": [{"video": "match.mp4", "state": "ready"}]}), \
             patch.object(server.subprocess, "Popen") as spawn:
            process = spawn.return_value
            process.poll.return_value = 0
            process.returncode = 0
            for mode in ("tracks", "vertical", "custom"):
                result = server.start_job("highlights", {
                    "project_id": self.pid, "mode": mode, "duration": 45,
                    "run_id": run["id"],
                })
                argv = spawn.call_args.args[0]
                self.assertIn("scripts.volleyball_ui.montage_runner", argv)
                self.assertFalse(spawn.call_args.kwargs.get("shell", False))
                config = json.loads(Path(argv[argv.index("--config") + 1]).read_text("utf-8"))
                self.assertEqual(config["mode"], mode)
                self.assertEqual(config["duration"], 45)
                self.assertEqual(config["run_id"], run["id"])
                self.assertEqual(config["cache_ids"], run["videos"])
                self.assertEqual(config["videos"], ["match.mp4"])
                self.assertNotIn("video", config)
                self.assertTrue(config["request_id"])
                self.assertEqual(result["status"], "complete")
        server.JOB.update(process=None, id=None, kind=None, log=None)

    def test_modes_and_track_subset_duration(self):
        cache = self.root / "cache"
        cache.mkdir()
        tracks = cache / "tracks" / "analysis" / "tracks"
        tracks.mkdir(parents=True)
        for idx, (length, is_rally) in enumerate([(100, True), (80, False), (90, True)]):
            (tracks / f"track_{idx:04d}.json").write_text(json.dumps({
                "start_frame": idx * 170, "last_frame": idx * 170 + length - 1,
                "track_id": idx,
                "positions": [[[20,30], idx * 170 + length // 2]],
                "rally_classification": {"label": "rally" if is_rally else "not_rally",
                                         "rally_confidence": .9}}))
        game=[json.loads(p.read_text(encoding="utf-8"))
              for p in sorted(tracks.glob("track_*.json"))]
        (cache/"analysis").mkdir()
        (cache/"analysis/game-tracks.json").write_text(json.dumps(game))
        (cache/"ranking.json").write_text(json.dumps([
            {"start":p["start_frame"],"end":p["last_frame"],"score":12.0,
             "confidence":.9} for p in game
            if p["rally_classification"]["label"]=="rally"]))
        with patch.object(data, "find_cache", return_value=cache):
            for mode in ("tracks", "vertical", "custom"):
                config = montage_runner.validate_request({"project_id": self.pid, "mode": mode,
                                                          "duration": 12})
                self.assertEqual(config["mode"], mode)
                self.assertEqual(config["duration"], 12)
            with self.assertRaisesRegex(ValueError, "режим"):
                montage_runner.validate_request({"project_id": self.pid,
                                                 "mode": "shell", "duration": 12})
        clips=montage_project.plan_clips(
            [{"video":"match.mp4","cache":cache,
              "video_path":self.sources/"match.mp4"}],
            self.root/"chosen",12,mode="tracks",padding=.3)
        self.assertEqual(len(clips),2)
        self.assertTrue(6 < sum(c["source_duration"] for c in clips) < 9)
        self.assertEqual(len(list((self.root/"chosen").glob("*/track_*.json"))),2)


if __name__ == "__main__":
    unittest.main()
