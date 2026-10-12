import type { Analysis, AssetId, ChatEvent, ChatMessage, ToolCallRecord } from '@/api/types'
import { CATALOG, classOf, searchCatalog, symbolOfId } from './catalog'
import { genAnalysis, genNews } from './content'
import { getDb, nextId, saveDb } from './db'
import { currentPrice, entryFor, genFundamentals, genIndicators, genQuote } from './market'
import { rngFor, sleep } from './rng'

const fmt = (v: number | null | undefined, d = 2) =>
  v == null ? '—' : v.toLocaleString('es-ES', { minimumFractionDigits: v < 1 ? Math.min(d + 3, 6) : d, maximumFractionDigits: v < 1 ? Math.min(d + 3, 6) : d })
const pct = (v: number | null | undefined) => (v == null ? '—' : `${(v * 100).toLocaleString('es-ES', { maximumFractionDigits: 1, signDisplay: 'exceptZero' })} %`)

/** Activos citados en el mensaje (por símbolo o nombre) y, si no hay, los de la conversación. */
function detectAssets(content: string, focus: AssetId[]): AssetId[] {
  const found: AssetId[] = []
  const lower = content.toLowerCase()
  for (const e of CATALOG) {
    const sym = symbolOfId(e.id).toLowerCase()
    const firstWord = e.name.toLowerCase().split(/[\s,.]+/)[0] ?? ''
    const symHit = sym.length >= 3 && new RegExp(`(^|[^a-z0-9])${sym.replace(/[.^=]/g, '\\$&')}($|[^a-z0-9])`).test(lower)
    const nameHit = firstWord.length >= 4 && lower.includes(firstWord)
    if (symHit || nameHit) found.push(e.id)
  }
  if (found.length === 0) {
    for (const w of lower.split(/[^a-záéíóúñ0-9.]+/)) {
      if (w.length < 3) continue
      const hit = searchCatalog(w, 1)[0]
      if (hit && symbolOfId(hit.id).toLowerCase() === w) found.push(hit.id)
    }
  }
  const uniq = [...new Set(found)].slice(0, 3)
  return uniq.length > 0 ? uniq : focus.slice(0, 2)
}

interface Plan {
  thinking: string
  tools: { name: string; args: Record<string, unknown>; summary: string }[]
  text: string
  analysis: Analysis | null
}

function plan(content: string, assets: AssetId[]): Plan {
  const lower = content.toLowerCase()
  const asks = {
    compare: /compar|frente a|versus| vs /.test(lower) && assets.length >= 2,
    risks: /riesgo/.test(lower),
    entry: /momento|comprar|entrar|invertir|analiz|merece|vale la pena/.test(lower),
  }
  if (assets.length === 0) {
    return {
      thinking: 'La pregunta no menciona ningún activo concreto. Conviene pedir aclaración antes de consultar datos.',
      tools: [],
      text: 'Para ayudarte necesito saber **qué activo** te interesa. Puedes escribir un nombre o símbolo (por ejemplo, *Apple*, *BTC* o *SAN.MC*) o añadirlo como activo en foco con el buscador de arriba.\n\nTambién puedo comparar dos activos o resumir sus riesgos.',
      analysis: null,
    }
  }

  const tools: Plan['tools'] = []
  for (const id of assets) {
    const q = genQuote(id)
    const ind = genIndicators(id)
    tools.push({ name: 'get_quote', args: { id }, summary: `${symbolOfId(id)}: ${fmt(q.price)} ${q.currency ?? ''} (${fmt(q.changePct)} %)` })
    tools.push({ name: 'get_indicators', args: { id }, summary: `RSI ${fmt(ind.rsi14, 1)}, tendencia ${ind.trend}, SMA50 ${fmt(ind.sma50)}` })
  }
  if (asks.entry || asks.risks) tools.push({ name: 'get_news', args: { id: assets[0], limit: 5 }, summary: '5 titulares recientes recuperados' })

  const blocks: string[] = []
  let analysis: Analysis | null = null

  if (asks.compare) {
    const rows = assets.map((id) => {
      const q = genQuote(id)
      const ind = genIndicators(id)
      const f = genFundamentals(id)
      return { id, q, ind, f }
    })
    blocks.push(`## Comparación: ${rows.map((r) => symbolOfId(r.id)).join(' vs ')}\n`)
    blocks.push(`| Métrica | ${rows.map((r) => symbolOfId(r.id)).join(' | ')} |\n|---|${rows.map(() => '---').join('|')}|`)
    const line = (label: string, f: (r: (typeof rows)[number]) => string) => `| ${label} | ${rows.map(f).join(' | ')} |`
    blocks.push(
      [
        line('Precio', (r) => `${fmt(r.q.price)} ${r.q.currency ?? ''}`),
        line('Retorno 3 meses', (r) => pct(r.ind.return3m)),
        line('Retorno 1 año', (r) => pct(r.ind.return1y)),
        line('RSI (14)', (r) => fmt(r.ind.rsi14, 1)),
        line('Volatilidad 30d', (r) => pct(r.ind.volatility30d)),
        line('Máx. caída 1 año', (r) => pct(r.ind.maxDrawdown1y)),
        line('PER', (r) => fmt(r.f?.peTrailing ?? null, 1)),
      ].join('\n'),
    )
    const best = [...rows].sort((a, b) => (b.ind.return3m ?? 0) - (a.ind.return3m ?? 0))[0]
    blocks.push(
      `\n**Lectura:** ${symbolOfId(best?.id ?? assets[0]!)} lidera en momentum a 3 meses, pero eso no implica mejor entrada: revisa también la volatilidad y la caída máxima. Si buscas menor riesgo, prioriza el de menor volatilidad.`,
    )
  } else if (asks.risks) {
    const id = assets[0]!
    const ind = genIndicators(id)
    const n = entryFor(id).name
    blocks.push(`## Riesgos principales de ${n}\n`)
    blocks.push(
      [
        `- **Volatilidad:** ${pct(ind.volatility30d)} anualizada en los últimos 30 días; la caída máxima del último año fue ${pct(ind.maxDrawdown1y)}.`,
        `- **Valoración y expectativas:** ${classOf(id) === 'stock' ? 'si los resultados decepcionan, el múltiplo puede comprimirse con rapidez.' : 'el precio descuenta buena parte de las buenas noticias recientes.'}`,
        '- **Macro:** sensibilidad a tipos de interés e inflación.',
        `- **Técnico:** RSI en ${fmt(ind.rsi14, 1)} y ${ind.trend === 'down' || ind.trend === 'strong_down' ? 'tendencia bajista vigente' : 'tendencia aún intacta, aunque con posibles correcciones'}.`,
      ].join('\n'),
    )
    blocks.push('\nSi quieres, genero un análisis completo con niveles y estrategia de entrada.')
  } else if (asks.entry) {
    const id = assets[0]!
    analysis = genAnalysis(id, 7)
    const ind = genIndicators(id)
    blocks.push(`## ${entryFor(id).name}: ¿buen momento?\n`)
    blocks.push(
      `Con los datos de ahora, mi lectura es **${analysis.verdict === 'buy' ? 'comprar' : analysis.verdict === 'accumulate' ? 'acumular' : analysis.verdict === 'hold' ? 'mantener' : analysis.verdict === 'wait' ? 'esperar' : analysis.verdict === 'reduce' ? 'reducir' : 'evitar'}** con una confianza del ${Math.round(analysis.confidence * 100)} %.\n`,
    )
    blocks.push(
      [
        `- Precio: **${fmt(currentPrice(id))} ${entryFor(id).currency ?? ''}**`,
        `- RSI (14): ${fmt(ind.rsi14, 1)}; MACD ${ind.macdHist != null && ind.macdHist > 0 ? 'por encima' : 'por debajo'} de su señal`,
        `- Retorno a 3 meses: ${pct(ind.return3m)}; distancia al máximo anual: ${pct(ind.distFromHigh52w)}`,
      ].join('\n'),
    )
    blocks.push(`\n${analysis.summary}\n\n> Esto no es asesoramiento financiero: contrasta la decisión con tu perfil de riesgo.`)
  } else {
    const id = assets[0]!
    const q = genQuote(id)
    const ind = genIndicators(id)
    const news = genNews(id, 3)
    blocks.push(`## Resumen de ${entryFor(id).name}\n`)
    blocks.push(`Cotiza a **${fmt(q.price)} ${q.currency ?? ''}** (${fmt(q.changePct)} % en la sesión). Tendencia: *${ind.trend}*, RSI ${fmt(ind.rsi14, 1)}.\n`)
    blocks.push('Titulares recientes:\n')
    blocks.push(news.map((n) => `- ${n.title} (${n.publisher ?? n.provider})`).join('\n'))
  }

  return {
    thinking: `El usuario pregunta por ${assets.map(symbolOfId).join(', ')}. Necesito datos en vivo: consulto cotización e indicadores${asks.entry || asks.risks ? ' y noticias' : ''} y los interpreto sin inventar cifras.`,
    tools,
    text: blocks.join('\n'),
    analysis,
  }
}

function chunks(text: string): string[] {
  const parts = text.match(/\s+|[^\s]+/g) ?? []
  const out: string[] = []
  for (let i = 0; i < parts.length; ) {
    const take = 2 + (i % 3)
    out.push(parts.slice(i, i + take).join(''))
    i += take
  }
  return out
}

/** Chat simulado: persiste el mensaje del usuario, hace streaming y guarda la respuesta (también si se aborta). */
export async function* mockChatStream(
  cid: string,
  body: { content: string; assetIds?: AssetId[] },
  signal?: AbortSignal,
): AsyncGenerator<ChatEvent> {
  const db = getDb()
  const conv = db.conversations.find((c) => c.id === cid)
  if (!conv) throw Object.assign(new Error('Conversación no encontrada'), { status: 404, code: 'not_found' })
  if (body.assetIds) conv.assetIds = body.assetIds
  const list = (db.messages[cid] ??= [])
  const now = () => new Date().toISOString()
  list.push({ id: nextId('m'), conversationId: cid, role: 'user', content: body.content, toolCalls: [], analysis: null, createdAt: now() })
  if (!conv.title) conv.title = body.content.length > 48 ? `${body.content.slice(0, 47)}…` : body.content
  conv.messageCount = list.length
  conv.updatedAt = now()
  saveDb()

  const assets = detectAssets(body.content, conv.assetIds)
  const p = plan(body.content, assets)
  const messageId = nextId('m')
  let text = ''
  const calls: ToolCallRecord[] = []
  let analysis: Analysis | null = null
  let completed = false
  const rand = rngFor(cid, body.content)

  try {
    await sleep(250, signal)
    yield { type: 'start', messageId }

    for (const piece of chunks(p.thinking)) {
      await sleep(15, signal)
      yield { type: 'thinking', text: piece }
    }
    for (const [i, t] of p.tools.entries()) {
      const id = `call_${i}`
      await sleep(180, signal)
      yield { type: 'tool_call', id, name: t.name, arguments: t.args }
      await sleep(260, signal)
      yield { type: 'tool_result', id, name: t.name, ok: true, summary: t.summary }
      calls.push({ id, name: t.name, arguments: t.args, resultSummary: t.summary, ok: true })
    }
    for (const piece of chunks(p.text)) {
      await sleep(14 + Math.floor(rand() * 24), signal)
      text += piece
      yield { type: 'token', text: piece }
    }
    if (p.analysis) {
      await sleep(120, signal)
      analysis = p.analysis
      yield { type: 'analysis', analysis: p.analysis }
    }
    completed = true
    yield { type: 'done', messageId, model: 'qwen2.5:7b', promptTokens: 812, completionTokens: Math.round(text.length / 4) }
  } finally {
    // El servidor guarda la respuesta aunque el cliente se desconecte.
    const msg: ChatMessage = {
      id: messageId,
      conversationId: cid,
      role: 'assistant',
      content: completed ? text : `${text}\n\n*(generación detenida)*`,
      toolCalls: calls,
      analysis,
      createdAt: now(),
    }
    if (text || calls.length > 0) {
      list.push(msg)
      conv.messageCount = list.length
      conv.lastMessagePreview = text.replace(/[#*>|`-]/g, '').trim().slice(0, 100)
      conv.updatedAt = now()
      saveDb()
    }
  }
}
