#!/usr/bin/env python3
"""Informe de ablación en markdown a partir de las salidas de run_eval.sh.

   ablation_report.py <raiz-calidad> [<raiz-tiempos-extra> ...]
     <raiz>/{base,a,b,ab}/<activo>_<prof>_<rep>.json  (+ _env.jsonl, + opcional ../_timing_overrides.json)

Calidad: todas las ejecuciones. Tiempos: solo las de timingValid (enchufado y GPU sin procesos ajenos), de todas las raíces.
Por métrica y profundidad: media [mín–máx] por variante y, frente a `base`, la diferencia de medias con IC 95 % por
bootstrap percentil (10 000 remuestreos, semilla fija, independiente por variante). Una diferencia cuyo IC contiene 0 se
marca «ruido».
"""
import glob, json, os, random, re, statistics as st, sys

VARS = ["base", "a", "b", "ab"]
NAMES = {"base": "base", "a": "+A (quick nuevo)", "b": "+B (cifras derivadas)", "ab": "+A+B"}

def frac_label(a):
    t = a["labeledOk"] + a["unlabeledOk"] + a["notInData"]
    return 100.0 * a["labeledOk"] / t if t else None

def frac_unverified(a):
    t = a["labeledOk"] + a["unlabeledOk"] + a["notInData"]
    return 100.0 * a["notInData"] / t if t else None

QUALITY = [  # (cabecera, getter(m, a), formato)
    ("llamadas LLM", lambda m, a: a["llmCalls"], "{:.1f}"),
    ("tokens in", lambda m, a: a["promptTokens"], "{:.0f}"),
    ("tokens out", lambda m, a: a["completionTokens"], "{:.0f}"),
    ("cifras en tesis", lambda m, a: a["figuresThesisTotal"], "{:.1f}"),
    ("cifras distintas en tesis", lambda m, a: a["figuresThesisDistinct"], "{:.1f}"),
    ("cifras (todo el texto)", lambda m, a: a["figuresAllTotal"], "{:.1f}"),
    ("verificadas por etiqueta", lambda m, a: a["labeledOk"], "{:.1f}"),
    ("verificadas solo por valor", lambda m, a: a["unlabeledOk"], "{:.1f}"),
    ("no verificadas (⚠)", lambda m, a: a["notInData"], "{:.2f}"),
    ("⚠ en resumen/tesis/viñetas/riesgos/catalizadores", lambda m, a: a["_warn_body"], "{:.2f}"),
    ("⚠ en «qué cambiaría» (umbrales hipotéticos)", lambda m, a: a["_warn_wci"], "{:.2f}"),
    ("% verificadas por etiqueta", lambda m, a: frac_label(a), "{:.1f}"),
    ("% no verificadas", lambda m, a: frac_unverified(a), "{:.2f}"),
    ("etiqueta errónea", lambda m, a: a["wrongLabel"], "{:.2f}"),
    ("descartes (afirm.+frases)", lambda m, a: a["droppedTotal"], "{:.2f}"),
    ("cifras inventadas atrapadas", lambda m, a: a["inventedFiguresCaught"], "{:.2f}"),
    ("tesis genérica detectada", lambda m, a: a["_generic_detected"], "{:.2f}"),
    ("reintentos por tesis genérica (llamada extra)", lambda m, a: a["_generic_retries"], "{:.2f}"),
    ("tesis genérica que queda (confianza −0,05)", lambda m, a: a["_generic_unresolved"], "{:.2f}"),
    ("correcciones de redacción", lambda m, a: m["wordingFixCalls"], "{:.2f}"),
    ("errores lint finales", lambda m, a: len(m["spanishIssuesFinal"]), "{:.2f}"),
    ("veredicto = umbral de puntuación", lambda m, a: 1 if a["verdictMatchesScore"] else 0, "{:.2f}"),
    ("confianza", lambda m, a: a["confidence"], "{:.2f}"),
    ("palabras de la tesis", lambda m, a: a["_thesis_words"], "{:.0f}"),
    ("palabras del resumen", lambda m, a: a["_summary_words"], "{:.0f}"),
    ("viñetas alcistas", lambda m, a: a["_bull"], "{:.1f}"),
    ("viñetas bajistas", lambda m, a: a["_bear"], "{:.1f}"),
    ("riesgos", lambda m, a: a["_risks"], "{:.1f}"),
    ("«qué cambiaría»", lambda m, a: a["_wci"], "{:.1f}"),
    ("catalizadores", lambda m, a: a["_catalysts"], "{:.1f}"),
]
TIME = ("segundos (solo tiempos válidos)", lambda m, a: m["wallSeconds"], "{:.1f}")


def parse_name(f):
    mt = re.match(r"(.+)_(quick|standard|deep)_(\d+)\.json$", os.path.basename(f))
    return (mt.group(1), mt.group(2), int(mt.group(3))) if mt else None


def valid_time(e, ov):
    if not e["timingValid"]:
        return False
    if ov and "gpuForeign" not in e and e["start"] >= ov["invalidSince"]:
        return False
    return True


def load(root, var):
    d = os.path.join(root, var)
    ovp = os.path.join(root, "_timing_overrides.json")
    ov = json.load(open(ovp)) if os.path.exists(ovp) else None
    env = {}
    ep = os.path.join(d, "_env.jsonl")
    if os.path.exists(ep):
        for line in open(ep):
            e = json.loads(line)
            env[e["file"]] = e
    out = []
    for f in sorted(glob.glob(os.path.join(d, "*.json"))):
        p = parse_name(f)
        if not p or p[0].endswith("battery"):
            continue
        j = json.load(open(f))
        if "auto" not in j["metrics"]:
            continue
        an = j["analysis"]
        a = dict(j["metrics"]["auto"])
        a["_catalysts"] = len(an.get("catalysts") or [])
        a["_bull"] = len(an.get("bullCase") or [])
        a["_bear"] = len(an.get("bearCase") or [])
        a["_risks"] = len(an.get("risks") or [])
        a["_wci"] = len(an.get("wouldChangeIf") or [])
        a["_thesis_words"] = len((an.get("thesis") or "").split())
        a["_summary_words"] = len((an.get("summary") or "").split())
        ev = j["metrics"].get("events", [])
        a["_generic_detected"] = sum(1 for e in ev if "tesis genérica" in e)
        a["_generic_retries"] = sum(1 for e in ev if "se pide reescribirla" in e)
        a["_generic_unresolved"] = 1 if any("poco específica" in n for n in (an.get("qualityNotes") or [])) else 0
        a["_warn_wci"] = sum(x.count("⚠") for x in (an.get("wouldChangeIf") or []))
        body = [an.get("summary") or "", an.get("thesis") or "", an.get("horizonNote") or ""] + list(an.get("bullCase") or []) + list(an.get("bearCase") or []) + list(an.get("risks") or []) + [c.get("description", "") for c in (an.get("catalysts") or [])]
        a["_warn_body"] = sum(x.count("⚠") for x in body)
        key = "%s_%s_%d" % p
        e = env.get(key)
        out.append({"asset": p[0], "depth": p[1], "rep": p[2], "m": j["metrics"], "a": a,
                    "tv": bool(e and valid_time(e, ov)), "env": e, "root": root})
    return out


def fmt_stats(xs, fmt):
    if not xs:
        return "–"
    return "%s [%s–%s]" % (fmt.format(st.mean(xs)), fmt.format(min(xs)), fmt.format(max(xs)))


def boot_diff(x, y, n=10000, seed=1):
    """IC 95 % de mean(y) - mean(x) por bootstrap percentil."""
    if len(x) < 2 or len(y) < 2:
        return None
    r = random.Random(seed)
    ds = []
    for _ in range(n):
        bx = [x[r.randrange(len(x))] for _ in x]
        by = [y[r.randrange(len(y))] for _ in y]
        ds.append(st.mean(by) - st.mean(bx))
    ds.sort()
    return ds[int(0.025 * n)], ds[int(0.975 * n)]


def cell(rows, g):
    xs = [g(r["m"], r["a"]) for r in rows]
    return [x for x in xs if x is not None]


def metric_table(data, depth, spec, time_rows=None):
    head, g, fmt = spec
    lines = ["| %s | %s |" % ("variante", " | ".join(["n", "media [mín–máx]", "Δ vs base (IC 95 %)", "lectura"])), "|---|---|---|---|---|"]
    base = None
    for v in VARS:
        rows = [r for r in (time_rows or data)[v] if r["depth"] == depth]
        if time_rows is not None:
            rows = [r for r in rows if r["tv"]]
        xs = cell(rows, g)
        if v == "base":
            base = xs
        if not xs:
            lines.append("| %s | 0 | – | – | – |" % NAMES[v])
            continue
        if v == "base":
            lines.append("| %s | %d | %s | – | – |" % (NAMES[v], len(xs), fmt_stats(xs, fmt)))
            continue
        ci = boot_diff(base, xs) if base else None
        if ci is None:
            lines.append("| %s | %d | %s | – | – |" % (NAMES[v], len(xs), fmt_stats(xs, fmt)))
            continue
        d = st.mean(xs) - st.mean(base)
        sig = "ruido (IC incluye 0)" if ci[0] <= 0 <= ci[1] else ("↓ distinto de 0" if d < 0 else "↑ distinto de 0")
        lines.append("| %s | %d | %s | %s%s [%s, %s] | %s |" % (
            NAMES[v], len(xs), fmt_stats(xs, fmt), "+" if d >= 0 else "", fmt.format(d), fmt.format(ci[0]), fmt.format(ci[1]), sig))
    return "\n".join(lines)


def summary(data, alltime, depth):
    """Tabla compacta: una fila por métrica, una columna por variante: media [mín–máx]; «*» = IC 95 % de la diferencia vs base excluye 0."""
    out = ["| métrica | n (base/+A/+B/+A+B) | base | +A | +B | +A+B |", "|---|---|---|---|---|---|"]
    for spec in QUALITY + [TIME]:
        head, g, fmt = spec
        is_time = spec is TIME
        cols, ns, base = [], [], None
        for v in VARS:
            rows = [r for r in (alltime if is_time else data)[v] if r["depth"] == depth and (r["tv"] or not is_time)]
            xs = cell(rows, g)
            ns.append(str(len(xs)))
            if v == "base":
                base = xs
                cols.append(fmt_stats(xs, fmt))
                continue
            ci = boot_diff(base, xs) if xs and base else None
            star = "*" if ci and not (ci[0] <= 0 <= ci[1]) else ""
            cols.append(fmt_stats(xs, fmt) + star)
        out.append("| %s | %s | %s |" % (head, "/".join(ns), " | ".join(cols)))
    return "\n".join(out)


def main():
    roots = sys.argv[1:]
    if roots and roots[0] == "--summary":
        roots = roots[1:]
        data = {v: load(roots[0], v) for v in VARS}
        alltime = {v: [r for root in roots for r in (data[v] if root == roots[0] else load(root, v))] for v in VARS}
        for depth in ("quick", "standard"):
            print("### %s\n" % depth)
            print(summary(data, alltime, depth))
            print()
        return
    data = {v: load(roots[0], v) for v in VARS}
    alltime = {v: [r for root in roots for r in (data[v] if root == roots[0] else load(root, v))] for v in VARS}
    print("## Calidad (todas las ejecuciones)\n")
    print("Ejecuciones por variante y profundidad: " + ", ".join(
        "%s: %s" % (v, "/".join("%s=%d" % (d, sum(1 for r in data[v] if r["depth"] == d)) for d in ("quick", "standard"))) for v in VARS) + "\n")
    for depth in ("quick", "standard"):
        print("### %s\n" % depth)
        for spec in QUALITY:
            print("**%s**\n" % spec[0])
            print(metric_table(data, depth, spec))
            print()
    print("## Tiempos (solo ejecuciones con timingValid)\n")
    for depth in ("quick", "standard"):
        print("### %s\n" % depth)
        print(metric_table(data, depth, TIME, time_rows=alltime))
        print()
        print("Ejecuciones con tiempo válido por activo (variante: n por activo):\n")
        for v in VARS:
            c = {}
            for r in alltime[v]:
                if r["depth"] == depth and r["tv"]:
                    c[r["asset"]] = c.get(r["asset"], 0) + 1
            print("- %s: %s" % (NAMES[v], ", ".join("%s=%d" % (a, c.get(a, 0)) for a in ("stock_aapl", "stock_msft", "crypto_btc", "etf_spy"))))
        print()
    print("## Carga de CPU durante las ejecuciones (loadMean del registro, por variante)\n")
    print("| variante | n registros | carga media (1 min) [mín–máx] | reloj SM medio MHz | potencia media W |\n|---|---|---|---|---|")
    for v in VARS:
        es = [r["env"] for r in alltime[v] if r["env"] and "loadMean" in r["env"]]
        print("| %s | %d | %s | %s | %s |" % (NAMES[v], len(es), fmt_stats([e["loadMean"] for e in es], "{:.1f}"),
              "{:.0f}".format(st.mean([e["clockMeanMHz"] for e in es])) if es else "–",
              "{:.0f}".format(st.mean([e["powerMeanW"] for e in es])) if es else "–"))


if __name__ == "__main__":
    main()
