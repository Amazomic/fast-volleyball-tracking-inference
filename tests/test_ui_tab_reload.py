"""Navigation deep links must survive browser reload and retain query params."""
from pathlib import Path
import unittest

class TabReloadTests(unittest.TestCase):
    def test_navigation_updates_view_query_and_reloads_from_it(self):
        content=(Path(__file__).resolve().parents[1]/
                 "scripts/volleyball_ui/static/app.js").read_text(encoding="utf-8")
        self.assertIn('url.searchParams.set("view",view)',content)
        self.assertIn('window.history.replaceState(window.history.state,"",url.href)',content)
        self.assertIn('new URLSearchParams(window.location.search).get("view")',content)
        self.assertIn('go(requestedView)',content)
        for tab in ("analysis","analysis-results","players","report","benchmark","results"):
            self.assertIn('WORKSPACE_VIEWS=new Set(',content)
            self.assertIn('"'+tab+'"',content)

if __name__=="__main__":
    unittest.main()
