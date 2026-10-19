#!/usr/bin/env bash
# Comparación de modelos sobre el pipeline: cada modelo es una variante. Los modelos NO se intercalan ejecución a ejecución
# (cada cambio recarga el modelo y la carga contaría en el tiempo); se intercalan por BLOQUES: repetición -> modelo -> 4 activos x profundidades,
# rotando el orden de los modelos en cada repetición. Antes de cada bloque se precarga el modelo (no cuenta).
#   run_models.sh <binario> <dir-salida> <repeticiones> "<profundidades>" "<modelos>"
set -u
BIN=$1; OUT=$2; REPS=$3; DEPTHS=$4; read -ra MODELS <<< "$5"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
n=${#MODELS[@]}
for r in $(seq 1 "$REPS"); do
  for k in $(seq 0 $((n - 1))); do
    m=${MODELS[$(((k + r - 1) % n))]}; label=$(echo "$m" | tr ':' '-')
    curl -s localhost:11434/api/generate -d "{\"model\":\"$m\",\"prompt\":\"\",\"keep_alive\":\"30m\",\"options\":{\"num_ctx\":16384}}" > /dev/null
    echo "== rep $r modelo $m"
    MODEL=$m VARIANTS="$label:$BIN" "$HERE/run_eval.sh" "$OUT" "$r" "$DEPTHS" || exit $?
  done
done
