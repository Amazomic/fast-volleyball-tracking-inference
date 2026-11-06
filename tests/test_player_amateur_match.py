"""Synthetic continuity and automatic-label safety regression tests."""
from __future__ import annotations
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np

from scripts.volleyball_ui.player_amateur_match import (
    stitch_rallies, auto_assignment_proposals)
from scripts.volleyball_ui.rally_players import player_info


def track(start, end, frames=20):
    return {"first_frame":start,"last_frame":end,
            "detected_frames":frames,"fps":30}


class AmateurIdentityTests(unittest.TestCase):
    def test_best_candidate_wins_even_with_small_similarity_gap(self):
        matches = {"1": {"player_id": "best", "similarity": .96,
                          "runner_up_similarity": .955, "reference_count": 2,
                          "ambiguous": True}}
        result = auto_assignment_proposals(matches, [], {"1": track(0, 60)}, {})
        self.assertEqual(result, {"best": ["1"]})

    def test_equal_best_candidates_require_manual_choice(self):
        matches = {"1": {"player_id": "best", "similarity": .96,
                          "runner_up_similarity": .96, "reference_count": 2}}
        self.assertEqual(auto_assignment_proposals(
            matches, [], {"1": track(0, 60)}, {}), {})

    def test_one_confirmed_reference_can_match_above_threshold(self):
        matches = {"1": {"player_id": "best", "similarity": .96,
                          "runner_up_similarity": .8, "reference_count": 1}}
        self.assertEqual(auto_assignment_proposals(
            matches, [], {"1": track(0, 60)}, {}), {"best": ["1"]})

    def test_auto_binding_requires_strictly_more_than_ninety_percent(self):
        tracks={str(i):track(i*100,i*100+50) for i in range(1,5)}
        matches={str(i):{"player_id":"p1","similarity":score,
                          "runner_up_similarity":.7,"reference_count":3,
                          "ambiguous":False}
                 for i,score in enumerate((.89999,.9,.90001,.95),1)}
        result=auto_assignment_proposals(matches,[],tracks,{},threshold_percent=90)
        self.assertEqual(set(result.get("p1",[])),{"3","4"})

    def test_fragments_across_rallies_yield_five_distinct_people(self):
        tracks = {
            "1":track(0,60),"2":track(0,270),"3":track(0,270),
            "4":track(0,270),"5":track(0,270),"6":track(90,270)}
        features = {
            "1":np.array([1.,0,0,0,0]),
            "2":np.array([0,1.,0,0,0]),
            "3":np.array([0,0,1.,0,0]),
            "4":np.array([0,0,0,1.,0]),
            "5":np.array([0,0,0,0,1.]),
            "6":np.array([.999, .025,0,0,0])}
        out = stitch_rallies(features,tracks,[],threshold_percent=88)
        self.assertEqual(len(out["groups"]),1)
        self.assertEqual(set(out["groups"][0]["track_ids"]),{"1","6"})
        # No fabricated fifth person: six IDs belong to five distinct people.
        self.assertEqual(len(out["groups"])+len(out["unmatched_ids"]),5)

    def test_simultaneously_visible_lookalikes_never_merge(self):
        tracks={"1":track(0,50),"2":track(10,60)}
        features={"1":np.array([1.,0]),"2":np.array([1.,0])}
        out=stitch_rallies(features,tracks,[],threshold_percent=88)
        self.assertEqual(out["groups"],[])
        self.assertEqual(out["unmatched_ids"],["1","2"])

    def test_session_boundary_prevents_merging_across_long_blackout(self):
        tracks={"1":track(0,60),"2":track(150*30,151*30)}
        features={str(tid):np.array([1.,0]) for tid in (1,2)}
        out=stitch_rallies(features,tracks,[],session_gap_sec=120)
        self.assertEqual(len(out["groups"]),0)
        self.assertEqual(out["session_boundaries"],[4500])

    def test_rejected_auto_link_never_reappears_without_manual_confirmation(self):
        from unittest.mock import patch
        from scripts.volleyball_ui import data
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/"identities.json"
            key="match.mp4::abc"
            data.save_json(path,{"schema":2,"videos":{key:[{
                "id":"p1","name":"Анна","track_ids":["10"],
                "link_meta":{"10":{"source":"reid_auto","reviewed":False}}}]}})
            with patch.object(data,"identity_file",return_value=path), \
                 patch.object(data,"video_path",return_value=Path(tmp)), \
                 patch("scripts.volleyball_ui.projects.assert_project_video"), \
                 patch("scripts.volleyball_ui.analysis_runs.cache_for"), \
                 patch.object(data,"global_players",return_value=[{"id":"p1","name":"Анна"}]), \
                 patch.object(data,"attach_player"):
                data.unassign("match.mp4",["10"],project_id="proj",cache_id="abc")
                raw=data.read_json(path,{})
                self.assertEqual(raw["auto_rejected"][key],["10"])
                self.assertEqual(raw["videos"][key],[])
                data.assign("match.mp4",["10"],project_id="proj",
                            player_id="p1",cache_id="abc")
                confirmed=data.read_json(path,{})
                self.assertEqual(confirmed["auto_rejected"][key],[])
                self.assertTrue(confirmed["videos"][key][0]["link_meta"]["10"]["reviewed"])
                self.assertEqual(confirmed["videos"][key][0]["link_meta"]["10"]["source"],"manual")

    def test_manual_conflict_overrides_neural_similarity(self):
        tracks={"1":track(0,30),"2":track(50,90)}
        features={"1":np.array([1.,0]),"2":np.array([1.,0])}
        out=stitch_rallies(features,tracks,[],{"1":"A","2":"B"})
        self.assertEqual(out["groups"],[])

    def test_auto_binding_uses_best_match_threshold_and_no_coexistence(self):
        tracks={str(k):track(*rng) for k,rng in {
            1:(0,60),2:(90,160),3:(100,160),4:(240,300),
            5:(290,340),6:(400,460)}.items()}
        matches={
            "2":{"player_id":"p1","similarity":.985,"runner_up_similarity":.80,
                 "reference_count":3,"ambiguous":False},
            "3":{"player_id":"p1","similarity":.987,"runner_up_similarity":.80,
                 "reference_count":3,"ambiguous":False},
            "4":{"player_id":"p1","similarity":.989,"runner_up_similarity":.80,
                 "reference_count":3,"ambiguous":False},
            "5":{"player_id":"p1","similarity":.990,"runner_up_similarity":.95,
                 "reference_count":3,"ambiguous":False},
            "6":{"player_id":"p2","similarity":.990,"runner_up_similarity":.98,
                 "reference_count":3,"ambiguous":False}
        }
        out=auto_assignment_proposals(matches,[
            {"source":"amateur_reid","track_ids":["2","3"],"similarity":.98}],
            tracks,{"1":"p1"},threshold_percent=97)
        # 2 + 3 simultaneous, so at most one may join p1.
        self.assertEqual(len(set(out.get("p1",[])) & {"2","3"}),1)
        self.assertNotIn("4",out.get("p1",[]))
        self.assertIn("5",out.get("p1",[]))
        self.assertIn("6",out.get("p2",[]))
        self.assertNotIn("1",sum(out.values(),[]))

    def test_observed_four_can_interpolate_five_only_between_seen_frames(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)
            players=cache/"players.json"
            players.write_text("{}",encoding="utf8")
            stat=players.stat()
            frames={str(f):["1","2","3","4"]+(["5"] if f in (15,75) else [])
                    for f in (0,15,30,45,60,75,90)}
            (cache/"player-occupancy.json").write_text(json.dumps({
                "schema":1,"available":True,"mtime_ns":stat.st_mtime_ns,
                "byte_size":stat.st_size,"frames":frames}),encoding="utf8")
            (cache/"player-continuity.json").write_text(json.dumps({
                "schema":2,"track_to_candidate":{str(i):"identity-"+str(i)
                                          for i in range(1,6)}}),encoding="utf8")
            result=player_info(cache,[(0,90)])[0]
            self.assertEqual(result["observed_player_count"],4)
            self.assertEqual(result["player_count"],5)
            self.assertTrue(result["player_count_interpolated"])
            frames["75"]=["1","2","3","4"]
            (cache/"player-occupancy.json").write_text(json.dumps({
                "schema":1,"available":True,"mtime_ns":stat.st_mtime_ns,
                "byte_size":stat.st_size,"frames":frames}),encoding="utf8")
            # index is memoized per player source; changing cached sidecar in
            # place is artificial, so clear only for this isolated test.
            from scripts.volleyball_ui.rally_players import _index
            _index.cache_clear()
            later=player_info(cache,[(0,90)])[0]
            self.assertEqual(later["player_count"],4)
            self.assertFalse(later["player_count_interpolated"])


if __name__=="__main__":
    unittest.main()
