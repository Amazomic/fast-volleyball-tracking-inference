#!/usr/bin/env node
/**
 * Independent human-labelled Python vs Rust full-match scorer.
 * Nothing observed on the model's output is ever silently used as truth.
 * A nonzero exit means NO-GO. All local paths are audited but redacted.
 *
 * node scripts/ci/benchmark_rust_parity_84.mjs --evidence annotations.json
 *   --source /verified/exact-video.mp4 --out /tmp/parity-report.json
 */
import {createHash} from "node:crypto";
import {createReadStream,readFileSync,writeFileSync,statSync} from "node:fs";
import {basename,resolve} from "node:path";
import {pathToFileURL} from "node:url";

const HASH=/^[a-f0-9]{64}$/;
const number=x=>typeof x==="number"&&Number.isFinite(x);
const bounded=(x,lo,hi)=>number(x)&&x>=lo&&x<=hi;
const id=x=>typeof x==="string"&&x.length>0&&x.length<=128;
function validate(cond,message){
  if(!cond)throw Error(message);
}
function checkTimes(rows,label,from,to){
  validate(Array.isArray(rows)&&rows.length<=30000,label+" events array invalid");
  for(const row of rows){
    validate(row&&bounded(row.time_sec,from,to),label+" invalid timestamp");
    if(row.player_id!==undefined)validate(id(row.player_id),label+" player ID invalid");
  }
}
function checkRallies(rows,label,from,to){
  validate(Array.isArray(rows)&&rows.length<=500,label+" rallies array invalid");
  for(const row of rows)validate(row&&bounded(row.start_sec,from,to)&&
    bounded(row.end_sec,from,to)&&row.end_sec>row.start_sec,
    label+" invalid rally boundaries");
}
function checkTracks(rows,label,maxFrame){
  validate(Array.isArray(rows)&&rows.length<=250000,label+" tracks array invalid");
  const observed=new Set();
  for(const row of rows){
    validate(row&&Number.isInteger(row.frame)&&row.frame>=0&&row.frame<maxFrame&&
      id(row.person_id)&&Array.isArray(row.xywh)&&row.xywh.length===4&&
      row.xywh.every(v=>bounded(v,0,1))&&
      row.xywh[2]>0&&row.xywh[3]>0&&
      row.xywh[0]+row.xywh[2]<=1.001&&row.xywh[1]+row.xywh[3]<=1.001,
      label+" invalid normalized frame or box");
    const key=row.frame+":"+row.person_id;
    validate(!observed.has(key),label+" duplicate identity in same frame");
    observed.add(key);
  }
}
export const harmonic=(tp,fp,fn)=>({
  tp,fp,fn,
  precision:tp+fp?tp/(tp+fp):0,
  recall:tp+fn?tp/(tp+fn):0,
  f1:2*tp+fp+fn?2*tp/(2*tp+fp+fn):0
});
export function touches(gt,found,tolerance=0.15){
  validate(bounded(tolerance,0.01,0.5),"Bad touch tolerance");
  const truth=[...gt].sort((a,b)=>a.time_sec-b.time_sec);
  const pred=[...found].sort((a,b)=>a.time_sec-b.time_sec);
  let i=0,j=0,tp=0,playerCompared=0,playerCorrect=0,error=0;
  while(i<truth.length&&j<pred.length){
    const delta=pred[j].time_sec-truth[i].time_sec;
    if(Math.abs(delta)<=tolerance){
      tp++;error+=Math.abs(delta);
      if(truth[i].player_id!==undefined&&pred[j].player_id!==undefined){
        playerCompared++;
        if(truth[i].player_id===pred[j].player_id)playerCorrect++;
      }
      i++;j++;
    }else if(delta < -tolerance){j++}
    else{i++}
  }
  return {...harmonic(tp,pred.length-tp,truth.length-tp),
    mean_abs_error_sec:tp?error/tp:null,
    player_attribution_accuracy:playerCompared?playerCorrect/playerCompared:null,
    player_matched:playerCompared};
}
const intervalIou=(a,b)=>{
  const intersection=Math.max(0,Math.min(a.end_sec,b.end_sec)-
    Math.max(a.start_sec,b.start_sec));
  const union=(a.end_sec-a.start_sec)+(b.end_sec-b.start_sec)-intersection;
  return union>0?intersection/union:0;
};
export function rallies(gt,found,minIou=0.5){
  validate(bounded(minIou,0.1,0.95),"Bad rally IoU threshold");
  const edges=gt.map(a=>found.map((b,j)=>({j,iou:intervalIou(a,b)}))
    .filter(edge=>edge.iou>=minIou)
    .sort((a,b)=>b.iou-a.iou || a.j-b.j));
  const assigned=Array(found.length).fill(-1);
  function tryAssign(i,seen){
    for(const edge of edges[i]){
      if(seen.has(edge.j))continue;
      seen.add(edge.j);
      if(assigned[edge.j]<0||tryAssign(assigned[edge.j],seen)){
        assigned[edge.j]=i;return true;
      }
    }
    return false;
  }
  let tp=0;
  for(let i=0;i<gt.length;i++)if(tryAssign(i,new Set()))tp++;
  let startError=0,endError=0,meanIou=0;
  for(let j=0;j<assigned.length;j++){
    const i=assigned[j];if(i<0)continue;
    startError+=Math.abs(gt[i].start_sec-found[j].start_sec);
    endError+=Math.abs(gt[i].end_sec-found[j].end_sec);
    meanIou+=intervalIou(gt[i],found[j]);
  }
  return {...harmonic(tp,found.length-tp,gt.length-tp),
    mean_iou:tp?meanIou/tp:null,
    start_mae_sec:tp?startError/tp:null,end_mae_sec:tp?endError/tp:null};
}
function boxIou(a,b){
  const [x1,y1,w1,h1]=a,[x2,y2,w2,h2]=b;
  const ix=Math.max(0,Math.min(x1+w1,x2+w2)-Math.max(x1,x2));
  const iy=Math.max(0,Math.min(y1+h1,y2+h2)-Math.max(y1,y2));
  const intersection=ix*iy;
  return intersection/(w1*h1+w2*h2-intersection)||0;
}
// Hungarian maximum assignment: IDTP is NOT frame detection TP; it counts
// correctly attributed identities under a global one-to-one mapping.
function optimalAssignment(weights,n,m){
  const size=Math.max(n,m);
  if(!size)return 0;
  validate(size<=256,"Too many identity IDs for bounded benchmark");
  const maximum=weights.reduce((high,row)=>
    Math.max(high,...row),0);
  const u=new Float64Array(size+1),v=new Float64Array(size+1);
  const p=new Int32Array(size+1),way=new Int32Array(size+1);
  for(let i=1;i<=size;i++){
    p[0]=i;
    const minv=new Float64Array(size+1).fill(Number.POSITIVE_INFINITY);
    const used=new Uint8Array(size+1);
    let j0=0;
    do{
      used[j0]=1;
      const i0=p[j0],weightRow=i0<=n?weights[i0-1]:null;
      let delta=Infinity,j1=0;
      for(let j=1;j<=size;j++){
        if(used[j])continue;
        const cost=maximum-(j<=m && weightRow?weightRow[j-1]:0);
        const current=cost-u[i0]-v[j];
        if(current<minv[j]){minv[j]=current;way[j]=j0}
        if(minv[j]<delta){delta=minv[j];j1=j}
      }
      for(let j=0;j<=size;j++){
        if(used[j]){u[p[j]]+=delta;v[j]-=delta}
        else{minv[j]-=delta}
      }
      j0=j1;
    }while(p[j0]!==0);
    do{
      const j1=way[j0];p[j0]=p[j1];j0=j1;
    }while(j0!==0);
  }
  let matched=0;
  for(let j=1;j<=size;j++)
    if(p[j]>0&&p[j]<=n&&j<=m)matched+=weights[p[j]-1][j-1];
  return matched;
}
export function tracking(gt,found,minIou=0.5){
  validate(bounded(minIou,0.1,0.95),"Bad box IoU threshold");
  // Annotators may sample frames rather than label every video frame.
  // Never count predicted boxes from UNANNOTATED frames as false IDs.
  const annotatedFrames=new Set(gt.map(row=>row.frame));
  found=found.filter(row=>annotatedFrames.has(row.frame));
  const truthByFrame=new Map(),predByFrame=new Map();
  for(const [rows,target] of [[gt,truthByFrame],[found,predByFrame]]){
    for(const item of rows){
      if(!target.has(item.frame))target.set(item.frame,[]);
      target.get(item.frame).push(item);
    }
  }
  const trueIds=[...new Set(gt.map(x=>x.person_id))].sort();
  const predIds=[...new Set(found.map(x=>x.person_id))].sort();
  const aIndex=new Map(trueIds.map((x,i)=>[x,i]));
  const bIndex=new Map(predIds.map((x,i)=>[x,i]));
  const weights=trueIds.map(()=>predIds.map(()=>0));
  let spatialMatches=0;
  for(const [frame,people] of truthByFrame){
    const detections=predByFrame.get(frame)||[];
    const pairs=[];
    for(let i=0;i<people.length;i++)for(let j=0;j<detections.length;j++){
      const overlap=boxIou(people[i].xywh,detections[j].xywh);
      if(overlap>=minIou)pairs.push({i,j,overlap});
    }
    pairs.sort((a,b)=>b.overlap-a.overlap||a.i-b.i||a.j-b.j);
    const usedTruth=new Set(),usedPred=new Set();
    for(const pair of pairs){
      if(usedTruth.has(pair.i)||usedPred.has(pair.j))continue;
      usedTruth.add(pair.i);usedPred.add(pair.j);spatialMatches++;
      weights[aIndex.get(people[pair.i].person_id)]
        [bIndex.get(detections[pair.j].person_id)]++;
    }
  }
  const idtp=optimalAssignment(weights,trueIds.length,predIds.length);
  const idfp=found.length-idtp,idfn=gt.length-idtp;
  return {
    idtp,idfp,idfn,
    id_precision:idtp+idfp?idtp/(idtp+idfp):0,
    id_recall:idtp+idfn?idtp/(idtp+idfn):0,
    idf1:2*idtp+idfp+idfn?2*idtp/(2*idtp+idfp+idfn):0,
    matched_boxes:spatialMatches,truth_boxes:gt.length,
    predicted_boxes:found.length,
    fragmentations_not_measured:true
  };
}
async function hashFile(file){
  const sha=createHash("sha256");
  for await(const block of createReadStream(file,{highWaterMark:1024*1024}))
    sha.update(block);
  return sha.digest("hex");
}
function runReady(run,evidence){
  if(!run||run.status!=="complete")return "Native run was not completed";
  if(run.asset_sha256!==evidence.asset.sha256)return "Run points to different video SHA";
  if(run.clip_start_sec!==evidence.asset.clip_start_sec||
    run.clip_end_sec!==evidence.asset.clip_end_sec||
    run.fps!==evidence.asset.fps)return "Run clip/fps does not match ground truth";
  if(run.predictions_sha256!==createHash("sha256")
    .update(JSON.stringify({touches:run.touches,rallies:run.rallies,tracks:run.tracks}))
    .digest("hex")) return "Native predictions SHA-256 receipt invalid";
  if(!HASH.test(run.model_weights_sha256||""))
    return "Run has no verifiable ONNX model weight SHA";
  for(const kind of ["touches","rallies","tracks"])
    if(!Array.isArray(run[kind]))return "Missing native "+kind+" output";
  return null;
}
function measurement(run,label){
  for(const key of ["total_wall_sec","peak_rss_mb","peak_vram_mb"])
    if(!number(run.performance?.[key])||run.performance[key]<=0)
      return label+": missing full-process "+key+" receipt";
  return null;
}
const round=value=>value===null?null:Number(value.toFixed(6));
export async function evaluate(manifest,sourcePath){
  validate(manifest?.schema==="volley-parity-evidence/1",
    "Invalid benchmark evidence schema");
  const asset=manifest.asset;
  validate(asset&&HASH.test(asset.sha256)&&id(asset.id),"Invalid source identity or SHA");
  validate(bounded(asset.clip_start_sec,0,1e7)&&
    bounded(asset.clip_end_sec,asset.clip_start_sec+1,1e7)&&
    bounded(asset.fps,1,120),"Invalid source time span/fps");
  const real=resolve(sourcePath);
  validate(statSync(real).isFile(),"Source is not a regular video file");
  const observed=await hashFile(real);
  const blockers=[];
  if(observed!==asset.sha256)blockers.push("Source file SHA-256 does not match evidence");
  const truth=manifest.truth;
  if(!truth||truth.kind!=="human_independent"||
      !id(truth.annotator)||!HASH.test(truth.annotations_sha256||"") ||
      truth.annotations_sha256!==createHash("sha256").update(JSON.stringify({
        touches:truth.touches,rallies:truth.rallies,tracks:truth.tracks
      })).digest("hex")){
    blockers.push("No independent human-labelled ground truth receipt");
  }
  const outputs={};
  for(const name of ["python","rust"]){
    const run=manifest.runs?.[name];
    const reason=runReady(run,manifest);
    if(reason){blockers.push(name+": "+reason);continue;}
    const resource=measurement(run,name);
    if(resource)blockers.push(resource);
    if(run.model_weights_sha256!==manifest.runs.python?.model_weights_sha256)
      blockers.push(name+": model weight SHA mismatch with Python");
    if(run.settings_sha256!==manifest.runs.python?.settings_sha256||
      !HASH.test(run.settings_sha256||""))
      blockers.push(name+": nonidentical or missing golden setting SHA");
    try{
      for(const kind of ["touches","rallies","tracks"]){
        if(kind==="touches")checkTimes(run[kind],name,
          asset.clip_start_sec,asset.clip_end_sec);
        if(kind==="rallies")checkRallies(run[kind],name,
          asset.clip_start_sec,asset.clip_end_sec);
        if(kind==="tracks")checkTracks(run[kind],name,
          Math.ceil((asset.clip_end_sec-asset.clip_start_sec)*asset.fps));
      }
    }catch(err){blockers.push(name+": "+err.message);continue;}
    outputs[name]=run;
  }
  const metrics={};
  if(truth?.kind==="human_independent"&&
     id(truth.annotator)&&
     truth.annotations_sha256===createHash("sha256")
       .update(JSON.stringify({touches:truth.touches,
         rallies:truth.rallies,tracks:truth.tracks})).digest("hex")){
    try{
      const span=Math.ceil((asset.clip_end_sec-asset.clip_start_sec)*asset.fps);
      checkTimes(truth.touches,"truth",asset.clip_start_sec,asset.clip_end_sec);
      checkRallies(truth.rallies,"truth",asset.clip_start_sec,asset.clip_end_sec);
      checkTracks(truth.tracks,"truth",span);
      if(!truth.touches.length||!truth.rallies.length||!truth.tracks.length)
        throw Error("Human annotations missing touches, rallies or track IDs");
      for(const name of ["python","rust"])if(outputs[name]){
        metrics[name]={
          touch:touches(truth.touches,outputs[name].touches),
          rally:rallies(truth.rallies,outputs[name].rallies),
          tracking:tracking(truth.tracks,outputs[name].tracks)
        };
      }
    }catch(error){blockers.push("Ground truth: "+error.message)}
  }
  if(!metrics.python||!metrics.rust)blockers.push("No comparable Python and Rust quality metrics");
  let comparison=null;
  if(metrics.python&&metrics.rust&&!blockers.length){
    const py=metrics.python,rs=metrics.rust;
    const pyPerf=outputs.python.performance,rsPerf=outputs.rust.performance;
    comparison={
      touch_f1_delta:round(rs.touch.f1-py.touch.f1),
      track_idf1_delta:round(rs.tracking.idf1-py.tracking.idf1),
      rally_f1_delta:round(rs.rally.f1-py.rally.f1),
      runtime_ratio:round(rsPerf.total_wall_sec/pyPerf.total_wall_sec),
      ram_ratio:round(rsPerf.peak_rss_mb/pyPerf.peak_rss_mb),
      vram_ratio:round(rsPerf.peak_vram_mb/pyPerf.peak_vram_mb),
      verified_montage:outputs.rust.montage?.quality_verified===true &&
        outputs.rust.montage?.source_original_verified===true &&
        outputs.rust.montage?.audio_verified===true &&
        outputs.python.montage?.quality_verified===true
    };
    if(comparison.touch_f1_delta < -0.02)blockers.push("Touch F1 degrades more than 2pp");
    if(comparison.track_idf1_delta < -0.02)blockers.push("IDF1 degrades more than 2pp");
    if(comparison.rally_f1_delta < -0.02)blockers.push("Rally F1 degrades more than 2pp");
    if(comparison.runtime_ratio>1.10)blockers.push("Runtime >110% of Python");
    if(comparison.ram_ratio>1.10)blockers.push("RAM >110% of Python");
    if(comparison.vram_ratio>1.10)blockers.push("VRAM >110% of Python");
    if(!comparison.verified_montage)blockers.push("No verified original-source montage with audio");
  }
  if(manifest.external?.cuda_ort_verified!==true)
    blockers.push("Linux CUDA ONNX Runtime end-to-end not verified");
  if(manifest.external?.docker_verified!==true)
    blockers.push("Actual Docker runtime image not verified");
  return {schema:"volley-rust-parity-report/1",
    asset:{id:asset.id,sha256:asset.sha256,clip_start_sec:asset.clip_start_sec,
      clip_end_sec:asset.clip_end_sec,fps:asset.fps,basename:basename(real)},
    verdict:blockers.length?"NO_GO":"GO_CANDIDATE",
    production_switch_allowed:false,
    reasons:[...new Set(blockers)],metrics,comparison,
    note:"GO_CANDIDATE still requires independent stage85 review; never auto-switch."};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href){
  const argv=process.argv.slice(2);
  const option=name=>{const n=argv.indexOf(name);return n>=0?argv[n+1]:null};
  const evidence=option("--evidence"),source=option("--source"),out=option("--out");
  if(!evidence||!source)throw Error("Usage: --evidence FILE --source EXACT_MP4 [--out FILE]");
  const report=await evaluate(JSON.parse(readFileSync(evidence,"utf8")),source);
  const json=JSON.stringify(report,null,2)+"\n";
  if(out)writeFileSync(out,json);
  console.log(json);
  process.exitCode=report.verdict==="NO_GO"?3:0;
}
