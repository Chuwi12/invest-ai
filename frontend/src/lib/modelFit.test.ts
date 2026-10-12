import { describe, expect, it } from 'vitest'
import type { HardwareInfo, LlmModel } from '@/api/types'
import { modelFit } from './modelFit'

const hw: HardwareInfo = {
  cpuModel: 'x', cpuThreads: 8, ramTotalGb: 32, ramAvailableGb: 20,
  gpus: [{ name: 'g', vendor: 'nvidia', vramTotalGb: 8, vramFreeGb: 7 }], diskFreeGb: 10,
}
const model = (req: Partial<NonNullable<LlmModel['requirements']>> | null, installed = true): LlmModel => ({
  id: 'm', name: 'm', family: 'f', parameters: '7B', activeParameters: null, quantization: 'Q4', contextWindow: 8192, tier: 'balanced',
  supportsTools: true, reasoning: false, description: '', curated: true, installed, active: false,
  requirements: req ? { minVramGb: 5, recommendedVramGb: 6, minRamGb: 10, diskGb: 5, cpuOnlyOk: true, ...req } : null,
})

describe('modelFit', () => {
  it('cabe en VRAM', () => expect(modelFit(model({}), hw).kind).toBe('vram'))
  it('con descarga parcial a RAM si la VRAM no alcanza', () => expect(modelFit(model({ minVramGb: 20, minRamGb: 24 }), hw).kind).toBe('partial'))
  it('no cabe si falta RAM', () => expect(modelFit(model({ minVramGb: 40, minRamGb: 64 }), hw).kind).toBe('no'))
  it('sin GPU: CPU', () => expect(modelFit(model({}), { ...hw, gpus: [] }).kind).toBe('cpu'))
  it('sin requisitos: desconocido', () => expect(modelFit(model(null), hw).kind).toBe('unknown'))
  it('avisa si no hay disco para descargarlo', () => {
    expect(modelFit(model({ diskGb: 40 }, false), hw).note).toMatch(/disco/)
    expect(modelFit(model({ diskGb: 40 }, true), hw).note).toBeNull()
  })
})
