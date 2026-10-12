import type { AssetId, PriceTick, WsClientMessage, WsServerMessage } from '@/api/types'
import type { SocketLike } from '@/realtime/client'
import { classOf } from './catalog'
import { currentPrice, entryFor, genQuote, setLivePrice } from './market'
import { gauss } from './rng'

/** WebSocket falso que habla el protocolo de docs/API.md y emite ticks de un paseo aleatorio. */
export class MockSocket implements SocketLike {
  readyState = 0
  onopen: ((ev: unknown) => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: unknown) => void) | null = null
  onerror: ((ev: unknown) => void) | null = null

  private subs = new Set<AssetId>()
  private timer: ReturnType<typeof setInterval> | null = null
  private prevClose = new Map<AssetId, number>()

  constructor() {
    setTimeout(() => {
      if (this.readyState !== 0) return
      this.readyState = 1
      this.onopen?.({})
      this.timer = setInterval(() => this.emitTicks(), 700)
    }, 150)
  }

  send(data: string): void {
    const msg = JSON.parse(data) as WsClientMessage
    if (msg.type === 'subscribe') msg.ids.forEach((id) => this.subs.add(id))
    else if (msg.type === 'unsubscribe') msg.ids.forEach((id) => this.subs.delete(id))
    else this.reply({ type: 'pong' })
  }

  close(): void {
    this.readyState = 3
    if (this.timer) clearInterval(this.timer)
    this.timer = null
    this.onclose?.({})
  }

  private reply(m: WsServerMessage): void {
    this.onmessage?.({ data: JSON.stringify(m) })
  }

  private emitTicks(): void {
    for (const id of this.subs) {
      if (Math.random() > 0.65) continue
      const e = entryFor(id)
      const cls = classOf(id)
      const last = currentPrice(id)
      // Volatilidad por tick amplificada para que se vea movimiento en una demo.
      const sigma = e.vol * Math.sqrt(0.7 / (cls === 'crypto' ? 365 * 86_400 : 252 * 23_400)) * 7
      const price = Math.max(last * (1 + sigma * gauss(Math.random) + 0.003 * (e.price / last - 1)), 1e-9)
      setLivePrice(id, price)
      let prev = this.prevClose.get(id)
      if (prev === undefined) {
        prev = genQuote(id).prevClose ?? e.price
        this.prevClose.set(id, prev)
      }
      const tick: PriceTick = {
        id,
        price,
        changePct: (price / prev - 1) * 100,
        volume: null,
        ts: new Date().toISOString(),
        source: 'mock',
      }
      this.reply({ type: 'tick', tick })
    }
  }
}
