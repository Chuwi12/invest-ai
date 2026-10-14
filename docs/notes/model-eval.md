# Evaluación de modelos: `qwen2.5:7b` frente a `qwen3:8b`

Fecha: 2026-10-14. Hardware: RTX 4070 Laptop (8188 MiB de VRAM), Ollama local con `-np 1`. Este documento compara; **no
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
