import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_ui.js",import.meta.url),"utf8")
  .split("// Minimal browser adapters")[0];
const favicon={href:"/studio/volleyball-photo.png"};
const ball={src:"/studio/volleyball-photo.png",style:{}};
const turns=[],frames=[];
let hidden=false;
const canvas={
  getContext:()=>({
    clearRect(){},save(){},translate(){},restore(){},
    rotate(value){turns.push(value);},drawImage(){}
  }),
  toDataURL:()=>"data:image/png;base64,test"
};
class MockImage{
  set src(value){this.value=value;this.onload?.();}
}
const document={
  body:{dataset:{uiMotion:"normal"}},
  querySelector:selector=>selector===".brand .volleyball-ball"?ball:
    selector==='link[rel="icon"]'?favicon:null,
  createElement:name=>name==="canvas"?canvas:null,
  addEventListener(){},
  get hidden(){return hidden;}
};
const window={
  matchMedia:()=>({matches:false}),
  fetch:async()=>({ok:true})
};
runInNewContext(source,{
  window,document,Image:MockImage,performance:{now:()=>100},
  setTimeout,clearTimeout,setInterval,clearInterval,
  requestAnimationFrame:fn=>{frames.push(fn);return frames.length;}
});
assert.equal(favicon.href,"/studio/volleyball-photo.png","idle favicon keeps the provided volleyball image");
assert.equal(frames.length,0,"idle should not animate");
window.VolleyballStudioActivity.setTaskActive("studio-job",true);
let time=100;
for(let i=0;i<25;i++){
  const cb=frames.shift();
  assert.ok(cb,"an active job must keep animation alive");
  cb(time+=16);
}
assert.ok(ball.style.transform.startsWith("rotate("),"ball rotates");
assert.ok(turns.some(value=>value>0),"the favicon must rotate");
assert.ok(favicon.href.startsWith("data:image/png"),"the tab icon is updated");
const lastAngle=parseFloat(ball.style.transform.slice(7));
assert.ok(Math.abs(turns.at(-1)*180/Math.PI-lastAngle)<25,"both icons share the same phase");
window.VolleyballStudioActivity.setTaskActive("studio-job",false);
for(let i=0;i<140&&frames.length;i++)frames.shift()(time+=16);
assert.equal(frames.length,0,"rotation must finish once work completes");
assert.equal(favicon.href.startsWith("data:image/png"),true,"last favicon frame is kept");
console.log("studio activity icon: OK");
