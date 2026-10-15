-- Archivo de conocimiento local (RAG): documentos, fragmentos y embeddings. Ver docs/notes/rag.md.
-- Las fechas son milisegundos Unix (UTC), como en el resto del esquema. Los embeddings son datos
-- DERIVADOS (se pueden regenerar desde `chunks.text`); el texto no.

CREATE TABLE documents (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Identidad lógica estable: «news:<id>», «filing:<accession>», «analysis:<uuid>».
    doc_key       TEXT NOT NULL UNIQUE,
    -- SHA-256 hex del contenido normalizado (ver `NewDocument::content_hash`): deduplica entre fuentes.
    content_hash  TEXT NOT NULL UNIQUE,
    kind          TEXT NOT NULL CHECK (kind IN ('news', 'filing', 'analysis')),
    -- Formulario SEC (S-1, F-1, 424B4…) u otro subtipo.
    subtype       TEXT,
    source        TEXT NOT NULL,
    publisher     TEXT,
    title         TEXT NOT NULL,
    url           TEXT,
    issuer        TEXT,
    language      TEXT,
    published_at  INTEGER NOT NULL,
    ingested_at   INTEGER NOT NULL,
    chunk_count   INTEGER NOT NULL DEFAULT 0,
    -- 1 = no se purga por antigüedad ni por tope (p. ej. citado por un análisis guardado).
    pinned        INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    -- JSON libre (tickers, resultado de un análisis…). No entra en el hash.
    meta          TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_documents_kind_pub ON documents (kind, published_at);

CREATE TABLE document_assets (
    document_id  INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    asset_id     TEXT NOT NULL,
    PRIMARY KEY (document_id, asset_id)
);
CREATE INDEX idx_document_assets_asset ON document_assets (asset_id);

CREATE TABLE chunks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id  INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    ordinal      INTEGER NOT NULL,
    section      TEXT,
    text         TEXT NOT NULL,
    -- SHA-256 truncado (32 hex) del texto normalizado: reutiliza embeddings y colapsa duplicados.
    text_hash    TEXT NOT NULL,
    token_count  INTEGER NOT NULL,
    UNIQUE (document_id, ordinal)
);
CREATE INDEX idx_chunks_text_hash ON chunks (text_hash);

CREATE TABLE embeddings (
    chunk_id    INTEGER NOT NULL REFERENCES chunks (id) ON DELETE CASCADE,
    -- Identificador del espacio vectorial (modelo + estilo de prompt + dimensiones).
    model       TEXT NOT NULL,
    dim         INTEGER NOT NULL,
    -- f32 little-endian, normalizado L2.
    vec         BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (chunk_id, model)
);
