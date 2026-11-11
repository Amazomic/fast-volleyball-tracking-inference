#!/usr/bin/env node
// Opt-in real captured footage benchmark, never modify the original project.
// Run on WSL/Linux with a built Rust release and FFmpeg/FFprobe available.
import assert from "node:assert/strict";
import {createHash,randomUUID} from "node:crypto";
import {spawn,spawnSync} from "node:child_process";
import {readFileSync,writeFileSync,mkdirSync,existsSync,symlinkSync,rmSync,mkdtempSync,readdirSync,statSync,createReadStream} from "node:fs";
import {tmpdir} from "node:os";
import {join,resolve} from "node:path";
import {fileURLToPath} from "node:url";
import net from "node:net";
const root=resolve(fileURLToPath(new URL("../..",import.meta.url)));
const source=resolve(process.env.VOLLEY_BENCH84_SOURCE||
  join(root,"results/prepared/fa5883350411c05b62783a4b43ac.mp4"));
const executable=join(root,"rust/target/release/volley-app");
const skip=process.argv.includes("--optional");
const outputArg=process.argv.indexOf("--out");
const reportPath=outputArg>=0?resolve(process.argv[outputArg+1]):null;
function requireTool(tool){
  const outcome=spawnSync(tool,["-version"],{encoding:"utf8",timeout:10000});
  if(outcome.error||outcome.status!==0)throw Error(tool+" not available");
}
if(!existsSync(source)||!existsSync(executable)){
  const reason="Real captured source or Rust release binary absent";
  if(skip){console.log("SKIP: "+reason);process.exit(0);}
  throw Error(reason);
}
requireTool("ffmpeg");requireTool("ffprobe");
const run=(command,args,timeout=120000)=>{
  const launched=spawnSync(command,args,{encoding:"utf8",maxBuffer:6*1024*1024,timeout});
  if(launched.error||launched.status!==0)throw Error(
    command+" failed: "+String(launched.stderr||launched.error).slice(-2400));
  return launched.stdout;
};
const probe=path=>JSON.parse(run("ffprobe",[
  "-v","error","-show_entries",
  "format=duration,size:stream=codec_type,codec_name,width,height,avg_frame_rate",
  "-of","json",path],20000));
const sleep=ms=>new Promise(ok=>setTimeout(ok,ms));
async function sha(path){
  const hash=createHash("sha256");
  for await(const chunk of createReadStream(path,{highWaterMark:1024*1024}))hash.update(chunk);
  return hash.digest("hex");
}
async function availablePort(){
  return await new Promise((ok,fail)=>{
    const server=net.createServer();
    server.once("error",fail);
    server.listen(0,"127.0.0.1",()=>{
      const port=server.address().port;
      server.close(()=>ok(port));
    });
  });
}
async function request(port,pathname,options={}){
  const abort=AbortSignal.timeout(4500);
  const response=await fetch("http://127.0.0.1:"+port+pathname,{
    ...options,headers:{"content-type":"application/json",...options.headers},
    signal:abort
  });
  const data=await response.json();
  if(!response.ok)throw Error(pathname+" HTTP "+response.status+": "+JSON.stringify(data));
  return data;
}
const record={
  schema:"volley-rust-real-media-benchmark/1",
  artifact_type:"observed_media_only",
  inputs:{source_kind:"existing_python_prepared_video",source_size_bytes:statSync(source).size,
    // Do not commit machine-specific private absolute paths
    source_basename:source.split(/[\\/]/).at(-1),
    clip_start_sec:12,selected_frames:180,analysis_fps:30},
  montage:{state:"not_run"},accuracy:{touch_f1:null,track_idf1:null,rally_f1:null,
    reason:"Independent manual labels and comparable full native GPU inference not available"},
  gpu:{cuda_inference:false,peak_vram_mb:null}
};
let tmp=null,server=null,port=null,logs="",peakRss=0,
  peakTreeRss=0,peakChildRss=0,monitor=null,shutdown=false;
function processTreeRss(pid,seen=new Set()){
  if(!Number.isInteger(pid)||pid<=0||seen.has(pid))return {total:0,child:0};
  seen.add(pid);
  let own=0;
  try{const data=readFileSync("/proc/"+pid+"/status","utf8");
    own=Number(data.match(/^VmRSS:\s+(\d+)/m)?.[1]||0)*1024;
  }catch{}
  let children=[];
  try{children=readFileSync("/proc/"+pid+"/task/"+pid+"/children","utf8")
    .trim().split(/\s+/).filter(Boolean).map(Number);
  }catch{}
  let total=own,child=0;
  for(const childPid of children){
    const item=processTreeRss(childPid,seen);total+=item.total;
    child+=item.total;
  }
  return {total,child};
}
try {
  const data=probe(source);
  const stream=data.streams.find(x=>x.codec_type==="video");
  assert(stream&&stream.width===1280&&stream.height===720,
    "Unexpected source geometry");
  assert(data.streams.some(x=>x.codec_type==="audio"),"Real source lacks original audio");
  assert(Number(data.format.duration)>19,"Captured footage too short");
  record.inputs.source_sha256=await sha(source);
  record.inputs.source_video={width:stream.width,height:stream.height,
    duration_sec:Number(data.format.duration),fps:stream.avg_frame_rate};
  tmp=mkdtempSync(join(tmpdir(),"volley-real84-"));
  const repository=join(tmp,"source-root");
  const dataRoot=join(tmp,"rust-root");
  const id="84b84b84b84b";
  mkdirSync(join(repository,"project"),{recursive:true});
  mkdirSync(join(dataRoot,"projects",id),{recursive:true});
  mkdirSync(join(dataRoot,"uploads","music"),{recursive:true});
  const originalFile=join(repository,"project","bench84.mp4");
  symlinkSync(source,originalFile);
  const original={
    id,name:"Ephemeral benchmark 84",videos:["bench84.mp4"],
    source_paths:{"bench84.mp4":source},ranges:{}
  };
  writeFileSync(join(dataRoot,"projects","projects.json"),
    JSON.stringify({schema:1,projects:[original]}));
  writeFileSync(join(dataRoot,"projects",id,"project.json"),JSON.stringify(original));
  const soundtrack="track_"+randomUUID().replaceAll("-","")+".m4a";
  const musicPath=join(dataRoot,"uploads","music",soundtrack);
  run("ffmpeg",["-nostdin","-hide_banner","-loglevel","error","-y",
    "-ss","12","-i",source,"-t","7","-vn","-c:a","aac","-b:a","128k",musicPath],60000);
  record.inputs.soundtrack_duration_sec=Number(probe(musicPath).format.duration);
  port=await availablePort();
  const env={...process.env,VOLLEY_REPOSITORY_ROOT:repository,
    VOLLEY_RUST_DATA_ROOT:dataRoot,VOLLEY_RUST_BIND:"127.0.0.1:"+port};
  server=spawn(executable,[],{env,cwd:join(root,"rust"),stdio:["ignore","pipe","pipe"]});
  for(const output of [server.stdout,server.stderr]){
    output.setEncoding("utf8");
    output.on("data",piece=>{logs=(logs+piece).slice(-6000);});
  }
  const startUntil=Date.now()+12000;
  let healthy=false;
  while(Date.now()<startUntil){
    if(server.exitCode!==null)throw Error("Rust server exited during startup: "+logs);
    try{
      const response=await request(port,"/internal/rewrite-status");
      if(response.production_ready===false && response.isolated_writes_enabled===true){
        healthy=true;break;
      }
    }catch{}
    await sleep(140);
  }
  if(!healthy)throw Error("Rust sandbox server never became healthy: "+logs);
  monitor=setInterval(()=>{
    try{
      const processStatus=readFileSync("/proc/"+server.pid+"/status","utf8");
      const value=processStatus.match(/^VmRSS:\s+(\d+)/m);
      if(value)peakRss=Math.max(peakRss,Number(value[1])*1024);
      const tree=processTreeRss(server.pid);
      peakTreeRss=Math.max(peakTreeRss,tree.total);
      peakChildRss=Math.max(peakChildRss,tree.child);
    }catch{}
  },35);
  const clips=[{video:"bench84.mp4",start_frame:360,end_frame:539,analysis_offset_sec:0}];
  const started=performance.now();
  const job=await request(port,"/api/montage/native/start",{
    method:"POST",body:JSON.stringify({project_id:id,clips,
      audio_file:soundtrack,encoder:"cpu",layout:"vertical"})
  });
  assert.match(job.job_id,/^montage_[0-9a-f]{32}$/);
  let status;
  const deadline=Date.now()+180000;
  while(Date.now()<deadline){
    status=await request(port,"/api/montage/native/status");
    if(["complete","failed","cancelled"].includes(status.state))break;
    await sleep(230);
  }
  assert.equal(status?.state,"complete",JSON.stringify(status));
  const wall=(performance.now()-started)/1000;
  clearInterval(monitor);monitor=null;
  const folder=join(dataRoot,"projects",id,"montages",job.job_id);
  const output=join(folder,"montage.mp4");
  const receipt=JSON.parse(readFileSync(join(folder,"selection.json"),"utf8"));
  const result=probe(output);
  const video=result.streams.find(x=>x.codec_type==="video");
  const audio=result.streams.find(x=>x.codec_type==="audio");
  assert(video&&video.codec_name==="h264","No H264");
  assert(audio&&audio.codec_name==="aac","No AAC soundtrack");
  assert(video.width*16===video.height*9 ||
    Math.abs(video.width/video.height-9/16)<0.004,
    "Not vertical 9:16");
  assert(Math.abs(Number(result.format.duration)-6)<0.35,"Incorrect clip duration");
  assert.equal(receipt.render_source,"original_recording");
  assert.equal(receipt.source_clips_native_resolution_verified,true);
  assert.equal(receipt.layout,"vertical");
  assert.equal(receipt.selected.length,1);
  assert.equal(receipt.selected[0].native_width,1280);
  assert.equal(receipt.selected[0].native_height,720);
  assert.equal(receipt.selected[0].frames,180);
  assert.equal(receipt.mp4_quality.has_audio,true);
  run("ffmpeg",["-nostdin","-hide_banner","-loglevel","error",
    "-i",output,"-f","null","-"],60000);
  const count=run("ffprobe",["-v","error","-count_frames",
    "-select_streams","v:0","-show_entries","stream=nb_read_frames",
    "-of","default=noprint_wrappers=1:nokey=1",output],20000).trim();
  assert.equal(Number(count),180,"Lost or duplicated rendered source frames");
  // Direct visual fidelity against the SAME 12-18s original frames, fit
  // into the SAME letterboxed 9:16 canvas, without reusing Rust's output
  // as its own reference. PSNR/SSIM are frame-quality measurements, not
  // learned volleyball Touch/Track/Rally accuracy.
  const filter="[0:v]trim=duration=6,setpts=PTS-STARTPTS,fps=30,"+
    "scale="+video.width+":"+video.height+
    ":force_original_aspect_ratio=decrease:flags=lanczos,"+
    "pad="+video.width+":"+video.height+
    ":(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[ref];"+
    "[ref][1:v]ssim";
  const compare=spawnSync("ffmpeg",["-nostdin","-hide_banner",
    "-loglevel","info","-ss","12","-i",source,"-i",output,
    "-filter_complex",filter,"-an","-frames:v","180",
    "-f","null","-"],{encoding:"utf8",timeout:90000,maxBuffer:5*1024*1024});
  if(compare.status!==0)throw Error("Original frame fidelity SSIM could not be measured: "+
    String(compare.stderr||compare.error).slice(-1600));
  const match=String(compare.stderr).match(/SSIM Y:[^\\r\\n]+All:([0-9.]+)/);
  if(!match)throw Error("No visual frame SSIM in actual FFmpeg stats");
  const ssim=Number(match[1]);
  assert(ssim>=0.90 && ssim<=1,"Original-source visual SSIM below acceptable baseline");  record.montage={
    state:"passed",engine:"native_rust_montage_ffmpeg",
    wall_seconds:Number(wall.toFixed(3)),
    source_seconds:6,relative_to_realtime:Number((6/wall).toFixed(3)),
    peak_rust_server_rss_mib:Number((peakRss/(1024*1024)).toFixed(2)),
    peak_observed_process_tree_rss_mib:peakChildRss>0?
      Number((peakTreeRss/(1024*1024)).toFixed(2)):null,
    peak_observed_ffmpeg_children_rss_mib:peakChildRss>0?
      Number((peakChildRss/(1024*1024)).toFixed(2)):null,
    memory_sampling_caveat:peakChildRss>0?
      "35ms /proc RSS polling; process RSS includes shared memory, not a full-match peak":
      "35ms /proc sampling did not observe FFmpeg child memory; total pipeline RSS is UNMEASURED",
    output_size_bytes:statSync(output).size,
    output_sha256:await sha(output),
    duration_sec:Number(result.format.duration),codec_video:video.codec_name,
    codec_audio:audio.codec_name,output_width:video.width,
    output_height:video.height,original_source_frames_preserved:receipt.source_clips_native_resolution_verified,
    provenance_checked:true,full_file_decode_verified:true,
    frame_ssim_against_source:Number(ssim.toFixed(5)),
    decoded_video_frames:180,
    frame_ssim_provenance:"12-18s original, FFmpeg letterbox reference, 180 frames",
    limitations:"Render alone: not an end-to-end GPU/CPU inference or Python Studio throughput comparison"
  };
  console.log("Rust montage real saved footage: "+JSON.stringify(record.montage));
}finally{
  if(monitor)clearInterval(monitor);
  if(server && server.exitCode===null){
    server.kill("SIGTERM");shutdown=true;
    const close=await Promise.race([
      new Promise(ok=>server.once("exit",()=>ok(true))),
      sleep(5000).then(()=>false)
    ]);
    if(!close){
      // Do not SIGKILL a Rust server that may own active work.
      console.error("WARNING: Rust benchmark server did not exit after SIGTERM, manual review required");
      throw Error("Benchmark Rust server did not exit gracefully");
    }
  }
  if(tmp&&(shutdown||!server||server.exitCode!==null))
    rmSync(tmp,{recursive:true,force:true});
}
if(reportPath)writeFileSync(reportPath,JSON.stringify(record,null,2)+"\n");
console.log("Real media source SHA-256: "+record.inputs.source_sha256);
console.log("Stage84 video benchmark passed; no unverified model accuracy declared");
