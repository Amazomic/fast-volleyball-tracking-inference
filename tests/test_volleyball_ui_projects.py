"""Project API, event timeline and global multi-source highlight selection tests."""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, timeline
from scripts import build_project_highlights as builder


class ProjectDataTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.p=self.root/"project";self.p.mkdir()
        self.out=self.root/"results";self.out.mkdir()
        for name in ("one.mp4","two.mp4"):
            (self.p/name).write_bytes(b"video")
        self.patches=[
            patch.object(data,"PROJECT",self.p),patch.object(data,"RESULTS",self.out),
            patch.object(data,"IDS_FILE",self.out/"player_identities.json"),
            patch("scripts.volleyball_ui.preview.video_info",return_value={"duration_sec":120.0}),
            patch.object(projects,"PROJECTS_DIR",self.out/"projects"),
            patch.object(projects,"REGISTER",self.out/"projects"/"projects.json"),
        ]
        for obj in self.patches:obj.start();self.addCleanup(obj.stop)

    def test_create_and_append_two_videos(self):
        first=projects.create_project("Турнир",["one.mp4"])
        self.assertEqual(first["videos"],["one.mp4"])
        second=projects.update_project(first["id"],videos=["one.mp4","two.mp4","one.mp4"])
        self.assertEqual(second["videos"],["one.mp4","two.mp4"])
        self.assertEqual(len(projects.all_projects()),1)
        self.assertEqual(len(projects.project_videos(first["id"])),2)

    def test_project_identities_are_separate(self):
        a=projects.create_project("Проект A",["one.mp4"])
        b=projects.create_project("Проект B",["one.mp4"])
        data.assign("one.mp4",["12","34"],"Игрок",project_id=a["id"])
        self.assertEqual(len(data.groups_for("one.mp4",a["id"])),1)
        self.assertEqual(data.groups_for("one.mp4",b["id"]),[])
        self.assertEqual(data.groups_for("one.mp4"),[])

    def test_project_roster_bulk_assignment_and_video_scoping(self):
        project = projects.create_project("Матч", ["one.mp4", "two.mp4"])
        project_id = project["id"]
        roster = data.create_player("Иван", project_id)
        ivan = roster[0]["id"]
        self.assertEqual(roster[0]["tracks_by_video"], {})
        self.assertEqual(data.groups_for("one.mp4", project_id), [])

        other = data.create_player("Пётр", project_id)[1]["id"]
        data.assign("one.mp4", ["12", "13", "14"], project_id=project_id, player_id=ivan)
        data.assign("two.mp4", ["12"], project_id=project_id, player_id=other)
        data.assign("one.mp4", ["13"], project_id=project_id, player_id=other)
        current = {p["id"]: p for p in data.players_for(project_id)}
        self.assertEqual(current[ivan]["tracks_by_video"]["one.mp4"], ["12", "14"])
        self.assertEqual(current[other]["tracks_by_video"]["one.mp4"], ["13"])
        self.assertEqual(current[other]["tracks_by_video"]["two.mp4"], ["12"])
        self.assertEqual(current[other]["total_track_ids"], 2)

        data.unassign("one.mp4", ["12", "14"], project_id)
        self.assertEqual(len(data.players_for(project_id)), 2)
        self.assertNotIn("one.mp4", data.players_for(project_id)[0]["tracks_by_video"])
        data.rename_player(ivan, "Иван №7", project_id)
        self.assertEqual(data.players_for(project_id)[0]["name"], "Иван №7")
        data.delete_player(other, project_id)
        self.assertEqual(len(data.players_for(project_id)), 1)
        self.assertEqual(data.groups_for("two.mp4", project_id), [])

    def test_global_players_available_for_new_projects(self):
        initial = projects.create_project("Первая игра", ["one.mp4"])
        alice = data.create_player("Алиса", initial["id"])[0]
        other = projects.create_project("Вторая игра", ["two.mp4"],
                                        player_ids=[alice["id"]])
        self.assertEqual(other["player_ids"], [alice["id"]])
        self.assertEqual(data.players_for(other["id"])[0]["name"], "Алиса")
        data.assign("one.mp4", ["4"], project_id=initial["id"],
                    player_id=alice["id"])
        self.assertEqual(data.players_for(other["id"])[0]["tracks_by_video"], {})
        data.assign("two.mp4", ["4", "5"], project_id=other["id"],
                    player_id=alice["id"])
        self.assertEqual(data.players_for(initial["id"])[0]["total_track_ids"], 1)
        self.assertEqual(data.players_for(other["id"])[0]["total_track_ids"], 2)
        data.delete_player(alice["id"], initial["id"])
        self.assertFalse(data.players_for(initial["id"]))
        self.assertEqual(len(data.players_for(other["id"])), 1)
        self.assertEqual(len(data.global_players()), 1)
        with self.assertRaisesRegex(ValueError, "общей базе"):
            projects.create_project("Некорректный", ["one.mp4"],
                                    player_ids=["not-found"])

    def test_verified_photos_follow_manual_track_owner(self):
        from scripts.volleyball_ui import player_library
        game = projects.create_project("Тренировка", ["one.mp4"])
        pid = game["id"]
        first = data.create_player("Игрок А", pid)[0]["id"]
        second = data.create_player("Игрок Б", pid)[1]["id"]
        data.assign("one.mp4", ["8"], project_id=pid, player_id=first)
        crop = self.root / "sample.jpg"
        crop.write_bytes(b"sample jpeg bytes")
        fake_samples = {"samples": [
            {"index": 0, "frame": 77, "confidence": 0.9},
            {"index": 1, "frame": 90, "confidence": 0.8}
        ]}
        with (patch.object(player_library, "photos_metadata", return_value=fake_samples),
              patch.object(player_library, "photo_path", return_value=crop)):
            result = player_library.sync_photos(pid, "one.mp4", ["8"])
            self.assertEqual(result["copied"], 2)
            self.assertEqual(player_library.photo_counts()[first], 2)
            # Repeated collection must not duplicate training samples.
            self.assertEqual(player_library.sync_photos(pid, "one.mp4", ["8"])["copied"], 0)
            data.assign("one.mp4", ["8"], project_id=pid, player_id=second)
            player_library.sync_photos(pid, "one.mp4", ["8"])
            self.assertEqual(player_library.photo_counts()[first], 0)
            self.assertEqual(player_library.photo_counts()[second], 2)
            data.unassign("one.mp4", ["8"], project_id=pid)
            player_library.sync_photos(pid, "one.mp4", ["8"])
            self.assertEqual(player_library.photo_counts()[second], 0)

    def test_roster_legacy_migration_and_validation(self):
        p = projects.create_project("Старая разметка", ["one.mp4", "two.mp4"])
        project_id = p["id"]
        legacy = {"schema": 1, "videos": {
            "one.mp4": [{"id": "old", "name": "Игрок А", "track_ids": ["7"]}],
            "two.mp4": [{"id": "another", "name": "Игрок А", "track_ids": ["9"]}]
        }}
        data.save_json(data.identity_file(project_id), legacy)
        roster = data.players_for(project_id)
        self.assertEqual(len(roster), 1)
        self.assertEqual(roster[0]["total_track_ids"], 2)
        self.assertEqual(data.groups_for("two.mp4", project_id)[0]["id"], roster[0]["id"])
        data.assign("one.mp4", ["10"], project_id=project_id, player_id=roster[0]["id"])
        self.assertEqual(data.read_json(data.identity_file(project_id))["schema"], 2)
        with self.assertRaisesRegex(ValueError, "уже существует"):
            data.create_player("игрок а", project_id)
        with self.assertRaisesRegex(ValueError, "не найден"):
            data.assign("one.mp4", ["99"], project_id=project_id, player_id="missing")

    def test_create_project_stores_video_ranges(self):
        p=projects.create_project("01.11.2026",["one.mp4","two.mp4"],{
            "one.mp4":{"start":10,"end":45},
            "two.mp4":{"start":20,"end":119}
        })
        self.assertEqual(projects.project_ranges(p["id"])["one.mp4"],{"start":10.0,"end":45.0})
        self.assertEqual(projects.project_ranges(p["id"])["two.mp4"],{"start":20.0,"end":119.0})
        with self.assertRaises(ValueError):
            projects.create_project("bad",["one.mp4"],{"one.mp4":{"start":45,"end":12}})
        with self.assertRaises(ValueError):
            projects.update_project(p["id"],ranges={"two.mp4":{"start":0,"end":121}})

    def test_default_project_uses_whole_video_and_preserves_saved_ranges(self):
        p=projects.create_project("Турнир",["one.mp4","two.mp4"])
        self.assertEqual(projects.project_ranges(p["id"]), {
            "one.mp4":{"start":0.0,"end":120.0},
            "two.mp4":{"start":0.0,"end":120.0}
        })
        projects.update_project(p["id"],ranges={"one.mp4":{"start":10,"end":30}})
        projects.update_project(p["id"],videos=["one.mp4"])
        self.assertEqual(projects.project_ranges(p["id"]),{"one.mp4":{"start":10.0,"end":30.0}})
        projects.update_project(p["id"],videos=["one.mp4","two.mp4"])
        self.assertEqual(projects.project_ranges(p["id"])["two.mp4"],{"start":0.0,"end":120.0})

    def test_duplicate_names_rejected_and_rename_checked(self):
        first=projects.create_project("01.11.2026",["one.mp4"])
        with self.assertRaisesRegex(ValueError,"уже существует"):
            projects.create_project(" 01.11.2026 ",["two.mp4"])
        with self.assertRaisesRegex(ValueError,"уже существует"):
            projects.create_project("01.11.2026",["two.mp4"])
        second=projects.create_project("Второй",["two.mp4"])
        with self.assertRaisesRegex(ValueError,"уже существует"):
            projects.update_project(second["id"],name="01.11.2026")
        projects.update_project(first["id"],name="01.11.2026")
        self.assertEqual(len(projects.all_projects()),2)

    def test_archive_project_keeps_source_and_is_recoverable(self):
        first=projects.create_project("Тестовый",["one.mp4"])
        other=projects.create_project("Не удалять",["two.mp4"])
        folder=projects.project_dir(first["id"])
        sample=folder/"analysis"/"cache"/"ranking.json"
        sample.parent.mkdir(parents=True)
        sample.write_text('{"data": "keep"}',encoding="utf-8")
        archived=projects.archive_project(first["id"])
        self.assertEqual(archived["id"],first["id"])
        self.assertTrue(archived["archived"])
        self.assertFalse(folder.exists())
        saved=self.out/"projects"/".trash"/first["id"]/"analysis"/"cache"/"ranking.json"
        self.assertEqual(saved.read_text(encoding="utf-8"),'{"data": "keep"}')
        self.assertTrue((self.p/"one.mp4").exists())
        self.assertTrue((self.p/"two.mp4").exists())
        self.assertEqual([x["id"] for x in projects.all_projects()],[other["id"]])
        with self.assertRaisesRegex(ValueError,"не найден"):
            projects.archive_project(first["id"])

    def test_archive_project_with_no_results_does_not_touch_other_project(self):
        first=projects.create_project("Удалить",["one.mp4"])
        other=projects.create_project("Сохранить",["one.mp4"])
        projects.archive_project(first["id"])
        self.assertEqual(projects.get_project(other["id"])["name"],"Сохранить")
        self.assertTrue(projects.project_dir(other["id"]).is_dir())

    def test_project_cache_does_not_see_other_projects(self):
        first=projects.create_project("A",["one.mp4"])
        second=projects.create_project("B",["one.mp4"])
        path=projects.project_cache_root(first["id"])/"unique"/"analysis"
        path.mkdir(parents=True)
        (path/"events.json").write_text(json.dumps({
            "source":str(self.p/"one.mp4"),"events":[{"type":"attack","frame":4}]
        }),encoding="utf-8")
        self.assertIsNotNone(data.find_cache("one.mp4",first["id"]))
        self.assertIsNone(data.find_cache("one.mp4",second["id"]))
        self.assertFalse(data.report("one.mp4",second["id"])["analyzed"])
        self.assertTrue(data.report("one.mp4",first["id"])["analyzed"])

    def test_rejects_unknown_video(self):
        with self.assertRaises(ValueError):
            projects.create_project("bad",["missing.mp4"])

    def test_timeline_respects_original_source_offset(self):
        cache=self.out/"analysis"/"123"
        (cache/"analysis").mkdir(parents=True)
        (cache/"analysis"/"events.json").write_text(json.dumps({
            "source":str(self.p/"one.mp4"),"fps":30,"analysis_offset_sec":120,
            "events":[
                {"type":"rally_start","track_id":"abc","time_sec":5},
                {"type":"attack","time_sec":6,"id":"x","confidence":.8},
                {"type":"rally_end","track_id":"abc","time_sec":9}
            ]
        }),encoding="utf-8")
        result=timeline.timeline("one.mp4")
        self.assertEqual(result["rallies"][0]["start"],125)
        self.assertEqual(result["rallies"][0]["end"],129)
        self.assertEqual(result["actions"][0]["time"],126)


class MultiSourceHighlightTests(unittest.TestCase):
    def test_montage_filters_detected_duration_not_padded_length(self):
        base = {"ranked":True, "detected_start_frame":50,
                "detected_end_frame":94, "duration_sec":7.0,
                "confidence":.83}
        # Raw detection lasts 1.5 seconds, even if editing adds padding.
        self.assertTrue(builder.montage_eligible(base,1.5,.8))
        self.assertFalse(builder.montage_eligible(base,2.0,.8))
        self.assertFalse(builder.montage_eligible(base,1.5,.9))
        self.assertFalse(builder.montage_eligible({**base,"ranked":False},1.5,.2))

    def test_all_source_videos_appear_in_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            sources=[root/"one.mp4",root/"two.mp4"]
            for src in sources:src.write_bytes(b"test")
            caches=[root/"cache-a",root/"cache-b"]
            for cache in caches:
                cache.mkdir()
                (cache/"ranking.json").write_text('[]',encoding="utf-8")
            clips={
                "cache-a":[{"ranked":True, "ranking_score":9,"duration_sec":3,"source_start_frame":0,"source_end_frame":89,
                     "detected_start_frame":0,"detected_end_frame":89,"confidence":.8}],
                "cache-b":[{"ranked":True, "ranking_score":8,"duration_sec":4,"source_start_frame":90,"source_end_frame":209,
                     "detected_start_frame":90,"detected_end_frame":209,"confidence":.9}],
            }
            produced=[]
            def mock_render(src,normalized,segments,output_path=None,segments_folder=None,progress_callback=None,**kwargs):
                produced.append(src.name)
                if progress_callback:
                    progress_callback("clip","start",1,1,0,90)
                    progress_callback("clip","complete",1,1,90,90)
                    progress_callback("join","start",1,1,0,1)
                    progress_callback("join","complete",1,1,1,1)
                output_path.write_bytes(b"fake-mp4-data")
            def mock_concat(args,check=False,**kwargs):
                Path(args[-1]).write_bytes(b"merged-mp4-data")
            out=root/"project-output";out.mkdir()
            with patch.object(builder,"project_videos",return_value=sources), \
                 patch.object(builder,"locate_cache",side_effect=caches), \
                 patch.object(builder,"load_groups",side_effect=lambda c,r,**kw:clips[c.name]), \
                 patch.object(builder,"render",side_effect=mock_render), \
                 patch.object(builder,"project_dir",return_value=out), \
                 patch.object(builder,"project_output",return_value=out/"highlights.mp4"), \
                 patch.object(builder.subprocess,"run",side_effect=mock_concat), \
                 patch.object(sys,"argv",["script","--project-id","abc","--duration","120"]):
                builder.main()
            manifest=json.loads((out/"highlights_manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(set(produced),{"one.mp4","two.mp4"})
            self.assertEqual(len(manifest["selected_rallies"]),2)
            self.assertEqual(manifest["min_rally_sec"],1.5)
            self.assertEqual(manifest["min_confidence"],.5)
            self.assertTrue((out/"highlights.mp4").is_file())


if __name__=="__main__":
    unittest.main()
