import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_library as library


class PlayerPhotoSaveTests(unittest.TestCase):
    def test_only_confirmed_tracks_become_reference_photos(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            image = root / "source.jpg"
            image.write_bytes(b"sample")
            groups = [{"id": "athlete", "track_ids": ["1", "2"],
                       "link_meta": {"2": {"source": "reid_auto", "reviewed": False}}}]
            with patch.object(data, "RESULTS", root), patch.object(
                data, "global_players", return_value=[{"id": "athlete"}]
            ), patch.object(data, "groups_for", return_value=groups), patch.object(
                library, "photos_metadata", return_value={"samples": [{"frame": 7, "index": 0}]}
            ), patch.object(library, "photo_path", return_value=image):
                result = library.sync_photos("project", "match.mp4", ["1", "2"], "cache")
                self.assertEqual(result["copied"], 1)
                album = library.album_photos("athlete")
                self.assertEqual([row["track_id"] for row in album["photos"]], ["1"])

    def test_save_is_scoped_to_selected_player_and_preserves_rejections(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            image = root / "source.jpg"
            image.write_bytes(b"sample")
            index = root / "players" / "athlete" / "index.json"
            rejected = library._key("project", "match.mp4", "1", 7, "cache") + ".jpg"
            data.save_json(index, {"schema": 1, "photos": [], "rejected": [rejected]})
            groups = [{"id": "athlete", "track_ids": ["1", "2"]},
                      {"id": "other", "track_ids": ["3"]}]
            with patch.object(data, "RESULTS", root), patch.object(
                data, "global_players", return_value=[{"id": "athlete"}, {"id": "other"}]
            ), patch.object(data, "groups_for", return_value=groups), patch.object(
                library, "photos_metadata", return_value={"samples": [{"frame": 7, "index": 0}]}
            ), patch.object(library, "photo_path", return_value=image):
                result = library.save_player_photos("project", "match.mp4", "athlete", "cache")
                self.assertEqual(result["copied"], 1)
                self.assertEqual(result["count"], 1)
                self.assertEqual(library.album_photos("other")["count"], 0)
                self.assertEqual(library.save_player_photos(
                    "project", "match.mp4", "athlete", "cache")["copied"], 0)
                self.assertIn(rejected, data.read_json(index)["rejected"])


if __name__ == "__main__":
    unittest.main()
