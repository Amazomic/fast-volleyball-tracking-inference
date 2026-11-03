"""Extract visible Russian UI strings for offline EN translation."""
from html.parser import HTMLParser
from pathlib import Path
import re,json
BASE=Path(__file__).resolve().parent/"static"
ru=re.compile(r"[а-яё]",re.I)
class Extract(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True);self.depth=0;self.result=set()
    def handle_starttag(self,tag,attrs):
        if tag in ("script","style"):self.depth+=1
        for attr,value in attrs:
            if attr in ("aria-label","title","placeholder","data-tooltip","data-hint","alt") and value and ru.search(value):self.result.add(value.strip())
    def handle_endtag(self,tag):
        if tag in ("script","style"):self.depth=max(0,self.depth-1)
    def handle_data(self,data):
        if not self.depth and ru.search(data):
            value=data.strip()
            if value:self.result.add(value)
ex=Extract();ex.feed((BASE/"index.html").read_text("utf-8"))
html=set(ex.result);js=set()
for path in BASE.glob("*.js"):
    if path.name in ("i18n-en.js","i18n.js"):
        continue
    source=path.read_text("utf-8")
    for match in re.finditer(r"""(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')""",source):
        value=match.group()[1:-1].strip()
        if ru.search(value) and len(value)<260:
            value=re.sub(r"<[^>]*>"," ",value)
            value=re.sub(r"\\n"," ",value).strip()
            if value and ru.search(value):js.add(value)
# Backend errors and analysis-stage names also reach the web UI dynamically.
import ast,io,tokenize
python=set()
src=Path(__file__).resolve().parents[1]
py_files=list(Path(__file__).resolve().parent.glob("*.py"))+[
    src/"volleyball_studio/analysis.py", src/"run_project_analysis.py"]
for path in py_files:
    try:
        for token in tokenize.generate_tokens(io.StringIO(path.read_text("utf-8")).readline):
            if token.type!=tokenize.STRING:continue
            try:value=ast.literal_eval(token.string)
            except (SyntaxError,ValueError):continue
            if isinstance(value,str) and ru.search(value) and len(value)<=280:
                python.add(value.strip())
    except (OSError,tokenize.TokenError):continue
combined=html|js|python
out=Path(__file__).resolve().parents[2]/"results"/"i18n_extract_strings.json"
out.write_text(json.dumps({"html":sorted(html),"js":sorted(js),"python":sorted(python),
    "all":sorted(combined)},ensure_ascii=False,indent=2),encoding="utf-8")
print("HTML",len(html),"JS",len(js),"PY",len(python),"COMBINED",len(combined),"OUTPUT",out)
for value in sorted(html)[:65]:print(value[:140])
