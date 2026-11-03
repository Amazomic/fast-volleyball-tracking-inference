"""Bilingual Volley Studio resources must remain complete and self-hosted."""
from html.parser import HTMLParser
from pathlib import Path
import json
import re
import unittest

ROOT=Path(__file__).resolve().parents[1]
STATIC=ROOT/"scripts/volleyball_ui/static"
CYR=re.compile("[а-яё]",re.I)


class RussianMarkup(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.strings=set()
        self.inside=0
    def handle_starttag(self,tag,attrs):
        if tag in ("script","style"):self.inside+=1
        for attr,val in attrs:
            if attr in ("title","aria-label","placeholder","alt","data-hint") and val and CYR.search(val):
                self.strings.add(val.strip())
    def handle_endtag(self,tag):
        if tag in ("script","style"):self.inside=max(0,self.inside-1)
    def handle_data(self,data):
        if not self.inside and CYR.search(data) and data.strip():
            self.strings.add(data.strip())


class BilingualUITests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.html=(STATIC/"index.html").read_text("utf-8")
        cls.i18n=(STATIC/"i18n.js").read_text("utf-8")
        raw=(STATIC/"i18n-en.js").read_text("utf-8")
        cls.english=json.loads(raw.split("window.VolleyEnglish=",1)[1].strip().rstrip(";"))

    def test_both_locales_and_switcher_accessible(self):
        self.assertIn('data-language-choice="ru"',self.html)
        self.assertIn('data-language-choice="en"',self.html)
        self.assertIn('aria-pressed="true"',self.html)
        self.assertIn('role="group" aria-label="Язык интерфейса"',self.html)
        self.assertLess(self.html.index('/i18n-en.js'),
                        self.html.index('/i18n.js'))
        self.assertLess(self.html.index('/i18n.js'),
                        self.html.index('/app.js'))
        self.assertIn("MutationObserver",self.i18n)
        self.assertIn("localStorage.setItem(STORAGE,current)",self.i18n)
        self.assertIn("document.documentElement.lang=current",self.i18n)
        css=(STATIC/"style.css").read_text("utf-8")
        self.assertIn(".sidebar-language-buttons",css)

    def test_every_static_russian_label_and_accessibility_attribute_has_english(self):
        scan=RussianMarkup()
        scan.feed(self.html)
        missing=[label for label in sorted(scan.strings)
                 if not self.english.get(label) or
                 CYR.search(self.english[label])]
        self.assertFalse(missing,"Untranslated HTML labels: "+repr(missing[:15]))
        self.assertGreaterEqual(len(scan.strings),350)

    def test_dynamic_rally_player_and_stage_labels_translated(self):
        required=("Розыгрыши","Найденные розыгрыши","Монтаж","Предпросмотр",
                  "Игроков","Количество игроков","Обнаружение мяча",
                  "Завершён","Запущен","Найденные розыгрыши · автоматический отбор",
                  "Сбросить контекст этого видео")
        missing=[label for label in required if CYR.search(self.english.get(label,label))]
        self.assertFalse(missing,"Missing dynamic English labels: "+repr(missing))
        self.assertIn('"/i18n-en.js"',(ROOT/"scripts/volleyball_ui/server.py").read_text("utf-8"))


if __name__=="__main__":
    unittest.main()
