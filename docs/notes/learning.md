# Notas del crate `learning`

Estado: **implementado y probado con datos sintéticos; sin datos reales con los que aprender todavía** (la BD de la app tiene
0 análisis el 15-oct-2026). El LLM NO se reentrena: el aprendizaje ocurre en cuatro capas medibles y con salvaguardas.

| Capa | Qué hace | Se aplica solo si… |
|---|---|---|
| 1. Resultados | rentabilidad a 1/4/12 semanas, exceso sobre índice, acierto del veredicto | siempre (es medición) |
| 2. Calibración | curva de fiabilidad + calibrador isotónico de la confianza | ≥ 200 análisis independientes por horizonte y mejora el Brier fuera de muestra |
| 3. Pesos | 7 multiplicadores sobre los pesos de `ai::scoring` | superan la puerta de validación posterior; decaen si la evidencia se agota |
| 4. Lecciones RAG | (otro crate, `knowledge`) | — |

Código: `backend/crates/learning` (`rules`, `resolve`, `calibration`, `weights`, `report`, `stats`, `store`). Migración
**`0005_learning.sql`** (tablas `analysis_outcomes`, `scoring_weights`, `learning_runs`; la 0004 es del RAG) y **`0009_learning_prev.sql`**
(archivo de cifras, vía de reserva y `rule_version` de los pesos; ver «Versiones de la regla»).

## 1. Resultados: reglas A PRIORI (versión 2, `rules::RULE_VERSION`)

Fijadas antes de ver ningún resultado. Cambiarlas exige subir `RULE_VERSION`, y **subirla re-etiqueta el histórico** con la regla
nueva (ver «Versiones de la regla», abajo) **sin perder las cifras anteriores**. Las estadísticas, la calibración y los pesos leen
solo filas de la versión vigente, más las de la vía de reserva explícita (abajo): nunca se mezclan versiones por descuido.

- **Horizontes**: 1, 4 y 12 semanas (7·h días) contadas desde el día de la **entrada** (no desde `generated_at`).
- **Precios**: cierres diarios de `MarketData::history(…, D1)`. **Salida** = primer cierre con día ≥ entrada + 7·h días. La vela
  de salida debe estar **completa** (`t + 24 h ≤ ahora`): la vela en curso (parcial) nunca se usa. Huecos > 5 días → «sin datos».
  Solo se usan velas, nunca `price_at_analysis` (ver el motivo en la entrada). Raw/ajustado: los cierres son los del proveedor
  (Yahoo, sin ajustar por dividendos); el sesgo afecta igual al índice.
- **Entrada (regla v2, `rules::compute`)** = cierre de la **última sesión que ya había abierto cuando el análisis vio los datos**.
  Ese instante es `generated_at − durationMs` (`generated_at` se fija al ACABAR de generar y los datos se leyeron al empezar; con
  un LLM local son minutos y, si cruzan la apertura, la sesión que el análisis no vio parecería la última; sin `durationMs`, o con
  más de 3 h, se usa `generated_at`). El resolutor lee ese dato del JSON de `analyses` (`resolve::analysis_meta`).

  | El análisis vio los datos… | Entrada |
  |---|---|
  | martes 07:15 UTC (antes de abrir EE. UU.: el pase de las 09:15 de Madrid) | cierre del **lunes** |
  | sábado, domingo o festivo | cierre del viernes (o del último día con sesión) |
  | miércoles 15:00 UTC (mercado abierto) | cierre de **ese miércoles** |
  | miércoles 21:30 UTC (tras el cierre) | cierre de ese miércoles |
  | cripto, cualquier hora del martes | cierre del **martes** (día UTC) |

  Criterio de inversor: el análisis decide con el precio que vio, y la rentabilidad se mide desde ahí hacia delante, sin acreditarle
  un movimiento que ya conocía ni descartar uno que no podía conocer.
  - *Mercado cerrado* (antes de abrir, fin de semana, festivo, tras el cierre): el precio que vio es el último cierre; la orden se
    ejecutaría en la apertura siguiente y el hueco nocturno cuenta como resultado. → último cierre.
  - *Mercado abierto*: vio un precio intradía que las velas diarias no tienen. El cierre **anterior** (la lectura literal de «último
    cierre disponible») incluiría el tramo cierre-anterior → momento del análisis, que el análisis ya conocía: le acreditaría un
    movimiento ya ocurrido (hasta ~1 de las 5 sesiones de la ventana de 1 semana) y, en el aprendizaje de pesos, haría que el
    momentum «predijera» un movimiento que forma parte de su propia señal. El cierre de **esa** sesión es el primer precio
    posterior que existe en las velas; descarta lo que quedaba de sesión (hasta 6,5 h), lo cual no sesga, solo resta señal.
  - *`price_at_analysis`* (el precio exacto que vio) se descartó: (i) el benchmark necesitaría el precio de SPY/BTC en el mismo
    instante y solo hay velas diarias, así que el exceso mezclaría instantes distintos; (ii) la cotización puede ir con retraso y sin
    ajustar por splits, mientras que el histórico de la salida sí viene ajustado; (iii) falta en algunos análisis. Haría falta pedir velas intradía
    del activo y del índice por cada análisis (más peticiones; no hecho).
  - *Cripto* (24/7): no hay «mercado cerrado»; cada día UTC es una sesión que abre a las 00:00 UTC y la entrada es el cierre del día
    UTC del análisis (igual que en la v1). **No es el precio del momento**, que las velas diarias no tienen: es el primer cierre
    posterior, 0-24 h después. El cierre del día anterior metería hasta 24 h de movimiento ya visto (7 h en el pase de las 07:15 UTC).
  - *Apertura de EE. UU.*: 9:30 de Nueva York, calculada con la regla de horario de verano de 2007 (2.º domingo de marzo-1.er domingo
    de noviembre): 13:30 UTC en verano, 14:30 UTC en invierno (`rules::session_open_s`, comprobada contra la base de zonas horarias
    en el test `session_open_matches_the_new_york_time_zone_database`). No hay calendario de festivos: la sesión se sabe por la
    vela (el día UTC de `t`; Yahoo la fecha en la apertura y Alpha Vantage a las 00:00 UTC del día, y ambas dan el mismo día), y un
    festivo no tiene vela. Las sesiones cortas abren igual. Un hueco > 5 días entre la última sesión abierta y el instante → «sin datos».
  - El benchmark entra en el mismo día de vela que el activo.
- **Benchmark** (`rules::benchmark_for`):
  - acción/ETF/fondo de EE. UU. (símbolo sin sufijo de bolsa extranjera tipo `.MC`, `.L`…) → **SPY** (exceso = activo − SPY);
  - **SPY**, `^GSPC/^IXIC/^DJI/^RUT/^NDX` y **BTC** son su propio índice → `bench_kind = absolute`: exceso = rentabilidad
    absoluta (índice = efectivo). Se marcan y **no entran** en el aprendizaje de pesos (otro objetivo);
  - cripto → **BTC**;
  - divisas, materias primas, bonos, futuros, opciones y valores extranjeros → **sin benchmark: no se resuelven** (`skipped`
    con nota). No se inventa un índice regional.
- **Acierto** (`rules::is_hit`), con `e` = exceso simple (no anualizado):
  comprar/acumular acierta si `e > 0`; reducir/evitar si `e < 0`; mantener/esperar si `|e| < 0,02·√semanas` (2 % a 1 s,
  4 % a 4 s, 6,9 % a 12 s: del orden de medio σ del exceso de una acción grande frente a SPY).
  **Asimetría a tener presente**: comprar/evitar aciertan ~50 % «por azar»; mantener/esperar aciertan ~35–45 % por azar con esa
  banda. Los aciertos solo se leen junto a la **tasa base** (`baseRateOutperform`) y por veredicto, nunca mezclados sin más.
- **Idempotente / reanudable / barato** (`resolve::resolve_pending`): nada que haya madurado → 0 peticiones. Una petición de
  histórico por activo distinto (y por benchmark; si un activo también es benchmark de otro, una sola), con el rango mínimo que
  cubre el análisis pendiente más antiguo. `max_assets_per_run` (25) acota la pasada; lo que falta sigue en la siguiente.
  `resolved`/`skipped` son definitivas (`ON CONFLICT … WHERE status='retry'`); los fallos transitorios quedan `retry` con
  retroceso 6 h·2^n (tope 7 d) y pasan a `skipped` tras 6 intentos; `NotFound` → `skipped` directo.
- **Sin clave foránea a `analyses`** a propósito: si el usuario borra un análisis, su resultado se conserva (si no, borrar los
  fallos sesgaría el historial: sesgo de supervivencia). Test `deleting_an_analysis_keeps_its_outcome`.
- Cada fila guarda los 7 componentes de la puntuación (riesgo invertido) para poder reentrenar sin releer el JSON.

### Versiones de la regla

| Versión | Entrada | Motivo |
|---|---|---|
| 1 | cierre de la sesión del día UTC de `generated_at`, o de la siguiente | diseño original, pensado para análisis hechos de día o tras el cierre |
| 2 (23-oct-2026) | cierre de la última sesión ya abierta cuando el análisis vio los datos | la lista de seguimiento pasó a hacerse en una ventana (de madrugada o por la mañana de D+1): con la v1, un análisis que vio el cierre de D se medía desde el cierre de D+1. Se cambió sin ningún resultado resuelto en la BD, no por rendimiento |

**Qué pasa al subir la versión** (`resolve::reopen_stale`, al principio de cada pasada de `resolve_pending`). Regla de oro: **las
cifras de una versión anterior no se anulan hasta que la nueva se ha calculado bien, y nunca se pierden.**
- Las filas `resolved` y `skipped` de una versión anterior pasan a `retry` (`note = 'recalculando con la regla vN'`, intentos a 0) y se
  **recalculan** con el mismo tope de activos por pasada. Solo cambian `status`, `retry_after`, `attempts` y `note`: las cifras
  (precios, rentabilidades, `hit`, factores) y la `rule_version` de la fila **siguen donde estaban**, así que si el proceso muere a medias
  no se pierde nada. `ResolveReport.reopened` dice cuántas filas se reabrieron. Se recalculan también las filas de análisis ya
  borrados (`analysis_outcomes` no tiene clave foránea): sin JSON que releer, la fila se queda con los factores guardados y
  `generated_at` como instante de referencia.
- **Recálculo correcto → archivo + sustitución en UNA transacción** (`resolve_pending`): `INSERT OR IGNORE INTO
  analysis_outcomes_archive … SELECT` de la fila antigua y, en la misma transacción, el `UPSERT` de la nueva (`rule_version` vigente,
  `fallback_for = NULL`). El archivo (`analysis_outcomes_archive`, clave `(analysis_id, horizon_weeks, rule_version)`) guarda las
  cifras con su versión, la que las sustituyó (`superseded_by`) y cuándo (`archived_at`): la cadena v1 → v2 → v3 queda completa y
  auditable con un `SELECT`.
  *Por qué una tabla de archivo y no columnas `prev_*`*: las `prev_*` solo guardan una generación (en la v3 pisarían la v1), exigen
  una docena de columnas en la tabla caliente y las métricas tendrían que elegir entre columnas; el archivo es un solo `INSERT … SELECT`,
  guarda todas las generaciones y no toca ninguna consulta de métricas.
- **Fallo transitorio** (red, límites, proveedor caído, benchmark no disponible): la fila sigue `retry` con sus cifras anteriores
  intactas y se reintenta con el retroceso de siempre (6 h·2^n, tope 7 d). **A diferencia de una fila nueva, nunca agota los intentos**:
  una caída larga del proveedor no la pasa a `skipped` ni a la reserva, porque no prueba que el histórico no exista. Mientras
  reintenta no cuenta en las métricas (queda como «pendiente»: muestra parcial pero homogénea) y no pierde nada. No se tocan su
  `rule_version` ni `resolved_at` (siguen siendo los de las cifras que conserva).
- **Fallo PERMANENTE → vía de reserva** (`resolve::write_fallback`). Es permanente: `NotFound` del histórico del activo, activo sin
  benchmark definido, o datos que no sirven (`NoData`: el histórico acaba antes de la fecha de salida, como un activo deslistado)
  tras `max_attempts` intentos. Entonces la fila vuelve a `resolved` **con las cifras de la versión anterior** (su `rule_version` no
  cambia, es la de esas cifras), `fallback_for = RULE_VERSION` y la nota «entrada de la regla anterior (vN), sin histórico para
  recalcular: …». Una sola sentencia, atómica. Sin esto, un activo deslistado cuya pérdida estaba resuelta en la v1 desaparecería en
  la v2 (`skipped`, fuera de las métricas): justo el sesgo de supervivencia que la migración 0005 quería evitar.
  - Las métricas la **cuentan**: `store::resolved_rows` y los contadores del panel filtran `rule_version = vigente OR fallback_for =
    vigente`. Con eso entran también en calibración y pesos (con la entrada de la regla anterior; es la contrapartida asumida frente a
    perder los fallos).
  - El informe lo dice: `HorizonReport.fallbackRows` (de los `resolved`, cuántos usan la reserva), un `caveat` con el total y
    `ResolveReport.fallback` (filas que esta pasada ha dejado en la reserva).
  - Es **definitiva para esa versión**: no se reintenta cada pasada (costaría una petición por activo y pasada). La siguiente subida
    de `RULE_VERSION` la reabre (`fallback_for < vigente`) y vuelve a probar. Si Yahoo vuelve a servir un activo y se quiere recuperar
    antes, hay que poner `status = 'retry'` y `fallback_for = NULL` a mano en esa fila.
  - Una fila `skipped` de la versión anterior no tiene cifras que conservar: si no se puede calcular, vuelve a `skipped` con la versión
    vigente, como antes.
- Idempotente: lo reabierto ya es `retry`, y lo que está en la reserva para esta versión no se reabre (test
  `a_permanent_not_found_keeps_the_old_figures…`, segunda pasada: 0 reabiertas, 0 peticiones).
- Nunca se mezclan por descuido: una fila de versión anterior sin marca de reserva (p. ej. la que espera su recálculo) no entra en
  ninguna métrica y cuenta como «pendiente» (antes de la v2, las filas de una versión anterior no se recalculaban nunca, quedaban fuera
  de toda métrica y hacían crecer `pending` para siempre).
- **Cuidado con el uso**: re-etiquetar el histórico permite, en teoría, ajustar la regla hasta que las cifras salgan bien. Por eso
  cada versión se anota aquí con su motivo, y el motivo no puede ser el rendimiento.
- **Pesos aprendidos y versión de la regla** (`weights::retire_stale`, desde `activate` y desde cada `run_weights_cycle`): cada versión
  de `scoring_weights` guarda la `rule_version` con la que se ajustó (columna `rule_version`; la API la expone como `ruleVersion`). Una
  versión **activa** de otra regla se retira (`status = 'retired'`, nota «desactivada: ajustada con la regla vN, vigente vM»; nunca se
  borra) y rigen los pesos por defecto; queda una entrada `decision = 'deactivate'` en `learning_runs` (el registro de decisiones) con el
  motivo. Pasa al arrancar (`activate`) y en el ciclo periódico, antes de que la puerta tome el incumbente; después, si hay datos, la
  puerta puede adoptar una versión nueva con la regla vigente. Es idempotente (una sola entrada). `rollback` crea la versión con la
  regla vigente: es una decisión explícita (queda su entrada `rollback`), no una adopción silenciosa. Las filas anteriores a la 0009 se
  dan por ajustadas con la v1 (`DEFAULT 1`: las escribió un binario anterior a la 0008, con `RULE_VERSION` 1; la BD real no tiene ninguna), así que una activa se retira al arrancar.
- **Pendiente conocido**: `run_weights_cycle` toma `last_eval_rows` del máximo de `learning_runs.n_rows` de todas las versiones, así que
  tras una subida la puerta espera a superar el recuento de la regla antigua + `min_new_rows`. Retrasa, no corrompe; arreglarlo pide
  guardar la `rule_version` también en `learning_runs`.

## 2. Calibración (`calibration`)

- **Curva de fiabilidad**: 10 tramos de 0,1 de confianza declarada → acierto real (Wilson 95 %) y nº por tramo.
- **Calibrador**: PAV isotónico, con cada bloque encogido hacia la tasa base con 20 pseudo-observaciones (evita escalones
  extremos), PAV de nuevo e interpolación lineal; salida acotada a [0,05; 0,95].
- **Solo se aplica** con ≥ `min_n` = 200 análisis **independientes** (una fila por activo y ventana de h semanas) *y* si, ajustado
  con el 70 % más antiguo, reduce el Brier del 30 % más reciente en ≥ 0,002. Si no: se informa (curva, Brier, motivo) y no se
  corrige. Se calibra solo con el modelo activo (otro modelo = otra escala de confianza).
- No se persiste: es función pura de `analysis_outcomes` (`calibrated_confidence` lo calcula al vuelo).

## 3. Pesos aprendidos con puerta (`weights`)

**Qué se aprende**: 7 *multiplicadores* (técnico, momentum, valoración, fundamentales, sentimiento, macro, riesgo) que se
multiplican por los pesos por clase de `ai::scoring::default_weights`. Así se conserva la estructura por clase (cripto sin
fundamentales) y solo se estiman 7 números con una muestra agrupada. Sin versión activa los multiplicadores son 1 y
`scoring.rs` se comporta exactamente igual que antes.

**Entrenamiento** (`weights::fit`): ridge ponderado por recencia (vida media 26 semanas) del exceso de rentabilidad a 4 semanas
(recortado a mediana ± 4σ robusta) sobre los componentes estandarizados (los ausentes valen la media). El ridge no encoge
hacia 0 sino **hacia la dirección de los pesos vigentes** (`α = 100` observaciones): con poca evidencia el resultado se queda en
los pesos actuales. Después: coeficientes negativos → 0 (los pesos no pueden ser negativos), reparto, multiplicador acotado a
[0,25; 3] y normalizado (por la normalización puede pasar de 3 ligeramente).

**Puerta** (`weights::evaluate_gate`), en orden:
1. Una fila por activo y ventana de 4 semanas (solapes = misma información).
2. Mínimos: entrenamiento ≥ 150, validación ≥ 80 y ≥ 8 bloques temporales de 4 semanas en validación.
3. **Validación posterior**: el 30 % final (mín. 80) valida; el resto entrena con **purga** (se descartan las filas cuyo horizonte
   invade la validación, porque comparten movimientos de mercado). Tras una adopción, la siguiente validación debe empezar
   después de la validación de la adopción anterior (datos nuevos).
4. Métrica: IC = correlación de rangos de Spearman entre la puntuación global (con unos u otros pesos) y el exceso.
5. Candidatos: el entrenado y «volver a los pesos por defecto» (si el vigente no es el de por defecto). Se adopta el mejor
   que cumpla **todo**: ΔIC ≥ 0,03 sobre el vigente, IC propio > 0, y p-valor unilateral ≤ α_eff/nº de candidatos, con
   **bootstrap por bloques temporales** de ΔIC (500 réplicas, pareado, semilla fija).
6. **Gasto de alfa**: α_eff = 0,05·0,61/(1+k)² con k = evaluaciones desde el último cambio (la suma sobre todos los intentos
   es ≤ 0,05). Y no se vuelve a evaluar hasta tener 40 filas independientes nuevas. Cada evaluación queda en `learning_runs`.
7. Se adopta el candidato **tal como se validó** (no se reentrena con la validación: se rompería la garantía). La siguiente
   vuelta ya entrena con esos datos.

**Retención / decaimiento** (`weights::retention_check`): adoptar exige pruebas; *mantener* también, aunque más laxas. Con
datos posteriores a la validación de la adopción (≥ 80 filas y ≥ 8 bloques), si los pesos aprendidos no superan a los de por
defecto (p > 0,25 o IC menor), se encogen hacia 1 (raíz cuadrada del multiplicador; si todo queda a < 0,05 de 1, a los de por
defecto). Es lo que hace que un factor que dejó de funcionar pierda peso con el tiempo aunque ningún candidato nuevo «gane».

**Versionado** (`scoring_weights`): nunca se borran filas; una sola activa (índice único parcial). Tipos `learned`,
`rollback`, `decay`, `manual`, cada una con la `rule_version` con la que se ajustó (ver «Versiones de la regla»: una activa de otra
regla se retira con una entrada `deactivate` en `learning_runs`). `weights::rollback(db, Some(v) | None, now)` crea una versión nueva con
los multiplicadores de `v` (o los de por defecto) y la activa. `weights::activate(db)` instala la vigente en `ai::scoring` (llamar al arrancar y tras
cada cambio; `run_cycle` ya lo hace tras adoptar). Cada versión guarda fechas de entrenamiento/validación, n y todas las
métricas de la puerta (JSON).

**Cambio en `ai/src/scoring.rs`** (mínimo): `weights` pasa a `pub fn default_weights` (mismo cuerpo); nuevo `weights` que
multiplica por `static LEARNED_MULTIPLIERS: RwLock<Option<[f64;7]>>` (`set_learned_multipliers`, `learned_multipliers`; se
ignoran valores no finitos o fuera de (0,10]); nueva `pub fn combine(clase, componentes, multiplicadores)` (la misma media
ponderada que `score`, para reevaluar puntuaciones guardadas) con test `combine_matches_score_overall_for_default_weights`.
Ningún otro fichero de `ai` cambia. Es estado global del proceso: es la única forma de no tocar `analysis/**`.

## 4. Panel de rendimiento (datos)

`learning::performance_report(db, now, modelo_activo, &CalibrationConfig)` → `PerformanceReport` (serde camelCase): por horizonte
(1/4/12) nº de análisis, resueltos, **resueltos independientes**, pendientes (maduros sin resultado), sin madurar, descartados;
tasa base de superar al índice; aciertos y exceso medio por veredicto con IC (acierto: Wilson sobre la muestra independiente;
exceso medio: bootstrap por bloques temporales, `null` con < 5 bloques); curva de fiabilidad y estado de la calibración;
versión de pesos vigente e historial; y una lista de `caveats`. `resolved` incluye `fallbackRows`, las filas que usan la entrada de la regla
anterior porque su activo ya no tiene histórico para recalcularlas (y hay un `caveat` con el total).

## 5. Pruebas sintéticas (sin red)

Generador: 7 factores N(50, 15) independientes (recortados a [0,100]), 12 análisis por semana sobre 60 activos, exceso =
0,07·(ρ·z_señal + √(1−ρ²)·ε), 156 semanas (1872 filas, todas independientes tras el filtro: cada activo reaparece cada 5 semanas). Ejecutar los Montecarlo:
`cargo test -p learning --release -- --ignored --nocapture` (≈ 2 min); los tests por defecto usan pocas semillas.

| Escenario | Resultado |
|---|---|
| (a) factor momentum con ρ = 0,30, 40 semillas | adopta **39/40**; multiplicador medio del momentum **4,24** (mayor de los demás 1,12); IC de validación +0,099 → +0,268 |
| (a) ρ = 0,15 | adopta **25/40**; multiplicador 3,74 (otros ≤ 1,55); IC +0,037 → +0,133 |
| (a) ρ = 0,08 | adopta **9/40**; multiplicador 3,47; IC −0,002 → +0,078 (los que adopta lo hacen sobrestimando: maldición del ganador) |
| (b) ρ = 0,40 pero ~100 filas, 50 semillas | **0/50** adopciones (motivo: «muestra insuficiente») |
| (b2) solo ruido (ρ = 0), 200 semillas, 1872 filas | adopción falsa **6/200 (3 %)** en un intento; **7/200 (3,5 %)** en una secuencia de intentos cada 8 semanas (nominal 5 %) |
| (c) ρ = 0,20 hasta la semana 103, ruido después; 20 semillas, intentos cada 4 semanas | multiplicador del momentum en la semana 110: **2,92**; al final (semana 262): **1,00**. Trayectoria de una semilla: 1,00 (s.90) → 2,80 (s.98) → 1,67 (s.122) → 1,29 (s.202) → 1,00 |

Lecturas honestas: la potencia con ρ = 0,15 (IC ≈ 0,15, ya optimista para un factor real) es 63 % con 1872 observaciones
independientes; con ρ < 0,10 casi nunca se detecta, y esa es la situación realista. El decaimiento tarda: sin datos nuevos
suficientes (≥ 8 bloques de 4 semanas ≈ 30 semanas) la puerta no se mueve. Un primer intento previo al cambio de régimen, con
α_eff = 0,030, ya es más exigente que el nominal 0,05.

El humo con red real (`cargo test -p learning --test live -- --ignored --nocapture`, 3 peticiones) comprobó AAPL/SPY/BTC a
1 y 4 semanas con velas reales (la última vela de AAPL era la sesión en curso y no se usó).

## Lista de seguimiento (análisis diarios) y cableado en `server`

**Por qué existe**: con el uso normal (unos pocos análisis a la semana) el historial tardaría años. La app analiza sola, cada día,
una lista fija de activos y `learning` los resuelve a 1, 4 y 12 semanas. Código: `server/src/watchlist.rs`, tablas
`watchlist_runs` / `watchlist_results` (migración `0006_watchlist.sql`), `analyses.origin = 'watchlist'`.

**Lista por defecto (20 activos), decidida A PRIORI** (antes de ver ningún resultado; `watchlist::DEFAULT_WATCHLIST`):

| Grupo | Activos | Motivo |
|---|---|---|
| Grandes de EE. UU., 8 sectores (12) | AAPL, MSFT, NVDA (tecnología), AMZN, TSLA (consumo discrecional), JPM (finanzas), XOM (energía), JNJ (salud), KO, WMT (consumo básico), CAT (industriales), NEE (utilities) | Liquidez y datos limpios; varios sectores para que el aprendizaje de pesos no dependa de uno; benchmark SPY bien definido |
| ADR europeos (2) | ASML, SAP | Exposición a Europa sin sufijo de bolsa: `rules::benchmark_for` NO resuelve `.MC`, `.L`, `.DE`… y esos análisis se descartarían |
| ETF (4) | QQQ, IWM, GLD, TLT | Otras clases de activo: crecimiento, pequeñas empresas, oro y bonos largos. Ojo: su exceso es frente a SPY, así que GLD/TLT miden sobre todo rotación entre activos |
| Cripto (2) | ETH, SOL | Benchmark BTC; volatilidad muy distinta |

Excluidos a propósito: **SPY** y **BTC** (son su propio índice: `bench_kind = absolute`, no entran en el aprendizaje de pesos) y
cualquier valor con sufijo de bolsa extranjera. Limitaciones asumidas: 20 activos de gran capitalización y muy correlacionados
entre sí (un solo régimen de mercado), sesgo hacia EE. UU. y hacia nombres conocidos para el LLM (que los «conoce» de su
entrenamiento: puede haber filtración de información posterior a su fecha de corte en los análisis). **Cambiar la lista según los
resultados invalida la medición** (sesgo de selección y de comparaciones múltiples): si hay que cambiarla, se anota la fecha y se
mide por separado. `WATCHLIST=` en `.env` la sustituye; `/watchlist` indica si es la de por defecto (`defaultList`).

**Ventana de la pasada** (rediseño del 23-oct-2026; código en `server/src/watchlist/window.rs`). El PC del usuario no está
encendido a una hora fija (tras la recuperación fallida del 20-oct a las 23:32 no se ejecutó ninguna pasada más: «una pasada a las 23:00» no llegaba a ocurrir), así que no hay una hora
de la pasada sino una **ventana**: la pasada del **día de bolsa D** puede hacerse en cualquier momento entre el **cierre de EE. UU. de D**
y la **apertura siguiente** (D+1, o el lunes si D es viernes). En ese intervalo el análisis ve el cierre de D, que es lo que se mide.

| | |
|---|---|
| Cierre / apertura | 16:00 / 9:30 **de Nueva York**, no «22:00 / 15:30 de Madrid»: los cambios de hora de EE. UU. (2.º domingo de marzo, 1.er domingo de noviembre) y de Europa (últimos domingos de marzo y octubre) no coinciden. Entre medias (2-3 semanas de marzo, 1 de octubre-noviembre; en 2026: 8-29 mar y 25 oct-1 nov) la diferencia es de 5 h y el mercado cierra a las **21:00** y abre a las **14:30** de Madrid. La hora de Nueva York se calcula con la regla de EE. UU. de 2007 (sin base de zonas horarias) |
| Margen | La pasada no empieza hasta **30 min después del cierre** (la vela diaria tarda en consolidarse) |
| `WATCHLIST_TIME` | Hora local **preferida** dentro de la ventana: la primera vez que el reloj la marca a partir del cierre + margen (23:00 → esa noche; 09:15 → la mañana siguiente, o el sábado si D es viernes). Si cae en plena sesión (p. ej. 16:00) se ajusta al cierre + margen **con un aviso** en el log. Un mismo valor puede valer una semana y otra no (21:30 de Madrid vale en las semanas desfasadas y no en las normales) |
| Con la hora preferida ya pasada | Si el servidor arranca (o el equipo despierta) con la ventana abierta y la preferida ya pasó, hace la pasada **de inmediato** (`trigger = recovery`; se guardan la hora prevista, `scheduledFor`, y la real, `startedAt`). El PC encendido a las 09:00 de D+1 hace la pasada de D |
| Fin de semana | La pasada del **viernes** vale hasta la apertura del **lunes** (sábado y domingo incluidos) |
| Apertura sin pasada | Si llega la apertura de EE. UU. y a algún activo le falta su análisis, **esa pasada se pierde**: se registra una pasada con estado **`missed`** (un resultado `skipped` permanente por activo, con `error_code = missed` y el último error si lo hubo) y **no se analiza con el mercado abierto**. Es idempotente (un reinicio durante la sesión no la vuelve a registrar). Al arrancar y en cada vuelta se registran todas las ventanas ya cerradas con pendientes desde la última pasada real (hasta 10 días de bolsa): un día entero sin servidor también deja su `missed`. Qué cuenta para una ventana lo decide la hora de inicio del resultado, no `local_date`. Una pasada en curso se **corta a la apertura**: no se empieza ningún análisis más (queda `aborted`) y lo pendiente se registra como `missed` |
| `local_date` | En `watchlist_runs` y `watchlist_results` es el **día de bolsa D**, no el día de ejecución (una pasada hecha el jueves a las 09:00 con el cierre del miércoles lleva `2026-10-14`) |
| Festivos de EE. UU. | **No hay calendario**: un festivo se trata como un día laborable más. La ventana de la víspera acaba a las 9:30 del festivo y la «pasada» del festivo analiza el último cierre real (se repite el mismo cierre). Las sesiones cortas (13:00) cierran antes de las 16:00, así que el margen es conservador. Un calendario de festivos es trabajo pendiente |
| Cripto | Opera 24/7: su precio es **el del momento de la pasada** (puede ser horas después del cierre de la bolsa), no el de las 16:00 de Nueva York. Para cripto la entrada medida por `learning` (cierre diario UTC del día del análisis) ya era una aproximación |

**Programador** (tarea Tokio cancelable con `state.shutdown`):
1. Calcula la fase del reloj: ventana de D abierta (cierre ≤ ahora < apertura siguiente) o mercado abierto. Con el mercado abierto,
   registra como `missed` lo que quedó pendiente de la ventana que acaba de cerrarse y espera al próximo cierre. Con la ventana abierta y
   activos pendientes, espera a la hora preferida (si no ha llegado), la batería (ver 5) y empieza.
2. **Recuperación**: es el caso «la hora preferida ya pasó» de la tabla. Una pasada que quedó `running` por un apagado o caída se marca
   `interrupted` al arrancar y sus activos sin `ok` se rehacen si la ventana sigue abierta.
3. **De uno en uno** y por el mismo camino que la API (`routes::analysis::run_to_completion`: single-flight, límite de generaciones,
   guardado en BD), con `WATCHLIST_DEPTH` y el modelo activo. Si ya había un análisis del activo en curso por la API, se suma a él.
4. **Comprobación previa y fallos.** Antes de analizar, cada ronda comprueba que **Ollama responde y que el modelo activo (el de la UI de
   Modelos, que puede no ser el de `LLM_MODEL`) está instalado** (`watchlist::llm_ready`, con `ModelService::list`). Si no, la ronda NO
   empieza: queda `aborted`, sus activos `skipped` transitorios (`error_code = llm_unavailable` y el motivo, visible en la vista de
   rendimiento) y se reintenta más tarde con las rondas de siempre (3 más cada 30 min mientras la ventana lo permita; una espera que
   cruzaría la apertura no se hace). Un activo que falla no detiene la lista y queda registrado con código y mensaje. Son **transitorios**
   (Ollama caído, **modelo no encontrado**, timeout del LLM, límites, red): se reintentan 3 veces con espera de 1 y 5 min y, con 2 activos
   seguidos agotados, se aborta la pasada (el resto queda `skipped`, visible). Son **permanentes** solo los fallos del propio activo
   (activo inexistente, entrada no válida). `NotFound` es ambiguo, así que se distingue el del modelo por su texto («modelo «X»: model 'X'
   not found», que llega con el prefijo «no encontrado: » repetido por capa) y, si un `NotFound` no lo parece, se contrasta con el estado
   real de Ollama antes de darlo por permanente (`is_transient` / `analyze_with_retries`). No se añadió un código de error nuevo: cambiaría el
   contrato de la API. *Motivo (20-oct-2026)*: Ollama arrancó sin los modelos, `not_found` se trató como permanente y la primera pasada real
   marcó los 20 activos como fallo definitivo de ese día sin reintento ni aborto.
   **Datos antiguos**: los 20 resultados del 20-oct se guardaron con `transient = 0`. No se reescriben (el histórico queda tal cual), pero
   `pending_assets` los **reinterpreta al leer**: si el último resultado de un activo ese día es un `failed` permanente cuyo texto es el de
   un modelo ausente, el activo vuelve a estar pendiente. Así, reiniciar el servicio con el arreglo el mismo día de bolsa (mientras su
   ventana siga abierta) los reanaliza; los `NotFound` de verdad de un activo no coinciden con ese texto y siguen cerrados. Solo cuenta
   mientras la ventana de ese día esté abierta: pasada la apertura siguiente queda `missed`.
5. **Batería**: si `/sys/class/power_supply` indica que el equipo va con batería (fuentes `Mains`/`USB*` desconectadas), la pasada
   espera a que se enchufe, con tope `WATCHLIST_BATTERY_MAX_DELAY_HOURS` (3 h; 0 = no esperar) y, además, nunca más allá de la apertura.
   Con la GPU limitada a ~30 W cada análisis tarda el doble. Se anota en `watchlist_runs.power` (`ac`/`battery`/`unknown`) y
   `power_wait_secs`. Sin dato de energía no espera.
6. Con `INVEST_AI_FAKE=1` la lista y el ciclo de aprendizaje se desactivan (datos simulados no pueden entrar en el historial).

**Cómo mide `learning` la entrada de la lista (resuelto el 23-oct-2026, regla v2).** La v1 tomaba como entrada «el cierre de la sesión del
día UTC de `generated_at`» y no leía `local_date`: con la pasada de D hecha a las 07:15 UTC de D+1, el análisis vio el cierre de D pero se
medía desde el de D+1. La v2 usa el último cierre disponible en el momento del análisis (sección 1) y **no hace falta leer `local_date`**:
para cualquier instante de la ventana de D (cierre de D + 30 min hasta la apertura siguiente) da el cierre de D, incluidos los fines de semana,
los festivos de EE. UU. y los cambios de hora de EE. UU. (8-mar y 1-nov de 2026), donde la ventana pasa de 21:00-14:30 UTC a 20:00-13:30 y al
revés. Lo comprueba el test `watchlist_passes_enter_at_the_close_of_their_trading_day` (inicio, +1 h, mitad y último segundo de siete ventanas) y, de punta a punta con
la BD, `analysis_before_the_open_is_measured_from_the_previous_close`. Casos que quedan:
- *Análisis en curso al llegar la apertura*: `generated_at` cae después de abrir, pero `generated_at − durationMs` (el instante en que leyó los
  datos) cae antes y la entrada sigue siendo el cierre de D (test `the_time_the_analysis_took_moves_the_reference_instant_before_the_open`). Solo
  fallaría si los datos se leyeron en los segundos entre la comprobación de la apertura y la lectura del snapshot, o sin `durationMs`.
- *Cripto de la lista* (ETH, SOL): su entrada es el cierre del día UTC del análisis, o sea D+1 en el pase de las 07:15 UTC, no D. Es lo
  coherente con «el precio del momento» (la pasada la analiza a las 07:15 UTC de D+1, no a las 22:00 de D), y su benchmark (BTC) entra el mismo día.
- No se une con `watchlist_results`: el crate `learning` no depende de las tablas de la lista (solo de `analyses` y `analysis_outcomes`).

**Aprendizaje cableado** (`main.rs`): `activate_learned_weights` al arrancar (si falla, se registra y rigen los pesos por defecto: nunca
impide arrancar) y `run_learning_loop`: primer `run_cycle` a los 2 min y luego cada 6 h; un fallo o un panic de un ciclo se registra y se
reintenta en el siguiente.

**Rutas** (`docs/API.md`): `GET /api/learning/performance`, `GET /api/learning/weights`, `POST /api/learning/weights/rollback`,
`GET /api/learning/calibrated-confidence` (la usa el informe de análisis con `model = analysis.model`; sale la calibrada solo con calibración aplicable) y
`GET /api/watchlist`. Vista `/performance` en el frontend.

## Datos reales hoy (15-oct-2026)

`data/invest-ai.db` (copiada al scratchpad, solo lectura): **0 análisis** (`analyses` vacía; también 0 mensajes). Resolubles:
**0**. Con el ritmo de un usuario que analiza unos pocos activos por semana, juntar los ~230 resultados independientes
que exige la puerta de pesos a 4 semanas llevaría **años**, y los 200 de la calibración también. Hasta entonces el sistema
solo **mide** (panel con intervalos que dirán «sin datos») y los pesos son los de por defecto. Es lo correcto: aprender con
menos sería ajustar ruido.

## Limitaciones y sesgos (no ocultar)

- **No estacionariedad**: el mercado cambia de régimen; lo aprendido caduca. Por eso recencia, validación posterior, gasto de
  alfa y decaimiento, que reducen el daño pero no lo eliminan.
- **Muestra efectiva ≪ n**: todos los análisis comparten régimen y fechas; se mitiga con una fila por activo/ventana y
  bootstrap por bloques, pero un solo año de mercado es un solo régimen.
- **Selección propia**: los activos que el usuario decide analizar no son una muestra aleatoria del mercado.
- **Supervivencia**: un activo que nunca llegó a resolverse (sin precio posterior) acaba `skipped`, se contabiliza pero no entra en las
  métricas (sesgo optimista). Un activo ya resuelto que deja de tener histórico tras una subida de versión **sí** entra: conserva las
  cifras de la regla anterior (vía de reserva, con marca y recuento en el informe). El historial no depende de que sigan existiendo los
  análisis.
- **Deriva de la puntuación**: si cambian las tablas de umbrales de `scoring.rs` (p. ej. R-05), los componentes guardados
  vienen de otra versión de la fórmula y no se pueden recalcular (no se guarda el snapshot). Al cambiar la puntuación
  conviene subir la versión de reglas o empezar un historial nuevo.
- **El objetivo de los pesos es el exceso del *activo*, no el acierto del veredicto del LLM**; y los componentes ya incluyen
  info pública que el mercado descuenta: esperar ICs de 0,02–0,05, no más.
- **Cambio de modelo**: la confianza del LLM cambia de significado al cambiar de modelo; la calibración filtra por el activo.
- **Dividendos/splits**: cierres sin ajustar por dividendos (efecto pequeño y común al índice).
- **Entrada con velas diarias**: el precio exacto del momento no está en ellas. Con el mercado cerrado no importa (es el último cierre);
  con el mercado abierto y en cripto la entrada es el primer cierre posterior (0-6,5 h en bolsa, 0-24 h en cripto después), sin sesgo
  pero con algo menos de señal. Velas intradía solo para la entrada lo afinaría a costa de más peticiones.
- **Comparaciones múltiples**: mitigadas, no anuladas (p-valores bootstrap aproximados; ver el 3 %/3,5 % medido).
- Los p-valores bootstrap con pocos bloques son frágiles; por eso los mínimos de bloques.

## Para el coordinador

1. **Workspace**: `crates/learning` ya está en `members`. **Migración `0005_learning.sql`** (tablas `analysis_outcomes`,
   `scoring_weights`, `learning_runs`). Las migraciones se embeben en `server`; nada más que hacer.
2. **Dependencia**: en `server/Cargo.toml` añadir `learning = { path = "../learning" }`.
3. **Arranque** (`main.rs`, tras abrir el pool): `learning::activate(&pool).await?;` instala los pesos vigentes en `ai::scoring`.
4. **Trabajo periódico**: una tarea de Tokio cada ~6 h (y al arrancar tras un retardo):
   `learning::run_cycle(&pool, market.as_ref(), Utc::now(), &LearningConfig { active_model: Some(modelo), ..Default::default() }).await`.
   Sin análisis maduros no hace peticiones de red; respeta el límite por proveedor (≤ 25 activos por pasada). Debe cancelarse
   con `state.shutdown`.
5. **Ruta HTTP**: `GET /api/learning/performance` → `learning::performance_report(&db, Utc::now(), Some(&modelo_activo),
   &CalibrationConfig::default())` (JSON camelCase). Opcionales: `POST /api/learning/run` (un `run_cycle` a demanda, con el
   mismo single-flight que los análisis), `POST /api/learning/weights/rollback` con `{ "version": 3 | null }` →
   `learning::rollback(&db, version, Utc::now())`, y `GET /api/learning/weights` → `weights::list_versions`.
6. **Confianza calibrada en la UI**: `learning::calibrated_confidence(&db, analysis.confidence, 4, &cfg)` devuelve
   `{ raw, calibrated | null, n, minRequired, reason }`; mostrar la calibrada solo si no es `null` y, si es `null`, «sin
   calibrar (n/200)». Cabe añadirla al GET de un análisis.
7. **Vista de frontend «Rendimiento»** (ruta `/rendimiento`): (a) tarjetas con resueltos / pendientes / sin madurar por
   horizonte; (b) tabla veredicto × horizonte con acierto % (IC), exceso medio (IC) y la tasa base al lado; (c) gráfico de
   fiabilidad (diagonal ideal, puntos con barras de error y n por tramo) con el texto de `calibration.reason`;
   (d) panel de pesos: versión vigente, multiplicadores por componente frente a 1, ΔIC y p de la validación, historial con botón
   «volver a esta versión»; (e) los `caveats` del informe visibles. Con muestras pequeñas mostrar «sin datos suficientes», no
   porcentajes engañosos.
8. **No tocado** (fuera de mi área): `server/src/**`, frontend, `analysis/**`. Una mejora posible en `analysis/` sería
   guardar en `Analysis.method` la versión de pesos con la que se generó (hoy se infiere por fecha en `scoring_weights`).
9. **Estado global**: `ai::scoring::set_learned_multipliers` es global al proceso; los tests de `server` que dependan de
   puntuaciones exactas no deben instalar pesos (solo `activate` lo hace).
