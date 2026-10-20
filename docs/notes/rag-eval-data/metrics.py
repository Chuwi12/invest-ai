import glob, json, re, statistics, sys, os
R = os.path.dirname(os.path.abspath(__file__))
def texts(a):
    t = [a.get("summary", ""), a.get("thesis", ""), a.get("strategy") or "", a.get("horizonNote") or ""]
    t += a.get("bullCase", []) + a.get("bearCase", []) + a.get("risks", []) + (a.get("wouldChangeIf") or [])
    t += [c.get("description", "") for c in a.get("catalysts", [])] + [s.get("description", "") for s in (a.get("scenarios") or [])]
    return "\n".join(t)
rows = []
for f in sorted(glob.glob(f"{R}/results/cmp_*.json")):
    r = json.load(open(f)); a = r.get("analysis")
    if not a: rows.append({"file": f, "error": r.get("error")}); continue
    txt = texts(a); notes = a.get("qualityNotes") or []
    st = dict((s, t) for t, s in r["stages"])
    dtags = re.findall(r"\[D\d+\]", txt)
    removed = sum(int(m) for n in notes for m in re.findall(r"Se quitaron (\d+) cita\(s\) del archivo", n))
    unver = sum(1 for n in notes if re.search(r"cifra|no figura|sin verificar|⚠", n, re.I) and "cita" not in n.lower())
    rows.append({"asset": r["asset"], "mode": r["mode"], "rep": r["rep"], "wall": r["wall"], "dur": a["durationMs"] / 1000, "prompt": a["promptTokens"], "compl": a["completionTokens"],
                 "calls": a["llmCalls"], "verdict": a["verdict"], "conf": a["confidence"], "d_valid": len(dtags), "d_unique": len(set(dtags)), "d_removed": removed,
                 "marks": txt.count("⚠"), "unver_notes": unver, "n_notes": len(notes), "n_cites": len(re.findall(r"\[N\d+\]", txt)),
                 "src": len(a["sources"]), "retr_s": st.get("technical", 0) - st.get("snapshot", 0), "thesis_len": len(a["thesis"]) + len(a["summary"]),
                 "rag_warn": any("archivo de noticias" in w for w in a.get("dataWarnings", []))})
json.dump(rows, open(f"{R}/results/metrics.json", "w"), indent=1)
ok = [x for x in rows if "mode" in x]
def agg(mode, k): return [x[k] for x in ok if x["mode"] == mode]
print(f"n_off={len(agg('off','wall'))} n_on={len(agg('on','wall'))}  errores={len(rows)-len(ok)}")
for k in ["wall", "prompt", "compl", "calls", "d_valid", "d_removed", "marks", "unver_notes", "n_notes", "n_cites", "src", "retr_s", "thesis_len", "conf"]:
    a, b = agg("off", k), agg("on", k)
    if a and b: print(f"{k:12} off {statistics.mean(a):9.2f} (mediana {statistics.median(a):9.2f})   on {statistics.mean(b):9.2f} (mediana {statistics.median(b):9.2f})")
print("veredicto distinto entre off/on (misma repetición):", sum(1 for x in ok if x["mode"] == "on" for y in ok if y["mode"] == "off" and y["asset"] == x["asset"] and y["rep"] == x["rep"] and y["verdict"] != x["verdict"]), "de", sum(1 for x in ok if x["mode"] == "on"))
print("on con D:", sum(1 for x in ok if x["mode"] == "on" and x["d_valid"] > 0), "de", len(agg("on", "wall")), "| rag_warn:", sum(x["rag_warn"] for x in ok))
