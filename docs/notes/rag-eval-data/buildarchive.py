import json, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from client import get
ASSETS = ["stock:AAPL", "stock:NVDA", "stock:TSLA", "stock:JPM", "stock:KO", "stock:XOM", "stock:SAN.MC", "etf:SPY", "etf:QQQ", "crypto:BTC"]
for a in ASSETS:
    s, snap, t = get(8776, f"/api/assets/{a}/snapshot", 120)
    n_snap = len((snap or {}).get("news", [])) if s == 200 else None
    s2, news, t2 = get(8776, f"/api/assets/{a}/news?limit=30", 120)
    print(a, "snapshot", s, f"{t:.1f}s news_in_snapshot={n_snap}", "| news", s2, len(news) if isinstance(news, list) else news, f"{t2:.1f}s", flush=True)
s, news, t = get(8776, "/api/news?limit=30", 120); print("market news", s, len(news) if isinstance(news, list) else news)
for sym in ["TRXB", "IAM"]:
    s, p, t = get(8776, f"/api/ipo/{sym}/prospectus?maxChars=60000", 180)
    print(sym, s, (p or {}).get("issuer"), (p or {}).get("form"), len((p or {}).get("sections", [])), f"{t:.1f}s")
