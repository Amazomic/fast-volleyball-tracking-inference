import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {createRequire} from "node:module";
import {join} from "node:path";
import {fileURLToPath} from "node:url";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const require=createRequire(import.meta.url);
const chain=require(join(root,"rust/volley-app/static/studio_chain.js"));
chain.setRuntime((await WebAssembly.instantiate(readFileSync(join(root,"rust/volley-app/static/volley_web.wasm")),{})).instance.exports);
const i=n=>String(n).padStart(32,"0");
const videoA="match-A.mp4",videoB="match-B.mp4";
const overview=(video,fusion,replay,gpu)=>({
  schema:"volley-native-analysis-overview/1",video,metadata_only:true,
  stages:[
    {key:"gpu",items:gpu||[]},
    {key:"fusion",items:fusion.map(id=>({id,
      ravel_job_id:id===i(1)?i(7):null,
      vball_job_id:id===i(1)?i(8):null}))},
    {key:"tracking",items:replay.map(([id,source_fusion_id])=>({id,source_fusion_id}))},
  ]
});
const overviews=new Map([
  [videoA,overview(videoA,[i(1),i(2)],[[i(3),i(1)],[i(4),i(2)]],
    [{id:i(7),model:"players"},{id:i(8),model:"ball"}])],
  [videoB,overview(videoB,[i(5)],[[i(6),i(5)]])]
]);
const runs=[
  {video:videoA,rally_run_id:i(9),source_replay_id:i(3),candidate_count:6},
  {video:videoA,rally_run_id:i(10),source_replay_id:i(4),candidate_count:8},
  {video:videoB,rally_run_id:i(11),source_replay_id:i(6),candidate_count:5},
  {video:videoA,rally_run_id:i(12),source_replay_id:i(99),candidate_count:2}
];
const result=chain.build([videoA,videoB],overviews,runs);
const a1=chain.key(videoA,"fusion",i(1));
const a2=chain.key(videoA,"fusion",i(2));
const b1=chain.key(videoB,"fusion",i(5));
assert.deepEqual(result.byAnalysis.get(a1).map(r=>r.rally_run_id),[i(9)]);
assert.deepEqual(result.byAnalysis.get(a2).map(r=>r.rally_run_id),[i(10)]);
assert.deepEqual(result.byAnalysis.get(b1).map(r=>r.rally_run_id),[i(11)]);
assert.equal(result.runToAnalysis.get(i(10)),a2);
assert.equal(result.runToAnalysis.get(i(11)),b1);
assert.equal(result.byAnalysis.has(chain.key(videoA,"gpu",i(7))),false,
  "GPU sources already used in fusion must not appear as unmerged");
const orphan=result.runToAnalysis.get(i(12));
assert.equal(JSON.parse(orphan)[1],"unlinked");
assert.match(result.choices.find(item=>item.value===orphan).label,/не установлен/);
assert.equal(result.byAnalysis.get(orphan).length,1);
const none=chain.build([videoA],new Map(),[]);
assert.equal(none.choices.length,0);
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const header=html.split('<header class="topbar studio-chain-topbar">')[1]?.split("</header>")[0];
assert.ok(header,"Expected source chain in application header");
for(const stale of [/class="breadcrumbs"/,/id="studio-ui-settings"/,/class="dev-chip"/,/id="studio-section-name"/]){
  assert.doesNotMatch(header,stale,"Removed header chrome must not reappear");
}
for(const id of ["studio-project","studio-inference","studio-run"]){
  assert.match(header,new RegExp('id="'+id+'"'),id);
  assert.equal((html.match(new RegExp('id="'+id+'"',"g"))||[]).length,1);
}
assert.ok(html.indexOf("/studio/chain.js")<html.indexOf("/studio/app.js"),
  "Chain model must load before the app");
assert.match(header,/id="studio-quick-open"[^>]*data-studio-tab="quick"/);
assert.match(header,/studio-quick-reel-icon/);
assert.doesNotMatch(header,/id="studio-refresh"/);
assert.doesNotMatch(html,/id="studio-nav-quick"|id="studio-quick-summary"/);
const routes=readFileSync(join(root,"rust/volley-app/src/main.rs"),"utf8");
assert.match(routes,/route\("\/studio\/chain\.js"/);
console.log("PASS: header has project -> GPU inference -> matching CPU versions, including isolation across videos.");
