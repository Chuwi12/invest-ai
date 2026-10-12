import type { Interval, Range } from '@/api/types'

/** Intervalos que tienen sentido (y suelen estar disponibles) para cada rango. */
export function intervalsFor(range: Range): Interval[] {
  switch (range) {
    case '1d': return ['1m', '5m', '15m', '30m', '1h']
    case '5d': return ['5m', '15m', '30m', '1h', '1d']
    case '1mo': return ['15m', '30m', '1h', '1d']
    case '3mo': case '6mo': case 'ytd': return ['1h', '1d', '1wk']
    case '1y': case '2y': return ['1d', '1wk', '1mo']
    case '5y': case '10y': return ['1d', '1wk', '1mo']
    case 'max': return ['1wk', '1mo']
  }
}

