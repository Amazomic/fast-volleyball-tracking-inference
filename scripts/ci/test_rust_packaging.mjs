import {readFileSync,statSync} from "node:fs";
import {join} from "node:path";
import {execFileSync} from "node:child_process";
import {fileURLToPath} from "node:url";
import assert from "node:assert/strict";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const attrs=readFileSync(join(root,".gitattributes"),"utf8");
assert.match(attrs,/^\*\.sh text eol=lf$/m,
  "Windows autocrlf=true must not corrupt WSL Bash scripts");
const content=path=>readFileSync(join(root,path),"utf8");
for(const script of ["scripts/volley-rust.sh",
  "scripts/ci/test_rust_lifecycle.sh","scripts/ci/test_rust_release_smoke.sh",
  "scripts/ci/test_rust_docker_smoke.sh",
  "scripts/ci/test_rust_faults.sh","scripts/ci/test_rust_shutdown_race.sh",
  "scripts/ci/build_rust_docker_local.sh"]){
  execFileSync("bash",["-n",join(root,script)]);
  assert.ok(!readFileSync(join(root,script)).includes(13),
    "CRLF corrupts Bash under Windows autocrlf: "+script);
}
const ortInstall=content("scripts/setup_rust_ort_cuda.sh");
assert.ok(!readFileSync(join(root,"scripts/setup_rust_ort_cuda.sh")).includes(13),
  "ORT install script must retain LF in Windows checkouts");
execFileSync("bash",["-n",join(root,"scripts/setup_rust_ort_cuda.sh")]);
const ortSmoke=content("scripts/ci/test_rust_cuda_runtime.sh");
assert.ok(!readFileSync(join(root,"scripts/ci/test_rust_cuda_runtime.sh")).includes(13),
  "CUDA smoke script must retain LF in Windows checkouts");
execFileSync("bash",["-n",join(root,"scripts/ci/test_rust_cuda_runtime.sh")]);
for(const value of ["onnxruntime-linux-x64-gpu-1.22.0.tgz",
  "2a19dbfa403672ec27378c3d40a68f793ac7a6327712cd0e8240a86be2b10c55",
  "onnxruntime-gpu[cuda,cudnn]==1.22.0","--target"]){
  assert.ok(ortInstall.includes(value),"Pinned native CUDA installer guard missing: "+value);
}
assert.ok(ortSmoke.includes("VOLLEY_TEST_ORT_DEVICE=cuda"));
assert.ok(ortSmoke.includes("native_onnx_clip_infers_and_decodes"));
const infer=content("rust/volley-inference/src/lib.rs");
assert.match(infer,/fn runtime_library\(/);
assert.match(infer,/libonnxruntime_providers_cuda\.so/);
assert.match(infer,/scripts\/setup_rust_ort_cuda\.sh/);const daemon=content("rust/volley-app/src/runtime.rs");
assert.match(daemon,/Sha256::digest/);
assert.match(daemon,/try_lock_exclusive/);
for(const guard of ["ShutdownGate","try_shutdown","validate_local_request",
  "inflight","sec-fetch-site","Rust shutdown denied"]){
  assert.ok(daemon.includes(guard),"Missing stage83 HTTP shutdown/CSRF guard: "+guard);
}
assert.match(daemon,/Another Volleyball Studio Rust instance/);
const server=content("rust/volley-app/src/main.rs");
assert.match(server,/from_fn_with_state\(\s*gate,\s*runtime::guard_request,/,
  "All Rust HTTP requests must cross the stage83 shutdown gate");
for(const term of ["runtime::exclusive_instance","runtime::rewrite_status",
  "shutdown_signal","SignalKind::terminate",
  "with_graceful_shutdown(shutdown_signal(state, gate))",
  "app_with_gate"]){
  assert.ok(server.includes(term),"Missing safe runtime policy: "+term);
}
for(const term of ["shutdown_safe","active_jobs","active_job_types"]){
  assert.ok(daemon.includes(term),"Missing lifecycle source ownership: "+term);
}
const bash=content("scripts/volley-rust.sh");
for(const command of ["start|stop|status|restart|doctor|check",
  "shutdown_safe","active_jobs","/proc/$pid/exe","/proc/$pid/cwd",
  "kill -TERM","cargo build --locked --offline","exec \"$binary\"",
  "VOLLEY_RUST_DATA_ROOT","ORT_DYLIB_PATH"]){
  assert.ok(bash.includes(command),"Missing Rust deployment contract: "+command);
}
for(const detail of ['"$debug (deleted)"','"$release (deleted)"',
  'IFS= read -r -d',
  '[[ "$cmdline" == "$debug" || "$cmdline" == "$release" ]]']){
  assert.ok(bash.includes(detail),
    'Hot-rebuilt Rust executable must still be safely stoppable: '+detail);
}
assert.doesNotMatch(bash,/kill -KILL|kill -9|pkill|taskkill|python\d?\s/);
const ps=content("scripts/start-volley-rust-dev.ps1");
assert.match(ps,/Ubuntu-25\.04/);
assert.match(ps,/127\.0\.0\.1:8875/);
assert.match(ps,/wsl\.exe -d \$Distro --exec bash/);
assert.match(ps,/ValidateSet\("Start","Status","Stop","Restart","Doctor","Check"\)/);
assert.doesNotMatch(ps,/Stop-Process|taskkill|python\.exe/);
const cmd=content("start-volley-ui.cmd");
for(const action of ["/restart","/status","/stop","/doctor","/check"]){
  assert.ok(cmd.includes(action),"Windows one-click entrypoint missing "+action);
}
const docker=content("Dockerfile");
assert.match(docker,/FROM debian:trixie-slim AS runtime-base/,
  "Prebuilt Ubuntu 25.04 ELF requires glibc >=2.39 (Bookworm is insufficient)");
for(const fragment of ["FROM rust:1-bookworm AS build","FROM runtime-base AS prebuilt",
  "FROM runtime-base AS final","--locked --release -p volley-app",
  "USER volley","STOPSIGNAL SIGTERM","127.0.0.1:8875",
  "COPY models/ /app/models/",
  "COPY prebuilt/volley-app /usr/local/bin/volley-app"]){
  assert.ok(docker.includes(fragment),"Missing secure Docker contract "+fragment);
}
assert.doesNotMatch(docker,/\buv\b|pip install|COPY results\/|COPY project\/|0\.0\.0\.0:8875/);
if(process.platform==="linux"){
  const executable=join(root,"rust/target/release/volley-app");
  try{
    if(statSync(executable).isFile()){
      const symbols=execFileSync("objdump",["-T",executable],{encoding:"utf8"});
      const needs=[...symbols.matchAll(/GLIBC_(\d+)\.(\d+)/g)].map(m=>
        Number(m[1])*1000+Number(m[2]));
      assert.ok(needs.length,"No glibc version references in native Linux release");
      const high=Math.max(...needs);
      assert.ok(high<=2041,"Native ELF requires a glibc newer than Debian Trixie 2.41");
      console.log("Prebuilt Rust ELF glibc requirement:",Math.floor(high/1000)+"."+String(high%1000).padStart(2,"0"));
    }
  }catch(e){
    if(e.code!=="ENOENT")throw e;
    console.log("Skip ELF ABI audit until a Linux release binary/objdump is available");
  }
}
const ignore=content(".dockerignore");
for(const secret of ["rust/target/","results/","project/","uploads/","*.mp4","*.mov"]){
  assert.ok(ignore.includes(secret),"Docker context must exclude "+secret);
}
const staging=content("scripts/ci/build_rust_docker_local.sh");
for(const fragment of ["--target prebuilt","mktemp -d","trap 'rm -rf",
  "cp \"$release\"","--locked --offline --release"]){
  assert.ok(staging.includes(fragment),"Local Docker context not isolated: "+fragment);
}
const wasm=join(root,"rust/volley-app/static/volley_web.wasm");
assert.ok(statSync(wasm).size>1000);
console.log("Rust packaging: one instance, guarded stop/restart, LF Bash, Windows controls, non-root Docker targets, private build context and bundled WASM verified");
