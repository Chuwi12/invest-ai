import json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from client import analyze
R = os.path.dirname(os.path.abspath(__file__))
ASSETS = ["stock:AAPL", "stock:NVDA", "stock:TSLA", "stock:JPM", "stock:KO", "stock:XOM", "stock:SAN.MC", "etf:SPY", "etf:QQQ", "crypto:BTC", "stock:TRXB", "stock:IAM"]
# ABBA por activo: sin RAG (A, :8775) / con RAG (B, :8776), intercalados para repartir la deriva (datos que cambian, GPU caliente).
ORDER = [("off", 8775, 1), ("on", 8776, 1), ("on", 8776, 2), ("off", 8775, 2)]
for asset in ASSETS:
    for mode, port, rep in ORDER:
        label = f"cmp_{asset.replace(':','_')}_{mode}{rep}"
        fn = f"{R}/results/{label}.json"
        if os.path.exists(fn): continue
        r = analyze(port, asset, "standard")
        r["mode"], r["rep"] = mode, rep
        json.dump(r, open(fn, "w"), ensure_ascii=False, indent=1)
        a = r.get("analysis") or {}
        print(time.strftime("%H:%M:%S"), label, "wall %.0fs" % r["wall"], a.get("verdict"), a.get("promptTokens"), r.get("error"), flush=True)
print("FIN", flush=True)
