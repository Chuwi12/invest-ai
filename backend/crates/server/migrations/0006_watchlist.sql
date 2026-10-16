-- Análisis diarios de la lista de seguimiento (ver docs/notes/learning.md, «Lista de seguimiento»).
--
-- `analyses.origin` ya admite texto libre: desde esta versión también vale 'watchlist'.
--
-- Una fila en `watchlist_runs` por pasada (programada, recuperada tras el arranque o reintento del
-- mismo día) y una en `watchlist_results` por activo intentado en ella. Nada se borra: un fallo
-- queda registrado con su motivo, y el historial dice con qué retraso real se hizo cada pasada.
CREATE TABLE watchlist_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Día local (YYYY-MM-DD) al que cuenta la pasada.
    local_date    TEXT    NOT NULL,
    -- Instante (ms UTC) en que tocaba ejecutarla según WATCHLIST_TIME.
    scheduled_for INTEGER NOT NULL,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    -- 'schedule' (a su hora), 'recovery' (el servidor arrancó tarde) o 'retry' (otra ronda del mismo día).
    trigger       TEXT    NOT NULL CHECK (trigger IN ('schedule', 'recovery', 'retry')),
    -- 'running' | 'completed' (todo intentado) | 'aborted' (Ollama/mercado caídos: se paró y se reintenta)
    -- | 'interrupted' (el servidor se apagó o cayó a mitad).
    status        TEXT    NOT NULL CHECK (status IN ('running', 'completed', 'aborted', 'interrupted')),
    depth         TEXT    NOT NULL,
    model         TEXT,
    planned       INTEGER NOT NULL,
    -- Fuente de energía al empezar: 'ac' (enchufado), 'battery' (con batería; se aplaza la pasada hasta
    -- WATCHLIST_BATTERY_MAX_DELAY_HOURS) o 'unknown' (no se pudo leer). `power_wait_secs`: lo que se esperó por ello.
    power         TEXT    NOT NULL DEFAULT 'unknown' CHECK (power IN ('ac', 'battery', 'unknown')),
    power_wait_secs INTEGER NOT NULL DEFAULT 0,
    ok            INTEGER NOT NULL DEFAULT 0,
    failed        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_watchlist_runs_date ON watchlist_runs (local_date, id);

CREATE TABLE watchlist_results (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id      INTEGER NOT NULL REFERENCES watchlist_runs (id) ON DELETE CASCADE,
    local_date  TEXT    NOT NULL,
    asset_id    TEXT    NOT NULL,
    -- 'ok' | 'failed' | 'skipped' (no se intentó porque la pasada se abortó)
    status      TEXT    NOT NULL CHECK (status IN ('ok', 'failed', 'skipped')),
    -- ¿Merece otro intento ese día? (fallos de Ollama/red/límites sí; activo inexistente no.)
    transient   INTEGER NOT NULL DEFAULT 0,
    attempts    INTEGER NOT NULL DEFAULT 0,
    analysis_id TEXT,
    verdict     TEXT,
    error_code  TEXT,
    error       TEXT,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER NOT NULL
);
CREATE INDEX idx_watchlist_results_run ON watchlist_results (run_id);
CREATE INDEX idx_watchlist_results_day ON watchlist_results (local_date, asset_id);
