import { describe, expect, it } from 'vitest'
import { safeHttpUrl, sourceUrl } from './sources'

describe('sources', () => {
  it('solo admite http(s)', () => {
    expect(safeHttpUrl('https://example.com/a')).toBe('https://example.com/a')
    expect(safeHttpUrl('javascript:alert(1)')).toBeNull()
    expect(safeHttpUrl('data:text/html,hi')).toBeNull()
    expect(safeHttpUrl('no es url')).toBeNull()
  })
  it('enlaza al activo en Yahoo con el símbolo codificado', () => {
    expect(sourceUrl('yahoo', 'index:^GSPC')).toBe('https://finance.yahoo.com/quote/%5EGSPC')
    expect(sourceUrl('mock')).toBeNull()
  })
})
