#!/usr/bin/env python3 -I
"""Regenera `crates/ai/models.json` consultando el registro de Ollama.

Uso:  python3 -I crates/ai/tools/gen_catalog.py > crates/ai/models.json

Para cada tag de `CURATED`:
  1. descarga el manifest (`registry.ollama.ai/v2/library/<nombre>/manifests/<tag>`); si el tag no
     existe (404) el script FALLA: no se admiten tags sin verificar;
  2. suma el tamaño de todas las capas (= lo que descarga `ollama pull`);
  3. lee el blob de configuración (parámetros, cuantización, familia);
  4. mira la plantilla: si usa `.Tools` el modelo admite tool calling (si no, se descarta); si menciona «think» razona;
  5. lee la cabecera GGUF del blob del modelo (petición Range, unos KB) para obtener capas, cabezas
     KV y dimensión de cabeza, con los que `ai::models` estima el KV cache.
Solo la descripción, el nivel y los parámetros activos de los MoE son editoriales.
"""
import json
import os
import sys
import urllib.request
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gguf_probe  # noqa: E402

REG = "https://registry.ollama.ai/v2/library"
ACCEPT = {"Accept": "application/vnd.docker.distribution.manifest.v2+json"}

# (tag, nivel, nombre, parámetros activos si es MoE, descripción)
CURATED = [
    ("qwen3:1.7b", "light", "Qwen3 1.7B", None, "Muy rápido y diminuto; razona poco. Solo para equipos modestos: los análisis serán superficiales."),
    ("qwen3:4b", "light", "Qwen3 4B", None, "El mejor equilibrio entre tamaño y calidad en la gama ligera; tool calling fiable."),
    ("llama3.2:3b", "light", "Llama 3.2 3B", None, "Compacto y veloz, bueno siguiendo instrucciones cortas; contexto de análisis limitado."),
    ("phi4-mini:3.8b", "light", "Phi-4 mini 3.8B", None, "Entrenado para razonamiento y matemáticas en poco tamaño; conocimiento del mundo escaso."),
    ("qwen3.5:4b", "light", "Qwen3.5 4B", None, "Generación más reciente de Qwen en tamaño ligero; atención híbrida con KV cache pequeño."),
    ("qwen2.5:7b", "balanced", "Qwen2.5 7B", None, "Modelo por defecto de la app: sólido con tools y JSON estructurado, cabe en 8 GB de VRAM."),
    ("qwen3:8b", "balanced", "Qwen3 8B", None, "Mejor razonamiento que Qwen2.5 7B; con «think» activado es más lento pero más cuidadoso."),
    ("qwen3.5:9b", "balanced", "Qwen3.5 9B", None, "Sucesor de Qwen3 8B con contexto largo y KV cache muy ligero."),
    ("llama3.1:8b", "balanced", "Llama 3.1 8B", None, "Generalista y estable con tool calling; buen estilo de redacción."),
    ("ministral-3:8b", "balanced", "Ministral 3 8B", None, "Modelo compacto de Mistral con tools; buena calidad en varios idiomas."),
    ("granite3.3:8b", "balanced", "Granite 3.3 8B", None, "Modelo empresarial de IBM; conservador y obediente con formatos estructurados."),
    ("deepseek-r1:8b", "balanced", "DeepSeek-R1 8B (destilado)", None, "Razonamiento largo paso a paso; muy lento para analizar y flojo con tools."),
    ("qwen3:14b", "heavy", "Qwen3 14B", None, "Salto claro de calidad en análisis; necesita unos 12 GB de VRAM para ir a toda velocidad."),
    ("qwen2.5:14b", "heavy", "Qwen2.5 14B", None, "Muy fiable con salida JSON y tools; sin modo razonamiento."),
    ("mistral-nemo:12b", "heavy", "Mistral Nemo 12B", None, "Contexto largo y buen multilingüe; algo menos preciso con cifras."),
    ("ministral-3:14b", "heavy", "Ministral 3 14B", None, "Versión grande de Ministral 3; equilibrio entre velocidad y calidad."),
    ("gpt-oss:20b", "heavy", "gpt-oss 20B (MoE)", "3.6B", "MoE de OpenAI con razonamiento configurable; rápido para su tamaño aunque no quepa en 8 GB de VRAM."),
    ("qwen3:30b-a3b", "heavy", "Qwen3 30B-A3B (MoE)", "3.3B", "MoE de 30B con solo 3.3B activos: calidad de modelo grande a velocidad razonable si hay 24 GB de RAM."),
    ("mistral-small3.2:24b", "heavy", "Mistral Small 3.2 24B", None, "Denso de 24B muy bueno siguiendo instrucciones y llamando herramientas."),
    ("qwen3.5:27b", "heavy", "Qwen3.5 27B", None, "Denso de 27B con atención híbrida; el mejor análisis en su categoría."),
    ("qwen3:32b", "max", "Qwen3 32B", None, "Denso de 32B con razonamiento; pide 24 GB de VRAM o mucha RAM y paciencia."),
    ("gpt-oss:120b", "max", "gpt-oss 120B (MoE)", "5.1B", "MoE grande de OpenAI; calidad de referencia en local, necesita 64 GB de RAM."),
    ("llama3.3:70b", "max", "Llama 3.3 70B", None, "Denso de 70B muy capaz; solo viable con 48 GB de VRAM o mucha RAM (lento)."),
    ("qwen2.5:72b", "max", "Qwen2.5 72B", None, "Denso de 72B, excelente con JSON y tools; muy pesado."),
    ("deepseek-r1:70b", "max", "DeepSeek-R1 70B (destilado)", None, "Razonamiento profundo en 70B; extremadamente lento sin GPU grande."),
    ("qwen3.5:122b", "max", "Qwen3.5 122B (MoE)", None, "MoE enorme para estaciones de trabajo con más de 96 GB de memoria."),
]


def get(url, headers=None):
    return urllib.request.urlopen(urllib.request.Request(url, headers=headers or {}), timeout=60)


def main():
    models = []
    for tag, tier, name, active, desc in CURATED:
        fam, t = tag.split(":")
        try:
            man = json.load(get(f"{REG}/{fam}/manifests/{t}", ACCEPT))
        except Exception as e:  # tag inexistente => se aborta
            sys.exit(f"TAG NO VERIFICADO {tag}: {e}")
        layers = man["layers"]
        disk = sum(l["size"] for l in layers) + man["config"]["size"]
        cfg = json.load(get(f"{REG}/{fam}/blobs/{man['config']['digest']}"))
        model_layer = next(l for l in layers if l["mediaType"].endswith("image.model"))
        tmpl_layer = next((l for l in layers if l["mediaType"].endswith("image.template")), None)
        tmpl = get(f"{REG}/{fam}/blobs/{tmpl_layer['digest']}").read().decode("utf-8", "replace") if tmpl_layer else ""
        meta = gguf_probe.fetch_meta(model_layer["digest"], f"library/{fam}")
        arch = meta["general.architecture"]
        k = lambda s, d=None: meta.get(f"{arch}.{s}", d)
        blocks = k("block_count")
        heads = k("attention.head_count")
        kv_heads = k("attention.head_count_kv", heads)
        if isinstance(kv_heads, list):  # algunas arquitecturas dan un valor por capa
            kv_heads = max(kv_heads)
        key_len = k("attention.key_length") or k("embedding_length") // heads
        val_len = k("attention.value_length", key_len)
        window = k("attention.sliding_window")
        # Capas con atención global (las demás son SWA o recurrentes y casi no usan KV).
        if k("full_attention_interval"):
            full, swa = blocks // k("full_attention_interval"), 0
        elif arch == "gemma3":
            full = blocks // 6
            swa = blocks - full
        elif arch == "gpt-oss":
            full = blocks // 2
            swa = blocks - full
        else:
            full, swa = blocks, 0
        per_layer = kv_heads * (key_len + val_len) * 2  # bytes por token y capa en f16
        if ".Tools" not in tmpl:
            # Sin soporte de tools verificable en la plantilla: no entra en el catálogo.
            print(f"SALTADO {tag}: su plantilla no declara tool calling", file=sys.stderr)
            continue
        models.append({
            "id": tag,
            "name": name,
            "family": cfg["model_family"],
            "parameters": cfg["model_type"],
            "activeParameters": active,
            "quantization": cfg["file_type"],
            "contextWindow": k("context_length"),
            "tier": tier,
            "supportsTools": ".Tools" in tmpl,
            "reasoning": "think" in tmpl.lower(),
            "description": desc,
            "diskBytes": disk,
            "arch": {
                "architecture": arch,
                "blocks": blocks,
                "kvHeads": kv_heads,
                "keyLength": key_len,
                "valueLength": val_len,
                "fullAttentionLayers": full,
                "swaLayers": swa,
                "slidingWindow": window if swa else None,
                "kvBytesPerTokenFull": per_layer * full,
                "kvBytesPerTokenSwa": per_layer * swa,
            },
            "manifestDigest": model_layer["digest"],
        })
        print(f"ok {tag:24s} {disk/1e9:7.2f} GB tools={models[-1]['supportsTools']!s:5} think={models[-1]['reasoning']!s:5} {arch}", file=sys.stderr)
    out = {
        "verifiedAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%MZ"),
        "source": "https://registry.ollama.ai/v2/library/<nombre>/manifests/<tag>",
        "models": models,
    }
    json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
    print()


if __name__ == "__main__":
    main()
