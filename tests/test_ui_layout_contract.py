"""Contract: page reload and data hydration may not change the UI shell geometry.

Run locally without paid CI:
    python -m unittest discover -s tests -p "test_ui_layout_contract.py"

These static guards complement the browser geometry test in players_workspace_layout.cjs.
"""
from pathlib import Path
import re
import unittest

STATIC = Path(__file__).resolve().parents[1] / "scripts" / "volleyball_ui" / "static"


class UiLayoutContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.css = (STATIC / "style.css").read_text(encoding="utf-8")
        cls.players = (STATIC / "players-workspace.css").read_text(encoding="utf-8")
        cls.app = (STATIC / "app.js").read_text(encoding="utf-8")
        cls.html = (STATIC / "index.html").read_text(encoding="utf-8")

    def test_single_viewport_height_for_every_desktop_tab(self):
        self.assertRegex(self.css, r"\.shell\s*\{height:100dvh;min-height:0\}")
        self.assertIn("height:100dvh;", self.css)
        self.assertNotRegex(self.css, r"height\s*:\s*125d?vh")
        self.assertNotRegex(self.players, r"\.shell\s*:\s*has\s*\(")
        self.assertIn(".shell>.sidebar{height:100%}", self.css)
        self.assertIn(".main>.content{display:flex;flex-direction:column;min-height:0;overflow:hidden}", self.css)

    def test_global_zoom_is_forbidden(self):
        self.assertNotRegex(self.css, r"(?i)\bzoom\s*:")
        self.assertNotIn("--studio-ui-scale", self.css)
        self.assertIn(".shell{height:100dvh;min-height:0}", self.css)

    def test_first_paint_players_matches_hydrated_players(self):
        self.assertRegex(
            self.css,
            r'html\[data-initial-view="players"\]\s*#view-players\s*'
            r'\{display:flex;flex-direction:column;flex:1 1 0;min-height:0;overflow-y:auto;overflow-x:hidden\}',
        )
        self.assertRegex(
            self.players,
            r'#view-players\.active\s*'
            r'\{display:flex;flex-direction:column;flex:1 1 0;min-height:0;overflow-y:auto;overflow-x:hidden\}',
        )
        self.assertIn("height:100%;max-height:100%;min-height:0;overflow:hidden;gap:20px", self.players)
        self.assertIn("flex:1 1 auto;min-height:0;max-height:none;overflow:auto", self.players)

    def test_restore_route_before_loading_async_data(self):
        entry = self.app.index('document.addEventListener("DOMContentLoaded"')
        head = self.app[entry:entry + 1100]
        self.assertLess(head.index('const initialView='), head.index("await loadAll("))
        self.assertLess(head.index('$$(".view").forEach('), head.index("await loadAll("))
        self.assertIn('document.documentElement.dataset.initialView=view;', self.app)
        self.assertIn('url.searchParams.set("view",view)', self.app)

    def test_data_updates_reuse_dom_and_scroll_inside_panels(self):
        self.assertIn("setMarkupIfChanged(", self.app)
        self.assertIn(".players-manager .player-roster-scroll", self.players)
        self.assertIn("scrollbar-gutter:stable", self.players)
        self.assertIn(".content>.view.active{flex:1 1 auto;min-height:0;overflow-y:auto;", self.css)

    def test_browser_cache_versioned_assets(self):
        self.assertRegex(self.html, r'/style\.css\?v=\d+')
        self.assertRegex(self.html, r'/players-workspace\.css\?v=\d+')
        self.assertRegex(self.html, r'/app\.js\?v=\d+')


if __name__ == "__main__":
    unittest.main()
