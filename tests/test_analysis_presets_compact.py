"""Compact analysis panel: presets visible, all technical inputs in details."""
from pathlib import Path
import unittest
from html.parser import HTMLParser


ROOT=Path(__file__).resolve().parents[1]
STATIC=ROOT/"scripts/volleyball_ui/static"


class _FormParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack=[]
        self.top=[]
        self.depths={}
        self.in_form=False
        self.in_preset=False
        self.inputs={}
        self.details=[]
    def handle_starttag(self,tag,attrs):
        attrs=dict(attrs)
        if tag=="form" and attrs.get("id")=="analysis-form":
            self.in_form=True
            self.stack=["form"]
            return
        if not self.in_form:return
        if len(self.stack)==1:self.top.append((tag,attrs.get("id")))
        if tag=="details":self.details.append(attrs.get("id"))
        if tag in ("input","select"):
            self.inputs[attrs.get("name")]=self.details.copy()
        if tag not in ("input","img","br","hr","meta","link","source"):
            self.stack.append(tag)
    def handle_endtag(self,tag):
        if not self.in_form:return
        if tag=="details" and self.details:self.details.pop()
        if tag in self.stack:
            while self.stack:
                popped=self.stack.pop()
                if popped==tag:break
        if tag=="form":self.in_form=False


class CompactAnalysisTests(unittest.TestCase):
    def test_technical_settings_are_in_profile_editor(self):
        html=(STATIC/"index.html").read_text(encoding="utf-8")
        p=_FormParser()
        p.feed(html)
        self.assertEqual(
            p.top,
            [("div",None),("span","analysis-run-status")])
        self.assertIn('class="analysis-profiles-layout"',html)
        self.assertIn('class="analysis-profile-choose"',html)
        self.assertIn('class="analysis-profile-editor"',html)
        self.assertIn('id="analysis-profile-save"',html)
        self.assertIn('id="analysis-preset-diff"',html)
        self.assertIn('id="analysis-timing-history"',html)
        for name in (
            "ball_model","ball_threshold","backend","analysis_width","threads",
            "beach","player_model","player_hysteresis_frames",
            "player_match_center_distance","track_max_distance",
            "track_no_ball_fill",
        ):
            with self.subTest(name=name):
                self.assertIn("analysis-technical-settings",p.inputs[name])
        self.assertNotIn('id="analysis-preset-apply"',html)
        self.assertEqual(html.count('id="analysis-start-submit"'),1)
        self.assertIn('id="analysis-preset-reset"',html)
        self.assertIn('id="analysis-save-panel" hidden',html)
        self.assertIn('id="analysis-advanced"',html)
        self.assertIn('id="analysis-pipeline-advanced"',html)

    def test_full_width_profile_above_settings_without_bottom_scope_selector(self):
        html=(STATIC/"index.html").read_text(encoding="utf-8")
        styles=(STATIC/"analysis-presets.css").read_text(encoding="utf-8")
        form=html.split('id="analysis-form"',1)[1].split('</form>',1)[0]
        choice=form.index('class="analysis-profile-choose"')
        editor=form.index('class="analysis-profile-editor"')
        start=form.index('id="analysis-start-submit"')
        self.assertLess(choice,start)
        self.assertLess(start,editor)
        self.assertNotIn("Что анализировать",form)
        self.assertNotIn('class="analysis-run-target"',form)
        self.assertNotIn('id="analysis-run-mode"',form)
        js=(STATIC/"app.js").read_text(encoding="utf-8")
        self.assertIn('kind==="tracking"?"full-segment":"full-project"',js)
        self.assertNotIn('scopeSelect.value',js)
        self.assertIn('grid-template-columns:minmax(0,1fr)',styles)
        self.assertIn('width:100%',styles)

    def test_compact_selector_uses_horizontal_space(self):
        html=(STATIC/"index.html").read_text(encoding="utf-8")
        css=(STATIC/"analysis-presets.css").read_text(encoding="utf-8")
        block=html.split('class="analysis-profile-choose"',1)[1].split(
            'class="analysis-profile-editor"',1)[0]
        self.assertIn('class="analysis-preset-toolbar"',block)
        self.assertIn('class="analysis-preset-meta"',block)
        self.assertLess(block.index('id="analysis-preset"'),
                        block.index('id="analysis-start-submit"'))
        for control in ("analysis-preset","analysis-preset-description",
                        "analysis-preset-diff","analysis-preset-reset",
                        "analysis-preset-delete","analysis-start-submit"):
            self.assertEqual(block.count('id="'+control+'"'),1)
        self.assertNotIn('id="analysis-preset-apply"',block)
        self.assertNotIn('id="analysis-preset-parameters"',block)
        self.assertIn('grid-template-columns:minmax(0,1fr) auto',css)
        self.assertIn('@media(max-width:720px)',css)

    def test_profile_differences_always_compare_to_default(self):
        html=(STATIC/"index.html").read_text(encoding="utf-8")
        js=(STATIC/"analysis-presets.js").read_text(encoding="utf-8")
        self.assertIn('id="analysis-preset-diff" class="profile-difference-list" role="list"',html)
        self.assertIn('Отличия от профиля по умолчанию «Новый»',html)
        self.assertIn('const defaults=profileValues("new")',js)
        self.assertIn('const differing=difference(current,defaults)',js)
        self.assertIn('diff.hidden=false',js)
        self.assertNotIn('changes.slice(0,6)',js)

    def test_frame_size_uses_single_custom_tooltip(self):
        code=(STATIC/"settings-tooltips.js").read_text(encoding="utf-8")
        self.assertIn('autoTitle.removeAttribute("title")',code)
        self.assertNotIn('autoTitle.title=autoTitle.dataset.tooltip',code)
        self.assertIn("autoTitle.dataset.tooltip=",code)
        self.assertIn("Автоматически по видео",code)


if __name__=="__main__":
    unittest.main()
