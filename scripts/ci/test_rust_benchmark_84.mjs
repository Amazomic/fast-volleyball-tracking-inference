#!/usr/bin/env node
// Stage84 harness validity is green only for instrumentation; acceptance
// remains NO_GO until independent labels and real matching GPU runs exist.
import assert from "node:assert/strict";
import {spawnSync} from "node:child_process";
import {readFileSync,mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {resolve,join} from "node:path";
import {fileURLToPath} from "node:url";
const root=resolve(fileURLToPath(new URL("../..",import.meta.url)));
const run=(script,args=[],expected=0)=>{
  const result=spawnSync("node",[join(root,"scripts/ci",script),...args],
    {encoding:"utf8",timeout:240000,maxBuffer:5*1024*1024});
  if(result.error||result.status!==expected)
    throw Error(script+" returned "+result.status+" expected "+expected+
      ": "+(result.stderr||result.error||"").slice(-2000));
  return result.stdout;
};
const strict=process.argv.includes("--require-parity");
const source=join(root,"results/prepared/fa5883350411c05b62783a4b43ac.mp4");
const report=join(root,"docs/RUST_BENCHMARK_84_GATE.json");
const benchmark=JSON.parse(readFileSync(report,"utf8"));
assert.equal(benchmark.schema,"volley-rust-parity-report/1");
assert.equal(benchmark.verdict,"NO_GO");
assert.equal(benchmark.production_switch_allowed,false);
const baseline=JSON.parse(readFileSync(
  join(root,"docs/RUST_BENCHMARK_84_PYTHON.json"),"utf8"));
assert.equal(baseline.archived_event_receipt_verified,true);
assert.equal(baseline.comparative_quality_available,false);
const media=JSON.parse(readFileSync(
  join(root,"docs/RUST_BENCHMARK_84_MEDIA.json"),"utf8"));
assert.equal(media.montage.state,"passed");
assert.equal(media.montage.provenance_checked,true);
assert.equal(media.montage.codec_video,"h264");
assert.equal(media.montage.codec_audio,"aac");
assert.equal(media.montage.decoded_video_frames,180);
assert.ok(media.montage.frame_ssim_against_source>=0.99,
  "Real original-frame SSIM fell below recorded stage84 acceptance");
assert.equal(media.inputs.source_sha256,benchmark.asset.sha256);
console.log(run("test_rust_parity_84.mjs").trim());
const temp=mkdtempSync(join(tmpdir(),"volley-accept84-"));
try{
  const stored=join(temp,"parity.json");
  if(process.argv.includes("--with-media")){
    console.log(run("test_rust_real_media_84.mjs",[
      "--out",join(temp,"media.json")]).trim());
    console.log(run("audit_python_baseline_84.mjs",[
      "--out",join(temp,"python.json")]).trim());
  }
  const output=run("benchmark_rust_parity_84.mjs",[
    "--evidence",join(root,"docs/RUST_BENCHMARK_84_EVIDENCE.json"),
    "--source",source,"--out",stored],3);
  const current=JSON.parse(readFileSync(stored,"utf8"));
  assert.equal(current.verdict,"NO_GO");
  assert.equal(current.production_switch_allowed,false);
  assert.deepEqual(current.reasons,benchmark.reasons,
    "Reproducible no-go reasons differ from committed stage84 verdict");
  console.log("Stage84 real source SHA gate: "+current.asset.sha256);
  console.log("Stage84 acceptance verdict: NO_GO");
  console.log("Blocked: human labels, same-source Python/Rust inference, Linux CUDA ORT, container smoke, full-match CPU/GPU/VRAM peaks");
  if(strict){console.error("Production switch forbidden");process.exitCode=3;}
}finally{rmSync(temp,{recursive:true,force:true});}
