"""Cross-video montage must mix source matches into ONE playable output."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from scripts.volleyball_ui import data, projects, montage_project, montage_runner


class MultiSourceMontageTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        video_root = self.root / "videos"
        video_root.mkdir()
        for name in ("one.mp4", "two.mp4"):
            (video_root / name).write_bytes(b"source")
        results = self.root / "results"
        results.mkdir()
        for target, key, path in (
            (data, "PROJECT", video_root),
            (data, "RESULTS", results),
            (projects, "PROJECTS_DIR", results / "projects"),
            (projects, "REGISTER", results / "projects" / "projects.json"),
        ):
            modification = patch.object(target, key, path)
            modification.start()
            self.addCleanup(modification.stop)
        request = patch("scripts.volleyball_ui.preview.video_info",
                        return_value={"duration_sec": 600})
        request.start()
        self.addCleanup(request.stop)
        self.project = projects.create_project("Multiple", ["one.mp4", "two.mp4"])
        self.cache = {}
        for i, name in enumerate(self.project["videos"]):
            folder = self.root / ("analysis_" + str(i))
            tracks = folder / "tracks" / "analysis" / "tracks"
            tracks.mkdir(parents=True)
            game_tracks = []
            ranking = []
            for number in range(4):
                start = 100 + number*360 + i*30
                payload = {
                    "start_frame": start, "last_frame": start + 210,
                    "track_id": i*10 + number + 1,
                    "positions": [[[200, 100], start+100]],
                    "prediction": [200, 100],
                    "rally_classification": {
                        "label": "rally", "is_rally": True, "rally_confidence": .9
                    },
                    "rally_features": {
                        "attack_count": 2 + number, "block_count": 1,
                        "contact_count": 4,
                        "last_contact_frame": start + 160
                    },
                }
                (tracks / f"track_{i*10+number+1:04}.json").write_text(json.dumps(payload))
                game_tracks.append(payload)
                ranking.append({"start": start, "end": start + 210,
                                "score": float(18+number*2+i),
                                "confidence": .9})
            analysis = folder / "analysis"
            analysis.mkdir()
            (analysis / "game-tracks.json").write_text(json.dumps(game_tracks))
            (analysis / "events.json").write_text(json.dumps({"events": []}))
            (folder / "ranking.json").write_text(json.dumps(ranking))
            self.cache[name] = folder

    def test_every_mode_uses_every_project_video(self):
        with patch.object(data, "find_cache",
                          side_effect=lambda name, pid: self.cache[name]):
            for mode in ("tracks", "vertical", "custom"):
                config = montage_runner.validate_request({
                    "project_id": self.project["id"], "duration": 24,
                    "mode": mode, "video": "one.mp4"
                })
                self.assertEqual(config["videos"], ["one.mp4", "two.mp4"])
                self.assertNotIn("video", config)

    def test_selected_episodes_interleave_sources(self):
        sources = [{"video": name, "video_path": self.root / name,
                    "cache": folder, "fps": 30.0}
                   for name, folder in self.cache.items()]
        sequence = montage_project.plan_clips(
            sources, self.root / "selected_tracks", 55,
            mode="tracks", padding=.2)
        self.assertGreaterEqual(len(sequence), 4)
        names = [x["video"] for x in sequence]
        self.assertEqual(set(names), {"one.mp4", "two.mp4"})
        self.assertTrue(all(x["source_duration"] >= 7 for x in sequence))
        self.assertTrue(all(x["end_frame"] - x["start_frame"] + 1 >= 211
                            for x in sequence))
        self.assertEqual(len(list((self.root / "selected_tracks").glob("*/track_*.json"))),
                         len(sequence))
        manifest = json.loads((self.root / "selection.json").read_text())
        self.assertEqual(manifest["source_count"], 2)
        self.assertEqual(len(manifest["selected"]), len(sequence))

    def test_vertical_runner_invokes_original_make_reels_for_each_source(self):
        import contextlib
        import io
        from unittest.mock import patch
        calls = []
        composed = []
        cut_sources = []
        def fake_clip(source, target, **params):
            cut_sources.append(Path(source).name)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"high fidelity original episode")
            return {"start_sec":params["start_frame"]/30,"fps":30,
                    "frames":params["end_frame"]-params["start_frame"]+1}

        def fake_run(cmd, **kwargs):
            calls.append(cmd)
            self.assertTrue(any("make_reels.py" in str(arg) for arg in cmd))
            self.assertNotIn("--zoom-max", cmd)
            self.assertEqual(cmd[cmd.index("--interpolation") + 1], "hold")
            self.assertEqual(cmd[cmd.index("--margin") + 1], "0.0")
            output_dir = Path(cmd[cmd.index("--output_dir") + 1])
            reel = output_dir / "video" / "reels" / "reel_video_0001.mp4"
            reel.parent.mkdir(parents=True, exist_ok=True)
            reel.write_bytes(b"fake mp4")
            return None

        def fake_compose(clips, output, duration, mode):
            self.assertEqual(mode, "vertical")
            self.assertEqual(duration, 25)
            composed.extend(clips)
            output.write_bytes(b"project vertical")

        request = {
            "project_id": self.project["id"], "mode": "vertical",
            "duration": 25, "request_id": "20261101_123456_12345678",
        }
        with (patch.object(data, "find_cache",
                           side_effect=lambda name, pid: self.cache[name]),
              patch("scripts.volleyball_ui.montage_source.clip_from_original",
                    side_effect=fake_clip),
              patch("scripts.volleyball_ui.montage_encoding.nvenc_available",
                    return_value=False),
              patch.object(montage_runner.subprocess, "run", side_effect=fake_run),
              patch.object(montage_runner, "compose_clips", side_effect=fake_compose),
              contextlib.redirect_stdout(io.StringIO())):
            montage_runner.execute(request)
        self.assertGreaterEqual(len(calls), 2)
        self.assertEqual(set(cut_sources),{"one.mp4","two.mp4"})
        self.assertEqual({Path(cmd[cmd.index("--video_path")+1]).name for cmd in calls},
                         {"original_episode.mp4"})
        self.assertEqual(len(composed), len(calls))
        self.assertEqual(len(list((projects.project_dir(self.project["id"]) /
                                   "montages" / request["request_id"]).glob("project_vertical.mp4"))), 1)

    def test_all_modes_share_exact_full_rally_boundaries(self):
        sources = [{"video": name, "cache": folder, "video_path": self.root / name}
                   for name, folder in self.cache.items()]
        expected = montage_project.plan_rallies(sources, 45, padding=1.5)
        keys = [(x["video"], x["source_start_frame"], x["source_end_frame"])
                for x in expected]
        for mode in ("tracks", "vertical", "custom"):
            selected = montage_project.plan_clips(
                sources, self.root / mode / "selected_tracks",
                45, mode=mode, padding=1.5)
            self.assertEqual(
                [(x["video"], x["start_frame"], x["end_frame"]) for x in selected],
                keys)
            for clip in selected:
                payload=json.loads(clip["track_json"].read_text(encoding="utf-8"))
                self.assertEqual((payload["start_frame"],payload["last_frame"]),
                                 (clip["start_frame"],clip["end_frame"]))
                self.assertTrue(payload["positions"])


    def test_montage_preview_pins_selected_profile_and_reconstruction(self):
        selected_id = "run_20261102_123456_aabbccdd"
        keys = {video: f"{index+1:016x}" for index, video in
                enumerate(self.project["videos"])}
        run = {"id": selected_id, "profile": "Accuracy",
               "videos": keys}
        def selected_cache(video, project_id, cache_id):
            self.assertEqual(project_id, self.project["id"])
            self.assertEqual(cache_id, keys[video])
            return self.cache[video]
        request = {
            "project_id": self.project["id"], "mode": "tracks",
            "duration": 55, "run_id": selected_id,
            "gap_sec": 3, "rally_min_sec": 3, "context_sec": 1.5,
            "min_rally": 1.5, "min_confidence": .5,
        }
        with (patch("scripts.volleyball_ui.analysis_runs.list_runs",
                    return_value=[run]),
              patch("scripts.volleyball_ui.analysis_runs.cache_for",
                    side_effect=selected_cache),
              patch.object(data, "find_cache",
                           side_effect=AssertionError("latest cache was used")),
              patch("scripts.volleyball_ui.prepared_videos.ready_path",
                    return_value=None)):
            preview = montage_runner.preview_selection(request)
            self.assertEqual(preview["run_id"], selected_id)
            self.assertEqual(preview["profile"], "Accuracy")
            self.assertEqual(preview["settings"]["gap_sec"], 3)
            self.assertTrue(preview["selected"])
            sources = montage_runner.sources_for(
                montage_runner.validate_request(request))
            actual = montage_project.plan_clips(
                sources, self.root/"pinned"/"selected_tracks",
                55, mode="tracks", padding=1.5, min_rally=1.5,
                min_confidence=.5, gap_sec=3, rally_min_sec=3)
            self.assertEqual(
                [(row["video"], row["start"], row["end"])
                 for row in preview["selected"]],
                [(row["video"], round(row["start_frame"]/30, 3),
                  round((row["end_frame"]+1)/30, 3)) for row in actual])
            widened = montage_runner.preview_selection(
                {**request, "gap_sec": 6})
            self.assertNotEqual(
                [(x["video"], x["start"], x["end"])
                 for x in preview["selected"]],
                [(x["video"], x["start"], x["end"])
                 for x in widened["selected"]])
            with self.assertRaisesRegex(ValueError, "запуск анализа"):
                montage_runner.validate_request({**request, "run_id": "run_unknown"})

    def test_montage_ui_requires_run_and_shows_selection(self):
        root = Path(__file__).resolve().parents[1]
        html = (root/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        app = (root/"scripts/volleyball_ui/static/app.js").read_text(encoding="utf-8")
        server = (root/"scripts/volleyball_ui/server.py").read_text(encoding="utf-8")
        self.assertIn('id="montage-plan-list"', html)
        self.assertIn('id="montage-plan-summary"', html)
        self.assertIn('id="chain-postprocess"', html)
        self.assertNotIn('data-analysis-run-select aria-label="Выбрать профиль анализа для монтажа"', html)
        self.assertIn('api("/api/montage/plan",params)', app)
        self.assertIn('run_id:selectedAnalysisRun()?.id', app)
        self.assertIn('"/api/montage/plan"', server)

    def test_saved_analysis_events_counted_per_rally_without_duplicates(self):
        from scripts.full_rallies import load_groups
        cache=self.cache["one.mp4"]
        events=[
            {"id":"attack1","frame":150,"type":"attack"},
            {"id":"attack1","frame":150,"type":"attack"},
            {"id":"attack2","frame":160,"type":"attack"},
            {"id":"block1","frame":170,"type":"block"},
            {"id":"other","frame":510,"type":"attack"},
            {"id":"boundary","frame":100,"type":"rally_start"},
        ]
        (cache/"analysis/events.json").write_text(
            json.dumps({"events":events}),encoding="utf8")
        ranking=json.loads((cache/"ranking.json").read_text(encoding="utf8"))
        groups=load_groups(cache,ranking)
        first=next(row for row in groups if row["detected_start_frame"]==100)
        self.assertEqual(first["action_counts"]["attack"],2)
        self.assertEqual(first["action_counts"]["block"],1)
        self.assertEqual(first["actions_total"],3)

    def test_manual_selection_strategy_and_action_filters(self):
        source={"video":"one.mp4","cache":self.cache["one.mp4"]}
        def group(start, length, base, *, attack=0, block=0, dig=0):
            counts={"attack":attack,"block":block,"dig":dig}
            return {
                "detected_start_frame":start,
                "detected_end_frame":start+length-1,
                "source_start_frame":start,
                "source_end_frame":start+length-1,
                "duration_sec":round(length/30,4),
                "confidence":.9, "ranked":True,
                "ranking_score":base,
                "action_counts":counts,
                "actions_total":sum(counts.values()),
            }
        items=[
            group(100,240,10,attack=4),
            group(500,270,20,block=3),
            group(1000,360,2,attack=2,block=1),
        ]
        def load(*args, **kwargs):
            return items
        def select(strategy, **extra):
            return montage_project.plan_rallies(
                [source],12,selection_strategy=strategy,loader=load,**extra)
        self.assertEqual(select("highlights")[0]["detected_start_frame"],500)
        self.assertEqual(select("longest")[0]["detected_start_frame"],1000)
        self.assertEqual(select("most_actions")[0]["detected_start_frame"],100)
        self.assertEqual(select("attacks")[0]["detected_start_frame"],100)
        self.assertEqual(select("blocks")[0]["detected_start_frame"],500)
        self.assertEqual(select("attacks",min_action_counts={"block":1})[0][
            "detected_start_frame"],1000)
        self.assertEqual(select("most_actions",min_action_counts={"actions":4})[0][
            "detected_start_frame"],100)
        with self.assertRaises(ValueError):
            select("blocks",min_action_counts={"attack":5})
        with self.assertRaises(ValueError):
            select("no-such-strategy")

    def test_single_mode_follow_ball_switches_renderer_not_selection(self):
        from types import SimpleNamespace
        cache=self.cache["one.mp4"]
        raw={"project_id":self.project["id"],"mode":"montage",
             "duration":16,"selection_strategy":"highlights",
             "min_actions":0,"follow_ball":True}
        commands=[]
        composed=[]
        cut_sources=[]
        def fake_clip(source,target,**params):
            cut_sources.append(Path(source).name)
            target.parent.mkdir(parents=True,exist_ok=True)
            target.write_bytes(b"original native quality")
            return {"start_sec":params["start_frame"]/30,"fps":30,
                    "frames":params["end_frame"]-params["start_frame"]+1}
        def fake_run(cmd,**kwargs):
            commands.append(cmd)
            if "make_reels.py" in str(cmd):
                folder=Path(cmd[cmd.index("--output_dir")+1])
                target=folder/"video"/"reels"/"reel_video_0001.mp4"
                target.parent.mkdir(parents=True,exist_ok=True)
                target.write_bytes(b"portrait")
            elif "track_processor.py" in str(cmd):
                Path(cmd[cmd.index("--output_path")+1]).write_bytes(b"landscape")
            return SimpleNamespace(stdout="")
        def fake_compose(segments,output,duration,mode):
            composed.append((mode,output.name))
            output.write_bytes(b"composed")
        for follow,expected_tool,aspect,name in (
            (True,"make_reels.py","vertical","highlights_reel.mp4"),
            (False,None,"tracks","highlights_full.mp4"),
        ):
            commands.clear()
            config={**raw,"follow_ball":follow,"request_id":
                    "20261102_000000_"+("yes" if follow else "no")}
            with (patch.object(data,"find_cache",return_value=cache),
                  patch("scripts.volleyball_ui.montage_source.clip_from_original",
                        side_effect=fake_clip),
                  patch("scripts.volleyball_ui.montage_encoding.nvenc_available",
                        return_value=False),
                  patch.object(montage_runner.subprocess,"run",side_effect=fake_run),
                  patch.object(montage_runner,"compose_clips",side_effect=fake_compose),
                  patch("scripts.volleyball_ui.prepared_videos.ready_path",return_value=None),
                  patch("scripts.volleyball_ui.montage_runner.project_dir",
                        side_effect=lambda _:self.root/"exports")):
                import contextlib,io
                with contextlib.redirect_stdout(io.StringIO()):
                    montage_runner.execute(config)
            self.assertTrue(cut_sources)
            self.assertEqual(cut_sources[-1],"one.mp4")
            if expected_tool:
                self.assertTrue(any(expected_tool in str(cmd) for cmd in commands))
            else:
                self.assertFalse(any("track_processor.py" in str(cmd)
                                     for cmd in commands))
            self.assertEqual(composed[-1],(aspect,name))
            record=self.root/"exports"/"montages"/config["request_id"]/"selection.json"
            stored=json.loads(record.read_text(encoding="utf8"))
            self.assertEqual(stored["follow_ball"],follow)
            self.assertEqual(stored["selection_filters"]["strategy"],"highlights")

    def test_single_mode_form_replaces_legacy_three_mode_selector(self):
        root=Path(__file__).resolve().parents[1]
        html=(root/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf8")
        js=(root/"scripts/volleyball_ui/static/app.js").read_text(encoding="utf8")
        self.assertNotIn('id="montage-mode"',html)
        self.assertIn('name="mode" value="montage"',html)
        self.assertIn('id="montage-follow-ball" checked',html)
        self.assertIn('name="selection_strategy"',html)
        for key in ("longest","most_actions","attacks","blocks","digs","passes","serves"):
            self.assertIn('value="'+key+'"',html)
        for key in ("min_actions","min_attack","min_block","min_dig",
                    "min_overhead_pass","min_serve"):
            self.assertIn('name="'+key+'"',html)
        self.assertIn('params.follow_ball=$("#montage-follow-ball").checked;',js)

    def test_unanalyzed_recording_blocks_montage(self):
        with patch.object(data, "find_cache",
                          side_effect=lambda name, pid: self.cache.get(name) if name=="one.mp4" else None):
            with self.assertRaisesRegex(ValueError, "two.mp4"):
                montage_runner.validate_request({
                    "project_id": self.project["id"], "mode": "tracks", "duration": 15
                })


if __name__ == "__main__":
    unittest.main()
