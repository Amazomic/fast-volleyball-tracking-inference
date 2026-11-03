"""The live analysis panel must remain a standalone card above history."""
from pathlib import Path
import unittest


BASE = Path(__file__).resolve().parents[1] / "scripts" / "volleyball_ui" / "static"


class AnalysisJobPanelLayoutTests(unittest.TestCase):
    def test_live_job_is_inside_collapsible_history(self):
        html = (BASE / "index.html").read_text(encoding="utf-8")
        view = html.split('<section id="view-analysis" class="view">', 1)[1].split(
            '<section id="view-analysis-results" class="view">', 1)[0]
        job = '<section class="analysis-running-panel analysis-history-live" id="analysis-job-panel" hidden'
        history = '<details class="panel analysis-runs-panel analysis-history-collapsible"'
        self.assertIn(job, view)
        self.assertEqual(view.count('id="analysis-job-panel"'), 1)
        self.assertLess(view.index(history), view.index(job))
        self.assertLess(view.index(job), view.index('id="analysis-timing-history"'))
        self.assertEqual(view.count('id="analysis-timing-history"'), 1)
        self.assertNotIn('>Ход обработки</h3>', view)
        self.assertNotIn('id="analysis-history-refresh"', view)
        self.assertIn('id="cancel-job"', view)
        self.assertIn('id="analysis-live-stages"', view)
        self.assertIn('id="job-progress"', view)

    def test_live_status_has_own_compact_styling(self):
        css = (BASE / "style.css").read_text(encoding="utf-8")
        self.assertIn(".analysis-history-live#analysis-job-panel{", css)
        self.assertIn(".analysis-history-live-progress", css)


if __name__ == "__main__":
    unittest.main()
