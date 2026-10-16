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
**`0005_learning.sql`** (tablas `analysis_outcomes`, `scoring_weights`, `learning_runs`; la 0004 es del RAG).

## 1. Resultados: reglas A PRIORI (versión 1, `rules::RULE_VERSION`)

Fijadas antes de ver ningún resultado. Cambiarlas exige subir `RULE_VERSION`; las filas ya etiquetadas no se reescriben y las
estadísticas solo cuentan la versión vigente.

- **Horizontes**: 1, 4 y 12 semanas (7·h días) desde `generated_at`.
- **Precios**: cierres diarios de `MarketData::history(…, D1)`. **Entrada** = cierre de la sesión del día (UTC) del análisis, o
  de la siguiente si ese día no hay sesión (equivale a una orden «al cierre» ejecutable tras leer el análisis). **Salida** =
  primer cierre con día ≥ entrada + 7·h días. La vela de salida debe estar **completa** (`t + 24 h ≤ ahora`): la vela en
  curso (parcial) nunca se usa. Huecos > 5 días → «sin datos». Solo se usan velas, nunca `price_at_analysis` (splits).
  Raw/ajustado: los cierres son los del proveedor (Yahoo, sin ajustar por dividendos); el sesgo afecta igual al índice.
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
`rollback`, `decay`, `manual`. `weights::rollback(db, Some(v) | None, now)` crea una versión nueva con los multiplicadores de `v`
(o los de por defecto) y la activa. `weights::activate(db)` instala la vigente en `ai::scoring` (llamar al arrancar y tras
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
versión de pesos vigente e historial; y una lista de `caveats`.

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

**Programador** (tarea Tokio cancelable con `state.shutdown`):
1. Una pasada al día a `WATCHLIST_TIME` (hora **local** del sistema). 23:00 en Madrid es posterior al cierre de EE. UU., así que el
   análisis ve el cierre del día; la entrada del benchmark es el cierre de la sesión (`rules`).
2. **Recuperación**: al arrancar, si ya pasó la hora de hoy y a algún activo de la lista le falta un análisis válido ese día
   (`watchlist_results`), se lanza de inmediato con `trigger = recovery`. Se guardan la hora prevista (`scheduledFor`) y la real
   (`startedAt`). Una pasada que quedó `running` por un apagado o caída se marca `interrupted` y sus activos sin `ok` se rehacen.
   Solo se recupera el día en curso (un día entero sin servidor se pierde y queda constancia por la ausencia de pasada).
3. **De uno en uno** y por el mismo camino que la API (`routes::analysis::run_to_completion`: single-flight, límite de generaciones,
   guardado en BD), con `WATCHLIST_DEPTH` y el modelo activo. Si ya había un análisis del activo en curso por la API, se suma a él.
4. **Fallos**: un activo que falla no detiene la lista y queda registrado con código y mensaje. Los transitorios (Ollama caído,
   límites, red) se reintentan 3 veces con espera de 1 y 5 min; con 2 activos seguidos agotados se aborta la pasada (el resto queda
   `skipped`, visible) y se hacen hasta 3 rondas más el mismo día, cada 30 min. Los permanentes (activo inexistente) no se reintentan.
5. **Batería**: si `/sys/class/power_supply` indica que el equipo va con batería (fuentes `Mains`/`USB*` desconectadas), la pasada
   espera a que se enchufe, con tope `WATCHLIST_BATTERY_MAX_DELAY_HOURS` (3 h; 0 = no esperar). Con la GPU limitada a ~30 W cada
   análisis tarda el doble. Se anota en `watchlist_runs.power` (`ac`/`battery`/`unknown`) y `power_wait_secs`. Sin dato de energía no espera.
6. Con `INVEST_AI_FAKE=1` la lista y el ciclo de aprendizaje se desactivan (datos simulados no pueden entrar en el historial).

**Aprendizaje cableado** (`main.rs`): `activate_learned_weights` al arrancar (si falla, se registra y rigen los pesos por defecto: nunca
impide arrancar) y `run_learning_loop`: primer `run_cycle` a los 2 min y luego cada 6 h; un fallo o un panic de un ciclo se registra y se
reintenta en el siguiente.

**Rutas** (`docs/API.md`): `GET /api/learning/performance`, `GET /api/learning/weights`, `POST /api/learning/weights/rollback`,
`GET /api/learning/calibrated-confidence` (la usa el informe de análisis: sale la calibrada solo con calibración aplicable) y
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
- **Supervivencia**: activos deslistados sin precio posterior acaban `skipped` y se contabilizan, pero no entran en las
  métricas (sesgo optimista). El historial no depende de que sigan existiendo los análisis.
- **Deriva de la puntuación**: si cambian las tablas de umbrales de `scoring.rs` (p. ej. R-05), los componentes guardados
  vienen de otra versión de la fórmula y no se pueden recalcular (no se guarda el snapshot). Al cambiar la puntuación
  conviene subir la versión de reglas o empezar un historial nuevo.
- **El objetivo de los pesos es el exceso del *activo*, no el acierto del veredicto del LLM**; y los componentes ya incluyen
  info pública que el mercado descuenta: esperar ICs de 0,02–0,05, no más.
- **Cambio de modelo**: la confianza del LLM cambia de significado al cambiar de modelo; la calibración filtra por el activo.
- **Dividendos/splits**: cierres sin ajustar por dividendos (efecto pequeño y común al índice).
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
