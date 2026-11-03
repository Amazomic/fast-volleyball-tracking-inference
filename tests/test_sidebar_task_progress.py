"""Project-specific live task cards in the left sidebar."""
from pathlib import Path
import shutil
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
STATIC = ROOT / "scripts" / "volleyball_ui" / "static"


class SidebarTaskProgressTests(unittest.TestCase):
    def test_sidebar_contains_separate_clickable_progress_cards(self):
        markup = (STATIC / "index.html").read_text(encoding="utf-8")
        sidebar = markup.split("</aside>", 1)[0]
        self.assertLess(sidebar.index('id="sidebar-tasks"'),
                        sidebar.index('id="video-preparation"'))
        for kind, view in (("analysis", "analysis"), ("montage", "results")):
            self.assertIn(f'id="sidebar-{kind}-task" data-task-view="{view}"', sidebar)
            for part in ("state", "stage", "fill", "count", "time"):
                self.assertIn(f'id="sidebar-{kind}-{part}"', sidebar)
        css = (STATIC / "style.css").read_text(encoding="utf-8")
        self.assertIn(".sidebar-task[hidden]{display:none!important}", css)
        app = (STATIC / "app.js").read_text(encoding="utf-8")
        self.assertIn('state.latestTask=info;', app)
        self.assertIn('renderSidebarActivity(info);', app)
        self.assertIn('[data-task-view]', app)

    @unittest.skipUnless(shutil.which("node"), "Node.js is required to execute UI logic")
    def test_state_transitions_and_project_isolation(self):
        js = r"""
const fs=require("fs"), assert=require("assert"), vm=require("vm");
const app=fs.readFileSync("scripts/volleyball_ui/static/app.js","utf8");
const code=app.slice(app.indexOf("function clockElapsed("),
                     app.indexOf("function updateMontageTask(info){"));
const nodes={};
function node(id){
  if(!nodes[id])nodes[id]={
    hidden:false,dataset:{},style:{},attributes:{},
    textContent:"",title:"",
    setAttribute(k,v){this.attributes[k]=v},
    querySelector(selector){assert.equal(selector,'[role="progressbar"]');return node(id+"/meter")}
  };
  return nodes[id];
}
const ctx={state:{project:"a",latestTask:null},$:selector=>node(selector.slice(1))};
vm.runInNewContext(code,ctx);
const analysis={kind:"analyze",project_id:"a",status:"running",progress:41.7,
  total_stages:8,completed_stages:3,stage:"Поиск мяча",elapsed_sec:84};
ctx.state.latestTask=analysis;
ctx.renderSidebarActivity(analysis);
assert.equal(node("sidebar-tasks").hidden,false);
assert.equal(node("sidebar-analysis-task").hidden,false);
assert.equal(node("sidebar-montage-task").hidden,true);
assert.equal(node("sidebar-analysis-state").textContent,"Выполняется");
assert.equal(node("sidebar-analysis-stage").textContent,"Поиск мяча");
assert.equal(node("sidebar-analysis-fill").style.width,"41.7%");
assert.equal(node("sidebar-analysis-count").textContent,"3/8 этапов · 41.7%");
assert.equal(node("sidebar-analysis-time").textContent,"01:24");
assert.equal(node("sidebar-analysis-task/meter").attributes["aria-valuenow"],"41.7");
// Switched project: the previous project's job must never leak into the sidebar.
ctx.state.project="b";
ctx.renderSidebarActivity();
assert.equal(node("sidebar-tasks").hidden,true);
const montage={kind:"highlights",project_id:"b",status:"running",progress:26,
  total_stages:4,completed_stages:1,stage:"Нарезка эпизодов",elapsed_sec:90};
ctx.renderSidebarActivity(montage);
assert.equal(node("sidebar-analysis-task").hidden,true);
assert.equal(node("sidebar-montage-task").hidden,false);
assert.equal(node("sidebar-montage-stage").textContent,"Нарезка эпизодов");
assert.equal(node("sidebar-montage-fill").style.width,"26%");
ctx.renderSidebarActivity({...montage,status:"failed",progress:63});
assert.equal(node("sidebar-montage-state").textContent,"Ошибка");
assert.equal(node("sidebar-montage-task").dataset.status,"failed");
ctx.renderSidebarActivity({...montage,status:"complete",progress:null});
assert.equal(node("sidebar-montage-state").textContent,"Готово");
assert.equal(node("sidebar-montage-count").textContent,"4/4 этапов · 100.0%");
ctx.renderSidebarActivity({...montage,status:"idle"});
assert.equal(node("sidebar-tasks").hidden,true);
console.log("sidebar task state transitions passed");
"""
        result = subprocess.run(
            ["node", "-e", js], cwd=ROOT, text=True, capture_output=True, timeout=15
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
