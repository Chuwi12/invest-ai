#!/usr/bin/env bash
# Matriz de evaluación sobre snapshots congelados (tests/fixtures/eval/*.json), con varias variantes INTERCALADAS.
#   VARIANTS="base:/ruta/cm-base ab:/ruta/cm-ab" run_eval.sh <dir-salida-raiz> [repeticiones=3] ["quick standard"] ["stock_aapl stock_msft crypto_btc etf_spy"]
# Salidas: <raiz>/<variante>/<activo>_<prof>_<rep>.json. Orden: repetición -> activo -> profundidad -> variante, rotando el
# orden de las variantes en cada celda (reparte deriva térmica, carga de CPU y efectos de orden entre variantes).
# Una ejecución a la vez (una sola GPU). Reanudable: salta las salidas que ya existen.
#
# Registra en <raiz>/<variante>/_env.jsonl, por ejecución: estado de energía (ADP1/online antes/mínimo/después), reloj SM y
# potencia medios de la GPU, carga de CPU y `gpuForeign` = procesos con la GPU abiertos que NO son ni el servidor de Ollama
# (llama-server) ni el escritorio (kwin_wayland, Xorg, Xwayland, plasmashell), muestreados cada 3 s durante la ejecución.
# `timingValid` = enchufado durante toda la ejecución Y sin procesos ajenos en la GPU; si es false, `invalidReason` dice por qué
# (`battery`, `gpu_shared`). Las métricas de calidad valen igualmente; los TIEMPOS solo con timingValid.
# - ALLOW_BATTERY=1: pasada de CALIDAD con batería; no se detiene, solo marca timingValid=false (invalidReason=battery).
# - Si el portátil pasa a batería durante una ejecución, la salida se guarda como *.battery.json y el script SE DETIENE (código 3).
# - Si hay procesos ajenos en la GPU, la matriz continúa (calidad) marcando los tiempos como no válidos; con
#   WAIT_FREE_GPU_SECS=N espera hasta N segundos a que la GPU esté libre antes de cada ejecución (para las pasadas de tiempos).
set -u
OUT=$1; REPS=${2:-3}; DEPTHS=${3:-"quick standard"}
ASSETS=${4:-"stock_aapl stock_msft crypto_btc etf_spy"}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
FIX=$HERE/../tests/fixtures/eval
MODEL=${MODEL:-qwen2.5:7b}
AC=/sys/class/power_supply/ADP1/online
WAIT=${WAIT_FREE_GPU_SECS:-0}
read -ra VARS <<< "${VARIANTS:?define VARIANTS=\"etiqueta:binario ...\"}"
export LLM_NUM_CTX=${LLM_NUM_CTX:-16384} LLM_TEMPERATURE=${LLM_TEMPERATURE:-0.2} LLM_TIMEOUT_SECS=${LLM_TIMEOUT_SECS:-180} LLM_LANGUAGE=es

foreign() { nvidia-smi --query-compute-apps=pid,process_name --format=csv,noheader 2>/dev/null | grep -v -E 'llama-server|kwin_wayland|Xorg|Xwayland|plasmashell' | tr -d ' "' | tr ',' ':' | paste -sd';' -; }

cell=0
for r in $(seq 1 "$REPS"); do
  for a in $ASSETS; do
    for d in $DEPTHS; do
      n=${#VARS[@]}
      for k in $(seq 0 $((n - 1))); do
        v=${VARS[$(((cell + k) % n))]}; label=${v%%:*}; bin=${v#*:}
        mkdir -p "$OUT/$label"
        name=${a}_${d}_$r; f=$OUT/$label/$name.json
        [ -s "$f" ] && continue
        if [ "$(cat $AC)" != 1 ] && [ "${ALLOW_BATTERY:-0}" != 1 ]; then echo "SIN CORRIENTE antes de $label/$name: me detengo" >&2; exit 3; fi
        waited=0
        while [ "$WAIT" -gt 0 ] && [ -n "$(foreign)" ] && [ "$waited" -lt "$WAIT" ]; do sleep 10; waited=$((waited + 10)); done
        samples=$(mktemp)
        ( while true; do fg=$(foreign); echo "$(cat $AC) $(nvidia-smi --query-gpu=clocks.sm,power.draw --format=csv,noheader,nounits | tr -d ',') $(cut -d' ' -f1 /proc/loadavg) ${fg:--}" >> "$samples"; sleep 3; done ) &
        sp=$!
        t0=$(date -Is)
        echo -n "$label "; "$bin" "$FIX/$a.json" "$MODEL" "$d" "$f" 2>&1 | tail -1
        kill $sp 2>/dev/null; wait $sp 2>/dev/null
        t1=$(date -Is); acafter=$(cat $AC)
        stats=$(awk '{if(NR==1||$1<m)m=$1; c+=$2; p+=$3; l+=$4; n++; if($5!="-"){split($5,xs,";"); for(i in xs) seen[xs[i]]=1}} END {fg=""; for(k in seen) fg=fg (fg==""?"":"|") k; if(n) printf "\"acMin\":%d,\"clockMeanMHz\":%.0f,\"powerMeanW\":%.1f,\"loadMean\":%.1f,\"samples\":%d,\"gpuForeign\":\"%s\"", m, c/n, p/n, l/n, n, fg; else printf "\"acMin\":-1,\"samples\":0,\"gpuForeign\":\"\""}' "$samples")
        rm -f "$samples"
        reason=""
        case "$stats" in *'"acMin":0'*) reason="battery";; esac
        [ "$acafter" != 1 ] && reason="battery"
        if [ -z "$reason" ]; then case "$stats" in *'"gpuForeign":""'*) ;; *) reason="gpu_shared";; esac; fi
        ok=true; [ -n "$reason" ] && ok=false
        echo "{\"file\":\"$name\",\"start\":\"$t0\",\"end\":\"$t1\",\"acAfter\":$acafter,$stats,\"timingValid\":$ok,\"invalidReason\":\"$reason\"}" >> "$OUT/$label/_env.jsonl"
        if [ "$reason" = battery ] && [ "${ALLOW_BATTERY:-0}" != 1 ]; then
          mv "$f" "$OUT/$label/$name.battery.json" 2>/dev/null
          echo "PASO A BATERIA durante $label/$name: salida guardada como $name.battery.json; me detengo" >&2
          exit 3
        fi
      done
      cell=$((cell + 1))
    done
  done
done
