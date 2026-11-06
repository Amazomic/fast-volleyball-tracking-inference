"""CPU reprocessing progress must stay visible in the project sidebar."""
from __future__ import annotations

import io
import json
import shutil
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts.run_project_reprocess import process_project
from scripts.volleyball_ui import server


class ReprocessProgressTests(unittest.TestCase):
    def test_project_runner_emits_monotonic_stage_and_video_progress(self):
        parent = {"id": "base-run", "videos": {
            "first.mp4": "a" * 24, "second.mp4": "b" * 24}}
        output = Path(tempfile.mkdtemp(prefix="volley-replay-test-")) / ("c" * 24)
        output.mkdir()
        self.addCleanup(shutil.rmtree, output.parent)
        (output / "ranking.json").write_text("[]", encoding="utf-8")
        (output / "analysis").mkdir()
        (output / "analysis/events.json").write_text("{}", encoding="utf-8")
        (output / "postprocess.manifest.json").write_text(
            json.dumps({"schema": "volley-postprocessing/1",
                        "version_id": output.name,
                        "settings": {"mode": "combined"}}), encoding="utf-8")

        def fake_replay(cache, **settings):
            progress = settings["on_progress"]
            for step in range(1, 6):
                progress(step, "CPU stage " + str(step))
            return output

        log = io.StringIO()
        with (patch("scripts.run_project_reprocess.list_runs", return_value=[parent]),
              patch("scripts.run_project_reprocess.cache_for", return_value=Path("cache")),
              patch("scripts.run_project_reprocess.reprocess", side_effect=fake_replay),
              patch("scripts.volleyball_ui.analysis_runs.restore_postprocessing_version"),
              patch("scripts.volleyball_ui.analysis_runs.publish_postprocessing_snapshot",
                    return_value={"id": "fake-snapshot"}),
              redirect_stdout(log)):
            outcome = process_project("test-project", "base-run", "combined")
        events = [json.loads(line) for line in log.getvalue().splitlines()]
        self.assertEqual(len(events), 12)
        self.assertTrue(all(event["event"] == "UI_PROGRESS"
                            and event["kind"] == "reprocess" for event in events))
        progress = [event["overall_percent"] for event in events]
        self.assertEqual(progress, sorted(progress))
        self.assertEqual(progress[-1], 100)
        self.assertEqual(events[-1]["total_stages"], 10)
        self.assertEqual(events[-1]["completed_stages"], 10)
        self.assertEqual(events[-1]["videos_total"], 2)
        self.assertEqual(events[-1]["video_name"], "second.mp4")
        self.assertEqual(len(outcome["videos"]), 2)

    def test_job_state_exposes_cpu_progress_after_poll_and_reconnect(self):
        event = {
            "event": "UI_PROGRESS", "kind": "reprocess",
            "stage": "Траектории построены", "video_index": 1,
            "video_name": "match.mp4", "videos_total": 2,
            "stage_index": 3, "stage_total": 5,
            "completed_stages": 3, "total_stages": 10,
            "overall_percent": 30.0,
        }
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "job.log"
            log.write_text(json.dumps(event, ensure_ascii=False) + "\n", encoding="utf-8")

            class Running:
                returncode = None
                def poll(self):
                    return None

            state = {
                "log": log, "process": Running(), "kind": "reprocess",
                "id": "job-1", "project_id": "test-project",
                "progress_event": None, "progress_offset": 0,
                "progress_buffer": b"", "started_epoch": time.time() - 6,
                "started_at": "2026-11-04 00:00:00",
                "finished_epoch": None, "stage_durations": {},
            }
            with patch.object(server, "JOB", state):
                first = server.job_state()
                again = server.job_state()
            for result in (first, again):
                self.assertEqual(result["progress"], 30.0)
                self.assertEqual(result["video_name"], "match.mp4")
                self.assertEqual(result["completed_stages"], 3)
                self.assertEqual(result["total_stages"], 10)
                self.assertEqual(result["project_id"], "test-project")
                self.assertEqual(result["status"], "running")

    def test_preview_settings_are_not_mixed_with_display_filters(self):
        markup = (Path(__file__).resolve().parents[1] /
                  "scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        filter_start = markup.index('<fieldset class="result-overlay-controls result-cpu-overlays"')
        filter_end = markup.index("</fieldset>", filter_start)
        player = markup.index('id="result-video-player"')
        cpu_start = markup.index('<section class="result-cpu-tools"')
        timeline = markup.index('class="result-timeline"', player)
        self.assertLess(filter_end, player)
        self.assertLess(player, timeline)
        self.assertLess(timeline, cpu_start)
        self.assertLess(markup.index('id="result-rally-track"'), cpu_start)
        self.assertLess(markup.index('id="result-rally-track"'),
                        markup.index('id="result-rally-settings"'))
        self.assertLess(cpu_start, markup.index('id="result-ball-threshold"'))
        self.assertLess(cpu_start, markup.index('id="result-player-threshold"'))
        self.assertLess(markup.index('id="view-analysis-results"'), cpu_start)
        self.assertNotIn('id="analysis-cpu-settings"', markup)
        self.assertIn('id="sidebar-reprocess-task"', markup)
        self.assertIn('id="sidebar-reprocess-cancel"', markup)
        self.assertIn('id="result-reprocess-mode"', markup)
        self.assertNotIn('id="reprocess-mode"', markup)


if __name__ == "__main__":
    unittest.main()
