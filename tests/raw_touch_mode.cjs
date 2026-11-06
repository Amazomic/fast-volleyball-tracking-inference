"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const source=fs.readFileSync(path.join(__dirname,
  "../scripts/volleyball_ui/static/model-touches-ui.js"),"utf8");
const nodes=new Map();
class Node{
  constructor(id){this.id=id;this.hidden=false;this.checked=false;this.events=new Map();
    this.style={};this.textContent="";this.value="0.5";this.children=[];this.attrs={};
    const classes=new Set();
    this.classList={toggle:(name,active)=>active?classes.add(name):classes.delete(name),
      contains:name=>classes.has(name)};
  }
  addEventListener(event,callback){
    const callbacks=this.events.get(event)||[];callbacks.push(callback);this.events.set(event,callbacks);
  }
  fire(event){for(const fn of this.events.get(event)||[])fn({target:this,currentTarget:this});}
  replaceChildren(...children){this.children=children;}
  setAttribute(k,v){this.attrs[k]=v;}
  getContext(){return {clearRect(){}};}
}
function E(id){if(!nodes.has(id))nodes.set(id,new Node(id));return nodes.get(id);}
const neural=new Node("neural");neural.value="model";
const cpu=new Node("cpu");cpu.value="events";cpu.checked=true;
const document={
  getElementById:E,
  querySelectorAll(selector){
    if(selector==='[name="result-preview-mode"]')return [neural,cpu];
    return [];
  }
};
const window={};
vm.runInNewContext(source,{document,window,console}, {filename:"model-touches-ui.js"});
const ui=window.VolleyballModelTouches;
let changes=0;
ui.init(()=>changes++);
assert.equal(ui.active(),false);
assert.equal(E("result-model-touch-section").hidden,true);
neural.checked=true;cpu.checked=false;neural.fire("change");
assert.equal(ui.active(),true);
assert.equal(E("view-analysis-results").classList.contains("raw-touch-mode"),true);
assert.equal(E("result-model-touch-section").hidden,false);
ui.setTouchVisible(false);
assert.equal(ui.active(),true,"hiding raw touches must keep neural mode active");
assert.equal(E("result-model-touch-section").hidden,true);
assert.equal(E("result-model-touch-controls").hidden,true);
ui.setTouchVisible(true);
assert.equal(E("result-model-touch-section").hidden,false);
cpu.checked=true;neural.checked=false;cpu.fire("change");
assert.equal(ui.active(),false);
assert.equal(E("view-analysis-results").classList.contains("raw-touch-mode"),false);
assert.equal(E("result-model-touch-section").hidden,true);
assert.ok(changes>=4);
console.log("RAVEL RAW TOUCH MODES / VISIBILITY / CPU RESTORATION: PASS");
