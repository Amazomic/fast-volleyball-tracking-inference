"use strict";
const assert=require("node:assert/strict");
const player=require("../scripts/volleyball_ui/static/shared-player.js");
assert.equal(player.frameDelta(60),1/60);
assert.equal(player.frameDelta(0),1/30);
assert.equal(player.frameDelta(NaN),1/30);
assert.equal(player.holdFrames(0),1);
assert.ok(player.holdFrames(1800)>player.holdFrames(900));
assert.ok(player.holdFrames(900)>player.holdFrames(450));
assert.ok(player.holdFrames(60000)<=240);
let time=2,paused=0;
const adapter={position:()=>time,bounds:()=>({start:1,end:3}),fps:()=>60,
  seek:value=>{time=value;},pause:()=>{paused++;}};
player.step(adapter,1);
assert.equal(time,2+1/60);
player.step(adapter,-1);
assert.ok(Math.abs(time-2)<1e-9);
player.step(adapter,1,1000);
assert.equal(time,3);
player.step(adapter,-1,1000);
assert.equal(time,1);
assert.equal(paused,4);
let timer,elapsed=0;
const hold=player.createHold(adapter,{
  now:()=>elapsed,setInterval:callback=>{timer=callback;return 7;},
  clearInterval:id=>{assert.equal(id,7);timer=null;}
});
time=2;
hold.start(1);
assert.ok(time>2);
const once=time;
hold.start(1);
assert.equal(time,once);
elapsed=200;timer();assert.equal(time,once);
elapsed=300;timer();const slowStep=time-once;
const slowPosition=time;
elapsed=1200;timer();assert.ok(time-slowPosition>slowStep);
hold.stop();
assert.equal(timer,null);
const nativeSet=global.setInterval,nativeClear=global.clearInterval;
try{
  global.setInterval=function(){
    assert.ok(this==null||this===globalThis,"browser timers require the global receiver");
    return 9;
  };
  global.clearInterval=function(){
    assert.ok(this==null||this===globalThis,"browser timers require the global receiver");
  };
  const nativeHold=player.createHold(adapter);
  nativeHold.start(1);
  nativeHold.stop();
}finally{
  global.setInterval=nativeSet;global.clearInterval=nativeClear;
}
console.log("Shared player: frame steps, acceleration, bounds and hold cancellation passed");
