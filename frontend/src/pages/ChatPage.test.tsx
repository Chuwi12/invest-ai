import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { setApi } from '@/api'
import type { ChatEvent, ChatMessage, Conversation, ConversationWithMessages } from '@/api/types'
import { App } from '@/app/App'
import { mockApi } from '@/mock'
import { renderApp } from '@/test/utils'

const NOW = '2026-10-13T10:00:00Z'
const conv: Conversation = { id: 'c1', title: 'Prueba', assetIds: [], pinned: false, messageCount: 0, lastMessagePreview: null, createdAt: NOW, updatedAt: NOW }
const msg = (id: string, role: 'user' | 'assistant', content: string, extra: Partial<ChatMessage> = {}): ChatMessage => ({
  id, conversationId: 'c1', role, content, toolCalls: [], analysis: null, createdAt: NOW, ...extra,
})

/** Backend simulado: `saved` es lo que el servidor ha guardado de la conversación. */
function fakeBackend(events: ChatEvent[], onStreamEnd: (saved: ChatMessage[]) => ChatMessage[]) {
  let saved: ChatMessage[] = []
  setApi({
    ...mockApi,
    listConversations: async () => [conv],
    getConversation: async (): Promise<ConversationWithMessages> => ({ conversation: conv, messages: saved }),
    sendMessage: async function* () {
      for (const ev of events) yield ev
      saved = onStreamEnd(saved)
    },
  })
}

async function ask(text: string) {
  renderApp(<App />, '/chat/c1')
  const box = await screen.findByRole('textbox', { name: 'Mensaje' })
  await userEvent.type(box, text)
  await userEvent.click(screen.getByRole('button', { name: 'Enviar' }))
}

describe('ChatPage: una respuesta no se pinta dos veces', () => {
  beforeAll(async () => {
    await import('@/pages/ChatPage') // chunk perezoso: se transforma una vez, fuera de las esperas
  })
  beforeEach(() => setApi(mockApi))

  it('si un error corta la respuesta y el servidor guardó el texto parcial, sale una sola vez (con el aviso de truncado)', async () => {
    fakeBackend(
      [{ type: 'token', text: 'Respuesta parcial del modelo' }, { type: 'error', message: 'Ollama se cayó' }],
      () => [msg('m1', 'user', 'hola'), msg('m2', 'assistant', 'Respuesta parcial del modelo', { truncated: true })],
    )
    await ask('hola')
    await screen.findByText(/Respuesta interrumpida por un error/)
    await waitFor(() => expect(screen.getAllByText(/Respuesta parcial del modelo/)).toHaveLength(1))
    // El mensaje del usuario tampoco se duplica.
    expect(screen.getAllByText('hola', { selector: 'div' })).toHaveLength(1)
  })

  it('si el error llega sin que el servidor guarde nada, el aviso del error sigue visible', async () => {
    fakeBackend([{ type: 'error', message: 'Ollama no responde' }], (s) => s)
    await ask('hola')
    await screen.findByText(/La respuesta se interrumpió/)
    expect(screen.getByText(/Ollama no responde/)).toBeInTheDocument()
  })

  it('una respuesta completa queda una sola vez cuando el hilo guardado ya la trae', async () => {
    fakeBackend(
      [{ type: 'token', text: 'Todo bien' }, { type: 'done', messageId: 'm2', model: 'x', promptTokens: 1, completionTokens: 1 }],
      () => [msg('m1', 'user', 'hola'), msg('m2', 'assistant', 'Todo bien')],
    )
    await ask('hola')
    await waitFor(() => expect(screen.getAllByText('Todo bien')).toHaveLength(1))
    await waitFor(() => expect(screen.getAllByText('hola', { selector: 'div' })).toHaveLength(1))
  })
})
