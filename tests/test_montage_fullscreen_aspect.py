"""Fullscreen montage previews retain the actual video display aspect ratio."""
from pathlib import Path
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "scripts" / "volleyball_ui" / "static"


class MontageFullscreenAspectTests(unittest.TestCase):
    def test_fullscreen_css_uses_contained_intrinsic_video_geometry(self):
        css = (UI / "montage-player.css").read_text(encoding="utf-8")
        fullscreen = css.split(".montage-preview-shell:fullscreen .preview-stage video{", 1)[1].split("}", 1)[0]
        self.assertIn("width:auto;height:auto", fullscreen)
        self.assertIn("max-width:100%", fullscreen)
        self.assertIn("max-height:100%", fullscreen)
        self.assertIn("object-fit:contain", fullscreen)

    def test_fullscreen_resize_metadata_and_exit(self):
        script = r'''
"use strict";
const vm=require("node:vm");
const fs=require("node:fs");
const assert=require("node:assert/strict");
const filename=process.argv[1];
const callbacks={};
const nodes=new Map();
function node(id){
  if(!nodes.has(id)){
    const handlers={};
    nodes.set(id,{
      id,handlers,style:{},value:"0",textContent:"",hidden:false,
      addEventListener(type,fn){handlers[type]=fn;},
      setAttribute(name,value){this[name]=value;}
    });
  }
  return nodes.get(id);
}
const video=node("preview-player");
video.videoWidth=1080;video.videoHeight=1920;video.duration=60;
video.currentSrc="/api/montage/media";video.currentTime=5;video.paused=true;
video.volume=1;video.muted=false;
const stage={clientWidth:1920,clientHeight:960};
const shell=node("montage-preview-shell");
shell.querySelector=()=>stage;
const document={
  fullscreenElement:null,
  getElementById:node,
  addEventListener(type,fn){callbacks[type]=fn;},
  exitFullscreen(){this.fullscreenElement=null;callbacks.fullscreenchange();return Promise.resolve();}
};
let observer=null;
const window={addEventListener(type,fn){callbacks["window:"+type]=fn;}};
class ResizeObserver{
  constructor(fn){observer=fn;}
  observe(target){assert.equal(target,stage);}
}
vm.runInNewContext(fs.readFileSync(filename,"utf8"),{
  document,window,ResizeObserver,requestAnimationFrame:()=>1,
  cancelAnimationFrame:()=>{},console
}, {filename});
const player=window.VolleyballMontagePlayer;
assert.equal(player.fitAspect(1080,1920,1920,960).width,540);
assert.equal(player.fitAspect(1080,1920,1920,960).height,960);
assert.equal(player.fitAspect(1920,1080,1920,960).width,1706);
assert.equal(player.fitAspect(1920,1080,1920,960).height,960);
assert.equal(player.fitAspect(1080,1920,0,960),null);
player.init();
assert.equal(video.style.width,undefined);
document.fullscreenElement=shell;
callbacks.fullscreenchange();
assert.equal(video.style.width,"540px");
assert.equal(video.style.height,"960px");
assert.equal(node("montage-fullscreen")["aria-label"],"Выйти из полного экрана");
stage.clientWidth=400;stage.clientHeight=720;
observer();
assert.equal(video.style.width,"400px");
assert.equal(video.style.height,"711px");
video.videoWidth=1920;video.videoHeight=1080;
video.handlers.loadedmetadata();
assert.equal(video.style.width,"400px");
assert.equal(video.style.height,"225px");
document.fullscreenElement=null;
callbacks.fullscreenchange();
assert.equal(video.style.width,"");
assert.equal(video.style.height,"");
assert.equal(node("montage-fullscreen")["aria-label"],"Полный экран");
'''
        result = subprocess.run(
            ["node", "-e", script, str(UI / "montage-player.js")],
            cwd=ROOT, capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
