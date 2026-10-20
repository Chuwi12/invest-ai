import json, sys, urllib.request
port=int(sys.argv[1]); msg=sys.argv[2]
def post(path, body):
    req=urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=json.dumps(body).encode(), headers={"Content-Type":"application/json"}, method="POST")
    return urllib.request.urlopen(req, timeout=300)
c=json.load(post("/api/conversations", {"title":"rag test","assetIds":[]}))
ev=[]; text=""
with post(f"/api/conversations/{c['id']}/messages", {"content":msg,"assetIds":[]}) as r:
    for raw in r:
        l=raw.decode().strip()
        if l.startswith("data:"):
            e=json.loads(l[5:]); ev.append(e)
            if e.get("type")=="token": text+=e["text"]
print("EVENTOS:", [ (e["type"], e.get("name"), e.get("summary") or e.get("ok")) for e in ev if e["type"] in("tool_call","tool_result")])
print("RESPUESTA:", text)
