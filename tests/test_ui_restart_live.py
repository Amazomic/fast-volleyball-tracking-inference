"""Integration test: re-exec keeps one listener and a stable PID (no user jobs)."""
import json
import socket
import subprocess
import sys
import time
from urllib.request import Request, urlopen
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]

def call(base, path, data=None):
    request=Request(base+path, data=data,
                    headers={"Content-Type":"application/json"} if data else {},
                    method="POST" if data else "GET")
    with urlopen(request,timeout=1.5) as result:
        return json.load(result)

def main():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1",0))
        port=sock.getsockname()[1]
    args=[sys.executable,"-m","scripts.volleyball_ui","--port",str(port)]
    base=f"http://127.0.0.1:{port}"
    server=subprocess.Popen(args,cwd=ROOT,stdout=subprocess.DEVNULL,stderr=subprocess.STDOUT)
    try:
        before=None
        for _ in range(50):
            if server.poll() is not None:raise RuntimeError("server exited before health")
            try:
                before=call(base,"/api/health")
                if before.get("ok"):break
            except (OSError, ValueError):pass
            time.sleep(.1)
        assert before and before["ok"], "server did not start"
        assert before["pid"] == server.pid
        ack=call(base,"/api/system/restart",b"{}")
        assert ack["accepted"] and ack["instance_id"]==before["instance_id"]
        after=None
        for _ in range(60):
            try:
                after=call(base,"/api/health")
                if after.get("instance_id")!=before["instance_id"]:break
            except (OSError, ValueError):pass
            time.sleep(.12)
        assert after and after["instance_id"]!=before["instance_id"],"restart failed"
        assert after["pid"]==before["pid"]==server.pid,"restart forked a duplicate"
        assert not after["restarting"]
        duplicate=subprocess.run(args,cwd=ROOT,capture_output=True,text=True,timeout=8)
        assert duplicate.returncode==0, duplicate.stderr
        assert "already listens" in duplicate.stdout,duplicate.stdout
        assert call(base,"/api/health")["instance_id"]==after["instance_id"]
        print("PASS: in-place re-exec, same PID, new instance, duplicate start refused")
    finally:
        server.terminate()
        try:server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill();server.wait(timeout=5)

if __name__=="__main__":
    main()
