#!/usr/bin/env python3
"""Velocidad del embedder directo contra Ollama (sin la app): GPU vs CPU, con y sin el LLM cargado."""
import json, sqlite3, statistics, subprocess, sys, time, urllib.request
OLL = "http://127.0.0.1:11434"
MODEL = "embeddinggemma:300m-qat-q8_0"

def post(path, body, timeout=600):
    req = urllib.request.Request(OLL + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r: return json.load(r)

def ps():
    return [(m["name"], round(m["size_vram"] / 2**20), round(m["size"] / 2**20)) for m in json.load(urllib.request.urlopen(OLL + "/api/ps"))["models"]]

def vram():
    return int(subprocess.run(["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"], capture_output=True, text=True).stdout)

def unload_all():
    for n, *_ in ps():
        post("/api/generate", {"model": n, "keep_alive": 0, "prompt": ""})
    for _ in range(30):
        if not ps(): return
        time.sleep(0.5)

def preload_llm():
    post("/api/chat", {"model": "qwen2.5:7b", "messages": [{"role": "user", "content": "di hola"}], "stream": False,
                       "options": {"num_ctx": 16384, "num_predict": 4}, "keep_alive": "30m"})

def run(texts, num_gpu, batch=16, label=""):
    opts = {} if num_gpu is None else {"num_gpu": num_gpu}
    extra = {"options": opts} if opts else {}
    docs = ["title: none | text: " + t for t in texts]
    t0 = time.time()
    post("/api/embed", {"model": MODEL, "input": docs[:1], "keep_alive": "2m", **extra})
    cold = time.time() - t0
    lat = []; t1 = time.time()
    for i in range(0, len(docs), batch):
        t = time.time()
        post("/api/embed", {"model": MODEL, "input": docs[i:i + batch], "keep_alive": "2m", **extra})
        lat.append(time.time() - t)
    tot = time.time() - t1
    q = []
    for _ in range(12):
        t = time.time()
        post("/api/embed", {"model": MODEL, "input": ["task: search result | query: Apple Inc. AAPL results, outlook, guidance and risks"], "keep_alive": "2m", **extra})
        q.append((time.time() - t) * 1000)
    return {"label": label, "n": len(docs), "batch": batch, "cold_first_call_s": round(cold, 2), "chunks_per_s": round(len(docs) / tot, 1),
            "batch_lat_p50_s": round(statistics.median(lat), 3), "batch_lat_max_s": round(max(lat), 3),
            "query_ms_p50": round(statistics.median(q[2:]), 1), "query_ms_max": round(max(q[2:]), 1), "ps_after": ps(), "vram_mib_after": vram()}

if __name__ == "__main__":
    db, n = sys.argv[1], int(sys.argv[2])
    texts = [r[0] for r in sqlite3.connect(db).execute("SELECT text FROM chunks WHERE length(text) >= ? ORDER BY RANDOM() LIMIT ?", (int(sys.argv[4]) if len(sys.argv) > 4 else 0, n))]
    print("chunks:", len(texts), "chars medios:", round(statistics.mean(map(len, texts))), flush=True)
    out = []
    unload_all(); print("base VRAM", vram(), flush=True)
    for name, ng, llm in [("GPU sin LLM", None, False), ("CPU sin LLM", 0, False), ("GPU con LLM cargado", None, True), ("CPU con LLM cargado", 0, True)]:
        unload_all()
        if llm: preload_llm()
        before = (ps(), vram())
        r = run(texts, ng, label=name); r["before"] = before
        print(json.dumps(r, ensure_ascii=False), flush=True); out.append(r)
    unload_all()
    json.dump(out, open(sys.argv[3], "w"), indent=1)
