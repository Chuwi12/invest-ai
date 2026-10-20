import { describe, expect, it } from 'vitest'
import type { HardwareInfo, LlmModel } from '@/api/types'
import { modelFit, TIGHT_MARGIN_GB } from './modelFit'

const hw: HardwareInfo = {
  cpuModel: 'x', cpuThreads: 8, ramTotalGb: 32, ramAvailableGb: 20,
  gpus: [{ name: 'g', vendor: 'nvidia', vramTotalGb: 8, vramFreeGb: 7 }], diskFreeGb: 10,
}
const model = (req: Partial<NonNullable<LlmModel['requirements']>> | null, installed = true, forceGpu = false): LlmModel => ({
  id: 'm', name: 'm', family: 'f', parameters: '7B', activeParameters: null, quantization: 'Q4', contextWindow: 8192, tier: 'balanced',
  supportsTools: true, reasoning: false, description: '', curated: true, installed, active: false, forceGpu, gpuFallback: null,
  requirements: req ? { minVramGb: 4, recommendedVramGb: 5, minRamGb: 10, diskGb: 5, cpuOnlyOk: true, ...req } : null,
})

describe('modelFit', () => {
  it('cabe en VRAM', () => expect(modelFit(model({}), hw).kind).toBe('vram'))
  it('con descarga parcial a RAM si la VRAM no alcanza', () => expect(modelFit(model({ minVramGb: 20, minRamGb: 24 }), hw).kind).toBe('partial'))
  it('no cabe si falta RAM', () => expect(modelFit(model({ minVramGb: 40, minRamGb: 64 }), hw).kind).toBe('no'))
  it('sin GPU: CPU', () => expect(modelFit(model({}), { ...hw, gpus: [] }).kind).toBe('cpu'))
  it('sin requisitos: desconocido', () => expect(modelFit(model(null), hw).kind).toBe('unknown'))
  it('no cuenta con la VRAM entera: 7,7 GB recomendados en 8 GB es justo (medido: 12 % a CPU)', () => {
    const fit = modelFit(model({ minVramGb: 4.8, recommendedVramGb: 7.7 }), hw)
    expect(fit.kind).toBe('partial')
    expect(fit.text).toMatch(/CPU/)
    expect(fit.text).toMatch(/10 %/)
    // 14B: ~40 % fuera de la VRAM.
    expect(modelFit(model({ minVramGb: 6.6, recommendedVramGb: 11.7 }), hw).text).toMatch(/40 %/)
  })
  it('cabe entero con margen tras la reserva (5,8 GB en 8 GB)', () => {
    const fit = modelFit(model({ minVramGb: 3.2, recommendedVramGb: 5.8 }), hw)
    expect(fit.kind).toBe('vram')
    expect(fit.text).toMatch(/utilizables/)
  })
  it('avisa si no hay disco para descargarlo', () => {
    expect(modelFit(model({ diskGb: 40 }, false), hw).note).toMatch(/disco/)
    expect(modelFit(model({ diskGb: 40 }, true), hw).note).toBeNull()
  })
  describe('cabe justo (puede ir parcialmente en CPU)', () => {
    // Medido (RTX 4070 Laptop 8 GB, num_ctx 16384, KV q8_0): con el escritorio ocupando ~1 GB, qwen3:8b (6,1 GB
    // recomendados tras la corrección del catálogo) quedó al 89 % en GPU y qwen3.5:9b (6,5 GB) al 81 %;
    // qwen2.5:7b (4,9 GB) y qwen3.5:4b (4,0 GB) iban enteros.
    const hw8: HardwareInfo = { ...hw, gpus: [{ name: 'g', vendor: 'nvidia', vramTotalGb: 7.99, vramFreeGb: 7 }] }
    it('qwen3:8b y qwen3.5:9b medidos: justo; qwen2.5:7b y qwen3.5:4b: cabe', () => {
      for (const rec of [6.1, 6.5]) expect(modelFit(model({ minVramGb: 3, recommendedVramGb: rec }), hw8).kind).toBe('tight')
      for (const rec of [4.9, 4.0]) expect(modelFit(model({ minVramGb: 2, recommendedVramGb: rec }), hw8).kind).toBe('vram')
    })
    it('el umbral es total − margen (en 8 GB, ~5,9 GB) y el margen se documenta en una constante', () => {
      expect(TIGHT_MARGIN_GB).toBeGreaterThan(1.5)
      expect(modelFit(model({ minVramGb: 3, recommendedVramGb: 8 - TIGHT_MARGIN_GB - 0.1 }), hw8).kind).toBe('vram')
      expect(modelFit(model({ minVramGb: 3, recommendedVramGb: 8 - TIGHT_MARGIN_GB + 0.2 }), hw8).kind).toBe('tight')
    })
    it('el texto avisa de la CPU y de «Forzar GPU»; con el ajuste activado dice cuánto queda libre', () => {
      const off = modelFit(model({ recommendedVramGb: 6.1 }), hw8)
      expect(off.text).toMatch(/CPU/)
      expect(off.text).toMatch(/Forzar GPU/)
      const on = modelFit(model({ recommendedVramGb: 6.1 }, true, true), hw8)
      expect(on.kind).toBe('tight')
      expect(on.text).toMatch(/quedan ~1,9 GB/)
    })
    it('lo que no cabe entero sigue siendo «parcial» y no se confunde con «justo»', () => {
      expect(modelFit(model({ minVramGb: 4.8, recommendedVramGb: 7.7 }), hw8).kind).toBe('partial')
    })
  })
})
