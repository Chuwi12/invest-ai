import type { HardwareInfo, LlmModel } from '@/api/types'

export type FitKind = 'vram' | 'tight' | 'partial' | 'cpu' | 'no' | 'unknown'

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

/**
 * Margen (GB) que debe quedar entre lo que el catálogo recomienda y la VRAM TOTAL para que Ollama cargue el
 * modelo entero en la GPU con seguridad. Sale de las mediciones del 16 y 19-oct-2026 en una RTX 4070 Laptop de
 * 8 GB (num_ctx 16384, KV q8_0; `docs/notes/model-eval.md`, «Forzar todas las capas en GPU»):
 *  - Con el escritorio ocupando ~0,95 GB de VRAM, Ollama necesitó ~1,0–1,5 GB libres por encima de lo que
 *    ocupa el modelo: qwen3:8b (ocupa 6,0 GB) y qwen3.5:9b (6,4 GB) quedaron al 89 % y 81 % en GPU, mientras
 *    qwen2.5:7b (4,85 GB) y qwen3.5:4b (4,0 GB) iban enteros. Con el escritorio liberado (0,08 GB) los cuatro iban al 100 %.
 *  - 0,9 GB de escritorio + 1,2 GB de holgura de Ollama ≈ 2,1 GB. Con la recomendada corregida del catálogo
 *    (6,1 y 6,5 GB para esos dos), la regla `recomendada > total − 2,1` los marca como «justo» (> 5,9 en 8 GB)
 *    y deja fuera a los dos que cabían (4,9 y 4,0).
 * Es una calibración con 4 modelos en UNA tarjeta: en tarjetas grandes el escritorio pesa relativamente menos,
 * así que el margen es absoluto (GB), no un porcentaje, y es orientativo: la app solo avisa, no decide.
 */
export const TIGHT_MARGIN_GB = 2.1

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
    // Cabe, pero sin holgura para lo que el escritorio ya use: Ollama puede dejar capas en la CPU según la sesión.
    const free = vram - req.recommendedVramGb
    if (req.recommendedVramGb > vram - TIGHT_MARGIN_GB * hw.gpus.length) {
      return {
        kind: 'tight',
        text: model.forceGpu
          ? `Cabe justo: necesita ~${gb(req.recommendedVramGb)} de ${gb(vram)}. Con «Forzar GPU» activado todo el modelo va en la tarjeta y quedan ~${gb(free)} para el escritorio y el navegador.`
          : `Cabe justo: necesita ~${gb(req.recommendedVramGb)} de ${gb(vram)}. Según lo que use el escritorio, Ollama puede dejar parte del modelo (en una tarjeta de 8 GB, entre el 10 y el 20 %) en la CPU y tardar bastante más. «Forzar GPU» lo mantiene entero en la tarjeta si hay sitio.`,
        note,
      }
    }
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
