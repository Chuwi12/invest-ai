#!/usr/bin/env python3
import json, sys, time, urllib.request, urllib.parse, os
R = os.path.dirname(os.path.abspath(__file__))

def get(port, path, timeout=60):
    t = time.time()
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout) as r:
            return r.status, json.load(r), time.time() - t
    except urllib.error.HTTPError as e:
        try: body = json.load(e)
        except Exception: body = None
        return e.code, body, time.time() - t
    except Exception as e:
        return 0, {"error": repr(e)}, time.time() - t

def analyze(port, asset, depth="standard", timeout=900):
    url = f"http://127.0.0.1:{port}/api/assets/{urllib.parse.quote(asset, safe=':')}/analysis/stream?depth={depth}"
    req = urllib.request.Request(url, method="POST", data=b"")
    t0 = time.time(); out = {"asset": asset, "port": port, "depth": depth, "t0": t0, "stages": []}
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            for raw in r:
                line = raw.decode("utf8", "ignore").strip()
                if not line.startswith("data:"): continue
                ev = json.loads(line[5:])
                if ev["type"] == "stage": out["stages"].append((time.time() - t0, ev["stage"]))
                elif ev["type"] == "done": out["analysis"] = ev["analysis"]
                elif ev["type"] == "error": out["error"] = ev
    except Exception as e:
        out["error"] = {"exc": repr(e)}
    out["wall"] = time.time() - t0
    return out

if __name__ == "__main__":
    if sys.argv[1] == "analyze":
        port, asset, label = int(sys.argv[2]), sys.argv[3], sys.argv[4]
        r = analyze(port, asset, sys.argv[5] if len(sys.argv) > 5 else "standard")
        json.dump(r, open(f"{R}/results/{label}.json", "w"), ensure_ascii=False, indent=1)
        a = r.get("analysis") or {}
        print(label, "wall %.0fs" % r["wall"], a.get("verdict"), a.get("promptTokens"), a.get("completionTokens"), r.get("error"))
