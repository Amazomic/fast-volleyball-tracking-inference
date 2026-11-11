import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
let xhr;
class XHR {
 constructor(){xhr=this;this.events={};this.upload={addEventListener:(k,f)=>this.progress=f};}
 open(method,url){this.method=method;this.url=url;}
 setRequestHeader(){}
 addEventListener(k,f){this.events[k]=f;}
 send(body){this.body=body;}
 abort(){this.events.abort();}
}
const window={};
runInNewContext(readFileSync(new URL("../../rust/volley-app/static/studio_upload.js",import.meta.url),"utf8"),{window,XMLHttpRequest:XHR,FormData:class{append(){}},Promise,Error});
const api=window.VolleyballUpload,limit=32*1024**3,progress=[];
assert.equal(api.maxVideoBytes,limit);
const op=api.start("/upload",{name:"a.mp4",size:limit},{progress:p=>progress.push(p)});
xhr.progress({lengthComputable:true,loaded:1,total:2});
xhr.status=200;xhr.responseText='{"ok":true}';xhr.events.load();
assert.equal((await op.promise).ok,true);assert.deepEqual(progress,[50]);
const bad=api.start("/upload",{name:"a",size:limit+1});await assert.rejects(bad.promise,/32/);
const empty=api.start("/upload",{name:"a",size:0});await assert.rejects(empty.promise,/32/);
const abort=api.start("/upload",{name:"a",size:1});abort.abort();await assert.rejects(abort.promise,/отменена/);
const network=api.start("/upload",{name:"a",size:1});xhr.events.error();await assert.rejects(network.promise,/прервана/);
const http=api.start("/upload",{name:"a",size:1});xhr.status=413;xhr.responseText='{"error":"too large"}';xhr.events.load();await assert.rejects(http.promise,/too large/);
console.log("PASS: shared upload 32 GiB boundary, progress, cancellation, empty input and HTTP/network failures");
