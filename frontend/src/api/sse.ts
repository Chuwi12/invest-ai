import { API_BASE, ApiError, isAbortError, toApiError } from './client'
import type { ChatEvent, PullEvent } from './types'

export interface SseMessage {
  event: string
  data: string
  id?: string
}

/**
 * Parser incremental de Server-Sent Events (https://html.spec.whatwg.org/multipage/server-sent-events.html).
 * No depende de red: se alimenta con texto ya decodificado y devuelve los eventos completos.
 */
export class SseParser {
  private buffer = ''
  private data: string[] = []
  private eventType = ''
  private lastId: string | undefined
  private first = true

  feed(chunk: string): SseMessage[] {
    if (this.first && chunk.length > 0) {
      this.first = false
      if (chunk.charCodeAt(0) === 0xfeff) chunk = chunk.slice(1) // BOM
    }
    this.buffer += chunk
    const out: SseMessage[] = []
    let start = 0
    const re = /\r\n|\n|\r/g
    let m: RegExpExecArray | null
    while ((m = re.exec(this.buffer)) !== null) {
      // Un `\r` al final podría ser la primera mitad de `\r\n`: esperamos al siguiente trozo.
      if (m[0] === '\r' && m.index === this.buffer.length - 1) break
      const msg = this.line(this.buffer.slice(start, m.index))
      if (msg) out.push(msg)
      start = m.index + m[0].length
    }
    this.buffer = this.buffer.slice(start)
    return out
  }

  /** Fin del stream: procesa lo pendiente. Se emite el último evento aunque falte la línea en blanco. */
  flush(): SseMessage[] {
    const out: SseMessage[] = []
    if (this.buffer) {
      const msg = this.line(this.buffer.replace(/\r$/, ''))
      if (msg) out.push(msg)
      this.buffer = ''
    }
    const last = this.dispatch()
    if (last) out.push(last)
    return out
  }

  private line(line: string): SseMessage | null {
    if (line === '') return this.dispatch()
    if (line.startsWith(':')) return null // comentario / keep-alive
    const idx = line.indexOf(':')
    const field = idx === -1 ? line : line.slice(0, idx)
    let value = idx === -1 ? '' : line.slice(idx + 1)
    if (value.startsWith(' ')) value = value.slice(1)
    switch (field) {
      case 'data':
        this.data.push(value)
        break
      case 'event':
        this.eventType = value
        break
      case 'id':
        if (!value.includes('\0')) this.lastId = value
        break
      default:
        break // retry y campos desconocidos: no nos hacen falta
    }
    return null
  }

  private dispatch(): SseMessage | null {
    if (this.data.length === 0) {
      this.eventType = ''
      return null
    }
    const msg: SseMessage = { event: this.eventType || 'message', data: this.data.join('\n') }
    if (this.lastId !== undefined) msg.id = this.lastId
    this.data = []
    this.eventType = ''
    return msg
  }
}

/** Itera los eventos SSE de un `ReadableStream` de bytes. Cancela el lector al terminar o abortar. */
export async function* parseSseStream(
  stream: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
): AsyncGenerator<SseMessage> {
  const reader = stream.getReader()
  const decoder = new TextDecoder('utf-8')
  const parser = new SseParser()
  const onAbort = () => void reader.cancel().catch(() => undefined)
  signal?.addEventListener('abort', onAbort, { once: true })
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      yield* parser.feed(decoder.decode(value, { stream: true }))
    }
    yield* parser.feed(decoder.decode())
    yield* parser.flush()
  } finally {
    signal?.removeEventListener('abort', onAbort)
    await reader.cancel().catch(() => undefined)
    reader.releaseLock()
  }
}

function isChatEvent(v: unknown): v is ChatEvent {
  return typeof v === 'object' && v !== null && typeof (v as { type?: unknown }).type === 'string'
}

/**
 * POST + SSE del chat. `EventSource` no sirve (solo GET), así que usamos `fetch` + `ReadableStream`.
 * Termina tras `done` o `error`; si el servidor corta antes emite un `error` sintético.
 * Con `signal` abortado lanza `AbortError`.
 */
export async function* streamChatEvents(
  path: string,
  body: unknown,
  signal?: AbortSignal,
  fetchImpl: typeof fetch = fetch,
): AsyncGenerator<ChatEvent> {
  const init: RequestInit = {
    method: 'POST',
    headers: { Accept: 'text/event-stream', 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  }
  if (signal) init.signal = signal

  let res: Response
  try {
    res = await fetchImpl(`${API_BASE}${path}`, init)
  } catch (e) {
    if (isAbortError(e)) throw e
    throw new ApiError(0, 'network', 'No se pudo conectar con el servidor')
  }
  if (!res.ok) throw await toApiError(res)
  if (!res.body) throw new ApiError(res.status, 'invalid_response', 'La respuesta no incluye un stream')

  let terminated = false
  try {
    for await (const msg of parseSseStream(res.body, signal)) {
      let parsed: unknown
      try {
        parsed = JSON.parse(msg.data)
      } catch {
        console.warn('[sse] evento no válido ignorado', msg.data.slice(0, 120))
        continue
      }
      if (!isChatEvent(parsed)) continue
      yield parsed
      if (parsed.type === 'done' || parsed.type === 'error') {
        terminated = true
        return
      }
    }
  } catch (e) {
    if (signal?.aborted || isAbortError(e)) throw new DOMException('Abortado', 'AbortError')
    throw e
  }
  if (signal?.aborted) throw new DOMException('Abortado', 'AbortError')
  if (!terminated) yield { type: 'error', message: 'La conexión se cerró antes de terminar la respuesta' }
}

function isPullEvent(v: unknown): v is PullEvent {
  if (typeof v !== 'object' || v === null) return false
  const t = (v as { type?: unknown }).type
  return t === 'progress' || t === 'done' || t === 'error'
}

/**
 * POST + SSE de la descarga de un modelo (`/models/{id}/pull`). Termina tras `done` o `error`;
 * si el servidor corta antes, emite un `error` sintético. Con `signal` abortado lanza `AbortError`.
 */
export async function* streamPullEvents(
  path: string,
  signal?: AbortSignal,
  fetchImpl: typeof fetch = fetch,
): AsyncGenerator<PullEvent> {
  const init: RequestInit = { method: 'POST', headers: { Accept: 'text/event-stream' } }
  if (signal) init.signal = signal

  let res: Response
  try {
    res = await fetchImpl(`${API_BASE}${path}`, init)
  } catch (e) {
    if (isAbortError(e)) throw e
    throw new ApiError(0, 'network', 'No se pudo conectar con el servidor')
  }
  if (!res.ok) throw await toApiError(res)
  if (!res.body) throw new ApiError(res.status, 'invalid_response', 'La respuesta no incluye un stream')

  let terminated = false
  try {
    for await (const msg of parseSseStream(res.body, signal)) {
      let parsed: unknown
      try {
        parsed = JSON.parse(msg.data)
      } catch {
        continue
      }
      if (!isPullEvent(parsed)) continue
      yield parsed
      if (parsed.type === 'done' || parsed.type === 'error') {
        terminated = true
        return
      }
    }
  } catch (e) {
    if (signal?.aborted || isAbortError(e)) throw new DOMException('Abortado', 'AbortError')
    throw e
  }
  if (signal?.aborted) throw new DOMException('Abortado', 'AbortError')
  if (!terminated) yield { type: 'error', message: 'La conexión se cerró antes de terminar la descarga' }
}
