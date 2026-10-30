import { ApiError } from '@/api/client'
import type { HardwareInfo, LlmModel, ModelRequirements, ModelsResponse, ModelTier, PullEvent } from '@/api/types'
import { sleep } from './rng'

/** Equipo simulado (portátil con 8 GB de VRAM): sirve para ver los tres veredictos de "cabe". */
export const MOCK_HARDWARE: HardwareInfo = {
  cpuModel: 'AMD Ryzen 9 7940HS (simulado)',
  cpuThreads: 16,
  ramTotalGb: 32,
  ramAvailableGb: 21.4,
  gpus: [{ name: 'NVIDIA GeForce RTX 4060 Laptop (simulada)', vendor: 'nvidia', vramTotalGb: 8, vramFreeGb: 7.1 }],
  diskFreeGb: 412,
}

type Row = [
  id: string, name: string, family: string, params: string, active: string | null, quant: string,
  ctx: number, tier: ModelTier | null, tools: boolean, reasoning: boolean, desc: string,
  req: [minVram: number, recVram: number, minRam: number, disk: number, cpuOk: boolean] | null,
  installed: boolean,
]

const rows: Row[] = [
  ['qwen2.5:3b', 'Qwen 2.5 3B', 'Qwen', '3B', null, 'Q4_K_M', 32_768, 'light', true, false, 'Pequeño y rápido. Razona poco con datos extensos; útil para consultas cortas.', [2.5, 4, 6, 1.9, true], false],
  ['llama3.2:3b', 'Llama 3.2 3B', 'Llama', '3B', null, 'Q4_K_M', 131_072, 'light', true, false, 'Contexto largo en un modelo muy ligero.', [2.5, 4, 6, 2, true], false],
  ['qwen2.5:7b', 'Qwen 2.5 7B', 'Qwen', '7B', null, 'Q4_K_M', 32_768, 'balanced', true, false, 'Buen equilibrio entre velocidad y calidad con herramientas.', [5, 6, 10, 4.7, true], true],
  ['llama3.1:8b', 'Llama 3.1 8B', 'Llama', '8B', null, 'Q4_K_M', 131_072, 'balanced', true, false, 'Generalista con contexto de 128k.', [5.5, 7, 11, 4.9, true], true],
  ['qwen3:8b', 'Qwen 3 8B', 'Qwen', '8B', null, 'Q4_K_M', 40_960, 'balanced', true, true, 'Incluye modo de razonamiento explícito.', [5.5, 7, 11, 5.2, true], false],
  ['gemma3:12b', 'Gemma 3 12B', 'Gemma', '12B', null, 'Q4_K_M', 131_072, 'heavy', false, false, 'Escribe muy bien, pero sin soporte de herramientas en Ollama.', [8.5, 11, 16, 8.1, false], false],
  ['qwen3:30b-a3b', 'Qwen 3 30B-A3B', 'Qwen', '30B', '3B', 'Q4_K_M', 40_960, 'heavy', true, true, 'Mezcla de expertos: 30B en total pero solo 3B activos por token. Rinde bien con descarga parcial a RAM.', [6, 20, 24, 18.6, true], true],
  ['qwen2.5:32b', 'Qwen 2.5 32B', 'Qwen', '32B', null, 'Q4_K_M', 32_768, 'max', true, false, 'Mejor calidad de la familia; necesita mucha memoria.', [20, 24, 40, 19.9, false], false],
  ['llama3.3:70b', 'Llama 3.3 70B', 'Llama', '70B', null, 'Q4_K_M', 131_072, 'max', true, false, 'Modelo grande de referencia. Pensado para estaciones con varias GPU.', [40, 48, 64, 42.5, false], false],
  ['deepseek-r1:14b', 'DeepSeek R1 14B', 'DeepSeek', '14B', null, 'Q4_K_M', 131_072, 'heavy', false, true, 'Razonamiento largo; sin herramientas.', [9, 12, 18, 9, false], false],
]

const state = {
  active: 'qwen2.5:7b',
  installed: new Set(rows.filter((r) => r[12]).map((r) => r[0])),
  // Un modelo instalado que no está en el catálogo curado.
  extra: [{ id: 'mi-modelo:latest', name: 'mi-modelo:latest' }],
  forceGpu: new Set<string>(),
}

function toModel(r: Row): LlmModel {
  const req: ModelRequirements | null = r[11]
    ? { minVramGb: r[11][0], recommendedVramGb: r[11][1], minRamGb: r[11][2], diskGb: r[11][3], cpuOnlyOk: r[11][4] }
    : null
  return {
    id: r[0], name: r[1], family: r[2], parameters: r[3], activeParameters: r[4], quantization: r[5],
    contextWindow: r[6], tier: r[7], supportsTools: r[8], reasoning: r[9], description: r[10],
    requirements: req, curated: true, installed: state.installed.has(r[0]), active: state.active === r[0],
    forceGpu: state.forceGpu.has(r[0]), gpuFallback: null,
  }
}

export function mockModels(): ModelsResponse {
  const extras: LlmModel[] = state.extra
    .filter((e) => state.installed.has(e.id))
    .map((e) => ({
      id: e.id, name: e.name, family: 'Desconocida', parameters: '?', activeParameters: null, quantization: '?',
      contextWindow: 0, tier: null, supportsTools: false, reasoning: false, description: 'Modelo local fuera del catálogo.',
      requirements: null, curated: false, installed: true, active: state.active === e.id,
      forceGpu: state.forceGpu.has(e.id), gpuFallback: null,
    }))
  return {
    active: state.active,
    ollamaReachable: true,
    models: [...rows.map(toModel), ...extras],
    hardware: MOCK_HARDWARE,
    // Simulado: el activo está cargado; sin forzar queda al 89 % en la GPU (lo que se ve en una tarjeta de 8 GB).
    loaded: [
      {
        id: state.active,
        size: 6_732_182_977,
        sizeVram: state.forceGpu.has(state.active) ? 6_732_182_977 : 6_004_639_006,
        gpuShare: state.forceGpu.has(state.active) ? 1 : 0.892,
      },
    ],
    numGpu: null,
  }
}

export function mockSetForceGpu(id: string, force: boolean): ModelsResponse {
  if (force) state.forceGpu.add(id)
  else state.forceGpu.delete(id)
  return mockModels()
}

export function installedModelIds(): string[] {
  return [...state.installed]
}

export function activeModelId(): string {
  return state.active
}

export function mockSetActive(id: string): ModelsResponse {
  if (!state.installed.has(id)) throw new ApiError(400, 'invalid_input', `el modelo ${id} no está instalado`)
  state.active = id
  return mockModels()
}

export function mockDelete(id: string): void {
  if (!state.installed.has(id)) throw new ApiError(404, 'not_found', `modelo no instalado: ${id}`)
  if (state.active === id) throw new ApiError(400, 'invalid_input', 'no se puede borrar el modelo activo')
  state.installed.delete(id)
}

/** Descarga simulada: capas con progreso creciente durante unos segundos. */
export async function* mockPull(id: string, signal?: AbortSignal): AsyncGenerator<PullEvent> {
  const row = rows.find((r) => r[0] === id)
  if (!row) throw new ApiError(404, 'not_found', `modelo desconocido: ${id}`)
  const totalBytes = (row[11]?.[3] ?? 4) * 1e9
  yield { type: 'progress', status: 'pulling manifest', completed: null, total: null }
  await sleep(400, signal)
  const steps = 24
  for (let i = 1; i <= steps; i++) {
    await sleep(250, signal)
    yield { type: 'progress', status: 'pulling layer', completed: Math.round((totalBytes * i) / steps), total: totalBytes }
  }
  yield { type: 'progress', status: 'verifying sha256 digest', completed: null, total: null }
  await sleep(500, signal)
  state.installed.add(id)
  yield { type: 'done' }
}
