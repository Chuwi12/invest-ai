-- Redes sociales (Reddit y X): opinión, no hechos. Ver docs/notes/social.md.
--
-- Todo se guarda con su FECHA REAL (`created_at` de la publicación, `fetched_at`/`as_of` de la
-- consulta) para que el crate `learning` pueda evaluar algún día, sin anticipación, si el tono o
-- los picos de atención predicen algo. Estas tablas NO alimentan la puntuación ni los pesos.
-- Sin credenciales no se escribe nada aquí.

-- Publicaciones recogidas por activo. `score` y `comments` se actualizan en consultas posteriores
-- (el valor guardado es el último visto, no el del momento de publicar). Retención corta.
CREATE TABLE social_posts (
    asset_id      TEXT    NOT NULL,
    post_id       TEXT    NOT NULL,          -- 'reddit:abc123' | 'x:180000...'
    source        TEXT    NOT NULL CHECK (source IN ('reddit', 'x')),
    community     TEXT    NOT NULL,
    author        TEXT    NOT NULL,          -- seudónimo (hash truncado); vacío si el autor está borrado
    title         TEXT,
    body          TEXT    NOT NULL,
    score         INTEGER NOT NULL,
    comments      INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,          -- ms UTC, fecha real de la publicación
    url           TEXT    NOT NULL,
    first_seen_at INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    PRIMARY KEY (asset_id, post_id)
);
CREATE INDEX social_posts_asset_time ON social_posts (asset_id, created_at);

-- Una fila por consulta a una red y activo. Dice qué periodos están REALMENTE observados: un día
-- sin consulta no es «cero menciones». `ok = 0` guarda también los fallos (último error en la ruta de estado).
CREATE TABLE social_runs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    asset_id     TEXT    NOT NULL,
    source       TEXT    NOT NULL CHECK (source IN ('reddit', 'x')),
    fetched_at   INTEGER NOT NULL,
    window_start INTEGER NOT NULL,
    ok           INTEGER NOT NULL,
    posts        INTEGER NOT NULL DEFAULT 0,
    reads        INTEGER NOT NULL DEFAULT 0,  -- posts leídos facturables (X)
    truncated    INTEGER NOT NULL DEFAULT 0,
    error        TEXT
);
CREATE INDEX social_runs_asset ON social_runs (asset_id, source, fetched_at);

-- Instantáneas de métricas, solo se AÑADEN (nunca se actualizan): cada una es lo que se sabía en `as_of`.
CREATE TABLE social_metrics (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    asset_id         TEXT    NOT NULL,
    as_of            INTEGER NOT NULL,
    window_days      INTEGER NOT NULL,
    mentions         INTEGER NOT NULL,        -- últimas 24 h
    authors          INTEGER NOT NULL,
    bullish          INTEGER NOT NULL,
    bearish          INTEGER NOT NULL,
    neutral          INTEGER NOT NULL,
    mentions_window  INTEGER NOT NULL,
    bullish_window   INTEGER NOT NULL,
    bearish_window   INTEGER NOT NULL,
    baseline_mean    REAL,
    baseline_days    INTEGER NOT NULL,
    ratio            REAL,
    z                REAL,
    truncated        INTEGER NOT NULL,
    by_source        TEXT    NOT NULL,        -- JSON [{source, mentions}]
    method           TEXT    NOT NULL DEFAULT 'lexicon-v1'
);
CREATE INDEX social_metrics_asset ON social_metrics (asset_id, as_of);

-- Posts de X leídos por mes (presupuesto). Persistente: sobrevive a reinicios.
CREATE TABLE social_x_budget (
    month      TEXT    PRIMARY KEY,           -- 'YYYY-MM' (UTC)
    reads      INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
