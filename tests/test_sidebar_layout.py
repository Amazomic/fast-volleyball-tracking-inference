"""Sidebar composition and analysis profile labelling regressions."""
from html.parser import HTMLParser
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1] / "scripts" / "volleyball_ui" / "static"


class Markup(HTMLParser):
    def __init__(self):
        super().__init__()
        self.stack = []
        self.parents = {}

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        key = attributes.get("id") or (
            "view_" + attributes["data-view"] if "data-view" in attributes else None
        )
        if key:
            self.parents[key] = tuple(
                attrs.get("class", "") for _, attrs in self.stack
            )
        if tag not in ("meta", "input", "link", "br", "img", "source"):
            self.stack.append((tag, attributes))

    def handle_endtag(self, tag):
        for index in range(len(self.stack) - 1, -1, -1):
            if self.stack[index][0] == tag:
                del self.stack[index:]
                return


class SidebarLayoutTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.html = (ROOT / "index.html").read_text(encoding="utf-8")
        cls.css = (ROOT / "style.css").read_text(encoding="utf-8")
        cls.app = (ROOT / "app.js").read_text(encoding="utf-8")
        cls.markup = Markup()
        cls.markup.feed(cls.html)

    def test_project_and_settings_only_in_header(self):
        sidebar = self.html.split("</aside>", 1)[0]
        self.assertIn('id="create-project"', sidebar)
        self.assertIn('id="quick-reel-open"', sidebar)
        self.assertNotIn('id="project-picker"', sidebar)
        self.assertNotIn('id="chain-project"', sidebar)
        self.assertNotIn('id="project-settings-trigger"', sidebar)
        self.assertIn('header-chain-project', " ".join(self.markup.parents["chain-project"]))
        self.assertIn('header-chain-project-row',
                      " ".join(self.markup.parents["project-settings-trigger"]))
        self.assertEqual(self.html.count('id="chain-project"'), 1)
        self.assertIn('id="top-project"', self.html)

    def test_project_create_button_remains_in_sidebar(self):
        sidebar = self.html.split("</aside>", 1)[0]
        self.assertEqual(sidebar.count('id="create-project"'), 1)
        self.assertIn("topbar-controls", self.markup.parents["create-project"])

    def test_language_switch_is_the_bottommost_sidebar_control(self):
        sidebar = self.html.split("</aside>", 1)[0]
        self.assertGreater(
            sidebar.index('data-language-switch'),
            sidebar.index('data-view="benchmark"'),
        )
        self.assertEqual(sidebar.count('data-language-switch'), 1)
        self.assertEqual(sidebar.count('data-view="benchmark"'), 1)
        self.assertIn("sidebar-bottom", self.markup.parents["view_benchmark"])
        self.assertTrue(sidebar.rstrip().endswith("</div>"))

    def test_preparation_status_is_compact_and_long_errors_wrap(self):
        self.assertIn('.sidebar-preparation[data-state="ready"]', self.css)
        self.assertIn('.sidebar-preparation[data-state="ready"] .sidebar-preparation-meter{display:none}', self.css)
        self.assertIn('overflow-wrap:anywhere;text-align:right', self.css)
        self.assertIn('$("#video-preparation-count").textContent="Ошибка"', self.app)
        self.assertIn('esc(error.message)', self.app)
        self.assertIn('if(detail&&!active&&!failed&&!wasReady)detail.open=false', self.app)

    def test_analysis_selection_is_only_in_header(self):
        sidebar = self.html.split("</aside>", 1)[0]
        self.assertNotIn("data-analysis-run-select", self.html)
        self.assertNotIn('id="sidebar-analysis-run-select"', sidebar)
        self.assertIn('id="chain-inference"', self.html)
        self.assertIn('id="chain-postprocess"', self.html)
        self.assertIn("renderHeaderChain()", self.app)
        self.assertIn('volley-analysis-run:"+project', self.app)
        self.assertNotIn('group.label=profile', self.app)


if __name__ == "__main__":
    unittest.main()
