import type { AssetId, PriceTick, WsClientMessage, WsServerMessage } from '@/api/types'

/** Subconjunto de `WebSocket` que usamos (permite inyectar un socket falso en tests y en el modo mock). */
export interface SocketLike {
  readyState: number
  send(data: string): void
  close(): void
  onopen: ((ev: unknown) => void) | null
  onmessage: ((ev: { data: unknown }) => void) | null
  onclose: ((ev: unknown) => void) | null
  onerror: ((ev: unknown) => void) | null
}

export type SocketFactory = (url: string) => SocketLike

/** idle: sin suscripciones, sin socket · connecting: primer intento · open · reconnecting: tras una caída. */
export type ConnectionState = 'idle' | 'connecting' | 'open' | 'reconnecting'

export interface LiveTick {
  id: AssetId
  price: number
  changePct: number | null
  volume: number | null
  ts: string
  source: string
  /** Contador por activo: cambia en cada tick (sirve para reiniciar animaciones). */
  seq: number
  /** Dirección respecto al tick anterior. */
  dir: 'up' | 'down' | 'flat'
}

export interface BackoffOptions {
  initialMs: number
  maxMs: number
  factor: number
  /** 0–1: fracción del retardo que se resta al azar para no sincronizar reconexiones. */
  jitter: number
}

export interface RealtimeOptions {
  url: string | (() => string)
  createSocket?: SocketFactory
  pingIntervalMs?: number
  /** Si tras un ping no llega ningún mensaje en este tiempo, se da la conexión por muerta. */
  pongTimeoutMs?: number
  /** Tiempo que una suscripción sin consumidores sigue viva antes de enviar `unsubscribe`. */
  unsubscribeGraceMs?: number
  backoff?: Partial<BackoffOptions>
  random?: () => number
  /**
   * Tiempo que una conexión debe seguir abierta para darla por estable y reiniciar el backoff. Si
   * se reiniciara al abrir, un servidor que acepta y cierra enseguida provocaría reconexiones a
   * ritmo fijo (≈ `initialMs`) para siempre.
   */
  stableAfterMs?: number
}

const OPEN = 1

export function defaultWsUrl(): string {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${location.host}/api/ws`
}

/**
 * Un único WebSocket multiplexado para toda la app (`/api/ws`).
 *  - Refcount de suscripciones por id: se suscribe al pasar de 0 a 1 consumidores y se desuscribe
 *    (con un pequeño margen) al llegar a 0.
 *  - Reconexión con backoff exponencial y reenvío de las suscripciones activas.
 *  - Ping cada 25 s (más vigilancia de pong para detectar conexiones colgadas).
 *  - Store externo por id: cada tick solo notifica a los oyentes de ese activo.
 */
export class RealtimeClient {
  private readonly opts: Required<Omit<RealtimeOptions, 'backoff' | 'createSocket'>> & {
    backoff: BackoffOptions
    createSocket: SocketFactory
  }

  /** id -> consumidores. Una entrada con 0 sigue viva hasta que expire el margen. */
  private readonly refs = new Map<AssetId, number>()
  private readonly lingering = new Map<AssetId, ReturnType<typeof setTimeout>>()
  /** ids que el servidor tiene suscritos en la conexión actual. */
  private serverSubs = new Set<AssetId>()
  private readonly ticks = new Map<AssetId, LiveTick>()
  private readonly idListeners = new Map<AssetId, Set<() => void>>()
  private readonly tickCallbacks = new Map<AssetId, Set<(t: LiveTick) => void>>()
  private readonly statusListeners = new Set<() => void>()

  private socket: SocketLike | null = null
  private state: ConnectionState = 'idle'
  private attempt = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private pingTimer: ReturnType<typeof setInterval> | null = null
  private pongTimer: ReturnType<typeof setTimeout> | null = null
  private stableTimer: ReturnType<typeof setTimeout> | null = null
  private syncScheduled = false
  private disposed = false

  constructor(options: RealtimeOptions) {
    this.opts = {
      url: options.url,
      createSocket: options.createSocket ?? ((url) => new WebSocket(url) as unknown as SocketLike),
      pingIntervalMs: options.pingIntervalMs ?? 25_000,
      pongTimeoutMs: options.pongTimeoutMs ?? 10_000,
      unsubscribeGraceMs: options.unsubscribeGraceMs ?? 1500,
      stableAfterMs: options.stableAfterMs ?? 10_000,
      random: options.random ?? Math.random,
      backoff: { initialMs: 500, maxMs: 30_000, factor: 2, jitter: 0.3, ...options.backoff },
    }
  }

  // --- API pública -----------------------------------------------------------------------

  /** Añade un consumidor para `id`. Devuelve la función que lo retira (idempotente). */
  subscribe(id: AssetId): () => void {
    this.refs.set(id, (this.refs.get(id) ?? 0) + 1)
    const timer = this.lingering.get(id)
    if (timer) {
      clearTimeout(timer)
      this.lingering.delete(id)
    }
    this.scheduleSync()
    this.ensureConnected()
    let released = false
    return () => {
      if (released) return
      released = true
      this.release(id)
    }
  }

  /** Consumidores activos de un id (para tests y depuración). */
  refCount(id: AssetId): number {
    return this.refs.get(id) ?? 0
  }

  /** Ids que el servidor tiene suscritos ahora mismo. */
  get serverSubscriptions(): ReadonlySet<AssetId> {
    return this.serverSubs
  }

  getTick(id: AssetId): LiveTick | undefined {
    return this.ticks.get(id)
  }

  /** Para `useSyncExternalStore`: notifica solo cuando llega un tick de `id`. */
  subscribeStore(id: AssetId, listener: () => void): () => void {
    let set = this.idListeners.get(id)
    if (!set) this.idListeners.set(id, (set = new Set()))
    set.add(listener)
    return () => {
      set.delete(listener)
      if (set.size === 0) this.idListeners.delete(id)
    }
  }

  /** Callback imperativo por tick (gráficos): no pasa por React. */
  onTick(id: AssetId, cb: (t: LiveTick) => void): () => void {
    let set = this.tickCallbacks.get(id)
    if (!set) this.tickCallbacks.set(id, (set = new Set()))
    set.add(cb)
    return () => {
      set.delete(cb)
      if (set.size === 0) this.tickCallbacks.delete(id)
    }
  }

  getStatus(): ConnectionState {
    return this.state
  }

  subscribeStatus(listener: () => void): () => void {
    this.statusListeners.add(listener)
    return () => void this.statusListeners.delete(listener)
  }

  /** Reintenta ya (p. ej. al volver la red o la pestaña) si estamos esperando para reconectar. */
  reconnectNow(): void {
    if (this.disposed || this.state !== 'reconnecting') return
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    // Un intento que sigue en el handshake probablemente no llegue a buen puerto (la red acaba de
    // volver): se sustituye, pero cerrando el socket anterior; si no, queda huérfano y abierto.
    this.discardSocket()
    this.connect()
  }

  /** Suelta el socket actual (sin disparar `handleClose`) y lo cierra. */
  private discardSocket(): void {
    const old = this.socket
    if (!old) return
    this.detach(old)
    this.socket = null
    this.serverSubs = new Set()
    this.stopPing()
    try {
      old.close()
    } catch {
      /* ya cerrado */
    }
  }

  dispose(): void {
    this.disposed = true
    this.clearTimers()
    for (const t of this.lingering.values()) clearTimeout(t)
    this.lingering.clear()
    this.detach(this.socket)
    this.socket?.close()
    this.socket = null
    this.serverSubs.clear()
    this.setState('idle')
  }

  // --- Refcount --------------------------------------------------------------------------

  private release(id: AssetId): void {
    const n = (this.refs.get(id) ?? 0) - 1
    if (n > 0) {
      this.refs.set(id, n)
      return
    }
    this.refs.set(id, 0)
    // Margen: navegar entre páginas desmonta y vuelve a montar consumidores casi a la vez.
    const timer = setTimeout(() => {
      this.lingering.delete(id)
      if ((this.refs.get(id) ?? 0) === 0) {
        this.refs.delete(id)
        this.ticks.delete(id)
        this.scheduleSync()
      }
    }, this.opts.unsubscribeGraceMs)
    this.lingering.set(id, timer)
  }

  /** Agrupa varios cambios del mismo render en un solo mensaje. */
  private scheduleSync(): void {
    if (this.syncScheduled) return
    this.syncScheduled = true
    queueMicrotask(() => {
      this.syncScheduled = false
      this.syncSubscriptions()
    })
  }

  private syncSubscriptions(): void {
    const s = this.socket
    if (!s || s.readyState !== OPEN) return
    const desired = new Set(this.refs.keys())
    const add = [...desired].filter((id) => !this.serverSubs.has(id))
    const remove = [...this.serverSubs].filter((id) => !desired.has(id))
    if (add.length > 0) this.send({ type: 'subscribe', ids: add })
    if (remove.length > 0) this.send({ type: 'unsubscribe', ids: remove })
    this.serverSubs = desired
  }

  // --- Conexión --------------------------------------------------------------------------

  private ensureConnected(): void {
    if (this.disposed || this.socket || this.reconnectTimer) return
    if (this.refs.size === 0) return
    this.connect()
  }

  private connect(): void {
    if (this.disposed) return
    this.reconnectTimer = null
    this.setState(this.attempt === 0 ? 'connecting' : 'reconnecting')
    let socket: SocketLike
    try {
      const url = typeof this.opts.url === 'function' ? this.opts.url() : this.opts.url
      socket = this.opts.createSocket(url)
    } catch {
      this.scheduleReconnect()
      return
    }
    this.socket = socket
    socket.onopen = () => {
      if (this.socket !== socket) return
      this.serverSubs = new Set()
      this.setState('open')
      this.syncSubscriptions() // reenvía todas las suscripciones activas
      this.startPing()
      // El backoff solo se reinicia si la conexión aguanta; ver `stableAfterMs`.
      this.stopStable()
      this.stableTimer = setTimeout(() => {
        this.stableTimer = null
        this.attempt = 0
      }, this.opts.stableAfterMs)
    }
    socket.onmessage = (ev) => {
      if (this.socket !== socket) return
      this.clearPong()
      this.handleMessage(ev.data)
    }
    socket.onerror = () => {
      /* el cierre llega por onclose */
    }
    socket.onclose = () => {
      if (this.socket !== socket) return
      this.handleClose()
    }
  }

  private handleClose(): void {
    this.detach(this.socket)
    this.socket = null
    this.serverSubs = new Set()
    this.stopPing()
    this.stopStable()
    if (this.disposed) return
    if (this.refs.size === 0) {
      this.attempt = 0
      this.setState('idle')
      return
    }
    this.scheduleReconnect()
  }

  private scheduleReconnect(): void {
    if (this.disposed) return
    const { initialMs, maxMs, factor, jitter } = this.opts.backoff
    const base = Math.min(maxMs, initialMs * factor ** this.attempt)
    const delay = Math.round(base * (1 - jitter * this.opts.random()))
    this.attempt += 1
    this.setState('reconnecting')
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (this.refs.size === 0) {
        this.attempt = 0
        this.setState('idle')
        return
      }
      this.connect()
    }, delay)
  }

  private detach(s: SocketLike | null): void {
    if (!s) return
    s.onopen = s.onmessage = s.onclose = s.onerror = null
  }

  // --- Ping ------------------------------------------------------------------------------

  private startPing(): void {
    this.stopPing()
    this.pingTimer = setInterval(() => {
      const s = this.socket
      if (!s || s.readyState !== OPEN) return
      this.send({ type: 'ping' })
      this.clearPong()
      this.pongTimer = setTimeout(() => {
        // Sin respuesta: la conexión está colgada. La cerramos nosotros y reconectamos.
        const dead = this.socket
        this.detach(dead)
        try {
          dead?.close()
        } catch {
          /* ya cerrado */
        }
        this.handleClose()
      }, this.opts.pongTimeoutMs)
    }, this.opts.pingIntervalMs)
  }

  private stopPing(): void {
    if (this.pingTimer) clearInterval(this.pingTimer)
    this.pingTimer = null
    this.clearPong()
  }

  private stopStable(): void {
    if (this.stableTimer) clearTimeout(this.stableTimer)
    this.stableTimer = null
  }

  private clearPong(): void {
    if (this.pongTimer) clearTimeout(this.pongTimer)
    this.pongTimer = null
  }

  private clearTimers(): void {
    this.stopPing()
    this.stopStable()
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
  }

  // --- Mensajes --------------------------------------------------------------------------

  private send(msg: WsClientMessage): void {
    const s = this.socket
    if (!s || s.readyState !== OPEN) return
    s.send(JSON.stringify(msg))
  }

  private handleMessage(raw: unknown): void {
    if (typeof raw !== 'string') return
    let msg: WsServerMessage
    try {
      msg = JSON.parse(raw) as WsServerMessage
    } catch {
      return
    }
    if (msg.type === 'tick') this.applyTick(msg.tick)
    else if (msg.type === 'error') console.warn('[ws] error del servidor:', msg.message)
  }

  private applyTick(t: PriceTick): void {
    if (!this.refs.has(t.id)) return // tick de un activo ya liberado
    const prev = this.ticks.get(t.id)
    const dir: LiveTick['dir'] = !prev || t.price === prev.price ? 'flat' : t.price > prev.price ? 'up' : 'down'
    const next: LiveTick = {
      id: t.id,
      price: t.price,
      changePct: t.changePct,
      volume: t.volume,
      ts: t.ts,
      source: t.source,
      seq: (prev?.seq ?? 0) + 1,
      dir,
    }
    this.ticks.set(t.id, next)
    this.idListeners.get(t.id)?.forEach((l) => l())
    this.tickCallbacks.get(t.id)?.forEach((cb) => cb(next))
  }

  private setState(next: ConnectionState): void {
    if (this.state === next) return
    this.state = next
    this.statusListeners.forEach((l) => l())
  }
}
