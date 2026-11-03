"""Browser file metadata resolves only accessible original files."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import file_selection


class BrowserSelectionTests(unittest.TestCase):
    def setUp(self):
        self.folder=tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root=Path(self.folder.name)
        (self.root/"Desktop").mkdir()
        (self.root/"Videos").mkdir()

    def item(self,file):
        stat=file.stat()
        return {"name":file.name,"size":stat.st_size,
                "last_modified":round(stat.st_mtime*1000)}

    def test_desktop_video_resolves_without_copying(self):
        source=self.root/"Desktop"/"match.mp4"
        source.write_bytes(b"faked large source")
        with patch.object(file_selection,"save_selections",
                          side_effect=lambda paths:[{"source_path":paths[0],
                                                     "name":source.name}]) as register:
            result=file_selection.resolve_files([self.item(source)],kind="video",
                roots=[self.root/"Desktop",self.root/"Videos"])
        self.assertEqual(result["videos"][0]["source_path"],str(source.resolve()))
        self.assertEqual(register.call_count,1)

    def test_dragged_file_in_nested_desktop_folder(self):
        parent=self.root/"Desktop"/"Games"/"Beach"
        parent.mkdir(parents=True)
        source=parent/"clip.mov"
        source.write_bytes(b"video")
        with patch.object(file_selection,"save_selections",
                          return_value=[{"name":"clip.mov"}]) as register:
            result=file_selection.resolve_files([self.item(source)],kind="video",
                roots=[self.root/"Desktop"])
        self.assertEqual(result["videos"][0]["name"],"clip.mov")
        register.assert_called_once_with([str(source.resolve())])

    def test_ambiguous_same_name_size_mtime_rejected(self):
        first=self.root/"Desktop"/"match.mp4"
        second=self.root/"Videos"/"match.mp4"
        first.write_bytes(b"same-content")
        second.write_bytes(b"same-content")
        import os
        os.utime(second,ns=(first.stat().st_mtime_ns,first.stat().st_mtime_ns))
        with self.assertRaisesRegex(ValueError,"несколько совпадающих"):
            file_selection.resolve_files([self.item(first)],kind="video",
                roots=[first.parent,second.parent])

    def test_wrong_metadata_not_resolved(self):
        file=self.root/"Desktop"/"match.mp4"
        file.write_bytes(b"real")
        wrong=self.item(file)
        wrong["size"]=999999
        with self.assertRaisesRegex(ValueError,"Не найден локальный оригинал"):
            file_selection.resolve_files([wrong],kind="video",roots=[file.parent])

    def test_audio_metadata_uses_existing_file(self):
        sound=self.root/"Desktop"/"song.mp3"
        sound.write_bytes(b"audio-source")
        with patch.object(file_selection,"inspect_audio",return_value={
             "name":"song.mp3","path":str(sound),"duration":30}) as inspection:
            result=file_selection.resolve_files([self.item(sound)],kind="audio",
                roots=[sound.parent])
        self.assertEqual(result["audio"]["duration"],30)
        inspection.assert_called_once_with(str(sound.resolve()))


if __name__=="__main__":
    unittest.main()
