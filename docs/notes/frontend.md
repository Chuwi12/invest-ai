# Notas del frontend

## Hecho
- **Shell**: `index.html`, `main.tsx`, layout con carril de navegación (barra inferior en móvil), buscador global (`/` o Ctrl+K, resultados agrupados por clase, teclado completo), menú de estado (WebSocket, LLM, proveedores vía `/status`), conmutador de tema (automático/claro/oscuro), aviso permanente de «no es asesoramiento financiero» y banner de datos simulados solo en modo mock.
- **Ficha de activo** (`/asset/:id`): precio en vivo con destello sutil por tick, gráfico de velas + volumen (lightweight-charts, volumen en panel propio), rango/intervalo, MM 20/50/200 y Bollinger, tick en vivo sobre la última vela; pestañas (en la URL: `?tab=`) Análisis de IA, Indicadores, Fundamentales, Noticias (con sentimiento), Macro.
- **Análisis de IA**: profundidad quick/standard/deep, progreso con cronómetro y cancelación, veredicto con escala, confianza, puntuaciones, tesis, casos, riesgos, catalizadores, niveles, estrategia, datos que faltan, fuentes citadas, modelo, histórico navegable y contexto (snapshot).
- **Panel** (`/dashboard`), **Chat** (`/chat/:cid?`, SSE, tool calls con origen, markdown seguro, conversaciones fijables/borrables), **Favoritos** (con notas), **Historial**.
- **Ajustes → Modelos de IA** (`/settings/models`): hardware detectado, catálogo por nivel, indicación neutra de encaje (VRAM / RAM con descarga parcial / CPU / no cabe), descargar con barra SSE y cancelar, activar, borrar con confirmación. Tipos, cliente, hooks y mock (`src/mock/models.ts`).
- Verificabilidad: cada panel con cifras lleva `Provenance` (proveedor con enlace cuando se conoce, antigüedad relativa, marca de tiempo en tooltip, «retrasado» según `lib/freshness.ts`).
- Tests: 14 ficheros (lib, SSE de descargas, Markdown seguro, ModelCard, AnalysisReport, shell/buscador/modelos con el mock).

## Pendiente / limitaciones conocidas
- **Opciones**: la API no expone cadenas de opciones (`docs/API.md`), así que no hay pestaña de opciones.
- Si un backend antiguo no manda cabeceras `X-Data-Source*` ni `Indicators.source`, la procedencia se deduce de `quote.source`/`summary.source` y se rotula «proveedor deducido, el backend no lo indicó».
- Chat: no se pueden añadir activos de contexto desde la propia pantalla (solo `?asset=` y los de la conversación) ni renombrar conversaciones.
- Con el fallback al `POST /analysis` (backend sin `/analysis/stream`) no hay etapas: el panel lo indica tras 6 s y el navegador registra el 404 del stream como error de recurso en consola (solo en ese caso).
- Moneda resuelta en el frontend: todo precio se rotula con `quote.currency` (nunca con la del summary). Ver «Pulido de cifras».
- No verificado: contraste AA medido con herramienta (solo diseño con tokens elegidos para AA), lectores de pantalla reales, Safari/Firefox, (el backend real SÍ se ha probado, ver abajo).

## Decisiones
- CSS plano con tokens (sin Tailwind): menos dependencias y bundle inicial menor. Tipografía: Geist (UI y cifras tabulares) y Newsreader (prosa del análisis y titulares), para que el análisis se lea como una nota de analista; el veredicto se dibuja como una escala de 6 pasos.
- Color de subida/bajada verde/rojo siempre acompañado de signo y flecha.
- Un solo WebSocket (`getRealtime`); `LivePrice` es el único componente que se re-renderiza por tick de su activo, y el gráfico actualiza la vela sin pasar por React.
- Mock fuera del bundle de producción: `main.tsx` hace `import('@/mock')` bajo `import.meta.env.VITE_MOCK` (comprobado: el build no contiene `modo MOCK`).
- CSP: no hay scripts ni estilos en línea en `index.html`; no se usa `eval`. Cabecera recomendable en el servidor: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ws: wss:; font-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'`. (`unsafe-inline` en estilos por los atributos `style` de React y de lightweight-charts; no se probó con la cabecera aplicada.) No se añade `<meta>` CSP porque rompería el HMR de Vite.
- Seguridad: sin `dangerouslySetInnerHTML`; Markdown sin `rehype-raw`, imágenes omitidas, enlaces solo http(s) con `noopener noreferrer`; el borrado del modelo activo se bloquea en la UI.
- `GET /assets/{id}` registra una visita: no se precarga al pasar el ratón (ya lo hacía `prefetchAsset`).
- `generateAnalysis(id, depth, signal)` cambia de firma (añade profundidad).

## Tamaños de bundle (`vite build`, minificado / gzip)
    dist/assets/index-CwHaY1Mn.css                                     23.68 kB │ gzip:  5.85 kB
    dist/assets/useTitle-Rt8sLgPO.js                                    0.16 kB │ gzip:  0.16 kB
    dist/assets/x-UtLts_a5.js                                           0.27 kB │ gzip:  0.21 kB
    dist/assets/trash-DujWDo7K.js                                       0.37 kB │ gzip:  0.24 kB
    dist/assets/types-CGaA31ub.js                                       0.74 kB │ gzip:  0.37 kB
    dist/assets/Provenance-BQeZCv1z.js                                  1.07 kB │ gzip:  0.56 kB
    dist/assets/FavoritesPage-Ds5YKtz9.js                               2.42 kB │ gzip:  1.14 kB
    dist/assets/NewsPanel-C7RDL65G.js                                   2.44 kB │ gzip:  1.16 kB
    dist/assets/HistoryPage-BJsiqsls.js                                 2.61 kB │ gzip:  0.98 kB
    dist/assets/WatchTable-vbrUv-Fn.js                                  2.94 kB │ gzip:  1.25 kB
    dist/assets/freshness-scJRzeKG.js                                   3.24 kB │ gzip:  1.51 kB
    dist/assets/SearchPalette-DMN2kDZZ.js                               4.17 kB │ gzip:  1.89 kB
    dist/assets/Dashboard-E-YmeDM_.js                                   4.75 kB │ gzip:  1.80 kB
    dist/assets/AnalysisReport-5Rspf8OX.js                              9.82 kB │ gzip:  3.03 kB
    dist/assets/ModelsPage-uXEFaROe.js                                 11.11 kB │ gzip:  3.86 kB
    dist/assets/VirtualList-0ZgV-h97.js                                27.03 kB │ gzip:  8.02 kB
    dist/assets/hooks-CnDbr6lV.js                                      58.58 kB │ gzip: 18.50 kB
    dist/assets/ChatPage-BnEzodAW.js                                  167.95 kB │ gzip: 51.15 kB
    dist/assets/AssetPage-BHiCDOLx.js                                 198.74 kB │ gzip: 65.04 kB
    dist/assets/index-CV56swxT.js                                     284.75 kB │ gzip: 91.45 kB
    ✓ built in 157ms

Carga inicial: `index` (React, router, query, shell) + CSS; las páginas y el gráfico llegan por ruta. Fuentes woff2 con subconjuntos por `unicode-range`: solo se descargan los usados.

## Procedencia, progreso real y prueba contra el backend real
- `api/types.ts`: `sourceUrl`/`timestampOrigin`/`Indicators.source`/`latencyMs`/`blockedSecs` (opcionales y `| null`), `HistoryResult`, `AnalysisStreamEvent`. `Api.history` devuelve `HistoryResult` (velas + cabeceras `X-Data-*`, vía `requestWithHeaders`).
- `Provenance`: enlace «verificar en la fuente ↗» solo con URL real http(s) del backend; «hora de consulta» si `timestampOrigin = local`. Aplicado a cotización, velas, indicadores, fundamentales y macro (por fila). El menú de estado muestra latencia y «en pausa N s».
- Análisis: `Api.streamAnalysis` (SSE `stage`/`done`/`error`, `streamAnalysisEvents`) con AbortController y fallback al `POST /analysis` si el stream da 404/405; `useGenerateAnalysis` expone `stages`; el panel pinta las etapas reales («paso 3 de 4»). Simulado en `mock/index.ts`. Contra el backend real (qwen2.5:7b, quick) llegaron las etapas y el análisis terminó.
- Números en español: nuevo `formatSigned` («impacto +0,4») y `formatNumber` en el sentimiento de noticias.
- Corregido por diferencias real/mock: `LivePrice` ya no formatea índices ni bonos como moneda (el real manda `currency: "USD"` para ^GSPC; el bono sale como «5,24 %»).
- Proxy de Vite: `BACKEND_URL=http://127.0.0.1:8799 npx vite --port 5181` (por defecto sigue 8787). Vite escucha en `localhost` (IPv6): usar `http://localhost:5181`.
- Prueba real con puppeteer-core (Chrome headless): panel, AAPL (cabecera, indicadores, fundamentales, noticias, macro), BTC con WS (el precio cambió entre lecturas), menú de estado, chat (respuesta con tool call), Ajustes → Modelos y generación de análisis. Resultado: 0 errores/avisos de consola y 0 peticiones fallidas (salvo el 404 del stream mientras el backend no lo tenía). Capturas en `/tmp/claude-1000/-home-chuwi/4d633c05-8d50-4a5d-b60f-b5bff5fd31d2/scratchpad/shots/`: 01-panel, 02-aapl, 03-aapl-{indicators,fundamentals,news,macro}, 04-btc, 05-estado, 06-chat, 07-modelos, 08-analisis-progreso, 09-analisis-resultado (.png).
- Tests: 16 ficheros, 123 pruebas (nuevos: `analysisStream.test.ts`, `Provenance.test.tsx`, `formatSigned`).

## Pulido de cifras (formato y moneda)
- `formatCompact`: 3 cifras significativas sin ceros finales (4.967.129.153.536 -> «4,97 B»; 12.345.678 -> «12,3 M»); si el redondeo llega a 1000 sube de escala (999.600 -> «1 M»). Por debajo de 10.000 se muestra el entero. `formatFull` da el valor completo y se usa en `title` de capitalización, valor empresa, caja, deuda, patrimonio, acciones, oferta, etc.
- Gráfico: `localization.locale = es-ES`, `dateFormat` español; el precio se formatea por serie (`priceFormat: custom` con `formatPrice`, sin símbolo, decimales según precio/clase) y el volumen con `formatCompact` («100 mil»). DESVIACIÓN respecto a pedir `localization.priceFormatter`: un formateador global pisa también el del volumen (comprobado: salía «20.000.000,00»), así que se configura por serie.
- Moneda: `LivePrice` ya no acepta `currency`; usa solo `quote.currency`. USDT se rotula «USDT» (Intl no lo conoce, cae al código) con tooltip «Tether (stablecoin ≈ USD), cotización de Binance» (`currencyNote`). Aplicado a tira de mercado, Favoritos, Más vistos cabecera de ficha (apertura/máx/mín/cierre/capitalización), histórico de análisis y contexto de datos. Pie del gráfico muestra la moneda de la cotización.
- Excepción consciente: Fundamentales no trae moneda propia y sigue rotulando con `summary.currency` (capitalización, objetivos de analistas, máximo histórico); no proceden de la cotización.
- Otros números: nº de analistas, visitas del historial y valores numéricos de «Otros datos del proveedor» pasan por el formateador. Quedan `toLocaleString('es-ES')` en tamaños de modelo (GB/MB, valores <1000, correcto) y `toFixed` solo en coordenadas SVG del `Sparkline`.
- El mock manda ahora `currency: 'USDT'` para cripto (como Binance real).

## Para el coordinador
- Backend: el `summary.currency` de cripto es «USD» mientras `quote.currency` es «USDT». El frontend ya no depende de ello, pero conviene que `/assets/{id}` y los fundamentales documenten qué moneda usan (los fundamentales no traen `currency`; sería útil añadirla para rotular capitalización/objetivos con su moneda real).
- `priceAtAnalysis` no lleva moneda: se rotula con la de la cotización actual; si cambia de proveedor entre análisis podría no coincidir. Idealmente el análisis guardaría `currency`.

## Pulido de cifras (formato y moneda)
- `formatCompact`: 3 cifras significativas sin ceros finales (4.967.129.153.536 -> «4,97 B»; 12.345.678 -> «12,3 M»); si el redondeo llega a 1000 sube de escala (999.600 -> «1 M»). Por debajo de 10.000 se muestra el entero. `formatFull` da el valor completo y se usa en `title` de capitalización, valor empresa, caja, deuda, patrimonio, acciones, oferta, etc.
- Gráfico: `localization.locale = es-ES` y `dateFormat` español; el precio se formatea por serie (`priceFormat: custom` con `formatPrice`, sin símbolo, decimales según precio/clase) y el volumen con `formatCompact` («100 mil»). Desviación respecto a pedir `localization.priceFormatter`: un formateador global pisa también el del volumen (comprobado: salía «20.000.000,00»), así que se configura por serie.
- Moneda: `LivePrice` ya no acepta `currency`; usa solo `quote.currency`. USDT se rotula «USDT» (Intl no lo conoce, cae al código) con tooltip «Tether (stablecoin ≈ USD), cotización de Binance» (`currencyNote`). Aplicado a tira de mercado, Favoritos, Más vistos, cabecera de ficha (apertura/máx/mín/cierre/capitalización), histórico de análisis, contexto de datos y pie del gráfico.
- Excepción consciente: Fundamentales no trae moneda propia y sigue rotulando con `summary.currency` (capitalización, objetivos de analistas, máximo histórico); no proceden de la cotización.
- Otros números: nº de analistas, visitas del historial y valores numéricos de «Otros datos del proveedor» pasan por el formateador. Quedan `toLocaleString('es-ES')` en tamaños de modelo (GB/MB, valores <1000, correcto) y `toFixed` solo en coordenadas SVG del `Sparkline`.
- El mock manda ahora `currency: 'USDT'` para cripto (como Binance real).

## Para el coordinador
- Backend: `summary.currency` de cripto es «USD» y `quote.currency` «USDT». El frontend ya no depende de ello, pero los fundamentales no traen `currency`; sería útil añadirla para rotular capitalización/objetivos con su moneda real.
- `priceAtAnalysis` no lleva moneda: se rotula con la de la cotización actual. Idealmente el análisis guardaría `currency`.


## Informe profundo, tiempos medidos y moneda de fundamentales
- `api/types.ts`: `Analysis` refleja todos los campos del dominio (`levelsNote`, `scenarios`, `expectedReturnPct`, `wouldChangeIf`, `horizonNote`, `positionSizing`, `method`, `qualityNotes`, `depth`, `durationMs`, `llmCalls`, `promptTokens`, `completionTokens`; todos opcionales y `| null`), más `Scenario`, `PositionSizing` y `Fundamentals.currency`. El mock (`mock/content.ts`) los genera por profundidad (escenarios solo en `deep`) y `mock/db.ts` siembra un par de análisis «antiguos» sin esos campos.
- `lib/analysis.ts` (puro, con tests): `scenarioSlices` (orden alcista/base/bajista; porciones enteras por mayor resto que suman 100; si el modelo no sumó 100 se reescala y se avisa; probabilidades inválidas -> sin barra), `splitThesis`, `medianDuration`, `formatDurationMs`.
- `components/AnalysisSections.tsx`: Escenarios (barra apilada, precio objetivo con su base, rentabilidad esperada con horizonte), «Qué cambiaría el veredicto», Tamaño de posición con su justificación, notas de calidad discretas (sin color de alarma; no se mezclan con «datos que faltan») y los campos del pie. Nota de niveles bajo «Niveles clave» y nota de horizonte bajo la fila de horizonte. Campo null/ausente/lista vacía -> no se pinta.
- Tesis: `splitThesis` quita el bloque «La conclusión cambiaría si» solo si hay `wouldChangeIf` (si no, es la única copia). «Incertidumbre» no tiene campo propio: se saca del cuerpo y se pinta una vez como nota con filete; si el backend deja de incluirla en la tesis, simplemente no aparece.
- Tiempos: se eliminan «≈ 10 s / ≈ 30 s / 60 s o más». `useDurationSamples` junta el histórico del activo (`/analyses`) y `latestAnalyses` del panel (React Query, sin duplicar por `id`); `medianDuration` toma la mediana de `durationMs` con el MISMO modelo activo (`/status`) y la misma profundidad; ignora análisis sin medición. Sin muestras: «sin medición todavía con este modelo». En generación: tiempo transcurrido (contra el reloj, no contando ticks) y estimado si hay mediana. Pie: modelo, profundidad, duración, llamadas y tokens (`method` del backend no se pinta: el pie lo estructura).
- Fundamentales: `fundamentalsCurrency(f, summary.currency)` rotula capitalización, valor empresa, caja/deuda, objetivos de analistas, máximo histórico, etc. con `fundamentals.currency`; si falta, usa la del summary y el tooltip dice que es deducida. Sustituye a la «excepción consciente» de «Pulido de cifras». El mock manda USD para cripto (CoinGecko).

### Móvil (360–494 px)
- Selector de profundidad (`.seg.depth-seg`): por debajo de 520 px ocupa todo el ancho, las tres opciones reparten el espacio y el estimado pasa a segunda línea.
- `.table-wrap` y `.tabs` tienen scroll horizontal con sombras laterales (scroll-shadow en CSS puro) que indican que hay más contenido; la pestaña activa se desplaza a la vista (`scrollIntoView` en `AssetPage`). `.table-wrap` lleva `position: relative`: sin eso el desbordamiento interno de la tabla ensanchaba el documento en móvil (495 px a 360).
- El banner de modo mock usaba un margen negativo en línea de 1,25 rem que desbordaba 8 px con el padding móvil de `.main`; ahora el margen va en CSS y se ajusta en ≤ 880 px.
- Comprobado con puppeteer (isMobile) a 360 y 494 px: `documentElement.scrollWidth` = ancho del viewport (antes 495/502). Capturas `m360-*.png` y `m494-*.png` en shots-analysis.


## Segunda revisión: hooks, chat, WebSocket y tests (R-21…R-29)

Detalle en `review.md → Segunda revisión`.

- **`useChatStream`**: cada `send` comprueba que sigue siendo la respuesta vigente (`ctrl.current === ac`) antes de tocar
  el búfer o el estado; los eventos pendientes de una respuesta abortada (un chunk SSE trae varios) ya no se mezclan con
  la nueva. `StreamState.base` guarda cuántos mensajes tenía el hilo al enviar: cuando el hilo guardado pasa de
  `base + 1` / `base + 2`, `ChatPage` oculta la pregunta/respuesta en vivo (tras un error o «Detener» el servidor guarda
  el texto parcial y salía dos veces). Si el servidor no guardó nada, el aviso del error se mantiene.
- **`RealtimeClient`**: `reconnectNow()` cierra el socket del intento anterior antes de abrir otro; el backoff solo se
  reinicia cuando la conexión lleva `stableAfterMs` (10 s) abierta, no al abrir.
- **Buscador**: Enter se ignora mientras los resultados visibles son los de una búsqueda anterior (antirrebote/respuesta en curso); antes elegía el primero de la búsqueda previa.
- **Claves de React Query**: `qk.search(q, clase, límite)`; `useHistory` solo conserva las velas previas (`placeholderData`)
  si son del mismo activo.
- **`applyTickToCandle`**: velas diarias y semanales abren vela nueva al cruzar su intervalo; las mensuales (duración
  variable) siguen actualizando la última.
- **Tests deterministas bajo carga**: `setup.ts` fija `asyncUtilTimeout` en 10 s (las esperas terminan en cuanto se
  cumple la condición; solo el tope es mayor), `vite.config.ts` `testTimeout`/`hookTimeout` de 30 s, `App.test` y
  `ChatPage.test` precargan los chunks perezosos, y `Element.prototype.scrollIntoView` se stubea (jsdom no lo tiene).
  Regla para tests nuevos: esperar por condición (`findBy*`/`waitFor`) o con `vi.useFakeTimers`, nunca con un `sleep` ni con
  umbrales de reloj; no fijar `{ timeout }` menores que el global.
- Tests: 24 ficheros, 175 pruebas (`npm test` ×5 seguidas sin fallos).

## Antigüedad medida frente a retraso declarado (13-oct-2026)
Problema: la cabecera de AAPL/MSFT mostraba «Retraso de 15 s» junto a «ahora mismo» (el 15 eran **minutos** del estándar de la bolsa, no segundos; ver `market.md → exchangeDataDelayedBy…`).
Regla de presentación (honesta con la procedencia):
- **Dato principal = antigüedad medida**: `ahora − timestamp`, solo cuando `timestampOrigin = provider`. Es lo que sale en `Provenance` («hace 2 s», «ahora mismo»…) y `quoteFreshness(...).ageMs`. Con `local` (el proveedor no dio hora) no hay medida: `ageMs = null`, `measured = false` y el pie dice «hora de consulta».
- **El retraso que declara el proveedor (`delaySecs`) es información secundaria**: un icono «i» junto al pie con el tooltip «El proveedor declara un retraso de hasta 15 min para esta bolsa» (`declaredDelayNote`). Sin hora del proveedor añade «; no dio la hora del dato, así que no se ha podido medir». Si no declara retraso (0 o `null`) no hay icono. Ya no se escribe «Retraso de N», «Tiempo real» ni «Retraso desconocido» en el pie (`formatDelay` desaparece; `formatDeclaredDelay`/`formatSpan` en `lib/format.ts`).
- **«retrasado»** (`quoteFreshness`): con la sesión abierta (`regular`/`continuous`) y antigüedad medida > retraso declarado + 2 min de margen (`QUOTE_MARGIN_MS`). Con el mercado cerrado/pre/post no se marca. Sin medida solo queda la declaración: se marca si es > 60 s y la sesión está abierta, y el motivo dice que no se pudo medir. Antes bastaba con que el proveedor declarase > 60 s, aunque el dato llegase con 2 s (justo el caso erróneo de arriba).
- `QuoteFreshness` añade `measured` y `declaredDelaySecs` a `Freshness`; `Dashboard` y `WatchTable` siguen usando `stale`/`reason` sin cambios.
- Tests: `freshness.test.ts` (medida < declarada, > declarada + margen y el límite exacto, feed diferido de verdad, sin hora del proveedor, origen desconocido, mercado cerrado/pre/post, cripto), `AssetHeader.test.tsx` (la cabecera no contiene «Retraso de 15 s», muestra «ahora mismo», el 15 solo en el tooltip, «retrasado» con 10 min de antigüedad, sin hora de proveedor, mercado cerrado), `Provenance.test.tsx` (nota secundaria) y `format.test.ts`.
- Límites: la antigüedad se mide con el reloj de este equipo (un reloj desajustado la falsea). El pie de la cabecera usa la hora de la cotización REST (sondeo de 15 s), no la del último tick WS que pinta `LivePrice`: con ticks más recientes el precio puede ser más nuevo que la antigüedad mostrada.

## Reloj de frescura (R-35, 14-oct)
`hooks/useClock.ts` fuerza un render periódico para que la antigüedad («hace N min») y «retrasado» avancen aunque el dato no cambie (React Query conserva la misma referencia si la respuesta es idéntica). Úsalo en cualquier componente que calcule `ahora − hora del dato`.
