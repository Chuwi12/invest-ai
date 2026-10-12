import { Brain, Check, Download, Trash2, Wrench, X } from 'lucide-react'
import { memo, useState } from 'react'
import type { PullState } from '@/api/hooks'
import type { HardwareInfo, LlmModel } from '@/api/types'
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
}

export const ModelCard = memo(function ModelCard({ model: m, hardware, pull, busy, onPull, onCancel, onDismiss, onActivate, onDelete }: Props) {
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
