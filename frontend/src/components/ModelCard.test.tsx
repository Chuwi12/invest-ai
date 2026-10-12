import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { HardwareInfo, LlmModel } from '@/api/types'
import { ModelCard } from './ModelCard'

const hw: HardwareInfo = { cpuModel: 'x', cpuThreads: 8, ramTotalGb: 32, ramAvailableGb: 20, gpus: [{ name: 'g', vendor: 'nvidia', vramTotalGb: 8, vramFreeGb: 7 }], diskFreeGb: 300 }
const m: LlmModel = {
  id: 'qwen3:30b', name: 'Qwen 3 30B', family: 'Qwen', parameters: '30B', activeParameters: '3B', quantization: 'Q4_K_M', contextWindow: 40960, tier: 'heavy',
  supportsTools: true, reasoning: true, description: 'MoE', curated: true, installed: false, active: false,
  requirements: { minVramGb: 20, recommendedVramGb: 24, minRamGb: 24, diskGb: 18.6, cpuOnlyOk: true },
}
const handlers = () => ({ onPull: vi.fn(), onCancel: vi.fn(), onDismiss: vi.fn(), onActivate: vi.fn(), onDelete: vi.fn() })

describe('ModelCard', () => {
  it('muestra requisitos y una indicación neutra de encaje, y descarga', async () => {
    const h = handlers()
    render(<ModelCard model={m} hardware={hw} pull={undefined} busy={false} {...h} />)
    expect(screen.getByText(/RAM con descarga parcial/)).toBeInTheDocument()
    expect(screen.getByText('(3B activos)', { exact: false })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: /Descargar/ }))
    expect(h.onPull).toHaveBeenCalledWith('qwen3:30b')
  })
  it('barra de progreso accesible durante la descarga', () => {
    render(<ModelCard model={m} hardware={hw} pull={{ status: 'pulling', completed: 5e9, total: 10e9 }} busy={false} {...handlers()} />)
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '50')
  })
  it('instalado: activar y borrado con confirmación; el activo no se puede borrar', async () => {
    const h = handlers()
    const { rerender } = render(<ModelCard model={{ ...m, installed: true }} hardware={hw} pull={undefined} busy={false} {...h} />)
    await userEvent.click(screen.getByRole('button', { name: 'Activar' }))
    expect(h.onActivate).toHaveBeenCalledWith('qwen3:30b')
    await userEvent.click(screen.getByRole('button', { name: /Borrar/ }))
    expect(h.onDelete).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: /Sí, borrar/ }))
    expect(h.onDelete).toHaveBeenCalledWith('qwen3:30b')
    rerender(<ModelCard model={{ ...m, installed: true, active: true }} hardware={hw} pull={undefined} busy={false} {...h} />)
    expect(screen.getByRole('button', { name: /Borrar/ })).toBeDisabled()
  })
})
