-- Esquema inicial. Convenciones:
--  * ids de entidades propias: uuid v7 en texto (ordenables por fecha de creación);
--  * fechas: milisegundos Unix (INTEGER) en UTC; ordenan bien y no dependen de formatos de texto;
--  * JSON en columnas TEXT (se serializa/deserializa con los tipos de `domain`).

CREATE TABLE conversations (
    id          TEXT PRIMARY KEY,
    -- '' = sin título: se genera a partir del primer mensaje del usuario.
    title       TEXT NOT NULL DEFAULT '',
    -- JSON: array de AssetId ("stock:AAPL").
    asset_ids   TEXT NOT NULL DEFAULT '[]',
    pinned      INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX idx_conversations_updated ON conversations (updated_at DESC);

CREATE TABLE messages (
    id               TEXT PRIMARY KEY,
    conversation_id  TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    role             TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content          TEXT NOT NULL,
    -- JSON: ToolCallRecord[]
    tool_calls       TEXT NOT NULL DEFAULT '[]',
    -- JSON: Analysis o NULL
    analysis         TEXT,
    model            TEXT,
    prompt_tokens    INTEGER,
    completion_tokens INTEGER,
    created_at       INTEGER NOT NULL
);
-- Cubre el historial de una conversación en orden y las subconsultas de contador/vista previa.
CREATE INDEX idx_messages_conversation ON messages (conversation_id, created_at, id);

CREATE TABLE favorites (
    asset_id  TEXT PRIMARY KEY,
    note      TEXT,
    -- JSON: AssetSummary (así el listado no depende del proveedor).
    summary   TEXT NOT NULL,
    added_at  INTEGER NOT NULL
);
CREATE INDEX idx_favorites_added ON favorites (added_at DESC);

CREATE TABLE search_history (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    query     TEXT NOT NULL,
    selected  TEXT,
    at        INTEGER NOT NULL
);
CREATE INDEX idx_search_history_at ON search_history (at DESC);

CREATE TABLE asset_views (
    asset_id         TEXT PRIMARY KEY,
    -- Contador total de visitas.
    views            INTEGER NOT NULL DEFAULT 0,
    -- Visitas con decaimiento exponencial (vida media de 7 días) hasta `last_viewed_at`.
    -- Para ordenar hay que decaerlo hasta «ahora»: score * 0.5^((now - last) / vida_media).
    score            REAL NOT NULL DEFAULT 0,
    first_viewed_at  INTEGER NOT NULL,
    last_viewed_at   INTEGER NOT NULL,
    -- JSON: AssetSummary (evita volver a pedirla al proveedor).
    summary          TEXT NOT NULL
);
CREATE INDEX idx_asset_views_last ON asset_views (last_viewed_at DESC);

CREATE TABLE analyses (
    id                 TEXT PRIMARY KEY,
    asset_id           TEXT NOT NULL,
    verdict            TEXT NOT NULL,
    confidence         REAL NOT NULL,
    price_at_analysis  REAL,
    model              TEXT NOT NULL,
    -- 'analyze' (POST /assets/{id}/analysis) o 'chat' (emitido por el asistente).
    origin             TEXT NOT NULL DEFAULT 'analyze',
    generated_at       INTEGER NOT NULL,
    -- JSON: Analysis completo.
    json               TEXT NOT NULL
);
CREATE INDEX idx_analyses_asset_date ON analyses (asset_id, generated_at DESC);
CREATE INDEX idx_analyses_date ON analyses (generated_at DESC);

CREATE TABLE asset_cache (
    asset_id    TEXT PRIMARY KEY,
    -- JSON: AssetSummary
    summary     TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);
