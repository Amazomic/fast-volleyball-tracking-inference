import unittest
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from scripts.volleyball_ui import projects


class ReportStatisticsTests(unittest.TestCase):
    def test_unassigned_contact_uses_source_identity_without_merging_other_contacts(self):
        from scripts.volleyball_ui.report_statistics import summarize_actions
        actions = [
            {"id": "touch", "contact_id": "ball-1:60", "type": "unknown_touch", "time": 2},
            {"id": "attack", "contact_id": "ball-1:60", "type": "attack", "time": 2},
            {"id": "other", "contact_id": "ball-2:60", "type": "block", "time": 2},
        ]
        result = summarize_actions(actions, {}, set())
        self.assertEqual(result["total_touches"], 2)
        self.assertEqual(result["unassigned_touches"], 2)
        self.assertEqual(result["counts"], {"attack": 1, "block": 1})

    def test_video_uses_all_preview_rallies_and_current_identity_assignments(self):
        from scripts.volleyball_ui.report_statistics import video_statistics
        events = [
            {"id": "generic", "type": "unknown_touch", "time_sec": 5, "frame": 150,
             "player_track_id": 1},
            {"id": "classified", "type": "attack", "time_sec": 5, "frame": 150,
             "player_track_id": 1},
            {"id": "second", "type": "dig", "time_sec": 80, "frame": 2400,
             "player_track_id": 2},
            {"id": "outside", "type": "serve", "time_sec": 40, "frame": 1200,
             "player_track_id": 1},
            {"id": "excluded", "type": "block", "time_sec": 10, "frame": 300,
             "player_track_id": 9},
            {"id": "unassigned-touch", "type": "unknown_touch", "time_sec": 12,
             "frame": 360, "track_id": "ball-a"},
            {"id": "unassigned-attack", "type": "attack", "time_sec": 12,
             "frame": 360, "track_id": "ball-a"},
        ]
        with TemporaryDirectory(dir=Path.cwd() / "output") as temp:
            cache = Path(temp)
            (cache / "analysis").mkdir()
            (cache / "analysis" / "events.json").write_text(json.dumps({
                "schema": "volleyball-events/2", "fps": 30, "events": events}), encoding="utf-8")
            report = {"analyzed": True, "postprocessed": True, "excluded_track_ids": ["9"],
                      "raw": [{"player_track_id": "1", "player_id": "a"},
                              {"player_track_id": "2", "player_id": "a"}]}
            with patch("scripts.volleyball_ui.analysis_runs.cache_for", return_value=cache), \
                 patch("scripts.volleyball_ui.data.video_path"), \
                 patch("scripts.volleyball_ui.data.groups_for", return_value=[]), \
                 patch("scripts.volleyball_ui.rally_view.from_tracks", return_value={"rallies": [
                     {"start": 0, "end": 20, "detected_start": 0, "detected_end": 20},
                     {"start": 70, "end": 90, "detected_start": 70, "detected_end": 90}]}):
                result = video_statistics("match.mp4", "project", "gpu.cpu", report)
            self.assertEqual(result["total_touches"], 3)
            self.assertEqual(result["players"]["a"]["touches"], 2)
            self.assertEqual(result["unassigned_touches"], 1)
            self.assertEqual(result["rallies"], 2)
            self.assertEqual(result["counts"], {"attack": 2, "dig": 1})

    def test_unique_contacts_include_unknowns_and_keep_distinct_players(self):
        from scripts.volleyball_ui.report_statistics import summarize_actions
        actions = [
            {"id": "touch", "type": "unknown_touch", "time": 2, "player_id": "1"},
            {"id": "attack", "type": "attack", "time": 2, "player_id": "1"},
            {"id": "block", "type": "block", "time": 2, "player_id": "2"},
            {"id": "next", "type": "low_touch", "time": 2.1, "player_id": "1"},
            {"id": "later", "type": "serve", "time": 80, "player_id": "3"},
            {"id": "excluded", "type": "dig", "time": 90, "player_id": "9"},
            {"id": "free", "type": "unknown_touch", "time": 100, "player_id": None},
        ]
        result = summarize_actions(actions, {"1": "a", "2": "b", "3": "a"}, {"9"})
        self.assertEqual(result["total_touches"], 5)
        self.assertEqual(result["unassigned_touches"], 1)
        self.assertEqual(result["players"]["a"]["touches"], 3)
        self.assertEqual(result["players"]["a"]["recognized_touches"], 2)
        self.assertEqual(result["counts"]["attack"], 1)
        self.assertEqual(result["counts"]["block"], 1)

    def test_activity_uses_even_and_odd_median_including_zero_players(self):
        from scripts.volleyball_ui.report_statistics import apply_activity
        players = [{"touches": n} for n in (0, 10, 20, 30)]
        self.assertEqual(apply_activity(players), 15)
        self.assertEqual([p["activity"] for p in players], [0, 2 / 3, 4 / 3, 2])
        self.assertEqual(apply_activity([{"touches": 5}, {"touches": 10}, {"touches": 30}]), 10)
        zero = [{"touches": 0}, {"touches": 0}, {"touches": 2}]
        self.assertEqual(apply_activity(zero), 0)
        self.assertTrue(all(p["activity"] is None for p in zero))

    def test_project_counts_only_the_selected_processing_version(self):
        stats = {"analyzed": True, "rallies": 4, "counts": {"attack": 1},
                 "total_touches": 1, "unassigned_touches": 1, "players": {}}
        with TemporaryDirectory(dir=Path.cwd() / "output") as temp, \
             patch.object(projects, "get_project", return_value={
                 "name": "Матч", "videos": ["one", "two"]}), \
             patch.object(projects, "project_dir", return_value=Path(temp)), \
             patch.object(projects.data, "players_for", return_value=[]), \
             patch.object(projects.data, "report", return_value={"groups": []}) as reports, \
             patch("scripts.volleyball_ui.report_statistics.video_statistics", return_value=stats), \
             patch("scripts.volleyball_ui.analysis_runs.list_runs", return_value=[
                 {"id": "selected", "videos": {"two": "gpu.cpu-selected"}},
                 {"id": "other", "videos": {"one": "gpu.cpu-other"}}]):
            result = projects.project_report("project", "selected")
        self.assertEqual(result["total_touches"], 1)
        self.assertEqual(result["rallies"], 4)
        self.assertEqual([row["name"] for row in result["sources"]], ["two"])
        reports.assert_called_once_with("two", "project", "gpu.cpu-selected")

    def test_project_keeps_full_roster_and_sums_all_videos_by_player_id(self):
        roster = [{"id": "a", "name": "Иван"}, {"id": "b", "name": "Иван"},
                  {"id": "c", "name": "Оля"}]
        report = {"analyzed": True, "rallies": 2, "counts": {}, "groups": [], "raw": []}
        stats = [
            {"analyzed": True, "rallies": 2, "counts": {"attack": 2}, "total_touches": 3,
             "unassigned_touches": 1, "players": {"a": {"touches": 2,
             "recognized_touches": 2, "actions": {"attack": 2}}}},
            {"analyzed": True, "rallies": 3, "counts": {"block": 1}, "total_touches": 2,
             "unassigned_touches": 0, "players": {"b": {"touches": 2,
             "recognized_touches": 1, "actions": {"block": 1}}}},
        ]
        with TemporaryDirectory(dir=Path.cwd() / "output") as temp, \
             patch.object(projects, "get_project", return_value={"name": "Матч", "videos": ["one", "two"]}), \
             patch.object(projects, "project_dir", return_value=Path(temp)), \
             patch.object(projects.data, "players_for", return_value=roster), \
             patch.object(projects.data, "report", return_value=report), \
             patch("scripts.volleyball_ui.report_statistics.video_statistics", side_effect=stats), \
             patch("scripts.volleyball_ui.player_library.album_photos", return_value={"photos": []}):
            result = projects.project_report("project")
        self.assertEqual(result["total_touches"], 5)
        self.assertEqual(result["rallies"], 5)
        self.assertEqual([p["id"] for p in result["groups"]], ["a", "b", "c"])
        self.assertEqual([p["touches"] for p in result["groups"]], [2, 2, 0])
        self.assertEqual([p["activity"] for p in result["groups"]], [1, 1, 0])
        self.assertEqual(result["unassigned_touches"], 1)


if __name__ == "__main__":
    unittest.main()
