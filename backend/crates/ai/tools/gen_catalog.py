#!/usr/bin/env python3 -I
"""Regenera `crates/ai/models.json` consultando el registro de Ollama.

Uso (desde `backend/`):
    python3 -I crates/ai/tools/gen_catalog.py --out crates/ai/models.json

`--out` escribe el fichero solo si TODO el catálogo se verificó (sin él, el JSON sale por stdout; no
redirijas `> models.json` a mano: el shell lo vacía antes de que el script falle). Los avisos y la
tabla de verificación salen por stderr.

Para cada tag de `CURATED`:
  1. descarga el manifest (`registry.ollama.ai/v2/library/<nombre>/manifests/<tag>`); si el tag no
     existe (404) el script FALLA: no se admiten tags sin verificar;
  2. suma el tamaño de todas las capas (= lo que descarga `ollama pull`: pesos, proyector de visión,
     modelo borrador si lo hay);
  3. lee el blob de configuración (parámetros, cuantización, familia, `renderer`/`parser`);
  4. tools y razonamiento, igual que Ollama (`server/images.go` y `model/parsers/`, v0.40.3):
     plantilla Go con `.Tools` / «think», `capabilities` de la configuración o, en los modelos nuevos
     (plantilla `{{ .Prompt }}` + `parser` en la configuración), la tabla `PARSERS` de abajo. Sin
     ninguna evidencia el modelo se descarta con aviso (no se admiten tools sin verificar);
  5. lee la cabecera GGUF del blob del modelo (petición Range, unos KB) y calcula capa a capa cuántas
     capas guardan KV (globales / con ventana) y sus bytes por token: ver `kv_profile`.
Solo la descripción, el nivel y los parámetros activos de los MoE son editoriales.
"""
import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gguf_probe  # noqa: E402

REG = "https://registry.ollama.ai/v2/library"
ACCEPT = {"Accept": "application/vnd.docker.distribution.manifest.v2+json"}

# Parsers integrados de Ollama → (tools, razonamiento). Fuente: `HasToolSupport` y
# `HasThinkingSupport` en ollama v0.40.3, `model/parsers/{qwen35,gemma4,nemotron3nano,glimmer,ministral}.go`.
# Ollama deriva de ahí las capacidades de los modelos cuya plantilla es solo `{{ .Prompt }}`.
PARSERS = {
    "qwen3.5": (True, True),
    "gemma4": (True, True),
    "ministral": (True, False),
    "gemma4-no-thinking": (True, False),
    "nemotron-3.5-nano": (True, True),
    "glimmer": (True, True),
}

# (tag, nivel, nombre, parámetros activos si es MoE, descripción)
CURATED = [
    # --- light ---
    ("qwen3:1.7b", "light", "Qwen3 1.7B", None, "Muy rápido y diminuto; razona poco. Solo para equipos modestos: los análisis serán superficiales."),
    ("qwen3:4b", "light", "Qwen3 4B", None, "El mejor equilibrio entre tamaño y calidad en la gama ligera; tool calling fiable."),
    ("llama3.2:3b", "light", "Llama 3.2 3B", None, "Compacto y veloz, bueno siguiendo instrucciones cortas; contexto de análisis limitado."),
    ("phi4-mini:3.8b", "light", "Phi-4 mini 3.8B", None, "Entrenado para razonamiento y matemáticas en poco tamaño; conocimiento del mundo escaso."),
    ("qwen3.5:0.8b", "light", "Qwen3.5 0.8B", None, "El más pequeño de Qwen3.5 (752M parámetros, Q8_0 por defecto): híbrido de capas recurrentes y atención, multimodal, contexto de 256K."),
    ("qwen3.5:2b", "light", "Qwen3.5 2B", None, "Qwen3.5 de 1,9B (Q8_0 por defecto): híbrido de capas recurrentes y atención, multimodal, contexto de 256K."),
    ("qwen3.5:4b", "light", "Qwen3.5 4B", None, "Qwen3.5 de 4,2B: híbrido de capas recurrentes y atención (8 de 32 capas guardan KV), multimodal, contexto de 256K."),
    ("gemma4:e2b", "light", "Gemma 4 E2B", None, "Gemma 4 E2B: 4,6B de parámetros con embeddings por capa («E» = efectivos); multimodal con audio, atención con ventana y KV compartido entre capas."),
    # --- balanced ---
    ("qwen2.5:7b", "balanced", "Qwen2.5 7B", None, "Modelo por defecto de la app: sólido con tools y JSON estructurado, cabe en 8 GB de VRAM."),
    ("qwen3:8b", "balanced", "Qwen3 8B", None, "Mejor razonamiento que Qwen2.5 7B; con «think» activado es más lento pero más cuidadoso."),
    ("qwen3.5:9b", "balanced", "Qwen3.5 9B", None, "Qwen3.5 de 9B: híbrido de capas recurrentes y atención (8 de 32 capas guardan KV), multimodal, contexto de 256K."),
    ("gemma4:e4b", "balanced", "Gemma 4 E4B", None, "Gemma 4 E4B: 7,5B de parámetros con embeddings por capa; multimodal con audio, atención con ventana y KV compartido entre capas."),
    ("llama3.1:8b", "balanced", "Llama 3.1 8B", None, "Generalista y estable con tool calling; buen estilo de redacción."),
    ("ministral-3:8b", "balanced", "Ministral 3 8B", None, "Modelo compacto de Mistral con tools; buena calidad en varios idiomas."),
    ("granite3.3:8b", "balanced", "Granite 3.3 8B", None, "Modelo empresarial de IBM; conservador y obediente con formatos estructurados."),
    ("deepseek-r1:8b", "balanced", "DeepSeek-R1 8B (destilado)", None, "Razonamiento largo paso a paso; muy lento para analizar y flojo con tools."),
    # --- heavy ---
    ("qwen3:14b", "heavy", "Qwen3 14B", None, "Salto claro de calidad en análisis; necesita unos 12 GB de VRAM para ir a toda velocidad."),
    ("qwen2.5:14b", "heavy", "Qwen2.5 14B", None, "Muy fiable con salida JSON y tools; sin modo razonamiento."),
    ("mistral-nemo:12b", "heavy", "Mistral Nemo 12B", None, "Contexto largo y buen multilingüe; algo menos preciso con cifras."),
    ("ministral-3:14b", "heavy", "Ministral 3 14B", None, "Versión grande de Ministral 3; equilibrio entre velocidad y calidad."),
    ("gemma4:12b", "heavy", "Gemma 4 12B", None, "Gemma 4 denso de 11,9B; multimodal con audio, atención con ventana en 40 de 48 capas, contexto de 256K."),
    ("gpt-oss:20b", "heavy", "gpt-oss 20B (MoE)", "3.6B", "MoE de OpenAI con razonamiento configurable; rápido para su tamaño aunque no quepa en 8 GB de VRAM."),
    ("qwen3:30b-a3b", "heavy", "Qwen3 30B-A3B (MoE)", "3.3B", "MoE de 30B con solo 3.3B activos: calidad de modelo grande a velocidad razonable si hay 24 GB de RAM."),
    ("gemma4:26b-a4b", "heavy", "Gemma 4 26B-A4B (MoE)", "4B", "Gemma 4 MoE de 25,2B con 8 de 128 expertos activos (≈4B); multimodal con audio, atención con ventana, contexto de 256K."),
    ("mistral-small3.2:24b", "heavy", "Mistral Small 3.2 24B", None, "Denso de 24B muy bueno siguiendo instrucciones y llamando herramientas."),
    ("qwen3.5:27b", "heavy", "Qwen3.5 27B", None, "Qwen3.5 denso de 27B: híbrido de capas recurrentes y atención (16 de 64 capas guardan KV), multimodal, contexto de 256K."),
    ("qwen3.6:27b", "heavy", "Qwen3.6 27B", None, "Qwen3.6 denso de 27B, misma arquitectura híbrida que Qwen3.5; multimodal, contexto de 256K."),
    ("qwen3.8:27b", "heavy", "Qwen3.8 27B", None, "Qwen3.8 denso de 27B, misma arquitectura híbrida que Qwen3.5; multimodal, contexto de 256K."),
    ("muse-glimmer:30b", "heavy", "Muse Glimmer 30B", None, "Modelo abierto de Meta para agentes locales (27,9B densos); multimodal, atención con ventana en 39 de 52 capas, contexto de 128K."),
    ("qwen3.5:35b-a3b", "heavy", "Qwen3.5 35B-A3B (MoE)", "3B", "Qwen3.5 MoE de 34,7B con 8 de 256 expertos activos (≈3B); híbrido, multimodal, contexto de 256K."),
    ("qwen3.6:35b-a3b", "heavy", "Qwen3.6 35B-A3B (MoE)", "3B", "Qwen3.6 MoE de 35,5B con ≈3B activos; misma arquitectura híbrida que Qwen3.5 35B-A3B."),
    ("nemotron-3.5-lightning:30b-a3b", "heavy", "Nemotron 3.5 Lightning 30B-A3B (MoE)", "3B", "MoE híbrido con capas SSM de NVIDIA (32,9B, ≈3B activos); solo 6 de 52 capas guardan KV; contexto de hasta 1M."),
    # --- max ---
    ("gemma4:31b", "max", "Gemma 4 31B", None, "Gemma 4 denso de 30,7B; multimodal con audio, atención con ventana en 50 de 60 capas, contexto de 256K."),
    ("qwen3:32b", "max", "Qwen3 32B", None, "Denso de 32B con razonamiento; pide 24 GB de VRAM o mucha RAM y paciencia."),
    ("gpt-oss:120b", "max", "gpt-oss 120B (MoE)", "5.1B", "MoE grande de OpenAI; calidad de referencia en local, necesita 64 GB de RAM."),
    ("llama3.3:70b", "max", "Llama 3.3 70B", None, "Denso de 70B muy capaz; solo viable con 48 GB de VRAM o mucha RAM (lento)."),
    ("qwen2.5:72b", "max", "Qwen2.5 72B", None, "Denso de 72B, excelente con JSON y tools; muy pesado."),
    ("deepseek-r1:70b", "max", "DeepSeek-R1 70B (destilado)", None, "Razonamiento profundo en 70B; extremadamente lento sin GPU grande."),
    ("qwen3.5:122b-a10b", "max", "Qwen3.5 122B-A10B (MoE)", "10B", "Qwen3.5 MoE de 125B con ≈10B activos; 81 GB en disco."),
]


def get(url, headers=None, tries=4):
    for i in range(tries):
        try:
            return urllib.request.urlopen(urllib.request.Request(url, headers=headers or {}), timeout=60)
        except urllib.error.HTTPError:
            raise  # 404 y similares: no se reintenta
        except OSError:
            if i == tries - 1:
                raise
            time.sleep(2 * (i + 1))


def capabilities(cfg, tmpl):
    """(tools, razonamiento, origen) como los deduce Ollama; `tools` None si no hay evidencia."""
    caps = cfg.get("capabilities") or []
    tools, think, src = False, "think" in tmpl.lower(), []
    if ".Tools" in tmpl:
        tools = True
        src.append("plantilla")
    if "tools" in caps:
        tools = True
        src.append("config")
    think = think or "thinking" in caps
    parser = cfg.get("parser")
    if parser in PARSERS:
        p_tools, p_think = PARSERS[parser]
        tools, think = tools or p_tools, think or p_think
        src.append(f"parser:{parser}")
    elif parser:
        print(f"  aviso: parser «{parser}» no está en PARSERS; solo cuenta la plantilla", file=sys.stderr)
    return tools, think, "+".join(src)


def kv_profile(meta, mtp_active):
    """Perfil de KV capa a capa a partir de la cabecera GGUF.

    No guardan KV (por token): las capas recurrentes/SSM (`recurrent_layers`, `full_attention_interval`
    o 0 cabezas KV en una lista por capa), las que comparten KV con otra (`attention.shared_kv_layers`,
    las últimas) y las capas NextN/MTP, que solo se cuentan si la cabeza MTP está activa por defecto
    (con su propio KV, a dimensiones globales: lado seguro). Las capas con ventana se identifican con
    `attention.sliding_window_pattern` (True = ventana), o por regla fija en gemma3 (5:1) y gpt-oss (1:1).
    """
    arch = meta["general.architecture"]
    k = lambda s, d=None: meta.get(f"{arch}.{s}", d)
    blocks = k("block_count")
    nextn = k("nextn_predict_layers", 0) or 0
    heads = k("attention.head_count")
    kvh = k("attention.head_count_kv", heads)
    key_g = k("attention.key_length") or k("embedding_length") // heads
    val_g = k("attention.value_length", key_g)
    key_s = k("attention.key_length_swa", key_g)
    val_s = k("attention.value_length_swa", val_g)
    pattern = k("attention.sliding_window_pattern")
    recurrent = k("recurrent_layers")
    interval = k("full_attention_interval")
    shared = k("attention.shared_kv_layers", 0) or 0
    heads_at = lambda i: kvh[i] if isinstance(kvh, list) else kvh
    kv_max = max(kvh) if isinstance(kvh, list) else kvh
    full_n = swa_n = full_b = swa_b = 0
    for i in range(blocks - nextn - shared):
        if recurrent:
            if recurrent[i]:
                continue
            if interval and (i + 1) % interval:
                raise SystemExit(f"recurrent_layers y full_attention_interval no concuerdan en la capa {i}")
        elif interval and (i + 1) % interval:
            continue
        h = heads_at(i)
        if not h:
            continue
        if pattern:
            is_swa = bool(pattern[i])
        elif arch == "gemma3":
            is_swa = (i + 1) % 6 != 0
        elif arch == "gpt-oss":
            is_swa = i % 2 == 0
        else:
            is_swa = False
        if is_swa:
            swa_n += 1
            swa_b += h * (key_s + val_s) * 2
        else:
            full_n += 1
            full_b += h * (key_g + val_g) * 2
    if nextn and mtp_active:
        full_n += nextn
        full_b += nextn * kv_max * (key_g + val_g) * 2
    if not full_n and not swa_n:
        raise SystemExit(f"{arch}: ninguna capa con KV; cabecera no reconocida")
    window = k("attention.sliding_window")
    return {
        "architecture": arch,
        "blocks": blocks,
        "kvHeads": kv_max,
        "keyLength": key_g,
        "valueLength": val_g,
        "fullAttentionLayers": full_n,
        "swaLayers": swa_n,
        "slidingWindow": window if swa_n else None,
        "kvBytesPerTokenFull": full_b,
        "kvBytesPerTokenSwa": swa_b,
        "recurrentLayers": blocks - nextn - shared - (full_n - (nextn if mtp_active else 0)) - swa_n,
        "sharedKvLayers": shared,
    }


def entry(tag, tier, name, active, desc):
    fam, t = tag.split(":")
    try:
        man = json.load(get(f"{REG}/{fam}/manifests/{t}", ACCEPT))
    except Exception as e:  # tag inexistente => se aborta
        sys.exit(f"TAG NO VERIFICADO {tag}: {e}")
    layers = man["layers"]
    disk = sum(l["size"] for l in layers) + man["config"]["size"]
    cfg = json.load(get(f"{REG}/{fam}/blobs/{man['config']['digest']}"))
    by_kind = {l["mediaType"].rsplit(".", 1)[-1]: l for l in layers}
    model_layer = by_kind["model"]
    tmpl = get(f"{REG}/{fam}/blobs/{by_kind['template']['digest']}").read().decode("utf-8", "replace") if "template" in by_kind else ""
    params = json.load(get(f"{REG}/{fam}/blobs/{by_kind['params']['digest']}")) if "params" in by_kind else {}
    tools, think, src = capabilities(cfg, tmpl)
    if not tools:
        # Sin soporte de tools verificable: no entra en el catálogo.
        print(f"SALTADO {tag}: ni su plantilla ni su parser declaran tool calling", file=sys.stderr)
        return None
    meta = gguf_probe.fetch_meta(model_layer["digest"], f"library/{fam}")
    nextn = meta.get(f"{meta['general.architecture']}.nextn_predict_layers", 0) or 0
    draft_n = params.get("draft_num_predict")
    # MTP embebido que Ollama activa por defecto: la configuración del modelo fija `draft_num_predict` > 0.
    mtp_active = bool(nextn and draft_n)
    arch = kv_profile(meta, mtp_active)
    e = {
        "id": tag,
        "name": name,
        "family": cfg["model_family"],
        "parameters": cfg["model_type"],
        "activeParameters": active,
        "quantization": cfg["file_type"],
        "contextWindow": meta[f"{arch['architecture']}.context_length"],
        "tier": tier,
        "supportsTools": tools,
        "reasoning": think,
        "description": desc,
        "diskBytes": disk,
        "arch": arch,
        "manifestDigest": model_layer["digest"],
    }
    spec = {}
    if nextn:
        spec["embeddedMtpLayers"] = nextn
    if "draft" in by_kind:
        spec["draftModel"] = cfg.get("draft", {}).get("architecture", "?")
        spec["draftBytes"] = by_kind["draft"]["size"]
    if draft_n is not None:
        spec["draftNumPredict"] = draft_n
    if spec:
        e["speculative"] = spec
    e["capabilitiesSource"] = src
    kinds = " ".join(f"{k}={l['size'] / 1e9:.2f}" for k, l in by_kind.items() if l["size"] > 1e7)
    print(
        f"ok {tag:32s} {disk / 1e9:7.2f} GB tools={tools!s:5} think={think!s:5} {arch['architecture']:14s}"
        f" kv={arch['kvBytesPerTokenFull']}+{arch['kvBytesPerTokenSwa']}B/tok [{kinds}] {src}"
        + (f" spec={spec}" if spec else ""),
        file=sys.stderr,
    )
    return e


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", help="escribe aquí el JSON solo si todo se verificó (por defecto, stdout)")
    args = ap.parse_args()
    models = [m for m in (entry(*c) for c in CURATED) if m]
    ids = [m["id"] for m in models]
    if len(set(ids)) != len(ids):
        sys.exit("ids duplicados en CURATED")
    out = {
        "verifiedAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%MZ"),
        "source": "https://registry.ollama.ai/v2/library/<nombre>/manifests/<tag>",
        "models": models,
    }
    text = json.dumps(out, ensure_ascii=False, indent=1) + "\n"
    if args.out:
        tmp = args.out + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, args.out)
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
