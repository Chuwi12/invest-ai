import { useCallback, useMemo, useState } from 'react'
import { useActivateModel, useDeleteModel, useModelPulls, useModels, useSetForceGpu } from '@/api/hooks'
import type { HardwareInfo, LlmModel, LoadedModel, ModelTier } from '@/api/types'
import { MODEL_TIERS } from '@/api/types'
import { ModelCard } from '@/components/ModelCard'
import { ErrorNotice, Section, SkeletonRows } from '@/components/states'
import { errorText } from '@/lib/errors'
import { useTitle } from '@/hooks/useTitle'

const TIER_INFO: Record<ModelTier | 'other', { title: string; text: string }> = {
  light: { title: 'Ligeros', text: 'Pequeños: responden rápido y piden poca memoria. Razonan menos con contextos grandes.' },
  balanced: { title: 'Equilibrados', text: 'Tamaño intermedio, el punto habitual entre velocidad y calidad.' },
  heavy: { title: 'Pesados', text: 'Más calidad a costa de memoria y tiempo; algunos usan descarga parcial a RAM.' },
  max: { title: 'Máximos', text: 'Los más grandes del catálogo. Requieren mucha VRAM o RAM.' },
  other: { title: 'Sin clasificar', text: 'Instalados en Ollama pero fuera del catálogo: no hay datos de requisitos.' },
}

const gb = (n: number) => `${n.toLocaleString('es-ES', { maximumFractionDigits: 1 })} GB`

function Hardware({ hw }: { hw: HardwareInfo }) {
  return (
    <div className="hw" aria-label="Hardware detectado">
      <div>
        <div className="small muted">Procesador</div>
        <div className="v trunc" title={hw.cpuModel}>{hw.cpuModel}</div>
        <div className="small muted">{hw.cpuThreads} hilos</div>
      </div>
      <div>
        <div className="small muted">Memoria RAM</div>
        <div className="v num">{gb(hw.ramTotalGb)}</div>
        <div className="small muted num">{gb(hw.ramAvailableGb)} disponibles</div>
      </div>
      <div>
        <div className="small muted">Tarjetas gráficas</div>
        {hw.gpus.length === 0 ? (
          <div className="v">Ninguna detectada</div>
        ) : (
          hw.gpus.map((g) => (
            <div key={g.name}>
              <div className="v trunc" title={g.name}>{g.name}</div>
              <div className="small muted num">
                {gb(g.vramTotalGb)} de VRAM{g.vramFreeGb !== null ? ` · ${gb(g.vramFreeGb)} libres` : ''}
              </div>
            </div>
          ))
        )}
      </div>
      <div>
        <div className="small muted">Disco libre</div>
        <div className="v num">{hw.diskFreeGb !== null ? gb(hw.diskFreeGb) : 'Desconocido'}</div>
      </div>
    </div>
  )
}

/** Modelos cargados ahora en Ollama con el % real en GPU (`/api/ps`: size_vram / size). */
function Loaded({ loaded, models }: { loaded: LoadedModel[]; models: LlmModel[] }) {
  if (loaded.length === 0) {
    return <p className="small muted">Ningún modelo cargado en memoria ahora mismo (Ollama lo carga con la primera petición).</p>
  }
  return (
    <ul className="stack small" style={{ gap: '0.25rem', margin: 0, paddingLeft: '1.1rem' }} aria-label="Modelos cargados">
      {loaded.map((l) => {
        const pct = Math.round(l.gpuShare * 100)
        const name = models.find((m) => m.id === l.id)?.name ?? l.id
        return (
          <li key={l.id}>
            <strong>{name}</strong>: <span className="num">{pct} % en GPU</span> ({gb(l.sizeVram / 2 ** 30)} en VRAM de {gb(l.size / 2 ** 30)})
            {pct < 100 ? <span className="down"> — el resto va en CPU, más lento. «Forzar todas las capas en GPU» (en la tarjeta del modelo) lo evita si cabe.</span> : null}
          </li>
        )
      })}
    </ul>
  )
}

export default function ModelsPage() {
  useTitle('Modelos de IA')
  const q = useModels()
  const activate = useActivateModel()
  const del = useDeleteModel()
  const forceGpu = useSetForceGpu()
  const { pulls, start, cancel, dismiss } = useModelPulls()
  const [actionError, setActionError] = useState<string | null>(null)

  const onActivate = useCallback(
    (id: string) => {
      setActionError(null)
      activate.mutate(id, { onError: (e) => setActionError(errorText(e)) })
    },
    [activate],
  )
  const onDelete = useCallback(
    (id: string) => {
      setActionError(null)
      del.mutate(id, { onError: (e) => setActionError(errorText(e)) })
    },
    [del],
  )
  const onForceGpu = useCallback(
    (id: string, force: boolean) => {
      setActionError(null)
      forceGpu.mutate({ id, force }, { onError: (e) => setActionError(errorText(e)) })
    },
    [forceGpu],
  )
  const onPull = useCallback((id: string) => void start(id), [start])

  const groups = useMemo(() => {
    const by = new Map<ModelTier | 'other', LlmModel[]>()
    for (const m of q.data?.models ?? []) {
      const k = m.tier ?? 'other'
      by.set(k, [...(by.get(k) ?? []), m])
    }
    return [...MODEL_TIERS, 'other' as const].filter((t) => by.has(t)).map((t) => ({ tier: t, models: by.get(t)! }))
  }, [q.data])

  const busy = activate.isPending || del.isPending || forceGpu.isPending

  return (
    <div className="page">
      <div className="page-head">
        <h1>Modelos de IA</h1>
        <p className="muted small grow" style={{ maxWidth: '46rem' }}>
          Aquí eliges qué modelo local genera los análisis y el chat. La aplicación no decide por ti: la
          indicación de cada tarjeta solo compara los requisitos con el hardware detectado.
        </p>
      </div>

      {q.isPending ? <SkeletonRows rows={4} h="4rem" /> : null}
      {q.isError ? <ErrorNotice error={q.error} onRetry={() => void q.refetch()} title="No se pudo cargar el catálogo" /> : null}
      {actionError ? (
        <div className="notice error" role="alert">
          <span className="grow">{actionError}</span>
          <button type="button" className="btn sm" onClick={() => setActionError(null)}>Cerrar</button>
        </div>
      ) : null}

      {q.data ? (
        <>
          {!q.data.ollamaReachable ? (
            <div className="notice warn" role="alert">
              Ollama no responde. Puedes consultar el catálogo, pero no descargar, activar ni borrar modelos hasta que arranque.
            </div>
          ) : null}
          <Section title="Hardware detectado" hint="Información, no una recomendación" id="hw">
            <Hardware hw={q.data.hardware} />
          </Section>
          <Section title="Cargado en memoria" hint="Cuánto del modelo está de verdad en la tarjeta" id="loaded">
            <Loaded loaded={q.data.loaded} models={q.data.models} />
          </Section>
          {groups.map(({ tier, models }) => (
            <Section key={tier} title={TIER_INFO[tier].title} hint={TIER_INFO[tier].text} id={`tier-${tier}`}>
              <div className="model-grid">
                {models.map((m) => (
                  <ModelCard
                    key={m.id}
                    model={m}
                    hardware={q.data.hardware}
                    pull={pulls[m.id]}
                    busy={busy || !q.data.ollamaReachable}
                    onPull={onPull}
                    onCancel={cancel}
                    onDismiss={dismiss}
                    onActivate={onActivate}
                    onDelete={onDelete}
                    onForceGpu={onForceGpu}
                    loaded={q.data.loaded.find((l) => l.id === m.id)}
                  />
                ))}
              </div>
            </Section>
          ))}
        </>
      ) : null}
    </div>
  )
}
