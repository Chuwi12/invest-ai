# Notas del crate `ai`

Estado: **funcional, en afinado de calidad**. Se actualiza tras cada parte.

## Arquitectura del pipeline (`backend/crates/ai/src/analysis/`)

```text
snapshot ─▶ saneado ─▶ puntuación (scoring.rs) + niveles (levels.rs, Rust)
   ├─ quick:    [técnico][fundamental+noticias+riesgo] (2 hallazgos c/u) ─▶ juez breve ─▶ verificación
   ├─ standard: [técnico][fundamental][sentimiento][macro/riesgo] ─▶ juez ─▶ verificación
   └─ deep:     lo anterior ─▶ [abogado alcista][abogado bajista] ─▶ juez (+ escenarios) ─▶ verificación
```

- **Contexto por etapa** (`context::Sections`): cada etapa solo recibe lo que necesita (técnico = cotización +
  indicadores; fundamental = cotización + fundamentales; sentimiento = noticias; macro/riesgo = macro + indicadores +
  fundamentales; juez y abogados = cotización + puntuaciones + niveles). El contexto completo se sigue construyendo en
  Rust: fija la numeración `[N#]` de noticias y es el índice de verificación de cifras.
- **Niveles en Rust** (`levels.rs`): zona de entrada (soporte más cercano o 1×ATR bajo el precio hasta el precio; con
  «esperar», sobre el soporte), **stop = soporte de referencia − 1×ATR14**, **objetivos = resistencias/niveles al alza
  de los datos** (resistencias, banda superior, máximo 52 s, objetivos de analistas), recompensa/riesgo. Sin soporte o
  sin ATR → `null` y `levelsNote` explica por qué. El modelo ya no propone números de precio (ni están en su esquema);
  el juez recibe el bloque «Niveles calculados por el sistema» y solo puede referirse a él.
- **Tamaño de posición** (`positionSizing`, Rust): riesgo fijo 1 % de la cartera / distancia al stop (o 2×ATR sin stop),
  con tope 3/5/8/10 % según puntuación de riesgo y volatilidad 30d.
- **Escenarios (solo deep)**: el juez da tipo, probabilidad y descripción; Rust normaliza a enteros que suman 100,
  asigna el precio objetivo desde un nivel concreto (alcista = 2.º nivel al alza, base = punto medio soporte/resistencia,
  bajista = 2.º soporte o soporte − ATR), calcula `returnPct` y `expectedReturnPct`.
- **Tesis específica**: el juez rellena tres campos (`thesis_valuation/technical/balance`) que se unen en tres
  párrafos; cada párrafo lleva al final sus fuentes (proveedor y hora). `lint::specificity` exige un mínimo de cifras
  distintas (quick 5, standard 7, deep 9), cifras en cada párrafo y que no haya frases plantilla; si falla, **un**
  reintento con feedback (**quick no reintenta**: nota de calidad y −0,05 de confianza; `QUICK_RETRY_GENERIC` en `mod.rs`); si sigue genérica: nota de calidad y −0,05 de confianza.
- **Castellano**: guía de estilo + términos del glosario y ejemplos BAD/GOOD solo en el prompt del juez (lo que ve el
  usuario). `lint::spanish_issues` (sin diccionario: otros alfabetos, inglés colado, calcos conocidos como «trancos»,
  palabras repetidas) revisa todos los textos del juez; **solo si detecta algo** se hace una llamada de corrección
  (devuelve los mismos textos; se acepta solo si las cifras y citas `[N#]` no cambian y mejora el lint). Sin problemas
  = cero llamadas extra (quick no duplica coste).
- **Citas de noticias**: `Verifier::clean_citations` quita `[N#]` que la noticia no respalda (ninguna cifra suya ni ≥2
  palabras en común): el 7B pega «[N2]» por costumbre.
- **Avisos**: `dataWarnings` = solo carencias reales (`snapshot.missing`, campos vacíos, huecos de la puntuación;
  `gaps.rs`). La nota de método va en `method`; descartes, cifras sin verificar, reintentos… en `qualityNotes`.
- **Progreso**: `AnalysisEngine::analyze_progress` emite `AnalysisEvent::Stage` (ids estables `snapshot`, `technical`,
  `fundamental`, `sentiment`, `macro_risk`, `debate`, `judge`, `verify`; `index` desde 1). quick: snapshot, technical,
  fundamental (etiqueta «Fundamentales, noticias y riesgo»), judge, verify (total 5); standard: 7; deep: 8 (con debate).

## Hecho
- Cliente Ollama, catálogo y `ModelManager`, hardware, chat con tools, scoring, contexto, fidelidad numérica (previo).
- `Analyst::analyze_stream(self: Arc<Self>, snapshot, depth)` en `domain` con implementación por defecto; `OllamaAnalyst`
  lo implementa con el progreso real (se aborta al soltar el stream); `FakeAnalyst` emite 4 etapas simuladas.
- `POST /api/assets/{id}/analysis/stream?depth=` (SSE de `AnalysisEvent`) en `routes/analysis.rs`. POST y SSE comparten
  **un único trabajo por activo** (single-flight + un registro de progreso por activo: el cliente tardío ve la última
  etapa); se guarda en BD igual que el POST, sigue si todos los clientes se van, y se cancela con `state.shutdown`.
  El permiso de `limits.analyses` lo toma solo el líder dentro del trabajo (429 / evento `error` `rate_limited`).
- `DELETE /models/{id}`: solo modelos instalados (404 si no) y nunca el activo (400). Tests en `ai` y `server`.
- Hardware: si `nvidia-smi` falla se lee `/proc/driver/nvidia/gpus/*/information` (nombre; VRAM 0 = desconocida), sin
  romper `GET /models`. Test incluido.
- Tests nuevos: `levels`, `lint`, `gaps`, pipeline (stages, contexto por etapa, reintento genérico, corrección de
  redacción, eventos), `server/tests/analysis_stream.rs`.

## Pendiente / límites conocidos (qwen2.5:7b)
- Las descripciones de escenario del 7B son flojas, pero ya no pueden contradecir su precio (ver «Coherencia de escenarios»). La repetición entre párrafos
  de la tesis se elimina (Jaccard ≥ 0,7 por frase, `lint::dedupe_sentences`).
- El lint de castellano es heurístico: no detecta frases sin sentido ni palabras inexistentes que no estén en la lista.
- El análisis de BTC/cripto y la prueba de chat con tools: ver «Prueba real».
- `analysis.thesis` conserva «Incertidumbre:» (prefijo exacto; el frontend lo extrae). «La conclusión cambiaría si» ya NO
  va en la tesis: solo en `wouldChangeIf`.

## Rediseño de `quick` (sin fusionar etapas)
Problema medido (`model-eval.md`): el `quick` de una llamada combinada + juez completo tardaba ~34 s frente a ~36 s de `standard`
(2,5 llamadas, 6,4k in / 1,4k out), con tesis menos específica. El tiempo lo domina la salida (~1,4k tokens, casi todos del
juez, que escribía lo mismo que en standard) y los reintentos por tesis genérica (otra generación completa del juez).
Diseño (estimado, **pendiente de medir** en la ablación):
- **Dos etapas cortas** con el contexto recortado a lo suyo: técnico (`Sections::technical`) y fundamental+noticias+riesgo
  (fundamentales, noticias, macro y puntuaciones de sentimiento/macro/riesgo; sin el bloque técnico). 2 hallazgos de ≤20
  palabras cada una, tope 300 tokens, esquema `maxItems 2`.
- **Juez breve** (`quick_judge_task` / `quick_judge_schema`, tope 900): veredicto, confianza, horizonte, resumen (≤35
  palabras), tres párrafos de tesis (≤35 palabras, ≥2 cifras cada uno), 1 punto alcista y 1 bajista, 3 riesgos, 2
  «cambiaría si», incertidumbre. No pide `horizon_note`, catalizadores, entrada/salida ni escenarios (salen `None`/vacíos;
  la estrategia solo lleva la línea de tamaño de posición de Rust, y los niveles salen en `levelsNote`). No recibe el
  bloque de niveles. El prompt lleva un ejemplo BAD/GOOD con marcadores `<…>` (no cifras) para evitar la tesis genérica.
- **Sin reintento por tesis genérica** en quick (`QUICK_RETRY_GENERIC = false`).
- Coste estático (fixture, tokens estimados con `tokens.rs`): ver `static_cost_report_and_quick_is_cheaper_than_standard`
  (`cargo test -p ai static_cost -- --nocapture`). Antes: quick 2 llamadas, topes de salida 600+2048; ahora 3 llamadas, 300+300+900.

## Decisiones
- Unidades de memoria: GiB (2^30 bytes) en todos los `*_gb`.
- El tiempo lo domina la **generación** (~38 tok/s), no la entrada: se recorta salida (sin `figures`, `missing`,
  `key_levels`, `position_sizing` en los esquemas; menos hallazgos) y entrada (contexto por etapa).
- Ollama corre con `-np 1`: las etapas «paralelas» se serializan en el servidor; no se gana nada con `LLM_PARALLEL`.
- Los precios de entrada/stop/objetivos/escenarios y el tamaño de posición NO los decide el LLM.
- Etapa `snapshot` la emite el motor (tras bajar el snapshot de mercado, que no genera evento).

## Para el coordinador
- Campos nuevos de `Analysis` (todos `Option` + `serde(default)`, documentados en `docs/API.md`): `levelsNote`,
  `scenarios`, `expectedReturnPct`, `wouldChangeIf`, `horizonNote`, `positionSizing`, `method`, `qualityNotes`.
  **`dataWarnings` cambia de significado**: ya no lleva «Método…», el aviso de no asesoramiento ni descartes; esos
  están en `method` y `qualityNotes`.
- `tests/api.rs` (`analysis_generate_store_and_list`): actualizada la lista de claves de `Analysis`.
- Cableado: nada nuevo en `main.rs`. `routes/mod.rs`: una línea (`/assets/{id}/analysis/stream` en el grupo streaming).
- `AnalysisEvent::Stage.index` empieza en 1 y `total` depende de la profundidad (5/7/8).

## Prueba real (RTX 4070 8 GB, qwen2.5:7b, Ollama compartido con otros agentes: los tiempos tienen ruido)

Servidor real en `127.0.0.1:8798`, BD temporal. «Antes» = pipeline anterior medido hoy; «después» = última pasada.

| Caso | Antes | Después |
|---|---|---|
| AAPL quick | 47 s, 2 llamadas, 7241 in / 1776 out | 39–49 s, 3–4 llamadas (reintento de tesis genérica y/o corrección), 6559 in / 1429 out |
| AAPL standard | 131 s (coordinador: 84 s), 5 llamadas, 17680 in / 2860 out | 43–59 s, 5–6 llamadas, 9250–9833 in / 1569–2150 out |
| AAPL deep | 266 s, 8 llamadas, 31537 in / 5861 out | 65–77 s, 8 llamadas, 14080–14289 in / 2362–2785 out |
| BTC quick | 72 s, 2 llamadas, 6421 in / 1622 out | 47 s, 3 llamadas, 6217 in / 1715 out |
| BTC deep | 178 s, 7 llamadas, 23584 in / 4350 out | 59–66 s, 7–8 llamadas, 11966–12576 in / 2158–2172 out |

Entrada −40 a −55 %, salida −15 a −50 %. Los tiempos quick/standard/deep medidos hoy son ~40–50 s / ~45–60 s / ~65–80 s
(sin contención con otros procesos de Ollama); deep ya no es mucho más lento que standard porque el coste es la
generación, no la entrada. Ahora el análisis guarda `depth`, `durationMs`, `llmCalls`, `promptTokens`,
`completionTokens` para que la UI muestre tiempos reales.

Extracto (AAPL deep): stop 329.01 = SMA20 335.71 − 1×ATR14 (6.6929); objetivos 342.44 / 344.96 / 405 (banda de
Bollinger superior, resistencia, máximo de analistas); recompensa/riesgo 0.6 (honesto: el recorrido es corto);
escenarios 40/40/20 con precios 344.96 / 339.07 (punto medio) / 328.98 (banda inferior). `dataWarnings: []` (el
snapshot estaba completo). Antes: `targets [350.0 redondo]`, `stopLoss null`, avisos tipo «no hay datos de competidores».
Aviso honesto: `riskReward` bajo 1 y «tamaño 39 % limitado a 10 %» son lo que salen de los datos, no un fallo.

Extracto (BTC deep, veredicto `hold`): zona de entrada 81827.38 + 0,5×ATR (esperar sobre el soporte), stop 79769.92 =
banda de Bollinger inferior 81827.38 − 1×ATR14 (2057.45), objetivos 84078.75 (SMA20) / 86330.11 (banda superior) /
87307.83 (resistencia), recompensa/riesgo 0.7; tamaño 8 % (cálculo 32 %, tope 8 % por riesgo 53/100); escenarios
30/40/30 con precios 86330.11 / 82953.06 (punto medio) / 81059.79 (SMA50), `dataWarnings: []`. Defecto visible: la
descripción del escenario bajista habla del soporte 79769.92 y su precio objetivo es la SMA50 (81059.79).

Chat con tools («¿Cómo está el RSI de NVDA comparado con AMD?»): 7 s; llama a `get_indicators` para `stock:NVDA` y
`stock:AMD`, responde con RSI14 49.6 vs 51.9 (cifras correctas); el 7B interpretó mal «sobrecompra» → se añadió al
system prompt la regla 70/30 (no re-medido en chat).

Defectos que quedan con el 7B (medidos): a veces repite ideas entre párrafos de la tesis (se eliminan frases idénticas,
solo parafraseadas casi literales); inventa porcentajes derivados («3.4 %») que se descartan/marcan con ⚠ y bajan la confianza;
descripciones de escenarios flojas; en `quick` la tesis sigue siendo poco específica más veces de lo deseable.

## Coherencia de escenarios y bases de los niveles (Rust)
- Formato para el usuario: `fmt::price_l` (castellano: `81.059,79`, sin punto de millares con 4 cifras) y `fmt::date_l`
  («12 oct 2026»). El prompt del juez sigue con punto decimal (`prompt_notes`), porque la verificación compara texto.
- `levels::Level` lleva `refs` (cifras de las que sale) y `derived` (la base ya incluye sus cifras). Zona de entrada,
  stop y objetivos (`levelsNote`) explican su base con cifras y «(proveedor, fecha)».
- Escenarios: `explain_scenarios` antepone «Precio = SMA50 81.059,79 (Binance, 12 oct 2026).» (derivados: «Precio X =
  punto medio entre el soporte A y la resistencia B (…)»). Del texto del modelo se conservan solo las frases cuyas cifras
  sean el precio, los niveles de ESE escenario, el precio de partida, su variación o su probabilidad; las demás se
  descartan y se anota en `qualityNotes` («Escenario bajista: 1 frase(s) … descartada(s) …»).
- Cifras en formato español del modelo («81.059,79», «2057,45», «-3,2 %»): se leen bien (ver «Fidelidad numérica»); el
  prompt sigue dando las cifras con punto, pero ya no se descarta una frase correcta por escribirlas con coma.
- Tests: `scenario_text_tests` (caso BTC exacto → solo la frase de Rust + nota; coherente → se conserva; una frase
  ajena entre varias → solo esa cae), `levels::level_notes_...`, `fmt::spanish_prices_...`.

Extracto real (BTC deep, qwen2.5:7b, hoy). Antes: «Precio objetivo 82113.96 = soporte (binance, 2026-10-13).» sin el
valor del nivel ni formato español; el caso del defecto (descripción con el soporte 79769.92 y objetivo SMA50) se
reproduce en el test. Después:
```
bearish 30% | Precio = soporte 82.113,96 (binance, 13 oct 2026). Si el MACD histograma se desvía a valores positivos y supera el cero, esto invalidaría el caso alcista y podría llevar a una caída del precio.
base    40% | Precio 83.432,07 = punto medio entre el soporte 82.850 y la resistencia 84.014,13 (binance, 13 oct 2026).
levelsNote  | Stop 80.691,34 = soporte 82.850 − 1×ATR14 2158,66 (binance, 13 oct 2026). Objetivos: 84.014,13 = SMA20; 86.369,77 = banda de Bollinger superior; ... (binance, 13 oct 2026).
qualityNotes| Escenario alcista: 1 frase(s) del modelo descartada(s) por citar cifras que no son las de este escenario (precio 86.369,77).
```
AAPL deep: «Precio = resistencia 344,96 (yahoo, 12 oct 2026).», «Precio 339,25 = punto medio entre el soporte 335,78 y la
resistencia 342,72 (…)»; los tres escenarios tuvieron una frase del 7B descartada (cifras ajenas): con el 7B el
descarte es frecuente. El nombre del proveedor ya sale con su nombre de visualización («Binance», «Yahoo Finance»; ver «Fidelidad numérica»).


## Fidelidad numérica: formatos, signo y unidades (segunda revisión)

Detalle de los hallazgos R-10…R-20 en `review.md → Segunda revisión`.

- **Unidades del contexto** (`context.rs`): `Quote.change_pct` está en PUNTOS porcentuales (1.2 = +1,2 %) y pasa por
  `pct_signed(c / 100)`; los rendimientos, márgenes, ROE, caída máxima, rentabilidad por dividendo y gastos del ETF son
  FRACCIONES (`domain`). Gastos y dividendo se imprimen con 2 decimales (`fmt::pct2`). Si la moneda de las cuentas
  (`extra.financialCurrency`) difiere de la de cotización, ingresos/FCF/caja/deuda llevan su moneda y hay una fila «Monedas».
- **Un único registro de noticias**: el contexto completo, el de cada etapa y el del juez se construyen con el mismo
  `NewsRegistry` y todos alimentan el `NumberIndex`; lo que ve una etapa siempre se puede citar y verificar.
- **Formatos de número** (`fidelity::extract_numbers`): inglés (`81059.79`, `1,234.5`) y español (`81.059,79`, `2057,45`,
  `-3,2 %`, también con espacio fino/no separable). Una cifra con UN separador y exactamente 3 dígitos detrás (`1.234`,
  `1,234`) es ambigua: (1) si el mismo texto trae una cifra inequívoca (`81.059,79` → español; `1,234.5` → inglés) todas
  las ambiguas se leen así; (2) si no, `value` es la lectura inglesa (la del contexto) y la española queda en `alt`; la
  cifra se da por verificada si CUALQUIER lectura está en los datos (`Num::values`). Sufijos `K/M/B/T` (pegados o con
  espacio) y palabras (`mil`, `millones`, `mil millones`, `billones` = 10^12, `billion` = 10^9). El `%` puede ir con
  espacio (`5 %`) o como «por ciento».
- **Signo**: con signo explícito debe coincidir; sin signo, una cifra encaja con un dato negativo solo si la frase habla
  de una bajada (`fidelity::DECLINE_CUES`). «Caída máxima del 13.8%» pasa contra `-13.8%`; «sube un 3.2%» no.
- **Exentos**: enteros ≤ 12 sin unidad, años (salvo con moneda al lado: «2050 USD»), códigos pegados a un entero (`52w`),
  fechas/horas con patrón de fecha, umbrales convencionales junto a su indicador (por palabra, no por subcadena) y los
  nombres de índices (`S&P 500`, `Nasdaq 100`…). Los rangos (`81000-82000`) y pares (`a/b`) NO son fechas.
- **Proveedor en el texto del usuario**: `fmt::provider_name` («binance» → «Binance», «yahoo» → «Yahoo Finance»; mismos
  nombres que `frontend/src/lib/sources.ts`) en bases de niveles/escenarios, citas `(fuente: …)` y avisos de datos.
  `SourceRef.provider` y el contexto del modelo conservan el id.
- **Lint**: «caja libre del» solo es un calco sin «flujo de» delante; «palabra repetida» exige los dos tokens contiguos
  sin puntuación; el inglés colado ignora nombres propios (mayúscula a mitad de frase); `dedupe_sentences` cuenta las
  cifras cortas.
- **Fixture**: `ai::fixture` da `change_pct` en puntos porcentuales, como los proveedores.

## Comparativa de modelos y arreglos asociados
Informe completo (metodología, tablas, extractos, bugs): `model-eval.md`. Ejemplo reproducible con snapshot congelado:
`cargo run --release -p ai --example compare_models -- snapshot.json <modelo> <depth> salida.json` (lee `LLM_*` del entorno,
no el `.env`; la app usa `LLM_NUM_CTX=16384`). Resumen: qwen3:8b a 16384 se reparte 12 % CPU / 88 % GPU (KV-cache 2,6× mayor
por token que qwen2.5:7b) y cuesta 2,1–2,5× el tiempo; el veredicto coincide siempre con la puntuación.
- **Citas `[N#]` inexistentes** (`Verifier::claim`): si, quitada la cita, la afirmación lleva cifras y todas se verifican, se
  conserva sin la cita y `qualityNotes` añade «Se quitaron N cita(s) a noticias inexistentes…»; sin cifras que la respalden, se
  descarta como antes. Antes se perdía la afirmación entera (qwen3 inventa `[N11]`…).
- **Fechas de catalizadores** (`assemble`/`catalyst_date`): la fecha del modelo solo se conserva si está entre las fechas de los
  datos (`dates_in_data`: resultados, ATH, vencimiento, fechas ISO de `fundamentals.extra` como `exDividendDate`, noticias,
  marcas de tiempo, macro); si no se quita y se avisa en `qualityNotes`. Las fechas dentro de la prosa se verifican aparte (ver abajo).
- `analysis::lint` es `pub` (lo usa el ejemplo).

## Etiqueta de la cifra, fechas en la prosa, bullets y lint (TODO 4b, R-38…R-41)
- **Valor Y etiqueta** (`labels.rs`, `fidelity::NumberIndex::review`): cada cifra del contexto lleva la familia de su fila/etiqueta
  (`Rendimientos (1m)`, `RSI14`, `P/E forward`, `Recompensa/riesgo`…); cada cifra de la salida, la de la mención de métrica más cercana
  (izquierda ≤ 6 palabras; si no hay, derecha ≤ 3; sin cruzar `, ; | .` ni otra cifra). Se rechaza solo si hay etiqueta reconocible y TODOS los
  datos con ese valor son de otras métricas conocidas. Sinónimos en `WORDS` (PER = P/E = múltiplo de beneficios; recompensa/riesgo = R/R =
  risk/reward…, es/en). Familias gruesas y palabras débiles/ambiguas: ver cabecera de `labels.rs`. Sin etiqueta reconocible → solo valor, como antes.
  `Finding::describe` explica el rechazo («en los datos es «Rendimientos (1m)», no «recompensa/riesgo»»).
- **Fechas**: `dates.rs`; criterio en R-40. Solo con `NumberIndex::with_dates` (análisis); el chat no las comprueba.
- **Bullets**: `lint::dedupe_bullets` sobre `bullCase`, `bearCase` y `risks` a la vez (Jaccard ≥ 0,7, primera aparición).
- **Lint**: `FINANCE_TERMS` en `lint.rs` = términos que el español financiero usa sin traducir. Criterio: (1) sin equivalente corriente o el inglés es
  el habitual, (2) sustantivo o locución fija, (3) solo como término completo (la locución admite espacio/guion/barra y plural en «s»). Añadir aquí, no en `ENGLISH`.
- Herramienta de medida: `compare_models reverify <snapshot.json> <salida.json>…` (re-verifica salidas guardadas con la verificación actual).

## Cifras derivadas calculadas en Rust (`ai/src/derived.rs`)
- Motivo (`model-eval.md`): el 7B y qwen3 calculaban a mano «recompensa/riesgo 6,7», leían la distancia al máximo como «corrección
  esperada» y un objetivo de analistas por debajo del precio como ganancia. Ahora el contexto lleva la sección **«Cifras derivadas
  (calculadas, no las recalcules)»** (`| Cifra | Valor | Base |`, % con signo y la fórmula en la tercera columna); el system prompt (regla 2)
  remite a ella.
- Filas (cada una solo si hay dato de base; si no, no existe): potencial hasta el objetivo de analistas (medio, máximo, mínimo; no si el
  objetivo está en otra moneda o `analyst_count = 0`); distancia a soportes/resistencias, a la zona de entrada, al stop y a los objetivos, y
  recompensa/riesgo (del plan de `levels.rs` con veredicto «acumular», el mismo que el bloque «Niveles calculados» y el informe); distancia al
  máximo y mínimo de 52 semanas y al máximo histórico (descriptivas, la cabecera dice «NO previsión»); precio frente a SMA20/50/200; ATR14 %,
  volatilidad 30d y volumen relativo (solo si el contexto no trae ya la tabla de indicadores); crecimiento de beneficios implícito
  (P/E trailing / P/E forward − 1) y rentabilidad por FCF (FCF/capitalización) solo con la misma moneda de cuentas y cotización (R-05) y
  fuera de bancos, fondos, cripto y bonos; cripto: oferta circulante / máxima (no si circulante > máxima).
- Por etapa (`Sections::derived`, máscara `derived::*`): técnico = soportes/resistencias + rango + medias; fundamental = analistas + rango +
  valoración; macro/riesgo = rango + técnico; juez = analistas + niveles + rango + valoración; contexto completo (quick, chat) = todo.
  Con el presupuesto reducido (`!lv.extras`) solo las filas «core».
- Coste medido (estimador de `tokens.rs`, ~4 caracteres/token), AAPL / BTC del fixture: completo +317 / +277; juez +200 / +160; técnico
  +183 / +219; fundamental +140 / +102; macro/riesgo +93 / +128. Test `the_section_adds_few_tokens_per_stage` fija el tope.
- Verificación (R-38): `Segment::Derived`; palabras nuevas en `labels.rs`: «potencial»/«upside» (admiten nivel, analistas, PER, BPA y
  crecimiento, NO rentabilidades pasadas) y «FCF yield». «Potencial del −3,2 % hasta el objetivo» se verifica; «recompensa/riesgo 6,7» sigue
  cayendo cuando el dato es otro.
- Límites: sin medir con el modelo real (no se lanzó Ollama); la sección es nueva para el LLM y puede cambiar cómo cita cifras. Las filas
  repiten algunos números del plan (más tokens, no más datos). `analysis::levels` pasa a `pub(crate)` para que `derived` use el plan.
