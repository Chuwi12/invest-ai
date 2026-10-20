import { Brain, Check, Download, Trash2, Wrench, X } from 'lucide-react'
import { memo, useState } from 'react'
import type { PullState } from '@/api/hooks'
import type { HardwareInfo, LlmModel, LoadedModel } from '@/api/types'
import { formatBytes } from '@/lib/bytes'
import { modelFit } from '@/lib/modelFit'

const gb = (n: number) => `${n.toLocaleString('es-ES', { maximumFractionDigits: 1 })} GB`

const ctxLabel = (n: number) => (n <= 0 ? 'desconocido' : n >= 1000 ? `${Math.round(n / 1024)}k tokens` : `${n} tokens`)

interface Props {
  model: LlmModel
  hardware: HardwareInfo
  pull: PullState | undefined
  busy: boolean
  onPull: (id: string) => void
  onCancel: (id: string) => void
  onDismiss: (id: string) => void
  onActivate: (id: string) => void
  onDelete: (id: string) => void
  onForceGpu: (id: string, force: boolean) => void
  /** Si este modelo está cargado ahora en Ollama: su reparto real GPU/CPU (`/api/ps`). */
  loaded?: LoadedModel | undefined
}

export const ModelCard = memo(function ModelCard({ model: m, hardware, pull, busy, onPull, onCancel, onDismiss, onActivate, onDelete, onForceGpu, loaded }: Props) {
  const [confirming, setConfirming] = useState(false)
  const fit = modelFit(m, hardware)
  const r = m.requirements
  const pct = pull && pull.total && pull.completed !== null ? Math.min(100, (pull.completed / pull.total) * 100) : null

  return (
    <article className={`model ${m.active ? 'is-active' : ''}`} aria-label={m.name}>
      <header className="row wrap">
        <h3 className="grow">{m.name}</h3>
        {m.active ? (
          <span className="chip accent">
            <Check aria-hidden="true" /> Activo
          </span>
        ) : m.installed ? (
          <span className="chip">Instalado</span>
        ) : null}
      </header>

      <div className="row wrap">
        <span className="chip">{m.parameters}{m.activeParameters ? ` (${m.activeParameters} activos)` : ''}</span>
        <span className="chip">{m.quantization}</span>
        <span className="chip">Contexto {ctxLabel(m.contextWindow)}</span>
        <span className={`chip ${m.supportsTools ? '' : 'warn'}`}>
          <Wrench aria-hidden="true" /> {m.supportsTools ? 'Herramientas' : 'Sin herramientas'}
        </span>
        {m.reasoning ? (
          <span className="chip">
            <Brain aria-hidden="true" /> Razonamiento
          </span>
        ) : null}
        {!m.curated ? <span className="chip">Fuera del catálogo</span> : null}
        {loaded ? (
          <span
            className={`chip ${loaded.gpuShare < 0.995 ? 'warn' : ''}`}
            title={`Cargado en Ollama: ${gb(loaded.sizeVram / 2 ** 30)} en VRAM de ${gb(loaded.size / 2 ** 30)}`}
          >
            Cargado: {Math.round(loaded.gpuShare * 100)} % en GPU
          </span>
        ) : null}
      </div>

      <p className="small muted">{m.description}</p>

      {r ? (
        <dl className="specs">
          <div><dt>VRAM mínima: </dt><dd className="num">{gb(r.minVramGb)}</dd></div>
          <div><dt>VRAM recomendada: </dt><dd className="num">{gb(r.recommendedVramGb)}</dd></div>
          <div><dt>RAM mínima: </dt><dd className="num">{gb(r.minRamGb)}</dd></div>
          <div><dt>Disco: </dt><dd className="num">{gb(r.diskGb)}</dd></div>
          <div><dt>Solo CPU: </dt><dd>{r.cpuOnlyOk ? 'sí vale' : 'no recomendado'}</dd></div>
        </dl>
      ) : (
        <p className="small muted">Requisitos desconocidos.</p>
      )}

      <div className="fit" data-fit={fit.kind}>
        <span aria-hidden="true">{fit.kind === 'vram' ? '●' : fit.kind === 'no' ? '○' : '◐'}</span>
        <span>
          {fit.text}
          {fit.note ? <span className="down"> {fit.note}</span> : null}
        </span>
      </div>

      {m.installed ? (
        <div className="stack" style={{ gap: '0.25rem' }}>
          <label className="row small" style={{ gap: '0.4rem' }}>
            <input
              type="checkbox"
              checked={m.forceGpu}
              disabled={busy}
              onChange={(e) => onForceGpu(m.id, e.target.checked)}
            />
            <span>Forzar todas las capas en GPU</span>
          </label>
          <p className="small muted" style={{ margin: 0 }}>
            Por defecto decide Ollama y, si la tarjeta va justa, deja parte del modelo en la CPU (más lento). Forzarlo
            lo mantiene entero en la VRAM si cabe; deja menos memoria libre para el escritorio y, si falta, la app
            vuelve sola al reparto automático y te avisa.
          </p>
          {m.forceGpu && m.gpuFallback ? (
            <div className="notice warn" role="status">
              No había memoria para forzar la GPU; se usa el reparto automático (se reintenta en 30 min o al cambiar el ajuste). Ollama dijo: {m.gpuFallback}
            </div>
          ) : null}
        </div>
      ) : null}

      {pull ? (
        <div className="stack" style={{ gap: '0.3rem' }}>
          {pull.error ? (
            <div className="notice error" role="alert">
              <span className="grow">No se pudo descargar: {pull.error}</span>
              <button type="button" className="btn sm" onClick={() => onDismiss(m.id)}>Cerrar</button>
            </div>
          ) : (
            <>
              <div
                className="bar"
                role="progressbar"
                aria-label={`Descargando ${m.name}`}
                aria-valuemin={0}
                aria-valuemax={100}
                {...(pct !== null ? { 'aria-valuenow': Math.round(pct) } : {})}
              >
                {pct !== null ? <i style={{ width: `${pct}%` }} /> : <div className="indet" />}
              </div>
              <div className="row small muted">
                <span className="grow trunc">{pull.status}</span>
                {pct !== null && pull.total ? (
                  <span className="num">{Math.round(pct)} % · {formatBytes(pull.completed ?? 0)} / {formatBytes(pull.total)}</span>
                ) : null}
                <button type="button" className="btn sm" onClick={() => onCancel(m.id)}>
                  <X aria-hidden="true" /> Cancelar
                </button>
              </div>
            </>
          )}
        </div>
      ) : (
        <div className="row wrap">
          {!m.installed ? (
            <button type="button" className="btn sm primary" onClick={() => onPull(m.id)} disabled={busy}>
              <Download aria-hidden="true" /> Descargar{r ? ` (${gb(r.diskGb)})` : ''}
            </button>
          ) : (
            <>
              {!m.active ? (
                <button type="button" className="btn sm primary" onClick={() => onActivate(m.id)} disabled={busy}>
                  Activar
                </button>
              ) : null}
              {confirming ? (
                <>
                  <span className="small">¿Borrar del disco?</span>
                  <button type="button" className="btn sm danger" onClick={() => { setConfirming(false); onDelete(m.id) }}>
                    Sí, borrar
                  </button>
                  <button type="button" className="btn sm" onClick={() => setConfirming(false)}>No</button>
                </>
              ) : (
                <button
                  type="button"
                  className="btn sm danger"
                  onClick={() => setConfirming(true)}
                  disabled={busy || m.active}
                  title={m.active ? 'Activa otro modelo antes de borrar este' : undefined}
                >
                  <Trash2 aria-hidden="true" /> Borrar
                </button>
              )}
            </>
          )}
        </div>
      )}
    </article>
  )
})
