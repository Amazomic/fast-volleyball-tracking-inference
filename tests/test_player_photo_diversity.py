import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, player_library as library


class PhotoDiversityTests(unittest.TestCase):
    def test_conservative_similarity(self):
        from scripts.volleyball_ui.photo_diversity import similar
        self.assertTrue(similar(('same', None), ('same', None)))
        self.assertFalse(similar(('one', None), ('two', None)))
        self.assertTrue(similar(('one', (bytes([100] * 48), 15)),
                                ('two', (bytes([103] * 48), 14))))
        self.assertFalse(similar(('one', (bytes([100] * 48), 15)),
                                 ('two', (bytes([150] * 48), 15))))
        self.assertFalse(similar(('one', (bytes([100] * 48), 0)),
                                 ('two', (bytes([100] * 48), 65535))))

    def test_real_jpeg_comparison_keeps_distinct_views(self):
        try:
            import cv2
            import numpy as np
        except ImportError:
            self.skipTest("OpenCV is available in the project WSL environment")
        from scripts.volleyball_ui.photo_diversity import signature, similar
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            image = np.full((192, 96, 3), 75, dtype=np.uint8)
            cv2.rectangle(image, (30, 45), (65, 125), (50, 150, 210), -1)
            cv2.circle(image, (48, 25), 15, (130, 170, 200), -1)
            cv2.line(image, (35, 120), (25, 185), (130, 130, 130), 12)
            cv2.line(image, (60, 120), (70, 185), (130, 130, 130), 12)
            front, compressed, pose = [root / name for name in ('front.jpg', 'compressed.jpg', 'pose.jpg')]
            cv2.imwrite(str(front), image, [cv2.IMWRITE_JPEG_QUALITY, 95])
            cv2.imwrite(str(compressed), image, [cv2.IMWRITE_JPEG_QUALITY, 85])
            changed = image.copy()
            cv2.line(changed, (30, 50), (4, 5), (50, 150, 210), 14)
            cv2.line(changed, (65, 50), (92, 5), (50, 150, 210), 14)
            cv2.imwrite(str(pose), changed)
            self.assertTrue(similar(signature(front), signature(compressed)))
            self.assertFalse(similar(signature(front), signature(pose)))

    def test_missing_source_is_reported_without_saving(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(data, 'RESULTS', Path(tmp)), patch.object(
            data, 'global_players', return_value=[{'id': 'p'}]
        ), patch.object(data, 'groups_for', return_value=[{'id': 'p', 'track_ids': ['1']}]), patch.object(
            library, 'photos_metadata', side_effect=FileNotFoundError('missing source')
        ):
            result = library.pending_player_photos('project', 'video', 'p', 'cache')
            self.assertEqual(result['count'], 0)
            self.assertIn('missing source', result['errors'][0])
            self.assertFalse((Path(tmp) / 'players' / 'p' / 'index.json').exists())

    def test_assignment_reconciles_without_automatic_enrollment(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            filename = 'a' * 24 + '.jpg'
            folder = root / 'players' / 'old'
            (folder / 'photos').mkdir(parents=True)
            (folder / 'photos' / filename).write_bytes(b'photo')
            data.save_json(folder / 'index.json', {'schema': 1, 'photos': [{
                'filename': filename, 'project_id': 'project', 'video': 'video',
                'cache_id': 'cache', 'track_id': '1'}]})
            with patch.object(data, 'RESULTS', root), patch.object(
                data, 'global_players', return_value=[{'id': 'old'}, {'id': 'new'}]
            ), patch.object(data, 'groups_for', return_value=[{'id': 'new', 'track_ids': ['1']}]), patch.object(
                library, 'photos_metadata'
            ) as metadata:
                result = library.sync_photos('project', 'video', ['1'], 'cache', allowed_filenames=set())
                self.assertEqual(result['copied'], 0)
                self.assertEqual(library.album_photos('old')['count'], 0)
                self.assertEqual(library.album_photos('new')['count'], 0)
                metadata.assert_not_called()

    def test_pending_save_and_filter_preserve_originals(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sources = [root / f'{i}.jpg' for i in range(3)]
            for path, content in zip(sources, (b'front', b'front', b'back')):
                path.write_bytes(content)
            groups = [{'id': 'p', 'track_ids': ['1']},
                      {'id': 'auto', 'track_ids': ['2'], 'link_meta': {'2': {'reviewed': False}}}]
            with patch.object(data, 'RESULTS', root), patch.object(
                data, 'global_players', return_value=[{'id': 'p'}, {'id': 'auto'}]
            ), patch.object(data, 'groups_for', return_value=groups), patch.object(
                library, 'photos_metadata', return_value={'samples': [
                    {'frame': i, 'index': i} for i in range(3)]}
            ), patch.object(library, 'photo_path', side_effect=lambda v, t, i, *args: sources[i]):
                pending = library.pending_player_photos('project', 'video', 'p', 'cache')
                self.assertEqual(pending['count'], 2)
                self.assertEqual(library.pending_player_photos('project', 'video', 'auto', 'cache')['count'], 0)
                self.assertFalse((root / 'players' / 'p' / 'index.json').exists())
                self.assertEqual(library.save_player_photos('project', 'video', 'p', 'cache')['copied'], 2)
                self.assertEqual(library.pending_player_photos('project', 'video', 'p', 'cache')['count'], 0)
                folder = library.album_dir('p')
                manifest = folder / 'index.json'
                index = data.read_json(manifest)
                duplicate = 'f' * 24 + '.jpg'
                (folder / 'photos' / duplicate).write_bytes(b'front')
                index['photos'].append({**index['photos'][0], 'filename': duplicate})
                data.save_json(manifest, index)
                before = manifest.read_bytes()
                result = library.filtered_album_photos('p')
                self.assertEqual(result['count'], 2)
                self.assertEqual(result['hidden_count'], 1)
                self.assertEqual(manifest.read_bytes(), before)
                self.assertTrue((folder / 'photos' / duplicate).exists())
                self.assertEqual(library.album_photos('p')['count'], 3)
                library.remove_album_photo('p', library._key('project', 'video', '1', 2, 'cache') + '.jpg')
                self.assertEqual(library.pending_player_photos('project', 'video', 'p', 'cache')['count'], 0)


if __name__ == '__main__':
    unittest.main()
