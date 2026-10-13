import type { Analysis, AnalysisDepth, AssetId, ChatMessage, Conversation, Favorite, SearchEntry } from '@/api/types'
import { toSummary } from './catalog'
import { genAnalysis } from './content'
import { entryFor } from './market'

/** «Base de datos» del mock: vive en memoria y se persiste en localStorage (si se puede). */
export interface MockDb {
  favorites: Favorite[]
  searches: SearchEntry[]
  views: Record<string, { views: number; lastViewedAt: string }>
  conversations: Conversation[]
  messages: Record<string, ChatMessage[]>
  analyses: Record<string, Analysis[]>
  seq: number
}

const KEY = 'invest-ai.mock.db.v1'
const H = 3_600_000

let db: MockDb | null = null

function ago(hours: number): string {
  return new Date(Date.now() - hours * H).toISOString()
}

function seed(): MockDb {
  const fav = (id: AssetId, note: string | null, hours: number): Favorite => ({
    id,
    summary: toSummary(entryFor(id)),
    note,
    addedAt: ago(hours),
  })
  const views: MockDb['views'] = {}
  const v = (id: AssetId, n: number, hours: number) => (views[id] = { views: n, lastViewedAt: ago(hours) })
  v('stock:AAPL', 14, 0.5)
  v('crypto:BTC', 11, 2)
  v('stock:NVDA', 9, 5)
  v('stock:TSLA', 6, 26)
  v('stock:SAN.MC', 5, 30)
  v('crypto:ETH', 4, 50)
  v('stock:ITX.MC', 3, 72)
  v('stock:MSFT', 2, 96)

  const c1: Conversation = {
    id: 'c_seed_1',
    title: '¿Es buen momento para entrar en Apple?',
    assetIds: ['stock:AAPL'],
    pinned: true,
    messageCount: 2,
    lastMessagePreview: 'Apple cotiza cerca de máximos, con un RSI en zona neutral…',
    createdAt: ago(30),
    updatedAt: ago(29.9),
  }
  const c2: Conversation = {
    id: 'c_seed_2',
    title: 'Compara Bitcoin con Ethereum',
    assetIds: ['crypto:BTC', 'crypto:ETH'],
    pinned: false,
    messageCount: 2,
    lastMessagePreview: 'Bitcoin muestra menor volatilidad y mayor capitalización…',
    createdAt: ago(60),
    updatedAt: ago(59.8),
  }
  const msg = (cid: string, n: number, role: 'user' | 'assistant', content: string, hours: number): ChatMessage => ({
    id: `m_${cid}_${n}`,
    conversationId: cid,
    role,
    content,
    toolCalls: [],
    analysis: null,
    createdAt: ago(hours),
  })

  const analyses: MockDb['analyses'] = {}
  const an = (id: AssetId, serial: number, hours: number, depth: AnalysisDepth = 'standard', legacy = false) => {
    const list = (analyses[id] ??= [])
    const a = genAnalysis(id, serial, new Date(Date.now() - hours * H), { depth })
    if (legacy) {
      // Análisis anterior a los campos nuevos: sin escenarios, método ni mediciones.
      for (const k of ['levelsNote', 'scenarios', 'expectedReturnPct', 'wouldChangeIf', 'horizonNote', 'positionSizing', 'method', 'qualityNotes', 'depth', 'durationMs', 'llmCalls', 'promptTokens', 'completionTokens'] as const) delete a[k]
      a.thesis = a.thesis.split('\n\n')[0]!
    }
    list.push(a)
  }
  an('stock:AAPL', 1, 1, 'deep')
  an('stock:AAPL', 0, 49, 'standard')
  an('stock:AAPL', 2, 100, 'standard')
  an('stock:AAPL', 3, 150, 'quick')
  an('stock:AAPL', 4, 400, 'standard', true)
  an('crypto:BTC', 0, 3, 'standard')
  an('stock:NVDA', 0, 6, 'deep')
  an('stock:TSLA', 0, 27, 'quick', true)
  an('stock:SAN.MC', 0, 31, 'standard')

  return {
    favorites: [
      fav('stock:AAPL', 'Posición a largo plazo. Revisar tras resultados.', 200),
      fav('crypto:BTC', null, 150),
      fav('stock:NVDA', 'Vigilar soporte en 120', 100),
      fav('stock:SAN.MC', null, 80),
      fav('index:^GSPC', null, 60),
    ],
    searches: [
      { query: 'apple', selected: 'stock:AAPL', at: ago(0.6) },
      { query: 'bitcoin', selected: 'crypto:BTC', at: ago(2.1) },
      { query: 'nvda', selected: 'stock:NVDA', at: ago(5.2) },
      { query: 'oro', selected: 'commodity:GC=F', at: ago(20) },
      { query: 'santander', selected: 'stock:SAN.MC', at: ago(30) },
      { query: 'vwce', selected: null, at: ago(55) },
    ],
    views,
    conversations: [c1, c2],
    messages: {
      c_seed_1: [
        msg('c_seed_1', 1, 'user', '¿Es buen momento para entrar en Apple?', 30),
        msg(
          'c_seed_1',
          2,
          'assistant',
          'Apple cotiza cerca de máximos, con un **RSI en zona neutral** y tendencia alcista.\n\n- Soporte cercano en la media de 50 sesiones\n- Resultados en unas semanas: posible volatilidad\n\nMi lectura: *acumular de forma escalonada* en lugar de entrar de golpe. No es asesoramiento financiero.',
          29.9,
        ),
      ],
      c_seed_2: [
        msg('c_seed_2', 1, 'user', 'Compara Bitcoin con Ethereum', 60),
        msg(
          'c_seed_2',
          2,
          'assistant',
          '| | Bitcoin | Ethereum |\n|---|---|---|\n| Volatilidad 30d | menor | mayor |\n| Capitalización | mayor | menor |\n\nBitcoin muestra menor volatilidad y mayor capitalización; Ethereum ofrece más beta al mercado cripto.',
          59.8,
        ),
      ],
    },
    analyses,
    seq: 100,
  }
}

export function getDb(): MockDb {
  if (db) return db
  try {
    const raw = localStorage.getItem(KEY)
    if (raw) {
      db = JSON.parse(raw) as MockDb
      return db
    }
  } catch {
    /* almacenamiento no disponible o corrupto: arrancamos con la semilla */
  }
  db = seed()
  return db
}

export function saveDb(): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(db))
  } catch {
    /* sin persistencia */
  }
}

export function nextId(prefix: string): string {
  const d = getDb()
  d.seq += 1
  return `${prefix}_${d.seq.toString(36)}${Date.now().toString(36).slice(-4)}`
}

export function resetDb(): void {
  db = seed()
  saveDb()
}
