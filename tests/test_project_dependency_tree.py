"""Project -> immutable analysis run -> montage; shared prepared source cache."""
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import (
    analysis_profiles, analysis_runs, data, montage_library, montage_runner,
    prepared_videos, projects
)


class DependencyTreeTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        self.original = root / "original" / "match.mp4"
        self.original.parent.mkdir()
        self.original.write_bytes(b"original file")
        self.videos = root / "project"
        self.videos.mkdir()
        for alias in ("match.mp4", "match (2).mp4"):
            (self.videos / alias).symlink_to(self.original)
        self.results = root / "results"
        self.results.mkdir()
        for obj, name, path in (
            (data, "PROJECT", self.videos),
            (data, "RESULTS", self.results),
            (projects, "PROJECTS_DIR", self.results / "projects"),
            (projects, "REGISTER", self.results / "projects" / "projects.json"),
        ):
            stub = patch.object(obj, name, path)
            stub.start()
            self.addCleanup(stub.stop)
        for location in ("scripts.volleyball_ui.preview.video_info",
                         "scripts.volleyball_ui.prepared_videos.video_info"):
            stub = patch(location, return_value={
                "duration_sec": 8, "width": 320, "height": 180, "auto_width": 320})
            stub.start()
            self.addCleanup(stub.stop)
        # Fixtures intentionally contain synthetic MP4 bytes, not a decodable
        # stream. Do not invoke the real ffprobe readiness check in this test.
        ready_check = patch("scripts.volleyball_ui.prepared_videos.is_analysis_ready",
                            return_value=False)
        ready_check.start()
        self.addCleanup(ready_check.stop)
        self.p1 = projects.create_project("First", ["match.mp4"])["id"]
        self.p2 = projects.create_project("Second", ["match (2).mp4"])["id"]

    def test_prepared_file_reused_between_projects_even_with_other_alias(self):
        _, _, output1, meta1, identity1 = prepared_videos._entry(self.p1, "match.mp4")
        _, _, output2, meta2, identity2 = prepared_videos._entry(self.p2, "match (2).mp4")
        self.assertEqual(output1, output2)
        self.assertEqual(meta1, meta2)
        self.assertEqual(identity1, identity2)
        self.assertEqual(output1.parent, self.results / "prepared")
        output1.parent.mkdir(parents=True)
        output1.write_bytes(b"prepared video")
        prepared_videos._save(meta1, {"identity": identity1, "status": "ready",
                                     "progress": 100})
        self.assertEqual(prepared_videos.ready_path(self.p2, "match (2).mp4"), output1)
        with patch.object(prepared_videos._POOL, "submit") as submit:
            prepared_videos.queue_project(self.p2)
            submit.assert_not_called()
        projects.archive_project(self.p1)
        self.assertTrue(output1.is_file())
        self.assertEqual(prepared_videos.ready_path(self.p2, "match (2).mp4"), output1)
        self.original.write_bytes(b"changed")
        self.assertIsNone(prepared_videos.ready_path(self.p2, "match (2).mp4"))

    def test_legacy_project_proxy_promoted_without_duplicate_bytes(self):
        name = "match.mp4"
        old_dir = projects.project_dir(self.p1) / "prepared"
        old_dir.mkdir(parents=True)
        old_stem = hashlib.sha256(name.encode()).hexdigest()[:18] + "_w320_v1"
        old_file = old_dir / (old_stem + ".mp4")
        old_file.write_bytes(b"legacy prepared")
        stat = self.original.stat()
        legacy = {"name": name, "size": stat.st_size, "mtime_ns": stat.st_mtime_ns,
                  "width": 320, "fps": 30, "schema": 1}
        prepared_videos._save(old_dir / (old_stem + ".json"),
                              {"identity": legacy, "status": "ready", "progress": 100})
        shared = prepared_videos.ready_path(self.p2, "match (2).mp4")
        self.assertIsNotNone(shared)
        self.assertEqual(shared.parent, self.results / "prepared")
        self.assertEqual(shared.stat().st_ino, old_file.stat().st_ino)
        projects.archive_project(self.p1)
        self.assertEqual(shared.read_bytes(), b"legacy prepared")
        self.assertEqual(prepared_videos.ready_path(self.p2, "match (2).mp4"), shared)

    def test_global_profile_is_shared_but_analysis_stays_in_each_project(self):
        profile = analysis_profiles.save_profile("Общий профиль", {
            "ball_threshold": 0.55, "player_frame_step": 1,
        })
        self.assertEqual(analysis_profiles.all_profiles(), [profile])
        self.assertEqual(
            analysis_profiles.resolve_profile_reference(profile["id"], profile["name"]),
            (profile["id"], profile["name"]))
        self.assertEqual(
            analysis_profiles.resolve_profile_reference("accuracy", "Точность"),
            ("accuracy", "Точность"))
        with self.assertRaises(ValueError):
            analysis_profiles.resolve_profile_reference(profile["id"], "Другое название")

        for project_id, video in ((self.p1, "match.mp4"),
                                  (self.p2, "match (2).mp4")):
            cache = projects.project_cache_root(project_id) / ("a" * 16)
            (cache / "analysis").mkdir(parents=True)
            (cache / "analysis" / "events.json").write_text(json.dumps({
                "source": str(self.original), "events": [],
            }), encoding="utf-8")
            timings = projects.project_run_root(project_id) / "analysis_timings"
            timings.mkdir(parents=True)
            (timings / "20261102_211000.json").write_text(json.dumps({
                "status": "complete", "profile_id": profile["id"],
                "profile_name": profile["name"],
                "started_at": "2026-11-02T21:10:00+00:00",
                "settings": {"backend": "onnx", "ball_threshold": 0.55},
                "videos": [{"status": "complete", "video": video,
                            "cache_id": cache.name}],
            }, ensure_ascii=False), encoding="utf-8")
            runs = analysis_runs.list_runs(project_id)
            self.assertEqual(len(runs), 1)
            self.assertEqual(runs[0]["profile_id"], profile["id"])
            manifest = data.read_json(
                projects.project_run_root(project_id) / "branches" /
                runs[0]["id"] / "analysis.json")
            self.assertEqual(manifest["profile_id"], profile["id"])
            self.assertEqual(manifest["settings"]["ball_threshold"], 0.55)
            self.assertEqual(manifest["project_id"], project_id)

        analysis_profiles.remove_profile(profile["id"])
        self.assertEqual(analysis_profiles.all_profiles(), [])
        self.assertEqual(len(analysis_runs.list_runs(self.p1)), 1)
        self.assertEqual(len(analysis_runs.list_runs(self.p2)), 1)

    def test_each_successful_run_has_distinct_branch_and_montage_binding(self):
        history = projects.project_run_root(self.p1) / "analysis_timings"
        history.mkdir(parents=True)
        ids = []
        for n in range(2):
            cache = projects.project_cache_root(self.p1) / (str(n + 1) * 16)
            (cache / "analysis").mkdir(parents=True)
            (cache / "analysis" / "events.json").write_text(json.dumps({
                "source": str(self.original),
                "events": [{"type": "attack", "frame": n * 8}]}), encoding="utf-8")
            (cache / "analysis" / "game-tracks.json").write_text("[]", encoding="utf-8")
            name = f"20261102_12000{n}_000001"
            (history / (name + ".json")).write_text(json.dumps({
                "status": "complete", "profile_name": "Одинаковый профиль",
                "started_at": f"2026-11-02T12:00:0{n}+00:00",
                "videos": [{"status": "complete", "video": "match.mp4",
                            "cache_id": cache.name}]}), encoding="utf-8")
            ids.append("run_" + name)
        runs = analysis_runs.list_runs(self.p1)
        self.assertEqual(len(runs), 2)
        self.assertEqual([run["id"] for run in runs], ids[::-1])
        self.assertEqual([run["profile"] for run in runs],
                         ["Одинаковый профиль"] * 2)
        for run in runs:
            branch = projects.project_run_root(self.p1) / "branches" / run["id"]
            manifest = data.read_json(branch / "analysis.json")
            self.assertEqual(manifest["videos"], run["videos"])
            self.assertEqual(manifest["project_id"], self.p1)
            config = montage_runner.validate_request({
                "project_id": self.p1, "mode": "montage", "run_id": run["id"],
                "duration": 20})
            sources = montage_runner.sources_for(config)
            self.assertEqual(sources[0]["cache"].name, run["videos"]["match.mp4"])
            clip = branch / "montages" / "20261102_123000_abcd1234" / "best_moments.mp4"
            clip.parent.mkdir(parents=True)
            clip.write_bytes(b"mp4 example")
            prepared_videos._save(clip.parent / "selection.json", {
                "analysis_run_id": run["id"], "analysis_profile": run["profile"]})
        items = [item for item in montage_library.listing(self.p1)
                 if item["name"] == "best_moments.mp4"]
        self.assertEqual({item["analysis_run_id"] for item in items}, set(ids))
        for item in items:
            self.assertEqual(montage_library.output_path(self.p1, item["id"]).name,
                             "best_moments.mp4")
        # Prove the actual renderer, not just the library, chooses the branch.
        selected = runs[0]
        (projects.project_cache_root(self.p1) /
         selected["videos"]["match.mp4"] / "tracks/analysis/tracks").mkdir(parents=True)
        with patch.object(montage_runner, "plan_clips",
                          side_effect=RuntimeError("render interrupted for test")):
            with self.assertRaisesRegex(RuntimeError, "render interrupted"):
                montage_runner.execute({
                    "project_id": self.p1, "mode": "montage", "duration": 20,
                    "run_id": selected["id"], "request_id": "unit_test",
                })
        self.assertTrue((projects.project_run_root(self.p1) / "branches" /
                         selected["id"] / "montages" / "unit_test").is_dir())
        self.assertFalse((projects.project_dir(self.p1) / "montages" / "unit_test").exists())
        with self.assertRaises(ValueError):
            montage_runner.validate_request({
                "project_id": self.p2, "mode": "montage", "run_id": ids[0],
                "duration": 20})


if __name__ == "__main__":
    unittest.main()
