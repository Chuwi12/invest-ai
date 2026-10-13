import { render, screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { QueryClientProvider } from '@tanstack/react-query'
import { createQueryClient } from '@/api/queryClient'
import type { Analysis } from '@/api/types'
import { AnalysisReport } from './AnalysisReport'

const a: Analysis = {
  id: '1', asset: 'stock:AAPL', verdict: 'accumulate', confidence: 0.74, horizon: 'medium',
  scores: { overall: 69, technical: 72, momentum: 80, fundamental: null, valuation: 60, sentiment: 50, macroScore: 55, risk: 36 },
  summary: 'Resumen', thesis: 'Tesis larga', bullCase: ['sube'], bearCase: ['baja'], risks: ['riesgo x'],
  catalysts: [{ date: '2026-11-21', description: 'Resultados', impact: 0.4 }],
  keyLevels: { supports: [200], resistances: [240], entryZone: [210, 220], stopLoss: 195, targets: [250] },
  strategy: 'Escalonar', dataWarnings: ['Faltan fundamentales'],
  sources: [{ title: 'Noticia', url: 'https://example.com/n', provider: 'gdelt', publishedAt: '2026-01-01T00:00:00Z' }, { title: 'Mala', url: 'javascript:alert(1)', provider: 'x', publishedAt: null }],
  priceAtAnalysis: 215, model: 'qwen2.5:7b', generatedAt: new Date().toISOString(),
}

const deep: Analysis = {
  ...a,
  thesis: 'Párrafo uno.\n\nIncertidumbre: faltan flujos de órdenes.\n\nLa conclusión cambiaría si:\n- pierde 200\n- resultados flojos',
  levelsNote: 'Stop = soporte − 1×ATR14.',
  scenarios: [
    { kind: 'bearish', probability: 25, description: 'Cae al soporte', targetPrice: 200, targetBasis: 'soporte más cercano', returnPct: -7 },
    { kind: 'bullish', probability: 30, description: 'Rompe resistencia', targetPrice: 240, targetBasis: 'resistencia', returnPct: 11.6 },
    { kind: 'base', probability: 45, description: 'Rango', targetPrice: null, targetBasis: null, returnPct: null },
  ],
  expectedReturnPct: 1.7,
  wouldChangeIf: ['pierde 200', 'resultados flojos'],
  horizonNote: 'Catalizadores en semanas.',
  positionSizing: { maxPortfolioPct: 4.5, riskPerTradePct: 1, stopDistancePct: 7.2, atrPct: 1.8, volatility30d: 0.27, rationale: 'Arriesgando el 1 % con stop a 7,2 %.' },
  qualityNotes: ['Se descartó 1 afirmación sin respaldo.'],
  depth: 'deep', durationMs: 71_000, llmCalls: 9, promptTokens: 18_000, completionTokens: 4_500,
}

const view = (an: Analysis) =>
  render(<QueryClientProvider client={createQueryClient()}><AnalysisReport analysis={an} /></QueryClientProvider>)

describe('AnalysisReport', () => {
  it('muestra veredicto, confianza, avisos de datos, modelo y fuentes seguras', () => {
    view(a)
    expect(screen.getByText('Acumular', { selector: '.verdict-word' })).toBeInTheDocument()
    expect(screen.getByText(/74/)).toBeInTheDocument()
    expect(screen.getByText('Faltan fundamentales')).toBeInTheDocument()
    expect(screen.getByText('qwen2.5:7b')).toBeInTheDocument()
    expect(screen.getByText('sin datos suficientes')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Noticia' })).toHaveAttribute('rel', 'noopener noreferrer')
    expect(screen.queryByRole('link', { name: 'Mala' })).toBeNull()
    expect(screen.getByText(/no es asesoramiento financiero/)).toBeInTheDocument()
  })

  it('campos ausentes (análisis antiguo): no pinta ningún apartado nuevo', () => {
    view(a)
    for (const name of ['Escenarios', 'Qué cambiaría el veredicto', 'Tamaño de posición', 'Calidad del análisis']) {
      expect(screen.queryByRole('region', { name })).toBeNull()
      expect(screen.queryByLabelText(name)).toBeNull()
    }
    expect(screen.queryByText(/Por qué este horizonte/)).toBeNull()
    expect(screen.queryByText(/tokens/)).toBeNull()
    expect(screen.queryByText(/llamadas/)).toBeNull()
  })

  it('campos null explícitos tampoco se pintan', () => {
    view({ ...a, scenarios: null, wouldChangeIf: null, positionSizing: null, qualityNotes: [], levelsNote: null, horizonNote: null, expectedReturnPct: null })
    expect(screen.queryByLabelText('Escenarios')).toBeNull()
    expect(screen.queryByLabelText('Qué cambiaría el veredicto')).toBeNull()
    expect(screen.queryByLabelText('Calidad del análisis')).toBeNull()
  })

  it('escenarios: orden alcista/base/bajista, barra que suma 100 %, base del precio y rentabilidad esperada', () => {
    view(deep)
    const sec = screen.getByLabelText('Escenarios')
    const bar = within(sec).getByRole('img', { name: /Probabilidades/ })
    expect(bar).toHaveAccessibleName('Probabilidades: Alcista 30 %, Base 45 %, Bajista 25 %')
    const heads = within(sec).getAllByRole('listitem').map((li) => li.querySelector('strong')?.textContent)
    expect(heads).toEqual(['Alcista', 'Base', 'Bajista'])
    expect(within(sec).getByText('Base del precio: resistencia.')).toBeInTheDocument()
    expect(within(sec).getByText(/sin precio objetivo/)).toBeInTheDocument()
    expect(within(sec).getByText(/Rentabilidad esperada/)).toBeInTheDocument()
    expect(within(sec).getByText(/\+1,7 %/)).toBeInTheDocument()
  })

  it('«qué cambiaría» no se repite: sale del campo y no de la tesis; la incertidumbre se muestra una vez', () => {
    view(deep)
    expect(screen.getAllByText('pierde 200')).toHaveLength(1)
    expect(screen.queryByText(/La conclusión cambiaría si/)).toBeNull()
    expect(screen.getAllByText(/faltan flujos de órdenes/)).toHaveLength(1)
    expect(screen.queryByText(/^Incertidumbre:/)).toBeNull()
  })

  it('sin wouldChangeIf, el apartado de la tesis se conserva', () => {
    view({ ...deep, wouldChangeIf: null })
    expect(screen.getByText(/La conclusión cambiaría si/)).toBeInTheDocument()
  })

  it('tamaño de posición, notas de nivel/horizonte, calidad y pie de método', () => {
    view(deep)
    expect(screen.getByText('Arriesgando el 1 % con stop a 7,2 %.')).toBeInTheDocument()
    expect(screen.getByText('Stop = soporte − 1×ATR14.')).toBeInTheDocument()
    expect(screen.getByText(/Catalizadores en semanas\./)).toBeInTheDocument()
    expect(within(screen.getByLabelText('Calidad del análisis')).getByText('Se descartó 1 afirmación sin respaldo.')).toBeInTheDocument()
    expect(screen.getByText('· profundidad profundo')).toBeInTheDocument()
    expect(screen.getByText('· duración 1 min 11 s')).toBeInTheDocument()
    expect(screen.getByText('· 9 llamadas al modelo')).toBeInTheDocument()
    expect(screen.getByText(/22\.500 tokens/)).toBeInTheDocument()
  })
})
