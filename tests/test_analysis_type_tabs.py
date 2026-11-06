"""Tab-based analysis setup keeps validated fields with one launch action."""
from html.parser import HTMLParser
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
STATIC = ROOT / "scripts" / "volleyball_ui" / "static"
EXPECTED = {
    "analysis-stage-ball": "ball",
    "analysis-pipeline-advanced": "players",
    "analysis-stage-performance": "performance",
    "analysis-stage-outputs": "outputs",
}

class _AnalysisNavParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.tabs = []
        self.actions = []
        self.panels = []
    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "button" and attrs.get("role") == "tab":
            self.tabs.append(attrs)
        if tag == "button" and attrs.get("data-analysis-run"):
            self.actions.append(attrs)
        if tag == "details" and attrs.get("role") == "tabpanel":
            self.panels.append(attrs)

class AnalysisTypeTabsTests(unittest.TestCase):
    def test_tab_selection_without_individual_launch_buttons(self):
        html = (STATIC / "index.html").read_text(encoding="utf-8")
        section = html.split('class="analysis-stage-workspace"', 1)[1].split(
            'class="analysis-profile-save"', 1)[0]
        parser = _AnalysisNavParser()
        parser.feed(section)
        self.assertEqual({x["data-analysis-tab"] for x in parser.tabs}, set(EXPECTED))
        self.assertEqual({x["data-analysis-run"] for x in parser.tabs}, set(EXPECTED.values()))
        self.assertEqual(parser.actions, parser.tabs)
        self.assertNotIn('class="analysis-run-type"', html)
        self.assertEqual({x["id"] for x in parser.panels}, set(EXPECTED))
        self.assertEqual(sum("open" in x for x in parser.panels), 1)
        self.assertEqual(sum(x.get("aria-selected") == "true" for x in parser.tabs), 1)
        self.assertTrue(all(x.get("type") == "button" for x in parser.tabs))
        for panel in parser.panels:
            self.assertTrue(any(tab["id"] == panel["aria-labelledby"] for tab in parser.tabs))
        self.assertNotIn("Создание шаблона", html)
        self.assertIn("<h4>Настройки анализа</h4>", html)
        self.assertIn('id="analysis-profile-name"', html)
        self.assertIn('value="Мой профиль"', html)
        self.assertIn("Сохранить профиль", html)
        self.assertEqual(html.count('id="analysis-start-submit"'), 1)

    def test_horizontal_navigation_and_stable_settings_height(self):
        html = (STATIC / "index.html").read_text(encoding="utf-8")
        css = (STATIC / "analysis-presets.css").read_text(encoding="utf-8")
        app = (STATIC / "app.js").read_text(encoding="utf-8")
        self.assertIn('aria-orientation="horizontal"', html)
        self.assertEqual(html.count('class="analysis-stage-content-panel"'), 1)
        self.assertIn('grid-template-areas:"profile" "editor"', css)
        self.assertIn('grid-template-columns:repeat(4,minmax(0,1fr))', css)
        self.assertIn('height:clamp(440px,58vh,640px)', css)
        self.assertIn('overflow-y:auto;', css)
        self.assertIn('settingsList.scrollTop=0', app)
        self.assertIn('"ArrowLeft","ArrowRight","Home","End"', app)

    def test_stage_routing_matches_supported_backend_modes(self):
        app = (STATIC / "app.js").read_text(encoding="utf-8")
        self.assertNotIn('kind==="tracking"?"full-segment":"full-project"', app)
        self.assertNotIn('await submitTask("/api/analysis-experiment",params,"analysis")', app)
        self.assertIn('await submitTask("/api/analyze",params,"analysis")', app)
        self.assertNotIn('id="analysis-tab-tracking"', (STATIC / "index.html").read_text(encoding="utf-8"))
        self.assertIn('function selectAnalysisStage(id)', app)
        self.assertIn('data-analysis-run', app)
        profiles = (STATIC / "analysis-presets.js").read_text(encoding="utf-8")
        self.assertIn('function suggestedName()', profiles)
        self.assertIn('$("analysis-profile-name").value=suggestedName()', profiles)
        self.assertIn('body:JSON.stringify({name,values:readValues()})', profiles)
        self.assertIn('option.textContent="Пользовательский · не сохранён"', profiles)
        self.assertIn('select().value=custom?"custom":sourceId', profiles)
        self.assertIn('applyProfile(event.target.value)', profiles)

if __name__ == "__main__":
    unittest.main()
