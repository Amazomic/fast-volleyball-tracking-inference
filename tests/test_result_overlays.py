"""Seven independent results toggles and bounded local ball trajectories."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.volleyball_ui.trajectory import trajectory_window

ROOT=Path(__file__).resolve().parents[1]


class ResultOverlayTests(unittest.TestCase):
    def test_controls_match_all_event_categories(self):
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        js=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf-8")
        expected=("touches","attacks","blocks","serves","digs","passes","trajectory")
        for name in expected:
            self.assertEqual(html.count('data-result-overlay="'+name+'"'),1)
            self.assertIn(name+":",js)
        self.assertIn('class="result-player-wrap"',html)
        self.assertLess(html.index('class="result-preview-mode-bar"'),
                        html.index('class="result-overlay-controls"'))
        self.assertLess(html.index('class="result-overlay-controls"'),
                        html.index('class="result-player-wrap"'))
        self.assertIn("localStorage.setItem(overlayKey()",js)
        self.assertIn("filter(actionVisible)",js)
        self.assertIn("if(overlays.trajectory)requestTrail(elapsed)",js)
        css=(ROOT/"scripts/volleyball_ui/static/result-media.css").read_text(encoding="utf-8")
        self.assertIn(".result-video-main > .result-overlay-controls",css)
        self.assertIn("flex-wrap:wrap;",css)
        self.assertIn("grid-template-columns:minmax(0,1fr) 304px;",css)
        for threshold in ("0","0.5","0.65","0.8"):
            self.assertIn('data-result-confidence="'+threshold+'"',html)
        self.assertIn('aria-pressed="true"',html)

    def test_generic_touches_distinct_colors_and_confidence(self):
        html=(ROOT/"scripts/volleyball_ui/static/index.html").read_text(encoding="utf-8")
        js=(ROOT/"scripts/volleyball_ui/static/results-ui.js").read_text(encoding="utf-8")
        css=(ROOT/"scripts/volleyball_ui/static/result-media.css").read_text(encoding="utf-8")
        self.assertIn("const actionVisible=a=>passesConfidence(a)",js)
        self.assertIn("(overlays.touches||Boolean(overlays[typeToControl[a.type]]))",js)
        self.assertIn("counters",js) if "counters" in js else self.assertIn("counts.touches++",js)
        self.assertIn('class="result-track-event result-track-touch"',js)
        self.assertIn("classifiedVisible(a)",js)
        for kind in ("attack","block","serve","dig","overhead_pass","touch"):
            self.assertIn(".result-track-event.result-track-"+kind,css)
        self.assertIn('id="result-confidence-threshold"',html)
        self.assertIn("trailStatus",js)
        self.assertIn("Math.round(fps*2.0)",js)

    def test_only_reads_local_bounded_csv(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)
            (cache/"analysis").mkdir(parents=True)
            (cache/"ball/analysis").mkdir(parents=True)
            (cache/"analysis/events.json").write_text(json.dumps({
                "fps":30,"coordinate_space":{"width":1080,"height":1920}
            }))
            csv_path=cache/"ball/analysis/ball.csv"
            csv_path.write_text("Frame,Visibility,X,Y,Radius\n"+
                "0,1,100,200,8\n"+
                "31,1,200,400,8\n"+
                "60,0,300,300,8\n"+
                "62,1,400,800,8\n"+
                "70,1,9999,100,8\n",encoding="utf-8")
            with patch("scripts.volleyball_ui.trajectory.assert_project_video"),\
                 patch("scripts.volleyball_ui.trajectory.find_cache",return_value=cache):
                result=trajectory_window("project","video.mp4",0,1.05)
                self.assertTrue(result["available"])
                self.assertEqual(result["points"],[[0,100,200],[31,200,400]])
                result=trajectory_window("project","video.mp4",1,1.2)
                self.assertEqual(result["points"],[[31,200,400],[62,400,800]])
                for start,length in ((-1,1),(0,13),(0,0),(float("nan"),1)):
                    with self.assertRaises(ValueError):
                        trajectory_window("project","video.mp4",start,length)


if __name__=="__main__":
    unittest.main()
