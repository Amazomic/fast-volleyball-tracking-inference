import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const js=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const start=js.indexOf("  const processedBallOverlayIsValid=");
const end=js.indexOf("  const requestOverlayWindow=",start);
assert.ok(start>=0&&end>start,"Processed overlay source gate must exist");
assert.match(js,/if\(!isRaw&&!processedBallOverlayIsValid\(value\)\)/);
const context={};
runInNewContext(js.slice(start,end)+"\nthis.validate=processedBallOverlayIsValid;",context);
const good={
  ball_policy:"vballnet_only_short_gap_interpolation_v1",
  start_frame:270,
  frames:[
    {frame:270,ball:[0.2,0.3],ball_origin:"vballnet",ball_interpolated:false},
    {frame:271,ball:[0.3,0.3],ball_origin:"vballnet_interpolated",ball_interpolated:true},
    {frame:272,ball:[0.4,0.3],ball_origin:"vballnet",ball_interpolated:false},
    {frame:273,ball:null,ball_origin:null,ball_interpolated:false}
  ]
};
assert.equal(context.validate(good),true);
const repaired=JSON.parse(JSON.stringify(good));
repaired.ball_policy="vballnet_physical_teleport_repair_player_kink_v3";
assert.equal(context.validate(repaired),true);
const invalid=edit=>{
  const clone=JSON.parse(JSON.stringify(good));
  edit(clone);
  assert.equal(context.validate(clone),false);
};
invalid(data=>data.ball_policy="prefer_vballnet_then_ravel");
invalid(data=>delete data.ball_policy);
invalid(data=>data.frames[1].ball_origin="ravel");
invalid(data=>data.frames[1].ball_interpolated=false);
invalid(data=>data.frames[1].ball=[1.2,0.2]);
invalid(data=>data.frames[2].frame=999);
invalid(data=>data.frames[3].ball_origin="vballnet");
console.log("Processed overlay rejects RAVEL, stale response formats, and unlabelled synthetic points: OK");
