# ¿Predicen algo las banderas del perfil de riesgo de una OPV? (estudio histórico)

Estado: **diseño fijado el 19-oct-2026 ANTES de descargar o ejecutar nada.** Cualquier cambio posterior se anota en «Desviaciones» con su
motivo (cambiarlo tras ver resultados es data snooping y se declara como tal). Modelo de disciplina: `docs/notes/backtest.md`.

Código: lógica pura con tests en `backend/crates/market/src/ipo_study.rs`; descarga y ejecución en
`backend/crates/market/examples/ipo_study.rs` (`cargo run --release -p market --example ipo_study -- download|run`).

## Pregunta

El perfil de riesgo de `ipo_risk::build_profile` es determinista y se calcula con el folleto 424B4, que se presenta al fijar el precio.
Por tanto el perfil de una OPV pasada **es información disponible en aquel momento** (el contenido del folleto queda fijado en el
precio; se presenta como mucho 2 días hábiles después). Pregunta: ¿se asocia cada bandera con un **exceso de rentabilidad frente a SPY**
distinto en las OPV que la tienen y en las que no? Es una pregunta descriptiva: «histórico, no predicción».

## Universo (fijo)

- OPV de EE. UU. del calendario de **Finnhub** con `status = priced`, símbolo, precio definitivo (`priceLow = priceHigh`, ≥ 1 USD) y
  **sin SPAC** (`spac_likely`, heurística de `finnhub.rs`; la SPAC que se cuela es un sesgo conocido), con fecha de primera cotización
  entre **F − 36 meses y F − 3 meses**, siendo F la fecha de la descarga (así toda OPV tiene al menos el horizonte de 3 meses).
- Respaldo de EDGAR: **no aplica** (el índice de texto completo no da precio ni símbolo). Sin clave de Finnhub no hay estudio.
- Folleto: el definitivo de la OPV (424B4 / 424B1 / 253G2, elegido por `ProspectusPick::Initial`), buscado por símbolo y, si falla, por
  nombre de empresa. Se acepta si `filed_on` está entre **−10 y +3 días** de la fecha de primera cotización y no es preliminar. Si no,
  la OPV se excluye **y se cuenta con su motivo** (sin folleto, folleto fuera de fecha, error de descarga).
- Rendimiento: cierres diarios de **Yahoo** (símbolo y SPY). Si Yahoo no tiene el símbolo, la OPV se trata aparte (ver sesgos).

## Variables (umbrales fijados ahora; 10 banderas)

Cada bandera es verdadero/falso cuando el indicador subyacente se extrajo; si el indicador queda «no extraído», la OPV **no entra en el
contraste de esa bandera** (se informa el n). Para `voting` y `controlled`, «extraído sin coincidencias» cuenta como falso (la ausencia de
coincidencia no prueba que no exista: error de clasificación que sesga hacia el nulo).

| id | bandera | definición |
|---|---|---|
| V1 | `secondary_gt50` | venta secundaria (`secondary_sale`) > 50 % de lo ofrecido |
| V2 | `debt_repayment` | bandera «devolver deuda» en `use_of_proceeds` |
| V3 | `multi_vote` | bandera de voto múltiple (`voting`, ≥ 2 votos por acción en alguna clase) |
| V4 | `controlled` | bandera «controlled company» |
| V5 | `dilution_high` | dilución por acción ≥ **75 %** del precio de la OPV (dilución > 0; precio del calendario) |
| V6 | `float_low` | acciones ofrecidas / acciones tras la oferta (`float_offered`) < **15 %** |
| V7 | `net_loss` | pérdidas en todos los periodos del resumen financiero (`net_result` con bandera `net_loss`) |
| V8 | `lockup_short` | lock-up (`lock_up`, un solo plazo en días) < **180 días** |
| V9 | `size_micro` | importe de la oferta < **50 M USD** (frente al resto) |
| V10 | `size_large` | importe de la oferta ≥ **250 M USD** (frente al resto) |

Los umbrales 75 %, 15 % y 180 días se eligen por criterio (la dilución se come ≥ 3/4 del precio; flotante de menos de 1/7; 180 días es el
estándar de mercado) **sin haber mirado ningún rendimiento**. No se afinarán.

## Resultados (fijos)

Para cada OPV y horizonte h ∈ {3, 6, 12} meses, con `ipo::compute` (mismas reglas que la app; horizonte alcanzado si el objetivo
≤ última sesión descargada):

- **Principal**: exceso a **6 meses desde el precio de colocación** = rentabilidad del activo desde el precio de la OPV − rentabilidad de
  SPY (cierre ajustado) desde el cierre previo a la OPV hasta la misma sesión.
- Secundarios (exploratorios): exceso a 3 y a 12 meses desde la colocación.
- **Desde el cierre del día 1** (lo que puede comprar un inversor normal): rentabilidad del activo desde el cierre del primer día − la de SPY
  (cierre ajustado) entre ese mismo día y la misma sesión de salida, a 3, 6 y 12 meses.

Medianas, no medias (las colas de las OPV son enormes). Sin winsorizar.

## Contraste

Por bandera y resultado: **diferencia de medianas (con − sin bandera)**.

**Estratificado por tamaño (V1–V8; enmienda 1).** El tamaño de la oferta domina el rendimiento (medido antes de este estudio: a 3 meses, mediana −29 % en
< 50 M USD y +12 % en ≥ 250 M) y se correlaciona con casi todas las banderas; un contraste sin estratificar atribuiría a la bandera lo que es tamaño. Para
V1–V8 el contraste **principal** es:
- estadístico: **media ponderada por n (n con + n sin) de las diferencias de medianas dentro de cada tramo** (< 50 M, 50–250 M, ≥ 250 M USD), usando solo los tramos con
  **n ≥ 5 en ambos grupos**; las OPV sin tamaño calculable no entran; el n que se informa es el de los tramos usados;
- p de dos colas: **permutación de la bandera dentro de cada tramo** (20 000 permutaciones);
- IC 95 %: **bootstrap percentil** (10 000 réplicas) que remuestrea **dentro de cada tramo y grupo**;
- V9 y V10 (el tamaño mismo) **no se estratifican**: diferencia de medianas directa, p por permutación simple e IC bootstrap simple.
- Para V1–V8 se da además, **solo como descriptivo**, el contraste sin estratificar (n, medianas, diferencia, IC y p sin ajustar) para ver cuánto cambia.

**Dependencia temporal (enmienda 2).** Las OPV se agrupan en mercados calientes y sus excesos están correlacionados dentro del mismo periodo, de modo que el
bootstrap ordinario (cada OPV independiente) da IC demasiado estrechos. Se añade un **IC bootstrap por conglomerados de mes de salida a bolsa** (se remuestrean meses
enteros con todas sus OPV; 5 000 réplicas; se recalcula el mismo estadístico, estratificado o no, que el análisis principal). Hace falta ≥ 3 meses distintos.

Común a ambos:
- semilla `20261019` en todo; n de cada grupo siempre visible; un grupo con **n < 10** (suma de los tramos usados) → «no contrastable».
- **Corrección por comparaciones múltiples: Holm** (control de FWER) sobre las 10 banderas, **por separado para cada resultado**. La
  familia que decide es la del resultado principal (10 pruebas). Los demás resultados son exploratorios, cada uno con su Holm.
- **Correlación entre banderas** (enmienda 3): matriz phi 10×10 con el n de cada solape, para leer los resultados con la confusión a la vista.
- **Referencia de azar**: 200 asignaciones aleatorias de bandera por cada prevalencia observada (permuta la bandera dentro de cada tramo en V1–V8; permutación con 1 000 réplicas, sin IC):
  proporción con p ajustada < 0,05 (debería ser ≤ 5 %).
- **Potencia**: efecto mínimo detectable por bandera, MDE = (z₀,₉₉₇₅ + z₀,₈₀) · √(se₁² + se₀²), con se = 1,2533 · σ / √n, σ = 1,4826 · MAD del resultado
  principal (α = 0,05/10 de Bonferroni, potencia 80 %). Aproximación normal; sirve como orden de magnitud.

## Regla de decisión a priori

Una bandera «se asocia» con el exceso solo si se cumplen **todas**:
1. p ajustado (Holm) < 0,05 en el resultado principal (6 m desde colocación);
2. **ambos** IC 95 % de la diferencia (el bootstrap ordinario y el de conglomerados de mes) excluyen 0;
3. n ≥ 10 en ambos grupos;
4. el signo de la diferencia coincide a 3 y a 12 meses desde la colocación (cuando ambos grupos tienen n ≥ 10 en ese horizonte).
Para decir que es **aprovechable por un inversor normal** se exige además que el IC ordinario de la diferencia a 6 meses **desde el cierre del día 1**
excluya 0 con el mismo signo. La regla se aplica al contraste estratificado (V1–V8) o directo (V9, V10), nunca al descriptivo sin estratificar. Si ninguna bandera cumple 1-4: **«no se distingue del azar»**, y no se muestra nada de esto en la ficha.

## Sesgos y límites (declarados ahora)

- **Supervivencia**: las OPV deslistadas pueden faltar en Yahoo, y las empresas renombradas o deslistadas pierden su ticker en el mapa de la SEC. Se
  **cuentan** (por motivo, y su reparto por bandera y por tramo de tamaño) y se tratan **aparte**: no entran en los contrastes. El sesgo
  esperado es optimista (faltan los peores).
- **Un solo régimen de mercado** (2023–2026) y **una sola muestra**: no hay subperiodos independientes fiables.
- **Muestra pequeña**: con unas decenas por grupo, solo efectos grandes son detectables (MDE en el resultado).
- Banderas **correlacionadas** entre sí y con el tamaño (confusión): no se ajusta; las diferencias son descriptivas, no causales.
- Errores de extracción (no diferenciales) sesgan hacia el nulo; no se corrigen.
- Rentabilidad de precio (sin dividendos, que en OPV son raros); SPY con cierre ajustado.
- La muestra es la que da Finnhub (puede omitir OPV pequeñas o por registro de reventa).

## Datos y reproducibilidad

Congelados **fuera del repo** en `~/.cache/invest-ai/ipo-study/` (ruta sobrescribible con `IPO_STUDY_DATA`): calendario, un fichero por OPV con el
folleto (gzip) y las series diarias, y un `manifest.json` con sha256 y fecha de descarga de cada fichero. La descarga es reanudable (por
fichero), con ≤ ~8 peticiones/s a la SEC (limitador del cliente) y 0,5 s entre peticiones a Yahoo. `run` solo lee ese directorio, verifica los
sha256, no usa red y es determinista (semillas fijas).

## Enmiendas al diseño (antes de ejecutar)

Hechas el **2026-10-19 12:55 CEST** (hora local del equipo), a petición de la revisión del diseño, **antes de descargar o ejecutar el análisis** y sin haber visto ningún resultado de este estudio
(el único dato previo es la tasa base por tamaño ya documentada en `ipo-risk.md`, que motiva la enmienda 1). Por eso van en el diseño y no en «Desviaciones».
1. Contraste principal estratificado por tramo de tamaño para V1–V8, con el sin estratificar como descriptivo.
2. IC adicional por conglomerados de mes de salida a bolsa; el criterio 2 de la regla de decisión exige que ambos IC excluyan 0.
3. Matriz de correlación (phi) entre banderas.

## Desviaciones

Anotadas el 19-oct-2026. Ninguna cambia el diseño ni se hizo mirando resultados:

1. **Error de código, corregido antes de ver ningún resultado.** La primera ejecución de `run` sobre los folletos reales hizo `panic` en `ipo_risk::duration_candidates`
   (índice dentro de un carácter multibyte, comillas tipográficas justo antes de «six months»). Es un fallo del código ya commiteado (afectaba también a la app con
   ciertos folletos); se corrigió con test de regresión (`lockup_scan_does_not_panic_on_non_ascii_before_a_spelled_out_number`). Ningún dato de salida se había
   generado antes. Como el árbol de trabajo lo comparten otros agentes y estaba sin compilar, el ejemplo se ejecutó sobre una copia de `HEAD` + los ficheros del estudio
   + esa corrección; el código es el mismo.
2. Sin otras desviaciones: universo, umbrales, resultados, contrastes, enmiendas, semillas y regla de decisión son los fijados arriba.

## Resultados (ejecución del 19-oct-2026)

Reproducir: `cargo run --release -p market --example ipo_study -- download` (una vez; reanudable) y `… -- run` (sin red; dos ejecuciones dan la salida idéntica byte a byte,
`cmp` verificado). Tests: `cargo test -p market --test ipo_study` (9) y `--lib ipo_study` (5).

**Datos congelados** (fuera del repo, `~/.cache/invest-ai/ipo-study/`, 45 MB, F = 2026-10-19, descarga el 19-oct-2026): 1 007 ficheros con sha256 en `manifest.json`
(sha256 del manifiesto `96a10ff283ebe5108162…`, del calendario `6dad6f97ea6df1baeda2…`).

**Universo**: 502 OPV candidatas del calendario de Finnhub (priced, símbolo, precio fijo ≥ 1 USD, sin SPAC; primera cotización entre F−36 m y F−3 m). **En el estudio: 410.**

| motivo de exclusión | n |
|---|---|
| folleto fuera de fecha o preliminar (p. ej. HYNE, MBX, SAG) | 39 |
| sin serie en Yahoo (**supervivencia**; 26 de 279 micro, 3 de 132 grandes, 0 de 91 medianas) | 29 |
| rendimiento no calculable (primera cotización lejos de la fecha del calendario, etc.) | 12 |
| sin folleto en la SEC (por símbolo ni por nombre) | 12 |

Las 29 sin serie son sobre todo micro-OPV: el sesgo de supervivencia (faltan probablemente las peores) afecta casi solo al tramo < 50 M. Hay 361 OPV con dato a 6 m y 308 a 12 m.
Cobertura de los indicadores (OPV con el indicador extraído): V2, V3, V4 y V9/V10 al 100 %; V5 86 %; V1 73 %; V6 72 %; V7 60 %; V8 53 %.
Prevalencias: V1 4 % (12 OPV), V2 23 %, V3 24 %, V4 54 %, V5 65 %, V6 60 %, V7 47 %, **V8 2 % (4 OPV)**, V9 51 %, V10 30 %.
**V1 (n = 9 con bandera en los tramos usados a 6 m) y V8 (ningún tramo con ≥ 5 en ambos grupos) no son contrastables**: se informan solo descriptivamente.

**Confusión con el tamaño (phi)**: V9 (micro) con V10 −0,66; con V6 (flotante bajo) +0,40 y con V2 (deuda) −0,33; V10 con V2 +0,38 y con V1 +0,20; V4 con V7 −0,39; V5 con V6 +0,46. El resto, |phi| ≤ 0,27.

### Resultado principal: exceso a 6 m desde la colocación (361 OPV; mediana global −36,8 pp; σ robusta 63,4 pp)

| bandera | análisis | n con / sin | diferencia (pp) | IC 95 % | IC conglom. | p | p Holm | MDE 80 % | sin estratificar: dif., p |
|---|---|---|---|---|---|---|---|---|---|
| V1 venta secundaria > 50 % | no contrastable | 9 / 62 | +35,7 | — | — | — | — | — | +30,9, 0,151 |
| V2 devolver deuda | estratificado | 79 / 282 | −18,8 | [−30,3, +9,1] | [−31,2, +10,6] | 0,062 | 0,311 | 37 | **+24,0**, 0,013 |
| V3 voto múltiple | estratificado | 91 / 270 | +0,3 | [−22,3, +17,4] | [−19,6, +18,1] | 0,968 | 1,000 | 35 | −2,9, 0,793 |
| V4 «controlled company» | estratificado | 203 / 158 | −7,1 | [−21,3, +10,9] | [−21,2, +9,4] | 0,342 | 1,000 | 31 | −12,3, 0,156 |
| V5 dilución ≥ 75 % | estratificado | 205 / 103 | −11,0 | [−24,9, +7,8] | [−26,3, +10,1] | 0,192 | 0,768 | 35 | −12,6, 0,156 |
| V6 capital ofrecido < 15 % | estratificado | 166 / 98 | −18,3 | [−37,8, −3,5] | [−39,8, −4,9] | 0,036 | 0,215 | 37 | −36,0, 0,000 |
| V7 pérdidas | estratificado | 96 / 117 | −8,2 | [−31,2, +9,1] | [−30,9, +12,2] | 0,379 | 1,000 | 40 | −9,8, 0,430 |
| V8 lock-up < 180 d | no contrastable | 0 / 0 | — | — | — | — | — | — | +14,7 (2 / 181) |
| V9 oferta < 50 M (directo) | directo | 203 / 158 | −50,7 | [−63,7, −38,0] | [−63,5, −39,6] | < 0,001 | < 0,001 | 31 | = |
| V10 oferta ≥ 250 M (directo) | directo | 92 / 269 | +39,1 | [+24,3, +64,1] | [+26,0, +63,3] | < 0,001 | 0,001 | 35 | = |

Resultados secundarios (3 m y 12 m desde la colocación; 3, 6 y 12 m desde el cierre del día 1; cada uno con su Holm): el patrón es el mismo, con **una excepción exploratoria**: **V6 (capital ofrecido < 15 %) a 12 m desde la colocación**
sale con diferencia **−32,2 pp** (n 141 / 53), IC ordinario [−57,0, −5,1], IC por conglomerados [−58,8, −5,4], p = 0,003 y **p Holm = 0,018**. Es la única de V1–V8 con p Holm < 0,05 en alguno de los seis resultados.
No es el resultado principal (la regla a priori se aplica al de 6 m), así que **no cuenta como señal**: a 6 m desde la colocación V6 da −18,3 pp con p Holm = 0,215 (y su IC ordinario excluye 0 por poco, [−37,8, −3,5]); desde el cierre del día 1, a 6 m p = 0,121 sin ajustar, y a 12 m p Holm = 0,121.
Los resultados a 6 y 12 m comparten OPV: no son evidencia independiente. Se lee como **pista exploratoria (hipótesis a confirmar con OPV nuevas), no como señal**.
V9 y V10 (el tamaño) salen significativos en todos los resultados salvo V10 a 3 m desde el cierre del día 1 (p Holm 0,208).
Desde el cierre del día 1 a 6 m: V9 −39,3 pp [−50,7, −27,1] y V10 +32,3 pp [+16,9, +42,2].

**Estratificar cambia el signo de V2.** Sin estratificar, «devolver deuda» parece una **señal buena** (+24,0 pp a 6 m, p = 0,013; +44,6 pp a 12 m): es puro tamaño (las OPV que devuelven deuda son
grandes, phi con V10 = +0,38). Dentro de cada tramo la diferencia es −18,8 pp con IC que incluye 0. Es el caso que la enmienda 1 quería evitar.

### Regla de decisión a priori

- **V1–V8: ninguna bandera se asocia** según la regla (ninguna con p Holm < 0,05 en el resultado principal; V1 y V8 ni siquiera son contrastables). V6 a 12 m es una pista exploratoria (ver arriba), no una excepción a la regla.
- **V9 y V10 sí cumplen la regla, y también «aprovechable por un inversor normal»** (IC desde el cierre del día 1 excluye 0 con el mismo signo): pero son el **tamaño de la oferta**, no
  una bandera del folleto. Confirma lo que ya mostraba la tasa base por tramos.
- **Referencia de azar**: banderas aleatorias con la misma prevalencia (permutadas dentro de tramo) dan p Holm < 0,05 en el 0,3 % de los casos: el procedimiento es conservador (no genera falsos positivos).

### Potencia

Con n ≈ 80–200 por grupo, σ robusta de 63 pp y α = 0,05/10, el efecto mínimo detectable al 80 % de potencia es de **unos 27–40 pp** (tabla). Efectos de la mitad de ese tamaño
son invisibles para este estudio: **«no se distingue del azar» no equivale a «no hay efecto»**; lo que se puede decir es que, si existe, no es grande (en el resultado principal el mayor efecto de V1–V8, V2 −19 pp y V6 −18 pp, queda por debajo del MDE; el −32 pp de V6 a 12 m queda por debajo del MDE aproximado de 40 pp y sale significativo: el MDE es una aproximación normal conservadora, orden de magnitud).

### Conclusión

1. **Lo que más se asocia con el rendimiento de una OPV es su tamaño**: las < 50 M tienen una mediana de exceso a 6 m ~51 pp peor que el resto y las ≥ 250 M ~39 pp mejor. Eso ya se usa en la tasa base por tramos.
2. **Según la regla a priori, ninguna bandera del perfil de riesgo (V1–V8) se asocia con el exceso una vez controlado el tamaño** y corregidas las comparaciones múltiples en el resultado principal. La única pista es
   **V6 (capital ofrecido < 15 %)**: negativa en todos los horizontes y, solo a 12 m desde la colocación (resultado exploratorio), con p Holm 0,018 e IC ordinario y por conglomerados que excluyen 0 (−32 pp). No sobrevive en el
   principal (Holm 0,215) ni se mantiene desde el cierre del día 1 (Holm 0,121 a 12 m): queda como hipótesis a confirmar con OPV posteriores, no como hallazgo. V1 y V8 no se pueden contrastar por falta de casos.
3. Por tanto **no se propone mostrar en la ficha ningún dato histórico asociado a banderas** (la regla a priori lo impide): hacerlo sería presentar ruido como señal. Las banderas siguen siendo
   hechos verificables del folleto (qué dice, no qué pasará), y el tramo de tamaño y su tasa base, que sí se distinguen, ya están en la ficha.

### Límites de este resultado

- **Supervivencia**: 29 OPV sin serie en Yahoo (26 micro) y 12 sin folleto quedan fuera; si faltan sobre todo desastres, las diferencias reales del tramo micro están subestimadas. Las 39 con folleto
  «fuera de fecha o preliminar» pueden incluir OPV cuya primera cotización del calendario no coincide con la presentación (p. ej. conversiones o registros de reventa).
- **Un régimen** (finales de 2023 – 2026) y rendimientos extremos en micro-OPV (mediana a 12 m de −73 pp): la dispersión es enorme y limita la potencia.
- **Error de extracción**: V1, V5, V6, V7 y V8 solo cubren el 53–86 % de las OPV (indicador extraído) y V8 tiene 4 positivos: la bandera no es contrastable con estas reglas de extracción.
- Banderas correlacionadas entre sí y con el tamaño: la estratificación controla el tamaño, no el resto de la confusión.
- «No extraído» no es aleatorio (las micro-OPV con folleto escueto tienen más huecos); se informan las coberturas.
