"""Player counts per rally must use concurrent detections and preserve track IDs."""
from pathlib import Path
import json
import tempfile
import unittest

from scripts.volleyball_ui.rally_players import player_info
from scripts.volleyball_ui.rally_view import from_tracks
from scripts.full_rallies import load_groups


class PlayerCountTests(unittest.TestCase):
    def setUp(self):
        directory=tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.cache=Path(directory.name)
        (self.cache/"analysis").mkdir()
        rows=[]
        for frame in (15,30,45,60):
            for track in (101,102,103):
                rows.append({"class_name":"player","frame_index":frame,
                             "track_id":track,"score":.94})
        # Two players during the second rally, after an unrelated identity reset.
        for frame in (150,165,180,195):
            for track in (201,202):
                rows.append({"class_name":"player","frame_index":frame,
                             "track_id":track,"score":.92})
        # Interpolated and low-confidence candidates must not be counted.
        rows.extend([
            {"class_name":"player","frame_index":30,"track_id":999,
             "score":.99,"interpolated":True},
            {"class_name":"player","frame_index":30,"track_id":998,"score":.05},
            {"class_name":"ball","frame_index":30,"track_id":1,"score":.99},
        ])
        (self.cache/"players.json").write_text(
            json.dumps({"predictions":rows}),encoding="utf-8")
        tracks=[]
        for i,(start,end) in enumerate(((0,90),(135,225)),1):
            tracks.append({"track_id":i,"start_frame":start,"last_frame":end,
              "rally_classification":{"label":"rally","rally_confidence":.91},
              "positions":[]})
        (self.cache/"analysis/game-tracks.json").write_text(json.dumps(tracks))
        (self.cache/"analysis/events.json").write_text('{"events":[]}')
        self.ranking=[{"start":0,"end":90,"score":3,"confidence":.9},
                      {"start":135,"end":225,"score":2,"confidence":.9}]

    def test_concurrent_count_and_links_for_both_rallies(self):
        results=player_info(self.cache,[(0,90),(135,225)])
        self.assertEqual([x["player_count"] for x in results],[3,2])
        self.assertEqual(results[0]["player_track_ids"],["101","102","103"])
        self.assertEqual(results[0]["linked_player_ids"],[])
        self.assertEqual(results[1]["player_track_ids"],["201","202"])
        self.assertTrue((self.cache/"player-occupancy.json").is_file())
        self.assertEqual(results,player_info(self.cache,[(0,90),(135,225)]))

    def test_preview_and_montage_share_player_metadata(self):
        shown=from_tracks(self.cache,offset=0,fps=30,duration=10,
                          gap_sec=0,min_sec=1,context_sec=0)
        exported=load_groups(self.cache,self.ranking,merge_gap=0,
                             context_padding=0,min_sec=1,duration_frames=300)
        self.assertEqual([r["player_count"] for r in shown["rallies"]],[3,2])
        self.assertEqual([r["player_count"] for r in exported],[3,2])
        for a,b in zip(shown["rallies"],exported):
            self.assertEqual(a["player_track_ids"],b["player_track_ids"])

    def test_absent_detections_are_unknown_not_zero(self):
        (self.cache/"players.json").unlink()
        self.assertEqual(player_info(self.cache,[(0,90)])[0]["player_count"],None)


if __name__=="__main__":
    unittest.main()
