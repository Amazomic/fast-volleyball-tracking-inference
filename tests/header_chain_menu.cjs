"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");

class Element {
  constructor(tag="div"){
    this.tag=tag;
    this.children=[];
    this.handlers={};
    this.attributes={};
    this.dataset={};
    this.classList={add:(name)=>this.className=(this.className||"")+" "+name};
    this.hidden=false;
    this.disabled=false;
    this.options=[];
    this.value="";
    this.textContent="";
  }
  closest(){return {querySelector(){return {textContent:"Проект"}}};}
  setAttribute(name,value){this.attributes[name]=String(value);}
  addEventListener(name,callback){this.handlers[name]=callback;}
  append(...nodes){this.children.push(...nodes);}
  replaceChildren(...nodes){this.children=[...nodes];}
  after(node){this.next=node;}
  querySelectorAll(selector){
    const all=[];
    const collect=node=>{
      if(selector==="button"&&node.tag==="button")all.push(node);
      for(const child of node.children)collect(child);
    };
    collect(this);
    return all;
  }
  fire(type,event={}){const handler=this.handlers[type];if(handler)handler(event);}
  focus(){document.activeElement=this;}
}
const elements={};
for(const id of ["chain-project","chain-inference","chain-postprocess"])
  elements[id]=new Element("select");
elements["chain-project"].options=[
  {value:"project-a",text:"A League"},
  {value:"project-b",text:"B League"}
];
elements["chain-project"].value="project-a";
elements["chain-inference"].options=[
  {value:"first.mp4|run_1",text:"first.mp4 · GPU pass 1"},
  {value:"second.mp4|run_2",text:"second.mp4 · GPU pass 2"}
];
elements["chain-inference"].value="first.mp4|run_1";
elements["chain-postprocess"].options=[
  {value:"run_1",text:"Исходная обработка",cpuVersion:false},
  {value:"run_1_pp_aaa",text:"RAVEL + геометрия",cpuVersion:true}
];
elements["chain-postprocess"].value="run_1_pp_aaa";
global.document={
  activeElement:null,
  createElement:tag=>new Element(tag),
  getElementById:id=>elements[id],
  addEventListener(){}
};
const menu=require("../scripts/volleyball_ui/static/header-chain-menu.js");
const selected=[],actions=[];
menu.init({
  onSelect:(kind,id)=>selected.push({kind,id}),
  onAction:(kind,op,id,label)=>actions.push({kind,op,id,label}),
});
assert(elements["chain-project"].next,"custom menu inserted after original project select");
for(const id of Object.keys(elements)){
  assert(elements[id].className.includes("header-chain-native"),
    "existing native source is hidden but remains available for app logic");
  const [trigger,popup]=elements[id].next.children;
  assert.equal(trigger.textContent,
    elements[id].options.find(o=>o.value===elements[id].value).text);
  assert.equal(popup.children.length,elements[id].options.length);
  assert.equal(popup.hidden,true);
  trigger.fire("click");
  assert.equal(popup.hidden,false);
  assert.equal(trigger.attributes["aria-expanded"],"true");
  if(id==="chain-postprocess"){
    const baseline=popup.children[0].children;
    assert.equal(baseline.length,1,
      "The original analysis is selectable, but not deletable from the CPU list");
    assert.equal(baseline[0].textContent,"Исходная обработка");
  }
  const editableIndex=id==="chain-postprocess"?1:0;
  const editable=popup.children[editableIndex].children;
  assert.equal(editable.length,3,"editable rows have text and two right-side actions");
  assert.equal(editable[0].textContent,elements[id].options[editableIndex].text);
  assert.equal(editable[1].textContent,"✎","pencil must be on the right");
  assert.equal(editable[2].textContent,"×","red cross must be rightmost");
  editable[2].fire("click");
  assert.equal(actions.at(-1).op,"delete");
  assert.equal(actions.at(-1).kind, id.replace("chain-",""));
  assert.equal(popup.hidden,id!=="chain-postprocess","CPU list stays open for sequential deletions");
  trigger.fire("click");
  popup.children[1].children[1].fire("click");
  assert.equal(actions.at(-1).op,"rename");
  trigger.fire("click");
  const alternate=elements[id].value===elements[id].options[1].value?0:1;
  popup.children[alternate].children[0].fire("click");
  assert.equal(selected.at(-1).id,elements[id].options[alternate].value);
  if(id==="chain-postprocess"){
    assert.equal(actions.filter(action=>action.id==="run_1").length,0,
      "CPU list never opens destructive actions on primary inference");
  }
}
elements["chain-postprocess"].options=[
  {value:"blocked",text:"Unavailable",disabled:true},
  {value:"ready",text:"Available",cpuVersion:true}
];
elements["chain-postprocess"].value="ready";
menu.render();
assert.equal(elements["chain-postprocess"].next.children[1].children.length,1,
  "disabled native choices must not become clickable custom rows");
elements["chain-postprocess"].options=[{value:"",text:"Нет версий"}];
elements["chain-postprocess"].value="";
elements["chain-postprocess"].disabled=true;
menu.render();
const [trigger,popup]=elements["chain-postprocess"].next.children;
assert.equal(trigger.disabled,true);
assert.equal(popup.hidden,true);
assert.equal(popup.children[0].textContent,"Нет версий");
const html=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/index.html"),"utf8");
assert(!html.includes('id="project-settings-trigger"'));
assert(html.includes('id="header-entry-action-dialog"'));
assert(html.includes('id="project-delete-dialog"'));
assert(html.includes('id="project-rename-dialog"'));
console.log("HEADER DROPDOWN ROW ACTIONS, FOCUS, POPUP STATE: PASS");
