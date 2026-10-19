import type { AssetId, AssetSocial, SocialExcerpt, SocialStatus } from '@/api/types'

const DISCLAIMER = 'Opinión de usuarios anónimos de Reddit y X: no verificada y posiblemente manipulada. Mide atención y tono, no hechos ni cifras.'

/** Estado simulado: redes activadas con X cerca de su tope, para ver la UI completa. */
export function mockSocialStatus(): SocialStatus {
  return {
    enabled: true,
    reddit: { enabled: true },
    x: {
      enabled: true,
      month: new Date().toISOString().slice(0, 7),
      monthlyReadBudget: 500,
      used: 130,
      remaining: 370,
      costPerReadUsd: 0.005,
      estimatedCostUsd: 0.65,
      maxMonthlyCostUsd: 2.5,
      exhausted: false,
      accounts: 3,
      searchAll: false,
    },
    days: 7,
    refreshMinutes: 360,
    lastError: null,
    notes: [],
  }
}

/** Datos inventados. El activo `*:NOSOCIAL` simula «sin publicaciones» y `*:SOCIALOFF` «apagado». */
export function mockAssetSocial(id: AssetId): AssetSocial {
  if (id.endsWith(':SOCIALOFF')) return { enabled: false, digest: null, disclaimer: DISCLAIMER }
  if (id.endsWith(':NOSOCIAL')) return { enabled: true, digest: null, disclaimer: DISCLAIMER }
  const now = Date.now()
  const ago = (h: number) => new Date(now - h * 3_600_000).toISOString()
  const symbol = id.split(':')[1] ?? ''
  const excerpts: SocialExcerpt[] = [
    { id: 'reddit:m1', source: 'reddit', community: 'stocks', author: '3fa9c0d1e2b47a65', title: `${symbol} parece fuerte antes de resultados`, text: 'Creo que sube tras el informe. Es solo una opinión, no un hecho.', score: 420, comments: 87, createdAt: ago(3), url: 'https://www.reddit.com/r/stocks/comments/m1/ejemplo/', stance: 'bullish' },
    { id: 'reddit:m2', source: 'reddit', community: 'investing', author: '91bb02aa77c3d4e8', title: `Vendo ${symbol}: valoración exigente`, text: 'Los márgenes están en riesgo y el múltiplo es alto.', score: 155, comments: 42, createdAt: ago(9), url: 'https://www.reddit.com/r/investing/comments/m2/ejemplo/', stance: 'bearish' },
    { id: 'x:m3', source: 'x', community: 'x', author: 'c4d1e8f09a2b3c71', title: null, text: `$${symbol} esperando resultados el jueves.`, score: 12, comments: 1, createdAt: ago(14), url: 'https://x.com/i/web/status/1800000000000000003', stance: 'neutral' },
  ]
  return {
    enabled: true,
    disclaimer: DISCLAIMER,
    digest: {
      asset: id,
      excerpts,
      metrics: {
        asOf: new Date(now).toISOString(),
        windowDays: 7,
        mentions: 18,
        authors: 11,
        bullish: 9,
        bearish: 4,
        neutral: 5,
        bullishShare: 0.69,
        mentionsWindow: 64,
        bullishWindow: 26,
        bearishWindow: 19,
        baselineMean: 7.4,
        baselineDays: 12,
        ratio: 2.43,
        z: 2.9,
        bySource: [{ source: 'reddit', mentions: 15 }, { source: 'x', mentions: 3 }],
        truncated: false,
      },
    },
  }
}
