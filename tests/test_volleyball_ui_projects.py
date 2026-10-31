"""Project API, event timeline and global multi-source highlight selection tests."""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, timeline
from scripts import build_project_highlights as builder


class ProjectDataTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.p=self.root/"project";self.p.mkdir()
        self.out=self.root/"results";self.out.mkdir()
        for name in ("one.mp4","two.mp4"):
            (self.p/name).write_bytes(b"video")
        self.patches=[
            patch.object(data,"PROJECT",self.p),patch.object(data,"RESULTS",self.out),
            patch.object(data,"IDS_FILE",self.out/"player_identities.json"),
            patch("scripts.volleyball_ui.preview.video_info",return_value={"duration_sec":120.0}),
            patch.object(projects,"PROJECTS_DIR",self.out/"projects"),
            patch.object(projects,"REGISTER",self.out/"projects"/"projects.json"),
        ]
        for obj in self.patches:obj.start();self.addCleanup(obj.stop)

    def test_create_and_append_two_videos(self):
        first=projects.create_project("Турнир",["one.mp4"])
        self.assertEqual(first["videos"],["one.mp4"])
        second=projects.update_project(first["id"],videos=["one.mp4","two.mp4","one.mp4"])
        self.assertEqual(second["videos"],["one.mp4","two.mp4"])
        self.assertEqual(len(projects.all_projects()),1)
        self.assertEqual(len(projects.project_videos(first["id"])),2)

    def test_project_identities_are_separate(self):
        a=projects.create_project("Проект A",["one.mp4"])
        b=projects.create_project("Проект B",["one.mp4"])
        data.assign("one.mp4",["12","34"],"Игрок",project_id=a["id"])
        self.assertEqual(len(data.groups_for("one.mp4",a["id"])),1)
        self.assertEqual(data.groups_for("one.mp4",b["id"]),[])
        self.assertEqual(data.groups_for("one.mp4"),[])

    def test_create_project_stores_video_ranges(self):
        p=projects.create_project("01.11.2026",["one.mp4","two.mp4"],{
            "one.mp4":{"start":10,"end":45},
            "two.mp4":{"start":20,"end":119}
        })
        self.assertEqual(projects.project_ranges(p["id"])["one.mp4"],{"start":10.0,"end":45.0})
        self.assertEqual(projects.project_ranges(p["id"])["two.mp4"],{"start":20.0,"end":119.0})
        with self.assertRaises(ValueError):
            projects.create_project("bad",["one.mp4"],{"one.mp4":{"start":45,"end":12}})
        with self.assertRaises(ValueError):
            projects.update_project(p["id"],ranges={"two.mp4":{"start":0,"end":121}})

    def test_default_project_uses_whole_video_and_preserves_saved_ranges(self):
        p=projects.create_project("Турнир",["one.mp4","two.mp4"])
        self.assertEqual(projects.project_ranges(p["id"]), {
            "one.mp4":{"start":0.0,"end":120.0},
            "two.mp4":{"start":0.0,"end":120.0}
        })
        projects.update_project(p["id"],ranges={"one.mp4":{"start":10,"end":30}})
        projects.update_project(p["id"],videos=["one.mp4"])
        self.assertEqual(projects.project_ranges(p["id"]),{"one.mp4":{"start":10.0,"end":30.0}})
        projects.update_project(p["id"],videos=["one.mp4","two.mp4"])
        self.assertEqual(projects.project_ranges(p["id"])["two.mp4"],{"start":0.0,"end":120.0})

    def test_duplicate_names_rejected_and_rename_checked(self):
        first=projects.create_project("01.11.2026",["one.mp4"])
        with self.assertRaisesRegex(ValueError,"уже существует"):
            projects.create_project(" 01.11.2026 ",["two.mp4"])
        with self.assertRaisesRegex(ValueError,"уже существует"):
            projects.create_project("01.11.2026",["two.mp4"])
        second=projects.create_project("Второй",["two.mp4"])
        with self.assertRaisesRegex(ValueError,"уже существует"):
            projects.update_project(second["id"],name="01.11.2026")
        projects.update_project(first["id"],name="01.11.2026")
        self.assertEqual(len(projects.all_projects()),2)

    def test_project_cache_does_not_see_other_projects(self):
        first=projects.create_project("A",["one.mp4"])
        second=projects.create_project("B",["one.mp4"])
        path=projects.project_cache_root(first["id"])/"unique"/"analysis"
        path.mkdir(parents=True)
        (path/"events.json").write_text(json.dumps({
            "source":str(self.p/"one.mp4"),"events":[{"type":"attack","frame":4}]
        }),encoding="utf-8")
        self.assertIsNotNone(data.find_cache("one.mp4",first["id"]))
        self.assertIsNone(data.find_cache("one.mp4",second["id"]))
        self.assertFalse(data.report("one.mp4",second["id"])["analyzed"])
        self.assertTrue(data.report("one.mp4",first["id"])["analyzed"])

    def test_rejects_unknown_video(self):
        with self.assertRaises(ValueError):
            projects.create_project("bad",["missing.mp4"])

    def test_timeline_respects_original_source_offset(self):
        cache=self.out/"analysis"/"123"
        (cache/"analysis").mkdir(parents=True)
        (cache/"analysis"/"events.json").write_text(json.dumps({
            "source":str(self.p/"one.mp4"),"fps":30,"analysis_offset_sec":120,
            "events":[
                {"type":"rally_start","track_id":"abc","time_sec":5},
                {"type":"attack","time_sec":6,"id":"x","confidence":.8},
                {"type":"rally_end","track_id":"abc","time_sec":9}
            ]
        }),encoding="utf-8")
        result=timeline.timeline("one.mp4")
        self.assertEqual(result["rallies"][0]["start"],125)
        self.assertEqual(result["rallies"][0]["end"],129)
        self.assertEqual(result["actions"][0]["time"],126)


class MultiSourceHighlightTests(unittest.TestCase):
    def test_all_source_videos_appear_in_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            sources=[root/"one.mp4",root/"two.mp4"]
            for src in sources:src.write_bytes(b"test")
            caches=[root/"cache-a",root/"cache-b"]
            for cache in caches:
                cache.mkdir()
                (cache/"ranking.json").write_text('[]',encoding="utf-8")
            clips={
                "cache-a":[{"ranked":True,"ranking_score":9,"duration_sec":3,"source_start_frame":0,"source_end_frame":89}],
                "cache-b":[{"ranked":True,"ranking_score":8,"duration_sec":4,"source_start_frame":90,"source_end_frame":209}],
            }
            produced=[]
            def mock_render(src,normalized,segments,output_path=None,segments_folder=None):
                produced.append(src.name)
                output_path.write_bytes(b"fake-mp4-data")
            def mock_concat(args,check=False,**kwargs):
                Path(args[-1]).write_bytes(b"merged-mp4-data")
            out=root/"project-output";out.mkdir()
            with patch.object(builder,"project_videos",return_value=sources), \
                 patch.object(builder,"locate_cache",side_effect=caches), \
                 patch.object(builder,"load_groups",side_effect=lambda c,r:clips[c.name]), \
                 patch.object(builder,"render",side_effect=mock_render), \
                 patch.object(builder,"project_dir",return_value=out), \
                 patch.object(builder,"project_output",return_value=out/"highlights.mp4"), \
                 patch.object(builder.subprocess,"run",side_effect=mock_concat), \
                 patch.object(sys,"argv",["script","--project-id","abc","--duration","120"]):
                builder.main()
            manifest=json.loads((out/"highlights_manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(set(produced),{"one.mp4","two.mp4"})
            self.assertEqual(len(manifest["selected_rallies"]),2)
            self.assertTrue((out/"highlights.mp4").is_file())


if __name__=="__main__":
    unittest.main()
