"""Build English UI dictionary with a locally installed Argos RU->EN model.

Only developer-authored UI literals are sent to the local model.
It never translates project data, user-entered names, or server responses.
"""
from pathlib import Path
import sys,json,re,time
import argostranslate.package as packages
import argostranslate.translate as translate

ROOT=Path(__file__).resolve().parents[2]
inputs=json.loads((ROOT/"results/i18n_extract_strings.json").read_text("utf-8"))["all"]
cache=ROOT/"results/i18n_translations_work.json"
previous=json.loads(cache.read_text("utf-8")) if cache.exists() else {}
en=ROOT/"scripts/volleyball_ui/static/i18n-en.json"
base=json.loads(en.read_text("utf-8")) if en.exists() else {}
# Trust hand-edited mappings, not old failed network attempts.
work={k:v for k,v in previous.items() if not re.search(r"[а-яё]",v,re.I)}
work.update({k:v for k,v in base.items() if v!=k and not re.search(r"[а-яё]",v,re.I)})
installed=translate.get_installed_languages()
ru=next((x for x in installed if x.code=="ru"),None)
target=next((x for x in installed if x.code=="en"),None)
translator=ru.get_translation(target) if ru and target else None
if translator is None:
    print("Downloading offline ru-en model",flush=True)
    packages.update_package_index()
    found=[pkg for pkg in packages.get_available_packages()
           if pkg.from_code=="ru" and pkg.to_code=="en"]
    if not found:raise RuntimeError("No offline ru-en model in index")
    local=found[0].download()
    print("Installing",local,flush=True)
    packages.install_from_path(local)
    installed=translate.get_installed_languages()
    ru=next(x for x in installed if x.code=="ru")
    target=next(x for x in installed if x.code=="en")
    translator=ru.get_translation(target)
total=sum(1 for k in inputs if k not in work)
print("Offline translate",total,"/ total",len(inputs),flush=True)
done=0
for k in inputs:
    if k in work:continue
    if len(k)>210 or any(symbol in k for symbol in ("\\n","\\t","</","<div","<span","'+","esc(")):
        continue
    try:
        v=translator.translate(k)
        if v and not re.search(r"[а-яё]",v,re.I):work[k]=v
    except Exception as e:
        if done<5:print("Error",str(e)[:100],flush=True)
    done+=1
    if done%45==0:
        en.write_text(json.dumps({x:work.get(x,x) for x in inputs},ensure_ascii=False,indent=2),encoding="utf-8")
        print("completed",done,"of",total,flush=True)
en.write_text(json.dumps({x:work.get(x,x) for x in inputs},ensure_ascii=False,indent=2),encoding="utf-8")
print("DONE",sum(1 for k in inputs if k in work),"of",len(inputs),flush=True)
