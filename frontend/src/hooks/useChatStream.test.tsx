import { QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import { createQueryClient } from '@/api/queryClient'
import type { ChatEvent } from '@/api/types'
import { mockApi } from '@/mock'
import { useChatStream } from './useChatStream'

const token = (text: string): ChatEvent => ({ type: 'token', text })

/** Control manual de un stream: el test decide cuándo llega cada evento. */
function manualStream() {
  const waiters: Array<(ev: ChatEvent | null) => void> = []
  const buffered: Array<ChatEvent | null> = []
  return {
    push(ev: ChatEvent | null) {
      const w = waiters.shift()
      if (w) w(ev)
      else buffered.push(ev)
    },
    async *iterate(): AsyncGenerator<ChatEvent> {
      for (;;) {
        const ev = buffered.length > 0 ? buffered.shift()! : await new Promise<ChatEvent | null>((r) => waiters.push(r))
        if (ev === null) return
        yield ev
      }
    },
  }
}

function setup() {
  const qc = createQueryClient()
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  return renderHook(() => useChatStream(), { wrapper })
}

describe('useChatStream', () => {
  beforeEach(() => setApi(mockApi))
  afterEach(() => setApi(mockApi))

  it('los eventos que aún llegan de una respuesta abortada no se mezclan con la nueva', async () => {
    // Un chunk de red trae varios eventos: el generador sigue entregando los ya decodificados
    // aunque se haya abortado (parseSseStream no mira `signal` entre eventos).
    const a = manualStream()
    const b = manualStream()
    const runs = [a, b]
    setApi({ ...mockApi, sendMessage: () => runs.shift()!.iterate() })
    const { result } = setup()

    act(() => void result.current.send('c1', 'primera'))
    a.push(token('A1 '))
    await waitFor(() => expect(result.current.state?.text).toBe('A1 '))

    // El usuario cambia de conversación: se aborta A y se lanza B.
    act(() => void result.current.send('c2', 'segunda'))
    b.push(token('B1'))
    // …pero A todavía entrega dos eventos que ya tenía en el buffer.
    a.push(token('A2 '))
    a.push(token('A3 '))
    await waitFor(() => expect(result.current.state?.cid).toBe('c2'))
    await waitFor(() => expect(result.current.state?.text).toContain('B1'))
    // Dejamos pasar las microtareas de A.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30))
    })
    expect(result.current.state?.cid).toBe('c2')
    expect(result.current.state?.text).toBe('B1')
    a.push(null)
    b.push(null)
  })

  it('abort deja el texto recibido y marca la respuesta como terminada', async () => {
    const a = manualStream()
    setApi({ ...mockApi, sendMessage: (_cid, _body, signal) => (async function* () {
      for await (const ev of a.iterate()) {
        if (signal?.aborted) throw new DOMException('Abortado', 'AbortError')
        yield ev
      }
    })() })
    const { result } = setup()
    act(() => void result.current.send('c1', 'hola'))
    a.push(token('parcial'))
    await waitFor(() => expect(result.current.state?.text).toBe('parcial'))
    act(() => result.current.abort())
    a.push(token('x'))
    await waitFor(() => expect(result.current.state?.done).toBe(true))
    expect(result.current.state?.text).toBe('parcial')
  })
})
