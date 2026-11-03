"use strict";
/* Offline RU/EN UI: text changes are translated after each async render,
 * without translating user input or sending project data to third parties. */
(function(){
  const dictionary=window.VolleyEnglish||{};
  const STORAGE="volley-ui-language";
  const ATTRS=["title","aria-label","placeholder","alt","data-tooltip","data-hint","aria-description"];
  const originals=new WeakMap(),attributeOriginals=new WeakMap();
  const cyr=/[а-яё]/i;
  let current="ru",observer=null;
  const exclusions="script,style,textarea,pre,code,[contenteditable=true]";
  const entries=Object.entries(dictionary).filter(([ru,en])=>
    ru!==en && !cyr.test(en) && ru.length>=5 && ru.length<=300 &&
    !/[<>\\\n]/.test(ru) && !/^\s*$/.test(ru) &&
    !ru.includes("'") && !ru.includes('"'))
    .sort((a,b)=>b[0].length-a[0].length);
  const escapeRegExp=s=>s.replace(/[.*+?^$()|[\]{}\\]/g,"\\$&");
  // Cyrillic word boundaries prevent partial translation inside words:
  // e.g. "монтаж" must not turn "монтажа" into "montageа".
  const pattern=entries.length?new RegExp(
    "(?<![А-Яа-яЁё])(?:"+entries.map(([r])=>escapeRegExp(r)).join("|")+
    ")(?![А-Яа-яЁё])","gu"):null;
  function translated(input){
    if(!cyr.test(input))return input;
    const trim=input.trim(),direct=dictionary[trim];
    if(direct&&direct!==trim&&!cyr.test(direct)){
      const at=input.indexOf(trim);
      return input.slice(0,at)+direct+input.slice(at+trim.length);
    }
    const result=pattern?input.replace(pattern,hit=>dictionary[hit]||hit):input;
    return result
      .replace(/(^|[^\p{L}])нет(?=$|[^\p{L}])/gu,"$1missing")
      .replace(/(^|[^\p{L}])из(?=$|[^\p{L}])/gu,"$1of");
  }
  function skip(node){
    const parent=node.nodeType===3?node.parentElement:node;
    return !parent||parent.closest?.(exclusions);
  }
  function updateText(node){
    if(node.nodeType!==3||skip(node))return;
    const value=node.nodeValue,prev=originals.get(node);
    if(!value||(!cyr.test(value)&&!prev))return;
    if(current==="en"){
      if(prev?.en===value)return;
      const en=translated(value);
      originals.set(node,{ru:value,en});
      if(en!==value)node.nodeValue=en;
    }else if(prev){
      if(value===prev.en&&value!==prev.ru)node.nodeValue=prev.ru;
      originals.delete(node);
    }
  }
  function updateAttrs(el){
    if(el.nodeType!==1||skip(el))return;
    let seen=attributeOriginals.get(el);
    if(!seen){seen=new Map();attributeOriginals.set(el,seen)}
    for(const attr of ATTRS){
      if(!el.hasAttribute(attr))continue;
      const value=el.getAttribute(attr),prev=seen.get(attr);
      if(current==="en"){
        if(prev?.en===value||!cyr.test(value))continue;
        const en=translated(value);
        seen.set(attr,{ru:value,en});
        if(en!==value)el.setAttribute(attr,en);
      }else if(prev){
        if(prev.en===value&&value!==prev.ru)el.setAttribute(attr,prev.ru);
        seen.delete(attr);
      }
    }
  }
  function subtree(node){
    if(node.nodeType===3){updateText(node);return}
    if(node.nodeType!==1||skip(node))return;
    updateAttrs(node);
    const walker=document.createTreeWalker(node,NodeFilter.SHOW_ELEMENT|NodeFilter.SHOW_TEXT);
    for(let child=walker.nextNode();child;child=walker.nextNode()){
      if(child.nodeType===3)updateText(child);
      else updateAttrs(child);
    }
  }
  function setLanguage(lang){
    current=lang==="en"?"en":"ru";
    if(observer)observer.disconnect();
    document.documentElement.lang=current;
    document.documentElement.dataset.lang=current;
    try{localStorage.setItem(STORAGE,current)}catch{}
    subtree(document.body);
    const ruTitle=document.documentElement.dataset.ruTitle||
      (document.documentElement.dataset.ruTitle=document.title);
    document.title=current==="en"?translated(ruTitle):ruTitle;
    document.querySelectorAll("[data-language-choice]").forEach(btn=>{
      const on=btn.dataset.languageChoice===current;
      btn.classList.toggle("is-active",on);
      btn.setAttribute("aria-pressed",String(on));
    });
    if(observer)observer.observe(document.body,{subtree:true,childList:true,
      characterData:true,attributes:true,attributeFilter:ATTRS});
    document.documentElement.dataset.i18nReady="1";
  }
  function init(){
    observer=new MutationObserver(records=>{
      if(current!=="en")return;
      observer.disconnect();
      for(const record of records){
        if(record.type==="childList"){
          for(const node of record.addedNodes)subtree(node);
        }else if(record.type==="characterData")updateText(record.target);
        else if(record.type==="attributes")updateAttrs(record.target);
      }
      observer.observe(document.body,{subtree:true,childList:true,
        characterData:true,attributes:true,attributeFilter:ATTRS});
    });
    document.querySelector("[data-language-switch]")?.addEventListener("click",event=>{
      const choice=event.target.closest("[data-language-choice]");
      if(choice)setLanguage(choice.dataset.languageChoice);
    });
    let saved="ru";
    try{saved=localStorage.getItem(STORAGE)||"ru"}catch{}
    setLanguage(saved);
  }
  window.VolleyI18n={getLanguage:()=>current,
    locale:()=>current==="en"?"en-US":"ru-RU",translate:translated,setLanguage};
  if(document.readyState==="loading")document.addEventListener("DOMContentLoaded",init,{once:true});
  else init();
})();
