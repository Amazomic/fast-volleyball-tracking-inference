"""Safe CPU-only provisional player re-tracking from saved RAVEL raw boxes."""
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np
from scripts.volleyball_ui.player_retracking import propose_retracked_groups


class PlayerRetrackingTests(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.cache=Path(temp.name)
        original=[]
        raw=[]
        for frame in range(112):
            boxes=[]
            for oid, x in [
                (1, .20) if frame<10 else (2,.21) if frame>=100 else (None,None),
                (3,.75),
            ]:
                if oid is None:
                    continue
                xmin=x-.03;xmax=x+.03
                box=[xmin,.3,xmax,.5]
                original.append({
                    "class_name":"player","track_id":oid,
                    "bbox_xyxy":[b*400 if idx%2==0 else b*600
                                 for idx,b in enumerate(box)],
                    "frame_index":frame,"score":.85})
                boxes.append([.85,*box])
            raw.append(json.dumps([frame,boxes]))
        (self.cache/"players.json").write_text(json.dumps({
            "fps":30,"source_size":{"width":400,"height":600},
            "predictions":original}),encoding="utf8")
        (self.cache/"players.raw.jsonl").write_text(
            "\n".join(raw)+"\n",encoding="utf8")
        self.features={
            "1":np.array([1.,0.],dtype=np.float32),
            "2":np.array([.99,.01],dtype=np.float32),
            "3":np.array([0.,1.],dtype=np.float32),
        }

    def test_long_gap_is_merged_only_with_reid_evidence(self):
        result=propose_retracked_groups(self.cache,self.features)
        self.assertEqual(result["source_track_ids"],3)
        self.assertEqual(result["retracked_ids"],3)
        self.assertEqual(result["candidate_people"],2)
        self.assertEqual(result["unmatched_ids"],["3"])
        self.assertEqual([g["track_ids"] for g in result["groups"]],[["1","2"]])
        self.assertGreaterEqual(result["groups"][0]["similarity"],.98)
        self.assertFalse(result["confirmed"])
        self.assertFalse((self.cache/"player_identities.json").exists())

    def test_similar_appearance_does_not_merge_different_visible_people(self):
        self.features["3"]=np.array([.999,.001],dtype=np.float32)
        result=propose_retracked_groups(self.cache,self.features)
        self.assertEqual(len(result["groups"]),1)
        self.assertEqual(result["groups"][0]["track_ids"],["1","2"])
        self.assertIn("3",result["unmatched_ids"])

    def test_higher_similarity_threshold_rejects_uncertain_reid(self):
        self.features["2"]=np.array([.85,.527],dtype=np.float32)
        result=propose_retracked_groups(self.cache,self.features,
                                          threshold_percent=90)
        self.assertFalse(result["groups"])
        self.assertEqual(result["candidate_people"],3)

    def test_conflicting_simultaneous_players_are_not_merged(self):
        path=self.cache/"players.json"
        data=json.loads(path.read_text(encoding="utf8"))
        # Pretend an ID was erroneously reused on a different visible player.
        # The same two original IDs then overlap at very different positions,
        # which must veto even a very strong appearance-only proposal.
        data["predictions"].append({
            "class_name":"player","track_id":2,"frame_index":5,
            "bbox_xyxy":[.70*400,.3*600,.8*400,.5*600],
            "score":.91})
        path.write_text(json.dumps(data),encoding="utf8")
        result=propose_retracked_groups(self.cache,self.features)
        self.assertFalse(any(set(g["track_ids"])=={"1","2"}
                             for g in result["groups"]))
        self.assertGreaterEqual(result["candidate_people"],3)

    def test_missing_raw_proposals_never_invokes_gpu(self):
        (self.cache/"players.raw.jsonl").unlink()
        self.assertIsNone(propose_retracked_groups(self.cache,self.features))


if __name__ == "__main__":
    unittest.main()
