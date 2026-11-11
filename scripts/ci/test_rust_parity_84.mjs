import assert from "node:assert/strict";
import {mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {createHash} from "node:crypto";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {evaluate,touches,rallies,tracking} from "./benchmark_rust_parity_84.mjs";
const h=value=>createHash("sha256").update(value).digest("hex");
assert.deepEqual(
  Object.keys(touches([{time_sec:1},{time_sec:2},{time_sec:3}],
    [{time_sec:1.04},{time_sec:2.11},{time_sec:4}]))
    .filter(k=>["tp","fp","fn"].includes(k))
    .map(k=>touches([{time_sec:1},{time_sec:2},{time_sec:3}],
      [{time_sec:1.04},{time_sec:2.11},{time_sec:4}])[k]),
  [2,1,1]);
assert.equal(touches([{time_sec:1,player_id:"A"}],
  [{time_sec:1.02,player_id:"B"}]).player_attribution_accuracy,0);
assert.equal(rallies([{start_sec:1,end_sec:4},{start_sec:5,end_sec:8}],
  [{start_sec:1.05,end_sec:4.05},{start_sec:10,end_sec:12}]).tp,1);
const tracks=[
  {frame:0,person_id:"A",xywh:[0.1,0.1,0.2,0.3]},
  {frame:0,person_id:"B",xywh:[0.6,0.1,0.2,0.3]},
  {frame:1,person_id:"A",xywh:[0.1,0.1,0.2,0.3]},
  {frame:1,person_id:"B",xywh:[0.6,0.1,0.2,0.3]}
];
assert.equal(tracking(tracks,tracks).idf1,1);
const swapped=[
  ...tracks.slice(0,2),
  {...tracks[2],person_id:"B"},
  {...tracks[3],person_id:"A"}
];
assert.equal(tracking(tracks,swapped).idf1,0.5,
  "IDF1 must penalize temporal identity switches despite perfect per-frame boxes");
assert.equal(tracking(tracks,[]).idf1,0);
assert.equal(tracking(tracks,[...tracks,{frame:20,person_id:"C",
  xywh:[0.1,0.1,0.2,0.3]}]).idf1,1,
  "Predictions from unlabelled frames are not tracked as FP");
const directory=mkdtempSync(join(tmpdir(),"volley-parity-unit-84-"));
try{
  const file=join(directory,"explicit-source.mp4");
  writeFileSync(file,"evidence-is-not-a-video; mathematical test only");
  const gt={touches:[{time_sec:1,player_id:"A"},{time_sec:2,player_id:"B"}],
    rallies:[{start_sec:0.5,end_sec:3},{start_sec:4,end_sec:8}],
    tracks};
  const record={
    schema:"volley-parity-evidence/1",
    asset:{id:"synthetic mathematical test only",sha256:h(
      "evidence-is-not-a-video; mathematical test only"),
      clip_start_sec:0,clip_end_sec:10,fps:30},
    truth:{kind:"human_independent",annotator:"fixture only",
      ...gt,annotations_sha256:h(JSON.stringify(gt))},
    runs:{},
    external:{cuda_ort_verified:true,docker_verified:true}
  };
  const runtime={total_wall_sec:12,peak_rss_mb:140,peak_vram_mb:3000};
  const base={status:"complete",asset_sha256:record.asset.sha256,
    clip_start_sec:0,clip_end_sec:10,fps:30,
    model_weights_sha256:h("test-weight"),settings_sha256:h("golden"),
    ...gt,predictions_sha256:h(JSON.stringify(gt)),performance:runtime,
    montage:{quality_verified:true,source_original_verified:true,audio_verified:true}};
  record.runs.python=structuredClone(base);
  record.runs.rust=structuredClone(base);
  let evaluated=await evaluate(record,file);
  assert.equal(evaluated.verdict,"GO_CANDIDATE",evaluated.reasons.join(" / "));
  assert.equal(evaluated.production_switch_allowed,false);
  assert.equal(evaluated.metrics.rust.touch.f1,1);
  record.runs.rust.tracks=swapped;
  record.runs.rust.predictions_sha256=h(JSON.stringify({
    touches:gt.touches,rallies:gt.rallies,tracks:swapped}));
  evaluated=await evaluate(record,file);
  assert.equal(evaluated.verdict,"NO_GO");
  assert.ok(evaluated.reasons.some(x=>x.includes("IDF1")));
  record.runs.rust.tracks=gt.tracks;
  record.runs.rust.predictions_sha256=h(JSON.stringify(gt));
  record.runs.rust.performance.total_wall_sec=24;
  evaluated=await evaluate(record,file);
  assert.ok(evaluated.reasons.some(x=>x.includes("Runtime")));
  record.runs.rust.performance.total_wall_sec=12;
  record.runs.rust.model_weights_sha256=h("different model");
  evaluated=await evaluate(record,file);
  assert.ok(evaluated.reasons.some(x=>x.includes("model weight SHA")));
  record.runs.rust.model_weights_sha256=base.model_weights_sha256;
  record.runs.rust.status="failed";
  evaluated=await evaluate(record,file);
  assert.ok(evaluated.reasons.some(x=>x.includes("not completed")));
  record.runs.rust.status="complete";
  record.truth.annotations_sha256=h("forged annotation bytes");
  evaluated=await evaluate(record,file);
  assert.ok(evaluated.reasons.some(x=>x.includes("ground truth receipt")));
  record.truth.annotations_sha256=h(JSON.stringify(gt));
  record.external.cuda_ort_verified=false;
  record.external.docker_verified=false;
  evaluated=await evaluate(record,file);
  assert.ok(evaluated.reasons.some(x=>x.includes("CUDA")));
  assert.ok(evaluated.reasons.some(x=>x.includes("Docker")));
  record.external.cuda_ort_verified=true;
  record.external.docker_verified=true;
  record.asset.sha256=h("a different video");
  evaluated=await evaluate(record,file);
  assert.ok(evaluated.reasons.some(x=>x.includes("Source file SHA")));
  assert.equal(evaluated.production_switch_allowed,false);
  console.log("Stage84 parity maths: Touch F1, rally IoU, globally mapped IDF1, SHA, label integrity, settings, time, RAM, VRAM, Docker/CUDA strict NO-GO passed");
}finally{rmSync(directory,{recursive:true,force:true});}
