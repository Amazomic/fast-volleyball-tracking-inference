"""Offline contract tests for cache separation and versioned CPU replay."""
from __future__ import annotations

import csv
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_studio.common import digest, save
from scripts.volleyball_studio.inference_cache import InferenceCacheError, load
from scripts.volleyball_studio.reprocess import (
    MODES, reprocess, _ravel_contacts, _merge_contacts
)


class ReplayTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.cache = root / "cache"
        self.cache.mkdir()
        self.source = root / "original.mp4"
        self.source.write_bytes(b"source video (fixture)")
        (self.cache / "analysis.mp4").write_bytes(b"prepared CFR input (fixture)")
        (self.cache / "court.json").write_text("{}", encoding="utf-8")
        (self.cache / "source.probe.json").write_text(
            json.dumps({"format": {"duration": "1.0"}}), encoding="utf-8")
        self.players = [
            {"frame_index": f, "class_name": "ball", "score": .8,
             "bbox_xyxy": [f*10, 20, f*10+4, 24]} for f in range(1, 31)]
        # Dense five-frame action evidence survives RAVEL-013 smoothing.
        for frame in range(3, 8):
            self.players.append({
                "frame_index": frame, "class_name": "player", "score": .85,
                "track_id": 9, "action": "Touch", "action_score": .85,
                "bbox_xyxy": [10, 10, 50, 100]})
        save(self.cache / "players.json", {
            "model": "models/RAVEL-VB-013-9f.onnx",
            "predictions": self.players, "benchmark": {"processed_frames": 31}})
        (self.cache / "ball/analysis").mkdir(parents=True)
        with (self.cache / "ball/analysis/ball.csv").open(
                "w", newline="", encoding="utf-8") as file:
            writer = csv.writer(file)
            writer.writerow(["Frame", "Visibility", "X", "Y", "Radius"])
            for frame in range(1, 31):
                writer.writerow([frame, 1, frame*10+2, 22, 5])
        self.original_bytes = (self.cache / "players.json").read_bytes()
        outputs = {}
        for relative in ("players.json", "ball/analysis/ball.csv"):
            candidate = self.cache / relative
            outputs[relative] = {
                "bytes": candidate.stat().st_size, "sha256": digest(candidate)}
        save(self.cache / "inference.manifest.json", {
            "schema": "volley-inference/1",
            "source": str(self.source),
            "source_identity": [str(self.source), 1, 1, "fixture-source-hash"],
            "input_sha256": digest(self.cache / "analysis.mp4"),
            "ball_model": "VballNetV4c", "ball_backend": "onnx",
            "ball_threshold": .5, "player_model": "RAVEL-VB-013-9f",
            "player_score_threshold": .42,
            "player_ball_threshold": .35,
            "player_action_threshold": .5,
            "fps": 30, "width": 1280, "height": 720, "outputs": outputs,
            "limitations": ["sidecars are not complete logits"],
        })

    @staticmethod
    def fake_tracker(command, logfile):
        assert "track_calculator_with_court.py" in str(command[1]), command
        directory = Path(command[command.index("--output_dir")+1])
        folder = directory / "analysis/tracks"
        folder.mkdir(parents=True, exist_ok=True)
        track = {
            "start_frame": 1, "last_frame": 30,
            "positions": [[[frame*10+2 if frame<=5 else 52+20*(frame-5), 22], frame]
                          for frame in range(1, 31)],
            "frame_observations": [
                {"frame": frame,
                 "x": frame*10+2 if frame<=5 else 52+20*(frame-5), "y": 22}
                for frame in range(1, 31)],
            "player_interaction": {"contacts": [{
                "frame": 15, "ball_x": 152, "ball_y": 22,
                "contact_type": "dig", "confidence": .85,
                "player_track_id": 9, "speed_after_px_frame": 10,
                "direction_change_deg": 90, "side": "near"}]},
            "rally_classification": {"is_rally": True,
                                      "rally_confidence": .95},
        }
        save(folder / "track_0001.json", track)

    def test_deleted_player_ids_are_removed_from_cpu_replay_and_versioned(self):
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker):
            original = reprocess(self.cache, mode="baseline")
            filtered = reprocess(
                self.cache, mode="baseline", excluded_player_ids={"9"})
        self.assertNotEqual(original, filtered)
        self.assertFalse((original / "players.replay.json").exists())
        player_data = json.loads(
            (filtered / "players.replay.json").read_text(encoding="utf-8"))
        self.assertFalse([p for p in player_data["predictions"]
                          if p.get("class_name") == "player"])
        settings = json.loads(
            (filtered / "postprocess.manifest.json").read_text(
                encoding="utf-8"))["settings"]
        self.assertEqual(settings["excluded_player_ids"], ["9"])
        self.assertEqual((self.cache / "players.json").read_bytes(),
                         self.original_bytes)

    def test_default_ravel013_replay_uses_touch_peaks(self):
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker):
            result = reprocess(self.cache)
        settings = json.loads((result / "postprocess.manifest.json").read_text("utf8"))["settings"]
        self.assertEqual(settings["mode"], "combined")
        self.assertTrue(any(event["status"] == "model_prediction"
                            for event in json.loads(
                                (result / "analysis/events.json").read_text("utf8"))["events"]))

    def test_all_modes_replay_without_neural_inference(self):
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker) as mock_run:
            folders = [reprocess(self.cache, mode=mode) for mode in MODES]
            self.assertEqual(mock_run.call_count, 4)
            self.assertEqual(len({folder.name for folder in folders}), 4)
            self.assertTrue(all((folder / "ranking.json").is_file()
                                for folder in folders))
            self.assertEqual((self.cache / "players.json").read_bytes(),
                             self.original_bytes)
            self.assertEqual(reprocess(self.cache, mode="combined"),
                             folders[-1])
            self.assertEqual(mock_run.call_count, 4)
        touch = json.loads((folders[1] / "analysis/events.json").read_text())
        self.assertTrue(any(
            event["status"] == "model_prediction"
            and event["type"] == "unknown_touch"
            for event in touch["events"]))
        self.assertFalse((self.cache / "ranking.json").exists())

    def test_ravel_tracker_explicit_output_path_and_nonempty_rallies(self):
        """Regression: no CSV used to save 334 tracks under players.replay/tracks,
        but replay read tracks/analysis/tracks and silently returned zero.
        """
        from scripts.volleyball_studio.common import save
        def tracker(command, logfile):
            self.assertIn("--tracks_dir", command)
            track_dir = Path(command[command.index("--tracks_dir") + 1])
            self.assertEqual(track_dir.name, "tracks")
            self.assertEqual(track_dir.parent.name, "analysis")
            self.assertTrue(str(command[command.index("--players_json_path") + 1]).endswith("players.json"))
            track_dir.mkdir(parents=True, exist_ok=True)
            self.assertEqual(command.count("--csv_path"), 0)
            save(track_dir / "track_0001.json", {
                "start_frame": 1, "last_frame": 30,
                "positions": [[[i * 10 + 2, 22], i] for i in range(1, 31)],
                "frame_observations": [
                    {"frame": i, "x": i * 10 + 2, "y": 22}
                    for i in range(1, 31)],
                "rally_classification": {
                    "is_rally": True, "rally_confidence": .95},
                "player_interaction": {"contacts": []},
            })
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=tracker):
            folder = reprocess(self.cache, mode="ravel-touch")
        tracks = json.loads((folder / "track-index.json").read_text("utf8"))
        self.assertEqual(len(tracks), 1)
        timeline = json.loads((folder / "analysis/events.json").read_text("utf8"))
        self.assertTrue(any(e["type"] == "rally_start" for e in timeline["events"]))

    def test_touch_is_near_player_and_merged_across_predicted_frames(self):
        track = {
            "start_frame": 1, "last_frame": 100,
            "frame_observations": [
                {"frame": i, "x": 30-5*min(abs(i-12),abs(i-62)), "y": 55}
                for i in range(1, 101)]
        }
        def sample(frame, score, box=None, player=7):
            return {"frame_index": frame, "class_name": "player",
                    "action": "Touch", "action_score": score,
                    "track_id": player,
                    "bbox_xyxy": box or [10, 10, 55, 90]}
        preds = [sample(10, .85), sample(11, .94), sample(12, .90),
                 sample(13, .86), sample(14, .83),
                 sample(11, .80, player=8),  # same physical moment
                 sample(40, .95, box=[400, 400, 460, 500]),
                 sample(60, .82), sample(61, .90), sample(62, .95),
                 sample(63, .88), sample(64, .82)]
        result = _ravel_contacts(track, preds, .5, .35, .35)
        self.assertEqual([p["frame"] for p in result], [12, 62])
        self.assertEqual(result[0]["player_track_id"], 7)
        self.assertAlmostEqual(result[0]["confidence"], .876, places=3)
        self.assertEqual(result[1]["origin"], "ravel_touch")
        geom = [{"frame": 12, "origin": "geometry", "contact_type": "dig"}]
        merged = _merge_contacts(geom, result, .35)
        self.assertEqual(len(merged), 2)
        self.assertEqual(merged[0]["origin"], "geometry")

    def test_touch_validation_settings_are_versioned(self):
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker):
            default = reprocess(self.cache, mode="ravel-touch")
            strict = reprocess(self.cache, mode="ravel-touch",
                               touch_min_interval_sec=.65,
                               touch_max_reach_ratio=.2)
        self.assertNotEqual(default.name, strict.name)
        settings = json.loads((strict / "postprocess.manifest.json").read_text("utf8"))["settings"]
        self.assertEqual(settings["touch_min_interval_sec"], .65)
        self.assertEqual(settings["touch_max_reach_ratio"], .2)
        with self.assertRaisesRegex(ValueError, "Touch interval"):
            reprocess(self.cache, mode="ravel-touch",
                      touch_min_interval_sec=0.01)

    def test_nonrally_flights_do_not_generate_phantom_contacts(self):
        def tracker(command, logfile):
            self.fake_tracker(command, logfile)
            folder = Path(command[command.index("--output_dir") + 1]) / "analysis/tracks"
            save(folder / "track_0002.json", {
                "start_frame": 1, "last_frame": 30,
                "positions": [[[i * 10 + 2, 22], i] for i in range(1, 31)],
                "frame_observations": [
                    {"frame": i, "x": i * 10 + 2, "y": 22} for i in range(1, 31)],
                "rally_classification": {"is_rally": False,
                                         "not_rally_reason": "technical_return"},
                "player_interaction": {"contacts": [{
                    "frame": 15, "ball_x": 152, "ball_y": 22,
                    "confidence": .99, "contact_type": "dig",
                    "player_track_id": 9}]},
            })
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=tracker):
            folder = reprocess(self.cache, mode="combined")
        tracks = json.loads((folder / "analysis/game-tracks.json").read_text("utf8"))
        nonrally = next(t for t in tracks if t["rally_classification"]["is_rally"] is False)
        self.assertEqual(nonrally["player_interaction"]["contacts"], [])

    def test_unsafe_lower_threshold_is_rejected(self):
        with self.assertRaisesRegex(InferenceCacheError, "Touch threshold"):
            reprocess(self.cache, mode="ravel-touch", touch_threshold=.25)
        with self.assertRaisesRegex(InferenceCacheError, "VballNet sidecar"):
            reprocess(self.cache, mode="combined", ball_threshold=.2)

    def test_new_raw_proposals_allow_lower_touch_threshold(self):
        raw = self.cache / "players.raw.v2.jsonl"
        with raw.open("w", encoding="utf-8") as file:
            for frame in range(3, 8):
                file.write(json.dumps({"frame": frame, "predictions": [
                    {"class_name": "player", "track_id": 9,
                     "bbox_xyxy_norm": [.01, .01, .05, .15],
                     "action_scores": {"Touch": .4, "none": .6}},
                    {"class_name": "ball", "score": .25,
                     "bbox_xyxy_norm": [.1, .1, .11, .11]},
                ]}) + "\n")
        manifest_path = self.cache / "inference.manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["outputs"]["players.raw.v2.jsonl"] = {
            "bytes": raw.stat().st_size, "sha256": digest(raw)}
        save(manifest_path, manifest)
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker):
            output = reprocess(self.cache, mode="ravel-touch",
                               touch_threshold=.3, ball_threshold=.2)
        timeline = json.loads((output / "analysis/events.json").read_text())
        self.assertTrue(any(event["status"] == "model_prediction"
                            for event in timeline["events"]))
        self.assertFalse((output / "players.replay.json").is_file())
        self.assertEqual((self.cache / "players.json").read_bytes(),self.original_bytes)

    def test_preview_thresholds_feed_cpu_replay_and_preserve_original(self):
        """Both preview sliders must affect saved tracking inputs, not only overlays."""
        raw_ball = self.cache / "ball/analysis/ball.raw.csv"
        with raw_ball.open("w", newline="", encoding="utf-8") as stream:
            writer = csv.writer(stream)
            writer.writerow(["Frame", "Confidence", "X", "Y", "Radius"])
            writer.writerow([1, .8, 111, 222, 1])
            writer.writerow([2, .6, 333, 444, 1])
        raw_meta = raw_ball.with_suffix(".json")
        save(raw_meta, {"format": "ball-raw-peak-v2", "analysis_threshold": .5})
        raw_players = self.cache / "players.raw.v2.jsonl"
        raw_players.write_text(json.dumps({
            "frame": 5, "predictions": [{
                "class_name": "player", "track_id": 23, "score": .3,
                "bbox_xyxy_norm": [.1, .1, .2, .4],
                "action_scores": {"Touch": .7, "none": .3}
            }]
        })+"\n", encoding="utf-8")
        manifest_file = self.cache / "inference.manifest.json"
        manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
        for path in [raw_ball, raw_meta, raw_players]:
            relative = path.relative_to(self.cache).as_posix()
            manifest["outputs"][relative] = {
                "bytes": path.stat().st_size, "sha256": digest(path)
            }
        save(manifest_file, manifest)
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker) as mock_run:
            version = reprocess(self.cache, mode="combined",
                                ball_threshold=.7, player_threshold=.25)
        command = mock_run.call_args.args[0]
        self.assertAlmostEqual(float(command[command.index("--player_score_threshold")+1]), .25)
        self.assertEqual(Path(command[command.index("--csv_path")+1]),
                         version / "ball/analysis/ball.csv")
        with (version / "ball/analysis/ball.csv").open(encoding="utf-8") as stream:
            rows = list(csv.DictReader(stream))
        self.assertEqual([int(row["Frame"]) for row in rows], [1])
        replay = json.loads((version / "players.replay.json").read_text(encoding="utf-8"))
        # Query 23 was a temporary model slot, not a verified GPU Track ID.
        # Reconstructed CPU tracklets receive distinct stable IDs instead.
        rebuilt_ids = [p["track_id"] for p in replay["predictions"]
                       if p.get("class_name") == "player"]
        self.assertTrue(rebuilt_ids)
        self.assertNotIn(23, rebuilt_ids)
        original = json.loads((self.cache / "players.json").read_text(encoding="utf-8"))
        original_ids = {p.get("track_id") for p in original["predictions"]
                        if p.get("class_name") == "player"}
        self.assertTrue(set(rebuilt_ids) - original_ids,
                        "The weak raw candidate must survive CPU replay with a fresh ID")
        self.assertNotIn(23, original_ids)
        stored = json.loads((version / "postprocess.manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(stored["settings"]["ball_threshold"], .7)
        self.assertEqual(stored["settings"]["player_threshold"], .25)
        with patch("scripts.volleyball_studio.reprocess.run",
                   side_effect=self.fake_tracker):
            higher = reprocess(self.cache, mode="combined", player_threshold=.9)
        self.assertNotEqual(version, higher)
        high_players = json.loads((higher / "players.replay.json").read_text(encoding="utf-8"))
        self.assertFalse([p for p in high_players["predictions"]
                          if p.get("class_name") == "player"])

    def test_corrupted_and_legacy_cache_are_rejected(self):
        (self.cache / "players.json").write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(InferenceCacheError, "Damaged"):
            load(self.cache)
        (self.cache / "inference.manifest.json").unlink()
        with self.assertRaisesRegex(InferenceCacheError, "Old or incomplete"):
            load(self.cache)


if __name__ == "__main__":
    unittest.main()
