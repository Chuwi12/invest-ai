import type { IpoEntry, IpoRiskProfile, IpoStatus } from '@/api/types'
import { formatCompactCurrency, formatNumber } from './format'

export const STATUS_LABEL: Record<IpoStatus, string> = {
  expected: 'Prevista',
  priced: 'Precio fijado',
  filed: 'Presentada',
  withdrawn: 'Retirada',
  unknown: 'Sin estado',
}

/** Clase del chip de estado. */
export const STATUS_CHIP: Record<IpoStatus, string> = {
  expected: 'accent',
  priced: 'up',
  filed: '',
  withdrawn: 'warn',
  unknown: '',
}

/** Qué significa `date` según el estado (el calendario la reutiliza). */
export const DATE_MEANING: Record<IpoStatus, string> = {
  expected: 'Fecha prevista',
  priced: 'Primer día de cotización',
  filed: 'Fecha de presentación',
  withdrawn: 'Fecha',
  unknown: 'Fecha',
}

export const SECTION_LABEL: Record<string, string> = {
  summary: 'Resumen',
  offering: 'La oferta',
  summary_financial_data: 'Datos financieros resumidos',
  risk_factors: 'Factores de riesgo',
  use_of_proceeds: 'Destino de los fondos',
  dividend_policy: 'Política de dividendos',
  capitalization: 'Capitalización',
  dilution: 'Dilución',
  principal_stockholders: 'Accionistas principales',
  shares_eligible: 'Acciones con derecho a venta futura',
  lock_up: 'Lock-up',
  underwriting: 'Colocación (suscriptores)',
}

/** Datos inventados del modo simulado del servidor o del mock del frontend. */
export const isSimulated = (source: string | null | undefined): boolean => source === 'fake'

export interface CalendarGroups {
  /** Previstas y presentadas, por fecha ascendente (la más próxima primero). */
  upcoming: IpoEntry[]
  /** Con precio fijado y retiradas, la más reciente primero. */
  recent: IpoEntry[]
}

export function splitCalendar(rows: readonly IpoEntry[]): CalendarGroups {
  const upcoming = rows.filter((e) => e.status === 'expected' || e.status === 'filed' || e.status === 'unknown')
  const recent = rows.filter((e) => e.status === 'priced' || e.status === 'withdrawn')
  const rank = (e: IpoEntry) => (e.status === 'expected' ? 0 : 1)
  upcoming.sort((a, b) => rank(a) - rank(b) || a.date.localeCompare(b.date))
  recent.sort((a, b) => b.date.localeCompare(a.date))
  return { upcoming, recent }
}

/** «16,00 USD», «18,00–20,00 USD» o «—». */
export function priceText(e: Pick<IpoEntry, 'priceLow' | 'priceHigh'>): string {
  const { priceLow: lo, priceHigh: hi } = e
  if (lo === null && hi === null) return '—'
  if (lo !== null && hi !== null && lo !== hi) return `${formatNumber(lo, 2)}–${formatNumber(hi, 2)} USD`
  return `${formatNumber(lo ?? hi, 2)} USD`
}

export function sizeText(e: Pick<IpoEntry, 'totalValue' | 'status'>): string {
  if (e.totalValue === null) return '—'
  const v = formatCompactCurrency(e.totalValue, 'USD')
  return e.status === 'expected' ? `hasta ${v}` : v
}

/** La OPV puede tener rendimiento posterior: precio fijado y primer día ya pasado. */
export function canHavePerformance(e: IpoEntry | null | undefined, today: string): boolean {
  return !!e && e.status === 'priced' && e.date <= today
}

export interface ProfileCounts {
  extracted: number
  total: number
}

export function profileCounts(p: Pick<IpoRiskProfile, 'indicators'>): ProfileCounts {
  return { extracted: p.indicators.filter((i) => i.status === 'extracted').length, total: p.indicators.length }
}

export const todayIso = (now: number = Date.now()): string => new Date(now).toISOString().slice(0, 10)
