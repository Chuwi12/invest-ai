import type { HardwareInfo, LlmModel } from '@/api/types'

export type FitKind = 'vram' | 'partial' | 'cpu' | 'no' | 'unknown'

export interface Fit {
  kind: FitKind
  /** Frase neutra, sin recomendar: solo describe qué pasaría con el hardware detectado. */
  text: string
  /** Aviso adicional (p. ej. disco insuficiente para descargarlo). */
  note: string | null
}

const gb = (n: number) => `${n.toLocaleString('es-ES', { maximumFractionDigits: 1 })} GB`

/**
 * Compara los requisitos del modelo con el hardware detectado. Es informativa: la app no elige
 * ni oculta nada por ti. La VRAM se suma entre GPUs (Ollama puede repartir capas).
 */
export function modelFit(model: LlmModel, hw: HardwareInfo): Fit {
  const req = model.requirements
  if (!req) return { kind: 'unknown', text: 'Sin datos de requisitos para este modelo.', note: null }

  const vram = hw.gpus.reduce((s, g) => s + g.vramTotalGb, 0)
  let note: string | null = null
  if (!model.installed && hw.diskFreeGb !== null && hw.diskFreeGb < req.diskGb) {
    note = `Ocupa ${gb(req.diskGb)} y quedan ${gb(hw.diskFreeGb)} libres en disco.`
  }

  if (vram >= req.minVramGb && vram > 0) {
    const roomy = vram >= req.recommendedVramGb
    return {
      kind: 'vram',
      text: roomy
        ? `Cabe en la VRAM (${gb(vram)} frente a ${gb(req.recommendedVramGb)} recomendados).`
        : `Cabe en la VRAM con poco margen (${gb(vram)}; recomendados ${gb(req.recommendedVramGb)}).`,
      note,
    }
  }
  if (hw.ramTotalGb >= req.minRamGb) {
    if (vram > 0) {
      return {
        kind: 'partial',
        text: `No cabe entero en la VRAM (${gb(vram)} de ${gb(req.minVramGb)} mínimos): iría en RAM con descarga parcial, más lento.`,
        note,
      }
    }
    return req.cpuOnlyOk
      ? { kind: 'cpu', text: 'Sin GPU detectada: funcionaría solo con CPU, más lento.', note }
      : { kind: 'cpu', text: 'Sin GPU detectada: iría en CPU y RAM; el fabricante no lo recomienda solo con CPU.', note }
  }
  return {
    kind: 'no',
    text: `No cabe: necesita al menos ${gb(req.minRamGb)} de RAM y hay ${gb(hw.ramTotalGb)}.`,
    note,
  }
}
