import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { ChatMessage } from '@/api/types'
import { MessageView } from './MessageView'

const base: ChatMessage = {
  id: 'm1',
  conversationId: 'c1',
  role: 'assistant',
  content: 'Empiezo a responder',
  toolCalls: [],
  analysis: null,
  createdAt: '2026-10-13T10:00:00Z',
}

describe('MessageView: respuesta cortada por un error', () => {
  it('avisa cuando el texto es parcial', () => {
    render(<MessageView m={{ ...base, truncated: true }} />)
    expect(screen.getByText(/Respuesta interrumpida/)).toBeInTheDocument()
  })
  it('no avisa en respuestas completas ni con backends antiguos (sin el campo)', () => {
    const { rerender } = render(<MessageView m={{ ...base, truncated: false }} />)
    expect(screen.queryByText(/Respuesta interrumpida/)).toBeNull()
    rerender(<MessageView m={base} />)
    expect(screen.queryByText(/Respuesta interrumpida/)).toBeNull()
  })
})
