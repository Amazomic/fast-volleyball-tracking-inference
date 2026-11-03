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

    def test_project_and_settings_are_in_shared_sidebar_row(self):
        parents = self.markup.parents
        self.assertIn("sidebar-project-head", parents["project-picker"])
        self.assertIn("sidebar-project-head", parents["project-settings-trigger"])
        self.assertNotIn('<header class="topbar">', self.html)
        self.assertIn("grid-template-columns:minmax(0,1fr) 38px", self.css)
        self.assertIn("width:100%!important", self.css)

    def test_create_project_appears_above_project_selector(self):
        sidebar = self.html.split("</aside>", 1)[0]
        self.assertEqual(sidebar.count('id="create-project"'), 1)
        self.assertLess(sidebar.index('id="create-project"'),
                        sidebar.index('id="project-picker"'))
        self.assertLess(sidebar.index('id="project-picker"'),
                        sidebar.index('id="project-settings-trigger"'))
        self.assertIn("topbar-controls", self.markup.parents["create-project"])
        self.assertIn("sidebar-project-head", self.markup.parents["project-picker"])
        self.assertIn("sidebar-project-head", self.markup.parents["project-settings-trigger"])

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

    def test_analysis_selection_shows_profile_not_only_timestamp(self):
        # A single project-scoped selector governs preview, players, report and montage.
        sidebar = self.html.split("</aside>", 1)[0]
        self.assertEqual(self.html.count("data-analysis-run-select"), 1)
        self.assertIn('id="sidebar-analysis-run-select"', sidebar)
        self.assertIn("sidebar-project", self.markup.parents["sidebar-analysis-run-select"])
        self.assertLess(sidebar.index('id="project-picker"'),
                        sidebar.index('id="sidebar-analysis-run-select"'))
        self.assertLess(sidebar.index('id="sidebar-analysis-run-select"'),
                        sidebar.index('class="nav-caption"'))
        self.assertNotIn('class="analysis-version-picker"', self.html)
        self.assertIn('new Option(run.profile+" · "+run.id.slice(-6),run.id)', self.app)
        self.assertIn('id="sidebar-run-info"', sidebar)
        self.assertIn('tooltip.textContent=picked', self.app)
        self.assertIn('.sidebar-analysis-run:is(:hover,:focus-within)', self.css)
        self.assertIn('ID: "+picked.id', self.app)
        self.assertIn('volley-analysis-run:"+project', self.app)
        self.assertNotIn('group.label=profile', self.app)


if __name__ == "__main__":
    unittest.main()
