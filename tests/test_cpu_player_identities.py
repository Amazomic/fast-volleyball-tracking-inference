"""Player identity interpretation is independent of expensive CPU/GPU stages."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_identity_groups
from scripts.volleyball_ui.player_overlay import player_boxes_window


class EffectiveIdentityTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.source = self.root / "players.json"
        self.source.write_text("{}", encoding="utf8")
        self.target = self.root / "groups.json"
        stat = self.source.stat()
        self.target.write_text(json.dumps({
            "schema": 1,
            "stamp": {"mtime_ns": stat.st_mtime_ns, "size": stat.st_size},
            "groups": [
                {"id": "manual-1", "source": "manual", "track_ids": ["5", "6"]},
                {"id": "reid-2", "source": "appearance", "track_ids": ["7", "8"]},
                {"id": "manual-conflict", "source": "manual", "track_ids": ["9", "10", "11"]},
            ],
        }), encoding="utf8")
        self.links = [
            {"id": "a", "name": "Анна", "track_ids": ["5"]},
            {"id": "b", "name": "Борис", "track_ids": ["9"]},
            {"id": "c", "name": "Сергей", "track_ids": ["10"]},
        ]

    def resolve(self):
        with patch.object(player_identity_groups, "_context",
                          return_value=(self.root, self.source, self.target)), \
             patch.object(data, "groups_for", return_value=self.links), \
             patch.object(data, "excluded_track_ids", return_value=set()):
            return player_identity_groups.effective_track_identities(
                "project", "video.mp4", "gpu123.cpu456")

    def test_manual_merge_inherits_only_unambiguous_named_player(self):
        resolved = self.resolve()
        self.assertEqual(resolved["players"]["5"]["name"], "Анна")
        self.assertEqual(resolved["players"]["6"]["name"], "Анна")
        self.assertEqual(resolved["players"]["6"]["source"], "manual_merge")
        self.assertEqual(resolved["candidates"]["6"], "player-a")
        self.assertEqual(resolved["candidates"]["7"], resolved["candidates"]["8"])
        self.assertNotIn("8", resolved["players"])  # ReID alone is not identity.
        self.assertNotIn("11", resolved["players"])  # Conflicting linked owners.
        self.assertNotEqual(resolved["candidates"]["9"],
                            resolved["candidates"]["10"])

    def test_name_is_read_live_from_player_registry(self):
        self.links[0]["name"] = "Анна П."
        self.assertEqual(self.resolve()["players"]["6"]["name"], "Анна П.")

    def test_overlay_uses_selected_cpu_version_and_returns_labels(self):
        self.source.write_text(json.dumps({
            "fps": 30, "source_size": {"width": 100, "height": 200},
            "predictions": [{"class_name": "player", "frame_index": 30,
                             "track_id": 6, "score": .9,
                             "bbox_xyxy": [10, 20, 60, 160]}],
        }), encoding="utf8")
        chosen = {"candidates": {"6": "player-a"},
                  "players": {"6": {"id": "a", "name": "Анна", "source": "manual_merge"}}}
        with patch("scripts.volleyball_ui.player_overlay.assert_project_video"), \
             patch("scripts.volleyball_ui.analysis_runs.cache_for",
                   return_value=self.root), \
             patch.object(player_identity_groups, "effective_track_identities",
                          return_value=chosen) as resolve, \
             patch.object(data, "excluded_track_ids", return_value=set()):
            result = player_boxes_window("project", "video.mp4", 1, 2,
                                         cache_id="gpu123.cpu456")
        self.assertEqual(result["player_labels"]["6"]["name"], "Анна")
        self.assertEqual(result["frames"][0][1][0][0], 6)
        resolve.assert_called_once_with("project", "video.mp4", "gpu123.cpu456")


if __name__ == "__main__":
    unittest.main()
