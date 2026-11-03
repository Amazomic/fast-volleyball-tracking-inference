"""Track-based reels: exact requested duration, meaningful clip selection, and stage progress."""
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from scripts.volleyball_ui import data, projects, montage_runner, montage_library, montage_project


class MontageProgressTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        videos = self.root / "project"
        videos.mkdir()
        (videos / "match.mp4").write_bytes(b"test source")
        results = self.root / "results"
        results.mkdir()
        for target, name, value in (
            (data, "PROJECT", videos),
            (data, "RESULTS", results),
            (projects, "PROJECTS_DIR", results / "projects"),
            (projects, "REGISTER", results / "projects" / "projects.json"),
            (montage_library, "RESULTS", results),
        ):
            current = patch.object(target, name, value)
            current.start()
            self.addCleanup(current.stop)
        info = patch("scripts.volleyball_ui.preview.video_info",
                     return_value={"duration_sec": 150})
        info.start()
        self.addCleanup(info.stop)
        self.project_id = projects.create_project("A", ["match.mp4"])["id"]
        self.cache = self.root / "cache"
        tracks = self.cache / "tracks" / "analysis" / "tracks"
        tracks.mkdir(parents=True)
        payload = {
            "start_frame": 100, "last_frame": 400,
            "positions": [[[15, 17], 250]],
            "prediction": [15, 17],
            "track_id": 1, "rally_classification": {
                "label": "rally", "is_rally": True,
                "rally_confidence": .94,
            },
            "rally_features": {
                "contact_count": 7, "attack_count": 2,
                "last_contact_frame": 350,
            },
        }
        (tracks / "track_0001.json").write_text(json.dumps(payload), encoding="utf-8")
        analysis = self.cache / "analysis"
        analysis.mkdir()
        (analysis / "game-tracks.json").write_text(json.dumps([payload]))
        (self.cache / "ranking.json").write_text(json.dumps([{
            "start": 100, "end": 400, "score": 17.4, "confidence": .94
        }]))

    def test_short_request_never_truncates_full_rally(self):
        sources=[{"video":"match.mp4", "cache":self.cache,
                  "video_path":self.root/"project"/"match.mp4"}]
        with self.assertRaisesRegex(RuntimeError, "complete rally|No complete"):
            montage_project.plan_clips(
                sources, self.root/"chosen", 1.0, mode="tracks", padding=.1)
        selected=montage_project.plan_clips(
            sources,self.root/"long"/"selected_tracks",15,mode="tracks",padding=.1)
        self.assertEqual(len(selected),1)
        self.assertEqual((selected[0]["detected_start_frame"],
                          selected[0]["detected_end_frame"]),(100,400))
        payload=json.loads(selected[0]["track_json"].read_text(encoding="utf-8"))
        self.assertEqual((payload["start_frame"],payload["last_frame"]),(97,400))

    def test_runner_produces_four_stages_and_uses_requested_duration(self):
        cut_origins=[]
        composed=[]
        def fake_clip(source,target,**params):
            cut_origins.append((Path(source).name,params))
            target.parent.mkdir(parents=True,exist_ok=True)
            target.write_bytes(b"original source frames")
            return {"start_sec":params["start_frame"]/30,"fps":30,
                    "frames":params["end_frame"]-params["start_frame"]+1}
        def fake_compose(clips,output,duration,mode):
            composed.append((list(clips),duration,mode))
            output.write_bytes(b"native quality montage")
        opts={"project_id":self.project_id,"mode":"tracks",
              "duration":15,"track_padding":1,
              "request_id":"20261101_195633_abcd1234"}
        out=io.StringIO()
        with (patch.object(data,"find_cache",return_value=self.cache),
              patch("scripts.volleyball_ui.montage_source.clip_from_original",
                    side_effect=fake_clip),
              patch.object(montage_runner,"compose_clips",side_effect=fake_compose),
              contextlib.redirect_stdout(out)):
            montage_runner.execute(opts)
        events=[json.loads(line) for line in out.getvalue().splitlines()
                if line.startswith("{")]
        self.assertEqual(len(events),8)
        self.assertEqual([event["completed_stages"] for event in events],
                         [0,1,1,2,2,3,3,4])
        self.assertTrue(all(event["total_stages"]==4 for event in events))
        self.assertEqual([item[0] for item in cut_origins],["match.mp4"])
        self.assertEqual(composed[0][1:],(15,"tracks"))
        output=projects.project_dir(self.project_id)/"montages"/opts["request_id"]/"project_tracks.mp4"
        self.assertTrue(output.is_file())
        selection=json.loads((output.parent/"selection.json").read_text(encoding="utf8"))
        self.assertEqual(selection["rally_settings"],
                         {"gap_sec":3.0,"min_sec":3.0,"context_sec":1.5})
        self.assertIn("analysis_cache_ids",selection)
        self.assertEqual(selection["selection_filters"]["min_confidence"],.5)

    def test_server_ignores_nested_percentage_events_for_montage(self):
        from scripts.volleyball_ui import server
        log = self.root / "job.log"
        events = [
            {"event": "UI_PROGRESS", "kind": "highlights", "origin": "montage_runner",
             "completed_stages": 1, "total_stages": 4, "overall_percent": 25},
            {"event": "UI_PROGRESS", "kind": "highlights", "origin": "child_builder",
             "completed_stages": 0, "total_stages": 0, "overall_percent": 99},
            {"event": "UI_PROGRESS", "kind": "highlights", "origin": "montage_runner",
             "completed_stages": 2, "total_stages": 4, "overall_percent": 50},
        ]
        log.write_text("".join(json.dumps(item) + "\n" for item in events), encoding="utf-8")
        saved = server.JOB.copy()
        try:
            server.JOB.update(
                log=log, kind="highlights", montage_mode="tracks",
                progress_event=None, progress_offset=0, progress_buffer=b"")
            server._refresh_progress_events()
            progress = server.JOB["progress_event"]
            self.assertEqual(progress["completed_stages"], 2)
            self.assertEqual(progress["total_stages"], 4)
            self.assertEqual(progress["origin"], "montage_runner")
        finally:
            server.JOB.clear()
            server.JOB.update(saved)

    def test_all_exports_ordered_newest_first_and_titled_by_date(self):
        folder = projects.project_dir(self.project_id) / "montages"
        older = folder / "20261101_173000_aaaa0000"
        newer = folder / "20261101_195633_bbbb0000"
        for target in (older, newer):
            target.mkdir(parents=True)
            (target / "finished.mp4").write_bytes(b"tiny")
        with patch.object(montage_library, "codec_for", return_value="h264"):
            items = montage_library.listing(self.project_id)
        self.assertEqual(len(items), 2)
        self.assertEqual([i["title"] for i in items],
                         ["01.11.2026 · 19:56:33", "01.11.2026 · 17:30:00"])
        self.assertEqual([i["date_source"] for i in items], ["created", "created"])
        self.assertEqual([i["name"] for i in items], ["finished.mp4", "finished.mp4"])


if __name__ == "__main__":
    unittest.main()
