/// <reference types="node" />
import { readFileSync } from 'node:fs'
import { afterEach, describe, expect, it } from 'vitest'

// `public/theme-init.js` es un script clásico que duplica a propósito la lógica de `theme.ts` (se ejecuta
// antes de que cargue la app): este test los mantiene de acuerdo.
const SRC = readFileSync('public/theme-init.js', 'utf8') // vitest corre con `frontend/` como raíz
const run = () => new Function(SRC)()

describe('theme-init.js', () => {
  afterEach(() => {
    localStorage.clear()
    document.documentElement.removeAttribute('data-theme')
  })

  it.each(['light', 'dark'])('con «%s» guardado fija data-theme', (v) => {
    localStorage.setItem('invest-ai.theme', v)
    run()
    expect(document.documentElement.getAttribute('data-theme')).toBe(v)
  })

  it.each([null, 'system', 'otra'])('con %s no toca el atributo (manda el sistema)', (v) => {
    if (v !== null) localStorage.setItem('invest-ai.theme', v)
    run()
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false)
  })

  it('si el almacenamiento falla, no lanza', () => {
    const orig = Storage.prototype.getItem
    Storage.prototype.getItem = () => {
      throw new Error('bloqueado')
    }
    try {
      expect(run).not.toThrow()
    } finally {
      Storage.prototype.getItem = orig
    }
  })
})
