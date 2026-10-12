import { render, screen } from '@testing-library/react'
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

describe('AnalysisReport', () => {
  it('muestra veredicto, confianza, avisos de datos, modelo y fuentes seguras', () => {
    render(<QueryClientProvider client={createQueryClient()}><AnalysisReport analysis={a} /></QueryClientProvider>)
    expect(screen.getByText('Acumular', { selector: '.verdict-word' })).toBeInTheDocument()
    expect(screen.getByText(/74/)).toBeInTheDocument()
    expect(screen.getByText('Faltan fundamentales')).toBeInTheDocument()
    expect(screen.getByText('qwen2.5:7b')).toBeInTheDocument()
    expect(screen.getByText('sin datos suficientes')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Noticia' })).toHaveAttribute('rel', 'noopener noreferrer')
    expect(screen.queryByRole('link', { name: 'Mala' })).toBeNull()
    expect(screen.getByText(/no es asesoramiento financiero/)).toBeInTheDocument()
  })
})
