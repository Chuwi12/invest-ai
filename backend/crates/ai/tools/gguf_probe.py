"""Lectura mínima de la cabecera GGUF de un blob del registro de Ollama (petición con Range).

Solo extrae los metadatos de arquitectura (capas, cabezas KV, dimensión…) para estimar el KV
cache. Se detiene al llegar a las claves `tokenizer.*`, que son enormes y van después.
"""
import struct
import urllib.request

SCALAR = {0: "B", 1: "b", 2: "H", 3: "h", 4: "I", 5: "i", 6: "f", 7: "?", 10: "Q", 11: "q", 12: "d"}


class NeedMore(Exception):
    pass


class Reader:
    def __init__(self, buf):
        self.b, self.o = buf, 0

    def take(self, n):
        if self.o + n > len(self.b):
            raise NeedMore
        r = self.b[self.o:self.o + n]
        self.o += n
        return r

    def unpack(self, fmt):
        return struct.unpack("<" + fmt, self.take(struct.calcsize("<" + fmt)))[0]

    def string(self):
        return self.take(self.unpack("Q")).decode("utf-8", "replace")

    def value(self, t):
        if t in SCALAR:
            return self.unpack(SCALAR[t])
        if t == 8:
            return self.string()
        if t == 9:
            et, n = self.unpack("I"), self.unpack("Q")
            return [self.value(et) for _ in range(n)] if n <= 4096 else self.skip_array(et, n)
        raise ValueError(f"tipo GGUF desconocido {t}")

    def skip_array(self, et, n):
        for _ in range(n):
            self.value(et)
        return None


def parse(buf):
    r = Reader(buf)
    if r.take(4) != b"GGUF":
        raise ValueError("no es GGUF")
    r.unpack("I")
    r.unpack("Q")
    kv = r.unpack("Q")
    out = {}
    for _ in range(kv):
        key = r.string()
        if key.startswith("tokenizer."):
            break
        out[key] = r.value(r.unpack("I"))
    return out


def fetch_meta(digest, name="library/qwen3", size=1 << 20):
    url = f"https://registry.ollama.ai/v2/{name}/blobs/{digest}"
    while size <= 64 << 20:
        req = urllib.request.Request(url, headers={"Range": f"bytes=0-{size - 1}"})
        buf = urllib.request.urlopen(req, timeout=60).read()
        try:
            return parse(buf)
        except NeedMore:
            size *= 4
    raise RuntimeError("cabecera GGUF demasiado grande")
