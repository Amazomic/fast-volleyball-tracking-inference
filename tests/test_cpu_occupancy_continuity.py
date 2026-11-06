import json
import tempfile
import unittest
from pathlib import Path

from scripts.volleyball_ui.rally_players import player_info


class CpuOccupancyContinuityTests(unittest.TestCase):
    def test_selected_cpu_continuity_fills_only_bounded_observation_gaps(self):
        with tempfile.TemporaryDirectory() as tmp:
            base=Path(tmp)
            cache=base/"postprocessing"/"selected"
            cache.mkdir(parents=True)
            rows=[]
            for frame in (0,15,30,45,60,75,90):
                ids=range(1,6) if frame in (15,75) else range(1,5)
                rows.extend({"class_name":"player","frame_index":frame,
                             "track_id":tid,"score":.9} for tid in ids)
            (cache/"players.replay.json").write_text(json.dumps({"predictions":rows}))
            (cache/"player-continuity.json").write_text(json.dumps({
                "schema":"volley-player-continuity/1",
                "track_to_candidate":{str(i):f"candidate-{i}" for i in range(1,6)}}))
            (base/"player-continuity.json").write_text(json.dumps({
                "schema":2,"track_to_candidate":{}}))
            result=player_info(cache,[(0,90)])[0]
            self.assertEqual(result["observed_player_count"],4)
            self.assertEqual(result["player_count"],5)
            self.assertTrue(result["player_count_interpolated"])


if __name__=="__main__":
    unittest.main()
