"""CPU reports aggregate known player IDs while preserving raw tracks."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_identity_groups


class CpuPlayerReportTests(unittest.TestCase):
    def test_manually_merged_ids_share_player_stats_and_candidate(self):
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp) / "postprocessing" / "cpu-id"
            (cache / "analysis").mkdir(parents=True)
            (cache / "analysis" / "events.json").write_text(
                json.dumps({"events": []}), encoding="utf8")
            (cache / "players.replay.json").write_text("{}", encoding="utf8")
            source_stats = [
                {"player_track_id": "5", "attack": 2, "dig": 0, "total_actions": 2},
                {"player_track_id": "6", "attack": 0, "dig": 1, "total_actions": 1},
            ]
            tracks = {"fps": 30, "tracks": {
                "5": {"detected_frames": 80, "duration_sec": 3.0},
                "6": {"detected_frames": 80, "duration_sec": 3.0},
            }}
            effective = {"candidates": {"5": "player-a", "6": "player-a"},
                         "players": {
                             "5": {"id": "a", "name": "Анна", "source": "assigned"},
                             "6": {"id": "a", "name": "Анна",
                                   "source": "manual_merge"}}}
            named = [{"id": "a", "name": "Анна", "track_ids": ["5"]}]
            with patch.object(data, "video_path"), \
                 patch("scripts.volleyball_ui.analysis_runs.cache_for",
                       return_value=cache), \
                 patch("scripts.volleyball_ui.player_tracks.tracks_for",
                       return_value=tracks), \
                 patch.object(data, "_raw_stats", return_value=source_stats), \
                 patch.object(data, "groups_for", return_value=named), \
                 patch.object(data, "excluded_track_ids",
                              return_value=set()) as exclusions, \
                 patch.object(player_identity_groups, "effective_track_identities",
                              return_value=effective):
                result = data.report("video.mp4", "project", "gpu-id.cpu-id")
                exclusions.return_value = {"6"}
                filtered = data.report("video.mp4", "project", "gpu-id.cpu-id")
            self.assertEqual(filtered["track_count"], 1)
            self.assertEqual([p["player_track_id"] for p in filtered["raw"]], ["5"])
            self.assertEqual(filtered["groups"][0]["track_ids"], ["5"])
            self.assertEqual(filtered["groups"][0]["total_actions"], 2)
            self.assertEqual(result["groups"][0]["track_ids"], ["5", "6"])
            self.assertEqual(result["groups"][0]["total_actions"], 3)
            self.assertEqual(result["unassigned_count"], 0)
            self.assertEqual(result["candidate_groups_count"], 1)
            self.assertEqual(result["track_count"], 2)
            self.assertEqual(result["raw"][0]["player_name"], "Анна")


if __name__ == "__main__":
    unittest.main()
