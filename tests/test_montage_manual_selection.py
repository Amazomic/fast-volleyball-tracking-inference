"""Manual montage selection must use only explicitly checked complete rallies."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui import data, projects, montage_project, montage_runner


class ManualMontageTests(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root=Path(temp.name)
        videos=self.root/"videos"
        videos.mkdir()
        (videos/"match.mp4").write_bytes(b"original")
        outputs=self.root/"outputs"
        outputs.mkdir()
        for module,key,value in (
            (data,"PROJECT",videos),
            (data,"RESULTS",outputs),
            (projects,"PROJECTS_DIR",outputs/"projects"),
            (projects,"REGISTER",outputs/"projects"/"projects.json"),
        ):
            hooked=patch.object(module,key,value)
            hooked.start()
            self.addCleanup(hooked.stop)
        info=patch("scripts.volleyball_ui.preview.video_info",
                   return_value={"duration_sec":60})
        info.start()
        self.addCleanup(info.stop)
        self.project=projects.create_project("Manual test",["match.mp4"])
        self.cache=self.root/"cache"
        track_dir=self.cache/"tracks/analysis/tracks"
        track_dir.mkdir(parents=True)
        analysis=self.cache/"analysis"
        analysis.mkdir()
        tracks=[]
        ranking=[]
        for i,(start,end) in enumerate(((100,240),(450,690),(900,1260)),1):
            row={"start_frame":start,"last_frame":end,"track_id":i,
                 "positions":[[[100,200],start+20]],
                 "rally_classification":{"label":"rally",
                    "is_rally":True,"rally_confidence":.95},
                 "rally_features":{"attack_count":i}}
            (track_dir/f"track_{i:04}.json").write_text(json.dumps(row))
            tracks.append(row)
            ranking.append({"start":start,"end":end,"score":20+i,
                            "confidence":.95})
        (analysis/"game-tracks.json").write_text(json.dumps(tracks))
        (analysis/"events.json").write_text("{}")
        (self.cache/"ranking.json").write_text(json.dumps(ranking))
        hooked=patch.object(data,"find_cache",return_value=self.cache)
        hooked.start()
        self.addCleanup(hooked.stop)

    def request(self,ids=None):
        return {"project_id":self.project["id"],"mode":"montage",
                "follow_ball":False,"duration":1,
                "rally_selection_mode":"manual","selected_rally_ids":ids or [],
                "context_sec":0,"rally_min_sec":1.5,"min_rally":1.5}

    def test_preview_and_montage_use_the_same_rally_ids(self):
        from scripts.volleyball_ui.rally_view import from_tracks
        preview=from_tracks(
            self.cache,0,30,60,gap_sec=3,min_sec=1.5,context_sec=0,
            source_video="match.mp4")
        montage=montage_runner.preview_selection(self.request())
        self.assertEqual(
            [r["rally_id"] for r in preview["rallies"]],
            [r["rally_id"] for r in montage["candidates"]])

    def test_candidates_are_complete_stable_and_not_capped_by_duration(self):
        # Manual mode presents the analysis rallies, not an automatic quality subset.
        with patch.object(montage_project,"montage_eligible",
                          side_effect=AssertionError("manual quality filter called")):
            plan=montage_runner.preview_selection(self.request())
        self.assertEqual(len(plan["candidates"]),3)
        self.assertEqual(plan["selected"],[])
        self.assertEqual(plan["selected_duration"],0)
        ids=[row["rally_id"] for row in plan["candidates"]]
        self.assertEqual(len(set(ids)),3)
        self.assertEqual(ids,[row["rally_id"] for row in
                             montage_runner.preview_selection(self.request())["candidates"]])
        chosen=[ids[0],ids[2]]
        plan=montage_runner.preview_selection(self.request(chosen))
        self.assertEqual([row["rally_id"] for row in plan["selected"]],chosen)
        self.assertGreater(plan["selected_duration"],1)
        self.assertAlmostEqual(plan["selected_duration"],
            sum(row["duration"] for row in plan["selected"]),places=2)
        config=montage_runner.validate_request(self.request(chosen))
        clips=montage_project.plan_clips(
            montage_runner.sources_for(config),
            self.root/"manual_tracks",plan["selected_duration"],
            mode="montage",padding=0,rally_min_sec=1.5,
            selected_rally_ids=chosen)
        self.assertEqual(len(clips),2)
        self.assertEqual([c["track_ids"][0] for c in clips],[1,3])
        self.assertAlmostEqual(sum(c["source_duration"] for c in clips),
                               plan["selected_duration"],places=2)

    def test_manual_export_uses_only_checked_rallies_and_exact_total(self):
        all_rows=montage_runner.preview_selection(self.request())["candidates"]
        target=all_rows[1]
        clip_calls=[]
        composed=[]
        def clip(source,dest,**kw):
            clip_calls.append(kw)
            dest.parent.mkdir(parents=True,exist_ok=True)
            dest.write_bytes(b"native clip")
            return {"start_sec":kw["start_frame"]/30,
                    "fps":30,"frames":kw["end_frame"]-kw["start_frame"]+1}
        def compose(segments,output,seconds,mode):
            composed.append((len(segments),seconds,mode))
            output.write_bytes(b"finished")
        with patch("scripts.volleyball_ui.montage_source.clip_from_original",
                   side_effect=clip),patch.object(montage_runner,"compose_clips",
                   side_effect=compose):
            montage_runner.execute({**self.request([target["rally_id"]]),
                                    "request_id":"manual_test"})
        self.assertEqual(len(clip_calls),1)
        self.assertEqual(clip_calls[0]["start_frame"],450)
        self.assertEqual(clip_calls[0]["end_frame"],690)
        self.assertEqual(composed[0][0],1)
        self.assertAlmostEqual(composed[0][1],target["duration"],places=2)
        manifest=projects.project_dir(self.project["id"])/"montages/manual_test/selection.json"
        saved=json.loads(manifest.read_text())
        self.assertEqual(saved["selection_filters"]["mode"],"manual")
        self.assertEqual(saved["selection_filters"]["selected_rally_ids"],
                         [target["rally_id"]])
        self.assertEqual(len(saved["selected"]),1)

    def test_auto_mode_lists_every_rally_but_only_marks_exported_subset(self):
        request={**self.request(),"rally_selection_mode":"auto","duration":8}
        plan=montage_runner.preview_selection(request)
        self.assertEqual(len(plan["candidates"]),3)
        self.assertEqual(len(plan["selected"]),1)
        self.assertEqual(sum(int(row["selected"]) for row in plan["candidates"]),1)
        self.assertEqual([row["rally_id"] for row in plan["selected"]],
                         [row["rally_id"] for row in plan["candidates"] if row["selected"]])
        # An impossible automatic time budget must not hide the catalogue.
        short=montage_runner.preview_selection({**request,"duration":1})
        self.assertEqual(len(short["candidates"]),3)
        self.assertEqual(short["selected"],[])
        self.assertTrue(short["selection_error"])

    def test_missing_ranking_does_not_hide_manual_catalogue(self):
        (self.cache/"ranking.json").unlink()
        manual=montage_runner.preview_selection(self.request())
        self.assertEqual(len(manual["candidates"]),3)
        self.assertTrue(all("rally_id" in row for row in manual["candidates"]))
        auto=montage_runner.preview_selection(
            {**self.request(),"rally_selection_mode":"auto","duration":40})
        self.assertEqual(len(auto["candidates"]),3)
        self.assertEqual(auto["selected"],[])
        self.assertTrue(auto["selection_error"])

    def test_two_source_project_manual_selection_uses_only_checked_video(self):
        (data.PROJECT/"second.mp4").write_bytes(b"second original")
        project=projects.create_project("Two sources",["match.mp4","second.mp4"])
        request={**self.request(),"project_id":project["id"],"selected_videos":["second.mp4"]}
        manual=montage_runner.preview_selection(request)
        self.assertEqual(manual["source_count"],1)
        self.assertEqual(len(manual["candidates"]),3)
        self.assertEqual({row["video"] for row in manual["candidates"]},{"second.mp4"})
        selected=manual["candidates"][0]["rally_id"]
        chosen=montage_runner.selection_for(
            montage_runner.validate_request({**request,"selected_rally_ids":[selected]}))
        self.assertEqual(len(chosen),1)
        self.assertEqual(chosen[0]["video"],"second.mp4")
        auto=montage_runner.preview_selection({
            **request,"rally_selection_mode":"auto","duration":120})
        self.assertEqual(auto["source_count"],2)
        self.assertEqual(len(auto["candidates"]),6)
        self.assertEqual({row["video"] for row in auto["candidates"]},
                         {"match.mp4","second.mp4"})

    def test_preview_video_selection_limits_manual_catalogue_and_export(self):
        all_rallies=montage_runner.preview_selection(self.request())["candidates"]
        self.assertEqual(len(all_rallies),3)
        none={**self.request(),"selected_videos":[]}
        empty=montage_runner.preview_selection(none)
        self.assertEqual(empty["candidates"],[])
        self.assertEqual(empty["source_count"],0)
        self.assertEqual(empty["selected_videos"],[])
        with self.assertRaises(ValueError):
            montage_runner.preview_selection({**none,
                "selected_rally_ids":[all_rallies[0]["rally_id"]]})
        chosen={**self.request(),"selected_videos":["match.mp4"]}
        self.assertEqual(len(montage_runner.preview_selection(chosen)["candidates"]),3)
        normalized=montage_runner.validate_request(chosen)
        self.assertEqual(montage_runner.validate_request(normalized)["selected_videos"],
                         ["match.mp4"])
        for invalid in (["unknown.mp4"],["match.mp4","match.mp4"],"match.mp4"):
            with self.subTest(invalid=invalid),self.assertRaises(ValueError):
                montage_runner.validate_request(
                    {**self.request(),"selected_videos":invalid})
        # The automatic mode continues considering every video.
        auto=montage_runner.validate_request({**none,
            "rally_selection_mode":"auto","duration":120})
        self.assertEqual(auto["selected_videos"],["match.mp4"])

    def test_stale_unknown_duplicate_ids_rejected(self):
        plan=montage_runner.preview_selection(self.request())
        good=plan["candidates"][0]["rally_id"]
        with self.assertRaisesRegex(ValueError,"изменился|изменились"):
            montage_runner.preview_selection(self.request(["0"*24]))
        for values in ([good,good],["../malicious"],"bad"):
            with self.subTest(values=values),self.assertRaises(ValueError):
                montage_runner.validate_request(self.request(values))
        config=montage_runner.validate_request(self.request())
        self.assertEqual(montage_runner.selection_for(config),[])
        # The API persists a normalized request; the subprocess validates again.
        self.assertEqual(montage_runner.validate_request(config)["selected_rally_ids"],[])
        auto={**self.request(),"rally_selection_mode":"auto","duration":120}
        normalized=montage_runner.validate_request(auto)
        self.assertEqual(montage_runner.validate_request(normalized)["rally_selection_mode"],"auto")
        self.assertGreater(len(montage_runner.selection_for(normalized)),0)


if __name__=="__main__":
    unittest.main()
