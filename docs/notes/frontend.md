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
- El histórico (`/history`) y los indicadores no devuelven proveedor: la procedencia se deduce de `quote.source`/`summary.source` y se rotula como tal («cálculo propio sobre velas diarias»). Conviene que el backend incluya `source` en esas respuestas.
- Chat: no se pueden añadir activos de contexto desde la propia pantalla (solo `?asset=` y los de la conversación) ni renombrar conversaciones.
- Las etapas de la barra de progreso del análisis son orientativas (el backend no emite progreso).
- No verificado: contraste AA medido con herramienta (solo diseño con tokens elegidos para AA), lectores de pantalla reales, Safari/Firefox, backend real (todo visto con `npm run dev:mock`).

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
