"""Header run entries: rename/delete metadata must not destroy shared caches."""
import json
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import analysis_runs as runs


class HeaderEntryTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.history = self.root / "runs" / "analysis_timings"
        self.history.mkdir(parents=True)
        self.source_id = "run_20261104_001800_833293"
        self.child_id = self.source_id + "_pp_12abcdef3456"
        (self.history / "20261104_001800_833293.json").write_text(
            json.dumps({
                "status": "complete",
                "started_at": "2026-11-04T01:18:00+00:00",
                "profile_name": "Original GPU profile",
                "videos": [{"video": "sample.mp4", "cache_id": "a"*24,
                            "status": "complete"}],
            }), encoding="utf-8"
        )
        patches = [
            patch.object(runs, "get_project", return_value={"videos": ["sample.mp4"]}),
            patch.object(runs, "project_run_root", return_value=self.root / "runs"),
            patch.object(runs, "project_cache_root", return_value=self.root / "analysis"),
            patch.object(runs, "cache_for", return_value=self.root / "analysis" / ("a"*24)),
            patch.object(runs, "_registry_write_lock", side_effect=lambda: nullcontext()),
            patch.object(runs, "_postprocessing_runs", side_effect=lambda project_id, original: [
                {**original, "id": self.child_id, "parent_run_id": original["id"],
                 "postprocess_settings": {"mode": "combined"}}
            ]),
        ]
        for item in patches:
            item.start()
            self.addCleanup(item.stop)

    def test_rename_primary_is_scoped_and_persistent(self):
        outcome = runs.rename_run("project-demo", self.source_id, "Матч 1 — RAVEL")
        self.assertEqual(outcome["name"], "Матч 1 — RAVEL")
        refreshed = runs.list_runs("project-demo")
        self.assertEqual(next(row for row in refreshed if row["id"] == self.source_id)
                         ["display_name"], "Матч 1 — RAVEL")
        self.assertEqual(next(row for row in refreshed if row["id"] == self.child_id)
                         ["profile"], "Original GPU profile")
        self.assertTrue((self.history / "20261104_001800_833293.json").is_file())

    def test_cpu_default_name_uses_saved_launch_timezone(self):
        with patch.object(runs, "_postprocessing_runs", side_effect=lambda project_id, original: [
            {**original, "id": self.child_id, "parent_run_id": original["id"],
             "started_at": "2026-12-24T22:14:00+03:00",
             "profile": "Different settings / combined",
             "postprocess_settings": {"mode": "combined"}}
        ]):
            child = next(r for r in runs.list_runs("project-demo") if r["id"] == self.child_id)
            self.assertEqual(child["auto_name"], "24.12.26 22:14")
            self.assertNotIn("combined", child["auto_name"])

    def test_rename_and_delete_cpu_version_keep_parent(self):
        runs.rename_run("project-demo", self.child_id, "Касания v2", kind="postprocess")
        self.assertEqual(next(row for row in runs.list_runs("project-demo")
                              if row["id"] == self.child_id)["display_name"], "Касания v2")
        outcome = runs.delete_run("project-demo", self.child_id)
        self.assertTrue(outcome["shared_cache_preserved"])
        self.assertEqual([row["id"] for row in runs.list_runs("project-demo")],
                         [self.source_id])
        with self.assertRaisesRegex(ValueError, "не найдена"):
            runs.delete_run("project-demo", self.child_id)

    def test_repeating_a_deleted_cpu_replay_restores_its_entry(self):
        import hashlib
        import json as json_module
        settings = {"mode": "combined"}
        settings_hash = hashlib.sha256(json_module.dumps(
            settings, ensure_ascii=False, sort_keys=True).encode("utf8")).hexdigest()[:12]
        derived_id = self.source_id + "_pp_" + settings_hash
        # Pretend that the synthetic run identity comes from these exact settings.
        with patch.object(runs, "_postprocessing_runs", side_effect=lambda project_id, parent: [
            {**parent, "id": derived_id, "parent_run_id": parent["id"],
             "postprocess_settings": settings}
        ]):
            runs.delete_run("project-demo", derived_id)
            self.assertEqual(len(runs.list_runs("project-demo")), 1)
            runs.restore_postprocessing_version("project-demo", self.source_id, settings)
            self.assertEqual(len(runs.list_runs("project-demo")), 2)

    def test_delete_primary_hides_dependent_versions(self):
        answer = runs.delete_run("project-demo", self.source_id)
        self.assertTrue(answer["child_versions_hidden"])
        self.assertEqual(runs.list_runs("project-demo"), [])
        self.assertTrue((self.history / "20261104_001800_833293.json").exists())
        self.assertTrue((self.root / "runs" / "header_entries.json").is_file())

    def test_cpu_scope_rejects_parent_and_keeps_all_branches(self):
        original_ids={run["id"] for run in runs.list_runs("project-demo")}
        with self.assertRaisesRegex(ValueError, "первичный анализ"):
            runs.delete_run("project-demo", self.source_id, kind="postprocess")
        with self.assertRaisesRegex(ValueError, "первичный анализ"):
            runs.rename_run("project-demo", self.source_id, "Bad", kind="postprocess")
        with self.assertRaisesRegex(ValueError, "CPU-версию"):
            runs.delete_run("project-demo", self.child_id, kind="inference")
        self.assertEqual({run["id"] for run in runs.list_runs("project-demo")},
                         original_ids)
        self.assertFalse((self.root / "runs" / "header_entries.json").exists(),
                         "Rejected mutations must not write tombstones")

    def test_http_rejects_cpu_delete_of_parent_but_accepts_child(self):
        import urllib.request
        import urllib.error
        import threading
        from http.server import ThreadingHTTPServer
        from scripts.volleyball_ui.server import Handler

        http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        worker = threading.Thread(target=http.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(worker.join, 3)
        self.addCleanup(http.server_close)
        self.addCleanup(http.shutdown)
        base = f"http://127.0.0.1:{http.server_port}"

        def request(kind, run_id):
            body = json.dumps({"project_id": "project-demo",
                               "run_id": run_id, "kind": kind}).encode("utf8")
            return urllib.request.Request(
                base + "/api/analysis-run/delete",
                data=body, method="POST",
                headers={"Content-Type": "application/json"})

        with self.assertRaises(urllib.error.HTTPError) as rejected:
            urllib.request.urlopen(request("postprocess", self.source_id), timeout=5)
        self.assertEqual(rejected.exception.code, 400)
        self.assertEqual(len(runs.list_runs("project-demo")), 2)
        with urllib.request.urlopen(
                request("postprocess", self.child_id), timeout=5) as response:
            self.assertEqual(response.status, 200)
            self.assertTrue(json.load(response)["deleted"])
        self.assertEqual([row["id"] for row in runs.list_runs("project-demo")],
                         [self.source_id])

    def test_cpu_scope_deletes_only_selected_version(self):
        answer=runs.delete_run("project-demo", self.child_id, kind="postprocess")
        self.assertTrue(answer["deleted"])
        self.assertFalse(answer["child_versions_hidden"])
        self.assertEqual([run["id"] for run in runs.list_runs("project-demo")],
                         [self.source_id])
        self.assertTrue((self.history / "20261104_001800_833293.json").exists())

    def test_primary_entry_edit_skips_cpu_directory_scan(self):
        # Removing an inference branch is metadata-only: no CPU cache traversal.
        with patch.object(runs, "_postprocessing_runs", side_effect=AssertionError(
                "Primary entry change should not scan CPU versions")):
            runs.rename_run("project-demo", self.source_id, "Renamed", kind="inference")
            result = runs.delete_run("project-demo", self.source_id,
                                     kind="inference")
        self.assertTrue(result["deleted"])

    def test_cpu_entry_edit_uses_only_parent_history_file(self):
        # Unrelated timing records must not be opened by a single CPU deletion.
        (self.history / "unrelated.json").write_text("{}", encoding="utf-8")
        with patch.object(runs, "cache_for", wraps=runs.cache_for) as cache_lookup:
            runs.delete_run("project-demo", self.child_id, kind="postprocess")
        self.assertEqual(cache_lookup.call_count, 1)

    def test_invalid_ids_and_names_are_rejected(self):
        with self.assertRaises(ValueError):
            runs.delete_run("project-demo", "../../outside")
        with self.assertRaises(ValueError):
            runs.rename_run("project-demo", self.source_id, " ")
        with self.assertRaises(ValueError):
            runs.rename_run("project-demo", self.source_id, "x"*101)

    def test_http_cpu_delete_during_active_gpu_analysis(self):
        """A new GPU inference must not block deleting older CPU versions."""
        import urllib.request
        import threading
        from unittest.mock import Mock
        from http.server import ThreadingHTTPServer
        from scripts.volleyball_ui import server

        http = ThreadingHTTPServer(("127.0.0.1", 0), server.Handler)
        worker = threading.Thread(target=http.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(worker.join, 3)
        self.addCleanup(http.server_close)
        self.addCleanup(http.shutdown)
        body = json.dumps({"project_id": "project-demo",
                           "run_id": self.child_id,
                           "kind": "postprocess"}).encode("utf8")
        request = urllib.request.Request(
            f"http://127.0.0.1:{http.server_port}/api/analysis-run/delete",
            data=body, method="POST",
            headers={"Content-Type": "application/json"})
        with patch.dict(server.JOB, {"process": Mock(poll=lambda: None),
                                     "project_id": "project-demo",
                                     "kind": "analyze", "run_id": None}):
            with urllib.request.urlopen(request, timeout=5) as response:
                self.assertTrue(json.load(response)["deleted"])
        self.assertEqual([run["id"] for run in runs.list_runs("project-demo")],
                         [self.source_id])

    def test_http_delete_guards_only_active_run(self):
        import urllib.request
        import urllib.error
        import threading
        from unittest.mock import Mock
        from http.server import ThreadingHTTPServer
        from scripts.volleyball_ui import server

        http = ThreadingHTTPServer(("127.0.0.1", 0), server.Handler)
        worker = threading.Thread(target=http.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(worker.join, 3)
        self.addCleanup(http.server_close)
        self.addCleanup(http.shutdown)

        def request(kind, run_id):
            body = json.dumps({"project_id": "project-demo",
                               "run_id": run_id, "kind": kind}).encode("utf8")
            return urllib.request.Request(
                f"http://127.0.0.1:{http.server_port}/api/analysis-run/delete",
                data=body, method="POST",
                headers={"Content-Type": "application/json"})

        with patch.dict(server.JOB, {"process": Mock(poll=lambda: None),
                                     "project_id": "project-demo",
                                     "kind": "highlights",
                                     "run_id": self.child_id}):
            with self.assertRaises(urllib.error.HTTPError) as exact:
                urllib.request.urlopen(request("postprocess", self.child_id), timeout=5)
            self.assertEqual(exact.exception.code, 400)
            with self.assertRaises(urllib.error.HTTPError) as parent:
                urllib.request.urlopen(request("inference", self.source_id), timeout=5)
            self.assertEqual(parent.exception.code, 400)
        self.assertEqual(len(runs.list_runs("project-demo")), 2)


if __name__ == "__main__":
    unittest.main()
