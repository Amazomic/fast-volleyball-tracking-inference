"""Rejected Track IDs remain per-project and affect CPU overlays and identities."""
from __future__ import annotations
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_identity_groups
from scripts.volleyball_ui.player_overlay import player_boxes_window


class PlayerExclusionsTests(unittest.TestCase):
    def test_exclude_is_project_scoped_and_removes_manual_link(self):
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp) / "player_identities.json"
            record = {"schema": 2, "videos": {"match.mp4::abc": [{
                "id": "a", "name": "Anna", "track_ids": ["5", "6"],
                "link_meta": {"5": {"source": "manual"}, "6": {"source": "manual"}}
            }]}}
            with patch("scripts.volleyball_ui.projects.assert_project_video"), \
                 patch("scripts.volleyball_ui.analysis_runs.cache_for"), \
                 patch("scripts.volleyball_ui.player_tracks.tracks_for",
                       return_value={"tracks": {"5": {}, "6": {}}}), \
                 patch.object(data, "_identities", return_value=record), \
                 patch.object(data, "identity_file", return_value=target):
                deleted = data.exclude_tracks(
                    "match.mp4", ["6"], "project", "abc.cpu")
                self.assertEqual(deleted, ["6"])
                self.assertEqual(data.excluded_track_ids(
                    "match.mp4", "project", "abc.other"), {"6"})
                self.assertEqual(data.excluded_track_ids(
                    "match.mp4", "project", "other"), set())
            stored = json.loads(target.read_text(encoding="utf-8"))
            self.assertEqual(stored["videos"]["match.mp4::abc"][0]["track_ids"], ["5"])
            self.assertNotIn("6", stored["videos"]["match.mp4::abc"][0]["link_meta"])
            self.assertEqual(stored["auto_rejected"]["match.mp4::abc"], ["6"])

    def test_manual_merges_skip_excluded_tracks(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with patch.object(player_identity_groups, "_context",
                              return_value=(root, root, root)), \
                 patch.object(player_identity_groups, "_read", return_value={
                     "groups": [{"id": "manual-1", "source": "manual",
                                 "track_ids": ["5", "6", "7"]}]}), \
                 patch.object(data, "groups_for", return_value=[
                     {"id": "a", "name": "Anna", "track_ids": ["5"]}]), \
                 patch.object(data, "excluded_track_ids", return_value={"6"}):
                effective = player_identity_groups.effective_track_identities(
                    "project", "match.mp4", "abc.cpu")
            self.assertEqual(effective["players"]["7"]["name"], "Anna")
            self.assertNotIn("6", effective["players"])
            self.assertNotIn("6", effective["candidates"])

    def test_cpu_player_boxes_hide_deleted_ids(self):
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp)
            payload = {"fps": 30, "source_size": {"width": 100, "height": 200},
                       "predictions": [
                           {"class_name": "player", "frame_index": 30, "track_id": i,
                            "score": .9, "bbox_xyxy": [10, 20, 60, 160]}
                           for i in (5, 6)]}
            (cache / "players.json").write_text(
                json.dumps(payload), encoding="utf-8")
            with patch("scripts.volleyball_ui.player_overlay.assert_project_video"), \
                 patch("scripts.volleyball_ui.analysis_runs.cache_for",
                       return_value=cache), \
                 patch.object(data, "excluded_track_ids", return_value={"6"}), \
                 patch.object(player_identity_groups, "effective_track_identities",
                              return_value={"players": {"5": {
                                  "id": "a", "name": "Anna", "source": "assigned"}},
                                  "candidates": {}}):
                result = player_boxes_window(
                    "project", "match.mp4", 1, 2, cache_id="abc.cpu")
            self.assertEqual([box[0] for box in result["frames"][0][1]], [5])
            self.assertEqual(result["player_labels"]["5"]["name"], "Anna")


if __name__ == "__main__":
    unittest.main()
