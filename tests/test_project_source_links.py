"""Project picker survives missing files; recorded source links can be restored."""
import json
import tempfile
import unittest
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, server


class SourceLinkPersistenceTests(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root=Path(temp.name)
        self.sources=self.root/"originals"
        self.sources.mkdir()
        self.links=self.root/"project"
        self.links.mkdir()
        self.results=self.root/"results"
        self.results.mkdir()
        self.a_source=self.sources/"a.mp4"
        self.b_source=self.sources/"b.mp4"
        for p in (self.a_source,self.b_source):
            p.write_bytes(p.name.encode())
            (self.links/p.name).symlink_to(p)
        for module,key,value in (
            (data,"PROJECT",self.links),
            (data,"RESULTS",self.results),
            (projects,"PROJECTS_DIR",self.results/"projects"),
            (projects,"REGISTER",self.results/"projects"/"projects.json"),
        ):
            mock=patch.object(module,key,value)
            mock.start()
            self.addCleanup(mock.stop)
        fake=patch("scripts.volleyball_ui.preview.video_info",
                   return_value={"duration_sec":30})
        fake.start()
        self.addCleanup(fake.stop)
        self.a=projects.create_project("Match A",["a.mp4"])
        self.b=projects.create_project("Match B",["b.mp4"])

    def test_source_provenance_saved_in_registry_and_project_folder(self):
        for project,source in ((self.a,self.a_source),(self.b,self.b_source)):
            self.assertEqual(project["source_paths"][source.name],str(source.resolve()))
            disk=data.read_json(projects.PROJECTS_DIR/project["id"]/"project.json")
            self.assertEqual(disk,project)

    def test_deleted_symlink_restored_only_from_saved_original(self):
        link=self.links/"a.mp4"
        link.unlink()
        self.assertFalse(link.exists())
        report=projects.project_summary(self.a["id"])
        self.assertEqual(report["missing_videos"],[])
        self.assertTrue(link.is_symlink())
        self.assertEqual(link.resolve(),self.a_source)
        self.assertEqual(self.a_source.read_bytes(),b"a.mp4")

    def test_missing_original_does_not_break_project_picker_or_other_project(self):
        (self.links/"a.mp4").unlink()
        self.a_source.unlink()
        web=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        thread=Thread(target=web.serve_forever,daemon=True)
        thread.start()
        self.addCleanup(web.server_close)
        self.addCleanup(web.shutdown)
        url=f"http://127.0.0.1:{web.server_port}/api/projects"
        with urllib.request.urlopen(url,timeout=8) as response:
            records=json.load(response)["projects"]
        self.assertEqual(len(records),2)
        by_id={row["id"]:row for row in records}
        self.assertEqual(by_id[self.a["id"]]["missing_videos"],["a.mp4"])
        self.assertEqual(by_id[self.b["id"]]["missing_videos"],[])
        self.assertFalse(by_id[self.a["id"]]["analyses"][0]["available"])
        self.assertTrue(by_id[self.b["id"]]["analyses"][0]["available"])
        self.assertEqual(self.b_source.read_bytes(),b"b.mp4")
        self.assertFalse((self.links/"a.mp4").is_symlink())
        report_url=f"http://127.0.0.1:{web.server_port}/api/project-report?id={self.a['id']}"
        with urllib.request.urlopen(report_url,timeout=8) as response:
            report=json.load(response)
        self.assertFalse(report["analyzed"])
        self.assertEqual(report["total_videos"],1)
        self.assertEqual(report["sources"][0]["name"],"a.mp4")

    def test_existing_wrong_link_is_never_overwritten(self):
        alias=self.links/"a.mp4"
        alias.unlink()
        alias.symlink_to(self.b_source)
        report=projects.project_summary(self.a["id"])
        self.assertEqual(report["missing_videos"],["a.mp4"])
        self.assertEqual(alias.resolve(),self.b_source)


if __name__=="__main__":
    unittest.main()
