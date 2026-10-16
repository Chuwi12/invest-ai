# Perfil de riesgo de una OPV (determinista, sin LLM)

Código: `backend/crates/market/src/ipo_risk.rs` (extracción, `build_profile`, `compute_base_rate`, tasa base con caché y adaptador `MarketIpo`), rutas en `server/src/routes/ipo.rs` (contrato en `docs/API.md → Rutas /api/ipo/*`), modo simulado en `server/src/fake.rs` (`FakeIpo`), pantalla `frontend/src/pages/IpoPage.tsx` (`/ipo/:symbol?`). Tipos en `domain` (`IpoRiskProfile`, `IpoRiskIndicator`, `IpoBaseRate`, trait `IpoData`).

**Qué es y qué no.** Un perfil de riesgo: hechos leídos del folleto (S-1/F-1/424B4) y del calendario con reglas fijas, cada uno con su fuente (sección del folleto + enlace a EDGAR, o «calendario») y la frase literal de la que sale (`evidence`). **No es una recomendación ni una predicción.** Lo que no se encuentra con fiabilidad queda `not_extracted` con el motivo; nunca se rellena ni se estima. No hay nota global ni «semáforo»: sumar indicadores en una puntuación daría una precisión que no existe.

## Indicadores

| Clave | Mide | Sale de | Cuándo es «no extraído» / límites |
|-------|------|---------|-----------------------------------|
| `offer_size` | Importe de la oferta (USD), acciones y precio | Calendario Finnhub | Sin entrada en el calendario o sin importe (presentaciones sin precio). En `expected` es el máximo posible (límite alto × acciones × 1,15). |
| `use_of_proceeds` | Banderas del destino de los fondos: devolver deuda, accionistas vendedores, rescate de títulos, fines corporativos generales, capital circulante, desarrollo clínico/I+D, adquisiciones, inversión de capital | `use_of_proceeds` (frases) | Sin la sección. Son palabras clave por frase (no mide importes ni peso relativo). «Devolver deuda» exige verbo (repay, prepay, pay down, redeem…) y objeto (indebtedness, term loan, credit facility…) en la misma frase y sin negación. |
| `secondary_sale` | % de lo ofrecido que venden accionistas existentes | Filas «offered by us» y «offered by the selling stockholders» de `offering` | Sin la fila de acciones nuevas. Si no hay fila de vendedores, solo dice «0 %» cuando el folleto no menciona vendedores en `offering` ni `use_of_proceeds`; si los menciona sin número, «no extraído». Sin contar sobreasignación. |
| `float_offered` | Acciones ofrecidas / acciones en circulación tras la oferta | `offering` | Usa la fila «Total … outstanding after this offering» si existe; si no, suma las clases. En ADS no se calcula (unidades distintas). **No es el flotante real** (lock-up, restringidas) y con colocación concurrente el total la incluye. |
| `lock_up` | Plazo (días o meses) que firman suscriptores y los de dentro | `lock_up`, `underwriting`, `shares_eligible`, `offering`, `summary` | Un plazo se acepta si está cerca de «lock-up», con verbo de compromiso («have agreed», «restrict»…) y anclado a «desde la fecha del folleto/cierre/oferta»; se descartan los de la Regla 144 y «within any three-month period». Con varios plazos distintos (escalonado, empresa vs. directivos) lista todos y no elige. Los meses no se convierten a días. |
| `underwriters` | Colocadores y su reparto | Tabla «Name \| Number of Shares» de `underwriting` | Solo si la suma de la tabla cuadra con su «Total» (o, sin «Total», con las acciones ofrecidas). No valora reputación. |
| `dilution` | Dilución por acción a los nuevos inversores (y % del precio) | Fila «Dilution per share to new investors» de `dilution` | Sin la fila con importe en dólares. Puede ser negativa (p. ej. ADRX, por una colocación privada concurrente): se muestra con una nota. El % usa el precio del calendario; el de la portada solo si ≥ 0,5 USD (SNSC deja 0,0001, el nominal). |
| `voting` | Voto múltiple («N votes per share», N ≥ 2) o varias clases | `summary`, `offering`, `principal_stockholders`, `dividend_policy`, `risk_factors` | Detección por frase. «No se detectó» **no prueba** que no exista; si hay «Class B» pero no se leyó la relación de votos, lo dice. |
| `controlled` | Menciona «controlled company» (+ % de voto si está en la misma frase o la siguiente) | las mismas | Puede ser «podría llegar a serlo»: lee la frase. Ausencia ≠ no lo es. |
| `net_result` | Signo del resultado neto en todos los periodos de la tabla | `summary_financial_data` | **No interpreta columnas**: el orden y la mezcla de años/semestres varía entre folletos y un valor equivocado sería peor que ninguno. Solo dice pérdida / beneficio / mixto y cita la fila. Etiquetas admitidas: «Net loss», «Net income (loss)» y variantes; ambigüedad de signos → «no extraído». |
| `revenue` | Ingresos > 0 en cuántos periodos, con la fila literal | `summary_financial_data` | Solo filas «Revenue», «Total revenues», «Net sales»… (no «Collaboration revenue»). Sin sección → «no extraído». |

Además, `warnings` avisa de folleto preliminar (cifras que pueden cambiar), probable SPAC, OPV que aún no cotiza, folleto o tasa base ausentes.

## Tasa base («momento de mercado»)

`IpoBaseRate`: mediana del rendimiento del primer día y a 3 meses (y exceso frente a SPY) de hasta 30 OPV con precio definitivo, sin SPAC, de los últimos 12 meses, repartidas uniformemente por la ventana (`sampled_ipo_performance`), con `nDay1`, `n3m` y los sesgos conocidos (rendimiento desde el precio de colocación, que solo recibe quien obtiene acciones; supervivencia). **Describe lo ocurrido con otras OPV; no predice la OPV en pantalla.** La UI avisa si `n3m < 10`.
Rendimiento: ~30 s en frío para 58 OPV (2 peticiones a Yahoo por OPV, 4 a la vez, medido con red real el 16-oct-2026); se cachea 6 h con un único cálculo en vuelo que sigue aunque el cliente se vaya.

## Cómo se ha probado

- `market/tests/ipo_risk.rs` (13 pruebas, sin red) sobre **extractos reales** de cinco 424B4 (`tests/fixtures/sec_prospectus_{trxb,lynx,oig,crwv,snsc}_excerpt.json`: secciones íntegras recortadas a unos miles de caracteres) y sobre el fixture HTML de folleto ya existente; incluye casos que deben quedar «no extraído».
- `server/tests/ipo.rs` (7): rutas, validaciones y perfil sobre la OPV simulada.
- `market/tests/live.rs::ipo_risk_profile_of_recent_real_ipos` (`#[ignore]`, red): perfil de 6 OPV reales recientes y tasa base.
- Resultado real (16-oct-2026): ver el informe de la tarea; en resumen, `use_of_proceeds`, `secondary_sale`, `float_offered`, `lock_up`, `underwriters` y `dilution` se leen en la mayoría de folletos de empresas operativas; `revenue` falla en biotecnológicas («Collaboration revenue»); `net_result` falla si el folleto no trae resumen financiero (F-1 pequeñas).

### Corrección: SPAC en la tasa base (16-oct-2026)
La heurística `spac_likely` (`providers/finnhub.rs::is_spac_likely`) solo miraba «acquisition» y dejaba pasar ~40 SPAC a 10,00 USD (Merger Corp, Churchill Capital Corp XIII, Cantor Equity Partners VII, SPAC 1...). Ahora, con precio exactamente 10,00, marca si se cumple (a) símbolo de 5 letras acabado en `U`, (b) palabra típica del nombre (acquisition, merger, spac, blank check, capital corp/partners, equity partners, holdings, investment corp, opportunit, liquidity, growth corp, special) o (c) nombre acabado en numeral de serie (II..XV, «Corp 4», «Capital Four»); se mantiene la regla antigua. El respaldo de EDGAR marca SPAC por SIC 6770, que el índice de texto completo ya trae (sin peticiones extra). Caso negativo con test: «Hoyne Bancorp, Inc.» (HYNE, conversión de caja de ahorros a 10 USD).

Tasa base real a 12 meses, antes y después (misma consulta, vía `ipo_risk_profile_of_recent_real_ipos`; la ventana se desplaza entre ejecuciones, así que la muestra no es idéntica):

| | Muestra (n día 1 / n a 3 m) | Mediana día 1 | % al alza día 1 | Mediana a 3 m |
|---|---|---|---|---|
| Antes | 27 / 22 | 0,0 % | 48 % | +0,9 % |
| Después | 30 / 25 | +0,3 % | 53 % | +0,1 % (exceso vs SPY −3,0 %) |

Calendario de 12 meses: 330 OPV con precio, 195 marcadas SPAC; la única a 10,00 USD sin marcar es HYNE (operativa). Las cifras siguen siendo una muestra de 30, no una estadística robusta.

### Tramos por tamaño de la oferta
La mediana global mezcla poblaciones distintas (micro-OPV y grandes). La tasa base se calcula además por tramos del importe de la oferta (`offer_size_usd`: total del calendario con precio fijado; acciones × punto medio del rango si aún no hay precio): **< 50 M**, **50–250 M** y **≥ 250 M USD**. Los cortes se fijaron antes de ver resultados, con criterio de mercado: micro-OPV (flotante diminuto, colocadores pequeños), pequeña y mediana-grande (cobertura institucional). El corte pertenece al tramo de arriba (50 M exactos → 50–250 M). Un tramo con menos de 10 OPV con dato del primer día se fusiona con el contiguo (el de menor muestra si hay dos) y la nota lo dice (`tierNotes`, `mergedFrom`); si todo se fusiona queda «todos los tamaños». El perfil de una OPV usa SU tramo (`baseRateTier`) y la global queda como referencia; sin tamaño calculable, solo la global y un aviso. La muestra sube a 60 OPV repartidas por la ventana.

Resultado real a 12 meses (16-oct-2026, 58 OPV muestreadas, sin SPAC por la heurística):

| Tramo | n día 1 | n a 3 m | Mediana día 1 | % al alza | Mediana a 3 m |
|---|---|---|---|---|---|
| < 50 M USD | 14 | 10 | −2,3 % | 43 % | −29,0 % |
| 50–250 M USD | 19 | 15 | 0,0 % | 47 % | +1,5 % |
| ≥ 250 M USD | 25 | 23 | +18,1 % | 76 % | +12,0 % |
| Global | 58 | 48 | +3,3 % | 59 % | +0,8 % |

La global (+3,3 %) no describe a ninguno de los tres tramos. Con n = 10–25 por tramo las medianas siguen siendo ruidosas. Coste en frío: ~30 s con `buffered(4)`, por debajo del umbral de 45 s; si hiciera falta bajarlo, se podría lanzar `base_rate(12)` en una tarea de fondo tras el arranque (no bloquea el servidor y hoy no se hace para no gastar peticiones a Yahoo en cada arranque).

## Límites

- Idioma: las reglas buscan texto en **inglés** de folletos de EE. UU.; un folleto en otro formato (PDF, antiguo, 253G2) no se ha probado.
- Una regla de texto puede fallar sin avisar si el folleto redacta el dato de forma inusual (por eso cada dato lleva su cita: se comprueba en 10 segundos).
- No hay valoración (múltiplos, comparables) ni evaluación de la calidad del negocio: eso no sale de un folleto con reglas fijas.

## Cómo encajaría lo siguiente (no hecho aquí)

- **Análisis con LLM:** que el modelo reciba las secciones relevantes (`risk_factors`, `use_of_proceeds`, `summary`) **y** el `IpoRiskProfile` ya calculado como hechos verificados, y devuelva solo una lectura estructurada con cita por afirmación (como en `ai/analysis`); nunca debe corregir ni reemplazar los indicadores. Las secciones con `parent` no se indexan dos veces en el RAG (`knowledge`).
- **Aprendizaje:** guardar el perfil (indicadores + tasa base en la fecha) junto a cada OPV vista y, cuando madure el rendimiento a 3/6/12 meses (`IpoPerformance`), medir si cada bandera (venta secundaria alta, deuda, voto múltiple, dilución > X %) se asocia a peor exceso frente a SPY, con la misma disciplina que `learning` (intervalos, muestra independiente, puerta de mínimos). Con unas decenas de OPV al año, hará falta mucho tiempo para que sea significativo: hasta entonces la tasa base es descriptiva.
- La detección de SPAC es heurística (precio 10,00 + nombre/símbolo). El sesgo que queda va en dos direcciones pequeñas: alguna SPAC sin palabras clave ni sufijo de unidades puede colarse (infla la mezcla con rendimientos pegados a 10 USD) y una empresa operativa a 10 USD con «Holdings», «Special» o un símbolo de 5 letras acabado en `U` se excluiría. Ni la UI ni la tasa base filtran «unidades» por otra vía: solo esta regla.
- La mediana del día 1 se mide desde el **precio de colocación**, que solo obtiene quien recibe acciones; quien compra al abrir parte de otro precio.
- En micro-OPV (< 50 M USD) el capital flotante es muy pequeño y la volatilidad del primer día no es representativa; aun así sale en su tramo, con aviso.
