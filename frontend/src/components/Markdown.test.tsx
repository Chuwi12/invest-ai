import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { Markdown } from './Markdown'

describe('Markdown seguro', () => {
  it('no interpreta HTML crudo', () => {
    const { container } = render(<Markdown text={'Hola <script>window.x=1</script><img src=x onerror=alert(1)> **fin**'} />)
    expect(container.querySelector('script')).toBeNull()
    expect(container.querySelector('img')).toBeNull()
    expect(screen.getByText('fin').tagName).toBe('STRONG')
  })
  it('enlaces externos con noopener noreferrer y solo http(s)', () => {
    render(<Markdown text={'[ok](https://example.com) y [mal](javascript:alert(1))'} />)
    const ok = screen.getByRole('link', { name: 'ok' })
    expect(ok).toHaveAttribute('rel', expect.stringContaining('noopener'))
    expect(ok).toHaveAttribute('rel', expect.stringContaining('noreferrer'))
    expect(ok).toHaveAttribute('target', '_blank')
    expect(screen.queryByRole('link', { name: 'mal' })).toBeNull()
  })
  it('las imágenes no se cargan', () => {
    const { container } = render(<Markdown text="![x](https://tracker.example/p.png)" />)
    expect(container.querySelector('img')).toBeNull()
  })
  it('renderiza tablas GFM', () => {
    render(<Markdown text={'| a | b |\n|---|---|\n| 1 | 2 |'} />)
    expect(screen.getByRole('table')).toBeInTheDocument()
  })
})
