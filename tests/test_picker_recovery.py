"""Native file dialog lifecycle and recovery of pre-index Quick Reel tasks."""
from __future__ import annotations

import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import local_sources, quick_reel_jobs


class NativePickerRecoveryTests(unittest.TestCase):
    def test_cancelled_picker_releases_slot_and_cannot_overwrite_status(self):
        def wait_until_cancelled(kind, *, cancel_event):
            cancel_event.wait(.25)
            return []

        with (patch.object(local_sources, "_PICKERS", {}),
              patch.object(local_sources, "_open_windows_picker",
                           side_effect=wait_until_cancelled)):
            first = local_sources.begin_picker()["picker_id"]
            with self.assertRaisesRegex(ValueError, "уже открыт"):
                local_sources.begin_picker()
            self.assertTrue(local_sources.cancel_picker(first)["cancelled"])
            self.assertEqual(local_sources.picker_status(first)["status"], "cancelled")
            second = local_sources.begin_picker("audio")["picker_id"]
            self.assertEqual(local_sources.picker_status(second)["status"], "pending")
            local_sources.cancel_picker(second)
            time.sleep(.05)
            self.assertEqual(local_sources.picker_status(first)["status"], "cancelled")

    def test_abandoned_browser_lease_expires_and_next_picker_starts(self):
        def wait_until_cancelled(kind, *, cancel_event):
            cancel_event.wait(.25)
            return []
        with (patch.object(local_sources, "_PICKERS", {}),
              patch.object(local_sources, "_open_windows_picker",
                           side_effect=wait_until_cancelled)):
            first = local_sources.begin_picker()["picker_id"]
            local_sources._PICKERS[first]["last_poll"] -= local_sources._PICKER_IDLE_SEC + 1
            second = local_sources.begin_picker()["picker_id"]
            self.assertNotEqual(first, second)
            self.assertEqual(local_sources.picker_status(first)["status"], "cancelled")
            self.assertTrue(local_sources._PICKERS[first]["cancel_event"].is_set())
            local_sources.cancel_picker(second)

    def test_latest_task_restored_without_pointer(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "quick_reels"
            a, b = "a" * 32, "b" * 32
            for reel in (a, b):
                (root / reel).mkdir(parents=True)
                (root / reel / "request.json").write_text('{"id":"'+reel+'"}')
                time.sleep(.01)
            with patch.object(quick_reel_jobs, "QUICK_ROOT", root):
                self.assertEqual(quick_reel_jobs.latest_reel_id(), b)
                (root / "latest.json").write_text('{"id":"'+a+'"}')
                self.assertEqual(quick_reel_jobs.latest_reel_id(), a)
                (root / a / "request.json").unlink()
                self.assertEqual(quick_reel_jobs.latest_reel_id(), b)


if __name__ == "__main__":
    unittest.main()
