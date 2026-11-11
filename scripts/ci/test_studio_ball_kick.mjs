import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const root=new URL("../../rust/volley-app/static/",import.meta.url);
const html=readFileSync(new URL("studio.html",root),"utf8");
const css=readFileSync(new URL("studio.css",root),"utf8");
const script=readFileSync(new URL("studio_ui.js",root),"utf8")
  .split("// Minimal browser adapters")[0];

assert.match(html,/<button class="logo" type="button" aria-label="Ударить по мячу"/);
assert.match(html,/<span class="volleyball-ball-stage" aria-hidden="true">/);
assert.match(css,/\.brand \.logo\{[^}]*overflow:hidden/,"all flight remains clipped");
assert.match(css,/\.brand \.logo\.is-kicked \.volleyball-ball-stage\{will-change:/);
assert.doesNotMatch(css,/\.studio-ball-projectile/, "nothing flies outside the frame");

function harness(reduced=false){
  let now=0;
  let handler;
  const timers=[],frames=[],classes=new Set();
  const stage={style:{transform:"",opacity:""}};
  let externalChildren=0;
  const logo={
    classList:{
      add:n=>classes.add(n),
      remove:n=>classes.delete(n),
      contains:n=>classes.has(n)
    },
    querySelector:s=>s===".volleyball-ball-stage"?stage:null,
    addEventListener:(n,cb)=>{if(n==="click")handler=cb;}
  };
  const ball={
    src:"/studio/volleyball-photo.png",style:{},
    closest:()=>logo,
    cloneNode(){throw Error("no projectile should be cloned");}
  };
  const document={
    body:{dataset:{uiMotion:"normal"},appendChild(){externalChildren++;}},
    hidden:false,
    querySelector:s=>s===".brand .volleyball-ball"?ball:null,
    addEventListener(){}
  };
  const window={matchMedia:()=>({matches:reduced}),fetch:async()=>({ok:true}),addEventListener(){}};
  runInNewContext(script,{document,window,Image:undefined,performance:{now:()=>now},
    setTimeout(fn,ms){timers.push({at:now+ms,fn});return timers.length;},
    clearTimeout(){},setInterval(){},clearInterval(){},
    requestAnimationFrame(fn){frames.push(fn);return frames.length;}
  });
  return {
    ball,window,classes,stage,click(){handler();},
    get externalChildren(){return externalChildren;},
    frameAt(time){
      now=time;
      const batch=frames.splice(0);
      assert.ok(batch.length,"an animation must be scheduled");
      for(const cb of batch)cb(now);
    },
    advance(ms){
      const end=now+ms;
      for(;;){
        const task=timers.filter(t=>t.at<=end).sort((a,b)=>a.at-b.at)[0];
        if(!task)break;
        timers.splice(timers.indexOf(task),1);
        now=task.at;
        task.fn();
      }
      now=end;
    }
  };
}
function flightPose(stage){
  const match=stage.style.transform.match(
    /^translate3d\(([-\d.]+)px,([-\d.]+)px,0\) rotate\(([-\d.]+)deg\) scale\(([-\d.]+)\)$/
  );
  assert.ok(match,"flight pose is physically evaluated rather than a decorative keyframe");
  return {x:Number(match[1]),y:Number(match[2]),spin:Number(match[3]),scale:Number(match[4])};
}

const h=harness();
h.click();
assert.ok(h.classes.has("is-kicked"));
assert.equal(h.externalChildren,0);
h.click();
assert.equal(h.externalChildren,0,"rapid click does not create a projectile");
h.frameAt(25);
assert.match(h.stage.style.transform,/^scale\(/,"impact compresses the ball");
h.window.VolleyballStudioActivity.setTaskActive("studio-job",true);
h.frameAt(200);
const early=flightPose(h.stage);
assert.match(h.ball.style.transform,/^rotate\(/,"ongoing activity spin persists");
h.frameAt(350);
const apex=flightPose(h.stage);
h.frameAt(700);
const late=flightPose(h.stage);
assert.ok(early.y<0&&apex.y<early.y,"ball rises along its arc");
assert.ok(late.y>apex.y,"gravity pulls ball downward");
assert.ok(early.x<apex.x&&apex.x<late.x,"horizontal motion is forward");
assert.ok(early.scale>apex.scale&&apex.scale>late.scale,"depth reduces apparent size");
assert.ok(late.spin>apex.spin,"flight spin increases");
h.frameAt(770);
assert.equal(h.stage.style.opacity,"0","flight ends far away");
h.advance(480);
assert.ok(!h.classes.has("is-kicked"));
assert.ok(h.classes.has("is-returning"));
assert.equal(h.stage.style.transform,"");
assert.equal(h.stage.style.opacity,"");
h.advance(410);
assert.ok(!h.classes.has("is-returning"));
h.click();
assert.ok(h.classes.has("is-kicked"),"repeatable after return");

const reduced=harness(true);
reduced.click();
assert.ok(reduced.classes.has("is-kicked"));
assert.equal(reduced.stage.style.opacity,"0","reduced-motion instantly hides ball");
reduced.advance(400);
assert.ok(reduced.classes.has("is-returning"));
reduced.advance(410);
reduced.click();
assert.ok(reduced.classes.has("is-kicked"));
assert.equal(reduced.externalChildren,0);
console.log("studio ball kick (ballistic in-frame): OK");
