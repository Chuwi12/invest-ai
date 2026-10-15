-- Aprendizaje por resultados (crate `learning`). Ver docs/notes/learning.md.
--
-- Decisiones:
--  * `analysis_outcomes` NO tiene clave foránea hacia `analyses`: el registro de aciertos debe
--    sobrevivir a que el usuario borre un análisis (si no, borrar los fallos sesgaría el historial:
--    sesgo de supervivencia). Guarda desnormalizado lo que necesita para las estadísticas.
--  * Una fila por (análisis, horizonte en semanas). `resolved` y `skipped` son definitivas;
--    `retry` se reintenta a partir de `retry_after`.
--  * `rule_version` congela la regla de acierto con la que se etiquetó (a priori, no se reescribe).

CREATE TABLE analysis_outcomes (
    analysis_id        TEXT    NOT NULL,
    horizon_weeks      INTEGER NOT NULL CHECK (horizon_weeks > 0),
    asset_id           TEXT    NOT NULL,
    asset_class        TEXT    NOT NULL,
    verdict            TEXT    NOT NULL,
    confidence         REAL    NOT NULL,
    model              TEXT    NOT NULL,
    generated_at       INTEGER NOT NULL,
    status             TEXT    NOT NULL CHECK (status IN ('resolved', 'skipped', 'retry')),
    note               TEXT,
    attempts           INTEGER NOT NULL DEFAULT 0,
    retry_after        INTEGER,
    rule_version       INTEGER NOT NULL,
    bench_id           TEXT,
    -- 'relative' = exceso sobre el índice; 'absolute' = el activo es su propio índice (SPY, BTC): exceso = rentabilidad.
    bench_kind         TEXT CHECK (bench_kind IN ('relative', 'absolute')),
    entry_ts           INTEGER,
    exit_ts            INTEGER,
    entry_price        REAL,
    exit_price         REAL,
    bench_entry_price  REAL,
    bench_exit_price   REAL,
    asset_return       REAL,
    bench_return       REAL,
    excess_return      REAL,
    hit                INTEGER CHECK (hit IN (0, 1)),
    -- JSON: 7 factores en orientación «favorable» (riesgo invertido), null si faltaban.
    factors            TEXT,
    overall            REAL,
    resolved_at        INTEGER NOT NULL,
    PRIMARY KEY (analysis_id, horizon_weeks)
);
CREATE INDEX idx_outcomes_status ON analysis_outcomes (status, horizon_weeks, generated_at);

-- Versiones de los multiplicadores de peso de la puntuación. Nunca se borran filas.
-- Sin ninguna fila activa rigen los pesos por defecto del código.
CREATE TABLE scoring_weights (
    version         INTEGER PRIMARY KEY,
    kind            TEXT    NOT NULL CHECK (kind IN ('learned', 'rollback', 'decay', 'manual')),
    status          TEXT    NOT NULL CHECK (status IN ('active', 'retired')),
    -- JSON: 7 multiplicadores [técnico, momentum, valoración, fundamentales, sentimiento, macro, riesgo].
    multipliers     TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    horizon_weeks   INTEGER,
    trained_to      INTEGER,
    validated_from  INTEGER,
    validated_to    INTEGER,
    n_train         INTEGER,
    n_val           INTEGER,
    -- JSON: métricas de entrenamiento y validación (IC, p-valor, margen…).
    metrics         TEXT    NOT NULL DEFAULT '{}',
    parent_version  INTEGER,
    note            TEXT
);
CREATE UNIQUE INDEX idx_scoring_weights_one_active ON scoring_weights (status) WHERE status = 'active';

-- Registro de cada evaluación de la puerta (también las rechazadas): sirve para vigilar el
-- exceso de intentos (múltiples comparaciones) y para reproducir decisiones.
CREATE TABLE learning_runs (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ran_at         INTEGER NOT NULL,
    horizon_weeks  INTEGER NOT NULL,
    decision       TEXT    NOT NULL,
    reason         TEXT    NOT NULL,
    n_rows         INTEGER NOT NULL,
    version        INTEGER,
    metrics        TEXT    NOT NULL DEFAULT '{}'
);
