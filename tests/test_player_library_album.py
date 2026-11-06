"""Global player photo gallery, permanent rejection and safe file serving."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_library as library


class PlayerAlbumTests(unittest.TestCase):
    def test_rejected_photo_is_removed_and_not_reenrolled(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            athlete="a"*12
            player={"id":athlete,"name":"Игрок"}
            project,video,track,cache="proj","match.mp4","42","inference"
            filename=library._key(project,video,track,7,cache)+".jpg"
            directory=root/"players"/athlete
            photo=directory/"photos"/filename
            photo.parent.mkdir(parents=True)
            photo.write_bytes(b"sample")
            index=directory/"index.json"
            data.save_json(index,{"schema":1,"photos":[{
                "filename":filename,"project_id":project,"video":video,
                "cache_id":cache,"track_id":track,"frame":7,
            }]})
            with patch.object(data,"RESULTS",root),                  patch.object(data,"global_players",return_value=[player]),                  patch.object(data,"groups_for",return_value=[{"id":athlete,"track_ids":[track]}]),                  patch.object(library,"photos_metadata",return_value={"samples":[{"frame":7,"index":0}]}),                  patch.object(library,"photo_path") as get_source:
                self.assertEqual(library.album_photos(athlete)["count"],1)
                self.assertEqual(library.album_photo_path(athlete,filename),photo)
                with self.assertRaises(ValueError):
                    library.album_photo_path(athlete,"../index.json")
                result=library.remove_album_photo(athlete,filename)
                self.assertEqual(result["count"],0)
                self.assertFalse(photo.exists())
                self.assertIn(filename,data.read_json(index)["rejected"])
                with self.assertRaises(FileNotFoundError):
                    library.album_photo_path(athlete,filename)
                synced=library.sync_photos(project,video,[track],cache)
                self.assertEqual(synced["copied"],0)
                get_source.assert_not_called()
                self.assertEqual(library.album_photos(athlete)["count"],0)

    def test_global_album_is_not_project_scoped(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            athlete="b"*12
            with patch.object(data,"RESULTS",root),                  patch.object(data,"global_players",return_value=[{"id":athlete,"name":"Игрок"}]):
                self.assertEqual(library.album_photos(athlete),{
                    "player_id":athlete,"photos":[],"count":0})


if __name__=="__main__":
    unittest.main()
