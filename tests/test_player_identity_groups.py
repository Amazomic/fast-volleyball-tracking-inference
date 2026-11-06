"""Safety and persistence checks for offline player appearance grouping."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from scripts.volleyball_ui import player_identity_groups as groups


def track(start, end, x=.4, n=40):
    return {
        "first_frame": start, "last_frame": end, "detected_frames": n, "fps": 30,
        "start_center": [x, .5], "end_center": [x, .5],
    }


class PlayerIdentityGroupingTests(unittest.TestCase):
    def test_reid_merges_sequential_identical_tracklets(self):
        features = {"1": np.array([1., 0., 0.]), "2": np.array([1., 0., 0.])}
        tracks = {"1": track(0, 20), "2": track(22, 60)}
        computed = groups.build_groups(features, tracks)
        self.assertEqual(computed["groups"][0]["track_ids"], ["1", "2"])

    def test_never_merges_simultaneous_players_even_if_identical(self):
        features = {"1": np.array([1., 0.]), "2": np.array([1., 0.])}
        tracks = {"1": track(0, 100), "2": track(20, 90)}
        computed = groups.build_groups(features, tracks)
        self.assertFalse(computed["groups"])
        self.assertEqual(computed["unmatched_ids"], ["1", "2"])

    def test_ambiguous_uniforms_are_left_unmatched(self):
        features = {str(i): np.array([1., 0.]) for i in (1, 2, 3)}
        # Two identical-looking alternatives coexist: neither is safe to merge.
        tracks = {"1": track(0, 20), "2": track(24, 45), "3": track(24, 65)}
        computed = groups.build_groups(features, tracks)
        self.assertFalse(computed["groups"])

    def test_confirmed_different_players_cannot_merge(self):
        features = {"1": np.array([1., 0.]), "2": np.array([1., 0.])}
        tracks = {"1": track(0, 20), "2": track(22, 60)}
        computed = groups.build_groups(features, tracks, {"1": "athlete-A", "2": "athlete-B"})
        self.assertEqual(computed["groups"], [])

    def test_without_neural_net_remote_looks_alike_not_merged(self):
        features = {"1": np.array([1., 0.]), "2": np.array([1., 0.])}
        tracks = {"1": track(0, 20), "2": track(500, 550)}
        computed = groups.build_groups(features, tracks, mode="color_fallback")
        self.assertEqual(computed["groups"], [])

    @unittest.skipUnless((groups.MODEL_ROOT / (groups.MODEL_NAME + ".xml")).is_file(),
                         "The optional OpenVINO ReID model is not installed")
    def test_real_reid_model_generates_vectors(self):
        mode, encoder = groups._encoder()
        self.assertEqual(mode, "openvino_reid")
        crops = [np.full((150, 90, 3), (25, 110, 210), np.uint8),
                 np.full((150, 90, 3), (230, 40, 90), np.uint8)]
        vectors = encoder(crops)
        self.assertEqual(vectors.shape, (2, 256))
        self.assertTrue(np.all(np.isfinite(vectors)))

    def test_embeddings_saved_and_reused_per_inference(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "players.json"
            source.write_text('{"predictions":[]}', encoding="utf-8")
            target = root / "groups.json"
            embedding = np.array([1., 0., 0.], dtype=np.float32)
            groups._save_embeddings(source, target, "openvino_reid", {"17": embedding})
            cached = groups._read_embeddings(source, target)
            self.assertIsNotNone(cached)
            self.assertEqual(cached[0], "openvino_reid")
            np.testing.assert_allclose(cached[1]["17"], embedding)
            source.write_text('{"predictions":[1]}', encoding="utf-8")
            self.assertIsNone(groups._read_embeddings(source, target))

    def test_large_player_json_streamed_by_photo_index_and_track_summary(self):
        import json
        from scripts.volleyball_ui import photos, player_tracks
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "players.json"
            payload = {
                "fps": 30, "video": "sample.mp4",
                "source_size": {"width": 640, "height": 480},
                "predictions": [
                    {"class_name": "player", "track_id": 42, "frame_index": frame,
                     "bbox_xyxy": [100.0, 90.0, 180.0, 310.0], "score": .9}
                    for frame in range(12)
                ],
                "unused_large_tail": "x" * (9 * 1024 * 1024),
            }
            path.write_text(json.dumps(payload), encoding="utf-8")
            stamp = {"mtime_ns": path.stat().st_mtime_ns, "size": path.stat().st_size}
            index = photos._build_index(root, stamp)
            self.assertIn("42", index["tracks"])
            summary = player_tracks._compute(str(path), stamp["mtime_ns"], stamp["size"])
            self.assertEqual(summary["tracks"]["42"]["detected_frames"], 12)
            self.assertEqual(summary["fps"], 30)

    def test_one_group_link_assigns_every_id_in_batches(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "players.json"
            source.write_text('{"predictions":[]}', encoding="utf-8")
            target = root / "groups.json"
            ids = [str(i) for i in range(1, 202)]
            groups.data.save_json(target, {
                "schema": groups.VERSION, "stamp": groups._stamp(source),
                "groups": [{"id": "group-1", "track_ids": ids,
                            "similarity": .98, "source": "automatic"}],
                "unmatched_ids": []})
            with patch.object(groups, "_context", return_value=(root, source, target)):
                with patch.object(groups.data, "assign") as assign:
                    result = groups.link_group("project", "video", "cache", "group-1", "player-A")
            self.assertEqual(result["linked"], 201)
            self.assertEqual(result["ids"], ids)
            self.assertEqual(assign.call_count, 2)
            self.assertEqual(assign.call_args_list[0].args[:2], ("video", ids[:200]))
            self.assertEqual(assign.call_args_list[1].args[:2], ("video", ids[200:]))
            self.assertEqual(assign.call_args_list[0].kwargs["player_id"], "player-A")
            self.assertEqual(assign.call_args_list[0].kwargs["cache_id"], "cache")

    def test_split_and_merge_are_project_scoped_and_do_not_assign_person(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "players.json"
            source.write_text('{"predictions":[]}', encoding="utf-8")
            target = root / "groups.json"
            initial = {"schema": groups.VERSION, "stamp": groups._stamp(source),
                       "groups": [{"id": "group-1", "track_ids": ["1", "2", "3"],
                                   "similarity": .99, "source": "automatic"}],
                       "unmatched_ids": ["4"], "suggestions": [], "method": "openvino_reid"}
            groups.data.save_json(target, initial)
            with patch.object(groups, "_context", return_value=(root, source, target)):
                split = groups.modify("project", "video", "cache", "split", ["2"])
                self.assertEqual(split["groups"][0]["track_ids"], ["1", "3"])
                self.assertIn("2", split["unmatched_ids"])
                merged = groups.modify("project", "video", "cache", "merge", ["2", "4"])
                self.assertIn(["2", "4"], [g["track_ids"] for g in merged["groups"]])
                self.assertEqual({g["id"]: g["track_ids"] for g in merged["groups"]}["group-1"], ["1", "3"])


if __name__ == "__main__":
    unittest.main()
