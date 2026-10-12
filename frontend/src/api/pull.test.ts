import { describe, expect, it } from 'vitest'
import { streamPullEvents } from './sse'

function sse(chunks: string[], status = 200): typeof fetch {
  return (async () =>
    new Response(new ReadableStream({ start(c) { chunks.forEach((x) => c.enqueue(new TextEncoder().encode(x))); c.close() } }), { status })) as typeof fetch
}

describe('streamPullEvents', () => {
  it('itera progress y termina en done', async () => {
    const out = []
    for await (const e of streamPullEvents('/models/x/pull', undefined, sse(['data: {"type":"progress","status":"pulling","completed":1,"total":4}\n\n', 'data: {"type":"done"}\n\n']))) out.push(e.type)
    expect(out).toEqual(['progress', 'done'])
  })
  it('emite error sintético si el stream se corta', async () => {
    const out = []
    for await (const e of streamPullEvents('/models/x/pull', undefined, sse(['data: {"type":"progress","status":"a","completed":null,"total":null}\n\n']))) out.push(e.type)
    expect(out).toEqual(['progress', 'error'])
  })
})
