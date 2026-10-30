# Ablación: modo `quick` nuevo (A) y cifras derivadas en Rust (B)

Estado: **completa** (2026-10-16). Línea base: completa. Este documento mide; no decide ni cambia código.

Resumen en cinco líneas (detalle y cautelas más abajo):

1. **+A (quick rediseñado) reduce el tiempo de `quick` de ~44 s a ~16 s (−63 %) con el mismo `qwen2.5:7b`** y la salida cae de ~2000 a ~740 tokens.
   El `quick` antiguo (~44 s) tardaba lo mismo que `standard` (~44 s); el nuevo, 2,7× menos. No es gratis: la tesis pasa de ~197 a ~120
   palabras, el resumen de ~44 a ~15, 1 viñeta alcista y 1 bajista en vez de ~3, y 0 catalizadores en vez de 0,7.
2. **+A introduce dos regresiones de fidelidad medibles en `quick`**: más cifras con la etiqueta equivocada (1,08 por análisis frente a 0)
   y más ⚠ (sobre todo umbrales hipotéticos en «qué cambiaría»: 0,92 por análisis frente a 0,08). Las etiquetas erróneas se
   concentran en `crypto:BTC` (8 de 13): el ejemplo de estilo del juez de `quick` (PER, margen, flujo de caja libre) se copia en
   activos que no tienen esas magnitudes.
3. **+B (cifras derivadas) no produce ninguna mejora medible con `qwen2.5:7b`** en estas métricas, y cuesta ~+12–15 % de tokens de entrada
   (significativo en `standard`: +1439). No empeora nada de forma distinguible del ruido. Aviso importante: con este modelo la tasa base de cifras
   no verificadas ya es muy baja (≈0,2–0,3 por análisis), así que esta métrica **no puede mostrar mejora** (efecto suelo): el resultado
   es «sin efecto medible», no «B no sirve».
4. **+A+B** se comporta como +A (el efecto de A domina); no se ve interacción distinguible del ruido.
5. n = 12 por celda de profundidad y variante; los tiempos tienen n = 8–10. Hay ~20 métricas × 2 profundidades × 3 contrastes: con IC al 95 % se
   esperan ~5 % de «significativos» por azar (ver «Ruido»). Solo se consideran creíbles los efectos grandes y coherentes entre métricas
   (los de A en `quick`).

## Metodología

- **Datos congelados.** Cuatro snapshots reales (`GET /api/assets/{id}/snapshot`, servidor del commit `cdf6fc4`, puerto 8769, BD temporal, claves de
  `.env` cargadas), bajados el 2026-10-14 ~20:54 UTC y guardados en `backend/crates/ai/tests/fixtures/eval/`:
  `stock_aapl.json`, `stock_msft.json`, `crypto_btc.json`, `etf_spy.json`. Se comprobó con `grep -F` que ninguna clave del `.env` aparece en ellos.
  Todas las variantes analizan exactamente estos ficheros.
- **Modelo y configuración.** `qwen2.5:7b` (Ollama), `LLM_NUM_CTX=16384` (como el `.env` de la app), temperatura 0,2, timeout 180 s, idioma `es`,
  `think` apagado. GPU NVIDIA de portátil con 8 GB, 100 % en GPU. Una ejecución a la vez. Modelo precargado.
- **Variantes** (cada una compilada como `cargo build --release -p ai --example compare_models` en su `git worktree`, con el MISMO
  `compare_models.rs` de medición copiado dentro):

  | variante | contenido | diferencia frente a `cdf6fc4` | tests `ai` |
  |---|---|---|---|
  | base | `cdf6fc4` | – | 233 ok |
  | +A | `cdf6fc4` + `analysis/{mod.rs,prompts.rs,types.rs,tests.rs}` de `1a7f333`, **deshaciendo** la regla 2 del system prompt que remite a «Cifras derivadas» (queda `pub(crate) mod levels;`, inocuo) | 4 ficheros, +325/−64 | 237 ok |
  | +B | `1a7f333` con `analysis/{mod.rs,prompts.rs,types.rs,tests.rs}` devueltos a `cdf6fc4`, **conservando** la regla 2 y `pub(crate) mod levels;` (para que `derived.rs` compile) | 7 ficheros, +550/−7 (`context.rs`, `derived.rs`, `fidelity.rs`, `labels.rs`, `lib.rs` + las dos líneas) | 245 ok |
  | +A+B | `1a7f333` | commit completo | 249 ok |

  Comprobado en los binarios (`strings`): la cadena «Derived figures/Cifras derivadas» solo está en +B y +A+B; la etapa nueva «Fundamentales, noticias y
  riesgo» solo en +A y +A+B. No hubo que adaptar nada más para compilar. Los worktrees se rehicieron dos veces (el reinicio borra `/tmp`; la última vez en
  `~/.cache/invest-ai/eval/`) con exactamente los mismos parches: mismas estadísticas de `git diff --stat` y mismo nº de tests.
- **Matriz.** 4 activos × {quick, standard} × 3 repeticiones = 24 ejecuciones por variante (96 en total). Orden: repetición → activo → profundidad →
  variante, **rotando el orden de las variantes en cada celda** (reparte deriva térmica, carga de CPU de otros agentes y efectos de orden).
- **Reproducir.** `VARIANTS="base:… a:… b:… ab:…" backend/crates/ai/examples/run_eval.sh <raíz> 3 "quick standard"` y
  `backend/crates/ai/examples/ablation_report.py [--summary] <raíz-calidad> [<raíz-tiempos>…]`. Salidas JSON (análisis completo + métricas + eventos) y
  `_env.jsonl` en `docs/notes/eval-data/ablation/<variante>/` y `docs/notes/eval-data/ablation-timing/<variante>/`.
- **Estadística.** Por variante: media [mín–máx]. Frente a `base`: diferencia de medias con IC 95 % por bootstrap percentil (10 000 remuestreos, semilla
  fija, variantes independientes). `*` en las tablas = el IC excluye 0. Con n = 12 y datos discretos (0/1/2…) los IC son aproximados.
  No hay corrección por comparaciones múltiples (ver «Ruido»).

### Métricas (automáticas; `compare_models.rs::auto_metrics` y `ablation_report.py`)

| Métrica | Definición |
|---|---|
| llamadas, tokens in/out | Los del propio `Analysis` |
| cifras en tesis / distintas | Cifras no exentas (sin enteros pequeños, años ni fragmentos de fecha) en los párrafos de la tesis redactados por el modelo; «distintas» = valores únicos |
| verificadas por etiqueta | Cifras del texto final (todo lo que redacta el modelo) cuyo valor Y etiqueta están en el contexto que vio el modelo (`audit_figures`, el mismo índice del motor, que incluye la sección «Cifras derivadas» cuando existe) |
| solo por valor | El valor existe en el contexto pero la etiqueta no se pudo comprobar |
| no verificadas (⚠) | El valor no está en el contexto y sobrevivió marcado con ⚠. **Es la medida de «cifra derivada inventada que llega al usuario»**. Se separa el ⚠ de «qué cambiaría» (umbrales hipotéticos del tipo «si el PER sube a 25») del resto |
| etiqueta errónea | El valor existe pero asociado a otra magnitud |
| descartes / inventadas atrapadas | Afirmaciones y frases de escenario descartadas, y cifras ausentes del contexto citadas en afirmaciones descartadas (parseadas de `qualityNotes`) |
| tesis genérica detectada / reintentos / que queda | Eventos `tracing` («tesis genérica», «se pide reescribirla») y nota «poco específica» en `qualityNotes` |
| correcciones de redacción, lint final | Llamadas de corrección; problemas de castellano que el lint ve en el resultado final |
| veredicto = umbral | El veredicto coincide con el de los umbrales de la puntuación global (≥70 comprar, ≥60 acumular, ≥50 mantener, ≥42 esperar, ≥32 reducir). Aproximación: no incluye los ajustes por riesgo/cobertura de `scoring` (privados) |
| completitud | Palabras de tesis y resumen, nº de viñetas alcistas/bajistas, riesgos, «qué cambiaría» y catalizadores |

Qué NO miden: corrección financiera, utilidad de la tesis ni calidad de redacción. Una tesis más corta con menos cifras puede ser peor o mejor;
las métricas solo dicen que es distinta. «Verificada» significa «el número está en el contexto», no «el razonamiento es correcto».

## Desviaciones y confusores (léase antes de los números)

1. **Energía.** El 14-oct el portátil iba con batería (GPU limitada a ~30 W y ~400 MHz: tiempos ~2× los de `model-eval.md`). Esas ejecuciones del
   primer lote (`docs/notes/eval-data/baseline/`, 24 ejecuciones, 12 con batería y 12 con corriente) **solo se usan para calidad** y para estimar el ruido
   entre lotes. `run_eval.sh` registra desde entonces ADP1/online en cada ejecución (`_env.jsonl`).
2. **GPU compartida.** Del 15-oct 15:20 al 16-oct por la mañana otro proceso del usuario (PID 26943, torch, ~1 GB de VRAM) compartió la GPU.
   Las ejecuciones de ese periodo (12–13 por variante, sin campo `gpuForeign`) tienen **tiempos invalidados** retroactivamente con
   `docs/notes/eval-data/ablation/_timing_overrides.json` (`gpu_shared_beelzebub`); su calidad sí se usa. Las ejecuciones posteriores registran `gpuForeign`
   (procesos de GPU que no son `llama-server` ni el escritorio) y `timingValid` = enchufado durante toda la ejecución Y sin procesos ajenos.
3. **Cortes de sesión.** Tres cortes (OOM del terminal, cierre de sesión, reinicio del equipo que borró `/tmp`). `run_eval.sh` es reanudable
   (salta lo ya hecho); al reanudar se rehicieron worktrees y binarios (mismo código, ver arriba). El 16-oct 12:40 el portátil volvió a batería durante la
   reanudación: las ejecuciones restantes de calidad se hicieron con `ALLOW_BATTERY=1` (marcan `timingValid=false`, `invalidReason=battery`) y
   **no se usan para tiempos**. Después se enchufó y se hizo la pasada solo de tiempos.
4. **Ficheros de la noche del 14-oct.** Los `stock_aapl_quick_1.json` de cada variante eran de la noche del 14-oct; se movieron a
   `eval-data/ablation/_night/` y se rehicieron el 16-oct (misma matriz, mismo día). Queda **una** salida del 14-oct (23:29) en la matriz principal:
   `stock_aapl_standard_1` de +A (mismo código y mismo `compare_models.rs`, corriente y GPU libre; 35 s, tiempo válido); no se rehizo.
5. **Pasada de tiempos.** `eval-data/ablation-timing/`: 1 repetición por celda y variante (32 ejecuciones), intercalada, enchufado, GPU sin procesos ajenos:
   **las 32 con `timingValid=true`**. Los tiempos de las tablas = ejecuciones válidas de la matriz principal (las del 16-oct hasta las 09:32 y 1 del 14-oct por la noche) + esta
   pasada. Todas las celdas (activo × profundidad) tienen ≥ 1 tiempo válido por variante; n por celda entre 1 y 3 (detalle en la sección de tiempos).
6. **Carga de CPU de otros agentes** (compilaciones y tests simultáneos). Se registra `loadMean` por ejecución (ver «Tiempos»).
7. **Hallazgo de métricas durante el análisis**: el evento «tesis genérica» también lo emite el `quick` nuevo cuando decide NO reintentar, y su nota de calidad
   tiene otro texto; el informe separa «detectada», «reintento (llamada extra)» y «queda». Las tablas usan esa separación.
8. `compare_models.rs` (harness) se modificó para esta ablación (añade `auto_metrics`); no se tocó código de producción.

## Línea base (`cdf6fc4`)

Mismas métricas, `qwen2.5:7b`, `num_ctx = 16384`. Dos lotes independientes del mismo código, útiles para ver el ruido de run a run:

| | lote 1 (14-oct, `baseline/`, calidad) | lote 2 (16-oct, `ablation/base/`, calidad) |
|---|---|---|
| n quick / standard | 12 / 12 | 12 / 12 |
| llamadas quick / standard | 2,6 / 5,6 | 2,8 / 5,4 |
| tokens in quick / standard | 6766 / 10084 | 7795 / 9686 |
| tokens out quick / standard | 1610 / 2025 | 2001 / 1884 |
| cifras en tesis quick / standard | 15,6 / 17,2 | 13,8 / 13,7 |
| cifras distintas en tesis quick / standard | 11,5 / 11,4 | 9,5 / 11,3 |
| verificadas por etiqueta quick / standard | 29,1 / 32,5 | 29,2 / 29,5 |
| solo por valor quick / standard | 10,5 / 11,2 | 8,2 / 8,2 |
| no verificadas (⚠) quick / standard | 0,33 / 0,17 | 0,33 / 0,17 |
| correcciones de redacción quick / standard | 0,50 / 0,42 | 0,50 / 0,33 |
| veredicto = umbral | 24/24 | 24/24 |
| confianza quick / standard | 0,72 / 0,70 | 0,69 / 0,70 |

Tiempos de la línea base: lote 1 **con corriente** (6 ejecuciones por profundidad): quick 32 s (28–39), standard 42 s (34–64); **con batería** (6 + 6):
quick 80 s (65–130), standard 88 s (73–101). Lote 2 (tiempos válidos): quick 43,6 s (30–70, mediana 36), standard 43,9 s (32–61, mediana 41).
La referencia previa de `model-eval.md` (34/36 s) cae dentro de este rango. **El `quick` antiguo no es más rápido que `standard`**: mediana 36 s frente a 41 s,
medias 43,6 y 43,9 s; en el lote 1 con corriente, 32 frente a 42 s (n = 6).

## Resultados: calidad y coste (todas las ejecuciones; n = 12 por celda)

Formato: media [mín–máx]; `*` = IC 95 % de la diferencia con base excluye 0. La fila de segundos usa solo tiempos válidos (n = 10/10/10/10 en quick; 9/9/8/9 en standard).

### quick

| métrica | n (base/+A/+B/+A+B) | base | +A | +B | +A+B |
|---|---|---|---|---|---|
| llamadas LLM | 12/12/12/12 | 2.8 [2.0–4.0] | 3.0 [3.0–3.0] | 2.9 [2.0–4.0] | 3.2 [3.0–4.0] |
| tokens in | 12/12/12/12 | 7795 [5765–10463] | 6368 [5867–6888]* | 8711 [6385–12480] | 7212 [6510–8174] |
| tokens out | 12/12/12/12 | 2001 [1287–3322] | 740 [700–811]* | 1987 [1377–3553] | 758 [614–891]* |
| cifras en tesis | 12/12/12/12 | 13.8 [7.0–22.0] | 10.9 [8.0–13.0]* | 14.2 [9.0–20.0] | 9.2 [4.0–12.0]* |
| cifras distintas en tesis | 12/12/12/12 | 9.5 [7.0–14.0] | 9.2 [5.0–11.0] | 11.2 [6.0–17.0] | 7.7 [4.0–11.0]* |
| cifras (todo el texto) | 12/12/12/12 | 37.8 [23.0–52.0] | 18.5 [15.0–22.0]* | 40.8 [30.0–52.0] | 16.6 [11.0–21.0]* |
| verificadas por etiqueta | 12/12/12/12 | 29.2 [21.0–40.0] | 15.7 [12.0–21.0]* | 27.7 [19.0–36.0] | 13.7 [5.0–17.0]* |
| verificadas solo por valor | 12/12/12/12 | 8.2 [2.0–14.0] | 0.9 [0.0–3.0]* | 12.2 [3.0–29.0] | 1.2 [0.0–4.0]* |
| no verificadas (⚠) | 12/12/12/12 | 0.33 [0.00–3.00] | 0.83 [0.00–2.00] | 0.67 [0.00–3.00] | 0.75 [0.00–2.00] |
| ⚠ en resumen/tesis/viñetas/riesgos/catalizadores | 12/12/12/12 | 0.42 [0.00–3.00] | 1.00 [0.00–4.00] | 0.92 [0.00–3.00] | 0.67 [0.00–3.00] |
| ⚠ en «qué cambiaría» (umbrales hipotéticos) | 12/12/12/12 | 0.08 [0.00–1.00] | 0.92 [0.00–2.00]* | 0.67 [0.00–3.00]* | 1.00 [0.00–2.00]* |
| % verificadas por etiqueta | 12/12/12/12 | 78.2 [65.7–91.3] | 89.8 [75.0–100.0]* | 70.4 [44.2–92.3] | 86.2 [50.0–100.0] |
| % no verificadas | 12/12/12/12 | 0.71 [0.00–5.77] | 4.82 [0.00–12.50]* | 1.52 [0.00–6.98] | 4.93 [0.00–12.50]* |
| etiqueta errónea | 12/12/12/12 | 0.00 [0.00–0.00] | 1.08 [0.00–3.00]* | 0.25 [0.00–1.00] | 0.92 [0.00–2.00]* |
| descartes (afirm.+frases) | 12/12/12/12 | 0.08 [0.00–1.00] | 0.08 [0.00–1.00] | 0.33 [0.00–2.00] | 0.00 [0.00–0.00] |
| cifras inventadas atrapadas | 12/12/12/12 | 0.17 [0.00–2.00] | 0.08 [0.00–1.00] | 0.33 [0.00–2.00] | 0.00 [0.00–0.00] |
| tesis genérica detectada | 12/12/12/12 | 0.33 [0.00–1.00] | 0.08 [0.00–1.00] | 0.33 [0.00–1.00] | 0.08 [0.00–1.00] |
| reintentos por tesis genérica (llamada extra) | 12/12/12/12 | 0.33 [0.00–1.00] | 0.00 [0.00–0.00]* | 0.33 [0.00–1.00] | 0.00 [0.00–0.00]* |
| tesis genérica que queda (confianza −0,05) | 12/12/12/12 | 0.33 [0.00–1.00] | 0.08 [0.00–1.00] | 0.33 [0.00–1.00] | 0.08 [0.00–1.00] |
| correcciones de redacción | 12/12/12/12 | 0.50 [0.00–1.00] | 0.00 [0.00–0.00]* | 0.58 [0.00–1.00] | 0.17 [0.00–1.00] |
| errores lint finales | 12/12/12/12 | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| veredicto = umbral de puntuación | 12/12/12/12 | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] |
| confianza | 12/12/12/12 | 0.69 [0.58–0.81] | 0.73 [0.67–0.79] | 0.66 [0.54–0.79] | 0.73 [0.64–0.77] |
| palabras de la tesis | 12/12/12/12 | 197 [145–241] | 120 [80–145]* | 191 [139–248] | 117 [70–141]* |
| palabras del resumen | 12/12/12/12 | 44 [20–59] | 15 [9–23]* | 42 [28–61] | 14 [7–26]* |
| viñetas alcistas | 12/12/12/12 | 2.8 [2.0–3.0] | 1.0 [1.0–1.0]* | 2.8 [2.0–3.0] | 1.0 [1.0–1.0]* |
| viñetas bajistas | 12/12/12/12 | 3.1 [3.0–4.0] | 0.9 [0.0–1.0]* | 2.8 [2.0–3.0] | 1.0 [1.0–1.0]* |
| riesgos | 12/12/12/12 | 1.2 [0.0–3.0] | 2.9 [2.0–3.0]* | 1.1 [0.0–3.0] | 3.0 [3.0–3.0]* |
| «qué cambiaría» | 12/12/12/12 | 3.0 [3.0–3.0] | 2.0 [2.0–2.0]* | 3.0 [3.0–3.0] | 2.0 [2.0–2.0]* |
| catalizadores | 12/12/12/12 | 0.7 [0.0–2.0] | 0.0 [0.0–0.0]* | 1.1 [0.0–2.0] | 0.0 [0.0–0.0]* |
| segundos (solo tiempos válidos) | 10/10/10/10 | 43.6 [30.3–69.5] | 16.1 [14.8–17.0]* | 38.2 [29.8–54.3] | 17.2 [14.1–21.0]* |

### standard

| métrica | n (base/+A/+B/+A+B) | base | +A | +B | +A+B |
|---|---|---|---|---|---|
| llamadas LLM | 12/12/12/12 | 5.4 [5.0–6.0] | 5.5 [5.0–6.0] | 5.6 [5.0–7.0] | 5.5 [5.0–6.0] |
| tokens in | 12/12/12/12 | 9686 [8568–12591] | 10015 [8572–12671] | 11125 [9332–15365]* | 10710 [9590–13486]* |
| tokens out | 12/12/12/12 | 1884 [1580–2838] | 1967 [1480–2849] | 1999 [1506–3137] | 1857 [1637–2548] |
| cifras en tesis | 12/12/12/12 | 13.7 [7.0–23.0] | 15.7 [9.0–23.0] | 15.2 [9.0–20.0] | 16.2 [9.0–31.0] |
| cifras distintas en tesis | 12/12/12/12 | 11.3 [7.0–16.0] | 11.6 [6.0–18.0] | 11.2 [7.0–18.0] | 10.7 [6.0–18.0] |
| cifras (todo el texto) | 12/12/12/12 | 38.1 [29.0–53.0] | 41.8 [24.0–63.0] | 39.2 [24.0–45.0] | 41.5 [27.0–65.0] |
| verificadas por etiqueta | 12/12/12/12 | 29.5 [20.0–43.0] | 30.6 [20.0–39.0] | 28.2 [19.0–35.0] | 26.3 [14.0–33.0] |
| verificadas solo por valor | 12/12/12/12 | 8.2 [5.0–12.0] | 10.8 [4.0–28.0] | 10.0 [5.0–21.0] | 14.1 [6.0–34.0]* |
| no verificadas (⚠) | 12/12/12/12 | 0.17 [0.00–1.00] | 0.25 [0.00–2.00] | 0.33 [0.00–3.00] | 0.42 [0.00–1.00] |
| ⚠ en resumen/tesis/viñetas/riesgos/catalizadores | 12/12/12/12 | 0.25 [0.00–1.00] | 0.17 [0.00–1.00] | 0.83 [0.00–3.00]* | 0.50 [0.00–2.00] |
| ⚠ en «qué cambiaría» (umbrales hipotéticos) | 12/12/12/12 | 0.25 [0.00–1.00] | 0.17 [0.00–1.00] | 0.25 [0.00–1.00] | 0.50 [0.00–1.00] |
| % verificadas por etiqueta | 12/12/12/12 | 77.6 [66.7–86.5] | 75.1 [55.6–86.5] | 73.5 [51.2–85.4] | 66.5 [43.6–82.9]* |
| % no verificadas | 12/12/12/12 | 0.45 [0.00–2.86] | 0.69 [0.00–5.26] | 0.79 [0.00–6.67] | 1.12 [0.00–3.70] |
| etiqueta errónea | 12/12/12/12 | 0.17 [0.00–1.00] | 0.00 [0.00–0.00] | 0.50 [0.00–3.00] | 0.42 [0.00–2.00] |
| descartes (afirm.+frases) | 12/12/12/12 | 0.00 [0.00–0.00] | 0.08 [0.00–1.00] | 0.17 [0.00–1.00] | 0.08 [0.00–1.00] |
| cifras inventadas atrapadas | 12/12/12/12 | 0.00 [0.00–0.00] | 0.25 [0.00–3.00] | 0.33 [0.00–2.00] | 0.17 [0.00–2.00] |
| tesis genérica detectada | 12/12/12/12 | 0.08 [0.00–1.00] | 0.17 [0.00–1.00] | 0.17 [0.00–1.00] | 0.08 [0.00–1.00] |
| reintentos por tesis genérica (llamada extra) | 12/12/12/12 | 0.08 [0.00–1.00] | 0.17 [0.00–1.00] | 0.17 [0.00–1.00] | 0.08 [0.00–1.00] |
| tesis genérica que queda (confianza −0,05) | 12/12/12/12 | 0.00 [0.00–0.00] | 0.17 [0.00–1.00] | 0.08 [0.00–1.00] | 0.08 [0.00–1.00] |
| correcciones de redacción | 12/12/12/12 | 0.33 [0.00–1.00] | 0.33 [0.00–1.00] | 0.42 [0.00–1.00] | 0.42 [0.00–1.00] |
| errores lint finales | 12/12/12/12 | 0.08 [0.00–1.00] | 0.08 [0.00–1.00] | 0.00 [0.00–0.00] | 0.08 [0.00–1.00] |
| veredicto = umbral de puntuación | 12/12/12/12 | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] | 0.92 [0.00–1.00] |
| confianza | 12/12/12/12 | 0.70 [0.67–0.79] | 0.70 [0.62–0.81] | 0.69 [0.58–0.77] | 0.70 [0.58–0.81] |
| palabras de la tesis | 12/12/12/12 | 183 [131–258] | 178 [135–218] | 182 [147–218] | 179 [144–218] |
| palabras del resumen | 12/12/12/12 | 43 [32–59] | 42 [28–65] | 44 [31–57] | 39 [23–53] |
| viñetas alcistas | 12/12/12/12 | 3.0 [3.0–3.0] | 2.8 [2.0–3.0] | 3.0 [3.0–3.0] | 2.4 [1.0–3.0]* |
| viñetas bajistas | 12/12/12/12 | 3.1 [3.0–4.0] | 3.1 [3.0–4.0] | 3.0 [3.0–3.0] | 3.0 [3.0–3.0] |
| riesgos | 12/12/12/12 | 1.8 [0.0–3.0] | 1.8 [1.0–3.0] | 1.1 [0.0–3.0] | 0.7 [0.0–2.0]* |
| «qué cambiaría» | 12/12/12/12 | 3.0 [3.0–3.0] | 3.0 [3.0–3.0] | 3.0 [3.0–3.0] | 3.0 [3.0–3.0] |
| catalizadores | 12/12/12/12 | 0.2 [0.0–2.0] | 0.2 [0.0–1.0] | 0.5 [0.0–2.0] | 1.4 [0.0–2.0]* |
| segundos (solo tiempos válidos) | 9/9/8/9 | 43.9 [31.7–60.5] | 39.8 [35.1–57.4] | 50.5 [35.8–65.8] | 43.1 [33.9–67.5] |


Lectura por bloques (solo efectos grandes o coherentes entre métricas; el resto es ruido, ver más abajo):

**+A en `quick` (efecto grande y coherente).**
- Coste: −63 % de tiempo (16,1 s [14,8–17,0] frente a 43,6 s [30–70]; ningún solape de rangos), −63 % de tokens de salida, −18 % de entrada, 3,0 llamadas frente a 2,8 (pasa de
  «1 etapa fusionada + juez + reintentos» a «2 etapas cortas + juez breve sin reintento»: más llamadas, pero mucho más cortas).
- Contenido: tesis −39 % de palabras, resumen −66 %, viñetas 1+1 en vez de ~3+3, 0 catalizadores, «qué cambiaría» 2 en vez de 3, riesgos 2,9 en vez de 1,2 (el esquema nuevo pide 3 riesgos).
  Cifras citadas en toda la salida: 18,5 frente a 37,8; en la tesis 10,9 frente a 13,8, pero **las distintas en la tesis no cambian** (9,2 frente a 9,5).
- Fidelidad: el % de cifras verificadas por etiqueta sube (89,8 % frente a 78,2 %) porque casi desaparecen las «solo por valor» (0,9 frente a 8,2), pero
  **sube la tasa de no verificadas** (4,8 % frente a 0,7 % de las cifras; 0,83 ⚠ por análisis frente a 0,33, este último no significativo en bruto), **aparecen etiquetas erróneas** (1,08 por análisis frente a 0) y
  los umbrales hipotéticos con ⚠ en «qué cambiaría» pasan de 0,08 a 0,92 por análisis. Ejemplo (BTC, +A): «El PER 64.9 con margen operativo de 12.3 y un crecimiento de ingresos de
  13.2% deja la valoración en zona razonable. El flujo de caja libre de 81782.31 USDT…» (el 64,9 es el % del máximo histórico). Es la forma del ejemplo del prompt del juez de `quick`
  (PER, margen, crecimiento, FCF) aplicada a un activo sin esas magnitudes; PER/FCF/BPA aparecen en la tesis o el resumen de 6 de 6 análisis `quick` de BTC y SPY con +A (3 de 6 en base).
  Con n = 6 es un indicio, no una tasa; las 13 etiquetas erróneas están en 3 de los 4 activos (BTC 8, MSFT 2, AAPL 3, SPY 0).
- Pipeline de forma: sin reintentos por tesis genérica (0 frente a 0,33 por análisis) y menos correcciones de redacción (0 frente a 0,50). La tesis genérica que queda sin resolver no cambia de forma distinguible (0,08 frente a 0,33; n.s.).
- Sin cambio medible: veredicto = umbral (12/12 en todas), errores de lint finales (0), descartes, confianza (0,73 frente a 0,69, n.s.).

**+B (cifras derivadas) en `quick` y `standard` (sin efecto medible en calidad).**
- Ninguna métrica de fidelidad ni de riqueza cambia de forma distinguible del ruido, salvo en `standard`: +1439 tokens de entrada (+15 %)* y +0,6 ⚠ en prosa por análisis*
  (0,83 frente a 0,25; peor, no mejor). En `quick`: +916 tokens de entrada (n.s.), 0,67 frente a 0,33 no verificadas (n.s.), % por etiqueta 70,4 frente a 78,2 (n.s.).
- Tiempo: `quick` 38,2 s frente a 43,6 s (n.s.); `standard` 50,5 s [36–66] frente a 43,9 s [32–61] (n.s.: el IC incluye 0; con 8 y 9 ejecuciones).
- Efecto suelo: la tasa base de cifras no verificadas es ≈0,2–0,3 por análisis con este modelo, así que B no tiene margen para mejorar esa métrica. Lo que B pretendía
  (errores como «recompensa/riesgo de 6,7», potencial al objetivo mal calculado, observados en `qwen3:8b` en `model-eval.md`) no se manifestó en `qwen2.5:7b` en estos 24 análisis por
  variante; esta ablación **no puede decir** si B ayuda con otro modelo o si las cifras derivadas se citan más (no se midió cuántas tesis citan cifras de la sección «Cifras derivadas»).

**+A+B.** En `quick` es +A (17,2 s frente a 16,1 s; tokens de salida 758 frente a 740; mismos descensos de contenido). B suma +844 tokens de entrada frente a +A (7212 frente a 6368; n.s.) y 1,1 s (n.s.).
En `standard`, +A+B muestra varias diferencias con IC que excluye 0: más tokens de entrada (+1024*), menos cifras «verificadas por etiqueta» (66,5 % frente a 77,6 %*), más «solo por valor» (14,1 frente a 8,2*),
menos riesgos (0,7 frente a 1,8*) y más catalizadores (1,4 frente a 0,2*). Ni +A ni +B por separado muestran esas diferencias de forma significativa en `standard`; el patrón común (más cifras verificadas solo por
valor) apunta a que, con la sección derivada, el modelo cita números que están en el contexto pero con palabras distintas de su etiqueta; no se ha comprobado a mano. Con ~20 métricas y 3 contrastes,
varias estrellas aisladas pueden ser azar; estas seis juntas, en la misma variante, son menos probables como azar puro pero **no hay un segundo lote que las replique**.

## Tiempos (solo ejecuciones con `timingValid`)

| profundidad | base | +A | +B | +A+B |
|---|---|---|---|---|
| quick, n | 10 | 10 | 10 | 10 |
| quick, media [mín–máx] (s) | 43,6 [30–70] | **16,1 [15–17]** | 38,2 [30–54] | **17,2 [14–21]** |
| quick, mediana (s) | 36,3 | 16,3 | 34,8 | 16,8 |
| standard, n | 9 | 9 | 8 | 9 |
| standard, media [mín–máx] (s) | 43,9 [32–61] | 39,8 [35–57] | 50,5 [36–66] | 43,1 [34–67] |
| standard, mediana (s) | 40,7 | 36,5 | 51,7 | 37,6 |

Por activo (media en s, n = 1–3 por celda; solo orientativo):

| activo | quick base / +A / +B / +A+B | standard base / +A / +B / +A+B |
|---|---|---|
| stock:AAPL | 43 / 16 / 39 / 19 | 39 / 36 / 38 / 52 |
| stock:MSFT | 34 / 16 / 43 / 17 | 40 / 40 / 49 / 39 |
| crypto:BTC | 47 / 16 / 33 / 16 | 51 / 36* / 61* / 35 |
| etf:SPY | 48 / 16 / 37 / 17 | 45 / 45 / 56 / 45 |

(* n = 1.) En `quick`, +A y +A+B quedan por debajo de cualquier ejecución de base/+B en todos los activos (rangos disjuntos: máx. 21 s frente a mín. 30 s); es la única diferencia de tiempo que
supera con holgura el ruido observado. En `standard` ningún IC de diferencia excluye 0: base 43,9 s frente a +A 39,8 s, +B 50,5 s, +A+B 43,1 s son **ruido** (el camino de `standard` de +A es prácticamente el de base:
mismas etapas y juez; su diferencia de −4 s da una idea del ruido: ±6–10 s). La evaluación anterior vio 41 s frente a 97 s en el mismo caso; aquí el máximo observado en `standard` fue 67 s y en `quick`-base 70 s.

**Carga de CPU** (`loadMean`, carga de 1 min media durante cada ejecución; otros agentes compilaban y ejecutaban tests):

| variante | carga media, ejecuciones con tiempo válido (n) | carga media, todas las ejecuciones (n) |
|---|---|---|
| base | 7,5 (19) | 10,1 (32) |
| +A | 7,0 (19) | 9,2 (32) |
| +B | 6,6 (18) | 9,9 (32) |
| +A+B | 7,6 (19) | 9,9 (32) |

La carga está repartida de forma parecida entre variantes (gracias al intercalado); las diferencias de 1 punto entre variantes no explican diferencias de 27 s. Mediana de las ejecuciones con tiempo válido: 6,7 (máx. 14,2).
Sensibilidad: con el umbral «2 × mediana» (13,4) hay solo dos ejecuciones con tiempo válido por encima (una de +A y otra de +B, ambas `etf:SPY` standard, 57 s y 66 s). Excluyéndolas,
`standard` queda en +A 37,6 s (n = 8) y +B 48,3 s (n = 7); `quick` no cambia (ninguna). Las conclusiones no cambian. La GPU funcionó a ~2150–2200 MHz y ~85 W de media en todas las variantes.

## Ruido: cómo leer los `*`

- **Mismo código, dos lotes.** Comparando el lote 1 y el lote 2 de la línea base (24 frente a 24 análisis idénticos en código) con el mismo método, 2 de 22 contrastes tuvieron IC que excluye 0
  («cifras distintas en tesis», `quick`: 11,5 frente a 9,5; «solo por valor», `standard`: 11,2 frente a 8,2), y los tokens de entrada de `quick` rozaron el límite (7795 frente a 6766; IC −75…+2085). Es decir: con este n, **diferencias de ~2 cifras en la tesis, ~3 verificadas «solo por valor» o ~1000 tokens de entrada pueden ser ruido**.
- En cada profundidad hay ~30 métricas × 3 contrastes: una de cada ~20 será «significativa» aunque no haya efecto. Los efectos de A en `quick` (tiempo, tokens de salida, longitud, nº de viñetas) son
  de orden de magnitud mucho mayor que cualquier discrepancia entre lotes y coherentes entre sí: no son ruido. Los de +B aislado y casi todos los de `standard` sí caben en el ruido.
- Temperatura 0,2 y sin semilla fija: dos ejecuciones idénticas no producen el mismo texto. Un único snapshot por activo (un solo instante de mercado) y 4 activos: los resultados no generalizan a otros activos, otros días ni otros modelos.

## Conclusiones (neutras)

- **A (quick rediseñado):** logra su objetivo de velocidad con `qwen2.5:7b` (≈16 s frente a ≈44 s; `standard` ≈40–44 s). A cambio entrega mucho menos texto (resumen ~15 palabras, 1 viñeta alcista y 1 bajista,
  sin catalizadores) y, en esta medición, más cifras mal etiquetadas y más umbrales hipotéticos con ⚠. Si el objetivo de `quick` es una lectura rápida, la reducción de contenido es coherente con el diseño; los dos puntos de fidelidad
  (ejemplo de estilo con PER/FCF aplicado a BTC/SPY; ⚠ en «qué cambiaría») son medibles y atribuibles a A (no aparecen en +B), y son candidatos a revisar. No se midió si una tesis más corta es mejor o peor para quien decide.
- **B (cifras derivadas):** sin efecto medible con este modelo y estas métricas; añade ~+12–15 % de tokens de entrada y ~+6 s en `standard` (n.s.). No se puede concluir que sea inútil (efecto suelo, un solo modelo, sin medir si el modelo cita las cifras derivadas),
  ni que merezca su coste. Su utilidad esperada (corregir errores de cálculo como los de `qwen3:8b`) quedaría por medir con ese modelo.
- **Juntos:** el tiempo y la forma de `quick` los fija A. En `standard`, +A+B muestra más cifras verificadas «solo por valor» y menos «por etiqueta» que base (efecto no replicado por A ni B solos; posible interacción o azar): **no concluyente**.
- **Lo que esta ablación no demuestra:** que alguna variante dé mejores decisiones de inversión; la corrección financiera no se midió (solo fidelidad a los datos del contexto y forma).

## Limitaciones

- n = 12 análisis por celda de profundidad y variante (3 por activo); n = 8–10 en tiempos. Sin corrección por comparaciones múltiples. IC bootstrap con datos discretos y muestra pequeña: aproximados.
- Un modelo (`qwen2.5:7b`), `num_ctx = 16384`, temperatura 0,2, un snapshot por activo, 4 activos. `deep` no se midió (no cambia en A ni B).
- Métricas basadas en regex y en el texto en español de `qualityNotes`; «veredicto = umbral» es una aproximación; los ⚠ de «qué cambiaría» son umbrales hipotéticos que pueden ser legítimos.
- Tiempos con carga de CPU de otros agentes (declarada arriba); 1 ejecución de la matriz es del 14-oct por la noche; los tiempos del 15-oct (GPU compartida) y los hechos con batería no se usan.
- El lint de castellano y la verificación son los de cada variante en el momento de la ejecución para lo que genera el pipeline, pero la auditoría de cifras (`audit_figures`) usa el contexto de la variante medida: cada una se evalúa con su propio contexto.
- No se leyó a mano cada salida; los extractos citados son ilustrativos.

## Arreglo de las regresiones de A

Fecha: 2026-10-16. Cambio de código (sin commitear): `backend/crates/ai/src/analysis/prompts.rs` (+312/−16) y una línea en `mod.rs`.
**Esta medición es con la caché KV en `q8_0`** (ver «Caché KV» más abajo); no es comparable en condiciones con la ablación anterior (f16).

### Qué se cambió

1. **Ejemplo de estilo del juez de `quick` construido con los datos del activo** (`quick_hints` / `quick_example`). Antes el ejemplo llevaba PER, margen operativo, crecimiento de
   ingresos y flujo de caja libre para todos los activos. Ahora se sacan de los snapshots las magnitudes que el activo SÍ tiene (por clase de activo, en orden de relevancia; 2–3 en el
   ejemplo de valoración y 2 en el técnico) y se añade al prompt: «Magnitudes this asset HAS» y «This asset has NO PER, BPA, margen operativo… never mention them». El párrafo (1) deja de
   decir «valuation and fundamentals» cuando el activo no tiene fundamentales de empresa (cripto: «cómo se sitúa en su rango y su perfil de riesgo»). Cripto sin PER/BPA/FCF/margen;
   ETF solo con lo que traiga (SPY sí trae PER y rentabilidad por dividendo). `standard` y `deep` no tienen ese patrón (su juez no lleva ejemplo con magnitudes fijas; solo la guía de
   vocabulario en español, que no se tocó) y no cambian: **no se midieron**.
2. **Umbrales de «qué cambiaría» anclados a niveles de los datos.** El juez de `quick` recibe una lista «Reference levels» (hasta 8) con valores copiables: SMA200, SMA50, soporte y
   resistencia más cercanos, máximo/mínimo de 52 semanas, máximo histórico (cripto), PER actual, objetivo medio de analistas, y los umbrales RSI 70/30 que el verificador ya conoce. Se le pide que cada
   condición se apoye en UNA de esas magnitudes, la nombre y copie su valor. **El verificador no se tocó.**
3. Tests (`prompts.rs::tests`, 5 nuevos): el ejemplo de cripto no contiene PER/margen/flujo de caja/BPA/crecimiento de ingresos; el de la acción empieza por «El PER de <valor>»; el del ETF (snapshot real
   de SPY) solo nombra magnitudes que el snapshot tiene; los umbrales salen de niveles reales (SMA200 con su valor, soporte) y caen al texto genérico si no hay niveles; el constructor en inglés. `cargo test --workspace`
   y `cargo clippy --workspace --all-targets -- -D warnings` (con `systemd-run`, 8 GB): **limpios** (`ai`: 261 tests ok, 1 ignorado).

### Medición

- HEAD `5dfd095` (+A+B) frente a HEAD + arreglo; solo `quick`; los 4 snapshots congelados; **5 repeticiones por activo (n = 20 por variante; se pidieron 3 y se ampliaron a 5 antes de ver
  resultados porque cada ejecución dura ~17 s)**; variantes intercaladas con orden rotatorio; `qwen2.5:7b`, `num_ctx` 16384, enchufado, GPU sin procesos ajenos, Ollama con KV `q8_0`.
  Las 40 ejecuciones tienen `timingValid=true`. Los worktrees están en `~/.cache/invest-ai/eval/{head,fix}`; salidas en `docs/notes/eval-data/fix-quick/`.
- Hubo un corte (apagado del equipo, 13:41) en mitad de la matriz; se reanudó (el script salta lo hecho). Una ejecución con batería (`etf_spy_quick_3` de `fix`) se descartó y se rehízo
  (queda en `fix-quick/_battery/`); su línea antigua de `_env.jsonl` queda sustituida por la de la repetición.
- Media [mín–máx] por variante; `*` = IC 95 % bootstrap de la diferencia con HEAD excluye 0 (como en el resto del documento).

| métrica | n (head/fix) | HEAD 5dfd095 (+A+B) | HEAD + arreglo |
|---|---|---|---|
| llamadas LLM | 20/20 | 3.2 [3.0–4.0] | 3.1 [3.0–4.0] |
| tokens in | 20/20 | 7230 [6506–8242] | 7503 [6795–8547] |
| tokens out | 20/20 | 749 [662–902] | 813 [680–1072]* |
| cifras en tesis | 20/20 | 10.2 [6.0–15.0] | 10.4 [6.0–15.0] |
| cifras distintas en tesis | 20/20 | 8.4 [4.0–14.0] | 8.2 [5.0–11.0] |
| cifras (todo el texto) | 20/20 | 17.5 [11.0–22.0] | 19.5 [13.0–25.0]* |
| verificadas por etiqueta | 20/20 | 14.2 [8.0–20.0] | 17.8 [11.0–24.0]* |
| verificadas solo por valor | 20/20 | 1.8 [0.0–8.0] | 1.5 [0.0–4.0] |
| no verificadas (⚠) | 20/20 | 0.45 [0.00–1.00] | 0.05 [0.00–1.00]* |
| ⚠ en resumen/tesis/viñetas/riesgos/catalizadores | 20/20 | 0.60 [0.00–3.00] | 0.00 [0.00–0.00]* |
| ⚠ en «qué cambiaría» (umbrales hipotéticos) | 20/20 | 1.00 [0.00–2.00] | 0.20 [0.00–1.00]* |
| % verificadas por etiqueta | 20/20 | 86.9 [50.0–100.0] | 92.1 [78.9–100.0] |
| % no verificadas | 20/20 | 2.69 [0.00–6.67] | 0.23 [0.00–4.55]* |
| etiqueta errónea | 20/20 | 1.15 [0.00–4.00] | 0.15 [0.00–1.00]* |
| descartes (afirm.+frases) | 20/20 | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| cifras inventadas atrapadas | 20/20 | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| tesis genérica detectada | 20/20 | 0.15 [0.00–1.00] | 0.05 [0.00–1.00] |
| reintentos por tesis genérica (llamada extra) | 20/20 | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| tesis genérica que queda (confianza −0,05) | 20/20 | 0.15 [0.00–1.00] | 0.05 [0.00–1.00] |
| correcciones de redacción | 20/20 | 0.20 [0.00–1.00] | 0.15 [0.00–1.00] |
| errores lint finales | 20/20 | 0.20 [0.00–3.00] | 0.10 [0.00–2.00] |
| veredicto = umbral de puntuación | 20/20 | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] |
| confianza | 20/20 | 0.74 [0.64–0.80] | 0.77 [0.72–0.81]* |
| palabras de la tesis | 20/20 | 120 [91–171] | 128 [90–179] |
| palabras del resumen | 20/20 | 14 [6–27] | 22 [14–30]* |
| viñetas alcistas | 20/20 | 1.0 [1.0–1.0] | 1.0 [1.0–1.0] |
| viñetas bajistas | 20/20 | 1.0 [1.0–1.0] | 1.0 [1.0–1.0] |
| riesgos | 20/20 | 2.9 [2.0–3.0] | 3.0 [2.0–3.0] |
| «qué cambiaría» | 20/20 | 2.0 [2.0–2.0] | 2.0 [2.0–2.0] |
| catalizadores | 20/20 | 0.0 [0.0–0.0] | 0.0 [0.0–0.0] |
| segundos (solo tiempos válidos) | 20/20 | 16.9 [14.5–20.9] | 18.4 [15.6–23.5]* |


### Lectura

- **Etiquetas erróneas:** 1,15 → 0,15 por análisis* (23 → 3 en 20 análisis). En `crypto:BTC` 10 → 2 en 5 análisis; las que quedan son umbrales de «caída máxima supera el 50 %» y «P/S a 12,0».
  En BTC, PER/FCF/BPA/margen en la tesis o el resumen: **5 de 5 → 0 de 5**. (En SPY el PER es un dato real y se sigue citando, por eso «5 de 10» en BTC+SPY.)
- **⚠ en prosa** (resumen, tesis, viñetas, riesgos): 0,60 → 0,00* por análisis (BTC y SPY 12 → 0). **⚠ en «qué cambiaría»:** 1,00 → 0,20* (20 → 4 en 20 análisis). **No desaparecen**: quedan umbrales
  redondos inventados («la caída máxima supera el 50 %», «el PER sube a 25», «el P/S se eleva a 12,0»): 4 de 20 análisis. Proporción de cifras no verificadas: 2,7 % → 0,2 %*.
- **Cifras verificadas por etiqueta:** 14,2 → 17,8*; porcentaje 86,9 % → 92,1 % (n.s.). Cifras totales 17,5 → 19,5*.
- **Contenido (que no se pierda):** tesis 120 → 128 palabras (n.s.); resumen 14 → 22 palabras*; viñetas 1+1 y «qué cambiaría» 2, igual; riesgos 2,9 → 3,0; catalizadores 0 y 0. Confianza 0,74 → 0,77*.
  No se pierde contenido; el resumen se alarga porque ahora cita cifras reales (el prompt con las magnitudes disponibles le da de qué hablar).
- **Tiempo:** 16,9 s [14,5–20,9] → 18,4 s [15,6–23,5] (+1,5 s, IC excluye 0). Es la consecuencia esperada de un prompt más largo (+~270 tokens de entrada, n.s. en bruto: 7230 → 7503) y de ~64 tokens más de salida (749 → 813*).
  Sigue en torno a 17–18 s (frente a ~44 s del `quick` antiguo en f16, y ~40 s de `standard`).
- Sin cambio: llamadas (3,2 → 3,1), descartes (0), reintentos por tesis genérica (0), veredicto = umbral (20/20), errores de lint (0,20 → 0,10; n.s.).

**Qué se puede afirmar y qué no.** Con n = 20 por variante, el descenso de etiquetas erróneas (23 → 3), de ⚠ en prosa (12 → 0) y el cambio de cripto (5/5 → 0/5) son grandes y coherentes entre métricas: el ejemplo de estilo era la causa de la contaminación de
BTC/SPY y el arreglo la elimina con este modelo. El descenso de ⚠ en «qué cambiaría» es real pero parcial (20 → 4). Los demás cambios (resumen +8 palabras, confianza +0,03, +1,5 s) son pequeños y deben leerse como
tendencia. Es un único modelo, cuatro activos y un solo snapshot por activo; no se midió utilidad para decidir ni corrección financiera, solo fidelidad a los datos. En AAPL queda un caso («P/S 12,0») y en MSFT otro
(«PER sube a 25») de umbral inventado: el modelo sigue sin copiar siempre la lista de niveles.

### Caché KV (`q8_0`)

`run_eval.sh` registra ahora por ejecución `kvCache` (argumentos `--cache-type-k/-v` del `llama-server` vivo + `OLLAMA_KV_CACHE_TYPE` y hora de arranque del servicio `ollama`) y `kvCacheAfter`/`kvChanged`.
En las 40 ejecuciones: `runner:k=q8_0,v=q8_0;servicio:OLLAMA_KV_CACHE_TYPE=q8_0;arranque:2026-10-16 13:28:59`, `kvChanged=false` en todas (el servicio no se reinició a mitad; el apagado del equipo no cambió el ajuste).
Toda la ablación A/B anterior se hizo con **f16** (`llama-server` sin `--cache-type-*`, comprobado a las 12:47; `OLLAMA_KV_CACHE_TYPE` no estaba definida entonces; esa ablación no registró el tipo por ejecución, así que
es una comprobación puntual, no por ejecución).

**Comparación indicativa (mismo código, +A+B; f16 de la ablación frente a q8_0 de HEAD en esta medición; `quick`; n = 12 frente a 20 en calidad y 10 frente a 20 en tiempo; otro momento del día y otra carga de CPU: carga media 7,6 frente a 4,7):**

| métrica | f16 (ablación) | q8_0 (esta medición) | Δ q8 − f16 [IC 95 %] |
|---|---|---|---|
| tiempo (s) | 17,2 [14,1–21,0] | 16,9 [14,5–20,9] | −0,4 [−1,9, +1,0] |
| tokens de entrada | 7212 | 7230 | +18 [−456, +488] |
| tokens de salida | 758 | 749 | −9 [−60, +42] |
| cifras en tesis | 9,2 | 10,2 | +1,0 [−0,5, +2,7] |
| verificadas por etiqueta | 13,7 | 14,2 | +0,5 [−1,7, +2,8] |
| no verificadas (⚠) | 0,75 | 0,45 | −0,30 [−0,8, +0,2] |
| etiqueta errónea | 0,92 | 1,15 | +0,23 [−0,4, +0,9] |
| ⚠ en «qué cambiaría» | 1,00 | 1,00 | 0,00 [−0,5, +0,5] |
| palabras de tesis / resumen | 117 / 14 | 120 / 14 | n.s. |
| confianza | 0,73 | 0,74 | +0,01 [−0,03, +0,04] |

Primera lectura: **sin diferencia distinguible del ruido** en tiempo ni en calidad entre f16 y q8_0 en `quick` con este modelo (todos los IC incluyen 0). No demuestra equivalencia: con estos n solo se descartarían
diferencias grandes (p. ej. ±2 s de tiempo, ±2 cifras). El beneficio medido por el coordinador es de memoria (5110 MiB frente a 5384 MiB de VRAM). Queda por medir `standard`/`deep` y contextos más largos.

### Propuesta (no implementada): distinguir «umbral hipotético» de «cifra inventada»

Hoy el ⚠ de «qué cambiaría» mezcla dos cosas: (a) un umbral que el modelo propone («si el PER sube a 25») y (b) una cifra que presenta como dato. Para quien lee, (a) es una condición legítima aunque 25 no esté en los datos;
llamarla «Cifras no verificables en los datos» sugiere un fallo. Propuesta: en el campo `would_change_if` (solo ahí), si el número aparece tras un verbo de umbral («sube a», «supera», «cae por debajo de», «cierra por debajo de») y **la magnitud nombrada sí existe en los datos** (PER, caída máxima, RSI…), mostrar una nota distinta
(«umbral propuesto por el modelo, no es un dato») en vez del ⚠ de cifra no verificable, sin dejar de marcarlo y sin contarlo como cifra inventada en la confianza ni en las métricas. Razones para hacerlo así y no relajar el verificador: (1) el verificador sigue marcando
cualquier cifra suelta; (2) restringirlo a un campo y a un patrón sintáctico evita que un dato falso pase disfrazado de umbral en la tesis; (3) permite medir por separado «umbrales no anclados» de «cifras inventadas», que hoy compiten en el mismo contador (en +A+B `quick`, 1,00 de los 1,67 ⚠ por análisis
eran de este tipo). Riesgo: un umbral absurdo (como «la caída máxima supera el 50 %») dejaría de parecer un error aunque no tenga base; mitigación: mostrarlo con un estilo distinto, no ocultarlo. No se implementó porque cambia el contrato de las notas de calidad y de la UI.

### Limitaciones de esta sección

n = 20 por variante, un modelo, 4 activos, un snapshot por activo, solo `quick`; muchas métricas (~30) sin corrección por comparaciones múltiples (varias estrellas aisladas, como el resumen +8 palabras o la confianza +0,03, pueden ser azar); la comparación con f16 no es
de la misma sesión (ver arriba). El IC de los tiempos compara 20 frente a 20 ejecuciones intercaladas con carga de CPU media 4,7 frente a 4,6.
