"""Project-local court calibration and source-window propagation tests."""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, court
from scripts import run_project_analysis as runner


class ProjectCourtTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root=Path(self.tmp.name)
        source=root/"project";source.mkdir()
        (source/"game.mp4").write_bytes(b"fake")
        results=root/"results";results.mkdir()
        self.mocks=[
            patch.object(data,"PROJECT",source),
            patch.object(data,"RESULTS",results),
            patch.object(projects,"PROJECTS_DIR",results/"projects"),
            patch.object(projects,"REGISTER",results/"projects"/"projects.json"),
            patch("scripts.volleyball_ui.preview.video_info",
                  return_value={"width":1080,"height":1920,"duration_sec":120,"auto_width":1080}),
            patch("scripts.volleyball_ui.court.video_info",
                  return_value={"width":1080,"height":1920,"duration_sec":120,"auto_width":1080}),
        ]
        for m in self.mocks:
            m.start();self.addCleanup(m.stop)
        self.a=projects.create_project("A",["game.mp4"])
        self.b=projects.create_project("B",["game.mp4"])

    @staticmethod
    def sample():
        raw=[(90,1600),(260,450),(810,440),(990,1610),
             (370,1100),(700,1110),(360,940),(740,940)]
        return {
            "frame_width":1080,"frame_height":1920,"frame_sec":23.2,
            "keypoints":[{"id":i,"name":court.NAMES[i],"x":x,"y":y,"visible":True}
                         for i,(x,y) in enumerate(raw)]
        }

    def test_persist_context_and_isolate_other_project(self):
        result=court.save_context(self.a["id"],"game.mp4",
                                  {"start":15,"end":90},court=self.sample())
        self.assertTrue(court.court_path(self.a["id"],"game.mp4").is_file())
        self.assertEqual(result["court"]["frame_index"],696)
        self.assertEqual(result["range"],{"start":15.0,"end":90.0})
        self.assertIsNone(court.get_court(self.b["id"],"game.mp4"))
        self.assertEqual(projects.project_ranges(self.b["id"])["game.mp4"]["start"],0)
        self.assertTrue(court.project_context(self.a["id"])["videos"][0]["court_saved"])
        self.assertFalse(court.project_context(self.b["id"])["videos"][0]["court_saved"])

    def test_save_range_without_overwriting_saved_court(self):
        court.save_context(self.a["id"],"game.mp4",{"start":0,"end":120},court=self.sample())
        court.save_context(self.a["id"],"game.mp4",{"start":20,"end":90})
        self.assertIsNotNone(court.get_court(self.a["id"],"game.mp4"))
        court.save_context(self.a["id"],"game.mp4",{"start":20,"end":90},clear_court=True)
        self.assertIsNone(court.get_court(self.a["id"],"game.mp4"))

    def test_rejects_partial_invalid_and_foreign_annotations(self):
        p=self.sample();p["keypoints"].pop()
        with self.assertRaisesRegex(ValueError,"восемь"):
            court.save_context(self.a["id"],"game.mp4",{"start":10,"end":90},court=p)
        self.assertEqual(projects.project_ranges(self.a["id"])["game.mp4"]["start"],0)
        p=self.sample();p["keypoints"][0]["x"]=999999
        with self.assertRaisesRegex(ValueError,"пределами"):
            court.validate_court(self.a["id"],"game.mp4",p)
        p=self.sample();p["keypoints"][0]["name"]="random"
        with self.assertRaisesRegex(ValueError,"порядок"):
            court.validate_court(self.a["id"],"game.mp4",p)
        with self.assertRaises(ValueError):
            court.save_context(self.a["id"],"other.mp4",{"start":0,"end":40},court=self.sample())

    def test_offscreen_estimated_corner_is_saved(self):
        annotated = self.sample()
        pt = annotated["keypoints"][0]
        pt.update(x=-100, y=1600, status="estimated", visible=False)
        result = court.save_context(self.a["id"],"game.mp4",
                                    {"start":10,"end":100},court=annotated)
        estimated = result["court"]["keypoints"][0]
        self.assertFalse(estimated["visible"])
        self.assertEqual(estimated["status"],"estimated")
        self.assertEqual(estimated["x"],-100.0)
        self.assertTrue(court.get_court(self.a["id"],"game.mp4"))
        self.assertEqual(court.project_context(self.a["id"])["videos"][0]["court_estimated"],1)
        self.assertTrue(court.project_context(self.a["id"])["videos"][0]["court_homography_ready"])
        other = self.sample()
        other["keypoints"][0].update(x=-650,y=1400,status="estimated",visible=False)
        with self.assertRaisesRegex(ValueError,"слишком далеко"):
            court.validate_court(self.a["id"],"game.mp4",other)

    def test_unknown_corner_preserves_id_without_invented_homography(self):
        annotated = self.sample()
        annotated["keypoints"][1].update(x=None,y=None,status="unknown",visible=False)
        result = court.save_context(self.a["id"],"game.mp4",
                                    {"start":10,"end":100},court=annotated)
        self.assertEqual(result["court"]["keypoints"][1]["name"],"2_back_left")
        self.assertIsNone(result["court"]["keypoints"][1]["x"])
        self.assertFalse(court.project_context(self.a["id"])["videos"][0]["court_homography_ready"])
        self.assertEqual(court.project_context(self.a["id"])["videos"][0]["court_unknown"],1)
        import sys
        from pathlib import Path
        src = Path(__file__).resolve().parents[1]/"src"
        if str(src) not in sys.path:
            sys.path.insert(0,str(src))
        from court_transformer import CourtTransformer
        matrix = CourtTransformer(str(court.court_path(self.a["id"],"game.mp4"))).load().matrix
        self.assertIsNone(matrix)

    def test_four_known_corners_can_transform_with_unknown_optional_point(self):
        annotated = self.sample()
        annotated["keypoints"][0].update(x=-100,y=1600,status="estimated",visible=False)
        annotated["keypoints"][5].update(x=None,y=None,status="unknown",visible=False)
        court.save_context(self.a["id"],"game.mp4",{"start":10,"end":100},court=annotated)
        import sys
        from pathlib import Path
        src = Path(__file__).resolve().parents[1]/"src"
        if str(src) not in sys.path:
            sys.path.insert(0,str(src))
        from court_transformer import CourtTransformer
        result = CourtTransformer(str(court.court_path(self.a["id"],"game.mp4"))).load()
        self.assertIsNotNone(result.matrix)
        self.assertEqual(len(result.geometry.keypoints),5)
        self.assertEqual(result.geometry.keypoints[0],(-100.,1600.))

    def test_project_runner_passes_saved_court_and_ranges(self):
        court.save_context(self.a["id"],"game.mp4",{"start":10,"end":40},court=self.sample())
        calls=[]
        def remember(command,**kwargs):
            calls.append(command)
        with patch.object(sys,"argv",["runner","--project-id",self.a["id"]]),\
             patch.object(runner.subprocess,"run",side_effect=remember):
            runner.main()
        command=calls[0]
        self.assertIn("--court-json",command)
        self.assertEqual(command[command.index("--court-json")+1],
                         str(court.court_path(self.a["id"],"game.mp4")))
        self.assertEqual(command[command.index("--analysis-start")+1],"10.0")
        self.assertEqual(command[command.index("--analysis-seconds")+1],"30.0")
        self.assertNotIn("--no-court-fallback",command)
        calls.clear()
        with patch.object(sys,"argv",["runner","--project-id",self.b["id"]]),\
             patch.object(runner.subprocess,"run",side_effect=remember):
            runner.main()
        self.assertIn("--no-court-fallback",calls[0])
        self.assertNotIn("--court-json",calls[0])


if __name__=="__main__":
    unittest.main()
