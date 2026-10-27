import { memo, useMemo, useState, type CSSProperties } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'
import '@fontsource-variable/archivo/wdth.css'
import '@/styles/landing.css'
import { api } from '@/api'
import { STALE, qk, useModels, usePerformance, useQuotes } from '@/api/hooks'
import type { Analysis, AssetId, HorizonReport, PerformanceReport, Quote, SourceRef, WatchlistRun, WatchlistStatus } from '@/api/types'
import { ExternalLink } from '@/components/ExternalLink'
import { useClock } from '@/hooks/useClock'
import { useTitle } from '@/hooks/useTitle'
import { CLASS_SINGULAR, HORIZON_LABEL, VERDICT_LABEL, assetHref, parseAssetId, symbolOf } from '@/lib/assets'
import { IS_MOCK } from '@/lib/env'
import { errorText } from '@/lib/errors'
import { NBSP, formatDate, formatDateTime, formatInt, formatNumber, formatPercent, formatPoints, formatPrice, formatRelative, toneOf } from '@/lib/format'
import { enoughData, hitVerdict, rateText } from '@/lib/performance'
import { mergeLive } from '@/lib/quote'
import { sourceName } from '@/lib/sources'
import { useConnectionState, useLivePrice } from '@/realtime/hooks'

/** El backend admite hasta 50 ids en `GET /quotes`. */
const MAX_QUOTE_IDS = 50
/** Celdas por mitad de la cinta: con celdas de 13 rem cubren 2560 px y el bucle no deja huecos. */
const MIN_CELLS_PER_HALF = 14
/** Una fuente con más de dos días se atenúa en el índice. */
const OLD_SOURCE_MS = 48 * 3_600_000
const ORIGIN_DERIVED = 'calculado por invest-ai'
const NO_ANALYSIS = ['analysis', 'none'] as const
/** La landing no se queda vieja: sondeo suave y refresco al volver a la pestaña (solo aquí, no en el resto de la app). */
const REFRESH = { refetchInterval: 60_000, refetchOnWindowFocus: true } as const

// --- Datos derivados (puros) -----------------------------------------------------------------

/** El activo de la lista con el análisis correcto más reciente (`lastOkAt`), o null si ninguno lo tiene. */
function latestAssetId(w: WatchlistStatus): AssetId | null {
  let best: { id: AssetId; at: number } | null = null
  for (const a of w.assets) {
    const at = a.lastOkAt ? Date.parse(a.lastOkAt) : Number.NaN
    if (Number.isFinite(at) && (!best || at > best.at)) best = { id: a.id, at }
  }
  return best?.id ?? null
}

/** Posición (1, 2…) de la primera fuente que cumple `test`, o null si el análisis no la trae. */
function refWhere(sources: readonly SourceRef[], test: (s: SourceRef) => boolean): number | null {
  const i = sources.findIndex(test)
  return i < 0 ? null : i + 1
}

/**
 * Títulos EXACTOS que fabrica el backend (`verify.rs`, `source_of`): «Cotización {id}» y «Puntuación cuantitativa
 * determinista {id}», sin URL. Un titular de noticias que empiece igual no cuenta.
 */
const isQuoteSource = (asset: AssetId) => (s: SourceRef) => s.url === null && (s.title === `Cotización ${asset}` || s.title === `Quote ${asset}`)
const isScoreSource = (asset: AssetId) => (s: SourceRef) =>
  s.url === null && s.provider === 'invest-ai' && (s.title === `Puntuación cuantitativa determinista ${asset}` || s.title === `Deterministic quantitative score ${asset}`)

/** `market` es el módulo de indicadores de invest-ai; `sourceName` no lo conoce. */
function providerLabel(provider: string): string {
  return provider
    .split(',')
    .map((p) => (p.trim().toLowerCase() === 'market' ? 'invest-ai' : sourceName(p.trim())))
    .join(', ')
}

/** El backend añade el id del activo al título («Cotización stock:AAPL»): ya está en el titular. */
function cleanTitle(title: string, asset: AssetId): string {
  return title.endsWith(` ${asset}`) ? title.slice(0, -asset.length - 1) : title
}

interface Figure {
  key: string
  label: string
  value: string
  unit?: string
  /** Posición de su fuente en el índice; null si es una cifra sin entrada propia. */
  ref: number | null
  origin: string
}

/** Cuatro cifras, solo de campos estructurados del análisis: nunca se leen del texto del modelo. */
function figuresOf(a: Analysis, currency: string | undefined): Figure[] {
  const cls = parseAssetId(a.asset).class
  const money = cls === 'index' || cls === 'bond' ? undefined : currency
  // La referencia del precio solo existe si hay precio: sin él no hay nada a lo que apuntar.
  const quoteRef = a.priceAtAnalysis === null ? null : refWhere(a.sources, isQuoteSource(a.asset))
  const scoreRef = refWhere(a.sources, isScoreSource(a.asset))
  const quote = quoteRef ? a.sources[quoteRef - 1] : undefined
  return [
    {
      key: 'price',
      label: 'Precio en el análisis',
      value: a.priceAtAnalysis === null ? '—' : formatPrice(a.priceAtAnalysis, { class: cls, currency: money }),
      ref: quote ? quoteRef : null,
      origin: quote ? providerLabel(quote.provider) : a.priceAtAnalysis === null ? 'sin precio en este análisis' : 'fuente no indicada en el análisis',
    },
    { key: 'score', label: 'Puntuación global', value: formatNumber(a.scores.overall, 0), unit: 'de 100', ref: scoreRef, origin: ORIGIN_DERIVED },
    // `calibrate()` mezcla al 50 % la confianza del modelo y la de la puntuación: no es una cifra de ninguna fuente.
    { key: 'confidence', label: 'Confianza', value: formatPercent(a.confidence, 0), ref: null, origin: 'la del modelo, calibrada por invest-ai' },
    // `qualityNotes` es null solo en análisis anteriores al verificador: ahí no hay cifra, no un cero.
    {
      key: 'quality',
      label: 'Notas de calidad',
      value: a.qualityNotes ? formatInt(a.qualityNotes.length) : '—',
      ref: null,
      origin: a.qualityNotes ? ORIGIN_DERIVED : 'análisis anterior al verificador',
    },
  ]
}

/** «acción», «ETF», «cripto»: el singular de la clase, en minúscula salvo las siglas. */
function kindOf(asset: AssetId): string {
  const k = CLASS_SINGULAR[parseAssetId(asset).class]
  return k === k.toUpperCase() ? k : k.toLowerCase()
}

function modelStatus(m: ReturnType<typeof useModels>): { text: string; on: boolean } {
  if (m.isPending) return { text: 'comprobando', on: false }
  if (m.isError || !m.data) return { text: 'sin respuesta del servidor', on: false }
  const { active, loaded, ollamaReachable } = m.data
  if (!ollamaReachable) return { text: `${active}, Ollama no responde`, on: false }
  if (loaded.some((l) => l.id === active)) return { text: `${active}, cargado en memoria`, on: true }
  return { text: `${active}, sin cargar ahora`, on: false }
}

function runText(r: WatchlistRun): string {
  switch (r.status) {
    case 'running':
      return 'en marcha'
    case 'completed':
      return `${r.ok} de ${r.planned} analizados`
    case 'missed':
      return 'perdida, no se analizó a tiempo'
    case 'aborted':
    case 'interrupted':
      return `cortada, ${r.ok} de ${r.planned} analizados`
  }
}

/** Misma clave y consulta que `useWatchlist`, con el refresco propio de la landing (C8). */
function useLandingWatchlist() {
  return useQuery({
    queryKey: qk.watchlist,
    queryFn: ({ signal }) => api.watchlist(signal),
    staleTime: 15_000,
    refetchOnWindowFocus: REFRESH.refetchOnWindowFocus,
    // Una pasada en marcha cambia sola: se sigue de cerca; si no, sondeo suave.
    refetchInterval: (q) => (q.state.data?.running ? 10_000 : REFRESH.refetchInterval),
  })
}
type Watchlist = ReturnType<typeof useLandingWatchlist>

// --- Cabecera --------------------------------------------------------------------------------

function Header({ watchlist }: { watchlist: Watchlist }) {
  const models = useModels()
  const model = modelStatus(models)
  const run = watchlist.data?.lastRun ?? null
  return (
    <header className="lp-top">
      <div className="lp-brand">
        <h1 className="lp-mark">invest-ai</h1>
        <p className="lp-tag">Análisis de inversión con un modelo de IA local: cada cifra con su fuente; el modelo y tus datos no salen de este equipo.</p>
      </div>
      <dl className="lp-status">
        {IS_MOCK ? (
          <div>
            <dt>Datos</dt>
            <dd>simulados, modo demostración</dd>
          </div>
        ) : null}
        <div>
          <dt>Modelo local</dt>
          <dd>
            <span className="lp-sq" data-off={!model.on} aria-hidden="true" />
            {model.text}
          </dd>
        </div>
        <div>
          <dt>Última pasada de la lista</dt>
          <dd>
            {watchlist.isPending ? (
              'comprobando'
            ) : watchlist.isError ? (
              'sin respuesta del servidor'
            ) : run ? (
              <span>
                <time dateTime={run.localDate}>{formatDate(run.localDate)}</time>, {runText(run)}
              </span>
            ) : (
              'todavía no se ha hecho ninguna'
            )}
          </dd>
        </div>
      </dl>
    </header>
  )
}

// --- Cinta -----------------------------------------------------------------------------------

type QuoteState = 'loading' | 'error' | 'ok'

/** Una celda de ancho fijo: un tick cambia el texto, nunca el ancho, así que no hay reflow en el bucle. */
const TapeCell = memo(function TapeCell({ id, quote, state, rep }: { id: AssetId; quote: Quote | undefined; state: QuoteState; rep: boolean }) {
  const tick = useLivePrice(id)
  const m = mergeLive(quote, tick)
  const cls = parseAssetId(id).class
  const money = cls === 'index' || cls === 'bond' ? undefined : (quote?.currency ?? undefined)
  const price = m.price === null ? null : `${formatPrice(m.price, { class: cls, currency: money })}${cls === 'bond' ? `${NBSP}%` : ''}`
  return (
    // Las copias que solo rellenan el bucle no existen para el teclado ni para los lectores de pantalla, pero sí para el
    // ratón (sin `inert`, que dejaría pasar el clic). Con ratón no se da foco: el foco detiene la cinta a mitad del clic.
    <li className={rep ? 'lp-cell lp-rep' : 'lp-cell'} {...(rep ? { 'aria-hidden': true } : {})}>
      <Link to={assetHref(id)} tabIndex={rep ? -1 : undefined} onMouseDown={(e) => e.preventDefault()}>
        <span className="lp-sym">{symbolOf(id)}</span>
        {price !== null ? (
          <span className="lp-px">{price}</span>
        ) : (
          <span className="lp-px lp-nopx">{state === 'loading' ? '—' : 'sin precio'}</span>
        )}
        {m.changePct !== null ? <span className={`lp-chg lp-${toneOf(m.changePct)}`}>{formatPoints(m.changePct)}</span> : null}
      </Link>
    </li>
  )
})

interface TapeProps {
  ids: AssetId[]
  quotes: Map<AssetId, Quote>
  quoteState: QuoteState
  /** Hora de los precios que se enseñan cuando el último refresco falló; null si están al día. */
  staleAt: number | null
  onRetry: () => void
}

function Tape({ ids, quotes, quoteState, staleAt, onRetry }: TapeProps) {
  const [paused, setPaused] = useState(false)
  const live = useConnectionState() === 'open'
  const reps = Math.max(1, Math.ceil(MIN_CELLS_PER_HALF / ids.length))
  const cells = (dup: boolean) =>
    Array.from({ length: reps }, (_, r) => ids.map((id) => <TapeCell key={`${r}:${id}`} id={id} quote={quotes.get(id)} state={quoteState} rep={dup || r > 0} />)).flat()
  return (
    <>
      <div className="lp-tape" data-paused={paused}>
        <div className="lp-tape-ctl">
          <button type="button" className="lp-pause" aria-pressed={paused} onClick={() => setPaused((p) => !p)}>
            Pausar cinta
          </button>
          <span className="lp-live">
            <span className="lp-sq" data-off={!live} aria-hidden="true" />
            <span className="lp-live-t">{live ? 'En vivo' : 'Sin ticks en vivo'}</span>
          </span>
        </div>
        {/* Con foco dentro la cinta se detiene y pasa a scroll: una celda enfocada nunca queda fuera de la pantalla. */}
        <div
          className="lp-viewport"
          // Chrome solo acerca una celda a medias visible; con `nearest` se ve entera.
          onFocus={(e) => e.target.scrollIntoView({ block: 'nearest', inline: 'nearest' })}
          onBlur={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) e.currentTarget.scrollLeft = 0
          }}
        >
          <div className="lp-track" style={{ '--lp-cells': ids.length * reps } as CSSProperties}>
            <ul className="lp-half" aria-label="Activos de la lista de seguimiento">
              {cells(false)}
            </ul>
            {/* Segunda mitad idéntica: el bucle sin costuras. Fuera del árbol de accesibilidad y del foco. */}
            <ul className="lp-half lp-dup" aria-hidden="true">
              {cells(true)}
            </ul>
          </div>
        </div>
      </div>
      {quoteState === 'error' ? (
        <p className="lp-notice" role="alert">
          <strong>No se pudieron cargar las cotizaciones.</strong> La cinta muestra los activos sin precio.
          <button type="button" className="lp-link" onClick={onRetry}>
            Reintentar
          </button>
        </p>
      ) : null}
      {staleAt !== null ? (
        <p className="lp-notice" role="alert">
          <strong>No se pudieron actualizar las cotizaciones.</strong> Última actualización: {formatDateTime(new Date(staleAt).toISOString())}.
          <button type="button" className="lp-link" onClick={onRetry}>
            Reintentar
          </button>
        </p>
      ) : null}
    </>
  )
}

function TapeBlock({ watchlist, ...tape }: TapeProps & { watchlist: Watchlist }) {
  let body
  if (watchlist.isPending) body = <p className="lp-tape-msg" role="status">Cargando la lista de seguimiento…</p>
  else if (watchlist.isError)
    body = (
      <p className="lp-tape-msg" role="alert">
        <strong>No se pudo cargar la lista de seguimiento.</strong> {errorText(watchlist.error)}
        <button type="button" className="lp-link" onClick={() => void watchlist.refetch()}>
          Reintentar
        </button>
      </p>
    )
  else if (tape.ids.length === 0) body = <p className="lp-tape-msg">La lista de seguimiento está vacía: no hay activos que mostrar.</p>
  else body = <Tape {...tape} />
  return <section className="lp-block lp-tapeblock" aria-label="Cotizaciones en vivo">{body}</section>
}

// --- Análisis y fuentes ----------------------------------------------------------------------

type StatementState =
  | { kind: 'loading' }
  | { kind: 'error'; error: unknown; retry: () => void }
  | { kind: 'empty' }
  | { kind: 'ok'; analysis: Analysis; currency: string | undefined }

function SourceRow({ n, s, asset, now, active, onActive }: { n: number; s: SourceRef; asset: AssetId; now: number; active: boolean; onActive: (n: number | null) => void }) {
  const at = s.publishedAt ? Date.parse(s.publishedAt) : Number.NaN
  const old = Number.isFinite(at) && now - at > OLD_SOURCE_MS
  const title = cleanTitle(s.title, asset)
  return (
    <li
      id={`fuente-${n}`}
      className="lp-src"
      data-active={active}
      data-old={old}
      onMouseEnter={() => onActive(n)}
      onMouseLeave={() => onActive(null)}
      onFocus={() => onActive(n)}
      onBlur={() => onActive(null)}
    >
      <span className="lp-src-n">{n}</span>
      <span className="lp-src-t" title={title}>
        <ExternalLink href={s.url}>{title}</ExternalLink>
      </span>
      <span className="lp-src-m">
        <span className="lp-src-p">{providerLabel(s.provider)}</span>
        <time className="lp-src-a" {...(s.publishedAt ? { dateTime: s.publishedAt } : {})}>
          {s.publishedAt ? formatRelative(s.publishedAt, now) : 'sin fecha'}
        </time>
      </span>
    </li>
  )
}

function FigureRow({ f, sources, active, onActive }: { f: Figure; sources: SourceRef[]; active: boolean; onActive: (n: number | null) => void }) {
  const target = f.ref ? sources[f.ref - 1] : undefined
  const handlers = f.ref
    ? { onMouseEnter: () => onActive(f.ref), onMouseLeave: () => onActive(null), onFocus: () => onActive(f.ref), onBlur: () => onActive(null) }
    : {}
  return (
    <div className="lp-fig" data-active={active} {...handlers}>
      <dt>{f.label}</dt>
      <dd className="lp-fig-v" style={{ '--lp-len': f.value.length } as CSSProperties}>
        {f.value}
        {f.unit ? <span className="lp-fig-u">{NBSP}{f.unit}</span> : null}
        {f.ref ? (
          <sup>
            <a href={`#fuente-${f.ref}`} aria-label={`Fuente ${f.ref}: ${target?.title ?? ''}`}>
              {f.ref}
            </a>
          </sup>
        ) : null}
      </dd>
      <dd className="lp-fig-o">{f.origin}</dd>
    </div>
  )
}

function Statement({ state }: { state: StatementState }) {
  const [active, setActive] = useState<number | null>(null)
  const now = useClock(60_000)
  const ok = state.kind === 'ok' ? state : null
  const a = ok?.analysis ?? null
  const sources = a?.sources ?? []
  const figures = ok ? figuresOf(ok.analysis, ok.currency) : []
  return (
    <div className="lp-first-body">
      <section className="lp-block lp-analysis" id="lp-analisis" aria-label="Último análisis de la lista">
        <div className="lp-grid">
          {a ? (
            <>
              <div className="lp-col-lead">
                {/* El tamaño sale de la columna (cqi) y del número de letras (--lp-len): ningún símbolo ni veredicto desborda. */}
                <h2 className="lp-mon">
                  <span className="lp-mon-sym" style={{ '--lp-len': symbolOf(a.asset).length } as CSSProperties}>{symbolOf(a.asset)}</span>{' '}
                  <span className="lp-verdict" style={{ '--lp-len': VERDICT_LABEL[a.verdict].length } as CSSProperties}>{VERDICT_LABEL[a.verdict]}</span>
                </h2>
                <p className="lp-sum">{a.summary}</p>
              </div>
              <div className="lp-col-side">
                <dl className="lp-figs" aria-label="Cifras del análisis">
                  {figures.map((f) => (
                    <FigureRow key={f.key} f={f} sources={sources} active={f.ref !== null && active === f.ref} onActive={setActive} />
                  ))}
                </dl>
                <p className="lp-meta">
                  {symbolOf(a.asset)}, {kindOf(a.asset)}. Guardado <time dateTime={a.generatedAt}>{formatRelative(a.generatedAt, now)}</time> con {a.model}; horizonte:{' '}
                  {HORIZON_LABEL[a.horizon].toLowerCase()}. Análisis automatizado: no es asesoramiento financiero.
                </p>
              </div>
            </>
          ) : (
            <div className="lp-col-lead">
              {state.kind === 'loading' ? <p className="lp-wait" role="status">Cargando el último análisis…</p> : null}
              {state.kind === 'error' ? (
                <div role="alert">
                  <h2 className="lp-h2">No se pudo cargar el análisis</h2>
                  <p className="lp-sum">{errorText(state.error)}</p>
                  <button type="button" className="lp-link" onClick={state.retry}>
                    Reintentar
                  </button>
                </div>
              ) : null}
              {state.kind === 'empty' ? (
                <>
                  <h2 className="lp-mon lp-mon-empty">La lista de seguimiento aún no ha guardado ningún análisis</h2>
                  <p className="lp-sum">
                    Si la lista de seguimiento está activa, se analiza sola una vez por día de bolsa, tras el cierre de EE. UU. El último análisis aparecerá aquí, con sus cifras y sus fuentes. Entra al panel para ver la lista o analizar un activo ahora.
                  </p>
                </>
              ) : null}
            </div>
          )}
        </div>
      </section>

      <div className="lp-block lp-foot">
        <div className="lp-grid">
          {/* Primero en el DOM: en móvil el bloque de acción va antes del índice y el foco sigue el orden visual. */}
          <Link to="/dashboard" className="lp-cta lp-col-side">Entrar al panel</Link>
          <section className="lp-col-wide" aria-labelledby="lp-src-h">
            <h2 className="lp-h3" id="lp-src-h">Fuentes de este análisis</h2>
            {a && sources.length > 0 ? (
              <ol className="lp-srcs" style={{ '--lp-rows': Math.ceil(sources.length / 2) } as CSSProperties}>
                {sources.map((s, i) => (
                  <SourceRow key={i} n={i + 1} s={s} asset={a.asset} now={now} active={active === i + 1} onActive={setActive} />
                ))}
              </ol>
            ) : (
              <p className="lp-quiet">{a ? 'Este análisis no cita ninguna fuente.' : 'Sin análisis no hay fuentes que citar.'}</p>
            )}
          </section>
        </div>
      </div>
    </div>
  )
}

// --- Bajo el pliegue -------------------------------------------------------------------------

/** Las notas de calidad del último análisis, tal cual: incluyen lo que el verificador descartó o marcó, y alguna nota más. */
function QualityProof({ analysis }: { analysis: Analysis }) {
  const notes = analysis.qualityNotes ?? []
  const sym = symbolOf(analysis.asset)
  return (
    <div className="lp-proof">
      <p className="lp-proof-h">
        {notes.length > 0 ? `Notas de calidad del último análisis de la lista, de ${sym}, tal cual:` : `El último análisis de la lista, de ${sym}, no dejó ninguna nota de calidad.`}
      </p>
      {notes.length > 0 ? (
        <ul>
          {notes.map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

function How({ analysis }: { analysis: Analysis | null }) {
  return (
    <section className="lp-block lp-how" aria-labelledby="lp-how-h">
      <div className="lp-grid">
        <h2 className="lp-h2 lp-col-label" id="lp-how-h">Cómo llega a cada cifra</h2>
        <ol className="lp-steps lp-col-body">
          <li>
            <h3>Datos en vivo</h3>
            <p>Cotización, histórico, fundamentales y noticias llegan de proveedores como Yahoo, Finnhub, FRED o la SEC. Cada dato conserva su fuente y su fecha.</p>
          </li>
          <li>
            <h3>Cifras calculadas en Rust</h3>
            <p>La puntuación global, los indicadores y los niveles de entrada, stop y objetivos los calcula el código de invest-ai. El modelo no los inventa.</p>
          </li>
          <li>
            <h3>El modelo local las explica</h3>
            <p>Un modelo de lenguaje que corre en este equipo, con Ollama, redacta el resumen y la tesis a partir de esas cifras. No hay nube ni cuentas.</p>
          </li>
          <li>
            <h3>El verificador descarta o marca lo que no cuadra</h3>
            <p>Si el modelo cita una cifra que no aparece en los datos, el verificador descarta esa afirmación o la marca con ⚠ en el texto, y deja una nota en el análisis.</p>
            {analysis ? <QualityProof analysis={analysis} /> : null}
          </li>
        </ol>
      </div>
    </section>
  )
}

function horizonLine(h: HorizonReport): string {
  if (h.resolved === 0) {
    const parts = [
      h.pending > 0 ? `${formatInt(h.pending)} pendientes de medir` : '',
      h.notMatured > 0 ? `${formatInt(h.notMatured)} esperando su plazo` : '',
    ].filter(Boolean)
    return parts.length > 0 ? parts.join(', ') : 'sin análisis'
  }
  if (!enoughData(h.overall) || h.overall.hitRate === null) return `${formatInt(h.resolved)} medidos: muestra pequeña, sin tasa`
  // Un porcentaje de acierto solo dice algo junto a la tasa base (como en la página de rendimiento); sin ella no se da.
  const base = h.baseRateOutperform
  if (base === null) return `${formatInt(h.resolved)} medidos: sin tasa base, no se da el porcentaje`
  const read = { above: 'por encima de', below: 'por debajo de', unclear: 'no se distingue de', none: 'no se distingue de' }[hitVerdict(h.overall, base)]
  return `acierto ${rateText(h.overall.hitRate, h.overall.hitRateCi)}, ${read} la tasa base (${formatPercent(base, 0)})`
}

function trackRecordHeadline(r: PerformanceReport): string {
  const resolved = Math.max(0, ...r.horizons.map((h) => h.resolved))
  const pending = Math.max(0, ...r.horizons.map((h) => h.pending))
  if (r.analysesTotal === 0) return 'Aún no hay análisis guardados, así que no hay nada que medir.'
  if (resolved === 0 && pending > 0) return `Todavía no hay resultados medidos: ${formatInt(r.analysesTotal)} análisis guardados, con ${formatInt(pending)} ya vencidos y pendientes de medir.`
  if (resolved === 0) return `Todavía no hay resultados que medir: ${formatInt(r.analysesTotal)} análisis guardados y ninguno ha cumplido su plazo.`
  return `${formatInt(resolved)} análisis ya medidos de ${formatInt(r.analysesTotal)} guardados. Se publica el resultado tal cual, aunque no favorezca.`
}

function TrackRecord() {
  const q = usePerformance()
  const r = q.data
  return (
    <section className="lp-block lp-record" aria-labelledby="lp-rec-h">
      <div className="lp-grid">
        <div className="lp-col-lead">
          <h2 className="lp-h2" id="lp-rec-h">Historial de aciertos</h2>
          {q.isPending ? <p className="lp-wait" role="status">Cargando el historial…</p> : null}
          {q.isError ? (
            <p className="lp-sum" role="alert">
              No se pudo cargar el historial. {errorText(q.error)}{' '}
              <button type="button" className="lp-link" onClick={() => void q.refetch()}>
                Reintentar
              </button>
            </p>
          ) : null}
          {r ? (
            <>
              <p className="lp-statement">{trackRecordHeadline(r)}</p>
              <p className="lp-quiet">
                Medido el <time dateTime={r.generatedAt}>{formatDate(r.generatedAt)}</time> con una regla fija, versión {r.ruleVersion}. Un análisis cuenta cuando cumple su plazo y se compara con el índice.
              </p>
            </>
          ) : null}
        </div>
        {r ? (
          <table className="lp-col-side lp-horizons">
            <caption className="lp-sr">Análisis medidos por plazo</caption>
            <thead>
              <tr>
                <th scope="col">Plazo</th>
                <th scope="col">Estado</th>
              </tr>
            </thead>
            <tbody>
              {r.horizons.map((h) => (
                <tr key={h.horizonWeeks}>
                  <th scope="row">{h.horizonWeeks} {h.horizonWeeks === 1 ? 'semana' : 'semanas'}</th>
                  <td>{horizonLine(h)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </div>
      <div className="lp-grid lp-backtest">
        <h3 className="lp-h3 lp-col-label">Backtest de la puntuación</h3>
        <p className="lp-col-body">
          El backtest de los factores de precio de la puntuación (114 símbolos, desde 2018, ejecutado el 16 de octubre de 2026) no mostró ventaja frente al mercado: ninguna de las 12 pruebas fijadas de antemano alcanzó el umbral y el compuesto no ganó nada tras costes. Los resultados completos, con sus límites, están en docs/notes/backtest.md.
        </p>
      </div>
    </section>
  )
}

function Closing() {
  return (
    <section className="lp-block lp-close" aria-label="Entrar">
      <div className="lp-grid">
        <p className="lp-col-label lp-disclaimer">Análisis automatizado con un modelo de IA local: no es asesoramiento financiero.</p>
        <Link to="/dashboard" className="lp-cta lp-cta-big lp-col-body">Entrar al panel</Link>
      </div>
    </section>
  )
}

// --- Página ----------------------------------------------------------------------------------

export default function LandingPage() {
  useTitle(null)
  const watchlist = useLandingWatchlist()
  const ids = useMemo(() => watchlist.data?.assets.map((a) => a.id).slice(0, MAX_QUOTE_IDS) ?? [], [watchlist.data])
  const quotes = useQuotes(ids)
  const quoteById = useMemo(() => new Map((quotes.data ?? []).map((q) => [q.id, q])), [quotes.data])
  // Tras un refresco fallido `data` conserva los precios viejos: se siguen enseñando, con su hora y un aviso.
  const quoteState: QuoteState = quotes.data ? 'ok' : quotes.isError ? 'error' : 'loading'
  const staleAt = quotes.isError && quotes.data ? quotes.dataUpdatedAt : null
  const latestId = watchlist.data ? latestAssetId(watchlist.data) : null
  // Misma clave que `useLatestAnalysis`, pero admite «todavía no sé qué activo»: así el árbol de la primera pantalla
  // es siempre el mismo y no se remonta al llegar los datos (un clic en «Entrar al panel» no se pierde a medias).
  const latest = useQuery({
    queryKey: latestId ? qk.analysis(latestId) : NO_ANALYSIS,
    queryFn: ({ signal }) => (latestId ? api.latestAnalysis(latestId, signal) : Promise.resolve(null)),
    enabled: latestId !== null,
    staleTime: STALE.analysis,
    ...REFRESH,
  })

  let state: StatementState
  if (watchlist.isPending) state = { kind: 'loading' }
  else if (watchlist.isError) state = { kind: 'error', error: watchlist.error, retry: () => void watchlist.refetch() }
  else if (!latestId) state = { kind: 'empty' }
  else if (latest.isPending) state = { kind: 'loading' }
  else if (latest.isError) state = { kind: 'error', error: latest.error, retry: () => void latest.refetch() }
  else if (!latest.data) state = { kind: 'empty' }
  else state = { kind: 'ok', analysis: latest.data, currency: quoteById.get(latest.data.asset)?.currency ?? undefined }

  return (
    <div className="lp">
      <a className="lp-skip" href="#lp-main">Saltar al análisis</a>
      <div className="lp-first">
        <Header watchlist={watchlist} />
        <TapeBlock watchlist={watchlist} ids={ids} quotes={quoteById} quoteState={quoteState} staleAt={staleAt} onRetry={() => void quotes.refetch()} />
        <main id="lp-main" tabIndex={-1} className="lp-main">
          <Statement state={state} />
        </main>
      </div>
      <How analysis={state.kind === 'ok' ? state.analysis : null} />
      <TrackRecord />
      <Closing />
    </div>
  )
}
