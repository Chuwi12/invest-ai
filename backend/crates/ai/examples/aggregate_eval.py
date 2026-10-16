#!/usr/bin/env python3
"""Agrega las salidas de run_eval.sh en tablas markdown.
   aggregate_eval.py <dir-variante>[:etiqueta] ...   (cada dir con <activo>_<prof>_<rep>.json)
Imprime, por profundidad: media (min-max) de cada métrica y n; y una tabla por activo."""
import glob, json, os, statistics as st, sys, re

COLS = [  # (cabecera, getter, formato)
    ("s (solo corriente)", lambda m, a: m["wallSeconds"] if m["timeValid"] else None, "{:.0f}"),
    ("llam.", lambda m, a: a["llmCalls"], "{:.1f}"),
    ("tok in", lambda m, a: a["promptTokens"], "{:.0f}"),
    ("tok out", lambda m, a: a["completionTokens"], "{:.0f}"),
    ("cif. tesis", lambda m, a: a["figuresThesisTotal"], "{:.1f}"),
    ("cif. tesis distintas", lambda m, a: a["figuresThesisDistinct"], "{:.1f}"),
    ("verif. etiqueta", lambda m, a: a["labeledOk"], "{:.1f}"),
    ("solo valor", lambda m, a: a["unlabeledOk"], "{:.1f}"),
    ("no verif. (⚠)", lambda m, a: a["notInData"], "{:.1f}"),
    ("etiqueta errónea", lambda m, a: a["wrongLabel"], "{:.1f}"),
    ("descartes", lambda m, a: a["droppedTotal"], "{:.1f}"),
    ("inventadas atrapadas", lambda m, a: a["inventedFiguresCaught"], "{:.1f}"),
    ("reint. genérica", lambda m, a: m["genericThesisRetries"], "{:.1f}"),
    ("correcc. redac.", lambda m, a: m["wordingFixCalls"], "{:.1f}"),
    ("lint final", lambda m, a: len(m["spanishIssuesFinal"]), "{:.1f}"),
    ("veredicto=punt.", lambda m, a: 1 if a["verdictMatchesScore"] else 0, "{:.2f}"),
    ("conf.", lambda m, a: a["confidence"], "{:.2f}"),
]

def valid_time(e, ov):
    """Registro de energía/GPU válido para tiempos. Los registros antiguos (sin `gpuForeign`) se invalidan
    con _timing_overrides.json si empezaron desde `invalidSince` (GPU compartida con un proceso ajeno)."""
    if not e["timingValid"]:
        return False
    if ov and "gpuForeign" not in e and e["start"] >= ov["invalidSince"]:
        return False
    return True

def load(d):
    runs = []
    for f in sorted(glob.glob(os.path.join(d, "*.json"))):
        mt = re.match(r"(.+)_(quick|standard|deep)_(\d+)\.json", os.path.basename(f))
        if not mt:
            continue
        j = json.load(open(f))
        m = j["metrics"]
        if "auto" not in m:
            continue
        # Los tiempos solo valen si hay registro de energía (_env.jsonl) y la ejecución fue con corriente.
        env = {}
        ovp = os.path.join(os.path.dirname(os.path.abspath(d.rstrip("/"))), "_timing_overrides.json")
        ov = json.load(open(ovp)) if os.path.exists(ovp) else None
        ep = os.path.join(d, "_env.jsonl")
        if os.path.exists(ep):
            for line in open(ep):
                e = json.loads(line)
                env[e["file"]] = e
        key = f"{mt.group(1)}_{mt.group(2)}_{mt.group(3)}"
        m = dict(m)
        m["timeValid"] = key in env and valid_time(env[key], ov)
        runs.append((mt.group(1), mt.group(2), int(mt.group(3)), m, m["auto"]))
    return runs

def cell(vals, fmt):
    if not vals:
        return "-"
    mean = st.mean(vals)
    lo, hi = min(vals), max(vals)
    return f"{fmt.format(mean)} ({fmt.format(lo)}–{fmt.format(hi)})"

def table(rows_by_label, depth, per_asset=None):
    out = ["| variante | n | " + " | ".join(c[0] for c in COLS) + " |", "|" + "---|" * (len(COLS) + 2)]
    for label, runs in rows_by_label.items():
        sel = [r for r in runs if r[1] == depth and (per_asset is None or r[0] == per_asset)]
        if not sel:
            continue
        cells = [cell([g(r[3], r[4]) for r in sel if g(r[3], r[4]) is not None], fmt) for _, g, fmt in COLS]
        out.append(f"| {label} | {len(sel)} | " + " | ".join(cells) + " |")
    return "\n".join(out)

if __name__ == "__main__":
    variants = {}
    for arg in sys.argv[1:]:
        d, _, lab = arg.partition(":")
        variants[lab or os.path.basename(d.rstrip("/"))] = load(d)
    for depth in ("quick", "standard", "deep"):
        if any(r[1] == depth for rs in variants.values() for r in rs):
            print(f"\n### {depth} (media (mín–máx), todos los activos)\n")
            print(table(variants, depth))
    assets = sorted({r[0] for rs in variants.values() for r in rs})
    for depth in ("quick", "standard", "deep"):
        for a in assets:
            if any(r[1] == depth and r[0] == a for rs in variants.values() for r in rs):
                print(f"\n#### {a} / {depth}\n")
                print(table(variants, depth, a))
