import { describe, expect, it, vi } from 'vitest'
import { ApiError } from './client'
import { SseParser, parseSseStream, streamChatEvents } from './sse'
import type { ChatEvent } from './types'

const enc = new TextEncoder()

function streamOf(chunks: (string | Uint8Array)[], opts?: { hang?: boolean }): ReadableStream<Uint8Array> {
  let i = 0
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      const c = chunks[i++]
      if (c === undefined) {
        if (!opts?.hang) controller.close()
        else return new Promise(() => undefined)
        return
      }
      controller.enqueue(typeof c === 'string' ? enc.encode(c) : c)
    },
  })
}

async function collect<T>(it: AsyncIterable<T>): Promise<T[]> {
  const out: T[] = []
  for await (const v of it) out.push(v)
  return out
}

function sseResponse(chunks: string[], init?: ResponseInit): Response {
  return new Response(streamOf(chunks), {
    status: 200,
    headers: { 'Content-Type': 'text/event-stream' },
    ...init,
  })
}

describe('SseParser', () => {
  it('parsea eventos separados por línea en blanco', () => {
    const p = new SseParser()
    expect(p.feed('data: {"a":1}\n\ndata: {"a":2}\n\n')).toEqual([
      { event: 'message', data: '{"a":1}' },
      { event: 'message', data: '{"a":2}' },
    ])
  })

  it('une varias líneas data con salto de línea', () => {
    const p = new SseParser()
    expect(p.feed('data: uno\ndata: dos\n\n')).toEqual([{ event: 'message', data: 'uno\ndos' }])
  })

  it('soporta CRLF y CR sueltos', () => {
    const p = new SseParser()
    expect([...p.feed('data: a\r\n\r\ndata: b\r\r'), ...p.flush()]).toEqual([
      { event: 'message', data: 'a' },
      { event: 'message', data: 'b' },
    ])
  })

  it('un CRLF partido entre trozos no genera un evento vacío', () => {
    const p = new SseParser()
    expect([...p.feed('data: a\r'), ...p.feed('\n\r'), ...p.feed('\n')]).toEqual([{ event: 'message', data: 'a' }])
  })

  it('une trozos partidos a mitad de línea', () => {
    const p = new SseParser()
    const out = [...p.feed('da'), ...p.feed('ta: {"type":"to'), ...p.feed('ken"}\n'), ...p.feed('\n')]
    expect(out).toEqual([{ event: 'message', data: '{"type":"token"}' }])
  })

  it('ignora comentarios y campos desconocidos, respeta event e id', () => {
    const p = new SseParser()
    expect(p.feed(': keep-alive\n\nevent: ping\nid: 7\nretry: 100\nfoo: bar\ndata: x\n\n')).toEqual([
      { event: 'ping', data: 'x', id: '7' },
    ])
  })

  it('quita solo un espacio inicial del valor', () => {
    const p = new SseParser()
    expect(p.feed('data:  dos espacios\n\ndata:sin espacio\n\n')).toEqual([
      { event: 'message', data: ' dos espacios' },
      { event: 'message', data: 'sin espacio' },
    ])
  })

  it('quita el BOM inicial', () => {
    const p = new SseParser()
    expect(p.feed('﻿data: a\n\n')).toEqual([{ event: 'message', data: 'a' }])
  })

  it('flush emite el último evento aunque falte la línea en blanco', () => {
    const p = new SseParser()
    expect(p.feed('data: ultimo\n')).toEqual([])
    expect(p.flush()).toEqual([{ event: 'message', data: 'ultimo' }])
  })

  it('una línea sin data no emite evento', () => {
    const p = new SseParser()
    expect(p.feed('event: x\n\n')).toEqual([])
  })
})

describe('parseSseStream', () => {
  it('decodifica UTF-8 partido en mitad de un carácter multibyte', async () => {
    const bytes = enc.encode('data: {"text":"acción €"}\n\n')
    // Partimos justo dentro de «ó» (2 bytes) y de «€» (3 bytes).
    const cut1 = bytes.indexOf(0xc3) + 1
    const cut2 = bytes.indexOf(0xe2) + 1
    const msgs = await collect(
      parseSseStream(streamOf([bytes.slice(0, cut1), bytes.slice(cut1, cut2), bytes.slice(cut2)])),
    )
    expect(msgs).toHaveLength(1)
    expect(JSON.parse(msgs[0]!.data)).toEqual({ text: 'acción €' })
  })

  it('se cancela con AbortSignal', async () => {
    const ac = new AbortController()
    const stream = streamOf(['data: a\n\n'], { hang: true })
    const it = parseSseStream(stream, ac.signal)
    const first = await it.next()
    expect(first.value).toEqual({ event: 'message', data: 'a' })
    const pending = it.next()
    ac.abort()
    const res = await pending
    expect(res.done).toBe(true)
  })
})

describe('streamChatEvents', () => {
  const ev = (e: ChatEvent) => `data: ${JSON.stringify(e)}\n\n`

  it('hace POST con JSON y emite los eventos hasta done', async () => {
    const fetchMock = vi.fn(async () =>
      sseResponse([
        ev({ type: 'start', messageId: 'm1' }),
        ev({ type: 'token', text: 'Ho' }) + ev({ type: 'token', text: 'la' }),
        ev({ type: 'done', messageId: 'm1', model: 'qwen', promptTokens: 3, completionTokens: 2 }),
        ev({ type: 'token', text: 'NO DEBE LLEGAR' }),
      ]),
    )
    const events = await collect(
      streamChatEvents('/conversations/c1/messages', { content: 'hola' }, undefined, fetchMock as unknown as typeof fetch),
    )
    expect(events.map((e) => e.type)).toEqual(['start', 'token', 'token', 'done'])
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('/api/conversations/c1/messages')
    expect(init.method).toBe('POST')
    expect(init.body).toBe('{"content":"hola"}')
    expect((init.headers as Record<string, string>).Accept).toBe('text/event-stream')
  })

  it('termina en el evento error', async () => {
    const f = vi.fn(async () => sseResponse([ev({ type: 'error', message: 'LLM caído' })]))
    const events = await collect(streamChatEvents('/x', {}, undefined, f as unknown as typeof fetch))
    expect(events).toEqual([{ type: 'error', message: 'LLM caído' }])
  })

  it('si el stream se corta sin done/error emite un error sintético', async () => {
    const f = vi.fn(async () => sseResponse([ev({ type: 'token', text: 'a' })]))
    const events = await collect(streamChatEvents('/x', {}, undefined, f as unknown as typeof fetch))
    expect(events.map((e) => e.type)).toEqual(['token', 'error'])
  })

  it('ignora JSON roto y eventos sin type', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const f = vi.fn(async () =>
      sseResponse(['data: {no json\n\n', 'data: {"foo":1}\n\n', ev({ type: 'done', messageId: 'm', model: 'x', promptTokens: null, completionTokens: null })]),
    )
    const events = await collect(streamChatEvents('/x', {}, undefined, f as unknown as typeof fetch))
    expect(events.map((e) => e.type)).toEqual(['done'])
    warn.mockRestore()
  })

  it('convierte un HTTP no-2xx en ApiError con el cuerpo del contrato', async () => {
    const f = vi.fn(
      async () =>
        new Response(JSON.stringify({ error: { code: 'not_found', message: 'conversación inexistente' } }), {
          status: 404,
        }),
    )
    const err = await collect(streamChatEvents('/x', {}, undefined, f as unknown as typeof fetch)).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 404, code: 'not_found', message: 'conversación inexistente' })
  })

  it('lanza AbortError si se aborta a mitad del stream', async () => {
    const ac = new AbortController()
    const f = vi.fn(
      async () =>
        new Response(streamOf([ev({ type: 'token', text: 'a' })], { hang: true }), {
          status: 200,
          headers: { 'Content-Type': 'text/event-stream' },
        }),
    )
    const it = streamChatEvents('/x', {}, ac.signal, f as unknown as typeof fetch)
    const first = await it.next()
    expect(first.value).toMatchObject({ type: 'token' })
    const pending = it.next()
    ac.abort()
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' })
  })

  it('propaga AbortError si el fetch ya está abortado', async () => {
    const ac = new AbortController()
    ac.abort()
    const f = vi.fn(async (_u: string, init: RequestInit) => {
      if (init.signal?.aborted) throw new DOMException('Abortado', 'AbortError')
      return sseResponse([])
    })
    await expect(collect(streamChatEvents('/x', {}, ac.signal, f as unknown as typeof fetch))).rejects.toMatchObject({
      name: 'AbortError',
    })
  })
})
