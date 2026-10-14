import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { Provenance } from './Provenance'

describe('Provenance', () => {
  it('muestra «verificar en la fuente» solo con una URL real y segura', () => {
    const { rerender } = render(<Provenance provider="yahoo" asOf="2026-10-09T20:00:00Z" verifyUrl="https://finance.yahoo.com/quote/AAPL" />)
    const a = screen.getByRole('link', { name: /verificar en la fuente/i })
    expect(a).toHaveAttribute('href', 'https://finance.yahoo.com/quote/AAPL')
    expect(a).toHaveAttribute('rel', expect.stringContaining('noopener'))
    rerender(<Provenance provider="yahoo" asOf="2026-10-09T20:00:00Z" verifyUrl="javascript:alert(1)" />)
    expect(screen.queryByText(/verificar en la fuente/i)).toBeNull()
    rerender(<Provenance provider="yahoo" asOf="2026-10-09T20:00:00Z" />)
    expect(screen.queryByText(/verificar en la fuente/i)).toBeNull()
  })
  it('avisa cuando la hora es la de consulta y no la del proveedor', () => {
    render(<Provenance provider="yahoo" asOf="2026-10-09T20:00:00Z" origin="local" />)
    expect(screen.getByText(/hora de consulta/)).toBeInTheDocument()
  })
  it('el retraso declarado es una nota secundaria (icono con tooltip), no un texto que compita con la antigüedad', () => {
    const note = 'El proveedor declara un retraso de hasta 15 s para esta bolsa'
    const { container } = render(<Provenance provider="yahoo" asOf="2026-10-13T16:30:00Z" origin="provider" declaredDelayNote={note} />)
    const icon = screen.getByRole('img', { name: note })
    expect(icon).toHaveAttribute('title', note)
    expect(container.textContent).not.toMatch(/retraso/i)
  })
  it('sin nota no hay icono', () => {
    render(<Provenance provider="yahoo" asOf="2026-10-13T16:30:00Z" declaredDelayNote={null} />)
    expect(screen.queryByRole('img')).toBeNull()
  })
})
