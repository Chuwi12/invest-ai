import type { HardwareInfo, LlmModel } from '@/api/types'

export type FitKind = 'vram' | 'partial' | 'cpu' | 'no' | 'unknown'

export interface Fit {
  kind: FitKind
  /** Frase neutra, sin recomendar: solo describe qué pasaría con el hardware detectado. */
  text: string
  /** Aviso adicional (p. ej. disco insuficiente para descargarlo). */
  note: string | null
}

/** VRAM que no puede usar el modelo (driver, contexto de CUDA, escritorio): ~1 GB o el 8 % en tarjetas
 * grandes. Calibrado con la medida de abajo (8 GB → ~7 GB utilizables; qwen3:8b estimado ~10 %, medido 12 %). */
export const vramReserveGb = (total: number) => Math.max(1, total * 0.08)

/** Porcentaje estimado del modelo que no cabe en la VRAM utilizable (orientativo, redondeado a 5). */
const cpuShare = (needGb: number, usableGb: number) => `${Math.max(5, Math.round(((needGb - usableGb) / needGb) * 20) * 5)} %`

const gb = (n: number) => `${n.toLocaleString('es-ES', { maximumFractionDigits: 1 })} GB`

/**
 * Compara los requisitos del modelo con el hardware detectado. Es informativa: la app no elige
 * ni oculta nada por ti. La VRAM se suma entre GPUs (Ollama puede repartir capas).
 *
 * No se cuenta con la VRAM entera: el driver, el contexto de CUDA y el escritorio se quedan una
 * parte. Medido el 14-oct-2026 en una RTX 4070 Laptop de 8 GB: qwen3:8b con 16k de contexto
 * (7,7 GB estimados) se desbordó un 12 % a la CPU, mientras que qwen2.5:7b (5,8 GB) cabía entero.
 */
export function modelFit(model: LlmModel, hw: HardwareInfo): Fit {
  const req = model.requirements
  if (!req) return { kind: 'unknown', text: 'Sin datos de requisitos para este modelo.', note: null }

  const vram = hw.gpus.reduce((s, g) => s + g.vramTotalGb, 0)
  const usable = hw.gpus.reduce((s, g) => s + Math.max(0, g.vramTotalGb - vramReserveGb(g.vramTotalGb)), 0)
  let note: string | null = null
  if (!model.installed && hw.diskFreeGb !== null && hw.diskFreeGb < req.diskGb) {
    note = `Ocupa ${gb(req.diskGb)} y quedan ${gb(hw.diskFreeGb)} libres en disco.`
  }

  if (vram > 0 && usable >= Math.max(req.recommendedVramGb, req.minVramGb)) {
    return {
      kind: 'vram',
      text: `Cabe en la VRAM: necesita ~${gb(req.recommendedVramGb)} y quedan ~${gb(usable)} utilizables de ${gb(vram)}.`,
      note,
    }
  }
  if (vram > 0 && usable >= req.minVramGb) {
    return {
      kind: 'partial',
      text: `No cabe entero: con el contexto configurado necesita ~${gb(req.recommendedVramGb)} y quedan ~${gb(usable)} utilizables de ${gb(vram)}; alrededor del ${cpuShare(req.recommendedVramGb, usable)} iría a la CPU (más lento). Con menos contexto (LLM_NUM_CTX) ocupa menos.`,
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
