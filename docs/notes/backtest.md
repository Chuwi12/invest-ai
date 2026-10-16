# Backtest de los factores de precio de la puntuación

Estado: **diseño fijado el 15-oct-2026 ANTES de ejecutar nada; ejecutado el 16-oct-2026 (resultados al final).** Cualquier cambio posterior se anota en «Desviaciones» con su
motivo (cambiar el diseño tras ver resultados es data snooping y se declara como tal).

Código: `backend/crates/learning/src/backtest.rs` (lógica pura, con tests) y `backend/crates/learning/examples/backtest.rs`
(descarga + ejecución). Un cambio mínimo en `ai/src/scoring.rs`: `pub fn score_en(&AssetSnapshot)`, porque `Lang` no es nombrable
fuera del crate `ai` y `score` no se podía llamar desde `learning`; no cambia ninguna lógica.

## Pregunta

¿Tienen poder predictivo los factores de `ai::scoring` que se pueden reconstruir **sin mirar al futuro** (los derivados solo de
precios) sobre el **exceso de rentabilidad a 1, 4 y 12 semanas frente al índice**?

Qué es reconstruible: **técnico** (tendencia, distancia a SMA50/SMA200, cruce de medias, RSI14, MACD, recompensa/riesgo entre
soporte y resistencia), **momentum** (rentabilidad 1m/3m/1a y distancia al máximo de 52 semanas) y **riesgo en su parte de
precios** (volatilidad 30d, caída máxima a 1 año, ATR14/precio). El riesgo real de la app usa además beta y capitalización
(`fundamentals`/`quote`), que solo existen «a fecha de hoy» y serían anticipación; en el backtest el snapshot se construye **sin
fundamentales ni cotización**, así que ese componente es «riesgo-precio». **No se pueden reconstruir** valoración,
fundamentales, sentimiento ni macro (no hay su historia punto-en-el-tiempo): quedan fuera y la conclusión **no dice nada** de
ellos. El compuesto («composite») es la media ponderada de los tres con los pesos por clase de `default_weights` (sin
multiplicadores aprendidos), es decir `scores.overall` de un snapshot solo con indicadores: **un compuesto parcial**, no la
puntuación completa de la app.

## Universo (fijo, declarado ahora)

114 símbolos de Yahoo (recuento corregido, ver «Desviaciones»), elegidos por tipo de activo de la lista de seguimiento de la app y **sin mirar sus rentabilidades**:

- **ETF de renta variable (36, `etf`)**: índice/estilo/internacional `QQQ IWM DIA MDY IJR EFA EEM VGK EWJ EWZ FXI EWG IWD IWF USMV`;
  sectoriales `XLK XLF XLE XLV XLY XLP XLI XLB XLU XLRE XLC`; temáticos `SMH XBI KRE XHB IBB GDX XME XOP VNQ ITA`.
  Los ETF no quiebran: son lo menos afectado por el sesgo de supervivencia.
- **Acciones de EE. UU. (51, `stock`)**: grandes y también «perdedoras» de la década para no tener solo ganadoras:
  `AAPL MSFT NVDA AMZN GOOGL META TSLA BRK-B JPM V UNH XOM JNJ PG HD MA CVX LLY KO PEP COST WMT DIS NFLX ADBE CRM CSCO INTC IBM
  BA GE F GM PFE MRK ABBV T VZ CMCSA NKE MCD ORCL AMD QCOM TXN CAT MMM KHC PYPL SBUX CVS`.
- **ADR europeos (15, `stock`)**: `ASML SAP NVO SHEL AZN UL DEO BP TTE SNY GSK NVS SAN BBVA ING`.
- **Cripto (11, `crypto`)**: `BTC ETH XRP LTC ADA DOGE BNB LINK XLM TRX BCH` (símbolos `X-USD` de Yahoo).
- **SPY**: solo benchmark (su exceso es 0 por definición; no entra en la sección cruzada).

Si algún símbolo falla en la descarga, se omite y se anota aquí en «Desviaciones» (no se sustituye por otro).

**Universos de análisis** (la tabla principal es el 1; el resto son sensibilidades *declaradas ahora*):
1. **Principal «renta variable»**: ETF + acciones + ADR (102 símbolos). Benchmark SPY. Es la familia sobre la que se corrige por
   comparaciones múltiples.
2. Solo ETF (36). El que menos sufre supervivencia.
3. Solo acciones y ADR (66). El más afectado por supervivencia.
4. Cripto (10 sin BTC). Benchmark BTC (regla de `learning::rules::benchmark_for`). Cruce de solo 10 activos: indicativo.

### Sesgo de supervivencia (no resuelto, solo mitigado)

La lista se fija **hoy** con valores que **siguen cotizando**. Faltan los que quebraron, fueron deslistados o absorbidos entre
2018 y 2026 (p. ej. SVB, Credit Suisse, FTX/Terra/LUNA en cripto, WBA…). Efectos esperados: (a) en acciones, sesgo **optimista** en
la rentabilidad absoluta y, sobre todo, en factores que compran caídas o perdedores (los perdedores que se salvaron rebotaron;
los que no, no están); un factor de tendencia queda *menos* afectado pero tampoco libre; (b) en cripto el sesgo es **muy grande**
(sobreviven las que hoy están entre las primeras). Mitigación: ETF (sin quiebra), acciones «perdedoras» que siguen vivas y la
tabla por universos para ver si el resultado depende de las acciones. **Sesgo restante: hay una selección implícita por
supervivencia y por «gran capitalización hoy» (la inclusión en 2026 se correlaciona con haber ido bien).** Un resultado positivo
en el universo 3 hay que leerlo con esa reserva; un resultado nulo en el 2 es el más fiable.

## Datos

- Yahoo Finance, velas diarias vía `market::providers::yahoo::Yahoo::daily_between` (ya implementada, con `events=split`), de
  **2016-09-01 a hoy**: una petición por símbolo (114), con 1,2 s entre peticiones por encima del limitador.
- Las velas OHLC vienen **ajustadas por escisiones, no por dividendos**. Para la **puntuación** se usan tal cual (es lo que ve la
  app). Para la **rentabilidad futura** se usa el **cierre ajustado por dividendos** (`adjclose`): rentabilidad total, en activo
  y benchmark. El factor de ajuste de dividendos es retroactivo, pero el cociente entre dos fechas pasadas solo incluye los
  dividendos entre ellas: no introduce anticipación. Un efecto menor: la puntuación ve el precio sin dividendos.
- **Congelado** en `~/.cache/invest-ai/backtest/prices.json` (fuera del repo: 40,8 MB; ruta sobrescribible con `BACKTEST_DATA`).
  Se anotan fecha de descarga, nº de símbolos y de velas y su `sha256sum`. `run` solo lee ese fichero (cero red).

## Sin anticipación (por construcción y con test)

- Rejilla **semanal**: cada **viernes** desde 2018-01-05. Para cada activo la señal usa las velas con día UTC ≤ ese viernes
  (última sesión ≤ viernes, a ≤ 4 días; si no, el activo falta esa semana), en una ventana de 2 años como la app
  (`Range::Y2`), por `market::indicators::compute_at` + `ai::scoring::score_en` sobre un snapshot solo con indicadores.
- El activo entra en la sección cruzada solo si tiene ≥ 400 días de historia antes de la fecha (52 semanas y rentabilidad a
  1 año) y los tres componentes (técnico, momentum, riesgo-precio) puntúan.
- **Rentabilidad futura**: entrada = **primer cierre posterior al viernes** (siguiente sesión; el sábado en cripto), salida = primer
  cierre ≥ entrada + 7·h días, con `learning::rules::compute` (las mismas reglas que el resolutor de la app; h = 1, 4, 12).
  Exceso = rentabilidad total del activo − la del benchmark en los mismos días.
- Test: la puntuación en la fecha *d* es idéntica con y sin velas posteriores (incluso si las posteriores son basura).

## Métricas (todas fijadas ahora)

1. **IC de Spearman por fecha** (sección cruzada, mín. 15 activos; 6 en cripto) entre el factor y el exceso a h semanas, por factor
   (`technical`, `momentum`, `risk` en orientación favorable = 100 − riesgo, `composite`). Resumen: media de los ICs semanales,
   **error típico Newey-West** (núcleo de Bartlett, rezago **L = h**, porque las rentabilidades a h semanas muestreadas cada
   semana se solapan), t = media/se, IC 95 % = media ± 1,96·se. Segundo intervalo, **bootstrap por bloques** de 13 semanas
   consecutivas (2000 réplicas, semilla 20261015, `stats::mean_ci`). Los ICs son de **orientación favorable**: positivo = el factor
   acierta como lo diseñó la app.
2. **Familia de contraste y regla de decisión (a priori)**: 4 factores × 3 horizontes = 12 pruebas en el universo 1. Se llamará
   «señal» a un resultado solo si **(i) |t_NW| ≥ 2,87** (Bonferroni, α = 0,05/12), **(ii) el signo coincide en los tres
   subperiodos** y **(iii) el efecto es positivo** (si fuera negativo y significativo es evidencia en contra de la app, no a
   favor). Para que sirva en la práctica, además **(iv) la cartera long-short neta de costes del mismo factor y horizonte gana
   > 0**. Todo lo que no cumpla (i)-(iii) se lee como «no se distingue del azar».
3. **Subperiodos** (por fecha de evaluación): 2018–2020, 2021–2023, 2024–hoy.
4. **Acierto por veredicto** con la regla a priori de `learning::rules` (`is_hit`, `RULE_VERSION` 1): el veredicto es el
   **cuantitativo provisional** de `score_en` sobre el snapshot de precios (no el del LLM). Se da, por veredicto y horizonte, nº,
   acierto, **acierto de referencia** (probabilidad de acierto de ese veredicto en la muestra entera: para comprar/acumular, la
   tasa base de superar al índice; para reducir/evitar, su complementaria; mantener/esperar, la masa de la banda neutra) y la
   diferencia, con IC por bootstrap por bloques (13 semanas; 26 en h = 12). Wilson sería engañoso porque las observaciones de
   un mismo viernes están correlacionadas.
5. **Cartera quintil superior − inferior** (equiponderada, `k = ⌊n/5⌋` activos por pata, universo 1, también cripto aparte),
   horizonte h: se rebalancea cada h semanas con *h cohortes escalonadas* (cada semana entra una cohorte con 1/h del capital; las
   cifras son la media de las rentabilidades por cohorte × 52/h, aritmética, anualizada). **Bruto**, **neto** y **solo largos**
   (quintil superior − SPY, neto). Rotación de cada cohorte respecto a la anterior de la misma cadena (t − h).
   - Costes: **10 pb por operación** (compra o venta) en acciones/ETF (medio diferencial + deslizamiento de grandes valores
     líquidos, sin comisión de bróker); **25 pb** en cripto. Coste por rebalanceo = pb × (volumen negociado por pata, que es 2 × fracción
     sustituida) sumado en las patas. **Préstamo de acciones** de la pata corta: 0,5 %/año en renta variable; financiación del
     corto en cripto 10 %/año. No se modela impacto de mercado, impuestos, ni límites de corto en ETF difíciles.
   - Sensibilidad a costes para el compuesto: 0 / 5 / 10 / 20 pb.
   - Se da media anualizada, desviación (anualizada), ratio de Sharpe sobre el exceso, t-NW, y por subperiodo.
6. **Referencias honestas**:
   - **Momentum 12-1** estándar: rentabilidad total (cierre ajustado) de hace 12 meses a hace 1 mes (T−365 d → T−30 d), mismo
     tratamiento (IC, cartera).
   - **Puntuación aleatoria**: uniforme por (fecha, activo) con semilla; **200 semillas** (1..=200): distribución del IC medio y
     de t y proporción de semillas con |t| > 1,96 y > 2,87; y la cartera aleatoria bruta/neta (para ver el lastre puro de
     costes y el ruido de una cartera de ~19 activos por pata).

## Decisiones que NO se tocan tras ver resultados

Universo, fechas, ventana, h, núcleo y rezago de Newey-West, costes, regla de decisión, los 3 subperiodos y las referencias.
Qué no se hace: elegir por factor el horizonte que mejor sale, recortar fechas, quitar activos, cambiar umbrales de la
puntuación para «mejorar» el IC.

## Desviaciones

Anotadas el 16-oct-2026, **antes de ver ningún resultado** (el diseño se escribió el 15-oct y los datos se descargaron el 16-oct):

1. **Recuento corregido, lista idéntica.** Al escribir el diseño sumé mal los tamaños (decía 34 ETF, 47 acciones, 108 símbolos, 96/62
   en los universos 1 y 3). Las listas de símbolos del documento son exactamente las del código; los recuentos reales son
   36 ETF, 51 acciones de EE. UU., 15 ADR, 11 cripto y SPY = **114**; universo 1 = 102, universo 3 = 66. No se añadió ni quitó ningún símbolo.
2. **Ningún símbolo omitido**: los 114 se descargaron a la primera.
3. **Detalles de implementación no especificados arriba** (no cambian ninguna decisión de contraste): en cripto, `k = máx(1, ⌊n/5⌋)`
   (con 10 activos, k = 2); la rejilla de viernes termina el día de la descarga (el último viernes no tiene resultados, y
   cada horizonte usa solo las semanas cuyo resultado ya maduró); si faltan semanas dentro de una serie de ICs, Newey-West las trata
   como consecutivas (afecta solo a cripto antes de 2019).
4. Un test de Newey-West (`newey_west_corrects_for_autocorrelation`) tenía mal la teoría de su cota (AR(1) con φ = 0,8: el cociente
   verdadero es √((1+φ)/(1−φ)) = 3, no 5); se corrigió el test, no el estimador.

Después de ver los resultados: **ninguna desviación** (nada se ha cambiado).

## Resultados (ejecución del 16-oct-2026)

Reproducir: `cd backend && cargo run --release -p learning --example backtest -- download` (una vez) y `… -- run` (sin red,
determinista: dos ejecuciones dan la salida idéntica byte a byte; tarda ~4 s). Tests: `cargo test -p learning backtest` (9: sin
anticipación con futuro envenenado, entrada en el siguiente cierre, reproducibilidad con semilla, IC con señal conocida
ρ = 0,30 → IC ≈ 0,288 recuperado y IC nulo sin señal, Newey-West, contabilidad de costes y rotación, quintiles, tabla de veredictos).

**Datos congelados**: `~/.cache/invest-ai/backtest/prices.json`, 40,8 MB (fuera del repo), 114 símbolos, 298 342 velas diarias
(2016-09-01 a 2026-10-16), sha256 `a0569542af69d712fa6425ca0556b70d1878d0d25664d8cb72cb44e94b887ffa`.
**Panel**: 459 viernes (2018-01-05 a 2026-10-16); en el universo 1, 101,8 activos de media por semana; 457/454/446 semanas con
resultado a 1/4/12 semanas. Universo 1 (`equity`) = 102 activos; ETF 36; acciones 66; cripto 10 (410 semanas usables).

### 1. IC de Spearman, universo principal (102 activos, orientación favorable)

| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|
| técnico | 1 | 457 | +0,017 | 1,52 | [-0,005, +0,039] | +0,025 | +0,014 | +0,012 |
| técnico | 4 | 454 | +0,017 | 1,10 | [-0,013, +0,048] | +0,038 | +0,002 | +0,012 |
| técnico | 12 | 446 | +0,026 | 1,13 | [-0,019, +0,070] | +0,049 | +0,013 | +0,014 |
| momentum | 1 | 457 | +0,019 | 1,54 | [-0,005, +0,043] | +0,030 | +0,005 | +0,022 |
| momentum | 4 | 454 | +0,018 | 0,98 | [-0,018, +0,054] | +0,044 | -0,011 | +0,022 |
| momentum | 12 | 446 | +0,011 | 0,43 | [-0,039, +0,062] | +0,045 | -0,017 | +0,005 |
| riesgo-precio (100 − riesgo) | 1 | 457 | -0,017 | -1,37 | [-0,041, +0,007] | -0,014 | -0,020 | -0,016 |
| riesgo-precio | 4 | 454 | -0,035 | -1,83 | [-0,072, +0,002] | -0,045 | -0,024 | -0,036 |
| riesgo-precio | 12 | 446 | -0,053 | -1,79 | [-0,111, +0,005] | -0,078 | -0,046 | -0,031 |
| **compuesto** | 1 | 457 | +0,013 | 1,01 | [-0,012, +0,037] | +0,025 | +0,002 | +0,011 |
| **compuesto** | 4 | 454 | +0,008 | 0,44 | [-0,028, +0,044] | +0,026 | -0,007 | +0,006 |
| **compuesto** | 12 | 446 | +0,002 | 0,09 | [-0,050, +0,055] | +0,018 | -0,011 | -0,000 |
| *ref.: momentum 12-1* | 1 | 457 | +0,025 | 1,87 | [-0,001, +0,051] | +0,039 | -0,002 | +0,039 |
| *ref.: momentum 12-1* | 4 | 454 | +0,027 | 1,39 | [-0,011, +0,065] | +0,039 | +0,011 | +0,031 |
| *ref.: momentum 12-1* | 12 | 446 | +0,028 | 0,90 | [-0,033, +0,088] | +0,024 | +0,011 | +0,053 |

(El intervalo por bootstrap de bloques de 13 semanas coincide con el de Newey-West en todas las filas, ±0,005; está en el apéndice.)

### 2. Qué IC sale por azar (puntuación aleatoria, 200 semillas, universo 1)

| h | IC medio | sd entre semillas | percentiles 2,5 / 97,5 | semillas con \|t\|>1,96 | \|t\|>2,87 |
|---|---|---|---|---|---|
| 1 | -0,000 | 0,004 | -0,008 / +0,009 | 4,5 % | 0,0 % |
| 4 | +0,000 | 0,004 | -0,008 / +0,008 | 3,0 % | 0,0 % |
| 12 | -0,000 | 0,004 | -0,008 / +0,008 | 3,0 % | 0,5 % |

El t de Newey-West está bien calibrado (≈ 5 % de falsos positivos al 1,96; ~0 % al 2,87). **Ningún IC observado en las filas del
apartado 1 sale del rango del azar de forma significativa** (el mayor |t| de los 12 de la familia es 1,83, y con signo negativo).

### 3. Regla de decisión a priori: 0 de 12

Ninguna de las 12 pruebas (técnico, momentum, riesgo, compuesto × 1/4/12 semanas) cumple (i) |t| ≥ 2,87; **ni siquiera** llega a
|t| ≥ 1,96 sin corregir. Además (ii) falla en 7 de las 12 (momentum 4 y 12 y compuesto 4 y 12 tienen algún subperiodo con IC ≤ 0, y
el riesgo es negativo en los tres). Detalle en el apéndice.

### 4. Carteras quintil superior − inferior (universo 1; 10 pb por operación, préstamo 0,5 %/año; anualizado)

| factor | h | bruto | **neto** | Sharpe neto | t neto | rotación media | solo largos neto vs SPY |
|---|---|---|---|---|---|---|---|
| técnico | 1 | +7,4 % | +0,6 % | 0,03 | 0,10 | 30 % | -0,6 % |
| técnico | 4 | +2,3 % | -0,5 % | -0,03 | -0,12 | 45 % | -0,1 % |
| técnico | 12 | +2,4 % | +0,8 % | 0,05 | 0,22 | 62 % | +0,7 % |
| momentum | 1 | +7,5 % | +2,7 % | 0,14 | 0,41 | 21 % | +2,3 % |
| momentum | 4 | +3,4 % | +0,7 % | 0,04 | 0,14 | 41 % | +1,3 % |
| momentum | 12 | +1,2 % | -0,4 % | -0,02 | -0,09 | 60 % | +0,7 % |
| riesgo-precio | 1 | -15,2 % | -17,6 % | -0,86 | -2,55 | 9 % | -8,7 % |
| riesgo-precio | 4 | -15,3 % | -17,0 % | -0,84 | -2,96 | 22 % | -7,6 % |
| riesgo-precio | 12 | -14,6 % | -15,7 % | -0,80 | -3,10 | 33 % | -6,9 % |
| **compuesto** | 1 | +1,0 % | **-3,7 %** | -0,18 | -0,53 | 20 % | -3,6 % |
| **compuesto** | 4 | -3,1 % | **-5,5 %** | -0,28 | -1,02 | 37 % | -2,9 % |
| **compuesto** | 12 | -3,8 % | **-5,2 %** | -0,30 | -1,23 | 56 % | -2,6 % |
| *ref.: momentum 12-1* | 1 | +6,6 % | +3,9 % | 0,18 | 0,54 | 11 % | +5,0 % |
| *ref.: momentum 12-1* | 4 | +5,5 % | +3,9 % | 0,19 | 0,69 | 21 % | +5,4 % |
| *ref.: momentum 12-1* | 12 | +5,1 % | +3,9 % | 0,20 | 0,74 | 37 % | +6,2 % |
| *ref.: aleatorio (media de 200 semillas)* | 1 / 4 / 12 | +0,2 / +0,1 / 0,0 % | -17,0 / -4,6 / -1,9 % | | | | |

Sensibilidad del compuesto a los costes (LS neto anual): h=1: +0,5 % (0 pb), -1,6 % (5), -3,7 % (10), -7,9 % (20); h=4: -3,6, -4,6, -5,5,
-7,5 %; h=12: -4,3, -4,7, -5,2, -6,2 %. **Con costes cero el compuesto ya no gana** (el mejor caso es +0,5 %/año a 1 semana).
Exceso medio por quintil del compuesto a 12 semanas (Q1 peor → Q5 mejor): +0,4 %, 0,0 %, +0,4 %, -0,1 %, **-0,5 %**: los extremos
están al revés. Los de momentum 12-1: +0,3, -0,5, -0,5, -0,5, +1,5 %.

### 5. Acierto por veredicto (veredicto cuantitativo provisional con solo precios; universo 1; regla a priori de `learning::rules`)

Tasa base de superar al índice: 48,9 % (1 s), 47,7 % (4 s), 46,3 % (12 s). Diferencia = acierto − acierto del mismo veredicto aplicado
al azar a toda la muestra; IC por bootstrap de bloques.

| veredicto | n (4 s) | acierto 4 s | diferencia 1 s | diferencia 4 s | diferencia 12 s |
|---|---|---|---|---|---|
| comprar | 3 219 | 47,0 % | -0,2 [-3,0, +2,6] | -0,7 [-4,2, +3,1] | -0,9 [-6,9, +6,3] |
| acumular | 16 373 | 46,4 % | -0,5 [-1,3, +0,4] | -1,4 [-3,0, +0,3] | -1,5 [-4,1, +0,9] |
| mantener | 12 048 | 56,5 % | +0,5 [-1,2, +1,9] | +0,3 [-1,5, +2,0] | +0,4 [-2,2, +2,7] |
| esperar | 7 541 | 53,6 % | **-3,0 [-5,0, -1,1]** | **-2,6 [-4,4, -0,7]** | **-4,3 [-6,6, -1,9]** |
| reducir | 4 923 | 51,0 % | -0,3 [-2,4, +1,7] | -1,3 [-4,1, +1,4] | -0,7 [-5,5, +4,2] |
| evitar | 2 123 | 47,3 % | -2,6 [-6,2, +0,5] | -4,9 [-10,3, +0,6] | **-6,6 [-13,6, -0,5]** |

«Comprar» y «acumular» **no aciertan más que el azar** (acierto por debajo de la tasa base; ningún intervalo está por encima de 0).
«Evitar» y «reducir» no aciertan más que el azar: el acierto de «evitar» es *menor* que el de referencia, es decir, los activos que
la puntuación manda evitar rindieron *mejor* que el resto (significativo solo a 12 semanas y sin corregir 18 intervalos: se lee como
sugerente, coherente con el signo del factor de riesgo, no como un hallazgo). «Esperar» acierta menos, probablemente porque agrupa valores más
volátiles, cuya banda neutra del ±2 %·√semanas se supera más a menudo (hipótesis, no medida); no es una señal direccional.

### 6. Otros universos (sensibilidad declarada)

- **Solo ETF (36)** — el menos afectado por supervivencia: IC del compuesto +0,010 / +0,008 / -0,014 (t 0,61 / 0,35 / -0,44); LS neto
  -2,4 / -4,6 / -6,3 %; en 2024-hoy el compuesto a 12 semanas es *negativo* (IC -0,096, t -2,9). Nada.
- **Solo acciones y ADR (66)** — el más afectado por supervivencia: IC del compuesto +0,014 / +0,010 / +0,010 (t ≤ 1,1); técnico y
  momentum +0,02 a +0,03 (t ≤ 1,64); LS neto del compuesto -2,8 / -3,8 / -3,0 %. Nada ni siquiera con el sesgo a favor.
  El momentum 12-1 de referencia sube a IC +0,025 / +0,033 / +0,034 (t 1,89 / 1,68 / 1,14) y LS neto +5,0 / +3,7 / +5,5 %, tampoco
  significativo, con 2021-23 plano.
- **Cripto (10 supervivientes, indicativo)**: aquí sí hay t grandes: riesgo-precio IC +0,080 / +0,114 / +0,200 (t 4,1 / 3,9 / 4,2) y
  compuesto +0,064 / +0,068 / +0,089 (t 3,2 / 2,4 / 2,2). Pero (a) el universo de 10 monedas es el máximo caso de supervivencia
  (están las que sobrevivieron y siguen en la cabeza del ranking; faltan las que murieron, que eran las de más riesgo), y el
  «riesgo bajo = mejor» frente a BTC es justo lo que ese sesgo fabrica; (b) en 2018-2020 el IC del riesgo es 0 (-0,004/-0,006),
  y el momentum 12-1 es *negativo* en 2024-hoy (-0,18 a 12 semanas); (c) las carteras LS netas no son significativas (compuesto
  +2,6 / -24,9 / +11,7 %, |t| ≤ 0,94) con volatilidades de 77–272 %/año; (d) 10 activos son una sección cruzada muy fina. No es evidencia a
  favor de la app.

## Conclusión honesta

**No hay señal detectable en los factores de precio de la puntuación, y el compuesto de precios no gana nada tras costes (de
hecho pierde incluso antes de ellos a 4 y 12 semanas).**

1. **Poder predictivo (IC):** técnico y momentum tienen IC puntuales pequeños y positivos (+0,01 a +0,03), pero ninguno es
   distinguible de 0 (|t| ≤ 1,54), ni sale del rango que produce una puntuación aleatoria con la corrección declarada. El
   compuesto, que es lo que la app usa, está en +0,013 / +0,008 / +0,002. El componente de riesgo-precio tiene signo **contrario** al
   diseño (los valores más volátiles rindieron más que los tranquilos en este universo y periodo; |t| ≤ 1,83).
   Los signos no son estables entre subperiodos.
   *Nota de revisión (añadida tras ver los resultados; es una hipótesis, no una medida):* buena parte de ese signo negativo es
   **esperable por construcción**. El exceso frente a SPY **no está ajustado por beta**: para un activo con beta β, el exceso es
   ≈ α + (β − 1)·r_SPY. Los valores más volátiles suelen tener β > 1, así que en un periodo alcista (SPY ≈ +13 %/año en 2018-2026)
   ganan al índice por pura exposición al mercado, sin «acierto» alguno. Un largo-corto «poco riesgo − mucho riesgo» tiene beta
   neta negativa: con una diferencia de beta de ~0,7, solo la exposición al mercado le restaría ~9 %/año, más de la mitad del
   -15 % bruto. Además, el componente de riesgo de la app no se diseñó para predecir rentabilidad, sino para penalizar el
   riesgo: que pierda en exceso bruto no significa que esté mal, sino que no es un factor de alfa. Para separar ambas cosas
   hace falta un análisis ajustado por beta (beta estimada solo con datos anteriores a cada viernes), que se declarará como
   **post hoc** y no cambia la regla de decisión de arriba.
2. **Lo que el test puede y no puede decir.** Con 457 semanas y ~100 activos, el error típico de Newey-West del IC del compuesto es
   0,0125 / 0,018 / 0,027 (1 / 4 / 12 semanas). El efecto mínimo detectable con potencia 80 % al umbral corregido (2,87) es
   ≈ 0,046 / 0,068 / 0,099, y sin corregir (1,96) ≈ 0,035 / 0,052 / 0,075. El techo de los intervalos de confianza del compuesto es
   +0,037 / +0,044 / +0,055. **Se descartan ICs grandes (> ~0,05); NO se descartan ICs reales de 0,01–0,03**, que son justo lo que la
   literatura y `learning.md` esperan de factores públicos. «Sin señal detectable» no es «señal nula». Prueba de ello: el
   **momentum 12-1**, la anomalía mejor documentada, sale aquí con IC +0,025 a +0,028 (t 0,9–1,9) y tampoco supera el umbral, con un
   2021-23 plano. Este diseño no tiene potencia para confirmar ni siquiera la referencia.
3. **Tras costes (lo que importa en la práctica):** el largo-corto del compuesto da -3,7 / -5,5 / -5,2 %/año netos; sin costes es
   +0,5 / -3,6 / -4,3 %. Un factor con IC ≈ 0,01 no cubre 10 pb por operación con rotaciones del 20–60 % por rebalanceo (el lastre de una
   cartera aleatoria a 1 semana es -17 %/año). Solo largos contra SPY: -3,6 / -2,9 / -2,6 %. Los quintiles del compuesto a 12 semanas
   están invertidos en los extremos.
4. **Veredictos:** comprar/acumular no acertaron más que el azar en ningún horizonte; evitar/reducir tampoco (los marcados «evitar»
   rindieron algo mejor que el resto).
5. **Sesgos que quedan** (ver arriba): supervivencia y selección por gran capitalización de 2026 (valores vivos y ganadores ex post:
   pueden explicar en parte el signo negativo del riesgo-precio, porque los volátiles que sobrevivieron, TSLA, NVDA, AMD, son los que
   se ven; para un factor de tendencia el signo del sesgo es ambiguo: la ausencia de los quebrados, que eran perdedores con
   tendencia bajista, lo rebaja; la selección de ganadores ex post lo infla); un solo régimen de 8,8 años
   (2018–2026, alcista y dominado por megacapitalización de EE. UU.), con 2021-23 muy distinto; exceso frente a SPY ponderado por
   capitalización; costes reales mayores en ETF temáticos y ADR; no modela impuestos ni impacto.
6. **Qué NO dice este backtest:** nada sobre valoración, fundamentales, sentimiento ni macro (no reconstruibles sin anticipación
   con estos datos), nada sobre la calidad del texto del LLM, y el riesgo se evaluó sin beta ni capitalización.

**Consecuencias prácticas para la app** (sin tocar la puntuación, para no hacer data snooping sobre estos resultados):
(a) no presentar la puntuación de precios ni su veredicto como predictivos del exceso a 1–12 semanas: los datos no lo respaldan;
presentarla como descripción del estado técnico; (b) el aprendizaje de pesos de `learning` necesitará mucha más muestra de la prevista si los ICs reales son ≤ 0,03: detectar un IC
de 0,02 con potencia 80 % y α = 0,05 con 100 activos exigiría del orden de 30 años de datos semanales a 1 semana y 60 o más a 4 semanas
(el error típico escala con 1/√T); los ~230 resultados independientes de la puerta de pesos (años de uso real) no bastan para eso, y la
puerta ya lo trata con prudencia; (c) si se quiere una respuesta firme, el siguiente paso
honesto es repetir este diseño ya fijado con datos libres de supervivencia (constituyentes históricos, incluidos deslistados) y
con más activos, no ajustar el modelo a esta muestra. Cualquier cambio de pesos justificado por la tabla de arriba sería ajustar a
ruido.

## Apéndice: salida completa de `run` (16-oct-2026)

### Resultados del backtest

Datos: 114 símbolos descargados, 298342 velas, descarga 2026-10-16. Panel: 459 viernes (2018-01-05 a 2026-10-16), 112 activos en la sección cruzada.

- Universo `equity`: 102 activos, 459 semanas usables, 101.8 activos de media por semana.
- Universo `etf`: 36 activos, 459 semanas usables, 35.8 activos de media por semana.
- Universo `stock`: 66 activos, 459 semanas usables, 66.0 activos de media por semana.
- Universo `crypto`: 10 activos, 410 semanas usables, 10.0 activos de media por semana.

#### IC de Spearman, universo `equity`

| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | IC 95 % (bloques) | 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 457 | +0.017 | 1.52 | [-0.005, +0.039] | [-0.003, +0.037] | +0.025 (t 1.1, n 156) | +0.014 (t 0.8, n 157) | +0.012 (t 0.7, n 144) |
| technical | 4 | 454 | +0.017 | 1.10 | [-0.013, +0.048] | [-0.011, +0.046] | +0.038 (t 1.2, n 156) | +0.002 (t 0.1, n 157) | +0.012 (t 0.6, n 141) |
| technical | 12 | 446 | +0.026 | 1.13 | [-0.019, +0.070] | [-0.017, +0.067] | +0.049 (t 1.1, n 156) | +0.013 (t 0.4, n 157) | +0.014 (t 0.4, n 133) |
| momentum | 1 | 457 | +0.019 | 1.54 | [-0.005, +0.043] | [-0.004, +0.041] | +0.030 (t 1.2, n 156) | +0.005 (t 0.3, n 157) | +0.022 (t 1.1, n 144) |
| momentum | 4 | 454 | +0.018 | 0.98 | [-0.018, +0.054] | [-0.015, +0.050] | +0.044 (t 1.2, n 156) | -0.011 (t -0.3, n 157) | +0.022 (t 0.9, n 141) |
| momentum | 12 | 446 | +0.011 | 0.43 | [-0.039, +0.062] | [-0.036, +0.056] | +0.045 (t 0.9, n 156) | -0.017 (t -0.4, n 157) | +0.005 (t 0.1, n 133) |
| risk | 1 | 457 | -0.017 | -1.37 | [-0.041, +0.007] | [-0.040, +0.006] | -0.014 (t -0.6, n 156) | -0.020 (t -0.9, n 157) | -0.016 (t -0.9, n 144) |
| risk | 4 | 454 | -0.035 | -1.83 | [-0.072, +0.002] | [-0.073, +0.002] | -0.045 (t -1.2, n 156) | -0.024 (t -0.7, n 157) | -0.036 (t -1.4, n 141) |
| risk | 12 | 446 | -0.053 | -1.79 | [-0.111, +0.005] | [-0.108, +0.004] | -0.078 (t -1.4, n 156) | -0.046 (t -0.9, n 157) | -0.031 (t -0.7, n 133) |
| composite | 1 | 457 | +0.013 | 1.01 | [-0.012, +0.037] | [-0.010, +0.036] | +0.025 (t 1.0, n 156) | +0.002 (t 0.1, n 157) | +0.011 (t 0.6, n 144) |
| composite | 4 | 454 | +0.008 | 0.44 | [-0.028, +0.044] | [-0.027, +0.043] | +0.026 (t 0.7, n 156) | -0.007 (t -0.2, n 157) | +0.006 (t 0.3, n 141) |
| composite | 12 | 446 | +0.002 | 0.09 | [-0.050, +0.055] | [-0.047, +0.051] | +0.018 (t 0.3, n 156) | -0.011 (t -0.2, n 157) | -0.000 (t -0.0, n 133) |
| mom12_1 | 1 | 457 | +0.025 | 1.87 | [-0.001, +0.051] | [+0.002, +0.048] | +0.039 (t 1.5, n 156) | -0.002 (t -0.1, n 157) | +0.039 (t 2.0, n 144) |
| mom12_1 | 4 | 454 | +0.027 | 1.39 | [-0.011, +0.065] | [-0.006, +0.059] | +0.039 (t 1.0, n 156) | +0.011 (t 0.4, n 157) | +0.031 (t 1.2, n 141) |
| mom12_1 | 12 | 446 | +0.028 | 0.90 | [-0.033, +0.088] | [-0.033, +0.084] | +0.024 (t 0.4, n 156) | +0.011 (t 0.2, n 157) | +0.053 (t 1.8, n 133) |

#### IC de Spearman, universo `etf`

| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | IC 95 % (bloques) | 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 457 | +0.010 | 0.75 | [-0.017, +0.038] | [-0.013, +0.035] | +0.033 (t 1.1, n 156) | +0.009 (t 0.5, n 157) | -0.012 (t -0.5, n 144) |
| technical | 4 | 454 | +0.010 | 0.49 | [-0.029, +0.049] | [-0.027, +0.046] | +0.033 (t 0.8, n 156) | +0.017 (t 0.6, n 157) | -0.024 (t -0.8, n 141) |
| technical | 12 | 446 | +0.011 | 0.38 | [-0.046, +0.068] | [-0.042, +0.067] | +0.074 (t 1.3, n 156) | +0.017 (t 0.4, n 157) | -0.070 (t -1.8, n 133) |
| momentum | 1 | 457 | +0.013 | 0.86 | [-0.017, +0.044] | [-0.013, +0.041] | +0.027 (t 0.8, n 156) | +0.010 (t 0.4, n 157) | +0.003 (t 0.1, n 144) |
| momentum | 4 | 454 | +0.009 | 0.37 | [-0.037, +0.054] | [-0.036, +0.052] | +0.030 (t 0.6, n 156) | +0.009 (t 0.2, n 157) | -0.015 (t -0.5, n 141) |
| momentum | 12 | 446 | -0.017 | -0.55 | [-0.079, +0.045] | [-0.076, +0.043] | +0.044 (t 0.7, n 156) | -0.031 (t -0.7, n 157) | -0.073 (t -1.6, n 133) |
| risk | 1 | 457 | -0.017 | -1.11 | [-0.048, +0.013] | [-0.049, +0.013] | +0.001 (t 0.0, n 156) | -0.012 (t -0.5, n 157) | -0.043 (t -1.6, n 144) |
| risk | 4 | 454 | -0.020 | -0.84 | [-0.068, +0.027] | [-0.071, +0.028] | -0.006 (t -0.1, n 156) | -0.002 (t -0.1, n 157) | -0.057 (t -1.4, n 141) |
| risk | 12 | 446 | -0.039 | -1.05 | [-0.113, +0.034] | [-0.113, +0.036] | +0.003 (t 0.0, n 156) | -0.018 (t -0.4, n 157) | -0.114 (t -1.8, n 133) |
| composite | 1 | 457 | +0.010 | 0.61 | [-0.021, +0.041] | [-0.016, +0.036] | +0.031 (t 0.9, n 156) | +0.006 (t 0.3, n 157) | -0.009 (t -0.3, n 144) |
| composite | 4 | 454 | +0.008 | 0.35 | [-0.038, +0.054] | [-0.035, +0.051] | +0.028 (t 0.6, n 156) | +0.018 (t 0.5, n 157) | -0.025 (t -0.7, n 141) |
| composite | 12 | 446 | -0.014 | -0.44 | [-0.080, +0.051] | [-0.077, +0.053] | +0.051 (t 0.8, n 156) | -0.011 (t -0.2, n 157) | -0.096 (t -2.9, n 133) |
| mom12_1 | 1 | 457 | +0.022 | 1.33 | [-0.010, +0.053] | [-0.008, +0.053] | +0.037 (t 1.1, n 156) | -0.004 (t -0.2, n 157) | +0.033 (t 1.2, n 144) |
| mom12_1 | 4 | 454 | +0.015 | 0.61 | [-0.034, +0.065] | [-0.033, +0.064] | +0.047 (t 0.9, n 156) | -0.006 (t -0.1, n 157) | +0.003 (t 0.1, n 141) |
| mom12_1 | 12 | 446 | +0.005 | 0.11 | [-0.081, +0.091] | [-0.080, +0.090] | +0.014 (t 0.1, n 156) | +0.001 (t 0.0, n 157) | -0.001 (t -0.0, n 133) |

#### IC de Spearman, universo `stock`

| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | IC 95 % (bloques) | 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 457 | +0.018 | 1.61 | [-0.004, +0.041] | [-0.003, +0.041] | +0.020 (t 1.0, n 156) | +0.014 (t 0.7, n 157) | +0.020 (t 1.1, n 144) |
| technical | 4 | 454 | +0.019 | 1.15 | [-0.013, +0.051] | [-0.016, +0.052] | +0.033 (t 1.1, n 156) | +0.000 (t 0.0, n 157) | +0.023 (t 1.0, n 141) |
| technical | 12 | 446 | +0.031 | 1.25 | [-0.018, +0.079] | [-0.013, +0.076] | +0.040 (t 0.8, n 156) | +0.018 (t 0.5, n 157) | +0.034 (t 0.9, n 133) |
| momentum | 1 | 457 | +0.020 | 1.64 | [-0.004, +0.045] | [-0.003, +0.044] | +0.028 (t 1.2, n 156) | +0.003 (t 0.1, n 157) | +0.031 (t 1.5, n 144) |
| momentum | 4 | 454 | +0.022 | 1.20 | [-0.014, +0.059] | [-0.014, +0.057] | +0.048 (t 1.4, n 156) | -0.014 (t -0.4, n 157) | +0.034 (t 1.3, n 141) |
| momentum | 12 | 446 | +0.025 | 0.90 | [-0.029, +0.078] | [-0.027, +0.073] | +0.052 (t 1.0, n 156) | -0.003 (t -0.1, n 157) | +0.026 (t 0.6, n 133) |
| risk | 1 | 457 | -0.012 | -0.83 | [-0.041, +0.017] | [-0.040, +0.016] | -0.008 (t -0.3, n 156) | -0.020 (t -0.7, n 157) | -0.008 (t -0.4, n 144) |
| risk | 4 | 454 | -0.032 | -1.37 | [-0.077, +0.014] | [-0.080, +0.015] | -0.038 (t -0.9, n 156) | -0.022 (t -0.5, n 157) | -0.036 (t -1.1, n 141) |
| risk | 12 | 446 | -0.042 | -1.13 | [-0.114, +0.031] | [-0.109, +0.033] | -0.068 (t -1.0, n 156) | -0.038 (t -0.6, n 157) | -0.016 (t -0.3, n 133) |
| composite | 1 | 457 | +0.014 | 1.09 | [-0.011, +0.040] | [-0.010, +0.039] | +0.023 (t 1.0, n 156) | -0.001 (t -0.0, n 157) | +0.021 (t 1.1, n 144) |
| composite | 4 | 454 | +0.010 | 0.50 | [-0.028, +0.048] | [-0.031, +0.050] | +0.024 (t 0.6, n 156) | -0.014 (t -0.4, n 157) | +0.020 (t 0.8, n 141) |
| composite | 12 | 446 | +0.010 | 0.35 | [-0.048, +0.068] | [-0.046, +0.064] | +0.014 (t 0.2, n 156) | -0.007 (t -0.1, n 157) | +0.027 (t 0.7, n 133) |
| mom12_1 | 1 | 457 | +0.025 | 1.89 | [-0.001, +0.052] | [+0.002, +0.048] | +0.038 (t 1.5, n 156) | -0.003 (t -0.1, n 157) | +0.043 (t 2.2, n 144) |
| mom12_1 | 4 | 454 | +0.033 | 1.68 | [-0.005, +0.071] | [+0.001, +0.063] | +0.040 (t 1.0, n 156) | +0.016 (t 0.5, n 157) | +0.043 (t 1.6, n 141) |
| mom12_1 | 12 | 446 | +0.034 | 1.14 | [-0.025, +0.093] | [-0.025, +0.088] | +0.023 (t 0.4, n 156) | +0.014 (t 0.3, n 157) | +0.070 (t 2.1, n 133) |

#### IC de Spearman, universo `crypto`

| factor | h | semanas | IC medio | t (NW) | IC 95 % (NW) | IC 95 % (bloques) | 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 408 | +0.021 | 1.09 | [-0.017, +0.058] | [-0.009, +0.050] | +0.054 (t 1.4, n 107) | +0.000 (t 0.0, n 157) | +0.018 (t 0.5, n 144) |
| technical | 4 | 405 | +0.014 | 0.57 | [-0.035, +0.064] | [-0.032, +0.065] | +0.070 (t 1.4, n 107) | +0.001 (t 0.0, n 157) | -0.013 (t -0.3, n 141) |
| technical | 12 | 397 | +0.011 | 0.33 | [-0.057, +0.080] | [-0.053, +0.072] | +0.087 (t 1.2, n 107) | -0.042 (t -0.9, n 157) | +0.013 (t 0.2, n 133) |
| momentum | 1 | 408 | +0.022 | 1.18 | [-0.014, +0.059] | [-0.011, +0.054] | +0.077 (t 2.0, n 107) | -0.017 (t -0.6, n 157) | +0.024 (t 0.8, n 144) |
| momentum | 4 | 405 | -0.002 | -0.07 | [-0.058, +0.054] | [-0.057, +0.057] | +0.051 (t 0.9, n 107) | -0.011 (t -0.3, n 157) | -0.032 (t -0.7, n 141) |
| momentum | 12 | 397 | -0.000 | -0.01 | [-0.079, +0.078] | [-0.078, +0.074] | +0.083 (t 1.0, n 107) | -0.012 (t -0.2, n 157) | -0.055 (t -0.7, n 133) |
| risk | 1 | 408 | +0.080 | 4.10 | [+0.042, +0.118] | [+0.045, +0.113] | -0.004 (t -0.1, n 107) | +0.095 (t 3.3, n 157) | +0.126 (t 3.7, n 144) |
| risk | 4 | 405 | +0.114 | 3.85 | [+0.056, +0.172] | [+0.048, +0.178] | -0.006 (t -0.1, n 107) | +0.187 (t 4.5, n 157) | +0.123 (t 2.5, n 141) |
| risk | 12 | 397 | +0.200 | 4.20 | [+0.106, +0.293] | [+0.095, +0.296] | -0.006 (t -0.1, n 107) | +0.283 (t 6.2, n 157) | +0.267 (t 3.2, n 133) |
| composite | 1 | 408 | +0.064 | 3.23 | [+0.025, +0.103] | [+0.030, +0.097] | +0.103 (t 2.8, n 107) | +0.025 (t 0.8, n 157) | +0.078 (t 2.2, n 144) |
| composite | 4 | 405 | +0.068 | 2.38 | [+0.012, +0.123] | [+0.012, +0.124] | +0.120 (t 2.1, n 107) | +0.060 (t 1.4, n 157) | +0.036 (t 0.7, n 141) |
| composite | 12 | 397 | +0.089 | 2.21 | [+0.010, +0.169] | [+0.012, +0.162] | +0.122 (t 1.4, n 107) | +0.061 (t 1.1, n 157) | +0.096 (t 1.3, n 133) |
| mom12_1 | 1 | 408 | +0.009 | 0.49 | [-0.028, +0.046] | [-0.025, +0.043] | +0.033 (t 1.0, n 107) | +0.003 (t 0.1, n 157) | -0.002 (t -0.1, n 144) |
| mom12_1 | 4 | 405 | -0.027 | -0.89 | [-0.085, +0.032] | [-0.082, +0.027] | -0.006 (t -0.1, n 107) | +0.003 (t 0.1, n 157) | -0.074 (t -1.4, n 141) |
| mom12_1 | 12 | 397 | -0.052 | -1.19 | [-0.137, +0.033] | [-0.132, +0.029] | +0.055 (t 0.8, n 107) | -0.012 (t -0.2, n 157) | -0.184 (t -2.5, n 133) |

#### Puntuación aleatoria (200 semillas), universo `equity`

| h | IC medio (media de semillas) | sd entre semillas | p2,5 / p97,5 | semillas con \|t\|>1,96 | \|t\|>2,87 | LS bruto anual | LS neto anual (sd) |
|---|---|---|---|---|---|---|---|
| 1 | -0.000 | 0.004 | -0.008 / +0.009 | 4.5 % | 0.0 % | +0.2% (+2.5%) | -17.0% (+2.5%) |
| 4 | +0.000 | 0.004 | -0.008 / +0.008 | 3.0 % | 0.0 % | +0.1% (+1.1%) | -4.6% (+1.1%) |
| 12 | -0.000 | 0.004 | -0.008 / +0.008 | 3.0 % | 0.5 % | -0.0% (+0.8%) | -1.9% (+0.8%) |

#### Regla de decisión a priori (universo `equity`)

(i) |t_NW| ≥ 2,87; (ii) IC medio > 0 en los tres subperiodos; (iii) IC medio > 0; (iv) cartera LS neta > 0.

| factor | h | IC | t | (i) | (ii) | (iii) | LS neto anual | (iv) | ¿señal? |
|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | +0.017 | 1.52 | no | sí | sí | +0.6% | sí | no |
| technical | 4 | +0.017 | 1.10 | no | sí | sí | -0.5% | no | no |
| technical | 12 | +0.026 | 1.13 | no | sí | sí | +0.8% | sí | no |
| momentum | 1 | +0.019 | 1.54 | no | sí | sí | +2.7% | sí | no |
| momentum | 4 | +0.018 | 0.98 | no | no | sí | +0.7% | sí | no |
| momentum | 12 | +0.011 | 0.43 | no | no | sí | -0.4% | no | no |
| risk | 1 | -0.017 | -1.37 | no | no | no | -17.6% | no | no |
| risk | 4 | -0.035 | -1.83 | no | no | no | -17.0% | no | no |
| risk | 12 | -0.053 | -1.79 | no | no | no | -15.7% | no | no |
| composite | 1 | +0.013 | 1.01 | no | sí | sí | -3.7% | no | no |
| composite | 4 | +0.008 | 0.44 | no | no | sí | -5.5% | no | no |
| composite | 12 | +0.002 | 0.09 | no | no | sí | -5.2% | no | no |

Pruebas de la familia que cumplen todo: 0 de 12.

#### Cartera quintil superior − inferior, universo `equity` (coste 10 pb por operación, préstamo 0.5 %/año)

| factor | h | cohortes | bruto anual | neto anual | vol anual | Sharpe neto | t neto (NW) | rotación | solo largos neto vs índice | neto 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 457 | +7.4% | +0.6% | 17.7% | 0.03 | 0.10 | 30% | -0.6% | -1.0% (t -0.1) | +2.8% (t 0.3) | -0.0% (t -0.0) |
| technical | 4 | 454 | +2.3% | -0.5% | 16.5% | -0.03 | -0.12 | 45% | -0.1% | +2.5% (t 0.3) | -4.1% (t -0.6) | +0.1% (t 0.0) |
| technical | 12 | 446 | +2.4% | +0.8% | 15.9% | 0.05 | 0.22 | 62% | +0.7% | +3.6% (t 0.4) | -1.1% (t -0.2) | -0.2% (t -0.0) |
| momentum | 1 | 457 | +7.5% | +2.7% | 19.8% | 0.14 | 0.41 | 21% | +2.3% | +3.3% (t 0.2) | -1.6% (t -0.2) | +6.8% (t 0.6) |
| momentum | 4 | 454 | +3.4% | +0.7% | 19.0% | 0.04 | 0.14 | 41% | +1.3% | +7.7% (t 0.7) | -8.2% (t -1.0) | +2.9% (t 0.4) |
| momentum | 12 | 446 | +1.2% | -0.4% | 17.8% | -0.02 | -0.09 | 60% | +0.7% | +5.7% (t 0.6) | -4.7% (t -0.8) | -2.4% (t -0.4) |
| risk | 1 | 457 | -15.2% | -17.6% | 20.5% | -0.86 | -2.55 | 9% | -8.7% | -20.0% (t -1.6) | -17.6% (t -1.4) | -15.1% (t -1.5) |
| risk | 4 | 454 | -15.3% | -17.0% | 20.2% | -0.84 | -2.96 | 22% | -7.6% | -20.1% (t -1.9) | -17.2% (t -1.7) | -13.4% (t -1.5) |
| risk | 12 | 446 | -14.6% | -15.7% | 19.6% | -0.80 | -3.10 | 33% | -6.9% | -21.3% (t -2.2) | -13.1% (t -1.6) | -12.1% (t -1.6) |
| composite | 1 | 457 | +1.0% | -3.7% | 20.3% | -0.18 | -0.53 | 20% | -3.6% | +2.0% (t 0.1) | -7.4% (t -0.7) | -5.8% (t -0.6) |
| composite | 4 | 454 | -3.1% | -5.5% | 19.7% | -0.28 | -1.02 | 37% | -2.9% | +1.3% (t 0.1) | -11.7% (t -1.3) | -6.3% (t -0.9) |
| composite | 12 | 446 | -3.8% | -5.2% | 17.4% | -0.30 | -1.23 | 56% | -2.6% | -2.6% (t -0.3) | -7.2% (t -1.1) | -6.0% (t -1.2) |
| mom12_1 | 1 | 457 | +6.6% | +3.9% | 21.7% | 0.18 | 0.54 | 11% | +5.0% | +6.0% (t 0.4) | -10.8% (t -0.9) | +17.6% (t 1.6) |
| mom12_1 | 4 | 454 | +5.5% | +3.9% | 20.2% | 0.19 | 0.69 | 21% | +5.4% | +5.3% (t 0.5) | -4.0% (t -0.5) | +11.0% (t 1.4) |
| mom12_1 | 12 | 446 | +5.1% | +3.9% | 19.6% | 0.20 | 0.74 | 37% | +6.2% | +1.3% (t 0.1) | +1.3% (t 0.2) | +10.1% (t 1.5) |

#### Cartera quintil superior − inferior, universo `etf` (coste 10 pb por operación, préstamo 0.5 %/año)

| factor | h | cohortes | bruto anual | neto anual | vol anual | Sharpe neto | t neto (NW) | rotación | solo largos neto vs índice | neto 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 457 | +5.5% | -2.1% | 17.0% | -0.12 | -0.37 | 34% | -5.3% | +2.4% (t 0.2) | +2.6% (t 0.3) | -12.0% (t -1.5) |
| technical | 4 | 454 | +0.6% | -2.4% | 15.3% | -0.16 | -0.61 | 47% | -4.1% | +3.2% (t 0.4) | -2.9% (t -0.5) | -7.9% (t -1.4) |
| technical | 12 | 446 | -1.3% | -2.9% | 14.4% | -0.20 | -0.97 | 62% | -3.7% | +3.4% (t 0.6) | -3.1% (t -0.7) | -9.9% (t -2.5) |
| momentum | 1 | 457 | +5.1% | +0.2% | 18.9% | 0.01 | 0.04 | 21% | -2.4% | +1.5% (t 0.1) | +2.5% (t 0.3) | -3.6% (t -0.4) |
| momentum | 4 | 454 | -0.9% | -3.5% | 17.2% | -0.21 | -0.77 | 41% | -3.7% | +0.9% (t 0.1) | -6.8% (t -0.9) | -4.8% (t -0.7) |
| momentum | 12 | 446 | -3.9% | -5.5% | 15.4% | -0.35 | -1.69 | 60% | -3.8% | -0.2% (t -0.0) | -7.0% (t -1.4) | -9.8% (t -2.2) |
| risk | 1 | 457 | -11.5% | -13.7% | 18.1% | -0.76 | -2.36 | 8% | -6.9% | -3.1% (t -0.3) | -14.4% (t -1.6) | -24.5% (t -2.7) |
| risk | 4 | 454 | -10.3% | -11.8% | 16.2% | -0.73 | -2.61 | 19% | -6.9% | -4.5% (t -0.5) | -9.6% (t -1.4) | -22.2% (t -3.3) |
| risk | 12 | 446 | -9.3% | -10.3% | 15.0% | -0.69 | -2.77 | 29% | -6.0% | -4.7% (t -0.6) | -6.0% (t -1.5) | -21.9% (t -3.6) |
| composite | 1 | 457 | +2.6% | -2.4% | 18.5% | -0.13 | -0.40 | 22% | -3.9% | +3.1% (t 0.2) | -2.0% (t -0.2) | -8.9% (t -1.0) |
| composite | 4 | 454 | -2.1% | -4.6% | 17.2% | -0.27 | -1.00 | 39% | -4.6% | +1.9% (t 0.2) | -6.4% (t -0.8) | -9.8% (t -1.6) |
| composite | 12 | 446 | -4.8% | -6.3% | 15.4% | -0.41 | -1.90 | 58% | -4.9% | +0.6% (t 0.1) | -6.4% (t -1.3) | -14.4% (t -5.0) |
| mom12_1 | 1 | 457 | +4.8% | +1.7% | 19.7% | 0.09 | 0.27 | 12% | +1.2% | +0.4% (t 0.0) | -3.0% (t -0.3) | +8.3% (t 0.9) |
| mom12_1 | 4 | 454 | +3.8% | +2.1% | 18.0% | 0.12 | 0.42 | 23% | +1.8% | +2.7% (t 0.3) | +0.5% (t 0.1) | +3.3% (t 0.5) |
| mom12_1 | 12 | 446 | +0.9% | -0.3% | 16.9% | -0.02 | -0.07 | 39% | +1.0% | -0.7% (t -0.1) | +0.8% (t 0.1) | -1.2% (t -0.4) |

#### Cartera quintil superior − inferior, universo `stock` (coste 10 pb por operación, préstamo 0.5 %/año)

| factor | h | cohortes | bruto anual | neto anual | vol anual | Sharpe neto | t neto (NW) | rotación | solo largos neto vs índice | neto 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 457 | +9.3% | +2.3% | 20.3% | 0.11 | 0.34 | 31% | +1.7% | -3.8% (t -0.3) | +5.8% (t 0.6) | +5.1% (t 0.4) |
| technical | 4 | 454 | +2.9% | -0.0% | 19.2% | -0.00 | -0.01 | 47% | +1.4% | +2.3% (t 0.2) | -2.3% (t -0.3) | -0.1% (t -0.0) |
| technical | 12 | 446 | +3.9% | +2.3% | 18.4% | 0.12 | 0.52 | 64% | +2.6% | +3.1% (t 0.3) | +1.5% (t 0.2) | +2.2% (t 0.4) |
| momentum | 1 | 457 | +9.8% | +4.9% | 22.7% | 0.22 | 0.65 | 21% | +5.8% | +8.3% (t 0.6) | -1.7% (t -0.1) | +8.5% (t 0.6) |
| momentum | 4 | 454 | +5.4% | +2.8% | 22.3% | 0.12 | 0.45 | 42% | +4.3% | +13.5% (t 1.2) | -9.7% (t -0.9) | +4.8% (t 0.5) |
| momentum | 12 | 446 | +4.3% | +2.8% | 20.9% | 0.13 | 0.54 | 61% | +3.0% | +9.9% (t 0.9) | -2.9% (t -0.4) | +1.0% (t 0.1) |
| risk | 1 | 457 | -18.1% | -21.0% | 27.1% | -0.77 | -2.25 | 11% | -8.3% | -25.3% (t -1.6) | -19.4% (t -1.2) | -18.1% (t -1.1) |
| risk | 4 | 454 | -18.3% | -20.1% | 27.4% | -0.73 | -2.54 | 26% | -7.5% | -23.1% (t -1.7) | -18.7% (t -1.3) | -18.5% (t -1.3) |
| risk | 12 | 446 | -15.1% | -16.2% | 27.6% | -0.59 | -2.24 | 36% | -5.6% | -23.5% (t -1.9) | -12.3% (t -1.0) | -12.3% (t -1.0) |
| composite | 1 | 457 | +1.9% | -2.8% | 23.9% | -0.12 | -0.34 | 20% | -2.9% | +1.4% (t 0.1) | -8.3% (t -0.6) | -1.3% (t -0.1) |
| composite | 4 | 454 | -1.3% | -3.8% | 23.9% | -0.16 | -0.57 | 37% | -1.2% | +3.3% (t 0.3) | -13.2% (t -1.2) | -1.2% (t -0.1) |
| composite | 12 | 446 | -1.5% | -3.0% | 21.5% | -0.14 | -0.54 | 56% | -1.0% | -1.7% (t -0.1) | -6.8% (t -0.8) | +0.2% (t 0.0) |
| mom12_1 | 1 | 457 | +7.9% | +5.0% | 25.1% | 0.20 | 0.60 | 11% | +6.4% | +10.5% (t 0.7) | -12.2% (t -0.9) | +17.8% (t 1.3) |
| mom12_1 | 4 | 454 | +5.4% | +3.7% | 24.1% | 0.16 | 0.55 | 22% | +6.4% | +6.0% (t 0.5) | -4.8% (t -0.4) | +10.8% (t 1.0) |
| mom12_1 | 12 | 446 | +6.6% | +5.5% | 23.6% | 0.23 | 0.86 | 39% | +8.1% | +2.4% (t 0.2) | +0.8% (t 0.1) | +14.6% (t 1.4) |

#### Cartera quintil superior − inferior, universo `crypto` (coste 25 pb por operación, préstamo 10.0 %/año)

| factor | h | cohortes | bruto anual | neto anual | vol anual | Sharpe neto | t neto (NW) | rotación | solo largos neto vs índice | neto 2018-20 | 2021-23 | 2024-hoy |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| technical | 1 | 408 | +14.2% | -20.0% | 77.3% | -0.26 | -0.67 | 47% | +8.9% | +81.8% (t 1.2) | -46.3% (t -1.2) | -67.1% (t -1.2) |
| technical | 4 | 405 | -36.9% | -54.5% | 114.2% | -0.48 | -1.93 | 59% | -0.3% | +25.8% (t 0.5) | -99.8% (t -1.8) | -65.0% (t -1.7) |
| technical | 12 | 397 | -14.0% | -27.1% | 225.9% | -0.12 | -0.87 | 72% | +31.9% | +34.4% (t 0.3) | -81.0% (t -1.9) | -12.9% (t -0.7) |
| momentum | 1 | 408 | +7.8% | -16.0% | 97.0% | -0.16 | -0.47 | 26% | +14.6% | +124.0% (t 1.7) | -91.0% (t -1.4) | -38.2% (t -1.1) |
| momentum | 4 | 405 | -6.5% | -22.8% | 134.8% | -0.17 | -0.73 | 48% | +28.2% | +95.6% (t 1.9) | -41.1% (t -0.8) | -92.1% (t -1.7) |
| momentum | 12 | 397 | -33.7% | -46.6% | 200.5% | -0.23 | -1.29 | 66% | +29.8% | +32.8% (t 0.3) | -39.8% (t -1.4) | -118.5% (t -1.9) |
| risk | 1 | 408 | +27.7% | +7.8% | 79.3% | 0.10 | 0.26 | 19% | +22.4% | -63.7% (t -1.3) | +65.0% (t 1.1) | -1.4% (t -0.0) |
| risk | 4 | 405 | +38.5% | +23.0% | 130.0% | 0.18 | 0.65 | 42% | +32.0% | -40.0% (t -1.2) | +95.4% (t 1.2) | -9.6% (t -0.3) |
| risk | 12 | 397 | +93.8% | +81.5% | 272.2% | 0.30 | 1.35 | 52% | +90.9% | +32.1% (t 0.3) | +169.8% (t 1.5) | +17.1% (t 0.7) |
| composite | 1 | 408 | +28.1% | +2.6% | 77.0% | 0.03 | 0.09 | 30% | +24.3% | +129.7% (t 1.8) | -36.3% (t -1.0) | -49.5% (t -1.1) |
| composite | 4 | 405 | -9.3% | -24.9% | 115.0% | -0.22 | -0.94 | 43% | +27.6% | +79.6% (t 1.9) | -71.8% (t -1.4) | -52.0% (t -1.4) |
| composite | 12 | 397 | +24.2% | +11.7% | 194.3% | 0.06 | 0.43 | 58% | +47.5% | +85.3% (t 1.1) | -14.2% (t -0.6) | -16.9% (t -0.7) |
| mom12_1 | 1 | 408 | -5.4% | -21.6% | 95.9% | -0.23 | -0.66 | 12% | +32.6% | +12.3% (t 0.2) | +29.4% (t 0.5) | -102.5% (t -2.2) |
| mom12_1 | 4 | 405 | -10.0% | -23.4% | 146.6% | -0.16 | -0.57 | 26% | +43.0% | +103.5% (t 1.9) | -9.0% (t -0.1) | -135.8% (t -1.9) |
| mom12_1 | 12 | 397 | -23.0% | -35.1% | 161.3% | -0.22 | -1.07 | 47% | +27.4% | +61.5% (t 0.9) | -25.3% (t -0.7) | -124.3% (t -2.5) |

#### Sensibilidad a costes, compuesto, universo `equity` (LS neto anual)

| h | 0 pb | 5 pb | 10 pb | 20 pb |
|---|---|---|---|---|
| 1 | +0.5% | -1.6% | -3.7% | -7.9% |
| 4 | -3.6% | -4.6% | -5.5% | -7.5% |
| 12 | -4.3% | -4.7% | -5.2% | -6.2% |

#### Exceso medio por quintil (Q1 = peor puntuación), universo `equity`, por semana de h semanas

| factor | h | Q1 | Q2 | Q3 | Q4 | Q5 |
|---|---|---|---|---|---|---|
| composite | 1 | -0.1% | +0.0% | +0.0% | +0.0% | -0.0% |
| composite | 4 | +0.1% | -0.0% | +0.1% | -0.0% | -0.1% |
| composite | 12 | +0.4% | +0.0% | +0.4% | -0.1% | -0.5% |
| mom12_1 | 1 | -0.0% | -0.1% | +0.0% | -0.0% | +0.1% |
| mom12_1 | 4 | +0.0% | -0.3% | -0.0% | -0.1% | +0.5% |
| mom12_1 | 12 | +0.3% | -0.5% | -0.5% | -0.5% | +1.5% |

#### Acierto por veredicto, h = 1 semanas, universo `equity` (n = 46533, tasa base de superar al índice 48.9 %)

| veredicto | n | acierto | acierto de referencia | diferencia | IC 95 % de la diferencia (bloques) |
|---|---|---|---|---|---|
| buy | 3229 | 48.7% | 48.9% | -0.2% | [-3.0%, +2.6%] |
| accumulate | 16466 | 48.4% | 48.9% | -0.5% | [-1.3%, +0.4%] |
| hold | 12158 | 58.9% | 58.4% | +0.5% | [-1.2%, +1.9%] |
| wait | 7587 | 55.5% | 58.4% | -3.0% | [-5.0%, -1.1%] |
| reduce | 4962 | 50.8% | 51.1% | -0.3% | [-2.4%, +1.7%] |
| avoid | 2131 | 48.6% | 51.1% | -2.6% | [-6.2%, +0.5%] |

#### Acierto por veredicto, h = 4 semanas, universo `equity` (n = 46227, tasa base de superar al índice 47.7 %)

| veredicto | n | acierto | acierto de referencia | diferencia | IC 95 % de la diferencia (bloques) |
|---|---|---|---|---|---|
| buy | 3219 | 47.0% | 47.7% | -0.7% | [-4.2%, +3.1%] |
| accumulate | 16373 | 46.4% | 47.7% | -1.4% | [-3.0%, +0.3%] |
| hold | 12048 | 56.5% | 56.2% | +0.3% | [-1.5%, +2.0%] |
| wait | 7541 | 53.6% | 56.2% | -2.6% | [-4.4%, -0.7%] |
| reduce | 4923 | 51.0% | 52.3% | -1.3% | [-4.1%, +1.4%] |
| avoid | 2123 | 47.3% | 52.3% | -4.9% | [-10.3%, +0.6%] |

#### Acierto por veredicto, h = 12 semanas, universo `equity` (n = 45411, tasa base de superar al índice 46.3 %)

| veredicto | n | acierto | acierto de referencia | diferencia | IC 95 % de la diferencia (bloques) |
|---|---|---|---|---|---|
| buy | 3173 | 45.4% | 46.3% | -0.9% | [-6.9%, +6.3%] |
| accumulate | 16036 | 44.8% | 46.3% | -1.5% | [-4.1%, +0.9%] |
| hold | 11820 | 54.2% | 53.7% | +0.4% | [-2.2%, +2.7%] |
| wait | 7421 | 49.5% | 53.7% | -4.3% | [-6.6%, -1.9%] |
| reduce | 4860 | 53.0% | 53.7% | -0.7% | [-5.5%, +4.2%] |
| avoid | 2101 | 47.0% | 53.7% | -6.6% | [-13.6%, -0.5%] |
