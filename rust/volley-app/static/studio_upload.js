"use strict";
(()=>{
  const maxVideoBytes=32*1024**3;
  const start=(url,file,{headers={},progress=()=>{}}={})=>{
    let xhr=null;
    const promise=new Promise((resolve,reject)=>{
      if(!file||!Number.isFinite(file.size)||file.size<=0||file.size>maxVideoBytes){
        reject(Error("Размер видео: от 1 байта до 32 ГиБ"));return;
      }
      xhr=new XMLHttpRequest();
      xhr.open("POST",url,true);xhr.withCredentials=true;
      for(const [name,value] of Object.entries(headers))xhr.setRequestHeader(name,String(value));
      xhr.upload.addEventListener("progress",event=>progress(event.lengthComputable&&event.total>0?
        Math.min(100,Math.round(event.loaded/event.total*100)):null));
      xhr.addEventListener("load",()=>{
        let data;
        try{data=JSON.parse(xhr.responseText);}catch{reject(Error("Некорректный ответ загрузки"));return;}
        if(xhr.status>=200&&xhr.status<300)resolve(data);
        else reject(Error(data.error||data.message||"HTTP "+xhr.status));
      });
      xhr.addEventListener("error",()=>reject(Error("Передача видео прервана")));
      xhr.addEventListener("abort",()=>reject(Error("Загрузка отменена")));
      const body=new FormData();body.append("file",file,file.name);xhr.send(body);
    });
    return {promise,abort:()=>xhr?.abort()};
  };
  window.VolleyballUpload={start,maxVideoBytes};
})();
