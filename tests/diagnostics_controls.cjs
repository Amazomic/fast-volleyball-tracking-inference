"use strict";
const assert=require("node:assert/strict"),fs=require("node:fs"),vm=require("node:vm"),path=require("node:path");
const nodes=new Map();
const get=id=>{
  if(!nodes.has(id))nodes.set(id,{value:"",checked:false,disabled:false,dataset:{},open:false,
    handlers:{},addEventListener(type,fn){this.handlers[type]=fn;},
    classList:{toggle(){}},closest(){return null;}});
  return nodes.get(id);
};
const fields=["tracks_export_mode","tracks_padding","tracks_include_not_rally","tracks_mark_ball","tracks_debug","show_rally_snapshot","show_rally_track"];
const form=get("diagnostics-form");
form.elements={namedItem:get};
get("top-project").value="project";
get("tracks_export_mode").value="none";
const context=vm.createContext({document:{readyState:"complete",getElementById:get},window:{setInterval(){}},console,URLSearchParams});
vm.runInContext(fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/results-diagnostics.js"),"utf8"),context);
context.window.VolleyballDiagnosticsUI.setSources("project",["match.mp4"],"match.mp4",{"match.mp4":"cache"});
assert.equal(get("diagnostics-submit").disabled,true,"no export selected means no actionable job");
assert.equal(get("show_rally_track").disabled,true,"track number only affects snapshot export");
assert.equal(get("tracks_padding").disabled,true,"padding only affects MP4 export");
get("show_rally_snapshot").checked=true;form.handlers.change();
assert.equal(get("diagnostics-submit").disabled,false);
assert.equal(get("show_rally_track").disabled,false);
assert.equal(get("tracks_padding").disabled,true);
get("tracks_export_mode").value="combined";form.handlers.change();
assert.equal(get("tracks_padding").disabled,false);
context.window.VolleyballDiagnosticsUI.setTask({status:"running",kind:"analyze"});
assert.equal(get("diagnostics-submit").disabled,true,"server executes only one job at a time");
context.window.VolleyballDiagnosticsUI.setTask({status:"complete"});
assert.equal(get("diagnostics-submit").disabled,false);
context.window.VolleyballDiagnosticsUI.setSources("project",[],"",{});
assert.equal(get("diagnostics-submit").disabled,true);
assert.equal(get("diagnostics-source-select").disabled,true);
console.log("DIAGNOSTIC CONTROLS: only available cache and effective export settings PASS");
