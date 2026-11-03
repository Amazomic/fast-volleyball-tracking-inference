"""Removing a project cancels only its active/queued workloads."""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from unittest.mock import patch

from scripts.volleyball_ui import data, prepared_videos, project_lifecycle, projects, server


class ProjectDeletionTests(unittest.TestCase):
    def setUp(self):
        folder=tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root=Path(folder.name)
        sources=self.root/"project"
        sources.mkdir()
        results=self.root/"results"
        results.mkdir()
        self.original=sources/"match.mp4"
        subprocess.run(
            ["ffmpeg","-nostdin","-y","-hide_banner","-loglevel","error",
             "-f","lavfi","-i","testsrc2=size=320x180:rate=24",
             "-t","1","-c:v","mpeg4",str(self.original)],
            check=True,timeout=20)
        for obj,attr,value in (
            (data,"PROJECT",sources),
            (data,"RESULTS",results),
            (projects,"PROJECTS_DIR",results/"projects"),
            (projects,"REGISTER",results/"projects"/"projects.json"),
        ):
            temporary=patch.object(obj,attr,value)
            temporary.start()
            self.addCleanup(temporary.stop)
        self.target=projects.create_project("To delete",["match.mp4"])
        self.other=projects.create_project("Keep it",["match.mp4"])
        self.before=self.original.read_bytes()

    def _launch_project_group(self, identifier):
        # A real separate process group simulates analysis on another UI port.
        command=[sys.executable,"-c","import time;time.sleep(90)",
                 "--project-id",identifier]
        process=subprocess.Popen(command,stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,
                                 start_new_session=True)
        self.addCleanup(lambda: self._cleanup_process(process))
        return process

    @staticmethod
    def _cleanup_process(process):
        if process.poll() is None:
            try:
                os.killpg(process.pid,signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait(timeout=5)

    def test_archive_stops_foreign_job_group_only_and_keeps_sources(self):
        target=self._launch_project_group(self.target["id"])
        other=self._launch_project_group(self.other["id"])
        deadline=time.monotonic()+4
        while target.pid not in project_lifecycle._project_processes(self.target["id"]):
            if time.monotonic()>deadline:
                self.fail("Target process not discovered")
            time.sleep(.05)
        result=project_lifecycle.archive_with_cancellation(self.target["id"])
        target.wait(timeout=5)
        self.assertNotEqual(target.returncode,0)
        self.assertIsNone(other.poll())
        self.assertTrue(result["archived"])
        self.assertEqual(self.original.read_bytes(),self.before)
        self.assertFalse((projects.PROJECTS_DIR/self.target["id"]).exists())
        self.assertTrue((projects.PROJECTS_DIR/".trash"/self.target["id"]).exists())
        self.assertEqual(projects.get_project(self.other["id"])["name"],"Keep it")

    def test_deletion_cancels_running_ffmpeg_preparation(self):
        # A sleeping subprocess stands in for a slow FFmpeg transcode.
        original_popen=subprocess.Popen
        spawned=[]
        def slow_ffmpeg(_cmd,**kwargs):
            process=original_popen(
                [sys.executable,"-c","import time;time.sleep(90)"],
                **kwargs)
            spawned.append(process)
            return process
        with patch.object(prepared_videos.subprocess,"Popen",side_effect=slow_ffmpeg):
            prepared_videos.queue_project(self.target["id"])
            key=(self.target["id"],"match.mp4",320)
            deadline=time.monotonic()+5
            while key not in prepared_videos._ACTIVE:
                if time.monotonic()>deadline:
                    self.fail("Transcoder did not begin")
                time.sleep(.05)
            result=project_lifecycle.archive_with_cancellation(self.target["id"])
        self.assertTrue(result["archived"])
        self.assertEqual(len(spawned),1)
        spawned[0].wait(timeout=4)
        self.assertNotEqual(spawned[0].returncode,0)
        self.assertEqual(self.original.read_bytes(),self.before)
        self.assertFalse(any(key[0]==self.target["id"]
                             for key in prepared_videos._QUEUED))
        self.assertEqual(projects.get_project(self.other["id"])["name"],"Keep it")

    def test_archive_cancels_queued_preparation_without_touching_other(self):
        original_popen=subprocess.Popen
        spawned=[]
        def slow_ffmpeg(_cmd,**kwargs):
            process=original_popen(
                [sys.executable,"-c","import time;time.sleep(90)"],
                **kwargs)
            spawned.append(process)
            return process
        other_key=(self.other["id"],"match.mp4",320)
        target_key=(self.target["id"],"match.mp4",320)
        marker=projects.PROJECTS_DIR/self.other["id"]/".delete-requested"
        try:
            with patch.object(prepared_videos.subprocess,"Popen",side_effect=slow_ffmpeg):
                prepared_videos.queue_project(self.other["id"])
                deadline=time.monotonic()+5
                while other_key not in prepared_videos._ACTIVE:
                    if time.monotonic()>deadline:
                        self.fail("Other project did not start")
                    time.sleep(.05)
                prepared_videos.queue_project(self.target["id"])
                # The normalized source is shared globally. While another
                # project owns its preparation lock, we must not queue a
                # duplicate transcoder for the same original recording.
                self.assertNotIn(target_key,prepared_videos._QUEUED)
                self.assertIn(other_key,prepared_videos._ACTIVE)
                result=project_lifecycle.archive_with_cancellation(self.target["id"])
                self.assertTrue(result["archived"])
                self.assertNotIn(target_key,prepared_videos._QUEUED)
                self.assertNotIn(target_key,prepared_videos._FUTURES)
                self.assertIsNone(spawned[0].poll())
                self.assertEqual(projects.get_project(self.other["id"])["id"],
                                 self.other["id"])
        finally:
            marker.write_text("cancel only test worker",encoding="utf8")
            prepared_videos.cancel_project(self.other["id"])
            marker.unlink(missing_ok=True)

    def test_job_launch_guard_rejects_project_during_deletion(self):
        marker=projects.PROJECTS_DIR/self.target["id"]/".delete-requested"
        with project_lifecycle.job_launch_guard(self.target["id"]):
            self.assertEqual(projects.get_project(self.other["id"])["id"],
                             self.other["id"])
        marker.write_text("pending",encoding="utf8")
        try:
            with self.assertRaises(ValueError):
                with project_lifecycle.job_launch_guard(self.target["id"]):
                    self.fail("Guard should reject deleting project")
        finally:
            marker.unlink(missing_ok=True)

    def test_archive_http_endpoint_cancels_job_before_archiving(self):
        process=self._launch_project_group(self.target["id"])
        web=ThreadingHTTPServer(("127.0.0.1",0),server.Handler)
        Thread(target=web.serve_forever,daemon=True).start()
        self.addCleanup(web.server_close)
        self.addCleanup(web.shutdown)
        url=f"http://127.0.0.1:{web.server_port}/api/project-archive"
        request=urllib.request.Request(
            url,data=json.dumps({"id":self.target["id"]}).encode(),
            headers={"Content-Type":"application/json"},method="POST")
        with urllib.request.urlopen(request,timeout=20) as response:
            reply=json.load(response)
        self.assertTrue(reply["project"]["archived"])
        process.wait(timeout=5)
        self.assertIsNone(project_lifecycle._project_processes(self.target["id"]).get(process.pid))
        self.assertEqual(projects.get_project(self.other["id"])["id"],self.other["id"])

    def test_tombstone_prevents_new_jobs(self):
        marker=projects.PROJECTS_DIR/self.target["id"]/".delete-requested"
        marker.write_text("delete",encoding="utf8")
        with self.assertRaises(ValueError):
            projects.get_project(self.target["id"])
        with self.assertRaises(ValueError):
            prepared_videos.queue_project(self.target["id"])
        marker.unlink()
        self.assertEqual(projects.get_project(self.target["id"])["id"],
                         self.target["id"])


if __name__=="__main__":
    unittest.main()
