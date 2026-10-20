import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { HardwareInfo, LlmModel } from '@/api/types'
import { ModelCard } from './ModelCard'

const hw: HardwareInfo = { cpuModel: 'x', cpuThreads: 8, ramTotalGb: 32, ramAvailableGb: 20, gpus: [{ name: 'g', vendor: 'nvidia', vramTotalGb: 8, vramFreeGb: 7 }], diskFreeGb: 300 }
const m: LlmModel = {
  id: 'qwen3:30b', name: 'Qwen 3 30B', family: 'Qwen', parameters: '30B', activeParameters: '3B', quantization: 'Q4_K_M', contextWindow: 40960, tier: 'heavy',
  supportsTools: true, reasoning: true, description: 'MoE', curated: true, installed: false, active: false, forceGpu: false, gpuFallback: null,
  requirements: { minVramGb: 20, recommendedVramGb: 24, minRamGb: 24, diskGb: 18.6, cpuOnlyOk: true },
}
const handlers = () => ({ onPull: vi.fn(), onCancel: vi.fn(), onDismiss: vi.fn(), onActivate: vi.fn(), onDelete: vi.fn(), onForceGpu: vi.fn() })

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
  it('forzar GPU: solo en modelos instalados, apagado por defecto y avisa al cambiarlo', async () => {
    const h = handlers()
    const { rerender } = render(<ModelCard model={m} hardware={hw} pull={undefined} busy={false} {...h} />)
    expect(screen.queryByRole('checkbox', { name: /Forzar todas las capas en GPU/ })).toBeNull()
    rerender(<ModelCard model={{ ...m, installed: true }} hardware={hw} pull={undefined} busy={false} {...h} />)
    const box = screen.getByRole('checkbox', { name: /Forzar todas las capas en GPU/ })
    expect(box).not.toBeChecked()
    await userEvent.click(box)
    expect(h.onForceGpu).toHaveBeenCalledWith('qwen3:30b', true)
    rerender(<ModelCard model={{ ...m, installed: true, forceGpu: true }} hardware={hw} pull={undefined} busy={false} {...h} />)
    expect(screen.getByRole('checkbox', { name: /Forzar todas las capas en GPU/ })).toBeChecked()
    await userEvent.click(screen.getByRole('checkbox', { name: /Forzar todas las capas en GPU/ }))
    expect(h.onForceGpu).toHaveBeenLastCalledWith('qwen3:30b', false)
  })
  it('si forzar la GPU falló por memoria lo dice y enseña el motivo de Ollama', () => {
    render(<ModelCard model={{ ...m, installed: true, forceGpu: true, gpuFallback: 'cudaMalloc failed: out of memory' }} hardware={hw} pull={undefined} busy={false} {...handlers()} />)
    expect(screen.getByRole('status')).toHaveTextContent(/reparto automático/)
    expect(screen.getByRole('status')).toHaveTextContent(/cudaMalloc failed/)
  })
  it('muestra el % real en GPU del modelo cargado y lo marca si no está entero', () => {
    const partial = { id: 'qwen3:30b', size: 6_732_182_977, sizeVram: 6_004_639_006, gpuShare: 0.892 }
    const { rerender } = render(<ModelCard model={{ ...m, installed: true }} hardware={hw} pull={undefined} busy={false} loaded={partial} {...handlers()} />)
    const chip = screen.getByText(/Cargado: 89 % en GPU/)
    expect(chip).toHaveClass('warn')
    rerender(<ModelCard model={{ ...m, installed: true }} hardware={hw} pull={undefined} busy={false} loaded={{ ...partial, sizeVram: partial.size, gpuShare: 1 }} {...handlers()} />)
    expect(screen.getByText(/Cargado: 100 % en GPU/)).not.toHaveClass('warn')
  })
})
