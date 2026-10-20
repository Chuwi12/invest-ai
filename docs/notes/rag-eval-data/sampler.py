#!/usr/bin/env python3
import json, subprocess, sys, time, urllib.request
out = open(sys.argv[1], "a", buffering=1)
while True:
    t = time.time()
    try:
        mem = subprocess.run(["nvidia-smi", "--query-gpu=memory.used,utilization.gpu,power.draw", "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=5).stdout.strip().split(", ")
        ps = json.load(urllib.request.urlopen("http://127.0.0.1:11434/api/ps", timeout=3)).get("models", [])
        out.write(json.dumps({"t": t, "mem_mib": int(mem[0]), "util": int(mem[1]), "power_w": float(mem[2]),
            "models": [{"name": m["name"], "size": m["size"], "size_vram": m["size_vram"]} for m in ps]}) + "\n")
    except Exception as e:
        out.write(json.dumps({"t": t, "err": repr(e)}) + "\n")
    time.sleep(max(0, 1.0 - (time.time() - t)))
