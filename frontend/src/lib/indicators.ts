import type { Candle, Indicators } from '@/api/types'
import { formatNumber, formatPercent, formatPrice, type Tone } from './format'

export interface LinePoint {
  time: number
  value: number
}

/** Media móvil simple; los primeros `period - 1` puntos no existen. */
export function smaSeries(candles: readonly Candle[], period: number): LinePoint[] {
  const out: LinePoint[] = []
  if (period <= 0 || candles.length < period) return out
  let sum = 0
  for (let i = 0; i < candles.length; i++) {
    sum += candles[i]!.c
    if (i >= period) sum -= candles[i - period]!.c
    if (i >= period - 1) out.push({ time: candles[i]!.t, value: sum / period })
  }
  return out
}

export interface BollingerPoint {
  time: number
  upper: number
  middle: number
  lower: number
}

/** Bandas de Bollinger (SMA ± `mult` desviaciones típicas poblacionales). */
export function bollingerSeries(candles: readonly Candle[], period = 20, mult = 2): BollingerPoint[] {
  const out: BollingerPoint[] = []
  if (candles.length < period) return out
  for (let i = period - 1; i < candles.length; i++) {
    let sum = 0
    for (let j = i - period + 1; j <= i; j++) sum += candles[j]!.c
    const mean = sum / period
    let sq = 0
    for (let j = i - period + 1; j <= i; j++) sq += (candles[j]!.c - mean) ** 2
    const sd = Math.sqrt(sq / period)
    out.push({ time: candles[i]!.t, upper: mean + mult * sd, middle: mean, lower: mean - mult * sd })
  }
  return out
}

/**
 * Actualiza la última vela con un tick (o abre una nueva en intradía). Devuelve la vela a pintar.
 * `intervalSecs` es la duración de la vela del gráfico.
 */
export function applyTickToCandle(
  last: Candle | undefined,
  price: number,
  tsSecs: number,
  intervalSecs: number,
  volume?: number | null,
): Candle {
  if (!last) return { t: tsSecs - (tsSecs % intervalSecs), o: price, h: price, l: price, c: price, v: volume ?? null }
  // Velas mensuales (duración variable): el tick siempre actualiza la última. Diarias y semanales
  // tienen duración fija: un tick del día/semana siguiente abre vela (si no, con una pestaña abierta
  // al cruzar las 00:00 UTC en cripto, los ticks de hoy reescribirían el máximo/mínimo/cierre de ayer).
  const sameBucket = intervalSecs > 604_800 || tsSecs < last.t + intervalSecs
  if (sameBucket) {
    return { ...last, h: Math.max(last.h, price), l: Math.min(last.l, price), c: price }
  }
  const t = tsSecs - ((tsSecs - last.t) % intervalSecs)
  return { t, o: price, h: price, l: price, c: price, v: null }
}

// --- Interpretación para la pestaña de técnicos -----------------------------------------------

export interface Reading {
  key: string
  label: string
  value: string
  /** Frase corta con la lectura del indicador. */
  text: string
  /** up = lectura alcista, down = bajista, flat = neutra. */
  tone: Tone
}

export function interpretIndicators(i: Indicators): Reading[] {
  const out: Reading[] = []
  const close = i.lastClose

  if (i.rsi14 !== null) {
    const v = i.rsi14
    let text = 'Zona neutral: ni sobrecompra ni sobreventa.'
    let tone: Tone = 'flat'
    if (v >= 70) {
      text = 'Sobrecompra: el precio ha subido con fuerza y podría corregir.'
      tone = 'down'
    } else if (v <= 30) {
      text = 'Sobreventa: el precio ha caído con fuerza y podría rebotar.'
      tone = 'up'
    } else if (v >= 55) {
      text = 'Momentum comprador moderado.'
      tone = 'up'
    } else if (v <= 45) {
      text = 'Momentum vendedor moderado.'
      tone = 'down'
    }
    out.push({ key: 'rsi', label: 'RSI (14)', value: formatNumber(v, 1), text, tone })
  }

  if (i.macd !== null && i.macdSignal !== null) {
    const hist = i.macdHist ?? i.macd - i.macdSignal
    const bull = hist > 0
    const near = Math.abs(hist) < Math.abs(i.macd) * 0.1
    const cross = near ? (bull ? ' Cruce alcista reciente o inminente.' : ' Cruce bajista reciente o inminente.') : ''
    out.push({
      key: 'macd',
      label: 'MACD (12, 26, 9)',
      value: formatNumber(i.macd, 3),
      text: `${bull ? 'Por encima' : 'Por debajo'} de su línea de señal: impulso ${bull ? 'alcista' : 'bajista'}.${cross}`,
      tone: bull ? 'up' : 'down',
    })
  }

  if (close !== null && i.sma50 !== null) {
    const above = close > i.sma50
    out.push({
      key: 'sma50',
      label: 'Media móvil 50',
      value: formatPrice(i.sma50),
      text: `El precio cotiza ${above ? 'por encima' : 'por debajo'} de la media de 50 sesiones (${formatPercent(close / i.sma50 - 1, 1, true)}).`,
      tone: above ? 'up' : 'down',
    })
  }

  if (close !== null && i.sma200 !== null) {
    const above = close > i.sma200
    out.push({
      key: 'sma200',
      label: 'Media móvil 200',
      value: formatPrice(i.sma200),
      text: `Tendencia de fondo ${above ? 'alcista' : 'bajista'}: precio ${above ? 'sobre' : 'bajo'} la media de 200 sesiones.`,
      tone: above ? 'up' : 'down',
    })
  }

  if (i.sma50 !== null && i.sma200 !== null) {
    const golden = i.sma50 > i.sma200
    out.push({
      key: 'cross',
      label: 'Cruce 50/200',
      value: golden ? 'Dorado' : 'De la muerte',
      text: golden
        ? 'La media de 50 está sobre la de 200: estructura de tendencia alcista.'
        : 'La media de 50 está bajo la de 200: estructura de tendencia bajista.',
      tone: golden ? 'up' : 'down',
    })
  }

  if (close !== null && i.bbUpper !== null && i.bbLower !== null && i.bbMiddle !== null) {
    let text: string
    let tone: Tone = 'flat'
    if (close > i.bbUpper) {
      text = 'Por encima de la banda superior: el movimiento está muy extendido.'
      tone = 'down'
    } else if (close < i.bbLower) {
      text = 'Por debajo de la banda inferior: posible sobreventa a corto plazo.'
      tone = 'up'
    } else {
      const pos = (close - i.bbLower) / (i.bbUpper - i.bbLower || 1)
      text = `Dentro de las bandas, al ${formatPercent(pos, 0)} del recorrido entre la inferior y la superior.`
    }
    out.push({
      key: 'bb',
      label: 'Bandas de Bollinger',
      value: `${formatPrice(i.bbLower)} – ${formatPrice(i.bbUpper)}`,
      text,
      tone,
    })
  }

  if (i.atr14 !== null) {
    out.push({
      key: 'atr',
      label: 'ATR (14)',
      value: formatPrice(i.atr14),
      text:
        close !== null && close > 0
          ? `Rango medio diario de ${formatPercent(i.atr14 / close, 2)} del precio: útil para dimensionar el stop.`
          : 'Rango medio diario de la acción del precio.',
      tone: 'flat',
    })
  }

  if (i.relativeVolume !== null) {
    const r = i.relativeVolume
    out.push({
      key: 'rvol',
      label: 'Volumen relativo',
      value: `${formatNumber(r, 2)}×`,
      text:
        r >= 1.5
          ? 'Volumen claramente superior al habitual: el movimiento tiene participación.'
          : r <= 0.6
            ? 'Volumen bajo: los movimientos son menos fiables.'
            : 'Volumen en línea con su media de 20 sesiones.',
      tone: 'flat',
    })
  }

  if (i.distFromHigh52w !== null) {
    const d = Math.abs(i.distFromHigh52w)
    out.push({
      key: 'high52',
      label: 'Distancia al máximo de 52 semanas',
      value: formatPercent(-d, 1),
      text: d < 0.03 ? 'Cerca de máximos anuales.' : d > 0.3 ? 'Muy por debajo de sus máximos anuales.' : 'Por debajo de sus máximos anuales.',
      tone: d < 0.03 ? 'up' : d > 0.3 ? 'down' : 'flat',
    })
  }

  return out
}
