"use strict";
const assert=require("node:assert/strict");

class Element {
  constructor(tag="div"){
    this.tagName=tag;this.children=[];this.handlers={};this.hidden=false;
    this.attributes={};this.style={};this.dataset={};this.options=[];
    this.value="";this.disabled=false;this.className="";this.textContent="";
    this.scrollTop=0;this.parentElement=null;this.title="";
    this.classList={
      add:(value)=>{this.className+=" "+value;},
      contains:(value)=>this.className.split(/\s+/).includes(value),
    };
  }
  setAttribute(key,value){this.attributes[key]=value;}
  append(...items){for(const child of items){child.parentElement=this;this.children.push(child);}}
  replaceChildren(...items){this.children=[];this.append(...items);this.scrollTop=0;}
  after(item){this.inserted=item;}
  addEventListener(name,fn){(this.handlers[name]||=[]).push(fn);}
  click(){for(const fn of this.handlers.click||[])fn({target:this});}
  focus(){document.activeElement=this;}
  closest(selector){
    if(selector===".header-chain-step")return {querySelector:()=>({textContent:"Results"})};
    if(selector===".header-chain-option-row"){
      let item=this;
      while(item){if(item.className.includes("header-chain-option-row"))return item;item=item.parentElement;}
    }
    return null;
  }
  querySelector(selector){
    const cls=selector.slice(1);
    return this.children.find(child=>child.classList.contains(cls))||null;
  }
  querySelectorAll(selector){
    if(selector!=="button")return [];
    return this.children.flatMap(child=>child.tagName==="button"?[child]:child.querySelectorAll(selector));
  }
}
const natives={};
const created=[];
global.document={
  activeElement:null,
  createElement:(name)=>new Element(name),
  getElementById:(id)=>natives[id],
  addEventListener:()=>{},
};
for(const [kind,id] of Object.entries({
  project:"chain-project",inference:"chain-inference",postprocess:"chain-postprocess",
})){
  const native=new Element("select");
  native.value=kind==="postprocess"?"cpu-1":"original";
  native.options=kind==="postprocess"
    ?[{value:"original",text:"Original",cpuVersion:false},
      {value:"cpu-1",text:"CPU 1",cpuVersion:true},
      {value:"cpu-2",text:"CPU 2",cpuVersion:true}]
    :[{value:"original",text:"Original"}];
  natives[id]=native;created.push(native);
}
const menu=require("../scripts/volleyball_ui/static/header-chain-menu.js");
const actions=[];
menu.init({onAction:(...args)=>actions.push(args),onSelect:()=>{}});
const native=natives["chain-postprocess"];
const popup=native.inserted.children[1];
const trigger=native.inserted.children[0];
trigger.click();
assert.equal(popup.hidden,false,"CPU menu should open");
const cross=popup.children[1].children.at(-1);
cross.click();
assert.deepEqual(actions[0].slice(0,3),["postprocess","delete","cpu-1"]);
assert.equal(popup.hidden,false,"one-click CPU deletion must not close menu");
native.options=native.options.filter(option=>option.value!=="cpu-1");
native.value="original";popup.scrollTop=66;
menu.render();
assert.equal(popup.hidden,false,"rerender must preserve open CPU menu");
assert.equal(popup.scrollTop,66,"rerender must preserve scroll position");
const nextCross=popup.children.at(-1).children.at(-1);
nextCross.click();
assert.deepEqual(actions[1].slice(0,3),["postprocess","delete","cpu-2"]);
assert.equal(popup.hidden,false,"second CPU deletion must not close menu");
console.log("CPU DELETE MENU: sequential crosses, preserved popup and scroll PASS");
