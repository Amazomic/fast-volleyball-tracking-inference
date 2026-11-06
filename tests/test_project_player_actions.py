"""Player recalc must preserve GPU events and assign touches to project players."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import player_actions


class PlayerActionsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.caches = {}
        self.mapping = {"1": {"id": "a", "name": "Ира"},
                        "2": {"id": "b", "name": "Оля"}}
        for video in ("first.mp4", "second.mp4"):
            path = self.root / video / "analysis"
            path.mkdir(parents=True)
            self.caches[video] = path.parent
        self._events("first.mp4", [
            {"id": "t1", "type": "attack", "player_track_id": 1,
             "time_sec": 3.1, "confidence": .95},
            {"id": "t2", "type": "dig", "player_track_id": 2,
             "time_sec": 3.5, "confidence": .6},
            {"id": "t3", "type": "serve", "player_track_id": 44,
             "time_sec": 4.1, "confidence": .55},
            {"type": "rally_start", "id": "r1"},
            {"id": "t4", "type": "attack", "player_track_id": 1,
             "time_sec": 9, "confidence": .77},
            {"type": "rally_end", "id": "r2"},
        ])
        self._events("second.mp4", [
            {"id": "t5", "type": "attack", "player_track_id": 1,
             "time_sec": 5, "confidence": .88},
            {"id": "t6", "type": "attack", "player_track_id": 55,
             "time_sec": 6, "confidence": .90},
        ])
        self.run = {"id": "run_test", "videos": {
            "first.mp4": "cache1.cpu", "second.mp4": "cache2.cpu"}}

    def _events(self, video, events):
        (self.caches[video] / "analysis" / "events.json").write_text(
            json.dumps({"events": events}), encoding="utf-8")

    def _patch(self, signature=None):
        from contextlib import ExitStack
        stack = ExitStack()
        stack.enter_context(patch.object(player_actions, "_run", return_value=self.run))
        stack.enter_context(patch.object(player_actions, "_signature",
                                         side_effect=signature or (lambda p, r: {"revision": 1})))
        stack.enter_context(patch.object(player_actions, "_path",
                         return_value=self.root / "actions.json"))
        stack.enter_context(patch.object(player_actions, "cache_for",
                         side_effect=lambda video, project, ident: self.caches[video]))
        stack.enter_context(patch.object(player_actions, "effective_track_identities",
                         side_effect=lambda p, v, c: {"players": self.mapping}))
        stack.enter_context(patch.object(player_actions.data, "players_for",
                         return_value=[{"id": "a", "name": "Ира"},
                                       {"id": "b", "name": "Оля"}]))
        stack.enter_context(patch.object(player_actions.data, "excluded_track_ids",
                         side_effect=lambda video, p, cache: (
                             {"55"} if video == "second.mp4" else set())))
        return stack

    def test_touches_are_scoped_and_available_for_montage(self):
        with self._patch():
            result = player_actions.recalculate("project", "run_test")
            self.assertEqual(result["total_touches"], 5)
            self.assertEqual(result["assigned_touches"], 4)
            self.assertEqual(result["unassigned_touches"], 1)
            players = {p["player_id"]: p for p in result["players"]}
            self.assertEqual(players["a"]["actions"]["attack"], 3)
            self.assertEqual(players["b"]["actions"]["dig"], 1)
            first = player_actions.get_actions("project", "run_test",
                                               player_id="a", action="attack")
            self.assertEqual([event["time_sec"] for event in first["events"]],
                             [3.1, 9, 5])
            self.assertEqual(first["events"][1]["rally_id"], "r1")
            self.assertNotIn("events", player_actions.get_actions("project", "run_test"))
            saved = json.loads((self.root / "actions.json").read_text(encoding="utf-8"))
            self.assertEqual(len(saved["events"]), 5)
            self.assertEqual(json.loads(
                (self.caches["first.mp4"] / "analysis" / "events.json").read_text(
                    encoding="utf-8"))["events"][0]["player_track_id"], 1)

    def test_recalculate_reflects_new_manual_merged_owner(self):
        with self._patch():
            player_actions.recalculate("project", "run_test")
            self.mapping["2"] = {"id": "a", "name": "Ира"}
            changed = player_actions.recalculate("project", "run_test")
            self.assertEqual(changed["players"][0]["touches"], 4)
            self.assertEqual(changed["players"][1]["touches"], 0)

    def test_stale_state_prevents_outdated_montage(self):
        with self._patch(signature=lambda p, r: {"revision": 3}):
            player_actions.recalculate("project", "run_test")
        with self._patch(signature=lambda p, r: {"revision": 4}):
            self.assertEqual(player_actions.get_actions("project", "run_test")["status"],
                             "stale")
        with self._patch():
            self.assertEqual(player_actions.get_actions("project", "run_test")["status"],
                             "stale")


if __name__ == "__main__":
    unittest.main()
