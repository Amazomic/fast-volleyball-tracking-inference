#!/usr/bin/env node
// Read-only archive inventory, not a claim of Python accuracy or fair parity.
// Old model predictions cannot serve as their own manual ground truth.
import assert from "node:assert/strict";
import {createHash} from "node:crypto";
import {readFileSync,statSync,writeFileSync} from "node:fs";
import {resolve,join,basename} from "node:path";
import {fileURLToPath} from "node:url";
const root=resolve(fileURLToPath(new URL("../..",import.meta.url)));
const relative="results/cpu-calibration/rally74-excerpt";
const saved=join(root,relative);
const selected="0433f43d2a43ca7de55fb5fe";
const post=join(saved,"postprocessing",selected);
const read=file=>JSON.parse(readFileSync(file,"utf8"));
const manifest=read(join(saved,"inference.manifest.json"));
const complete=read(join(post,"events.complete.json"));
const eventsPath=join(post,"analysis","events.json");
const original=readFileSync(eventsPath);
const checksum=createHash("sha256").update(original).digest("hex");
const expected=Object.entries(complete.outputs||{}).filter(([path])=>
  path.replaceAll("\\","/").endsWith("/analysis/events.json"));
assert.equal(expected.length,1,"Python event receipt missing or ambiguous");
assert.equal(checksum,expected[0][1],"Historical Python event SHA mismatch");
const events=JSON.parse(original.toString("utf8"));
assert.equal(events.schema,"volleyball-events/2");
assert(Array.isArray(events.events),"Missing archived predictions");
const types={};
for(const event of events.events){
  assert.equal(event.source,manifest.source,"Python source artifact mismatch");
  types[event.type]=(types[event.type]||0)+1;
}
const video=read(join(root,"docs/RUST_BENCHMARK_84_MEDIA.json"));
const observedSource=video.inputs.source_sha256;
const report={
  schema:"volley-python-baseline-audit/1",
  result_kind:"archived_predictions_not_human_truth",
  source_basename:basename(manifest.source),
  archived_inference_source_sha256:manifest.input_sha256,
  archived_events_sha256:checksum,
  archived_event_receipt_verified:true,
  historical_postprocess_events:events.events.length,
  event_types:types,
  historical_event_stage_seconds:complete.seconds,
  event_stage_only_not_end_to_end:true,
  distinct_from_rust_montage_source:
    observedSource!==manifest.input_sha256,
  comparative_quality_available:false,
  historical_input_video_not_provided_by_this_audit:true,
  caution:"Python predictions are heuristics, not independent labelled truth; source differs from the Rust montage capture and none of these saved timings are full inference throughput."
};
assert.equal(report.distinct_from_rust_montage_source,true,
  "Archived Python result suddenly shares source digest: provenance must be rechecked");
assert.equal(report.historical_postprocess_events,21,
  "Historical fixture changed; do not silently report old observation");
const idx=process.argv.indexOf("--out");
if(idx>=0)writeFileSync(resolve(process.argv[idx+1]),
  JSON.stringify(report,null,2)+"\n");
console.log(JSON.stringify(report,null,2));
