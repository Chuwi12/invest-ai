#!/usr/bin/env python3
"""Proxy de registro entre la app y Ollama (solo 127.0.0.1). Registra cada peticion con tiempos;
guarda los prompts de /api/chat para leerlos a mano. No altera nada."""
import http.client, json, sys, time, threading, os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]); UP = ("127.0.0.1", 11434)
LOG = open(sys.argv[2], "a", buffering=1); PROMPTS = sys.argv[3]
lock = threading.Lock(); counter = [0]

def log(d):
    with lock:
        LOG.write(json.dumps(d, ensure_ascii=False) + "\n")

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else b""
        t0 = time.time()
        rec = {"t0": t0, "path": self.path, "method": self.command, "port": PORT}
        try:
            j = json.loads(body) if body else {}
        except Exception:
            j = {}
        rec["model"] = j.get("model")
        if self.path.startswith("/api/embed"):
            inp = j.get("input"); rec["n_inputs"] = len(inp) if isinstance(inp, list) else 1
            rec["chars"] = sum(len(x) for x in inp) if isinstance(inp, list) else len(str(inp))
            rec["options"] = j.get("options"); rec["keep_alive"] = j.get("keep_alive")
        if self.path.startswith("/api/chat"):
            with lock:
                counter[0] += 1; k = counter[0]
            rec["seq"] = k
            fn = os.path.join(PROMPTS, f"{int(t0)}_{k:05d}.json")
            open(fn, "w").write(json.dumps(j, ensure_ascii=False))
            rec["prompt_file"] = fn
            rec["n_tools"] = len(j.get("tools") or [])
        c = http.client.HTTPConnection(*UP, timeout=900)
        hdrs = {k: v for k, v in self.headers.items() if k.lower() not in ("host", "connection", "content-length")}
        c.request(self.command, self.path, body=body, headers=hdrs)
        r = c.getresponse()
        self.send_response(r.status)
        for k, v in r.getheaders():
            if k.lower() in ("transfer-encoding", "connection", "content-length"): continue
            self.send_header(k, v)
        self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
        last = b""; first = None; full = bytearray()
        try:
            while True:
                chunk = r.read1(65536)
                if not chunk: break
                if first is None: first = time.time() - t0
                last = (last + chunk)[-4000:]
                if rec.get('prompt_file'): full += chunk
                self.wfile.write(b"%x\r\n" % len(chunk) + chunk + b"\r\n"); self.wfile.flush()
            self.wfile.write(b"0\r\n\r\n"); self.wfile.flush()
        except Exception as e:
            rec["client_error"] = repr(e)
        if rec.get('prompt_file'):
            try:
                txt = ''.join(json.loads(l).get('message', {}).get('content', '') for l in bytes(full).decode('utf8', 'ignore').splitlines() if l.strip())
                open(rec['prompt_file'].replace('.json', '.out.txt'), 'w').write(txt)
            except Exception as e:
                rec['resp_err'] = repr(e)
        rec["status"] = r.status; rec["dur"] = time.time() - t0; rec["first_byte"] = first
        try:
            tail = [l for l in last.decode("utf8", "ignore").splitlines() if l.strip()]
            d = json.loads(tail[-1]) if tail else {}
            for k in ("prompt_eval_count", "eval_count", "total_duration", "load_duration", "prompt_eval_duration", "eval_duration"):
                if k in d: rec[k] = d[k]
        except Exception:
            pass
        log(rec)
    do_GET = do_POST = do_DELETE = do_PUT = do_any

ThreadingHTTPServer.daemon_threads = True
ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
