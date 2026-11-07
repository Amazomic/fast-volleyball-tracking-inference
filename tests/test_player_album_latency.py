"""Album reads must remain fast during slow background photo enrollment."""
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_library


class AlbumConcurrencyTest(unittest.TestCase):
    def test_album_is_readable_while_background_sync_holds_write_lock(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            player_id = "a" * 12
            photo_dir = root / "players" / player_id / "photos"
            photo_dir.mkdir(parents=True)
            rows = []
            for i in range(100):
                filename = f"{i:024x}.jpg"
                (photo_dir / filename).write_bytes(b"sample")
                rows.append({
                    "filename": filename, "video": "match.mp4",
                    "track_id": i, "frame": i
                })
            manifest = photo_dir.parent / "index.json"
            data.save_json(manifest, {"schema": 1, "photos": rows})
            with patch.object(data, "RESULTS", root), patch.object(
                data, "global_players", return_value=[{"id": player_id, "name": "player"}]
            ):
                ready, release = threading.Event(), threading.Event()

                def writer():
                    with player_library._ALBUM_LOCK:
                        ready.set()
                        release.wait(timeout=4)

                thread = threading.Thread(target=writer, daemon=True)
                thread.start()
                self.assertTrue(ready.wait(timeout=2))
                try:
                    start = time.perf_counter()
                    album = player_library.album_photos(player_id)
                    result = player_library.album_photo_path(player_id, rows[0]["filename"])
                    latency = time.perf_counter() - start
                    self.assertEqual(album["count"], 100)
                    self.assertTrue(result.is_file())
                    self.assertLess(latency, 0.8, "Album reads blocked by photo enrollment")
                finally:
                    release.set()
                    thread.join(timeout=2)

                # A committed manifest update must invalidate cached membership.
                data.save_json(manifest, {"schema": 1, "photos": rows[1:]})
                with self.assertRaises(FileNotFoundError):
                    player_library.album_photo_path(player_id, rows[0]["filename"])
                self.assertEqual(player_library.album_photos(player_id)["count"], 99)


if __name__ == "__main__":
    unittest.main()
