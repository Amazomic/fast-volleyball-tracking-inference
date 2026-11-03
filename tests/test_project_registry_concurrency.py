"""Separate UI server processes cannot overwrite each other's project registry."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKER = r'''
import sys,time
from pathlib import Path
from unittest.mock import patch
from scripts.volleyball_ui import data,projects
source,results,name=sys.argv[1:]
data.PROJECT=Path(source)
data.RESULTS=Path(results)
projects.PROJECTS_DIR=data.RESULTS/"projects"
projects.REGISTER=projects.PROJECTS_DIR/"projects.json"
old=projects._load
def delayed_read():
    value=old()
    time.sleep(.12)
    return value
projects._load=delayed_read
with patch("scripts.volleyball_ui.preview.video_info",
           return_value={"duration_sec":30}):
    result=projects.create_project(name,["shared.mp4"])
print(result["id"],flush=True)
'''


class ProjectRegistryConcurrencyTests(unittest.TestCase):
    def test_creation_responds_with_project_id_if_preparation_queue_fails(self):
        import json
        import urllib.request
        from http.server import ThreadingHTTPServer
        from threading import Thread
        from unittest.mock import patch
        from scripts.volleyball_ui import data, projects, server

        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            source=root/"project"
            source.mkdir()
            (source/"shared.mp4").write_bytes(b"video")
            results=root/"results"
            results.mkdir()
            with (
                patch.object(data,"PROJECT",source),
                patch.object(data,"RESULTS",results),
                patch.object(projects,"PROJECTS_DIR",results/"projects"),
                patch.object(projects,"REGISTER",results/"projects"/"projects.json"),
                patch("scripts.volleyball_ui.preview.video_info",
                      return_value={"duration_sec":30}),
                patch("scripts.volleyball_ui.prepared_videos.queue_project",
                      side_effect=RuntimeError("queue temporarily unavailable")),
            ):
                web=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
                thread=Thread(target=web.serve_forever,daemon=True)
                thread.start()
                try:
                    request=urllib.request.Request(
                        f"http://127.0.0.1:{web.server_port}/api/project-create",
                        method="POST",
                        data=json.dumps({"name":"Survived", "videos":["shared.mp4"]}).encode(),
                        headers={"Content-Type":"application/json"})
                    with urllib.request.urlopen(request,timeout=8) as response:
                        result=json.load(response)
                    self.assertEqual(result["project"]["name"],"Survived")
                    self.assertEqual(len(projects.all_projects()),1)
                    self.assertEqual(result["project"]["id"],projects.all_projects()[0]["id"])
                finally:
                    web.shutdown()
                    web.server_close()

    def test_multiple_server_processes_create_projects_without_lost_updates(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            source=root/"project"
            source.mkdir()
            (source/"shared.mp4").write_bytes(b"fixture")
            results=root/"results"
            results.mkdir()
            names=[f"Server project {i}" for i in range(5)]
            workers=[
                subprocess.Popen(
                    [sys.executable,"-c",WORKER,str(source),str(results),name],
                    cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    text=True, env={**os.environ, "PYTHONPATH":str(ROOT)},
                ) for name in names
            ]
            try:
                for process in workers:
                    stdout,stderr=process.communicate(timeout=30)
                    self.assertEqual(process.returncode,0,stderr)
                    self.assertTrue(stdout.strip(),stderr)
            finally:
                for process in workers:
                    if process.poll() is None:
                        process.kill()
                    process.communicate()
            registry=json.loads((results/"projects"/"projects.json").read_text("utf8"))
            records=registry["projects"]
            self.assertEqual(len(records),len(names),records)
            self.assertEqual({p["name"] for p in records},set(names))
            self.assertEqual(len({p["id"] for p in records}),len(names))
            for record in records:
                project=json.loads((results/"projects"/record["id"]/"project.json").read_text("utf8"))
                self.assertEqual(project,record)


if __name__=="__main__":
    unittest.main()
