#!/usr/bin/env python3
"""Ajuste en VRAM, velocidad y sondeo de «think» de un modelo de Ollama con la configuración de la app.
   bench_model.py <modelo> [num_ctx=16384] [num_gpu] > salida.json
   (num_gpu opcional: fuerza ese nº de capas en GPU, 999 = todas; sin él, el reparto lo decide Ollama como en la app)
Descarga los demás modelos, carga <modelo> con num_ctx, y mide:
 - /api/ps: size, size_vram (reparto GPU/CPU) y nvidia-smi (MiB usados, restando la línea base sin modelo);
 - velocidad: 3 peticiones con ~2500 tokens de prompt y 256 de salida (prefill tok/s y generación tok/s, de los contadores de Ollama; la variante va AL PRINCIPIO para que la caché de prompt no falsee el prefill);
 - think: capacidades (/api/show), y una petición con `think:false` (como la app) a un problema que invita a razonar:
   ¿hay campo `thinking`? ¿`<think>` en el contenido? ¿cuántos tokens de salida?"""
import json, subprocess, sys, threading, time, urllib.request

BASE = "http://127.0.0.1:11434"

def call(path, body=None, timeout=600):
    req = urllib.request.Request(BASE + path, data=None if body is None else json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())

def vram_used():
    out = subprocess.run(["nvidia-smi", "--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits"], capture_output=True, text=True).stdout.split(",")
    return int(out[0]), int(out[1])

def main():
    model = sys.argv[1]
    ctx = int(sys.argv[2]) if len(sys.argv) > 2 else 16384
    num_gpu = int(sys.argv[3]) if len(sys.argv) > 3 else None
    for m in call("/api/ps").get("models", []):
        call("/api/generate", {"model": m["name"], "keep_alive": 0})
    time.sleep(3)
    base_used, total = vram_used()
    caps = call("/api/show", {"model": model}).get("capabilities", [])
    opts = {"num_ctx": ctx, "temperature": 0.2}
    if num_gpu is not None:
        opts["num_gpu"] = num_gpu
    t0 = time.time()
    chat_think = {"think": False} if "thinking" in caps else {}
    call("/api/chat", {"model": model, "messages": [{"role": "user", "content": "Di hola."}], "stream": False, "options": {**opts, "num_predict": 8}, "keep_alive": "30m", **chat_think})
    load_s = time.time() - t0
    time.sleep(2)
    ps = [m for m in call("/api/ps")["models"] if m["name"] == model][0]
    used, _ = vram_used()
    filler = ("El precio de cierre fue 341.27 con RSI 60.5, SMA50 323.79 y SMA200 291.42; el margen operativo fue 32.6% y el crecimiento de ingresos 16.4%. " * 40)
    speeds = []
    for i in range(3):
        r = call("/api/chat", {"model": model, "messages": [{"role": "user", "content": f"Variante {i}.\n" + filler + "\nEscribe un resumen de cinco frases sobre estos datos."}], "stream": False,
                               "options": {**opts, "num_predict": 256}, "keep_alive": "30m", **chat_think})
        speeds.append({"promptTokens": r["prompt_eval_count"], "prefillTokPerS": r["prompt_eval_count"] / (r["prompt_eval_duration"] / 1e9),
                       "genTokens": r["eval_count"], "genTokPerS": r["eval_count"] / (r["eval_duration"] / 1e9), "loadS": r.get("load_duration", 0) / 1e9})
    q = "Un tren sale de A a las 9:00 a 80 km/h y otro de B (300 km) a las 9:30 a 100 km/h hacia A. ¿A qué hora se cruzan? Razona paso a paso y da la respuesta."
    probe = call("/api/chat", {"model": model, "messages": [{"role": "user", "content": q}], "stream": False, "options": {**opts, "num_predict": 600}, "keep_alive": "30m", **chat_think})
    msg = probe["message"]
    probe_out = {"thinkParamSent": chat_think or None, "thinkingChars": len(msg.get("thinking") or ""), "thinkTagInContent": "<think>" in msg.get("content", ""),
                 "evalTokens": probe["eval_count"], "contentChars": len(msg.get("content", "")), "doneReason": probe.get("done_reason")}
    # Contexto casi lleno (~14k tokens de prompt): el pico de memoria es mayor que al cargar. Se muestrea nvidia-smi durante la petición.
    peak = {"mib": 0}
    stop = threading.Event()
    def sampler():
        while not stop.is_set():
            peak["mib"] = max(peak["mib"], vram_used()[0])
            time.sleep(0.5)
    th = threading.Thread(target=sampler)
    th.start()
    stress = {}
    try:
        big = ("El precio de cierre fue 341.27 con RSI 60.5, SMA50 323.79 y SMA200 291.42; el margen operativo fue 32.6% y el crecimiento de ingresos 16.4%. " * 200)
        t1 = time.time()
        r = call("/api/chat", {"model": model, "messages": [{"role": "user", "content": "Variante 9.\n" + big + "\nResume en tres frases."}], "stream": False,
                               "options": {**opts, "num_predict": 128}, "keep_alive": "30m", **chat_think})
        stress = {"ok": True, "promptTokens": r["prompt_eval_count"], "prefillTokPerS": r["prompt_eval_count"] / (r["prompt_eval_duration"] / 1e9),
                  "genTokPerS": r["eval_count"] / (r["eval_duration"] / 1e9), "seconds": round(time.time() - t1, 1)}
    except Exception as e:  # p. ej. 500 por falta de memoria
        stress = {"ok": False, "error": str(e)[:300]}
    stop.set()
    th.join()
    stress["peakMiB"] = peak["mib"]
    ps2 = [m for m in call("/api/ps")["models"] if m["name"] == model]
    stress["psAfter"] = {"size": ps2[0]["size"], "sizeVram": ps2[0]["size_vram"]} if ps2 else None
    json.dump({"model": model, "numGpu": num_gpu, "stressLongPrompt": stress, "numCtx": ctx, "capabilities": caps, "loadSeconds": round(load_s, 1),
               "ps": {"size": ps["size"], "sizeVram": ps["size_vram"], "gpuShare": ps["size_vram"] / ps["size"], "contextLength": ps.get("context_length")},
               "nvidiaMiB": {"baseline": base_used, "withModel": used, "model": used - base_used, "total": total},
               "speed": speeds, "thinkProbe": probe_out}, sys.stdout, indent=1, ensure_ascii=False)

main()
