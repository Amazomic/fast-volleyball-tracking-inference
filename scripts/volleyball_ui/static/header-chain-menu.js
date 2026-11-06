"use strict";
/* Header selector with row-local rename/delete actions.
   Native selects remain the state carrier for existing project/run bindings.
   Native <option> does not support clickable child controls. */
(function(global){
  const IDS={project:"chain-project",inference:"chain-inference",
             postprocess:"chain-postprocess"};
  const menus=new Map();
  let handlers=null;
  let started=false;
  function close(){
    for(const item of menus.values()){
      item.popup.hidden=true;
      item.trigger.setAttribute("aria-expanded","false");
    }
  }
  function makeButton(className,label,text){
    const button=document.createElement("button");
    button.type="button";
    button.className=className;
    button.setAttribute("aria-label",label);
    button.title=label;
    button.textContent=text;
    return button;
  }
  function render(){
    if(!started)return;
    for(const [kind,item] of menus){
      const {native,trigger,popup}=item;
      const entries=Array.from(native.options||[]).filter(option=>option.value);
      const selected=entries.find(option=>option.value===native.value);
      trigger.textContent=selected?.text||native.options?.[0]?.text||"Нет записей";
      trigger.title=selected?.title||selected?.text||"";
      trigger.disabled=native.disabled||!entries.length;
      popup.replaceChildren();
      if(!entries.length){
        const empty=document.createElement("div");
        empty.className="header-chain-popover-empty";
        empty.textContent=native.options?.[0]?.text||"Нет записей";
        popup.append(empty);
        popup.hidden=true;
        trigger.setAttribute("aria-expanded","false");
        continue;
      }
      for(const option of entries){
        const row=document.createElement("div");
        row.className="header-chain-option-row"+(option.value===native.value?" is-selected":"");
        const text=option.text;
        const isProtectedOriginal=kind==="postprocess"&&
          option.cpuVersion!==true;
        const remove=isProtectedOriginal?null:makeButton(
          "header-chain-option-action delete","Удалить: "+text,"×");
        const rename=isProtectedOriginal?null:makeButton(
          "header-chain-option-action rename","Переименовать: "+text,"✎");
        const choose=makeButton("header-chain-option-text",
          "Выбрать: "+text,text);
        if(option.title)choose.title=option.title;
        choose.setAttribute("aria-selected",String(option.value===native.value));
        if(remove)remove.addEventListener("click",()=>{
          close();handlers.onAction(kind,"delete",option.value,text);
        });
        if(rename)rename.addEventListener("click",()=>{
          close();handlers.onAction(kind,"rename",option.value,text);
        });
        choose.addEventListener("click",()=>{
          close();
          if(native.value!==option.value)handlers.onSelect(kind,option.value);
        });
        // Text on the left; editing controls on the RIGHT of every entry.
        row.append(choose);
        if(rename)row.append(rename);
        if(remove)row.append(remove);
        popup.append(row);
      }
    }
  }
  function init(callbacks){
    if(started)return;
    handlers=callbacks;
    for(const [kind,id] of Object.entries(IDS)){
      const native=document.getElementById(id);
      if(!native)continue;
      native.classList.add("header-chain-native");
      const wrap=document.createElement("div");
      wrap.className="header-chain-select-wrap";
      wrap.dataset.chainKind=kind;
      const title=native.closest(".header-chain-step")
        ?.querySelector(".header-chain-label")?.textContent?.trim()||kind;
      const trigger=makeButton("header-chain-select-trigger",
        "Открыть список: "+title, "");
      trigger.setAttribute("aria-expanded","false");
      trigger.setAttribute("aria-haspopup","listbox");
      const popup=document.createElement("div");
      popup.className="header-chain-popover";
      popup.hidden=true;
      popup.setAttribute("role","listbox");
      popup.id=id+"-menu";
      trigger.setAttribute("aria-controls",popup.id);
      wrap.append(trigger,popup);
      native.after(wrap);
      menus.set(kind,{native,wrap,trigger,popup});
      trigger.addEventListener("click",()=>{
        const opening=popup.hidden;
        close();
        if(opening&&!trigger.disabled){
          popup.hidden=false;
          trigger.setAttribute("aria-expanded","true");
        }
      });
      wrap.addEventListener("keydown",event=>{
        if(event.key==="Escape"){
          event.preventDefault();
          close();trigger.focus();
        }else if(event.key==="ArrowDown"||event.key==="ArrowUp"){
          if(popup.hidden)return;
          const actions=Array.from(popup.querySelectorAll("button"));
          if(!actions.length)return;
          event.preventDefault();
          const index=actions.indexOf(document.activeElement);
          const next=event.key==="ArrowDown"?1:-1;
          actions[(index+next+actions.length)%actions.length].focus();
        }
      });
    }
    document.addEventListener("pointerdown",event=>{
      if(!event.target.closest(".header-chain-select-wrap"))close();
    });
    started=true;
    render();
  }
  const api={init,render,close};
  global.VolleyballHeaderMenu=api;
  if(typeof module!=="undefined"&&module.exports)module.exports=api;
})(typeof window!=="undefined"?window:globalThis);
