import { ArrowLeft, ExternalLink as ExternalIcon, Info, ShieldAlert } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import { useIpoBaseRate, useIpoCalendar, useIpoPerformance, useIpoProspectus, useIpoRiskProfile } from '@/api/hooks'
import type { IpoBaseRate, IpoBaseRateTier, IpoEntry, IpoPerformance, IpoRiskIndicator, IpoRiskProfile, Prospectus } from '@/api/types'
import { ExternalLink } from '@/components/ExternalLink'
import { Empty, ErrorNotice, Section, SkeletonRows } from '@/components/states'
import { useTitle } from '@/hooks/useTitle'
import { formatDate, formatInt, formatNumber, formatPoints, toneOf } from '@/lib/format'
import {
  canHavePerformance,
  DATE_MEANING,
  isSimulated,
  priceText,
  profileCounts,
  SECTION_LABEL,
  sizeText,
  splitCalendar,
  STATUS_CHIP,
  STATUS_LABEL,
  todayIso,
} from '@/lib/ipo'

type Tab = 'upcoming' | 'recent'

/** Aviso permanente: perfil de riesgo, no recomendación; la tasa base no predice. */
function RiskNotice() {
  return (
    <div className="notice warn" role="note" aria-label="Aviso sobre el perfil de riesgo">
      <ShieldAlert aria-hidden="true" />
      <div>
        <strong>Perfil de riesgo, no una recomendación.</strong> Resume hechos leídos del folleto con reglas fijas (sin IA) y dice cuándo no
        pudo leer algo. <strong>La tasa base de OPV recientes describe lo que pasó con otras OPV; no predice cómo irá una en concreto.</strong>
      </div>
    </div>
  )
}

function SimBadge({ source }: { source: string | null | undefined }) {
  return isSimulated(source) ? <span className="chip warn">Simulado</span> : null
}

function CalendarRow({ e, active }: { e: IpoEntry; active: boolean }) {
  const body = (
    <>
      <span className="ipo-sym num">{e.symbol ?? '—'}</span>
      <span className="ipo-name trunc">{e.name}</span>
      <span className={`chip ${STATUS_CHIP[e.status]}`}>{STATUS_LABEL[e.status]}</span>
      <span className="ipo-meta small muted num">
        {formatDate(e.date)} · {priceText(e)} · {sizeText(e)}
        {e.spacLikely ? ' · posible SPAC' : ''}
      </span>
    </>
  )
  if (!e.symbol) return <li className="ipo-row disabled" title="Sin símbolo: no se puede abrir la ficha">{body}</li>
  return (
    <li>
      <Link
        className="ipo-row"
        to={`/ipo/${encodeURIComponent(e.symbol)}?date=${e.date}`}
        aria-current={active ? 'true' : undefined}
        aria-label={`${e.name} (${e.symbol}), ${STATUS_LABEL[e.status].toLowerCase()}`}
      >
        {body}
      </Link>
    </li>
  )
}

function Calendar({ selected }: { selected: string | undefined }) {
  const cal = useIpoCalendar()
  const [tab, setTab] = useState<Tab>('upcoming')
  const groups = useMemo(() => splitCalendar(cal.data ?? []), [cal.data])
  const rows = groups[tab]
  return (
    <Section title="Calendario" hint="30 días atrás, 60 adelante" id="ipo-calendar">
      <div className="seg" role="group" aria-label="Calendario de OPV">
        <button type="button" aria-pressed={tab === 'upcoming'} onClick={() => setTab('upcoming')}>
          Próximas ({groups.upcoming.length})
        </button>
        <button type="button" aria-pressed={tab === 'recent'} onClick={() => setTab('recent')}>
          Recientes ({groups.recent.length})
        </button>
      </div>
      {cal.isPending ? <SkeletonRows rows={4} h="3rem" /> : null}
      {cal.isError ? <ErrorNotice error={cal.error} onRetry={() => void cal.refetch()} title="No se pudo cargar el calendario" /> : null}
      {cal.data && rows.length === 0 ? (
        <Empty>{tab === 'upcoming' ? 'No hay OPV previstas ni presentadas en esta ventana.' : 'No hay OPV recientes en esta ventana.'}</Empty>
      ) : null}
      {rows.length > 0 ? (
        <ul className="ipo-list panel flush" aria-label={tab === 'upcoming' ? 'OPV próximas' : 'OPV recientes'}>
          {rows.map((e) => (
            <CalendarRow key={`${e.symbol ?? e.name}-${e.date}-${e.status}`} e={e} active={!!selected && e.symbol?.toUpperCase() === selected.toUpperCase()} />
          ))}
        </ul>
      ) : null}
      {cal.data && cal.data.length > 0 ? (
        <p className="small muted">
          Fuente: {cal.data[0]?.source === 'finnhub' ? 'Finnhub' : cal.data[0]?.source === 'fake' ? 'datos simulados' : (cal.data[0]?.source ?? '—')}. La fecha es la prevista, la de
          presentación o el primer día de cotización según el estado.
        </p>
      ) : null}
    </Section>
  )
}

function IndicatorCard({ i }: { i: IpoRiskIndicator }) {
  const extracted = i.status === 'extracted'
  return (
    <article className={`ind ${extracted ? '' : 'missing'}`} aria-label={i.label}>
      <header>
        <h3>{i.label}</h3>
        {extracted ? null : <span className="chip">No extraído</span>}
      </header>
      {extracted ? <p className="ind-value">{i.value}</p> : <p className="small muted">{i.note ?? 'No se pudo leer.'}</p>}
      {i.flags.length > 0 ? (
        <ul className="ind-flags" aria-label="Banderas">
          {i.flags.map((f) => (
            <li key={f.code}>
              <span className={`chip ${f.attention ? 'warn' : ''}`} title={f.evidence}>
                {f.attention ? 'Atención: ' : ''}
                {f.label}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
      {i.evidence || i.flags.length > 0 ? (
        <details className="ind-evidence">
          <summary>Ver texto del folleto</summary>
          {i.evidence ? <blockquote>{i.evidence}</blockquote> : null}
          {i.flags.map((f) => (
            <blockquote key={f.code}>
              <strong className="small">{f.label}:</strong> {f.evidence}
            </blockquote>
          ))}
        </details>
      ) : null}
      {extracted && i.note ? <p className="small muted">{i.note}</p> : null}
      <p className="small muted ind-source">
        Fuente:{' '}
        {i.source.url ? (
          <ExternalLink href={i.source.url}>
            {i.source.label} <ExternalIcon aria-hidden="true" className="inline-icon" />
          </ExternalLink>
        ) : (
          i.source.label
        )}
      </p>
    </article>
  )
}

type RateStats = Pick<IpoBaseRateTier, 'nDay1' | 'n3m' | 'medianDay1Pct' | 'positiveDay1Pct' | 'median3mPct' | 'median3mExcessPct' | 'beatBenchmark3mPct'>

function RateGrid({ s, bench }: { s: RateStats; bench: string }) {
  const pp = (v: number | null) => formatPoints(v, 1)
  return (
    <dl className="stat-grid">
      <div>
        <dt>Mediana día 1 (n={s.nDay1})</dt>
        <dd className="num">{pp(s.medianDay1Pct)}</dd>
      </div>
      <div>
        <dt>Cerraron al alza el día 1</dt>
        <dd className="num">{s.positiveDay1Pct === null ? '—' : `${formatNumber(s.positiveDay1Pct, 0)} %`}</dd>
      </div>
      <div>
        <dt>Mediana a 3 meses (n={s.n3m})</dt>
        <dd className="num">{pp(s.median3mPct)}</dd>
      </div>
      <div>
        <dt>Exceso mediano vs {bench} a 3 meses</dt>
        <dd className="num">{pp(s.median3mExcessPct)}</dd>
      </div>
      <div>
        <dt>Batieron a {bench} a 3 meses</dt>
        <dd className="num">{s.beatBenchmark3mPct === null ? '—' : `${formatNumber(s.beatBenchmark3mPct, 0)} %`}</dd>
      </div>
    </dl>
  )
}

function BaseRatePanel({ b, tier }: { b: IpoBaseRate; tier: IpoBaseRateTier | null }) {
  const main: RateStats = tier ?? b
  return (
    <div className="stack">
      <p className="small">
        Tasa base de las <strong className="num">{formatInt(tier?.sample ?? b.sample)}</strong> OPV (sin SPAC) de los últimos {b.months} meses
        {tier ? (
          <>
            {' '}
            de tamaño comparable: <strong>OPV de {tier.label}</strong>
          </>
        ) : null}
        , frente a {b.benchmark}. <strong>No predice esta OPV:</strong> solo dice cómo le ha ido de media al mercado de OPV recientemente.
        {isSimulated(b.source) ? ' (Datos simulados.)' : ''}
      </p>
      <RateGrid s={main} bench={b.benchmark} />
      {main.n3m < 10 ? (
        <div className="notice warn" role="status">
          <Info aria-hidden="true" />
          <span>Muestra pequeña a 3 meses (n={main.n3m}): la mediana es poco fiable.</span>
        </div>
      ) : null}
      {b.tierNotes.map((n) => (
        <p key={n} className="small muted">{n}</p>
      ))}
      {!tier && b.tiers.length > 0 ? (
        <p className="small muted">No se pudo asignar esta OPV a un tramo de tamaño: se muestra la tasa global.</p>
      ) : null}
      {tier ? (
        <details className="ind-evidence">
          <summary>Referencia: todas las OPV (n={b.sample})</summary>
          <RateGrid s={b} bench={b.benchmark} />
          <p className="small muted">Mezcla micro-OPV y grandes, que se comportan distinto: por eso se compara con el tramo de tamaño.</p>
        </details>
      ) : null}
      <ul className="caveats small muted">
        {b.caveats.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
    </div>
  )
}

function ReturnCell({ v }: { v: number | null }) {
  if (v === null) return <td className="r muted">Pendiente</td>
  const tone = toneOf(v)
  return <td className={`r num ${tone === 'up' ? 'up' : tone === 'down' ? 'down' : ''}`}>{formatPoints(v, 1)}</td>
}

function PerformancePanel({ p }: { p: IpoPerformance }) {
  return (
    <div className="stack">
      <dl className="stat-grid">
        <div>
          <dt>Precio de la OPV</dt>
          <dd className="num">{formatNumber(p.offerPrice, 2)} USD</dd>
        </div>
        <div>
          <dt>Día 1 (cierre)</dt>
          <dd className="num">{formatPoints(p.firstDayReturnPct, 1)}</dd>
        </div>
        <div>
          <dt>Ahora</dt>
          <dd className="num">{formatPoints(p.latestReturnPct, 1)}</dd>
        </div>
        <div>
          <dt>Peor caída desde máximo</dt>
          <dd className="num">{formatPoints(p.maxDrawdownPct, 1)}</dd>
        </div>
      </dl>
      <div className="table-wrap panel flush">
        <table className="tbl" aria-label={`Rentabilidad de ${p.symbol} tras la OPV`}>
          <thead>
            <tr>
              <th className="tl">Horizonte</th>
              <th className="r">Desde el precio de la OPV</th>
              <th className="r">{p.benchmark} en el periodo</th>
              <th className="r">Exceso</th>
            </tr>
          </thead>
          <tbody>
            {p.points.map((x) => (
              <tr key={x.horizon}>
                <th scope="row" className="tl">{x.horizon.replace('m', ' meses').replace('1 meses', '1 mes')}</th>
                <ReturnCell v={x.returnPct} />
                <td className="r num">{x.benchmarkReturnPct === null ? '—' : formatPoints(x.benchmarkReturnPct, 1)}</td>
                <ReturnCell v={x.excessPct} />
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="small muted">
        Rentabilidad de precio desde el precio de colocación (solo la obtiene quien recibe acciones en la OPV; quien compra al abrir parte del
        cierre del primer día). «Pendiente» = aún no se alcanza el horizonte; nunca se extrapola.
        {p.warnings.length > 0 ? ` ${p.warnings.join(' ')}` : ''}
      </p>
    </div>
  )
}

function ProspectusPanel({ p }: { p: Prospectus }) {
  return (
    <div className="stack">
      <p className="small muted">
        {p.form} presentado el {formatDate(p.filedOn)}
        {p.preliminary ? ' (preliminar)' : ''} ·{' '}
        <ExternalLink href={p.url}>
          Documento original en EDGAR <ExternalIcon aria-hidden="true" className="inline-icon" />
        </ExternalLink>
        {p.missing.length > 0 ? ` · No localizadas: ${p.missing.map((k) => SECTION_LABEL[k] ?? k).join(', ')}.` : ''}
      </p>
      <div className="stack" style={{ gap: '0.4rem' }}>
        {p.sections.map((s) => (
          <details key={s.key} className="panel flush prospectus-sec">
            <summary>
              <span className="grow">{SECTION_LABEL[s.key] ?? s.key}</span>
              <span className="small muted num">{formatInt(s.originalChars)} caracteres</span>
            </summary>
            <div className="prospectus-body">
              <p className="small muted">Título original: «{s.title}»</p>
              <pre>{s.text}</pre>
              {s.truncated ? (
                <p className="small muted">
                  Texto recortado ({formatInt(s.text.length)} de {formatInt(s.originalChars)} caracteres):{' '}
                  <ExternalLink href={p.url}>léelo entero en EDGAR</ExternalLink>.
                </p>
              ) : null}
            </div>
          </details>
        ))}
      </div>
    </div>
  )
}

function DetailHeader({ profile }: { profile: IpoRiskProfile }) {
  const e = profile.entry
  const counts = profileCounts(profile)
  return (
    <div className="stack" style={{ gap: '0.5rem' }}>
      <div className="row wrap">
        <h2 className="ipo-title">{profile.name ?? profile.symbol}</h2>
        <span className="chip num">{profile.symbol}</span>
        {e ? <span className={`chip ${STATUS_CHIP[e.status]}`}>{STATUS_LABEL[e.status]}</span> : null}
        {e?.spacLikely ? <span className="chip warn">Posible SPAC</span> : null}
        <SimBadge source={e?.source} />
      </div>
      {e ? (
        <dl className="stat-grid compact">
          <div>
            <dt>{DATE_MEANING[e.status]}</dt>
            <dd className="num">{formatDate(e.date)}</dd>
          </div>
          <div>
            <dt>Precio</dt>
            <dd className="num">{priceText(e)}</dd>
          </div>
          <div>
            <dt>Tamaño</dt>
            <dd className="num">{sizeText(e)}</dd>
          </div>
          <div>
            <dt>Mercado</dt>
            <dd>{e.exchange ?? '—'}</dd>
          </div>
        </dl>
      ) : (
        <p className="small muted">La OPV no figura en el calendario consultado.</p>
      )}
      <p className="small muted">
        Indicadores leídos: <strong className="num">{counts.extracted}</strong> de <span className="num">{counts.total}</span>. Los demás figuran como
        «no extraído»: más vale eso que un número mal leído.
      </p>
    </div>
  )
}

function Detail({ symbol, date }: { symbol: string; date: string | undefined }) {
  const profile = useIpoRiskProfile(symbol, date)
  const prospectus = useIpoProspectus(symbol, !!profile.data?.prospectus)
  const p = profile.data
  const entry = p?.entry ?? null
  const showPerf = canHavePerformance(entry, todayIso())
  const perf = useIpoPerformance(symbol, entry?.date, showPerf)
  const fallbackBase = useIpoBaseRate(12, !!p && !p.baseRate)
  const base = p?.baseRate ?? fallbackBase.data ?? null

  if (profile.isPending) return <SkeletonRows rows={5} h="4rem" />
  if (profile.isError) return <ErrorNotice error={profile.error} onRetry={() => void profile.refetch()} title={`No se pudo cargar el perfil de ${symbol}`} />
  if (!p) return null

  return (
    <div className="stack" style={{ gap: '1.25rem' }}>
      <DetailHeader profile={p} />
      {p.warnings.length > 0 ? (
        <ul className="stack warn-list" aria-label="Avisos">
          {p.warnings.map((w) => (
            <li key={w} className="notice warn">
              <Info aria-hidden="true" />
              <span>{w}</span>
            </li>
          ))}
        </ul>
      ) : null}

      <Section title="Perfil de riesgo" hint="Cada dato con su fuente" id="ipo-risk">
        <p className="small muted">
          Las etiquetas ámbar señalan algo que conviene leer con más cuidado (p. ej. que los de dentro vendan o que la oferta sirva para devolver
          deuda); no son un veredicto. Despliega «Ver texto del folleto» para comprobar la frase de la que sale cada dato.
        </p>
        <div className="ind-grid">
          {p.indicators.map((i) => (
            <IndicatorCard key={i.key} i={i} />
          ))}
        </div>
      </Section>

      <Section title="Momento de mercado: tasa base" hint="Otras OPV recientes, no esta" id="ipo-base">
        {base ? (
          <BaseRatePanel b={base} tier={p.baseRateTier ?? null} />
        ) : fallbackBase.isError ? (
          <ErrorNotice error={fallbackBase.error} onRetry={() => void fallbackBase.refetch()} title="No se pudo calcular la tasa base" />
        ) : (
          <SkeletonRows rows={2} h="3rem" />
        )}
      </Section>

      {showPerf ? (
        <Section title="Rendimiento posterior" hint="Desde el precio de la OPV" id="ipo-perf">
          {perf.isPending ? <SkeletonRows rows={3} h="2.4rem" /> : null}
          {perf.isError ? <ErrorNotice error={perf.error} onRetry={() => void perf.refetch()} title="No se pudo cargar el rendimiento" /> : null}
          {perf.data ? <PerformancePanel p={perf.data} /> : null}
        </Section>
      ) : null}

      {p.prospectus ? (
        <Section title="Folleto" hint="Secciones plegables" id="ipo-prospectus">
          {prospectus.isPending ? <SkeletonRows rows={3} h="2.4rem" /> : null}
          {prospectus.isError ? <ErrorNotice error={prospectus.error} onRetry={() => void prospectus.refetch()} title="No se pudo cargar el folleto" /> : null}
          {prospectus.data ? <ProspectusPanel p={prospectus.data} /> : null}
        </Section>
      ) : null}

      <p className="small muted">{p.disclaimer}</p>
    </div>
  )
}

export default function IpoPage() {
  const { symbol } = useParams<{ symbol?: string }>()
  const [search] = useSearchParams()
  const date = search.get('date') ?? undefined
  useTitle(symbol ? `OPV ${symbol.toUpperCase()}` : 'OPV')

  return (
    <div className="page">
      <div className="page-head">
        <h1>OPV de EE. UU.</h1>
        <p className="muted small grow" style={{ maxWidth: '50rem' }}>
          Ofertas públicas de venta: calendario y, para cada una, un perfil de riesgo con lo que dice el folleto. Para entender una OPV, no para decidir
          si comprarla.
        </p>
      </div>
      <RiskNotice />
      <div className="ipo-layout" data-detail={symbol ? 'true' : 'false'}>
        <div className="ipo-cal">
          <Calendar selected={symbol} />
        </div>
        <div className="ipo-detail">
          {symbol ? (
            <>
              <Link to="/ipo" className="btn sm ghost ipo-back">
                <ArrowLeft aria-hidden="true" /> Volver al calendario
              </Link>
              <Detail symbol={symbol.toUpperCase()} date={date} />
            </>
          ) : (
            <div className="empty">
              <p>Elige una OPV del calendario para ver su perfil de riesgo.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

