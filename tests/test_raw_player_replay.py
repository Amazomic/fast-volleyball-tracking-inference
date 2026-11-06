"""One-shot weak RAVEL candidates and stable CPU-only track association."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import numpy as np

from src.inference_player_ball_openvino import decode_batch
from scripts.volleyball_studio.replay_player_tracking import reconstruct_players


class RawReplayTests(unittest.TestCase):
    def test_raw_decoder_keeps_weak_overlapping_candidates_before_nms(self):
        scores=np.array([.9,.035],np.float64)
        logits=np.stack([np.log(1-scores),np.log(scores)],axis=-1)
        outputs={
            "logits":logits[None,None].astype("float32"),
            "boxes":np.array([[[[.5,.5,.3,.6],[.51,.5,.3,.6]]]],dtype="float32"),
            "action_logits":np.log(np.array([[[[.72,.28],[.45,.55]]]],dtype="float32")),
        }
        normal=decode_batch(outputs,.02,0,0,
                            action_names=["Touch","none"])[0]
        saved=decode_batch(outputs,.02,0,0,
                           action_names=["Touch","none"],
                           preserve_player_candidates=True,max_players=32)[0]
        players=lambda rows:[row for row in rows if row.get("class_name")=="player"]
        self.assertEqual(len(players(normal)),1)
        self.assertEqual(len(players(saved)),2)
        self.assertAlmostEqual(players(saved)[1]["score"],.035,places=3)
        self.assertIn("Touch",players(saved)[1]["action_scores"])

    def test_cpu_rebuild_preserves_verified_gpu_id_across_weak_gap(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)
            original=[]
            for frame in [0,1,2,3]:
                if frame!=1:
                    original.append({
                        "class_name":"player","track_id":11,
                        "frame_index":frame,"interpolated":False,
                        "bbox_xyxy":[100+frame,50,200+frame,300]})
            rows=[]
            for frame in range(4):
                rows.append({"frame":frame,"predictions":[{
                    "class_name":"player","score":.12 if frame==1 else .9,
                    "track_id":80+frame,
                    "bbox_xyxy_norm":[(100+frame)/1000,.05,
                                       (200+frame)/1000,.30]}]})
            (cache/"players.raw.v2.jsonl").write_text(
                "".join(json.dumps(row)+"\n" for row in rows),encoding="utf8")
            result=reconstruct_players(cache,1000,1000,.30,original)
            self.assertEqual([r["frame_index"] for r in result],[0,1,2,3])
            self.assertEqual({r["track_id"] for r in result},{11})
            self.assertEqual([round(r["score"],2) for r in result],[.9,.12,.9,.9])
            self.assertTrue(all("interpolated" not in r for r in result))
            # Source query IDs 80..83 never become real athlete identities.

    def test_cpu_hold_duration_is_configurable_without_gpu(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)
            samples=[]
            for frame in (0,10):
                samples.append({"frame":frame,"predictions":[{
                    "class_name":"player","score":.85,"track_id":frame+23,
                    "bbox_xyxy_norm":[.2,.2,.4,.7]}]})
            (cache/"players.raw.v2.jsonl").write_text(
                "".join(json.dumps(x)+"\n" for x in samples),encoding="utf8")
            short=reconstruct_players(cache,1000,1000,.30,[],
                hold_frames=2,max_center_distance=.14)
            long=reconstruct_players(cache,1000,1000,.30,[],
                hold_frames=15,max_center_distance=.14)
            self.assertEqual(len({p["track_id"] for p in short}),2)
            self.assertEqual(len({p["track_id"] for p in long}),1)

    def test_new_identity_gets_unique_cpu_id_not_original_track(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)
            samples=[]
            for frame in range(3):
                samples.append({"frame":frame,"predictions":[
                    {"class_name":"player","score":.8,"track_id":100,
                     "bbox_xyxy_norm":[.1,.1,.2,.4]},
                    {"class_name":"player","score":.6,"track_id":100,
                     "bbox_xyxy_norm":[.7,.2,.8,.55]}]})
            (cache/"players.raw.v2.jsonl").write_text(
                "".join(json.dumps(s)+"\n" for s in samples),encoding="utf8")
            old=[{"class_name":"player","track_id":7,"frame_index":f,
                  "bbox_xyxy":[100,100,200,400]} for f in range(3)]
            result=reconstruct_players(cache,1000,1000,.3,old)
            self.assertEqual({r["track_id"] for r in result},{7,8})
            per_frame={}
            for item in result:
                per_frame.setdefault(item["frame_index"],set()).add(item["track_id"])
            self.assertTrue(all(identifiers=={7,8} for identifiers in per_frame.values()))


if __name__=="__main__":
    unittest.main()
