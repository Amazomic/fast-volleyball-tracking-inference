import {readFileSync, existsSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import assert from "node:assert/strict";
const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const text=p=>readFileSync(join(root,p),"utf8");
const html=text("rust/volley-app/static/studio.html");
const native=text("rust/volley-app/src/main.rs");
const runtime=text("rust/volley-app/src/runtime.rs");
const ui=text("rust/volley-app/static/studio_ui.js");
const tabs=text("rust/volley-app/static/studio_tabs.js");
const guard=text("rust/volley-app/src/project_guard.rs");
for(const source of [html,native,runtime,tabs]){
  for(const removed of ["studio-nav-legacy","studio-legacy-view","/api/project-legacy/native","/api/project-archive/native","/studio/legacy.js"]){
    assert.ok(!source.includes(removed),"Removed feature still referenced: "+removed);
  }
}
assert.ok(!html.includes('data-studio-tab="legacy"'));
assert.ok(!ui.includes('"quick","legacy"'));
assert.ok(ui.includes('"preview","analysis","players","montage","quick"'));
for(const route of ["preview","analysis","players","report","montage","quick"]){
  assert.ok(html.includes('data-studio-tab="'+route+'"'),"Missing real nav "+route);
}
for(const file of [
  "rust/volley-app/static/studio_legacy.js",
  "rust/volley-app/src/legacy_projects.rs",
  "rust/volley-app/src/project_archive.rs",
  "rust/volley-app/src/project_archive_restore.rs"
]){
  assert.equal(existsSync(join(root,file)),false,"Obsolete module still present: "+file);
}
assert.ok(guard.includes("pub(crate) fn write_guard("));
assert.ok(guard.includes("try_lock_exclusive"));
console.log("Six routes and standalone project-write guard: obsolete archives and legacy imports removed");