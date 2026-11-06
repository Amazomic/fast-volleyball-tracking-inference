"""Regression: CPU-filtered player counts follow the selected result branch."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_tracks, rally_players


def write_players(path, ids):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({
        "fps": 30, "source_size": {"width": 1000, "height": 600},
        "predictions": [
            {"class_name": "player", "track_id": tid, "score": .9,
             "frame_index": 0, "bbox_xyxy": [100 + tid*100, 80, 200 + tid*100, 300]}
            for tid in ids
        ],
    }), encoding="utf-8")


class CPUPlayerTrackCounts(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.base = Path(temp.name) / ("a" * 24)
        self.replay = self.base / "postprocessing" / ("b" * 24)
        write_players(self.base / "players.json", [1, 2])
        write_players(self.replay / "players.replay.json", [1])
        for cache in (self.base, self.replay):
            path = cache / "analysis/events.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps({"source": "video.mp4", "events": []}), encoding="utf-8")
        self.base_id = self.base.name
        self.replay_id = self.base_id + "." + self.replay.name

        def resolve(video, project, cache_id):
            return self.replay if cache_id == self.replay_id else self.base

        patches = [
            patch("scripts.volleyball_ui.analysis_runs.cache_for", side_effect=resolve),
            patch.object(player_tracks, "assert_project_video"),
            patch.object(data, "video_path"),
            patch.object(data, "_raw_stats", return_value=[
                {"player_track_id": "1", "total_actions": 0},
                {"player_track_id": "2", "total_actions": 0},
            ]),
            patch.object(data, "groups_for", return_value=[]),
        ]
        for item in patches:
            item.start()
            self.addCleanup(item.stop)
        player_tracks._compute.cache_clear()

    def test_active_ids_follow_replay_but_identity_ids_remain_stable(self):
        original = player_tracks.tracks_for("video.mp4", "project", self.replay_id)
        active = player_tracks.tracks_for(
            "video.mp4", "project", self.replay_id, selected_version=True)
        self.assertEqual(set(original["tracks"]), {"1", "2"})
        self.assertEqual(set(active["tracks"]), {"1"})

    def test_player_report_excludes_filtered_out_ids(self):
        report = data.report("video.mp4", "project", self.replay_id)
        self.assertEqual(report["source_track_count"], 2)
        self.assertEqual(report["track_count"], 1)
        self.assertEqual([x["player_track_id"] for x in report["raw"]], ["1"])
        self.assertTrue(report["replay_filtered_tracks"])

    def test_original_report_unaffected(self):
        report = data.report("video.mp4", "project", self.base_id)
        self.assertEqual(report["track_count"], 2)
        self.assertEqual(report["source_track_count"], 2)
        self.assertFalse(report["replay_filtered_tracks"])

    def test_rally_counts_use_cpu_replay_when_present(self):
        with patch.object(rally_players, "_index", return_value={
            "available": True, "frames": {"0": ["1"], "15": ["1"]}}) as index:
            result = rally_players.player_info(self.replay, [(0, 30)])
        self.assertEqual(result[0]["player_count"], 1)
        self.assertEqual(Path(index.call_args.args[0]), self.replay / "players.replay.json")


if __name__ == "__main__":
    unittest.main()
