#!/usr/bin/env bash
# Prueba de forzar todas las capas en GPU (options.num_gpu=999) frente al reparto por defecto de Ollama.
#   numgpu_probe.sh <binario compare_models> <dir-salida> "<modelos>"
# Por modelo: bench por defecto y forzado (carga, /api/ps, nvidia-smi, 3 velocidades, petición de ~14k tokens con pico de VRAM)
# y un `standard` real de stock:AAPL x2 a través de gpu_proxy.py (que inyecta num_gpu) con la VRAM mínima libre muestreada.
# Al final descarga los modelos (keep_alive 0) y para el proxy (por PID). Requiere enchufe (run_eval.sh se detiene si no).
set -u
BIN=$1; OUT=$2; read -ra MODELS <<< "$3"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "$OUT"
unload() { local m; for m in "${MODELS[@]}"; do curl -s localhost:11434/api/generate -d "{\"model\":\"$m\",\"keep_alive\":0}" > /dev/null; done; sleep 3; }
for m in "${MODELS[@]}"; do
  l=$(echo "$m" | tr ':' '-')
  unload
  python3 -I "$HERE/bench_model.py" "$m" 16384 > "$OUT/bench_default_$l.json" 2> "$OUT/bench_default_$l.err"
  unload
  python3 -I "$HERE/bench_model.py" "$m" 16384 999 > "$OUT/bench_forced_$l.json" 2> "$OUT/bench_forced_$l.err"
  unload
done
python3 -I "$HERE/gpu_proxy.py" 8771 999 &
PROXY=$!
sleep 2
for m in "${MODELS[@]}"; do
  l=$(echo "$m" | tr ':' '-')
  unload
  curl -s localhost:8771/api/generate -d "{\"model\":\"$m\",\"prompt\":\"\",\"keep_alive\":\"30m\",\"options\":{\"num_ctx\":16384}}" > /dev/null
  ( while true; do nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits >> "$OUT/vram_$l.log"; sleep 1; done ) &
  SP=$!
  LLM_BASE_URL=http://127.0.0.1:8771 MODEL=$m VARIANTS="forced-$l:$BIN" "$HERE/run_eval.sh" "$OUT/standard" 2 "standard" "stock_aapl"
  rc=$?
  kill $SP 2>/dev/null; wait $SP 2>/dev/null
  [ $rc -ne 0 ] && { echo "run_eval terminó con $rc para $m" >&2; }
done
kill $PROXY 2>/dev/null
unload
