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
