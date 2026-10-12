import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AssetId, PriceTick } from '@/api/types'
import { RealtimeClient, type SocketLike } from './client'

class FakeSocket implements SocketLike {
  static all: FakeSocket[] = []
  readyState = 0
  sent: { type: string; ids?: string[] }[] = []
  onopen: ((ev: unknown) => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: unknown) => void) | null = null
  onerror: ((ev: unknown) => void) | null = null
  closed = false

  constructor(public url: string) {
    FakeSocket.all.push(this)
  }
  send(data: string) {
    this.sent.push(JSON.parse(data) as { type: string; ids?: string[] })
  }
  close() {
    this.closed = true
    this.readyState = 3
  }
  // --- helpers de test
  open() {
    this.readyState = 1
    this.onopen?.({})
  }
  drop() {
    this.readyState = 3
    this.onclose?.({})
  }
  push(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }
  tick(id: AssetId, price: number) {
    const tick: PriceTick = { id, price, changePct: 0.5, volume: null, ts: '2026-10-11T10:00:00Z', source: 'test' }
    this.push({ type: 'tick', tick })
  }
  subs() {
    return this.sent.filter((m) => m.type === 'subscribe').map((m) => m.ids)
  }
  unsubs() {
    return this.sent.filter((m) => m.type === 'unsubscribe').map((m) => m.ids)
  }
}

const BTC: AssetId = 'crypto:BTC'
const AAPL: AssetId = 'stock:AAPL'

function make(extra: ConstructorParameters<typeof RealtimeClient>[0] extends infer O ? Partial<O> : never = {}) {
  return new RealtimeClient({
    url: 'ws://test/api/ws',
    createSocket: (url) => new FakeSocket(url),
    random: () => 0, // sin jitter: retardos deterministas
    ...extra,
  })
}

/** Deja correr los microtasks (el envío de suscripciones se agrupa con queueMicrotask). */
const flush = () => vi.advanceTimersByTimeAsync(0)
const last = () => FakeSocket.all[FakeSocket.all.length - 1]!

beforeEach(() => {
  vi.useFakeTimers()
  FakeSocket.all = []
})
afterEach(() => {
  vi.useRealTimers()
})

describe('refcount de suscripciones', () => {
  it('no abre el socket hasta la primera suscripción', () => {
    const c = make()
    expect(FakeSocket.all).toHaveLength(0)
    expect(c.getStatus()).toBe('idle')
    c.subscribe(BTC)
    expect(FakeSocket.all).toHaveLength(1)
    expect(c.getStatus()).toBe('connecting')
  })

  it('agrupa varias suscripciones en un solo mensaje y solo suscribe una vez por id', async () => {
    const c = make()
    c.subscribe(BTC)
    c.subscribe(BTC)
    c.subscribe(AAPL)
    last().open()
    await flush()
    expect(last().subs()).toEqual([[BTC, AAPL]])
    expect(c.refCount(BTC)).toBe(2)
  })

  it('solo desuscribe del servidor cuando cae el último consumidor, tras el margen', async () => {
    const c = make({ unsubscribeGraceMs: 1000 })
    const a = c.subscribe(BTC)
    const b = c.subscribe(BTC)
    last().open()
    await flush()

    a()
    a() // idempotente
    await vi.advanceTimersByTimeAsync(5000)
    expect(last().unsubs()).toEqual([])
    expect(c.refCount(BTC)).toBe(1)

    b()
    await vi.advanceTimersByTimeAsync(999)
    expect(last().unsubs()).toEqual([])
    await vi.advanceTimersByTimeAsync(2)
    expect(last().unsubs()).toEqual([[BTC]])
    expect(c.serverSubscriptions.has(BTC)).toBe(false)
  })

  it('volver a suscribirse dentro del margen no genera tráfico', async () => {
    const c = make({ unsubscribeGraceMs: 1000 })
    const off = c.subscribe(BTC)
    last().open()
    await flush()
    off()
    await vi.advanceTimersByTimeAsync(500)
    c.subscribe(BTC)
    await vi.advanceTimersByTimeAsync(5000)
    expect(last().subs()).toEqual([[BTC]])
    expect(last().unsubs()).toEqual([])
  })

  it('las suscripciones previas a la apertura se envían al abrir', async () => {
    const c = make()
    c.subscribe(BTC)
    await flush()
    expect(last().sent).toEqual([])
    last().open()
    expect(last().subs()).toEqual([[BTC]])
  })
})

describe('ticks', () => {
  it('notifica solo a los oyentes del activo del tick y calcula la dirección', async () => {
    const c = make()
    c.subscribe(BTC)
    c.subscribe(AAPL)
    last().open()
    await flush()
    const onBtc = vi.fn()
    const onAapl = vi.fn()
    c.subscribeStore(BTC, onBtc)
    c.subscribeStore(AAPL, onAapl)

    last().tick(BTC, 100)
    last().tick(BTC, 101)
    last().tick(BTC, 100.5)
    expect(onBtc).toHaveBeenCalledTimes(3)
    expect(onAapl).not.toHaveBeenCalled()
    const t = c.getTick(BTC)!
    expect(t).toMatchObject({ price: 100.5, dir: 'down', seq: 3 })
    expect(c.getTick(AAPL)).toBeUndefined()
  })

  it('el snapshot es estable entre ticks y cambia con cada tick', async () => {
    const c = make()
    c.subscribe(BTC)
    last().open()
    await flush()
    last().tick(BTC, 100)
    const a = c.getTick(BTC)
    expect(c.getTick(BTC)).toBe(a)
    last().tick(BTC, 101)
    expect(c.getTick(BTC)).not.toBe(a)
  })

  it('ignora ticks de activos sin suscripción y mensajes basura', async () => {
    const c = make()
    c.subscribe(BTC)
    last().open()
    await flush()
    last().tick(AAPL, 10)
    last().onmessage?.({ data: 'no es json' })
    last().push({ type: 'pong' })
    expect(c.getTick(AAPL)).toBeUndefined()
  })

  it('onTick entrega cada tick sin pasar por el store', async () => {
    const c = make()
    c.subscribe(BTC)
    last().open()
    await flush()
    const cb = vi.fn()
    const off = c.onTick(BTC, cb)
    last().tick(BTC, 1)
    off()
    last().tick(BTC, 2)
    expect(cb).toHaveBeenCalledTimes(1)
  })
})

describe('reconexión', () => {
  it('reconecta con backoff exponencial y reenvía las suscripciones activas', async () => {
    const c = make({ backoff: { initialMs: 1000, maxMs: 8000, factor: 2, jitter: 0 } })
    c.subscribe(BTC)
    c.subscribe(AAPL)
    last().open()
    await flush()
    expect(c.getStatus()).toBe('open')

    // 1ª caída: espera 1 s
    last().drop()
    expect(c.getStatus()).toBe('reconnecting')
    expect(FakeSocket.all).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(999)
    expect(FakeSocket.all).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(FakeSocket.all).toHaveLength(2)

    // el reintento falla sin llegar a abrir: espera 2 s, luego 4 s, luego 8 s (tope)
    last().drop()
    await vi.advanceTimersByTimeAsync(1999)
    expect(FakeSocket.all).toHaveLength(2)
    await vi.advanceTimersByTimeAsync(1)
    expect(FakeSocket.all).toHaveLength(3)
    last().drop()
    await vi.advanceTimersByTimeAsync(4000)
    expect(FakeSocket.all).toHaveLength(4)
    last().drop()
    await vi.advanceTimersByTimeAsync(8000)
    expect(FakeSocket.all).toHaveLength(5)
    last().drop()
    await vi.advanceTimersByTimeAsync(8000) // tope
    expect(FakeSocket.all).toHaveLength(6)

    // al abrir, se reenvían las suscripciones y el backoff se reinicia
    last().open()
    expect(c.getStatus()).toBe('open')
    expect(last().subs()).toEqual([[BTC, AAPL]])
    last().drop()
    await vi.advanceTimersByTimeAsync(1000)
    expect(FakeSocket.all).toHaveLength(7)
  })

  it('tras reconectar solo reenvía lo que sigue suscrito', async () => {
    const c = make({ unsubscribeGraceMs: 100, backoff: { initialMs: 1000, jitter: 0 } })
    const offBtc = c.subscribe(BTC)
    c.subscribe(AAPL)
    last().open()
    await flush()
    offBtc()
    await vi.advanceTimersByTimeAsync(200)
    last().drop()
    await vi.advanceTimersByTimeAsync(1000)
    last().open()
    expect(last().subs()).toEqual([[AAPL]])
  })

  it('no reconecta si ya no queda ninguna suscripción', async () => {
    const c = make({ unsubscribeGraceMs: 100, backoff: { initialMs: 500, jitter: 0 } })
    const off = c.subscribe(BTC)
    last().open()
    await flush()
    off()
    await vi.advanceTimersByTimeAsync(200)
    last().drop()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(FakeSocket.all).toHaveLength(1)
    expect(c.getStatus()).toBe('idle')
    // y una nueva suscripción vuelve a abrir
    c.subscribe(AAPL)
    expect(FakeSocket.all).toHaveLength(2)
  })

  it('reconnectNow salta la espera del backoff', async () => {
    const c = make({ backoff: { initialMs: 30_000, jitter: 0 } })
    c.subscribe(BTC)
    last().open()
    last().drop()
    expect(FakeSocket.all).toHaveLength(1)
    c.reconnectNow()
    expect(FakeSocket.all).toHaveLength(2)
  })

  it('si createSocket lanza, reintenta con backoff', async () => {
    let n = 0
    const c = new RealtimeClient({
      url: 'ws://x',
      random: () => 0,
      backoff: { initialMs: 100, jitter: 0 },
      createSocket: (url) => {
        if (n++ === 0) throw new Error('boom')
        return new FakeSocket(url)
      },
    })
    c.subscribe(BTC)
    expect(FakeSocket.all).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(100)
    expect(FakeSocket.all).toHaveLength(1)
  })

  it('notifica los cambios de estado de conexión', async () => {
    const c = make()
    const l = vi.fn()
    c.subscribeStatus(l)
    c.subscribe(BTC)
    last().open()
    last().drop()
    expect(l).toHaveBeenCalledTimes(3) // connecting, open, reconnecting
  })
})

describe('ping', () => {
  it('envía ping cada 25 s mientras está abierto', async () => {
    const c = make()
    c.subscribe(BTC)
    last().open()
    await flush()
    const pings = () => last().sent.filter((m) => m.type === 'ping').length
    expect(pings()).toBe(0)
    await vi.advanceTimersByTimeAsync(24_999)
    expect(pings()).toBe(0)
    await vi.advanceTimersByTimeAsync(1)
    expect(pings()).toBe(1)
    last().push({ type: 'pong' })
    await vi.advanceTimersByTimeAsync(25_000)
    expect(pings()).toBe(2)
  })

  it('deja de hacer ping al caer la conexión', async () => {
    const c = make({ backoff: { initialMs: 1_000_000, jitter: 0 } })
    c.subscribe(BTC)
    const s = last()
    s.open()
    s.drop()
    await vi.advanceTimersByTimeAsync(100_000)
    expect(s.sent.filter((m) => m.type === 'ping')).toHaveLength(0)
  })

  it('si no llega pong, cierra la conexión colgada y reconecta', async () => {
    const c = make({ backoff: { initialMs: 500, jitter: 0 }, pongTimeoutMs: 10_000 })
    c.subscribe(BTC)
    const s = last()
    s.open()
    await flush()
    await vi.advanceTimersByTimeAsync(25_000) // ping
    await vi.advanceTimersByTimeAsync(10_000) // sin pong
    expect(s.closed).toBe(true)
    expect(c.getStatus()).toBe('reconnecting')
    await vi.advanceTimersByTimeAsync(500)
    expect(FakeSocket.all).toHaveLength(2)
  })

  it('cualquier mensaje cancela la vigilancia del pong', async () => {
    const c = make({ pongTimeoutMs: 10_000 })
    c.subscribe(BTC)
    const s = last()
    s.open()
    await flush()
    await vi.advanceTimersByTimeAsync(25_000)
    s.push({ type: 'pong' })
    await vi.advanceTimersByTimeAsync(10_000)
    expect(s.closed).toBe(false)
    expect(c.getStatus()).toBe('open')
  })
})

describe('dispose', () => {
  it('cierra el socket y no reconecta', async () => {
    const c = make()
    c.subscribe(BTC)
    const s = last()
    s.open()
    c.dispose()
    expect(s.closed).toBe(true)
    await vi.advanceTimersByTimeAsync(60_000)
    expect(FakeSocket.all).toHaveLength(1)
  })
})
