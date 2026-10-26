-- Subir `RULE_VERSION` no puede hacer perder cifras ya medidas (crate `learning`, docs/notes/learning.md).
--
--  * `analysis_outcomes_archive`: foto de las cifras de una fila justo antes de que una versión nueva de la
--    regla las sustituya (una fila por análisis, horizonte y versión: se conserva toda la cadena v1 -> v2 -> v3).
--    Se escribe en la misma transacción que la sustitución, así que nunca hay una sin la otra. Elegida frente
--    a columnas `prev_*` porque guarda más de una generación y se escribe con un solo INSERT ... SELECT.
--  * `analysis_outcomes.fallback_for`: vía de reserva. Si la regla nueva no se puede calcular de forma
--    permanente (activo sin histórico o deslistado), la fila conserva las cifras de la versión anterior
--    (`rule_version` sigue siendo la de esas cifras) y esta columna dice para QUÉ versión vigente se usan en su
--    lugar. NULL = fila normal. Las métricas cuentan `rule_version = vigente OR fallback_for = vigente`.
--  * `scoring_weights.rule_version`: regla con la que se ajustaron los pesos. Las filas anteriores a esta
--    migración se dan por ajustadas con la v1: las escribió un binario anterior a la 0008, que aún tenía RULE_VERSION 1.
--  * `learning_runs.decision` es texto libre: la desactivación de pesos de otra versión se anota con 'deactivate'.

ALTER TABLE analysis_outcomes ADD COLUMN fallback_for INTEGER;

CREATE TABLE analysis_outcomes_archive (
    analysis_id        TEXT    NOT NULL,
    horizon_weeks      INTEGER NOT NULL,
    -- Versión de la regla con la que se calcularon estas cifras.
    rule_version       INTEGER NOT NULL,
    -- Versión que las sustituyó y cuándo (ms UTC).
    superseded_by      INTEGER NOT NULL,
    archived_at        INTEGER NOT NULL,
    -- Si estas cifras ya estaban en la vía de reserva (para qué versión vigente).
    fallback_for       INTEGER,
    bench_id           TEXT,
    bench_kind         TEXT,
    entry_ts           INTEGER,
    exit_ts            INTEGER,
    entry_price        REAL,
    exit_price         REAL,
    bench_entry_price  REAL,
    bench_exit_price   REAL,
    asset_return       REAL,
    bench_return       REAL,
    excess_return      REAL,
    hit                INTEGER,
    factors            TEXT,
    overall            REAL,
    resolved_at        INTEGER NOT NULL,
    PRIMARY KEY (analysis_id, horizon_weeks, rule_version)
);

ALTER TABLE scoring_weights ADD COLUMN rule_version INTEGER NOT NULL DEFAULT 1;
