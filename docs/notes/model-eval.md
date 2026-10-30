# Evaluación de modelos: `qwen2.5:7b` frente a `qwen3:8b`

Fecha: 2026-10-14. Hardware: GPU NVIDIA de portátil (8188 MiB de VRAM), Ollama local con `-np 1`. Este documento compara; **no
elige**. La decisión es del usuario (el modelo activo se cambia en Ajustes o con `PUT /api/models/active`).

## Metodología

- **Datos congelados.** Los snapshots de `stock:AAPL` y `crypto:BTC` se bajaron una vez de `GET /api/assets/{id}/snapshot`
  (servidor real compilado con `CARGO_TARGET_DIR=…/target/agent-eval`, `BIND_ADDR=127.0.0.1:8774`, BD temporal) y se
  guardaron en JSON (2026-10-14 07:40 UTC). Los dos modelos analizan exactamente el mismo `AssetSnapshot`.
- **Ejemplo reproducible** `backend/crates/ai/examples/compare_models.rs` (nuevo). Lee el snapshot, fija el modelo, ejecuta
  `OllamaAnalyst::analyze_with` (el motor de producción) y guarda `Analysis` + métricas (reparaciones de JSON, reintentos
  de tesis genérica y correcciones de redacción capturadas de los eventos `tracing`, y el lint de castellano sobre todos
  los textos del resultado):
  `cargo run --release -p ai --example compare_models -- snapshot.json qwen3:8b deep salida.json`
  (`compare_models relint salida.json…` recalcula el lint de resultados guardados).
- **`num_ctx`.** La app real usa `LLM_NUM_CTX=16384` (`.env` del usuario); `AiConfig::default()` y el ejemplo sin `.env`
  usan 8192. Hubo dos matrices completas:
  - **Matriz principal, `num_ctx = 16384`** (lo que obtiene el usuario en la app). Etiquetas `q25c` / `q3c`.
  - **Matriz secundaria, `num_ctx = 8192`** (valor por defecto del código). `q25` / `q3`. Se hizo antes de descubrir lo del
    `.env`; sirve para ver el efecto del contexto sobre `qwen3:8b`.
- **Matriz.** 2 modelos × {quick, standard, deep} × 2 activos, y `deep` ×3 por modelo y activo (la primera + 2
  repeticiones). 20 ejecuciones por matriz, de una en una (una sola GPU), temperatura 0,2, sin otros procesos propios en la GPU.
- **`think` en qwen3.** El código lo gestiona así (`analysis/mod.rs::think_param/base_params`): si el modelo declara la
  capacidad «thinking» (qwen3 sí), envía `think: false` salvo que `LLM_THINK=1`; con `think: true` multiplica `num_predict`
  por 4. Toda la matriz va con `think: false` (valor por defecto de la app). Aparte se midió `LLM_THINK=1` (ver más abajo).
- **Métricas por ejecución.** Duración, llamadas LLM, tokens de entrada/salida (los del propio `Analysis`), `ollama ps`
  (PROCESSOR y tamaño, muestreado cada 3 s) y `nvidia-smi` (mediana de MiB usados durante la ejecución), veredicto y
  confianza, «cifras» (nº de tokens numéricos distintos de la tesis: regex tosca), «desc.» (frases/afirmaciones
  descartadas, sumadas de `qualityNotes`), ⚠ (cifras sin verificar marcadas en el texto), reparaciones de JSON,
  reintentos de tesis genérica, llamadas de corrección de redacción y problemas del lint de castellano **que quedan**
  tras la corrección del pipeline.
- **Truncado de prompt.** Del journal de Ollama (`slot release … n_tokens, truncated`) se tomó, por ejecución, el mayor
  `n_tokens` de un slot (prompt + generado, ventana de tiempo de la ejecución ±2 s) y si hubo `truncated = 1`.
  Resultado: **ninguna llamada truncada** en ninguna ejecución; el pico fue 6554 (con think) y ≤ 6075 (sin think, en las matrices), frente
  a 8192/16384 de contexto. Una sola llamada nunca superó ~6,6k tokens.
- **Calidad cualitativa.** Leí las tesis, riesgos, «qué cambiaría» y escenarios y comprobé contra el snapshot las cifras
  derivadas (no solo las citadas). Puntuación 1–5 por mí (un LLM; no es una evaluación ciega ni humana: ver Limitaciones).

## Ajuste en VRAM: lo que más importa con 8 GB

Sondeo directo con `/api/generate` (prompt mínimo) y `ollama ps` / `nvidia-smi`:

| `num_ctx` | `qwen3:8b` (Ollama) | `qwen3:8b` VRAM usada | `qwen2.5:7b` (Ollama) |
|---|---|---|---|
| 8192 | 6,2 GB, 100 % GPU | 6117 MiB | 5,1 GB, 100 % GPU |
| 10240 | 6,5 GB, 100 % GPU | 6407 MiB | – |
| 12288 | 6,8 GB, 100 % GPU | 6697 MiB | – |
| 14336 | 7,5 GB, **8 %/92 % CPU/GPU** | 6833 MiB | – |
| **16384 (la de la app)** | 7,8 GB, **12 %/88 % CPU/GPU** | 6745–6761 MiB | 5,6 GB, 100 % GPU (5585 MiB) |
| 32768 | – | – | 6,4 GB, 100 % GPU (6327 MiB) |

- **`qwen3:8b` no cabe entero en GPU con la configuración de la app (16384)**: Ollama reparte 12 % de las capas en CPU.
  Cabe entero hasta 12288. `qwen2.5:7b` cabe entero incluso a 32768.
- Causa (de `/api/show`): qwen3:8b tiene 36 bloques y 8 cabezas KV; qwen2.5:7b, 28 bloques y 4 cabezas KV. El KV-cache en
  f16 por token es 2·36·8·128·2 B = 144 KiB frente a 2·28·4·128·2 B = 56 KiB: a 16384 tokens, 2,25 GiB frente a 0,88 GiB.
  Ese es el motivo del reparto, no los pesos (5,2 GB frente a 4,7 GB).
- Coste en tiempo del reparto: `qwen3:8b` tarda 2,1–2,5× lo que `qwen2.5:7b` a 16384, frente a 1,3–1,7× a 8192 (tabla de abajo).
- El prompt máximo observado en una llamada (~6,6k tokens incluida la generación) cabe en 8192 con margen; bajar
  `LLM_NUM_CTX` es una decisión del usuario (no se ha cambiado nada). No se probó `OLLAMA_FLASH_ATTENTION` ni cuantizar el KV-cache.

## Resultados (medias; detalle por ejecución en el apéndice)

### `num_ctx = 16384` (configuración de la app)

| Profundidad | Modelo | Tiempo | Llamadas | Tokens in | Tokens out | GPU / VRAM |
|---|---|---|---|---|---|---|
| quick (n=2) | qwen2.5:7b | 34 s | 2,5 | 6439 | 1438 | 100 % GPU, 5585 MiB |
| quick (n=2) | qwen3:8b | 72 s | 3,0 | 6842 | 1998 | 12 %/88 % CPU/GPU, 6761 MiB |
| standard (n=2) | qwen2.5:7b | 36 s | 5,0 | 9150 | 1711 | idem |
| standard (n=2) | qwen3:8b | 90 s | 6,0 | 9931 | 2584 | idem |
| deep (n=6) | qwen2.5:7b | 47 s (45–49) | 7,3 | 13347 | 2235 | idem |
| deep (n=6) | qwen3:8b | 118 s (107–139) | 8,0 | 14923 | 3323 | idem |

Tokens de salida por segundo (globales, incluyen el prefill): deep 48 (qwen2.5) frente a 28 (qwen3).

### `num_ctx = 8192`

| Profundidad | Modelo | Tiempo | Llamadas | Tokens in | Tokens out | GPU / VRAM |
|---|---|---|---|---|---|---|
| quick (n=2) | qwen2.5:7b | 35 s | 3,0 | 6756 | 1516 | 100 % GPU, 5121 MiB |
| quick (n=2) | qwen3:8b | 58 s | 2,5 | 6728 | 2232 | 100 % GPU, 6131 MiB |
| standard (n=2) | qwen2.5:7b | 50 s | 5,5 | 11195 | 2328 | idem |
| standard (n=2) | qwen3:8b | 67 s | 6,0 | 10006 | 2748 | idem |
| deep (n=6) | qwen2.5:7b | 53 s (45–76) | 7,7 | 14290 | 2518 | idem |
| deep (n=6) | qwen3:8b | 84 s (80–88) | 8,0 | 14972 | 3434 | idem |

La entrada es casi igual en ambos (mismo prompt; difiere el tokenizador solo marginalmente y los reintentos). `qwen3:8b`
genera entre un 18 % y un 51 % más de tokens de salida por análisis (frases más largas y repetidas).

### Modo `think` de qwen3 (`LLM_THINK=1`), `stock:AAPL`, una ejecución por celda

| `num_ctx` | quick | deep |
|---|---|---|
| 8192 | 81 s, 3341 tok out (frente a 57 s sin think) | **281 s**, 11939 tok out, 9 llamadas (frente a 83 s, 3368) |
| 16384 | 104 s, 3010 out (frente a 69 s) | **314 s**, 9013 out (frente a 121 s) |

Con think, deep cuesta 2,6–3,4× más tiempo y 2,7–3,5× más tokens. Con n = 1 no se aprecia una mejora clara de calidad
(cifras citadas 25–35 frente a 19–31 sin think: sin diferencia clara; mismos tipos de fallo). **Inviable como valor por defecto** en 8 GB: más de 5 min
por análisis deep. En una de las 9 llamadas del deep a 8192 el modelo devolvió contenido vacío (`respuesta vacía`) y el
reintento de reparación lo resolvió (la única reparación de JSON de todo el experimento).

### Fiabilidad de forma (matriz `num_ctx = 16384`; entre paréntesis, la de 8192)

| Indicador | qwen2.5:7b | qwen3:8b |
|---|---|---|
| Veredicto = veredicto de la puntuación cuantitativa | 20/20 (20/20) | 20/20 (20/20) |
| Reparaciones de JSON | 0 (0) | 0 (0) |
| Reintentos por tesis genérica | 0 (2) | 0 (0) |
| Ejecuciones que necesitaron la llamada de corrección de redacción | 3/10 (5/10) | **10/10** (9/10) |
| Errores de castellano que el lint aún ve al final | 0 (1 falso positivo: «market» de «FDV / market cap») | 4: «invalidation» y «condition» en la estrategia, y 2× «market» (falso positivo) (5) |
| Frases/afirmaciones descartadas por deep (media) | 2,0 (2,3) | **6,0** (5,7) |
| Afirmaciones descartadas por citar noticias inexistentes `[N11]`… (20 ejecuciones) | 0 (1) | **8 (6)** |
| ⚠ por ejecución (media) | 0,6 | 0,3 |
| Confianza deep (rango en las 3 repeticiones) | AAPL 0,69–0,79; BTC 0,68–0,74 | AAPL 0,49–0,69; BTC 0,60–0,74 |
| Probabilidades de escenarios en las 3 repeticiones deep | BTC idénticas (30/40/30 ×3); AAPL cambian | varían entre repeticiones (en AAPL, 2 de 3 iguales) |

Notas:
- El veredicto del modelo coincidió con el de la puntuación en las 40 ejecuciones de las matrices (AAPL con 62/100 →
  `accumulate`; BTC con 59/100 → `hold`). **El veredicto no discrimina entre modelos**: la diferencia está en el texto,
  la confianza y los escenarios.
- Coherencia: en AAPL ambos modelos dan `accumulate` con rentabilidad esperada de los escenarios **negativa** (−0,1 a −0,9 %)
  y recompensa/riesgo 0,6. Es un resultado del pipeline (niveles y escenarios salen de Rust), no de un modelo.
- Los escenarios y niveles son iguales entre modelos (los calcula Rust); solo cambian las probabilidades y las
  descripciones de escenario que sobreviven al filtro de fidelidad (qwen3 pierde más: 2 frases por escenario con frecuencia).
- Las matrices miden el pipeline **antes** de los arreglos de este informe (citas inexistentes) y, la de 16384, ya con el de
  fechas de catalizadores. Ver «Bugs encontrados».

## Calidad cualitativa (1–5)

Puntuación sobre las tesis leídas (quick/standard/deep de ambos activos; extractos abajo). Una línea por puntuación.

| Criterio | qwen2.5:7b | qwen3:8b |
|---|---|---|
| Especificidad | **3** | **4** |
| Corrección factual frente al snapshot | **3** | **3** |
| Razonamiento | **2** | **3** |
| Castellano | **4** | **3** |

- Especificidad. qwen2.5: cita 17–29 cifras, pero reutiliza las mismas 5–6 (RSI, MACD, caída máxima, P/E) en tesis,
  riesgos y «qué cambiaría», y en 16384 repite cada cifra entre paréntesis («ROE alto (ROE = 148,8%)»). qwen3: más cifras
  derivadas y distintas (distancia a SMA50/SMA200, ATR/precio, soporte −0,7 %, interés corto, consenso, VIX), aunque repite
  hasta 4 veces la distancia al máximo histórico en BTC.
- Factual. Todas las derivadas que comprobé son correctas en ambos (341,27/291,42 = +17,1 %; /323,79 = +5,4 %; SMA50/SMA200
  +11,1 %; ATR 6,74/341,27 = 2,0 %; objetivo 329,38 = −3,5 %; BTC 83432/126080 = 66,2 %; 83432/72252 = +15,5 %). Errores de
  qwen2.5: «potencial de ganancia del 3,5 %» con el objetivo medio *por debajo* del precio (descartado en la tesis, pero
  quedó en el resumen de una ejecución con ⚠), «bono a 10 años a 6,0 %» inventado (⚠), «oferta circulante 20,1M sugiere
  escasez» sin base. Errores de qwen3: «recompensa/riesgo de 6,7» cuando el sistema calcula 0,3 (el verificador lo dio por
  bueno porque 6,7 aparece como rentabilidad a 1 mes), «Fondo de Valoración (FDV)» (es Fully Diluted Valuation) y «FDV/cap =
  1,00x sugiere dilución futura» (es lo contrario), «ATR13», fechas inventadas de catalizadores (25-oct, 28-oct) y
  «resultados el 2026-11-05» en prosa (el dato es 2026-11-02), y 14 citas `[N11]…` a noticias que no existen (ver bugs).
  Empate: errores distintos, ninguno ausente.
- Razonamiento. qwen2.5 contradice sus propios datos: mete «RSI 60,5 en zona neutra, sin sobrecompra» en el caso *bajista*,
  deduce «RSI 52,9 neutro → mercado sobrecomprado», «caída máxima −13,8 % sugiere potencial de recuperación», y «se fortalecerá si el RSI14 sube por encima de 70, indicando sobrecompra» como
  refuerzo alcista. qwen3 sí interpreta bien que el objetivo de analistas por
  debajo del precio implica caída y que recompensa/riesgo 0,6 es desfavorable, pero trata la distancia al máximo histórico
  como «corrección esperada del 33,8 %» (y la pone en el caso alcista) y deduce «falta de impulso alcista» de un RSI de 60.
- Castellano. qwen2.5: fluido pero con «un valoración», «los datos son actual», «fuerte alcance», y un «Volume 12.88K» (descartado).
  qwen3: más verboso, frases que repiten la anterior, anglicismos («FDV / market cap», «invalidation condition»
  en la estrategia que la corrección no arregló) y «ATR13». Necesitó la llamada de corrección en 10/10.

## Extractos lado a lado (mismos apartados; `num_ctx = 16384`, `deep`, repetición 2)

**stock:AAPL.** Datos: precio 341,27; SMA200 291,42; SMA50 323,79; RSI14 60,5; MACD hist. −0,03; objetivo medio 329,38; resultados 2026-11-02.

| Apartado | qwen2.5:7b | qwen3:8b |
|---|---|---|
| Veredicto / confianza | acumular / 0,74 | acumular / 0,49 |
| Tesis, párrafo de valoración | «El P/E forward alto (P/E forward = 35.58) y el P/S elevado (P/S = 10.67) indican una valoración alta (P/E forward = 35.58, P/S = 10.67). Sin embargo, el ROE alto (ROE = 148.8%) y el margen operativo alto (32.6%) son indicadores de rentabilidad sólida (…).» | «La valoración es exigente, con un P/E forward de 35.58, lo que sugiere una sobrevaloración en comparación con el sector Tecnología. El crecimiento de beneficios fue del +28.7% y el de ingresos del +16.4% […]. El margen neto es del 27.6% y el margen operativo del 32.6% […].» |
| Tesis, párrafo técnico | «La tendencia alcista fuerte (precio +17.1% respecto a la SMA200) y la SMA50 sobre la SMA200 (SMA50 sobre la SMA200 (+11.1%)) son fuertes indicadores alcistas (…). Sin embargo, el RSI14 en zona neutra […] y el MACD bajista […] son señales de precaución.» | «[…] +17.1% por encima de la SMA200 (291.42) y +5.4% por encima de la SMA50 (323.79). El RSI14 (60.5) se encuentra en zona neutra, pero el MACD muestra una señal bajista con un histograma de -0.03. La volatilidad anualizada del 21.1% y la recompensa/riesgo de 0.6 sugieren una entrada moderada en la zona de 336.28 – 341.27.» |
| Caso bajista | «El MACD histograma es -0.03 […] tendencia bajista»; «El RSI14 está en 60.5, en zona neutra, sin señales de sobrecompra»; «P/E forward 35.58 […]» | «P/E forward (35.58) está por encima del nivel exigente del sector Tecnología […]»; «El precio objetivo medio es de $329.38, lo que implica un potencial de caída del -3.5%»; MACD |
| Riesgos | 3 (caída máxima «sugiere potencial para una recuperación», P/E, MACD) | 1 (volatilidad); otras 4 afirmaciones descartadas por citar `[N11]`–`[N14]` |
| Qué cambiaría | RSI > 70; cierre bajo el stop 329,54; MACD hist. positivo | RSI > 70; cierre bajo el stop 329,54; «Los resultados trimestrales se publican el 2026-11-05» (dato: 2026-11-02) |
| Escenario alcista (texto del modelo conservado) | «La tendencia alcista se fortalecerá si el RSI14 sube por encima de 70, indicando sobrecompra» (incoherente) | ninguno (2 frases descartadas por cifras ajenas) |
| Notas de calidad | 1 frase descartada | 4 descartes por `[N11-14]`, 6 frases de escenarios, 2 ⚠ (70) |

**crypto:BTC.** Datos: precio 83432,45; ATH 126080; SMA200 72251,92; RSI14 52,9; R/R del sistema 0,3.

| Apartado | qwen2.5:7b | qwen3:8b |
|---|---|---|
| Veredicto / confianza | mantener / 0,72 | mantener / 0,60 |
| Tesis, apertura | «La oferta circulante 20.1M, total 20.1M, sugiere escasez de oferta. El precio al 66.2% de su máximo histórico, indica posible corrección.» | «El precio actual del Bitcoin está al 66.2% de su máximo histórico (126080), lo que sugiere una posible corrección del 33.8%. El Fondo de Valoración (FDV) es de 1.68T, lo que equivale a un múltiplo de 1.00x […].» |
| Tesis, técnico | «El RSI14 en 52.9, en zona neutra, no indica sobrecompra. […] SMA20 83935.78 y SMA50 81228.12 […] sugiere una valoración positiva.» | «El precio está 2.7% por encima de la SMA50 y 15.5% por encima de la SMA200 […]. La volatilidad anualizada […] y la media móvil de 20 sesiones […] reflejan una posible corrección del 33.8%» (non sequitur) |
| Caso bajista | MACD −448,8; caída máxima −48,8 %; «RSI14 52.9 […] no indica sobrecompra» (no es un argumento bajista) | «cerca del soporte de 82850 (-0.7%) y la resistencia de 87307.83 (+4.6%), con una recompensa/riesgo de 6.7» (el sistema: 0,3); MACD; volatilidad |
| Qué cambiaría | RSI > 70; precio < 82850; «caída máxima supera −50 %⚠» | RSI > 70; cierre bajo el stop 80772,31; bono 10 años «sube más del 0,02 pp» |

(En ambos, los escenarios y precios son los de Rust: «Precio = banda de Bollinger superior 86.321,26», «punto medio entre el
soporte 82.850 y la resistencia 83.935,78», «soporte 82.113,96».)

**Quick, AAPL (8192).** qwen3 puso tres catalizadores con fechas inventadas (2026-10-25, 11-01, 11-15; «Resultados trimestrales»
frente al dato 2026-11-02); qwen2.5 devolvió `catalysts: []` en 9 de 10 ejecuciones (cero catalizadores es una carencia, no un error).

## Conclusiones (sin decidir por el usuario)

- **Para velocidad y bajo coste en 8 GB con la configuración actual (`LLM_NUM_CTX=16384`): `qwen2.5:7b`.** 100 % en GPU con
  5,6 GB, caben hasta 32768 de contexto; quick/standard/deep en 34/36/47 s.
- **Para más cifras derivadas y para que la lectura del objetivo de analistas/recompensa-riesgo sea consistente: `qwen3:8b`**,
  asumiendo 2,1–2,5× más tiempo (72/90/118 s) a 16384, con 12 % del modelo en CPU. Con `num_ctx ≤ 12288` cabe entero en GPU
  (6,2–6,8 GB) y el sobrecoste baja a 1,3–1,7× (58/67/84 s a 8192); el prompt máximo observado (~6,6k tokens) cabe, pero
  cambiar `LLM_NUM_CTX` lo decide el usuario y afecta también al chat con herramientas (no medido).
- **Para consistencia de forma (menos descartes, menos correcciones de redacción, menos mezcla de inglés): `qwen2.5:7b`.**
  qwen3 descartó el triple de frases en deep, necesitó corrección de redacción en el 100 % de las ejecuciones y metió
  citas `[N11]…` inexistentes (ya mitigado: ver bugs).
- **Para estabilidad entre repeticiones:** qwen2.5 varía menos en confianza y escenarios (en BTC, escenarios idénticos
  ×3, que también indica poca sensibilidad); qwen3 varía más (confianza 0,49–0,69 en AAPL).
- **Modo razonamiento de qwen3 (`LLM_THINK=1`):** no recomendable en este hardware (deep 281–314 s); sin mejora de calidad
  visible en las dos ejecuciones que se hicieron.
- **Ninguno resuelve** las debilidades del pipeline: veredicto siempre igual a la puntuación, `accumulate` con rentabilidad
  esperada negativa, escenarios descartados por cifras ajenas, cifras con ⚠, bullets repetidos.

## Bugs encontrados

1. **Citas a noticias inexistentes (qwen3)** — `verify.rs::Verifier::claim`. qwen3 añade `[N11]`, `[N12]`… (por encima de
   las ≤10 noticias) a hechos que salen de los datos. El verificador descartaba la afirmación entera: 14 afirmaciones
   correctas perdidas en 20 ejecuciones (6 + 8) frente a 1 en qwen2.5. **Arreglado**: si, quitada la cita inexistente, el
   texto lleva cifras y todas se verifican, se conserva sin la cita y se suma a una nota agregada en `qualityNotes` («Se
   quitaron N cita(s) a noticias inexistentes; las cifras de esas frases sí están en los datos»); sin cifras (la cita era
   la única base) o con cifras inventadas se descarta como antes. Test: `a_nonexistent_news_tag_is_stripped_when_the_figures_check_out`.
   Comprobado en vivo: una ejecución deep de AAPL con qwen3 produjo «Se quitaron 3 cita(s)…». *Las matrices son previas al arreglo.*
2. **Fechas de catalizadores inventadas** — `analysis/mod.rs::assemble`. qwen3 rellena `catalysts[].date` con fechas que no
   figuran en los datos (25-oct, 28-oct, la de hoy…; el dato real es 2026-11-02). qwen2.5 casi no emite catalizadores, así que
   el fallo solo afloraba con qwen3. **Arreglado**: la fecha se conserva solo si está entre las de los datos (`dates_in_data`:
   resultados, ATH, vencimiento, cualquier fecha ISO de `fundamentals.extra` como `exDividendDate`, fecha de noticias,
   marcas de cotización/indicadores/macro y del snapshot); si no, se quita y se avisa en `qualityNotes`. Test:
   `catalyst_dates_not_in_the_data_are_dropped` (incluye ex-dividendo). Comprobado en vivo (fecha quitada + aviso). *No cubre
   fechas escritas dentro de la prosa* («se publican el 2026-11-05»): el verificador exime las fechas y no se tocó.
3. **Falso positivo del verificador (sin arreglar)**: «recompensa/riesgo de 6,7» pasó porque 6,7 coincide con la rentabilidad
   a 1 mes (6,7 %). El verificador comprueba que el número exista, no a qué se refiere.
4. **Bullets repetidos (sin arreglar, ambos modelos)**: `bullCase/bearCase/risks` pueden traer la misma frase dos veces; el
   `dedupe` solo actúa sobre la tesis. En qwen3 la misma idea aparece parafraseada hasta 4 veces en la tesis (Jaccard < 0,7).
5. **Lint (sin arreglar)**: «market» de «FDV / market cap» se marca como inglés (falso positivo en cripto, ambos modelos).
6. **`think: true`**: en 1 de 9 llamadas deep el contenido llegó vacío (`respuesta vacía`); la reparación de JSON (1 reintento)
   lo resolvió. No aparecieron bloques `<think>` en `content` (Ollama separa `thinking`), así que `think.rs` no intervino.
7. **Configuración (no es un bug del código)**: `AiConfig::default()` usa 8192 y solo el `.env` fija 16384; el ejemplo
   `compare_models` no lee `.env` (se exportó `LLM_NUM_CTX=16384` a mano). Con 16384, `qwen3:8b` se reparte entre CPU y GPU.

Cambio auxiliar: `analysis::lint` pasa a `pub` (para el ejemplo). `cargo test -p ai` (218 tests) y `cargo clippy -p ai --all-targets` sin avisos.

## Limitaciones

- **Muestra pequeña**: dos activos, un snapshot por activo (un solo instante de mercado), una ejecución quick/standard por
  celda y tres deep. Sin pruebas estadísticas; los rangos son ilustrativos. Los tiempos tienen ruido (otros procesos pudieron usar Ollama/GPU: dos
  quick de AAPL con el mismo modelo y config: 41 s con 3 llamadas y 97 s con 4).
- **Calidad cualitativa subjetiva**: puntuada por un solo evaluador (un LLM), no ciego (veía el modelo de cada salida) y sin
  experto financiero humano. Las cifras se comprobaron contra el snapshot, no contra la realidad del mercado.
- Las matrices **no incluyen** los arreglos del punto 1 (citas) y la de 8192 tampoco el del punto 2 (fechas); sus descartes y
  catalizadores reflejan el pipeline anterior. Tras los arreglos solo hubo 6 ejecuciones deep de comprobación.
- La métrica «cifras» es una regex tosca; «desc.» suma cosas distintas (afirmaciones, frases de escenario).
- El pico de contexto sale de ventanas de tiempo del journal de Ollama (±2 s), por lo que puede incluir una llamada vecina.
- El modo think se midió solo en AAPL, una vez por celda; el chat con herramientas no se evaluó con qwen3; no se probaron
  cuantizaciones, `OLLAMA_FLASH_ATTENTION`, KV-cache cuantizado, otros `num_ctx` en el análisis completo (solo el sondeo de ajuste) ni más modelos.
- Comprobación de extremo a extremo por el servidor real: una ejecución `quick` de BTC con qwen3 por SSE
  (`/analysis/stream`, 4 etapas, 78 s incluido el snapshot) con la BD temporal; el modelo activo de esa BD se devolvió a
  `qwen2.5:7b` (el modelo guardado del usuario no se tocó).

## Apéndice: ejecuciones

Columnas: Prof., repetición, segundos, llamadas, tokens in/out (totales), pico de slot (tokens, máx. de una llamada),
GPU (`ollama ps`), VRAM (mediana MiB), veredicto, confianza, cifras en la tesis, descartes, ⚠, reparaciones de JSON,
reintentos por tesis genérica, correcciones de redacción, errores finales del lint. `truncated = 1`: ninguna.

### num_ctx = 16384

| Modelo | Activo | Prof. | Rep | s | Llam. | in | out | Pico | GPU | MiB | Veredicto | Conf. | Cif. | Desc. | ⚠ | JSON | Gen. | Redac. | Lint |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| qwen2.5:7b | crypto:BTC | quick | 1 | 31 | 2 | 5825 | 1413 | 4914 | 100% GPU | 5585 | hold | 0.74 | 27 | 0 | 0 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | crypto:BTC | standard | 1 | 37 | 5 | 8646 | 1721 | 4041 | 100% GPU | 5585 | hold | 0.72 | 16 | 0 | 1 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | crypto:BTC | deep | 1 | 48 | 7 | 12536 | 2250 | 4041 | 100% GPU | 5585 | hold | 0.74 | 22 | 2 | 0 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | crypto:BTC | deep | 2 | 45 | 8 | 12752 | 2115 | 4664 | 100% GPU | 5585 | hold | 0.72 | 23 | 3 | 1 | 0 | 0 | 1 | 0 |
| qwen2.5:7b | crypto:BTC | deep | 3 | 46 | 8 | 12813 | 2175 | 4547 | 100% GPU | 5585 | hold | 0.68 | 22 | 3 | 1 | 0 | 0 | 1 | 0 |
| qwen2.5:7b | stock:AAPL | quick | 1 | 36 | 3 | 7053 | 1462 | 4006 | 100% GPU | 5583 | accumulate | 0.67 | 26 | 0 | 1 | 0 | 0 | 1 | 0 |
| qwen2.5:7b | stock:AAPL | standard | 1 | 36 | 5 | 9653 | 1701 | 4006 | 100% GPU | 5585 | accumulate | 0.63 | 29 | 1 | 1 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | stock:AAPL | deep | 1 | 47 | 7 | 14020 | 2250 | 4883 | 100% GPU | 5585 | accumulate | 0.79 | 22 | 2 | 0 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | stock:AAPL | deep | 2 | 49 | 7 | 13968 | 2356 | 5010 | 100% GPU | 5585 | accumulate | 0.74 | 18 | 1 | 0 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | stock:AAPL | deep | 3 | 48 | 7 | 13993 | 2263 | 5010 | 100% GPU | 5585 | accumulate | 0.69 | 19 | 1 | 1 | 0 | 0 | 0 | 0 |
| qwen3:8b | crypto:BTC | quick | 1 | 76 | 3 | 6501 | 2153 | 5763 | 12%/88% CPU/GPU | 6761 | hold | 0.74 | 29 | 0 | 0 | 0 | 0 | 1 | 1 |
| qwen3:8b | crypto:BTC | standard | 1 | 84 | 6 | 9428 | 2466 | 4535 | 12%/88% CPU/GPU | 6761 | hold | 0.68 | 25 | 1 | 1 | 0 | 0 | 1 | 3 |
| qwen3:8b | crypto:BTC | deep | 1 | 118 | 8 | 14068 | 3447 | 5745 | 12%/88% CPU/GPU | 6761 | hold | 0.69 | 33 | 6 | 1 | 0 | 0 | 1 | 0 |
| qwen3:8b | crypto:BTC | deep | 2 | 112 | 8 | 13977 | 3292 | 5745 | 12%/88% CPU/GPU | 6761 | hold | 0.60 | 23 | 8 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | crypto:BTC | deep | 3 | 107 | 8 | 14131 | 3162 | 5555 | 12%/88% CPU/GPU | 6761 | hold | 0.74 | 19 | 5 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | quick | 1 | 69 | 3 | 7183 | 1842 | 4547 | 12%/88% CPU/GPU | 6753 | accumulate | 0.69 | 25 | 0 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | standard | 1 | 96 | 6 | 10434 | 2703 | 5195 | 12%/88% CPU/GPU | 6761 | accumulate | 0.73 | 27 | 0 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | deep | 1 | 121 | 8 | 15993 | 3443 | 6015 | 12%/88% CPU/GPU | 6761 | accumulate | 0.57 | 26 | 5 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | deep | 2 | 139 | 8 | 15816 | 3398 | 6015 | 12%/88% CPU/GPU | 6761 | accumulate | 0.49 | 30 | 10 | 1 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | deep | 3 | 112 | 8 | 15555 | 3197 | 5763 | 12%/88% CPU/GPU | 6761 | accumulate | 0.69 | 24 | 2 | 0 | 0 | 0 | 1 | 0 |

### num_ctx = 8192

| Modelo | Activo | Prof. | Rep | s | Llam. | in | out | Pico | GPU | MiB | Veredicto | Conf. | Cif. | Desc. | ⚠ | JSON | Gen. | Redac. | Lint |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| qwen2.5:7b | crypto:BTC | quick | 1 | 37 | 3 | 6477 | 1736 | 4789 | 100% GPU | 5121 | hold | 0.72 | 21 | 0 | 1 | 0 | 0 | 1 | 0 |
| qwen2.5:7b | crypto:BTC | standard | 1 | 61 | 6 | 12700 | 2782 | 5187 | 100% GPU | 5121 | hold | 0.69 | 18 | 0 | 0 | 0 | 1 | 0 | 0 |
| qwen2.5:7b | crypto:BTC | deep | 1 | 49 | 8 | 12850 | 2329 | 5187 | 100% GPU | 5121 | hold | 0.68 | 25 | 4 | 1 | 0 | 0 | 1 | 0 |
| qwen2.5:7b | crypto:BTC | deep | 2 | 47 | 7 | 12478 | 2164 | 4569 | 100% GPU | 5121 | hold | 0.62 | 22 | 2 | 3 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | crypto:BTC | deep | 3 | 76 | 9 | 17653 | 3584 | 5949 | 100% GPU | 5121 | hold | 0.70 | 20 | 3 | 0 | 0 | 1 | 1 | 1 |
| qwen2.5:7b | stock:AAPL | quick | 1 | 33 | 3 | 7036 | 1297 | 3929 | 100% GPU | 5119 | accumulate | 0.63 | 20 | 1 | 1 | 0 | 0 | 1 | 0 |
| qwen2.5:7b | stock:AAPL | standard | 1 | 39 | 5 | 9690 | 1875 | 4424 | 100% GPU | 5121 | accumulate | 0.63 | 33 | 1 | 1 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | stock:AAPL | deep | 1 | 51 | 7 | 14141 | 2441 | 5062 | 100% GPU | 5121 | accumulate | 0.65 | 24 | 2 | 1 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | stock:AAPL | deep | 2 | 50 | 7 | 14317 | 2413 | 5062 | 100% GPU | 5121 | accumulate | 0.69 | 24 | 2 | 0 | 0 | 0 | 0 | 0 |
| qwen2.5:7b | stock:AAPL | deep | 3 | 45 | 8 | 14300 | 2176 | 5036 | 100% GPU | 5121 | accumulate | 0.69 | 17 | 1 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | crypto:BTC | quick | 1 | 58 | 3 | 6698 | 2359 | 4438 | 100% GPU | 6131 | hold | 0.72 | 24 | 0 | 0 | 0 | 0 | 1 | 3 |
| qwen3:8b | crypto:BTC | standard | 1 | 69 | 6 | 9513 | 2821 | 5031 | 100% GPU | 6131 | hold | 0.62 | 25 | 3 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | crypto:BTC | deep | 1 | 87 | 8 | 14134 | 3579 | 5888 | 100% GPU | 6131 | hold | 0.66 | 25 | 3 | 2 | 0 | 0 | 1 | 0 |
| qwen3:8b | crypto:BTC | deep | 2 | 80 | 8 | 14020 | 3252 | 5888 | 100% GPU | 6131 | hold | 0.66 | 27 | 6 | 1 | 0 | 0 | 1 | 0 |
| qwen3:8b | crypto:BTC | deep | 3 | 88 | 8 | 14104 | 3512 | 5722 | 100% GPU | 6131 | hold | 0.68 | 27 | 6 | 2 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | quick | 1 | 57 | 2 | 6758 | 2105 | 4738 | 100% GPU | 6123 | accumulate | 0.67 | 23 | 1 | 1 | 0 | 0 | 0 | 0 |
| qwen3:8b | stock:AAPL | standard | 1 | 65 | 6 | 10498 | 2676 | 5173 | 100% GPU | 6131 | accumulate | 0.73 | 33 | 0 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | deep | 1 | 83 | 8 | 15819 | 3368 | 5926 | 100% GPU | 6131 | accumulate | 0.65 | 31 | 5 | 1 | 0 | 0 | 1 | 1 |
| qwen3:8b | stock:AAPL | deep | 2 | 87 | 8 | 15916 | 3541 | 6075 | 100% GPU | 6131 | accumulate | 0.57 | 28 | 8 | 0 | 0 | 0 | 1 | 0 |
| qwen3:8b | stock:AAPL | deep | 3 | 82 | 8 | 15842 | 3355 | 5913 | 100% GPU | 6131 | accumulate | 0.72 | 19 | 6 | 1 | 0 | 0 | 1 | 1 |

### Con `LLM_THINK=1` (AAPL)

| `num_ctx` | Prof. | s | Llam. | in | out | Pico | GPU | MiB | Conf. | Cif. | Desc. | ⚠ | JSON | Redac. | Lint |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 8192 | quick | 81 | 3 | 7131 | 3341 | 5072 | 100% GPU | 6131 | 0.73 | 25 | 0 | 0 | 0 | 1 | 0 |
| 8192 | deep | 281 | 9 | 16688 | 11939 | 6292 | 100% GPU | 6131 | 0.61 | 30 | 6 | 0 | 1 | 1 | 0 |
| 16384 | quick | 104 | 3 | 7116 | 3010 | 4769 | 12%/88% CPU/GPU | 6761 | 0.65 | 35 | 2 | 0 | 0 | 1 | 0 |
| 16384 | deep | 314 | 8 | 15801 | 9013 | 6554 | 12%/88% CPU/GPU | 6761 | 0.68 | 34 | 4 | 1 | 0 | 1 | 0 |

### Después de los arreglos (16384, deep, una por celda, solo comprobación)

qwen3:8b: AAPL 125 s, BTC 109 s (una de ellas con «Se quitaron 3 cita(s) a noticias inexistentes…», sin ningún descarte por `[N#]`);
qwen2.5:7b: AAPL 55 s, BTC 77 s (9 llamadas). Ningún descarte por cita inexistente en ninguna de las cuatro.

---

## 2026-10-19: comparación de cuatro modelos sobre el pipeline actual (q8_0, `num_ctx` 16384)

Compara; **no cambia nada** (ni el modelo activo ni el catálogo). Estado: matriz de `quick` y `standard` completa y prueba de forzar todas las capas en GPU hecha; `deep` no se midió.

### Condiciones

- Código: `2d50cc6` (quick arreglado + verificador con plazos «a 30 días»); un único binario `compare_models` compilado de ese commit (`~/.cache/invest-ai/eval/models`). `qwen2.5:7b`, `qwen3:8b`, `qwen3.5:4b`, `qwen3.5:9b`; Ollama **0.40.4** en todas las ejecuciones (el paquete se actualizó a 0.40.6 el 19-oct a las 13:07 y el servidor no se reinició hasta después de acabar la matriz), `OLLAMA_FLASH_ATTENTION=1`, **caché KV `q8_0`** (`--cache-type-k/-v q8_0` en las 96 ejecuciones, `kvChanged=false`), `think` apagado como en la app (se envía `think:false` a los modelos con capacidad «thinking»), temperatura 0,2, timeout 180 s, español. Snapshots congelados `tests/fixtures/eval/*.json` (los mismos de la ablación).
- Matriz: 4 modelos × {quick, standard} × 4 activos × 3 repeticiones = 96 ejecuciones (n = 12 por modelo y profundidad). Cada modelo se precarga antes de su bloque y los bloques se rotan entre repeticiones (los modelos no se intercalan ejecución a ejecución: cada cambio recargaría el modelo y la carga entraría en el tiempo). Enchufado y GPU sin procesos ajenos en las 96 (`timingValid=true` en todas).
- Tres sesiones: **16-oct tarde** (24 ejecuciones), **19-oct mañana** (~16) y **19-oct tarde** (~56), con dos reinicios del equipo en medio. Los lotes se declaran porque **no son equivalentes en tiempo** (siguiente apartado).
- Métricas: las de la ablación (definiciones en `ablation.md`), calculadas por `ablation_report.py`. n pequeño: `*` = IC 95 % bootstrap de la diferencia frente a `qwen2.5:7b` (el modelo actual) excluye 0.

### Hallazgo previo: los tiempos dependen de la sesión, y de cuánto del modelo cae en la CPU

| | 16-oct tarde | 19-oct (mañana y tarde) |
|---|---|---|
| `qwen3:8b` en GPU (`/api/ps`: `size_vram/size`) | **91 %** (ya con q8_0) | **100 %** |
| `qwen3.5:9b` en GPU | **83 %** | **100 %** |
| `qwen2.5:7b`, `qwen3.5:4b` en GPU | 100 % | 100 % |
| reloj SM medio / potencia media / carga de CPU | 2106–2342 MHz / 69–83 W / 8–11 | 2254–2443 MHz / 85–103 W / 4–11 |

Tiempos medios por lote (s):

| modelo, profundidad | 16-oct (n) | 19-oct mañana (n) | 19-oct tarde (n) |
|---|---|---|---|
| qwen2.5:7b quick | 21,2 (4) | 19,5 (1) | 18,0 (7) |
| qwen2.5:7b standard | 54,3 (4) | – | 37,4 (8) |
| qwen3:8b quick / standard | 27,2 (4) / 84,3 (4) | 22,1 (4) / 63,1 (4) | 22,0 (4) / 64,1 (4) |
| qwen3.5:4b quick / standard | 16,5 (4) / 43,6 (4) | 15,5 (4) / 36,2 (4) | 14,7 (4) / 37,0 (4) |
| qwen3.5:9b quick / standard | 37,2 (2) / 100,6 (2) | 27,0 (6) / 55,2 (6) | 27,3 (4) / 55,8 (4) |

- Los modelos que **ya iban enteros en GPU el 16-oct** (qwen2.5:7b, qwen3.5:4b) tampoco son iguales entre lotes (qwen2.5:7b standard 54 s frente a 37 s; qwen3.5:4b standard 44 s frente a 37 s): coincide con un reloj y una potencia de GPU menores (82 W frente a 100 W) y más carga de CPU el 16-oct. No sé qué la causó (plan de energía, temperatura, otros agentes compilando); solo lo registro.
- En `qwen3:8b` y `qwen3.5:9b` se suma el reparto GPU/CPU: el 16-oct `qwen3.5:9b` standard tardaba 101 s (83 % en GPU) y el 19-oct 55 s (100 %).
- **Conclusión de método: los tiempos se comparan solo con las ejecuciones del 19-oct** (todas con el modelo entero en GPU, reloj ~2300–2440 MHz, ~90–100 W). Las del 16-oct entran en la calidad (mismo código, misma configuración, sin diferencia de fondo: el reparto CPU/GPU no cambia el texto salvo ruido numérico) pero no en los tiempos.
- El **reparto depende de la sesión**: en el mismo equipo y con la misma configuración (`num_ctx` 16384, q8_0) `qwen3:8b` quedó al 91 % en GPU un día y al 100 % otro. Es un caso límite del ajuste de Ollama (disponible ~7,0–7,2 GiB frente a la estimación propia de ~7,1 GiB para qwen3:8b; ver «Predicciones»), no una propiedad estable del modelo.

### Resultados (calidad: las 12 ejecuciones por celda; tiempo: ver más abajo)

#### quick

| métrica | n (qwen2.5-7b/qwen3-8b/qwen3.5-4b/qwen3.5-9b) | qwen2.5:7b | qwen3:8b | qwen3.5:4b | qwen3.5:9b |
|---|---|---|---|---|---|
| llamadas LLM | 12/12/12/12 | 3.2 [3.0–4.0] | 3.0 [3.0–3.0] | 3.2 [3.0–4.0] | 3.5 [3.0–4.0] |
| tokens in | 12/12/12/12 | 7545 [6800–8483] | 7488 [6881–8045] | 7483 [6771–8388] | 7712 [7015–8489] |
| tokens out | 12/12/12/12 | 792 [687–904] | 834 [787–882]* | 802 [747–871] | 959 [864–1094]* |
| cifras en tesis | 12/12/12/12 | 9.8 [7.0–14.0] | 11.5 [7.0–14.0] | 11.5 [9.0–15.0] | 11.7 [9.0–14.0]* |
| cifras distintas en tesis | 12/12/12/12 | 8.4 [6.0–11.0] | 9.8 [7.0–12.0]* | 10.3 [8.0–13.0]* | 11.3 [9.0–13.0]* |
| cifras (todo el texto) | 12/12/12/12 | 18.2 [14.0–25.0] | 23.1 [17.0–27.0]* | 21.0 [17.0–26.0]* | 21.2 [17.0–25.0]* |
| verificadas por etiqueta | 12/12/12/12 | 16.9 [14.0–24.0] | 20.9 [14.0–25.0]* | 19.7 [17.0–23.0]* | 18.3 [12.0–23.0] |
| verificadas solo por valor | 12/12/12/12 | 0.8 [0.0–2.0] | 2.2 [1.0–4.0]* | 1.3 [0.0–5.0] | 2.7 [0.0–5.0]* |
| no verificadas (⚠) | 12/12/12/12 | 0.08 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.17 [0.00–1.00] |
| ⚠ en resumen/tesis/viñetas/riesgos/catalizadores | 12/12/12/12 | 0.17 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| ⚠ en «qué cambiaría» (umbrales hipotéticos) | 12/12/12/12 | 0.25 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.17 [0.00–1.00] |
| % verificadas por etiqueta | 12/12/12/12 | 94.9 [87.5–100.0] | 90.3 [81.0–95.8]* | 94.1 [77.3–100.0] | 86.4 [70.6–95.8]* |
| % no verificadas | 12/12/12/12 | 0.44 [0.00–5.26] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.70 [0.00–4.35] |
| etiqueta errónea | 12/12/12/12 | 0.33 [0.00–2.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| descartes (afirm.+frases) | 12/12/12/12 | 0.00 [0.00–0.00] | 0.17 [0.00–1.00] | 0.50 [0.00–1.00]* | 0.00 [0.00–0.00] |
| cifras inventadas atrapadas | 12/12/12/12 | 0.00 [0.00–0.00] | 0.33 [0.00–2.00] | 1.00 [0.00–2.00]* | 0.00 [0.00–0.00] |
| tesis genérica detectada | 12/12/12/12 | 0.08 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| reintentos por tesis genérica (llamada extra) | 12/12/12/12 | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| tesis genérica que queda (confianza −0,05) | 12/12/12/12 | 0.08 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| correcciones de redacción | 12/12/12/12 | 0.25 [0.00–1.00] | 0.00 [0.00–0.00] | 0.17 [0.00–1.00] | 0.50 [0.00–1.00] |
| errores lint finales | 12/12/12/12 | 0.33 [0.00–3.00] | 0.00 [0.00–0.00] | 0.25 [0.00–2.00] | 0.58 [0.00–2.00] |
| veredicto = umbral de puntuación | 12/12/12/12 | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] | 1.00 [1.00–1.00] |
| confianza | 12/12/12/12 | 0.76 [0.71–0.81] | 0.72 [0.67–0.78]* | 0.72 [0.58–0.78]* | 0.69 [0.63–0.76]* |
| palabras de la tesis | 12/12/12/12 | 120 [71–159] | 131 [90–162] | 146 [127–179]* | 165 [144–185]* |
| palabras del resumen | 12/12/12/12 | 22 [13–29] | 17 [15–21]* | 22 [14–31] | 19 [16–26] |
| viñetas alcistas | 12/12/12/12 | 1.0 [1.0–1.0] | 1.0 [1.0–1.0] | 1.0 [1.0–1.0] | 1.0 [1.0–1.0] |
| viñetas bajistas | 12/12/12/12 | 1.0 [1.0–1.0] | 0.8 [0.0–1.0] | 1.0 [1.0–1.0] | 1.0 [1.0–1.0] |
| riesgos | 12/12/12/12 | 2.8 [2.0–3.0] | 3.0 [3.0–3.0] | 3.0 [3.0–3.0] | 3.0 [3.0–3.0] |
| «qué cambiaría» | 12/12/12/12 | 2.0 [2.0–2.0] | 2.0 [2.0–2.0] | 1.9 [1.0–2.0] | 2.0 [2.0–2.0] |
| catalizadores | 12/12/12/12 | 0.0 [0.0–0.0] | 0.0 [0.0–0.0] | 0.0 [0.0–0.0] | 0.0 [0.0–0.0] |
| segundos (solo tiempos válidos) | 12/12/12/12 | 19.2 [15.8–23.0] | 23.8 [20.5–28.1]* | 15.5 [13.9–17.6]* | 28.8 [24.5–37.3]* |

#### standard

| métrica | n (qwen2.5-7b/qwen3-8b/qwen3.5-4b/qwen3.5-9b) | qwen2.5:7b | qwen3:8b | qwen3.5:4b | qwen3.5:9b |
|---|---|---|---|---|---|
| llamadas LLM | 12/12/12/12 | 5.4 [5.0–7.0] | 6.0 [6.0–6.0]* | 5.2 [5.0–6.0] | 5.6 [5.0–6.0] |
| tokens in | 12/12/12/12 | 11024 [9436–15688] | 11002 [10372–11583] | 10534 [9685–11534] | 10866 [9936–11902] |
| tokens out | 12/12/12/12 | 1898 [1511–2874] | 2633 [2105–2951]* | 2241 [2008–2538]* | 2148 [1691–2787] |
| cifras en tesis | 12/12/12/12 | 12.1 [7.0–17.0] | 16.4 [11.0–25.0]* | 17.3 [8.0–31.0]* | 15.2 [9.0–29.0] |
| cifras distintas en tesis | 12/12/12/12 | 9.1 [7.0–12.0] | 14.8 [11.0–20.0]* | 15.1 [8.0–25.0]* | 14.2 [9.0–24.0]* |
| cifras (todo el texto) | 12/12/12/12 | 34.8 [26.0–56.0] | 45.9 [35.0–59.0]* | 49.5 [33.0–68.0]* | 38.9 [22.0–64.0] |
| verificadas por etiqueta | 12/12/12/12 | 24.7 [15.0–31.0] | 35.2 [24.0–45.0]* | 38.5 [24.0–53.0]* | 29.9 [15.0–56.0] |
| verificadas solo por valor | 12/12/12/12 | 9.5 [3.0–31.0] | 9.5 [5.0–23.0] | 10.6 [7.0–15.0] | 8.6 [5.0–13.0] |
| no verificadas (⚠) | 12/12/12/12 | 0.33 [0.00–1.00] | 0.67 [0.00–3.00] | 0.08 [0.00–1.00] | 0.08 [0.00–1.00] |
| ⚠ en resumen/tesis/viñetas/riesgos/catalizadores | 12/12/12/12 | 0.33 [0.00–2.00] | 0.08 [0.00–1.00] | 0.08 [0.00–1.00] | 0.08 [0.00–1.00] |
| ⚠ en «qué cambiaría» (umbrales hipotéticos) | 12/12/12/12 | 0.42 [0.00–2.00] | 0.67 [0.00–2.00] | 0.17 [0.00–1.00] | 0.08 [0.00–1.00] |
| % verificadas por etiqueta | 12/12/12/12 | 73.2 [41.8–89.3] | 78.1 [58.6–87.5] | 78.2 [72.7–85.7] | 75.6 [57.7–87.5] |
| % no verificadas | 12/12/12/12 | 0.89 [0.00–3.57] | 1.44 [0.00–6.67] | 0.21 [0.00–2.50] | 0.19 [0.00–2.22] |
| etiqueta errónea | 12/12/12/12 | 0.25 [0.00–1.00] | 0.08 [0.00–1.00] | 0.17 [0.00–1.00] | 0.08 [0.00–1.00] |
| descartes (afirm.+frases) | 12/12/12/12 | 0.08 [0.00–1.00] | 0.75 [0.00–2.00]* | 0.58 [0.00–2.00]* | 0.58 [0.00–2.00]* |
| cifras inventadas atrapadas | 12/12/12/12 | 0.17 [0.00–2.00] | 1.33 [0.00–3.00]* | 1.00 [0.00–4.00] | 0.83 [0.00–4.00] |
| tesis genérica detectada | 12/12/12/12 | 0.17 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| reintentos por tesis genérica (llamada extra) | 12/12/12/12 | 0.17 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| tesis genérica que queda (confianza −0,05) | 12/12/12/12 | 0.17 [0.00–1.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] | 0.00 [0.00–0.00] |
| correcciones de redacción | 12/12/12/12 | 0.25 [0.00–1.00] | 1.00 [1.00–1.00]* | 0.17 [0.00–1.00] | 0.58 [0.00–1.00] |
| errores lint finales | 12/12/12/12 | 0.00 [0.00–0.00] | 0.92 [0.00–5.00]* | 0.25 [0.00–2.00] | 0.75 [0.00–3.00]* |
| veredicto = umbral de puntuación | 12/12/12/12 | 0.92 [0.00–1.00] | 1.00 [1.00–1.00] | 0.92 [0.00–1.00] | 0.92 [0.00–1.00] |
| confianza | 12/12/12/12 | 0.69 [0.56–0.81] | 0.66 [0.54–0.72] | 0.69 [0.59–0.77] | 0.66 [0.53–0.75] |
| palabras de la tesis | 12/12/12/12 | 160 [114–196] | 283 [229–329]* | 228 [169–341]* | 212 [160–375]* |
| palabras del resumen | 12/12/12/12 | 40 [24–77] | 37 [27–52] | 41 [30–55] | 43 [25–68] |
| viñetas alcistas | 12/12/12/12 | 3.0 [2.0–4.0] | 4.0 [4.0–4.0]* | 3.6 [3.0–4.0]* | 2.2 [0.0–4.0] |
| viñetas bajistas | 12/12/12/12 | 3.1 [3.0–4.0] | 3.9 [3.0–4.0]* | 3.9 [3.0–4.0]* | 2.3 [0.0–4.0] |
| riesgos | 12/12/12/12 | 0.9 [0.0–2.0] | 2.7 [0.0–5.0]* | 2.8 [2.0–3.0]* | 1.9 [0.0–4.0] |
| «qué cambiaría» | 12/12/12/12 | 3.0 [3.0–3.0] | 3.6 [3.0–4.0]* | 2.8 [1.0–3.0] | 1.8 [0.0–3.0]* |
| catalizadores | 12/12/12/12 | 0.5 [0.0–2.0] | 3.0 [1.0–4.0]* | 0.4 [0.0–2.0] | 0.3 [0.0–2.0] |
| segundos (solo tiempos válidos) | 12/12/12/12 | 43.0 [33.5–69.9] | 70.5 [52.8–91.6]* | 38.9 [34.4–46.9] | 62.9 [46.0–106.7]* |


La fila de segundos de estas tablas mezcla lotes; **usa la tabla de tiempos de abajo**.

### Tiempos (solo 19-oct, modelo entero en GPU, enchufado)

| modelo | quick: n, media [mín–máx] | Δ vs qwen2.5:7b (IC 95 %) | standard: n, media [mín–máx] | Δ vs qwen2.5:7b (IC 95 %) | tok/s de salida en el pipeline (standard) |
|---|---|---|---|---|---|
| qwen2.5:7b | 8, 18,2 s [15,8–19,8] | – | 8, 37,4 s [33,5–44,3] | – | 46 |
| qwen3:8b | 8, 22,0 s [20,5–23,2] | +2,7 a +5,1 s * | 8, 63,6 s [52,8–71,2] | +21,5 a +30,3 s * | 41 |
| qwen3.5:4b | 8, 15,1 s [13,9–16,7] | −4,3 a −1,9 s * | 8, 36,6 s [34,4–38,4] | −3,5 a +1,6 s (ruido) | 60 |
| qwen3.5:9b | 10, 27,1 s [24,5–30,2] | +7,5 a +10,5 s * | 10, 55,4 s [46,0–73,2] | +11,9 a +24,5 s * | 37 |

(«tok/s de salida» = tokens de salida / segundo de pared, incluye prefill y llamadas; no es la velocidad pura de generación.) Medido en el 19-oct, con 100 % en GPU, la penalización de `qwen3:8b` y `qwen3.5:9b` frente a `qwen2.5:7b` es de ×1,2–1,5 en `quick` y ×1,5–1,7 en `standard` (por más tokens de salida —qwen3:8b genera un 39 % más en standard— y una generación más lenta, 37–41 tok/s frente a 46). Por activo, el ranking de `standard` se mantiene en los cuatro (qwen3:8b 58–70 s; qwen2.5:7b 34–42 s).

### VRAM, reparto y velocidad medidos (`bench_model.py`, 16-oct, `num_ctx` 16384, q8_0)

| modelo | `size` (GiB) | `size_vram` (GiB) / % GPU | nvidia-smi: VRAM del modelo (MiB, sin escritorio ≈ 815–848) | generación (tok/s) | prefill (tok/s, 7,7k tokens) |
|---|---|---|---|---|---|
| qwen2.5:7b | 4,69 | 4,69 / 100 % | 4962 | 44 | 2829 |
| qwen3:8b | 6,27 | 5,59 / 89 % | 5939 | 30 | 2259 |
| qwen3.5:4b | 3,20 | 3,20 / 100 % | 4088 | 60 | 3655 |
| qwen3.5:9b | 6,04 | 5,03 / 83 % | 6188 | 27 | 1812 |

El 19-oct, con el pipeline, `ollama ps` dio 100 % GPU para los cuatro (qwen3:8b: `size` 5,85 GiB; qwen3.5:9b: 5,40 GiB; qwen2.5:7b: 4,84 GiB; qwen3.5:4b: 3,20 GiB): las estimaciones propias de Ollama varían con la sesión. La velocidad de generación del 19-oct (derivada del pipeline) es mayor que la del 16-oct para qwen3:8b y qwen3.5:9b (38–41 y 36–37 tok/s frente a 30 y 27) porque pasaron de reparto parcial a entero en GPU. **Antes, con f16 (`model-eval.md`, 2026-10-14), `qwen3:8b` iba un 12 % en CPU; con q8_0 el 16-oct iba un 9–11 % y el 19-oct entero**: q8_0 mejora el ajuste pero no lo garantiza.

### Predicciones de la app frente a lo medido (`LLM_KV_CACHE_TYPE=q8_0`, `num_ctx` 16384; no se cambia el catálogo)

Predicción del catálogo (`catalog_pred.rs`, mismas fórmulas que `modelFit`), en GiB:

| modelo | recomendada | mínima | medido: `size` de Ollama | medido: VRAM real (nvidia-smi) | ¿cabe entero? app / medido |
|---|---|---|---|---|---|
| qwen2.5:7b | 5,4 | 2,8 | 4,7–4,8 | 4,85 | sí / sí |
| qwen3:8b | 6,7 | 3,7 | 5,85–6,27 | 5,80 | sí / **depende de la sesión (91 % o 100 %)** |
| qwen3.5:4b | 4,0 | 2,1 | 3,20 | 3,99 | sí / sí |
| qwen3.5:9b | 7,0 | 3,3 | 5,40–6,04 | 6,04 | sí (justo) / **depende de la sesión (83 % o 100 %)** |

- La KV se predice bien: `qwen3:8b` 36 capas × 8 cabezas × 256 × 2 B × 16384 × 0,53125 = 1224 MiB; el journal de Ollama da `K (q8_0): 612 MiB, V (q8_0): 612 MiB, 36 layers`. En f16 predecía 7,7 GiB para qwen3:8b (12 % en CPU en la medición anterior), coherente con lo observado.
- La «recomendada» sobreestima el uso real para qwen2.5:7b, qwen3:8b y qwen3.5:9b (lado seguro) y acierta en qwen3.5:4b. **Corrección (20-oct):** la columna «VRAM real» de esta tabla usa las medidas del 16-oct, que en qwen3:8b y qwen3.5:9b eran con reparto parcial (5,80 y 6,04 GiB); con el modelo entero en GPU (19-oct) ocupan 6,02 y 6,42 GiB, así que la sobreestimación real es de 0,55 / 0,68 / 0,58 GiB (qwen2.5:7b, qwen3:8b, qwen3.5:9b) y 0,0 en qwen3.5:4b. Original: «0,5–0,9 GiB» (3,99 medido frente a 4,0; ahí `size` de Ollama, 3,2, subestima lo que muestra nvidia-smi).
- **Posible predicción errónea**: para `qwen3:8b` y `qwen3.5:9b` la app dice que caben enteros en 8 GiB, y el 16-oct no fue así. Ollama decide con su propia estimación (el journal del 19-oct registra `predicted="7.1 GiB"` justo antes de cargar `qwen3:8b` con `num_ctx` 16384 y `available="7.0 GiB"`, frente a los 6,7 GiB del catálogo), no con la del catálogo. Propuesta (solo números, sin implementarla): marcar «cabe justo / puede ir parcialmente en CPU» cuando `recomendada > VRAM_total − margen`. **Corrección (20-oct):** el margen de 1,2 GiB propuesto aquí no sumaba el escritorio (la aritmética «8 − 1,2 ≈ 6,0» era errónea); la calibración correcta es ~0,9 GiB de escritorio + ~1,2 GiB de holgura de Ollama = 2,1 GiB (umbral ≈ 5,9 GiB en 8 GB), y se aplicó junto con las recomendadas corregidas del catálogo (ver `ai.md`, «Cabe justo»). Evidencia: 4 modelos × 2 escenarios; es una regla de una máquina.

### «Think» en la familia qwen3 / qwen3.5

Con `think:false` (lo que envía la app a los modelos con capacidad «thinking»; qwen3:8b, qwen3.5:4b y qwen3.5:9b la declaran): **no se observó razonamiento filtrado**. En la sonda de `bench_model.py` (problema de trenes, 600 tokens máx.) `thinking` vacío y sin `<think>` en el contenido en los tres; en las 96 salidas del pipeline no hay `<think>`/`thinking` en ninguna, y los tokens de salida y las latencias son coherentes entre llamadas (no hay llamadas anómalas: máx. standard 91,6 s en qwen3:8b del 16-oct con reparto parcial, 73 s el 19-oct). Los tokens de salida por análisis son normales para un modelo que no razona: quick ≈ 800–960; standard 1900–2630. Si el usuario activara `LLM_THINK=1` la situación sería otra (en la medición anterior, deep 281–314 s con qwen3:8b); no se midió aquí.

### Lectura de calidad (solo lo que cabe en n = 12)

- **`quick`:** los cuatro dan el mismo formato (1 viñeta alcista y 1 bajista, 3 riesgos, 2 condiciones, sin catalizadores). `qwen2.5:7b` cita menos cifras distintas en la tesis (8,4 frente a 9,8–11,3*), pero es el que más cifras verifica por etiqueta en proporción (94,9 % frente a 90,3 % qwen3:8b* y 86,4 % qwen3.5:9b*; qwen3.5:4b 94,1 %). Etiquetas erróneas: 0,33 por análisis en qwen2.5:7b frente a 0 en los otros tres. `qwen3.5:4b` es el que más afirmaciones se le descartan (0,5 por análisis*, 1,0 cifras inventadas atrapadas*): inventa más pero el verificador las quita.
- **`standard`:** los tres modelos de la familia qwen3/3.5 citan más cifras (distintas en la tesis: 14,2–15,1 frente a 9,1*), tesis más largas (212–283 palabras frente a 160*) y más viñetas, pero también **más afirmaciones descartadas** (0,6–0,75 por análisis frente a 0,08*) y más cifras inventadas atrapadas (0,8–1,3 frente a 0,17; solo qwen3:8b con IC que excluye 0). `qwen3:8b` además necesita la llamada de corrección de redacción en el 100 % de las ejecuciones (1,00 frente a 0,25*) y deja más errores de castellano (0,92 por análisis*), y genera 3,0 catalizadores por análisis frente a 0,5*. `qwen3.5:4b` es el más limpio de los nuevos (lint 0,25, ⚠ 0,08).
- Fiabilidad de forma: veredicto = umbral de puntuación en 12/12 salvo 1 ejecución por modelo en `crypto:BTC` standard (en qwen2.5:7b, qwen3.5:4b y qwen3.5:9b; es la aproximación por umbrales sin los ajustes de riesgo, ver `ablation.md`). Confianza similar (0,66–0,69 en standard).
- **Qué no mide esto:** corrección financiera, utilidad ni calidad de redacción; más cifras no es mejor análisis, y cifras descartadas no son errores que lleguen al usuario (el verificador las quita). Un único evaluador automático, 4 activos, un snapshot por activo.

### Recomendación para este equipo (GPU NVIDIA de portátil con 8 GB, q8_0, `num_ctx` 16384)

No hay un ganador claro con esta muestra; esto es lo que sostienen los datos:

1. **Tiempo:** `qwen3.5:4b` ≈ `qwen2.5:7b` en `standard` (36,6 frente a 37,4 s, ruido) y algo más rápido en `quick` (15 frente a 18 s); `qwen3:8b` y `qwen3.5:9b` cuestan ×1,5–1,7 en standard (55–64 s) y, según la sesión, pueden ir parcialmente en CPU (y entonces tardar mucho más: qwen3.5:9b 101 s).
2. **Calidad medible:** los qwen3/3.5 aportan más cifras y textos más largos en `standard`, a costa de más afirmaciones descartadas. `qwen2.5:7b` es el más conservador (menos descartes, lint limpio) y el menos específico. No hay evidencia, con estas métricas, de que ninguno dé mejores decisiones.
3. **Si hubiera que elegir hoy:** `qwen2.5:7b` sigue siendo una opción segura (sin riesgo de ajuste, mismo tiempo que `qwen3.5:4b`, menos descartes); **`qwen3.5:4b` es el candidato a probar como alternativa**: igual de rápido, 3,4 GB de pesos (deja más de 4 GiB libres para el escritorio y para un contexto mayor), más cifras específicas en `standard`; sus puntos débiles medidos (más afirmaciones descartadas, 1 cifra inventada atrapada por análisis en `quick`) no llegan al usuario porque el verificador las filtra, pero indican que el modelo es menos fiel al contexto. **`qwen3:8b` y `qwen3.5:9b` no se justifican en este equipo** por tiempo y por el riesgo de reparto parcial, sin una ventaja de calidad que lo compense.
4. Esta recomendación es **no concluyente entre `qwen2.5:7b` y `qwen3.5:4b`**: lo que decide es si se valoran más las cifras específicas (a favor de 3.5:4b) o menos descartes (a favor de 2.5:7b), y eso requiere evaluación humana ciega con más activos.

### Forzar todas las capas en GPU (`options.num_gpu = 999`)

Hecho el 19-oct con **Ollama 0.40.6** (el servidor se actualizó tras la matriz; no es la misma versión que las tablas anteriores), enchufado, q8_0, `num_ctx` 16384. `bench_model.py` (3 velocidades con un prompt de ~7,7k tokens + 1 petición de ~13–14k tokens con pico de VRAM muestreado cada 0,5 s) y un `standard` real de `stock:AAPL` ×2 con `num_gpu` inyectado por un proxy (`gpu_proxy.py`; el código de producción no tiene esa opción). Dos escenarios:

- **A. Escritorio liberado** (79 MiB usados en la GPU antes de cargar). Es el estado del 19-oct.
- **B. Escritorio ocupando ~1 GB**: un proceso mío (`hold_vram.cu`, 760 MiB) deja la GPU en 975 MiB usados antes de cargar, parecido a los 815–848 MiB del 16-oct (algo más exigente). Con él se **reproduce el reparto parcial del 16-oct**.

| escenario | modelo | carga | `size_vram`/`size` | VRAM usada con el modelo (MiB) | pico con ~14k tokens de prompt (MiB) | VRAM libre en el pico (MiB) | generación (tok/s) | prefill (tok/s) |
|---|---|---|---|---|---|---|---|---|
| A | qwen3:8b | defecto | 100 % | 6245 | 6253 | 1935 | 43 | 2806 |
| A | qwen3:8b | `num_gpu` 999 | 100 % | 6245 | 6253 | 1935 | 43 | 2752 |
| A | qwen3.5:9b | defecto | 100 % | 6653 | 6661 | 1527 | 40 | 2667 |
| A | qwen3.5:9b | `num_gpu` 999 | 100 % | 6653 | 6661 | 1527 | 38–40 | 2570 |
| B | qwen3:8b | defecto | **89 %** (5,59 de 6,27 GiB) | 6878 | 6896 | 1292 | **30–31** | 2357 |
| B | qwen3:8b | `num_gpu` 999 | 100 % | 7138 | 7146 | **1042** | **41–42** | 2727 |
| B | qwen3.5:9b | defecto | **81 %** (4,87 de 6,05 GiB) | 7008 | 7022 | 1166 | **28–29** | 1972 |
| B | qwen3.5:9b | `num_gpu` 999 | 100 % | 7546 | 7554 | **634** | **39–40** | 2618 |

(Velocidades: 3 repeticiones cada una, rango de las tres; la petición larga tardó sin fallos en todos los casos; en B con el defecto, la generación con 14k tokens de prompt cayó a 18–20 tok/s y con `num_gpu` 999 se mantuvo en 35–39.) Respuestas a las cuatro preguntas:

1. **¿Carga sin error de memoria?** Sí, en los dos escenarios y los dos modelos; ningún error ni evicción. Con `num_gpu` 999 en B usa 6163 MiB (qwen3:8b) y 6571 MiB (qwen3.5:9b) además de los 975 MiB ajenos.
2. **Velocidad:** en A **no cambia nada** (ya iban enteros en GPU: mismo `size_vram`, 43 y 40 tok/s). En B forzar gana mucho: generación **+37 % en qwen3:8b (30 → 42 tok/s) y +40 % en qwen3.5:9b (28 → 40)**, prefill +16 % y +33 %, y con 14k tokens de prompt la generación pasa de 18–20 a 35–39 tok/s.
3. **`standard` real con `num_gpu` forzado** (AAPL ×2 por modelo, escenario B, sin caídas ni errores de memoria con el contexto creciendo): qwen3:8b 59,5 y 59,6 s; qwen3.5:9b 70,7 y 61,8 s (6 y 5–6 llamadas). Comparación indicativa, no pareada, con los lotes de la matriz (misma máquina, otro día y otra versión de Ollama): reparto parcial del 16-oct qwen3:8b 84 s, qwen3.5:9b 101 s; entero el 19-oct 64 y 55 s. Esos dos `standard` no son tiempos válidos para comparar modelos (el proceso ajeno de 760 MiB figura como `gpu_shared`).
4. **Margen para el escritorio:** con `num_gpu` forzado y el contexto lleno, quedan **1042 MiB libres en qwen3:8b y 634 MiB en qwen3.5:9b** por encima de los 975 MiB ya ocupados (≈1,0 y ≈0,6 GiB de margen total para usos puntuales). Ambos están por encima del umbral de ~300 MiB, pero **qwen3.5:9b queda justo**: un navegador con aceleración que pase de ~1,5 GiB en total en la GPU lo dejaría sin sitio, y con `num_gpu` forzado Ollama no tiene reparto de reserva (fallaría la carga o la petición en lugar de pasar capas a CPU). `qwen3:8b` deja más holgura. En A el margen es de ~1,5–1,9 GiB.

**Qué significa:** el reparto parcial del 16-oct no era un límite del modelo sino de la memoria libre: cuando el escritorio ocupa ~1 GB, Ollama (0.40.4 y 0.40.6) deja 11–19 % de las capas en CPU y la generación cae ~30 %. Forzar todas las capas cabe y gana ~37–40 %, a costa de no tener red de seguridad.

**Propuesta para la app (no implementada):** (a) una opción `LLM_NUM_GPU` (entero; vacía = decisión de Ollama) que `ChatParams::body()` incluiría en `options.num_gpu`; (b) mejor, que `modelFit` la decida por modelo: si el modelo cabe entero con ≥ ~1 GiB de margen sobre `recomendada` y la VRAM libre medida al cargar (`/api/ps` o nvidia-smi) y el reparto devuelto es parcial, reintentar con `num_gpu` = nº de capas (999) y avisar «sin margen para el escritorio» si el margen estimado en contexto lleno queda < ~600 MiB; (c) mostrar en Ajustes el % en GPU real (`size_vram/size`), que hoy no se ve. Con 8 GB, `qwen3.5:9b` forzado es viable pero justo; `qwen3:8b` forzado deja más margen.

Notas y limitaciones de esta prueba: n = 3 velocidades por celda y 2 `standard` por modelo; una sola GPU con un ocupante sintético de 760 MiB (no es un navegador real: su consumo varía); Ollama 0.40.6 (matriz en 0.40.4); no se probó `num_ctx` mayor ni otros modelos; se descargaron todos los modelos (`keep_alive: 0`) y se paró el proxy al terminar. Salidas en `docs/notes/eval-data/models/numgpu/` (escenario A) y `numgpu-held/` (B); herramientas `numgpu_probe.sh`, `bench_model.py`, `gpu_proxy.py`, `hold_vram.cu` (ocupante sintético de VRAM; compilar con `nvcc`).

### Pendiente

- `deep` (1 repetición por activo) no se midió.

### Implementado (20-oct-2026)

Las propuestas de arriba están en el código: overhead medido solo para estos cuatro modelos, aviso «cabe justo» y `num_gpu` (global y por modelo, con vuelta al reparto automático si falta memoria). Detalle, constantes y cifras corregidas en `ai.md`.
