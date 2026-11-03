"""The quick-reel HTTP launcher never creates or consults a regular project."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import server, quick_reel_jobs
from scripts.volleyball_ui.quick_reel_selection import save_selections


class FakeProcess:
    pid = 12345
    returncode = None

    def poll(self):
        return None


class QuickReelLaunchTests(unittest.TestCase):
    def test_standalone_job_ignores_project_id(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            source = base/"match.mp4"
            source.write_bytes(b"source-video")
            music = base/"music.mp3"
            music.write_bytes(b"track")
            with patch.object(quick_reel_jobs,"QUICK_ROOT",base/"quick_reels"):
                token = save_selections([str(source)])[0]["id"]
                state = dict(server.JOB,process=None,kind=None,log=None,
                             progress_event=None,progress_offset=0)
                with (patch.object(server,"JOB",state),
                      patch.object(server.subprocess,"Popen",return_value=FakeProcess()) as popen,
                      patch("scripts.volleyball_ui.quick_reel_audio.inspect_audio",
                            return_value={"path":str(music),"duration":20.0}),
                      patch.object(server,"project_videos",
                                   side_effect=AssertionError("Quick reel tried to read projects")),
                      patch.object(server,"create_project",
                                   side_effect=AssertionError("Quick reel created a project"))):
                    job = server.start_job("quick_reel",{
                        "videos":[token], "audio_path":str(music),
                        "project_id":"invalid_should_be_ignored"})
                self.assertEqual(job["kind"],"quick_reel")
                self.assertEqual(job["status"],"running")
                self.assertIsNone(job["project_id"])
                self.assertEqual(len(job["reel_id"]),32)
                self.assertEqual(job["quick_request_id"],job["reel_id"])
                folder = quick_reel_jobs.quick_dir(job["reel_id"])
                self.assertTrue((folder/"request.json").is_file())
                self.assertTrue((folder/"status.json").is_file())
                self.assertTrue((folder/"logs").is_dir())
                self.assertFalse((folder/"project.json").exists())
                self.assertIn("--id",popen.call_args.args[0])
                self.assertNotIn("--project-id",popen.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
