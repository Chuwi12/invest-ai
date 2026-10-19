#!/usr/bin/env python3
"""Proxy mínimo para medir `options.num_gpu` SIN tocar el código de producción: reenvía todo a Ollama y, en /api/chat y /api/generate,
fuerza `options.num_gpu`. Se usa apuntando el harness con LLM_BASE_URL=http://127.0.0.1:<puerto>.
   gpu_proxy.py <puerto> <num_gpu>"""
import http.server, json, socketserver, sys, urllib.request

UP = "http://127.0.0.1:11434"
PORT, NUM_GPU = int(sys.argv[1]), int(sys.argv[2])

class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else None
        if body and self.path in ("/api/chat", "/api/generate"):
            j = json.loads(body)
            j.setdefault("options", {})["num_gpu"] = NUM_GPU
            body = json.dumps(j).encode()
        req = urllib.request.Request(UP + self.path, data=body, method=self.command, headers={"Content-Type": "application/json"})
        try:
            r = urllib.request.urlopen(req, timeout=900)
        except urllib.error.HTTPError as e:
            data = e.read()
            self.send_response(e.code); self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data); return
        self.send_response(r.status)
        self.send_header("Content-Type", r.headers.get("Content-Type", "application/json"))
        self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
        while True:
            chunk = r.read1(65536) if hasattr(r, "read1") else r.read(65536)
            if not chunk: break
            self.wfile.write(b"%x\r\n" % len(chunk) + chunk + b"\r\n"); self.wfile.flush()
        self.wfile.write(b"0\r\n\r\n")
    do_GET = do_POST = do_DELETE = _go

class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True

S(("127.0.0.1", PORT), H).serve_forever()
