import type {
  Analysis,
  AnalysisDepth,
  AssetId,
  AssetSnapshot,
  Catalyst,
  Horizon,
  MacroContext,
  NewsItem,
  PositionSizing,
  Scenario,
  Sentiment,
  SentimentLabel,
  SourceRef,
  Verdict,
} from '@/api/types'
import { classOf, symbolOfId, toSummary } from './catalog'
import { currentPrice, entryFor, genFundamentals, genIndicators, genQuote } from './market'
import { between, hashString, pick, rngFor } from './rng'
import { activeModelId } from './models'

const HOUR = 3_600_000

// --- Noticias --------------------------------------------------------------------------------

const PUBLISHERS = ['Reuters', 'Bloomberg', 'Expansión', 'Cinco Días', 'Financial Times', 'CNBC', 'El Economista', 'Investing.com']

const POSITIVE = [
  '{n} supera las previsiones de beneficios del trimestre',
  'Varias casas de análisis elevan el precio objetivo de {n}',
  '{n} anuncia un programa de recompra de acciones',
  'Los flujos institucionales apuntan a {n} tras un mes de dudas',
]
const NEGATIVE = [
  '{n} cae tras conocerse un aumento de costes',
  'Un regulador abre una investigación sobre {n}',
  'Recortan la recomendación de {n} por valoración exigente',
  'Toma de beneficios en {n} después de máximos recientes',
]
const NEUTRAL = [
  'Qué esperar de {n} esta semana',
  '{n} mantiene su guía de resultados para el año',
  'Resumen de la sesión: {n} cotiza de lado con poco volumen',
  'Los analistas, divididos ante el próximo movimiento de {n}',
]
const CRYPTO_EXTRA = [
  'Los ETF al contado de {n} registran entradas netas por tercera semana',
  'El hash rate de {n} marca un nuevo máximo histórico',
]

const LABELS: [SentimentLabel, number][] = [
  ['very_negative', -0.8],
  ['negative', -0.4],
  ['neutral', 0],
  ['positive', 0.4],
  ['very_positive', 0.8],
]

function sentimentFor(bucket: 'pos' | 'neg' | 'neu', r: () => number): Sentiment {
  const idx = bucket === 'pos' ? (r() > 0.7 ? 4 : 3) : bucket === 'neg' ? (r() > 0.7 ? 0 : 1) : 2
  const [label, base] = LABELS[idx] as [SentimentLabel, number]
  return { score: Math.round((base + between(r, -0.12, 0.12)) * 100) / 100, label, method: r() > 0.5 ? 'llm' : 'lexicon' }
}

export function genNews(id: AssetId | null, limit: number): NewsItem[] {
  const r = rngFor(id ?? 'market', 'news')
  const name = id ? entryFor(id).name.replace(/,? (Inc\.|S\.A\.|N\.V\.|SE|Corporation|Corp\.)$/, '') : 'el mercado'
  const out: NewsItem[] = []
  const n = Math.min(limit, id ? 30 : 12)
  for (let i = 0; i < n; i++) {
    const roll = r()
    const bucket = roll < 0.38 ? 'pos' : roll < 0.68 ? 'neg' : 'neu'
    let pool = bucket === 'pos' ? POSITIVE : bucket === 'neg' ? NEGATIVE : NEUTRAL
    if (id && classOf(id) === 'crypto' && r() > 0.6) pool = CRYPTO_EXTRA
    const title = id
      ? pick(r, pool).replace('{n}', name)
      : pick(r, [
          'Wall Street cierra mixto a la espera de los datos de inflación',
          'El BCE mantiene los tipos y avisa de que el recorte será gradual',
          'El petróleo rebota por las tensiones geopolíticas en Oriente Medio',
          'El oro roza máximos históricos ante la caída del dólar',
          'Las tecnológicas lideran las subidas en Europa',
          'El IBEX 35 se apoya en la banca para cerrar en positivo',
          'Bitcoin supera los 67.000 dólares con volumen creciente',
          'La Fed deja la puerta abierta a un nuevo recorte en diciembre',
          'El Tesoro de EE. UU. coloca deuda a 10 años con menor demanda',
          'Los inversores rotan hacia valores defensivos',
        ])
    const hoursAgo = i * between(r, 1.2, 9) + between(r, 0.1, 1)
    const publisher = pick(r, PUBLISHERS)
    const h = hashString(`${id ?? 'm'}|${i}|${title}`).toString(16)
    out.push({
      id: h,
      title,
      summary:
        r() > 0.3
          ? 'Resumen de ejemplo de la noticia (datos simulados). Incluye contexto del movimiento y la reacción de los analistas.'
          : null,
      url: `https://example.com/noticias/${h}`,
      publisher,
      publishedAt: new Date(Date.now() - hoursAgo * HOUR).toISOString(),
      imageUrl: null,
      tickers: id ? [symbolOfId(id)] : [],
      sentiment: r() > 0.12 ? sentimentFor(bucket, r) : null,
      provider: pick(r, ['gdelt', 'google_news', 'yahoo_rss', 'finnhub']),
      language: 'es',
    })
  }
  return out
}

export function genMacro(): MacroContext {
  const now = new Date().toISOString()
  const ind = (key: string, name: string, value: number, change: number | null, unit: string, source = 'fred') => ({
    key, name, value, change, unit, asOf: now, source,
  })
  return {
    asOf: now,
    indicators: [
      ind('fed_funds', 'Tipo de la Fed', 4.75, 0, '%'),
      ind('us10y', 'Bono EE. UU. 10 años', 4.18, 0.03, '%'),
      ind('us2y', 'Bono EE. UU. 2 años', 3.62, 0.01, '%'),
      ind('curve_10y2y', 'Curva 10a-2a', 0.56, 0.02, 'pp'),
      ind('cpi_yoy', 'IPC interanual (EE. UU.)', 2.4, -0.1, '%'),
      ind('unemployment', 'Desempleo (EE. UU.)', 4.1, 0, '%'),
      ind('vix', 'VIX', 20.4, -0.6, 'pts', 'yahoo'),
      ind('ecb_rate', 'Tipo depósito BCE', 3.25, 0, '%', 'ecb'),
    ],
  }
}

export function genSnapshot(id: AssetId): AssetSnapshot {
  const missing: string[] = []
  const fundamentals = genFundamentals(id)
  if (!fundamentals && ['stock', 'etf', 'crypto'].includes(classOf(id))) missing.push('fundamentals: proveedor sin datos')
  return {
    summary: toSummary(entryFor(id)),
    quote: genQuote(id),
    fundamentals,
    indicators: genIndicators(id),
    news: genNews(id, 8),
    macroContext: genMacro(),
    missing,
    generatedAt: new Date().toISOString(),
  }
}

// --- Análisis de IA --------------------------------------------------------------------------

const clamp = (v: number, lo = 0, hi = 100) => Math.max(lo, Math.min(hi, v))

function verdictFor(overall: number, risk: number): Verdict {
  if (overall >= 74 && risk < 70) return 'buy'
  if (overall >= 62) return 'accumulate'
  if (overall >= 50) return 'hold'
  if (overall >= 42) return 'wait'
  if (overall >= 32) return 'reduce'
  return 'avoid'
}

export interface GenAnalysisOptions {
  depth?: AnalysisDepth
  /** Duración medida; si falta se inventa una verosímil (solo para los análisis sembrados del histórico). */
  durationMs?: number
}

const DEPTH_NAME: Record<AnalysisDepth, string> = { quick: 'rápido', standard: 'estándar', deep: 'profundo' }

export function genAnalysis(id: AssetId, serial = 0, generatedAt: Date = new Date(), opts: GenAnalysisOptions = {}): Analysis {
  const entry = entryFor(id)
  const cls = classOf(id)
  const ind = genIndicators(id)
  const fund = genFundamentals(id)
  const news = genNews(id, 6)
  const r = rngFor(id, 'analysis', serial)
  const price = currentPrice(id)
  const sym = symbolOfId(id)

  const rsi = ind.rsi14 ?? 50
  const trendScore = { strong_up: 85, up: 68, sideways: 50, down: 32, strong_down: 15 }[ind.trend ?? 'sideways']
  const technical = clamp(trendScore + (rsi > 72 ? -12 : rsi < 30 ? 8 : 0) + between(r, -6, 6))
  const momentum = clamp(50 + (ind.return3m ?? 0) * 140 + (ind.return1m ?? 0) * 80 + between(r, -5, 5))
  const fundamental = fund ? clamp(between(r, 35, 88)) : null
  const valuation = fund?.peTrailing ? clamp(100 - (fund.peTrailing - 10) * 1.9 + between(r, -8, 8)) : null
  const avgSent = news.reduce((s, n) => s + (n.sentiment?.score ?? 0), 0) / (news.length || 1)
  const sentiment = clamp(50 + avgSent * 60 + between(r, -4, 4))
  const macroScore = clamp(between(r, 38, 68))
  const risk = clamp((ind.volatility30d ?? 0.3) * 120 + between(r, -6, 6))
  const parts = [technical, momentum, fundamental, valuation, sentiment, macroScore].filter((v): v is number => v !== null)
  const overall = clamp(parts.reduce((s, v) => s + v, 0) / parts.length - Math.max(0, risk - 60) * 0.25)
  const verdict = verdictFor(overall, risk)
  const confidence = Math.round(clamp(0.5 + Math.abs(overall - 50) / 90 + between(r, -0.08, 0.1), 0.35, 0.93) * 100) / 100
  const horizon: Horizon = pick(r, ['short', 'medium', 'medium', 'long'] as const)

  const atr = ind.atr14 ?? price * 0.02
  const catalysts: Catalyst[] = []
  const dayMs = 86_400_000
  const iso = (days: number) => new Date(generatedAt.getTime() + days * dayMs).toISOString().slice(0, 10)
  if (cls === 'stock') {
    catalysts.push({ date: fund?.nextEarningsDate ?? iso(21), description: 'Publicación de resultados trimestrales', impact: 0.4 })
  }
  catalysts.push({ date: iso(Math.round(between(r, 3, 12))), description: 'Datos de inflación en EE. UU. (IPC)', impact: -0.2 })
  catalysts.push({ date: iso(Math.round(between(r, 14, 40))), description: 'Reunión de política monetaria de la Fed', impact: 0.1 })
  if (cls === 'crypto') catalysts.push({ date: null, description: 'Flujos de los ETF al contado', impact: 0.5 })

  const sources: SourceRef[] = [
    { title: 'Cotización e histórico', url: null, provider: cls === 'crypto' ? 'binance' : 'yahoo', publishedAt: generatedAt.toISOString() },
    ...news.slice(0, 3).map((n) => ({ title: n.title, url: n.url, provider: n.provider, publishedAt: n.publishedAt })),
  ]

  const rsiText = rsi >= 70 ? 'en sobrecompra' : rsi <= 30 ? 'en sobreventa' : 'en zona neutral'
  const trendText = { strong_up: 'una tendencia alcista fuerte', up: 'una tendencia alcista', sideways: 'un tramo lateral', down: 'una tendencia bajista', strong_down: 'una tendencia bajista fuerte' }[ind.trend ?? 'sideways']
  const fmt = (v: number) => v.toLocaleString('es-ES', { maximumFractionDigits: v < 1 ? 6 : 2 })

  const summaryByVerdict: Record<Verdict, string> = {
    buy: `${entry.name} combina ${trendText} con un RSI ${rsiText} y un flujo de noticias favorable. El perfil riesgo/beneficio es atractivo a este precio.`,
    accumulate: `${entry.name} muestra ${trendText} y fundamentos razonables, aunque la entrada de una sola vez es arriesgada. Conviene escalonar las compras.`,
    hold: `${entry.name} ofrece señales mixtas: ${trendText} con RSI ${rsiText}. Si ya se tiene, mantener; no hay una ventaja clara para aumentar posición.`,
    wait: `${entry.name} atraviesa ${trendText} y la valoración no deja margen de seguridad. Es preferible esperar un mejor punto de entrada o un catalizador.`,
    reduce: `${entry.name} acumula señales de deterioro (${trendText}) y la relación riesgo/beneficio se ha vuelto desfavorable. Reducir exposición de forma gradual.`,
    avoid: `${entry.name} presenta ${trendText}, momentum negativo y riesgos elevados. No hay motivos técnicos ni fundamentales para entrar ahora.`,
  }

  const lows = ind.supports
  const highs = ind.resistances
  const warnings: string[] = []
  if (!fund && cls !== 'index' && cls !== 'forex') warnings.push('No se pudieron obtener datos fundamentales; el análisis se basa solo en precio y noticias.')
  if (entry.delay) warnings.push(`El precio tiene un retraso de ${Math.round(entry.delay / 60)} minutos.`)
  if (r() > 0.6) warnings.push('El sentimiento de noticias se calculó con léxico, no con el LLM, para 2 de los 6 titulares.')

  const depth: AnalysisDepth = opts.depth ?? 'standard'
  const model = activeModelId()
  const durationMs = opts.durationMs ?? Math.round(
    ({ quick: between(r, 38_000, 52_000), standard: between(r, 45_000, 62_000), deep: between(r, 64_000, 82_000) })[depth],
  )
  const llmCalls = { quick: 3, standard: 6, deep: 9 }[depth] + (r() > 0.7 ? 1 : 0)
  const promptTokens = Math.round(llmCalls * between(r, 1500, 2600))
  const completionTokens = Math.round(llmCalls * between(r, 350, 700))
  const cur = entry.currency ?? ''
  const withMoney = (v: number) => `${fmt(v)} ${cur}`.trim()

  const changeIf = [
    `El precio cierra por debajo de ${withMoney(lows[0] ?? price - atr * 2)} con volumen superior a la media`,
    'El próximo informe de resultados queda por debajo de las previsiones de los analistas',
    'El RSI de 14 sesiones supera 75 sin confirmación de nuevos máximos',
  ]
  const uncertainty = 'El análisis usa solo precio, noticias y fundamentales disponibles; no incluye flujos de órdenes ni posicionamiento de grandes fondos.'
  const thesisTail = `\n\nIncertidumbre: ${uncertainty}\n\nLa conclusión cambiaría si:\n${changeIf.map((c) => `- ${c}`).join('\n')}`

  const noLevels = verdict === 'avoid'
  const levelsNote = noLevels
    ? 'No se propone stop ni objetivos porque el veredicto es no entrar: sin posición no hay nada que proteger.'
    : 'Stop = soporte más cercano − 1×ATR14; objetivos = resistencias de los datos. El modelo no decide estos precios.'

  const bullT = highs[0] ?? price + atr * 3
  const baseT = price + (bullT - price) * 0.15
  const bearT = lows[0] ?? price - atr * 3
  const pBull = Math.round(clamp(20 + (overall - 50) * 0.6 + between(r, -4, 4), 10, 55))
  const pBear = Math.round(clamp(20 + (50 - overall) * 0.6 + between(r, -4, 4), 10, 55))
  const pBase = 100 - pBull - pBear
  const ret = (t: number) => Math.round(((t - price) / price) * 1000) / 10
  const scenarios: Scenario[] | null = depth === 'deep' ? [
    { kind: 'bullish', probability: pBull, description: 'Rompe la resistencia con volumen y el flujo de noticias se mantiene favorable.', targetPrice: bullT, targetBasis: 'resistencia más cercana de los datos', returnPct: ret(bullT) },
    { kind: 'base', probability: pBase, description: 'Se mueve en rango, con rebotes limitados entre soporte y resistencia mientras se digieren los resultados.', targetPrice: baseT, targetBasis: 'punto intermedio del rango entre soporte y resistencia', returnPct: ret(baseT) },
    { kind: 'bearish', probability: pBear, description: 'Pierde el soporte tras una sorpresa macro o de resultados y corrige hasta la zona siguiente.', targetPrice: bearT, targetBasis: 'soporte más cercano de los datos', returnPct: ret(bearT) },
  ] : null
  const expectedReturnPct = scenarios
    ? Math.round(scenarios.reduce((t, sc) => t + (sc.probability / 100) * (sc.returnPct ?? 0), 0) * 10) / 10
    : null
  const stopPct = Math.round(((atr * 2.5) / price) * 1000) / 10
  const atrPct = Math.round((atr / price) * 1000) / 10
  const maxPortfolioPct = Math.round(clamp(2 / Math.max(stopPct, 1) * 10, 1, 10) * 10) / 10
  const positionSizing: PositionSizing | null = verdict === 'avoid' ? null : {
    maxPortfolioPct,
    riskPerTradePct: 1,
    stopDistancePct: stopPct,
    atrPct,
    volatility30d: ind.volatility30d ?? null,
    rationale: `Arriesgando el 1 % de la cartera con un stop a ${fmt(stopPct)} % de la entrada, la posición máxima orientativa es del ${fmt(maxPortfolioPct)} % de la cartera. El ATR diario es del ${fmt(atrPct)} %.`,
  }
  const qualityNotes = r() > 0.5 ? [
    'Se descartó 1 afirmación del modelo porque la cifra no aparecía en los datos.',
    ...(depth === 'quick' ? [] : ['Una frase se reescribió tras detectar redacción dudosa.']),
  ] : null
  const method = `Modelo ${model}, análisis ${DEPTH_NAME[depth]}, ${llmCalls} llamadas, ${promptTokens + completionTokens} tokens, ${Math.round(durationMs / 1000)} s. Análisis automatizado: no es asesoramiento financiero.`

  return {
    id: `an_${hashString(`${id}|${serial}|${generatedAt.getTime()}`).toString(16)}`,
    asset: id,
    verdict,
    confidence,
    horizon,
    scores: {
      overall: Math.round(overall),
      technical: Math.round(technical),
      momentum: Math.round(momentum),
      fundamental: fundamental === null ? null : Math.round(fundamental),
      valuation: valuation === null ? null : Math.round(valuation),
      sentiment: Math.round(sentiment),
      macroScore: Math.round(macroScore),
      risk: Math.round(risk),
    },
    summary: summaryByVerdict[verdict],
    thesis: `${sym} cotiza a ${fmt(price)} ${entry.currency ?? ''} con ${trendText}. El RSI de 14 sesiones está ${rsiText} (${fmt(rsi)}) y la volatilidad a 30 días es del ${fmt((ind.volatility30d ?? 0) * 100)} %. ${
      fund?.peTrailing ? `El PER trailing de ${fmt(fund.peTrailing)} ${fund.peTrailing > 30 ? 'incorpora expectativas de crecimiento altas' : 'resulta razonable para su sector'}. ` : ''
    }La conclusión se apoya en los datos del snapshot y no constituye asesoramiento financiero.${thesisTail}`,
    bullCase: [
      `Tendencia ${ind.trend === 'down' || ind.trend === 'strong_down' ? 'a revertir si recupera la media de 50 sesiones' : 'favorable con precio sobre sus medias'}`,
      'Flujo de noticias con sentimiento neto positivo en la última semana',
      cls === 'stock' ? 'Márgenes y generación de caja por encima de la media del sector' : 'Liquidez alta y demanda institucional creciente',
    ],
    bearCase: [
      rsi > 65 ? 'RSI elevado: riesgo de corrección a corto plazo' : 'Momentum todavía débil frente al mercado',
      'Valoración exigente si los resultados decepcionan',
      'Sensibilidad a un giro en las expectativas de tipos de interés',
    ],
    risks: [
      'Volatilidad macroeconómica (inflación, bancos centrales)',
      'Riesgo regulatorio y geopolítico',
      cls === 'crypto' ? 'Riesgo de liquidez y de custodia en mercados 24/7' : 'Riesgo de concentración sectorial',
    ],
    catalysts,
    keyLevels: {
      supports: lows,
      resistances: highs,
      entryZone: verdict === 'buy' || verdict === 'accumulate' ? [price - atr * 1.2, price + atr * 0.2] : null,
      stopLoss: verdict === 'avoid' ? null : price - atr * 2.5,
      targets: verdict === 'avoid' || verdict === 'reduce' ? [] : [price + atr * 3, price + atr * 5.5],
    },
    strategy:
      verdict === 'buy' || verdict === 'accumulate'
        ? 'Entrar en 3 tramos (40 % / 30 % / 30 %) dentro de la zona de entrada, con stop bajo el último soporte y sin superar el 5 % de la cartera.'
        : verdict === 'hold'
          ? 'Mantener la posición actual y revisar si pierde el soporte más cercano o si mejoran los fundamentos.'
          : null,
    dataWarnings: warnings,
    sources,
    priceAtAnalysis: price,
    model,
    generatedAt: generatedAt.toISOString(),
    levelsNote,
    scenarios,
    expectedReturnPct,
    wouldChangeIf: changeIf,
    horizonNote: `Horizonte ${horizon === 'short' ? 'corto' : horizon === 'long' ? 'largo' : 'medio'} porque los catalizadores identificados se concentran en las próximas semanas y la volatilidad a 30 días es del ${fmt((ind.volatility30d ?? 0) * 100)} %.`,
    positionSizing,
    method,
    qualityNotes,
    depth,
    durationMs,
    llmCalls,
    promptTokens,
    completionTokens,
  }
}
