import { ApiError } from '@/api/client'
import type {
  IpoBaseRate,
  IpoEntry,
  IpoPerformance,
  IpoReturnPoint,
  IpoRiskFlag,
  IpoRiskIndicator,
  IpoRiskProfile,
  IpoStatus,
  Prospectus,
  ProspectusSection,
} from '@/api/types'
import { hashString } from './rng'

/**
 * OPV simuladas (empresas inventadas). Reproducen el contrato de `/api/ipo/*`, incluidos los
 * indicadores «no extraído», para ver la interfaz sin backend. Los números son inventados.
 */

const DAY = 86_400_000
const iso = (offsetDays: number) => new Date(Date.now() + offsetDays * DAY).toISOString().slice(0, 10)
const AS_OF = '2025-01-02T15:30:00Z'
const EDGAR = 'https://www.sec.gov/edgar/search/'

interface Spec {
  symbol: string
  name: string
  exchange: string
  offset: number
  status: IpoStatus
  price: [number, number] | null
  shares: number | null
  spac?: boolean
  drift: [number, number]
}

const SPECS: Spec[] = [
  { symbol: 'LHTX', name: 'Lumen Harbor Therapeutics', exchange: 'NASDAQ Global Select', offset: -12, status: 'priced', price: [16, 16], shares: 12_500_000, drift: [31, 0] },
  { symbol: 'ORFS', name: 'Orbital Freight Systems', exchange: 'NYSE', offset: -25, status: 'priced', price: [24, 24], shares: 33_000_000, drift: [-6, -18] },
  { symbol: 'CBME', name: 'Cobalt Meridian Energy', exchange: 'NYSE', offset: 9, status: 'expected', price: [18, 20], shares: 20_000_000, drift: [0, 0] },
  { symbol: 'NRWF', name: 'Nimbus Row Foods', exchange: 'NASDAQ Global Market', offset: 23, status: 'expected', price: [12, 14], shares: 12_000_000, drift: [0, 0] },
  { symbol: 'HQACU', name: 'Helix Quarry Acquisition Corp.', exchange: 'NASDAQ Global', offset: -5, status: 'priced', price: [10, 10], shares: 20_000_000, spac: true, drift: [0.5, 0] },
  { symbol: 'VLPM', name: 'Verdant Loop Mobility', exchange: 'NASDAQ', offset: -3, status: 'filed', price: null, shares: null, drift: [0, 0] },
  { symbol: 'STRB', name: 'Saffron Tide Robotics', exchange: 'NASDAQ', offset: 15, status: 'withdrawn', price: null, shares: null, drift: [0, 0] },
]

function entryOf(s: Spec): IpoEntry {
  const total =
    s.shares === null || s.price === null
      ? null
      : s.status === 'priced'
        ? s.price[0] * s.shares
        : s.status === 'expected'
          ? s.price[1] * s.shares * 1.15
          : null
  return {
    date: iso(s.offset),
    symbol: s.symbol,
    name: s.name,
    exchange: s.exchange,
    status: s.status,
    priceLow: s.price?.[0] ?? null,
    priceHigh: s.price?.[1] ?? null,
    shares: s.shares,
    totalValue: total,
    spacLikely: s.spac ?? false,
    source: 'fake',
    sourceUrl: null,
    asOf: AS_OF,
  }
}

export function mockIpoCalendar(from?: string, to?: string): IpoEntry[] {
  const lo = from ?? iso(-30)
  const hi = to ?? iso(60)
  return SPECS.map(entryOf)
    .filter((e) => e.date >= lo && e.date <= hi)
    .sort((a, b) => a.date.localeCompare(b.date))
}

// --- Indicadores -----------------------------------------------------------------------------

const calendarSource = { kind: 'calendar' as const, section: null, label: 'Calendario de OPV (simulado)', url: null }
const src = (section: string, title: string) => ({
  kind: 'prospectus' as const,
  section,
  label: `Folleto 424B4 (simulado), sección «${title}»`,
  url: EDGAR,
})

function ok(
  key: string,
  label: string,
  section: string,
  title: string,
  value: string,
  extra: Partial<IpoRiskIndicator> = {},
): IpoRiskIndicator {
  return { key, label, status: 'extracted', value, numeric: null, unit: null, flags: [], evidence: null, note: null, source: src(section, title), ...extra }
}

function missing(key: string, label: string, section: string, title: string, note: string): IpoRiskIndicator {
  return { key, label, status: 'not_extracted', value: null, numeric: null, unit: null, flags: [], evidence: null, note, source: src(section, title) }
}

const flag = (code: string, label: string, attention: boolean, evidence: string): IpoRiskFlag => ({ code, label, attention, evidence })

function indicatorsFor(symbol: string, e: IpoEntry): IpoRiskIndicator[] {
  const size: IpoRiskIndicator = {
    key: 'offer_size',
    label: 'Tamaño de la oferta',
    status: e.totalValue === null ? 'not_extracted' : 'extracted',
    value: e.totalValue === null ? null : `${(e.totalValue / 1e6).toFixed(1).replace('.', ',')} M USD (${(e.shares ?? 0).toLocaleString('es-ES')} acciones)`,
    numeric: e.totalValue,
    unit: 'USD',
    flags: [],
    evidence: null,
    note: e.totalValue === null ? 'El calendario no da importe ni número de acciones (suele ocurrir en presentaciones sin precio).' : 'Acciones × precio, según el calendario.',
    source: calendarSource,
  }
  if (symbol === 'ORFS') {
    return [
      size,
      ok('use_of_proceeds', 'Destino de los fondos', 'use_of_proceeds', 'Use of Proceeds', 'Devolver deuda · Accionistas vendedores · Fines corporativos generales', {
        flags: [
          flag('debt_repayment', 'Devolver deuda', true, 'We intend to use the net proceeds to repay approximately $240 million of outstanding indebtedness under our term loan and for general corporate purposes.'),
          flag('selling_holders', 'Accionistas vendedores (la empresa no recibe esa parte)', true, 'We will not receive any proceeds from the sale of shares by the selling stockholders.'),
          flag('general_corporate', 'Fines corporativos generales', false, 'We intend to use the net proceeds to repay approximately $240 million of outstanding indebtedness under our term loan and for general corporate purposes.'),
        ],
        evidence: 'We estimate that we will receive net proceeds of approximately $330.7 million from the sale of shares in this offering.',
        note: 'Categorías detectadas por palabras clave en la sección; no mide importes ni su peso relativo.',
      }),
      ok('secondary_sale', 'Venta de accionistas existentes', 'offering', 'The Offering', '54,5 % de lo ofrecido lo venden accionistas existentes (18.000.000 de 33.000.000 acciones; la empresa vende 15.000.000)', {
        numeric: 54.5,
        unit: '%',
        flags: [flag('secondary', 'Venta secundaria: los de dentro venden', true, 'Common stock offered by the selling stockholders | 18,000,000 shares.')],
        evidence: 'Common stock offered by us | 15,000,000 shares. || Common stock offered by the selling stockholders | 18,000,000 shares.',
        note: 'Sin contar la opción de sobreasignación. Lo que venden los accionistas no entra en la caja de la empresa.',
      }),
      ok('float_offered', 'Capital ofrecido frente al total', 'offering', 'The Offering', '15,6 % (33.000.000 de 212.000.000 acciones)', {
        numeric: 15.6,
        unit: '%',
        evidence: 'Common stock offered by us | 15,000,000 shares. || Common stock to be outstanding immediately after this offering | 212,000,000 shares.',
        note: 'Acciones ofrecidas (sin sobreasignación) entre todas las acciones en circulación tras la oferta. No es el flotante real: el resto está en manos de los de dentro y sujeto a lock-up o restricciones.',
      }),
      ok('lock_up', 'Lock-up', 'lock_up', 'Lock-Up Agreements', '180 días', {
        numeric: 180,
        unit: 'días',
        evidence: 'have agreed with the underwriters that for a period of 180 days after the date of this prospectus we will not sell or transfer any shares, subject to exceptions.',
        note: 'Plazo que firman los suscriptores con la empresa y los accionistas de siempre; suele tener excepciones y salidas anticipadas.',
      }),
      ok('underwriters', 'Colocadores principales', 'underwriting', 'Underwriting', 'Example Capital Markets (45 %), Fictional Securities LLC (35 %), Sample Partners LLC (20 %)', {
        numeric: 3,
        unit: 'colocadores',
        evidence: '3 colocadores en la tabla; total 33.000.000',
        note: 'Porcentaje = acciones que cada colocador se compromete a comprar sobre el total. No se valora su reputación.',
      }),
      ok('dilution', 'Dilución por acción', 'dilution', 'Dilution', '21,40 USD por acción (89 % del precio de la OPV, 24,00 USD)', {
        numeric: 21.4,
        unit: 'USD/acción',
        evidence: 'Dilution per share to new investors in this offering | $21.40',
        note: 'Diferencia entre lo que paga el nuevo inversor por acción y el valor contable tangible pro forma por acción tras la oferta.',
      }),
      ok('voting', 'Clases de acciones y voto', 'offering', 'The Offering', 'Voto múltiple: una clase tiene hasta 10 votos por acción', {
        numeric: 10,
        unit: 'votos/acción',
        flags: [flag('multi_vote', 'Acciones con voto múltiple (los fundadores pueden controlar con menos capital)', true, 'Our Class A common stock will be entitled to one vote per share, and our Class B common stock will be entitled to ten votes per share.')],
        evidence: 'Our Class A common stock will be entitled to one vote per share, and our Class B common stock will be entitled to ten votes per share.',
        note: 'Buscado en: resumen, la oferta.',
      }),
      ok('controlled', 'Empresa controlada', 'summary', 'Prospectus Summary', 'Sí: se presenta como «controlled company» (un accionista controla ≈71 % de los votos)', {
        numeric: 71,
        unit: '%',
        flags: [flag('controlled_company', 'Mencionada como «controlled company»', true, 'As a “controlled company” under the listing rules we may exempt ourselves from some governance requirements.')],
        evidence: 'As a “controlled company” under the listing rules we may exempt ourselves from some governance requirements. Our sponsor will control approximately 71% of the voting power of our outstanding capital stock after this offering.',
        note: 'Una «empresa controlada» puede acogerse a exenciones de gobierno corporativo de la bolsa.',
      }),
      ok('net_result', 'Resultado neto', 'summary_financial_data', 'Summary Financial Data', 'Resultado mixto: pérdida en 1 y beneficio en 2 de los 3 periodos de la tabla', {
        evidence: 'Net income (loss) | $18,400 | $(22,700) | $31,200',
        note: 'cifras en miles de USD. Los periodos mezclan años completos y trimestres o semestres y su orden varía según el folleto: no se interpretan las columnas, solo el signo.',
      }),
      ok('revenue', 'Ingresos', 'summary_financial_data', 'Summary Financial Data', 'Ingresos en los 3 periodos de la tabla', {
        evidence: 'Revenue | $612,300 | $781,900 | $904,500',
        note: 'cifras en miles de USD. Fila tal como aparece; el orden de las columnas (años, semestres) no se interpreta.',
      }),
    ]
  }
  if (symbol === 'LHTX') {
    return [
      size,
      ok('use_of_proceeds', 'Destino de los fondos', 'use_of_proceeds', 'Use of Proceeds', 'Fines corporativos generales · Capital circulante · Desarrollo clínico o I+D', {
        flags: [
          flag('general_corporate', 'Fines corporativos generales', false, 'We intend to use the net proceeds to fund clinical and product development, working capital and other general corporate purposes.'),
          flag('working_capital', 'Capital circulante', false, 'We intend to use the net proceeds to fund clinical and product development, working capital and other general corporate purposes.'),
          flag('rnd', 'Desarrollo clínico o I+D', false, 'We intend to use the net proceeds to fund clinical and product development, working capital and other general corporate purposes.'),
        ],
        note: 'Categorías detectadas por palabras clave en la sección; no mide importes ni su peso relativo.',
      }),
      ok('secondary_sale', 'Venta de accionistas existentes', 'offering', 'The Offering', '0 % (solo se ofrecen 12.500.000 acciones nuevas; el folleto no menciona accionistas vendedores)', {
        numeric: 0,
        unit: '%',
        evidence: 'Common stock offered by us | 12,500,000 shares.',
      }),
      ok('float_offered', 'Capital ofrecido frente al total', 'offering', 'The Offering', '25,9 % (12.500.000 de 48.300.000 acciones)', { numeric: 25.9, unit: '%' }),
      ok('lock_up', 'Lock-up', 'lock_up', 'Lock-Up Agreements', '180 días', { numeric: 180, unit: 'días' }),
      missing('underwriters', 'Colocadores principales', 'underwriting', 'Underwriting', 'La tabla de suscriptores no termina en una fila «Total» legible ni su suma coincide con las acciones ofrecidas.'),
      ok('dilution', 'Dilución por acción', 'dilution', 'Dilution', '9,12 USD por acción (57 % del precio de la OPV, 16,00 USD)', { numeric: 9.12, unit: 'USD/acción' }),
      ok('voting', 'Clases de acciones y voto', 'offering', 'The Offering', 'No se detectó voto múltiple', { note: 'Buscado en: resumen, la oferta. La ausencia de coincidencias no prueba que no exista.' }),
      ok('controlled', 'Empresa controlada', 'offering', 'The Offering', 'No se detectó', { note: 'Buscado en: resumen, la oferta. La ausencia de coincidencias no prueba que no lo sea.' }),
      ok('net_result', 'Resultado neto', 'summary_financial_data', 'Summary Financial Data', 'Pérdida neta en los 3 periodos de la tabla', {
        flags: [flag('net_loss', 'Pérdidas en todos los periodos', true, 'Net loss | $(41,200) | $(63,900) | $(78,400)')],
        evidence: 'Net loss | $(41,200) | $(63,900) | $(78,400)',
        note: 'cifras en miles de USD. Los periodos mezclan años completos y trimestres o semestres y su orden varía según el folleto: no se interpretan las columnas, solo el signo.',
      }),
      missing('revenue', 'Ingresos', 'summary_financial_data', 'Summary Financial Data', 'No se encontró una fila «Revenue» / «Total revenues» en el resumen financiero (las filas con otro nombre, como «Collaboration revenue», no se usan).'),
    ]
  }
  // Resto: lo mínimo (sin folleto utilizable, casi todo «no extraído»).
  return [size].concat(
    ['use_of_proceeds', 'secondary_sale', 'float_offered', 'lock_up', 'underwriters', 'dilution', 'voting', 'controlled', 'net_result', 'revenue'].map((k) =>
      missing(k, LABELS[k] ?? k, 'offering', 'The Offering', 'Folleto no disponible.'),
    ),
  )
}

const LABELS: Record<string, string> = {
  use_of_proceeds: 'Destino de los fondos',
  secondary_sale: 'Venta de accionistas existentes',
  float_offered: 'Capital ofrecido frente al total',
  lock_up: 'Lock-up',
  underwriters: 'Colocadores principales',
  dilution: 'Dilución por acción',
  voting: 'Clases de acciones y voto',
  controlled: 'Empresa controlada',
  net_result: 'Resultado neto',
  revenue: 'Ingresos',
}

// --- Tasa base y rendimiento -----------------------------------------------------------------

export function mockIpoBaseRate(months = 12): IpoBaseRate {
  return {
    months,
    benchmark: 'SPY',
    sample: 27,
    nDay1: 27,
    n3m: 22,
    medianDay1Pct: 4.2,
    positiveDay1Pct: 63,
    median3mPct: 0.9,
    median3mExcessPct: -0.6,
    beatBenchmark3mPct: 45,
    symbols: ['LHTX', 'ORFS', 'GPKS', 'AMBR', 'KLRN', 'PNDL'],
    source: 'fake',
    asOf: AS_OF,
    caveats: [
      'Datos simulados (modo demostración): no son OPV reales.',
      'Rentabilidad de precio desde el precio de la OPV: solo la obtiene quien recibe acciones en la colocación, no quien compra al abrir.',
      'Muestra limitada y sin SPAC; las OPV que se deslistaron pueden faltar (sesgo de supervivencia).',
      'Mediana de otras OPV: describe el momento del mercado, no predice cómo irá una OPV concreta.',
    ],
  }
}

export function mockIpoPerformance(symbol: string): IpoPerformance {
  const s = SPECS.find((x) => x.symbol === symbol.toUpperCase())
  if (!s) throw new ApiError(404, 'not_found', `${symbol}: OPV desconocida`)
  const e = entryOf(s)
  if (e.status !== 'priced' || e.priceLow === null) throw new ApiError(404, 'not_found', `${symbol}: aún no cotiza`)
  const days = -s.offset
  const wob = (k: number) => ((hashString(`${symbol}${k}`) % 1000) / 1000 - 0.5) * 8
  const [d1, long] = s.drift
  const points: IpoReturnPoint[] = ([['1m', 1], ['3m', 3], ['6m', 6], ['12m', 12]] as const).map(([h, m]) => {
    const reached = days >= m * 30
    const r = reached ? d1 + (long - d1) * Math.min(m / 3, 2) + wob(m) : null
    const b = reached ? 1.4 * m : null
    return {
      horizon: h,
      targetDate: iso(s.offset + m * 30),
      priceDate: reached ? iso(s.offset + m * 30) : null,
      price: r === null ? null : e.priceLow! * (1 + r / 100),
      returnPct: r,
      sinceFirstClosePct: r === null ? null : ((1 + r / 100) / (1 + d1 / 100) - 1) * 100,
      benchmarkReturnPct: b,
      excessPct: r === null || b === null ? null : r - b,
    }
  })
  const latest = days >= 30 ? d1 + (long - d1) * Math.min(days / 90, 2) : d1
  return {
    symbol: s.symbol,
    ipoDate: e.date,
    offerPrice: e.priceLow,
    offerPriceSource: 'calendar',
    firstDayClose: e.priceLow * (1 + d1 / 100),
    firstDayReturnPct: d1,
    firstDayHighPct: d1 + 6,
    points,
    latestPrice: e.priceLow * (1 + latest / 100),
    latestDate: iso(0),
    latestReturnPct: latest,
    maxDrawdownPct: -(10 + (hashString(symbol) % 30)),
    benchmark: 'SPY',
    source: 'fake',
    sourceUrl: null,
    warnings: ['Datos simulados (modo demostración).'],
    asOf: AS_OF,
  }
}

// --- Folleto y perfil ------------------------------------------------------------------------

function sectionOf(key: string, title: string, text: string): ProspectusSection {
  return { key, title, text, parent: null, truncated: false, originalChars: text.length }
}

export function mockIpoProspectus(symbol: string): Prospectus {
  const s = SPECS.find((x) => x.symbol === symbol.toUpperCase())
  if (!s || s.status === 'filed' || s.status === 'withdrawn') throw new ApiError(404, 'not_found', `folleto no encontrado: ${symbol}`)
  const e = entryOf(s)
  const sections = [
    sectionOf('summary', 'Prospectus Summary', 'Texto de ejemplo del resumen del folleto (simulado). Describe la empresa, su mercado y la oferta.'),
    sectionOf('offering', 'The Offering', 'Common stock offered by us | 12,500,000 shares.\nCommon stock to be outstanding immediately after this offering | 48,300,000 shares.'),
    sectionOf('risk_factors', 'Risk Factors', 'Invertir en estas acciones conlleva un alto grado de riesgo (texto de ejemplo simulado).'),
    sectionOf('use_of_proceeds', 'Use of Proceeds', 'We intend to use the net proceeds to fund clinical and product development, working capital and other general corporate purposes.'),
    sectionOf('lock_up', 'Lock-Up Agreements', 'We, our directors and executive officers have agreed with the underwriters that for a period of 180 days after the date of this prospectus we will not sell or transfer any shares.'),
  ]
  return {
    issuer: s.name,
    cik: 9_000_000,
    ticker: s.symbol,
    form: e.status === 'priced' ? '424B4' : 'S-1/A',
    preliminary: e.status !== 'priced',
    filedOn: iso(s.offset - 1),
    accession: '0000000000-00-000000',
    url: EDGAR,
    indexUrl: EDGAR,
    offerPrice: e.priceHigh,
    sections,
    missing: ['capitalization', 'dividend_policy', 'principal_stockholders', 'shares_eligible'],
    documentChars: sections.reduce((n, x) => n + x.originalChars, 0),
    source: 'fake',
    fetchedAt: AS_OF,
  }
}

export function mockIpoRiskProfile(symbol: string): IpoRiskProfile {
  const s = SPECS.find((x) => x.symbol === symbol.toUpperCase())
  if (!s) throw new ApiError(404, 'not_found', 'no se encontró la OPV ni su folleto')
  const e = entryOf(s)
  const hasProspectus = s.status === 'priced' || s.status === 'expected'
  const warnings: string[] = []
  if (s.status === 'expected') warnings.push('El folleto disponible es preliminar (S-1/A): el precio, el número de acciones y las cifras pueden cambiar en el definitivo (424B4).')
  if (s.status === 'expected' || s.status === 'filed') warnings.push('La OPV aún no cotiza: precio y acciones pueden cambiar hasta la fijación del precio.')
  if (s.spac) warnings.push('Probable SPAC (sociedad vehículo sin negocio): este perfil está pensado para empresas operativas; en una SPAC no hay ingresos ni resultados y su riesgo es otro (fusión posterior).')
  if (!hasProspectus) warnings.push('No se pudo obtener el folleto: los indicadores que dependen de él quedan como «no extraído».')
  return {
    symbol: s.symbol,
    name: s.name,
    entry: e,
    prospectus: hasProspectus
      ? { form: s.status === 'priced' ? '424B4' : 'S-1/A', preliminary: s.status !== 'priced', filedOn: iso(s.offset - 1), url: EDGAR, indexUrl: EDGAR, documentChars: 420_000, missing: ['capitalization', 'dividend_policy', 'principal_stockholders', 'shares_eligible'] }
      : null,
    indicators: indicatorsFor(s.symbol, e),
    baseRate: mockIpoBaseRate(12),
    warnings,
    disclaimer:
      'Perfil de riesgo, no una recomendación ni una predicción. Resume hechos leídos del folleto y del calendario con reglas fijas; lo que no se pudo leer figura como «no extraído». La tasa base de OPV recientes describe lo ocurrido con otras OPV y no predice cómo irá esta.',
    generatedAt: AS_OF,
  }
}
