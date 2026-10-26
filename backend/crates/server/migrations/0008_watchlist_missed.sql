-- NO EDITAR una vez desplegada: sqlx guarda el checksum de cada migración aplicada y, si el fichero cambia,
-- el servidor no arranca. Cualquier ajuste va en una migración nueva.
--
-- La pasada diaria pasa a tener una VENTANA (entre el cierre de EE. UU. del día de bolsa D y la apertura
-- siguiente, ver docs/notes/learning.md): si llega la apertura sin haberla hecho, la pasada de D se
-- pierde y se registra con estado 'missed'. `local_date` pasa a ser ese día de bolsa D (antes, el día
-- local de ejecución); las filas anteriores se quedan como están.
--
-- SQLite no permite cambiar un CHECK de una tabla existente: se reconstruyen las dos (copiando todas las
-- filas con sus ids). Se copia primero a tablas nuevas y se borra el hijo (`watchlist_results`) ANTES
-- que el padre: con las claves ajenas activas, el DROP del padre borraría en cascada los resultados.
-- Al renombrar, SQLite reescribe la referencia de la tabla nueva de resultados a `watchlist_runs`.
-- El contador de AUTOINCREMENT (`sqlite_sequence`) puede ser mayor que `max(id)` si se borraron filas: se
-- traslada de la tabla antigua a la nueva (el DROP borraría el de la antigua), para no reutilizar ids.

CREATE TABLE watchlist_runs_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Día de bolsa (YYYY-MM-DD) al que cuenta la pasada: la ventana va del cierre de ese día a la apertura siguiente.
    local_date    TEXT    NOT NULL,
    -- Instante (ms UTC) preferido para ejecutarla (WATCHLIST_TIME dentro de la ventana).
    scheduled_for INTEGER NOT NULL,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    -- 'schedule' (a su hora), 'recovery' (el servidor arrancó con la hora preferida ya pasada) o 'retry' (otra ronda).
    trigger       TEXT    NOT NULL CHECK (trigger IN ('schedule', 'recovery', 'retry')),
    -- 'running' | 'completed' (todo intentado) | 'aborted' (Ollama/mercado caídos o apertura: se paró y se reintenta
    -- si queda ventana) | 'interrupted' (el servidor se apagó o cayó a mitad) | 'missed' (llegó la apertura sin
    -- haberla hecho: la pasada de ese día de bolsa se pierde y no se analiza con el mercado abierto).
    status        TEXT    NOT NULL CHECK (status IN ('running', 'completed', 'aborted', 'interrupted', 'missed')),
    depth         TEXT    NOT NULL,
    model         TEXT,
    planned       INTEGER NOT NULL,
    power         TEXT    NOT NULL DEFAULT 'unknown' CHECK (power IN ('ac', 'battery', 'unknown')),
    power_wait_secs INTEGER NOT NULL DEFAULT 0,
    ok            INTEGER NOT NULL DEFAULT 0,
    failed        INTEGER NOT NULL DEFAULT 0
);
INSERT INTO watchlist_runs_new
    (id, local_date, scheduled_for, started_at, finished_at, trigger, status, depth, model, planned, power, power_wait_secs, ok, failed)
SELECT id, local_date, scheduled_for, started_at, finished_at, trigger, status, depth, model, planned, power, power_wait_secs, ok, failed
  FROM watchlist_runs;

CREATE TABLE watchlist_results_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id      INTEGER NOT NULL REFERENCES watchlist_runs_new (id) ON DELETE CASCADE,
    -- Día de bolsa (el de su pasada).
    local_date  TEXT    NOT NULL,
    asset_id    TEXT    NOT NULL,
    -- 'ok' | 'failed' | 'skipped' (no se intentó: pasada abortada, o perdida si error_code = 'missed')
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
INSERT INTO watchlist_results_new
    (id, run_id, local_date, asset_id, status, transient, attempts, analysis_id, verdict, error_code, error, started_at, finished_at)
SELECT id, run_id, local_date, asset_id, status, transient, attempts, analysis_id, verdict, error_code, error, started_at, finished_at
  FROM watchlist_results;

-- Sin fila en `sqlite_sequence` (tabla que nunca tuvo filas) no hay nada que trasladar.
DELETE FROM sqlite_sequence WHERE name IN ('watchlist_runs_new', 'watchlist_results_new');
INSERT INTO sqlite_sequence (name, seq)
SELECT name || '_new', seq FROM sqlite_sequence WHERE name IN ('watchlist_runs', 'watchlist_results');

DROP TABLE watchlist_results;
DROP TABLE watchlist_runs;
ALTER TABLE watchlist_runs_new RENAME TO watchlist_runs;
ALTER TABLE watchlist_results_new RENAME TO watchlist_results;

CREATE INDEX idx_watchlist_runs_date ON watchlist_runs (local_date, id);
CREATE INDEX idx_watchlist_results_run ON watchlist_results (run_id);
CREATE INDEX idx_watchlist_results_day ON watchlist_results (local_date, asset_id);
