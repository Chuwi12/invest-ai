#!/bin/bash
# uso: srv.sh <nombre> <puerto> <rag 0|1> <ruta.db> [VAR=valor ...]  -> PID (por puerto) en <nombre>.pid, log en logs/<nombre>.log
R=$HOME/.cache/invest-ai/rag-eval
name=$1; port=$2; rag=$3; db=$4; shift 4
cd "$(dirname "$0")/../../../backend" || exit 1
nohup systemd-run --user --scope --quiet -p MemoryMax=4G -p MemorySwapMax=0 env \
  INVEST_AI_NO_BROWSER=1 INVEST_AI_OLLAMA_AUTOSTART=0 STATIC_DIR=/nonexistent WATCHLIST_ENABLED=0 \
  BIND_ADDR=127.0.0.1:$port DATABASE_URL=sqlite://$db RAG_ENABLED=$rag \
  LLM_BASE_URL=http://127.0.0.1:11500 RAG_EMBED_BASE_URL=http://127.0.0.1:11500 LLM_KV_CACHE_TYPE=q8_0 \
  RUST_LOG=info,server=debug,knowledge=debug "$@" \
  "$PWD/target/agent-rag/release/invest-ai" > $R/logs/$name.log 2>&1 &
for i in $(seq 1 20); do sleep 0.5; p=$(ss -ltnp "sport = :$port" 2>/dev/null | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2); [ -n "$p" ] && break; done
echo "$p" > $R/$name.pid; echo "$name pid $p"
