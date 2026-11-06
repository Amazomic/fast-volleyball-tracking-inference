"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
for(const file of ["index.html","app.js"]){
  const source=fs.readFileSync("scripts/volleyball_ui/static/"+file,"utf8");
  assert.doesNotMatch(source,/\?{3,}/,file+" contains corrupted UI text");
}
console.log("Player UI text encoding passed");
