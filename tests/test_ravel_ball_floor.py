import json
import tempfile
import unittest
from pathlib import Path

from src.player_contacts import PlayerStore
from scripts.volleyball_studio.reprocess import _proposal_records
from scripts.volleyball_ui.ravel_ball import _ball_index


class RavelBallFloorTests(unittest.TestCase):
    def test_saved_ball_requires_more_than_half_confidence(self):
        store = PlayerStore(None, ball_score_threshold=.1)
        for score in (.49, .5, .5001):
            item = {"class_name": "ball", "frame_index": 0,
                    "score": score, "bbox_xyxy": [1, 1, 3, 3]}
            self.assertEqual(store._parse_ball(item, 1, 1) is not None,
                             score > .5)

    def test_raw_replay_and_preview_enforce_floor(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            records = [{"class_name": "ball", "score": score,
                        "frame_index": frame, "bbox_xyxy": [1, 1, 3, 3],
                        "bbox_xyxy_norm": [.1, .1, .3, .3]}
                       for frame, score in enumerate((.49, .5, .5001))]
            raw = cache / "players.raw.v2.jsonl"
            raw.write_text("\n".join(json.dumps({"frame": i, "predictions": [p]})
                                     for i, p in enumerate(records)), encoding="utf8")
            _, balls = _proposal_records(cache, {"outputs": {raw.name: {}}},
                                         10, 10, .5, .1)
            self.assertEqual([p["frame_index"] for p in balls], [2])
            saved = cache / "players.json"
            saved.write_text(json.dumps({"fps": 30,
                "source_size": {"width": 10, "height": 10},
                "predictions": records}), encoding="utf8")
            stat = saved.stat()
            self.assertEqual(_ball_index(str(saved), stat.st_mtime_ns,
                                         stat.st_size)[4], [2])
