"""Permanent project deletion must not touch shared video cache or source files."""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import project_lifecycle as lifecycle


class PermanentProjectDeleteTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name) / "results" / "projects"
        self.root.mkdir(parents=True)
        self.project_id = "abc123def456"
        self.archived = self.root / ".trash" / self.project_id
        self.archived.mkdir(parents=True)
        (self.archived / "analysis").mkdir()
        (self.archived / "analysis" / "events.json").write_text("{}", encoding="utf8")
        self.shared = self.root.parent / "prepared" / "shared.mp4"
        self.shared.parent.mkdir(parents=True)
        self.shared.write_bytes(b"shared-prepared")
        self.original = Path(temp.name) / "project" / "original.mp4"
        self.original.parent.mkdir(parents=True)
        self.original.write_bytes(b"original-bytes")

    def test_purge_only_project_directory(self):
        with patch.object(lifecycle.projects, "PROJECTS_DIR", self.root), patch.object(
            lifecycle, "archive_with_cancellation",
            return_value={"id": self.project_id, "archived": True},
        ) as archive:
            value = lifecycle.delete_permanently_with_cancellation(self.project_id)
        archive.assert_called_once_with(self.project_id)
        self.assertTrue(value["deleted"])
        self.assertFalse(self.archived.exists())
        self.assertEqual(self.shared.read_bytes(), b"shared-prepared")
        self.assertEqual(self.original.read_bytes(), b"original-bytes")

    def test_never_follows_trash_symlink(self):
        other = self.root / "outside"
        other.mkdir()
        (other / "protected").write_text("keep", encoding="utf8")
        import shutil
        shutil.rmtree(self.archived)
        self.archived.symlink_to(other, target_is_directory=True)
        with patch.object(lifecycle.projects, "PROJECTS_DIR", self.root), patch.object(
            lifecycle, "archive_with_cancellation",
            return_value={"id": self.project_id, "archived": True},
        ):
            with self.assertRaises(RuntimeError):
                lifecycle.delete_permanently_with_cancellation(self.project_id)
        self.assertEqual((other / "protected").read_text(encoding="utf8"), "keep")


if __name__ == "__main__":
    unittest.main()
