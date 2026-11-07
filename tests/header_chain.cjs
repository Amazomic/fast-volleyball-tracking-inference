"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const model=require("../scripts/volleyball_ui/static/header-chain.js");
const a1={id:"run_1",profile:"Neural A",videos:{"match.mp4":"aa","other.mp4":"bb"},
  started_at:"2026-11-04T09:00:00Z",
  video_ranges:{"match.mp4":{start:0,end:35},"other.mp4":{start:0,end:0}}};
const a2={id:"run_2",profile:"Neural B",videos:{"match.mp4":"cc","other.mp4":"dd"},
  started_at:"2026-11-04T11:00:00Z",
  video_ranges:{"match.mp4":{start:95,end:170},"other.mp4":{start:12,end:41}}};
const v1={...a1,id:"run_1_pp_x",parent_run_id:"run_1",
  videos:{"match.mp4":"aa.xx","other.mp4":"bb.yy"},
  postprocess_settings:{mode:"combined",ball_threshold:.6,player_threshold:.4,touch_threshold:.75}};
const v2={...a2,id:"run_2_pp_y",parent_run_id:"run_2",
  postprocess_settings:{mode:"ravel-geometry",player_threshold:.55,touch_threshold:.8}};
const differentVideo={id:"run_other",videos:{"only.mp4":"ee"},profile:"Other"};
const runs=[v2,a2,v1,a1,differentVideo];

assert.deepEqual(model.originalRuns(runs,"match.mp4").map(r=>r.id),["run_2","run_1"]);
assert.deepEqual(model.originalRuns(runs,"only.mp4").map(r=>r.id),["run_other"]);
assert.deepEqual(model.branches(runs,"match.mp4","run_1").map(r=>r.id),["run_1","run_1_pp_x"]);
assert.deepEqual(model.branches(runs,"match.mp4","run_2").map(r=>r.id),["run_2","run_2_pp_y"]);
assert.equal(model.branches(runs,"only.mp4","run_1").length,0);
assert.equal(model.selected(runs,"match.mp4","run_1_pp_x").parent.id,"run_1");
assert.equal(model.selected(runs,"match.mp4","run_1_pp_x").version.id,"run_1_pp_x");
assert.equal(model.selected(runs,"match.mp4","run_2").version.id,"run_2");
assert.equal(model.selected(runs,"match.mp4","run_other").version.id,"run_2");
assert.equal(model.selected(runs,"missing.mp4","run_1_pp_x").version,null);
assert.equal(model.rangeText({start:0,end:0}),"всё видео");
assert(model.rangeText({start:0,end:null}).includes("конец не записан"));
assert(model.rangeText({start:95,end:170}).includes("00:01:35"));
assert(model.initialLabel(a1,"match.mp4").includes("00:00:35"));
assert(model.initialLabel(a2,"match.mp4").includes("00:01:35"));
assert(!model.initialLabel(a1,"match.mp4").includes("00:01:35"));
assert(model.initialLabel(a1,"match.mp4").includes("кэш aa"));
assert(model.initialLabel(a1,"other.mp4").includes("кэш bb"));
assert.match(model.versionLabel(v1),/^\d{2}\.\d{2}\.\d{2} \d{2}:\d{2}$/);
assert(!model.versionLabel(v1).includes("Touch"));
assert.equal(model.versionLabel({...v1,started_at:"2026-12-24T22:14:00"}),
  "24.12.26 22:14");
assert.equal(model.versionLabel({...v1,started_at:"2026-12-24T22:14:00+03:00"}),
  "24.12.26 22:14");
assert.equal(model.versionLabel({...v1,auto_name:"24.12.26 22:14"}),
  "24.12.26 22:14");
assert.equal(model.versionLabel({...v1,started_at:null}),"Время обработки не записано");
assert.equal(model.versionLabel({...v1,display_name:"Manual label"}),"Manual label");
assert.equal(model.versionLabel(a1),"Старый полный анализ · треки, касания и розыгрыши");
assert.equal(model.versionLabel({...a1,inference_only:true}),
  "Первичный инференс нейросетей · CPU ещё не выполнен");

const app=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
const html=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/index.html"),"utf8");
for(const id of ["chain-project","chain-inference","chain-postprocess"]){
  assert(html.includes('id="'+id+'"'),id);
  assert(app.includes('"#'+id+'"'),"header change handler for "+id);
}
assert(!html.includes('id="chain-video"'));
assert(html.includes('src="/header-chain.js?v=7"'));
assert.match(html,/src="\/header-chain-menu\.js\?v=\d+"/);
assert(!html.includes('id="project-settings-trigger"'));
assert(html.includes('id="header-entry-action-dialog"'));
assert.equal(model.parseChoiceId(model.choiceId("video | clip.mp4","run3")).video,"video | clip.mp4");
assert.equal(model.parseChoiceId(model.choiceId("first.mp4","run3")).runId,"run3");
assert(app.includes("volley:select-video"));
assert(app.includes("volley-header-run:"));
console.log("HEADER CASCADE / SAME-VIDEO SEGMENTS / ISOLATED CPU BRANCHES: PASS");
