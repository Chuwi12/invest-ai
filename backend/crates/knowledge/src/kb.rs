//! `KnowledgeBase`: ingesta, búsqueda, retención y estadísticas sobre SQLite + índice en memoria.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use domain::{AssetId, NewsItem};
use sqlx::SqlitePool;

use crate::{
    chunk::chunk_sections,
    config::KnowledgeConfig,
    embed::{EmbedKind, Embedder, normalize_l2},
    error::{EmbedError, KnowledgeError, Result},
    index::{DocMeta, Filter, VectorIndex},
    lexical::{fts_query, rrf_max, rrf_term},
    model::*,
    store::{self, ChunkRow, InsertDoc},
    text::{fold_key, hash128, hash128_hex, looks_like_injection, normalize_text, parse_hash128},
};

/// Tras un fallo transitorio del embedder no se vuelve a intentar durante este tiempo.
const EMBED_BACKOFF: Duration = Duration::from_secs(30);
/// Si el modelo de embeddings no está instalado no hay nada que reintentar hasta que el usuario lo descargue
/// (`ollama pull`): pausa más larga, para que el archivado por defecto no llame a Ollama con cada documento.
const EMBED_BACKOFF_MODEL_MISSING: Duration = Duration::from_secs(300);
/// Una respuesta inválida (4xx que no es 404, un 200 que no es JSON, dimensiones que no cuadran con
/// `RAG_EMBED_DIMS`, vectores con NaN) no se arregla sola: se repetiría con cada documento y en cada pasada
/// de mantenimiento. Misma pausa larga que con el modelo ausente.
const EMBED_BACKOFF_BAD: Duration = Duration::from_secs(300);

/// Cuánto se deja en paz al embedder tras este error.
fn embed_pause(e: &EmbedError) -> Duration {
    match e {
        EmbedError::Unavailable(_) => EMBED_BACKOFF,
        EmbedError::ModelMissing(_) => EMBED_BACKOFF_MODEL_MISSING,
        EmbedError::Bad(_) => EMBED_BACKOFF_BAD,
    }
}

/// Valida lo que devolvió el embedder: un vector por texto, todos de la misma dimensión, finitos y
/// normalizados a norma 1.
fn checked_vectors(mut vs: Vec<Vec<f32>>, expected: usize) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
    if vs.len() != expected {
        return Err(EmbedError::Bad("número de vectores incorrecto".into()));
    }
    let dim = vs.first().map_or(0, Vec::len);
    for v in &mut vs {
        if v.len() != dim {
            return Err(EmbedError::Bad("dimensiones desiguales".into()));
        }
        normalize_l2(v)?;
    }
    Ok(vs)
}

/// Búsqueda de conocimiento. Contrato mínimo que puede consumir `ai` (o moverse a `domain`).
#[async_trait]
pub trait KnowledgeSearch: Send + Sync + 'static {
    async fn search(&self, req: &SearchRequest) -> Result<SearchResults>;
}

pub struct KnowledgeBase {
    pool: SqlitePool,
    embedder: Arc<dyn Embedder>,
    model: String,
    cfg: KnowledgeConfig,
    index: Arc<RwLock<VectorIndex>>,
    write_gate: tokio::sync::Mutex<()>,
    backoff_until: Mutex<Option<Instant>>,
    /// La tabla FTS5 existe y funciona (se pone a `false` si algo falla: se degrada a solo vectores).
    fts: AtomicBool,
    /// El último intento de calcular embeddings falló: sirve para avisar UNA vez por caída (y otra al recuperarse).
    embedder_down: AtomicBool,
}

fn meta_of(d: &DocumentInfo) -> DocMeta {
    DocMeta {
        id: d.id,
        kind: d.kind,
        source: d.source.clone(),
        issuer_key: d.issuer.as_deref().map(fold_key).filter(|s| !s.is_empty()),
        published_ms: d.published_at.timestamp_millis(),
        assets: d.assets.clone(),
    }
}

/// Texto que se embebe: los folletos llevan «Título — Sección» delante (el fragmento solo, sin
/// el nombre del emisor, se recupera mal). En noticias el título ya es la primera línea.
fn embed_text(kind: DocKind, title: &str, section: Option<&str>, text: &str) -> String {
    if kind == DocKind::News {
        return text.to_string();
    }
    match section {
        Some(s) if !s.is_empty() => format!("{title} — {s}\n{text}"),
        _ => format!("{title}\n{text}"),
    }
}

fn recency(kind: DocKind, age_days: f64) -> f32 {
    let half_life = match kind {
        DocKind::News => 14.0,
        DocKind::Filing => 365.0,
        DocKind::Analysis => 90.0,
    };
    0.5f64.powf(age_days.max(0.0) / half_life) as f32
}

impl KnowledgeBase {
    /// Abre la base, comprueba el esquema y carga el índice del modelo activo en memoria.
    pub async fn open(pool: SqlitePool, embedder: Arc<dyn Embedder>, cfg: KnowledgeConfig) -> Result<Arc<Self>> {
        store::check_schema(&pool).await?;
        let model = embedder.model_id();
        let mut index = VectorIndex::new();
        for d in store::load_documents(&pool).await? {
            index.upsert_doc(meta_of(&d));
        }
        let mut skipped = 0usize;
        for v in store::load_vectors(&pool, &model).await? {
            if index.add(v.chunk_id, v.document_id, v.text_hash, &v.vec).is_err() {
                skipped += 1;
            }
        }
        if skipped > 0 {
            tracing::warn!(skipped, "vectores descartados al cargar el índice (dimensión o documento incoherente)");
        }
        // Búsqueda léxica: perezosa y opcional. Un fallo NUNCA impide abrir el archivo.
        let fts = cfg.hybrid
            && match store::fts_init(&pool).await {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(error = %e, "FTS5 no disponible: el archivo funciona solo con vectores");
                    false
                }
            };
        tracing::info!(model = %model, vectors = index.len(), docs = index.doc_count(), lexical = fts, "archivo de conocimiento cargado");
        let kb = Arc::new(Self {
            pool,
            embedder,
            model,
            cfg,
            index: Arc::new(RwLock::new(index)),
            write_gate: tokio::sync::Mutex::new(()),
            backoff_until: Mutex::new(None),
            fts: AtomicBool::new(fts),
            embedder_down: AtomicBool::new(false),
        });
        if fts {
            // Los fragmentos anteriores a la tabla se indexan en segundo plano y en tandas.
            let bg = kb.clone();
            tokio::spawn(async move {
                match bg.backfill_lexical().await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(n, "fragmentos añadidos al índice léxico"),
                    Err(e) => tracing::warn!(error = %e, "no se pudo completar el índice léxico"),
                }
            });
        }
        Ok(kb)
    }

    #[doc(hidden)]
    pub fn pool_for_tests(&self) -> &SqlitePool {
        &self.pool
    }

    /// ¿La búsqueda léxica está activa?
    pub fn lexical_enabled(&self) -> bool {
        self.fts.load(Ordering::Relaxed)
    }

    fn disable_fts(&self, e: &dyn std::fmt::Display) {
        if self.fts.swap(false, Ordering::Relaxed) {
            tracing::warn!(error = %e, "FTS5 falló: se degrada a solo vectores hasta reiniciar");
        }
    }

    /// Indexa en la tabla FTS5 los fragmentos que aún no están (primera vez o tras un fallo), en
    /// tandas de 500 y cediendo el paso a las escrituras entre tandas. Idempotente. Devuelve cuántos reindexó.
    pub async fn backfill_lexical(&self) -> Result<usize> {
        if !self.lexical_enabled() {
            return Ok(0);
        }
        if store::fts_count(&self.pool).await? == store::total_chunks(&self.pool).await? {
            return Ok(0);
        }
        let (mut after, mut done) = (0i64, 0usize);
        loop {
            let _gate = self.write_gate.lock().await;
            let rows = store::chunks_after(&self.pool, after, 500).await?;
            let Some(last) = rows.last().map(|r| r.chunk_id) else { break };
            if let Err(e) = store::fts_reindex(&self.pool, &rows).await {
                self.disable_fts(&e);
                return Err(e);
            }
            after = last;
            done += rows.len();
            drop(_gate);
            tokio::task::yield_now().await;
        }
        Ok(done)
    }

    pub fn config(&self) -> &KnowledgeConfig {
        &self.cfg
    }

    pub fn model_id(&self) -> &str {
        &self.model
    }

    fn index_read(&self) -> std::sync::RwLockReadGuard<'_, VectorIndex> {
        self.index.read().unwrap_or_else(|e| e.into_inner())
    }

    fn index_write(&self) -> std::sync::RwLockWriteGuard<'_, VectorIndex> {
        self.index.write().unwrap_or_else(|e| e.into_inner())
    }

    /// ¿El embedder está en pausa por un fallo reciente? (No se le llama hasta que pase.)
    fn embedder_paused(&self) -> bool {
        self.backoff_until.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|until| Instant::now() < until)
    }

    async fn embed_texts(&self, texts: &[String], kind: EmbedKind) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        if self.embedder_paused() {
            return Err(EmbedError::Unavailable("embedder en pausa tras un fallo reciente".into()));
        }
        // La validación va dentro del mismo `Result`: una respuesta incoherente cuenta como un fallo más
        // (aviso único y pausa), no como un error que escapa sin más.
        let res = match self.embedder.embed(texts, kind).await {
            Ok(vs) => checked_vectors(vs, texts.len()),
            Err(e) => Err(e),
        };
        match res {
            Ok(vs) => {
                *self.backoff_until.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if self.embedder_down.swap(false, Ordering::Relaxed) {
                    tracing::info!("embeddings disponibles otra vez: se completan los fragmentos pendientes");
                }
                Ok(vs)
            }
            Err(e) => {
                // Un solo aviso por caída (con Ollama parado el archivado sigue guardando texto y reintenta cada pausa).
                if !self.embedder_down.swap(true, Ordering::Relaxed) {
                    tracing::warn!(error = %e, "embeddings no disponibles: el texto se sigue archivando y los vectores quedan pendientes (aviso único por caída)");
                }
                *self.backoff_until.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() + embed_pause(&e));
                Err(e)
            }
        }
    }

    // ---------------------------------------------------------------- ingesta

    pub async fn ingest(&self, doc: NewDocument) -> Result<IngestReport> {
        let key = doc.doc_key.trim();
        if key.is_empty() || key.len() > 300 {
            return Err(KnowledgeError::Invalid("doc_key vacía o demasiado larga".into()));
        }
        if doc.title.trim().is_empty() {
            return Err(KnowledgeError::Invalid("el documento no tiene título".into()));
        }
        let bytes: usize = doc.sections.iter().map(|s| s.text.len()).sum();
        if bytes > self.cfg.max_doc_bytes {
            return Err(KnowledgeError::Invalid(format!("documento de {bytes} bytes; tope {}", self.cfg.max_doc_bytes)));
        }
        let mut assets: Vec<String> = doc.assets.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty() && a.len() <= 64).collect();
        assets.sort();
        assets.dedup();
        assets.truncate(20);

        let _gate = self.write_gate.lock().await;
        let hash = doc.content_hash();
        let mut replacing = None;
        if let Some(ex) = store::find_by_key(&self.pool, key).await? {
            if ex.content_hash == hash {
                return self.merge_assets(ex.id, &assets, IngestOutcome::Unchanged).await;
            }
            replacing = Some(ex.id);
        }
        if let Some(dup) = store::find_by_hash(&self.pool, &hash).await?
            && Some(dup) != replacing
        {
            return self.merge_assets(dup, &assets, IngestOutcome::Duplicate { of: dup }).await;
        }

        let mut drafts = chunk_sections(&doc.sections, &self.cfg.chunk);
        let truncated = drafts.len() > self.cfg.max_chunks_per_doc;
        drafts.truncate(self.cfg.max_chunks_per_doc);
        if drafts.is_empty() {
            return Err(KnowledgeError::Invalid("el documento no tiene texto".into()));
        }
        let suspicious = doc.sections.iter().any(|s| looks_like_injection(&s.text)) || looks_like_injection(&doc.title);
        if suspicious {
            tracing::warn!(doc_key = key, source = %doc.source, "documento con órdenes al modelo: se guarda como dato, sin alterar");
        }

        // Embeddings: primero los ya calculados para el mismo texto; luego el resto, fuera de la transacción.
        let hashes: Vec<String> = drafts.iter().map(|d| hash128_hex(&d.text)).collect();
        let mut vectors: HashMap<String, Vec<f32>> = store::embeddings_by_text_hash(&self.pool, &self.model, &hashes).await?;
        let reused = hashes.iter().filter(|h| vectors.contains_key(*h)).count();
        let mut todo: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<&String> = HashSet::new();
        for (d, h) in drafts.iter().zip(&hashes) {
            if !vectors.contains_key(h) && seen.insert(h) {
                todo.push((h.clone(), embed_text(doc.kind, &doc.title, d.section.as_deref(), &d.text)));
            }
        }
        let mut embedded = 0usize;
        if !todo.is_empty() {
            let texts: Vec<String> = todo.iter().map(|t| t.1.clone()).collect();
            match self.embed_texts(&texts, EmbedKind::Document).await {
                Ok(vs) => {
                    for ((h, _), v) in todo.iter().zip(vs) {
                        vectors.insert(h.clone(), v);
                    }
                    embedded = hashes.iter().filter(|h| todo.iter().any(|t| &t.0 == *h)).count();
                }
                // El aviso (una vez por caída) lo da `embed_texts`; por documento solo queda rastro a nivel `debug`.
                Err(e) => tracing::debug!(error = %e, doc_key = key, "sin embeddings: los fragmentos quedan pendientes"),
            }
        }

        let now_ms = Utc::now().timestamp_millis();
        let meta = serde_json::to_string(&doc.meta).unwrap_or_else(|_| "{}".into());
        let mut tx = self.pool.begin().await?;
        if let Some(old) = replacing {
            store::delete_documents(&mut tx, &[old], self.lexical_enabled()).await?;
        }
        let id = store::insert_document(
            &mut tx,
            &InsertDoc {
                doc_key: key,
                content_hash: &hash,
                kind: doc.kind,
                subtype: doc.subtype.as_deref(),
                source: &doc.source,
                publisher: doc.publisher.as_deref(),
                title: doc.title.trim(),
                url: doc.url.as_deref(),
                issuer: doc.issuer.as_deref(),
                language: doc.language.as_deref(),
                published_ms: doc.published_at.timestamp_millis(),
                ingested_ms: now_ms,
                chunk_count: drafts.len() as i64,
                meta: &meta,
            },
        )
        .await?;
        store::add_assets(&mut tx, id, &assets).await?;
        let mut new_rows: Vec<(i64, u128, &Vec<f32>)> = Vec::new();
        let mut pending = 0usize;
        let mut fts = self.lexical_enabled();
        for (d, h) in drafts.iter().zip(&hashes) {
            let cid = store::insert_chunk(
                &mut tx,
                id,
                &ChunkRow { ordinal: d.ordinal as i64, section: d.section.clone(), text: d.text.clone(), text_hash: h.clone(), tokens: d.tokens as i64 },
            )
            .await?;
            if fts && let Err(e) = store::fts_insert(&mut tx, cid, &d.text, doc.title.trim(), d.section.as_deref()).await {
                // Un fallo de FTS5 no debe perder el documento: se sigue sin índice léxico.
                self.disable_fts(&e);
                fts = false;
            }
            match vectors.get(h) {
                Some(v) => {
                    store::insert_embedding(&mut tx, cid, &self.model, v, now_ms).await?;
                    new_rows.push((cid, hash128(&d.text), v));
                }
                None => pending += 1,
            }
        }
        tx.commit().await?;

        let info = DocMeta {
            id,
            kind: doc.kind,
            source: doc.source.clone(),
            issuer_key: doc.issuer.as_deref().map(fold_key).filter(|s| !s.is_empty()),
            published_ms: doc.published_at.timestamp_millis(),
            assets,
        };
        {
            let mut ix = self.index_write();
            if let Some(old) = replacing {
                ix.remove_docs(&HashSet::from([old]));
            }
            ix.upsert_doc(info);
            for (cid, th, v) in new_rows {
                if let Err(e) = ix.add(cid, id, th, v) {
                    tracing::warn!(error = %e, "vector fuera del índice en memoria");
                }
            }
        }
        Ok(IngestReport {
            outcome: if replacing.is_some() { IngestOutcome::Updated } else { IngestOutcome::Added },
            document_id: id,
            chunks: drafts.len(),
            embedded,
            reused,
            pending,
            suspicious,
            truncated,
        })
    }

    async fn merge_assets(&self, doc_id: i64, assets: &[String], outcome: IngestOutcome) -> Result<IngestReport> {
        let current = store::assets_of(&self.pool, doc_id).await?;
        let new: Vec<String> = assets.iter().filter(|a| !current.contains(a)).cloned().collect();
        if !new.is_empty() {
            let mut tx = self.pool.begin().await?;
            store::add_assets(&mut tx, doc_id, &new).await?;
            tx.commit().await?;
            let mut all = current;
            all.extend(new);
            all.sort();
            let mut ix = self.index_write();
            if let Some(mut m) = ix.doc_by_id(doc_id).cloned() {
                m.assets = all;
                ix.upsert_doc(m);
            }
        }
        Ok(IngestReport { outcome, document_id: doc_id, chunks: 0, embedded: 0, reused: 0, pending: 0, suspicious: false, truncated: false })
    }

    /// Noticias que la app YA ha descargado. No hace ninguna petición a proveedores.
    pub async fn ingest_news(&self, asset: Option<&AssetId>, items: &[NewsItem]) -> BatchReport {
        let mut rep = BatchReport::default();
        for item in items {
            let Some(doc) = NewDocument::from_news_item(item, asset) else {
                rep.skipped += 1;
                continue;
            };
            match self.ingest(doc).await {
                Ok(r) => match r.outcome {
                    IngestOutcome::Added => rep.added += 1,
                    IngestOutcome::Updated => rep.updated += 1,
                    IngestOutcome::Unchanged => rep.unchanged += 1,
                    IngestOutcome::Duplicate { .. } => rep.duplicates += 1,
                },
                Err(e) => {
                    tracing::warn!(error = %e, "no se pudo ingerir una noticia");
                    rep.failed += 1;
                }
            }
        }
        rep
    }

    /// Calcula los embeddings que faltan para el modelo activo (embedder caído al ingerir, o modelo
    /// cambiado). Devuelve cuántos fragmentos se completaron.
    pub async fn embed_pending(&self, limit: usize) -> Result<usize> {
        let _gate = self.write_gate.lock().await;
        let pend = store::pending_chunks(&self.pool, &self.model, limit.clamp(1, 2000) as i64).await?;
        if pend.is_empty() {
            return Ok(0);
        }
        let hashes: Vec<String> = pend.iter().map(|p| p.text_hash.clone()).collect();
        let mut vectors = store::embeddings_by_text_hash(&self.pool, &self.model, &hashes).await?;
        let mut todo: Vec<(String, String)> = Vec::new();
        for p in &pend {
            if !vectors.contains_key(&p.text_hash) && !todo.iter().any(|t| t.0 == p.text_hash) {
                todo.push((p.text_hash.clone(), embed_text(p.kind, &p.title, p.section.as_deref(), &p.text)));
            }
        }
        for batch in todo.chunks(self.cfg.embed.batch_size.max(1) * 4) {
            let texts: Vec<String> = batch.iter().map(|t| t.1.clone()).collect();
            match self.embed_texts(&texts, EmbedKind::Document).await {
                Ok(vs) => {
                    for ((h, _), v) in batch.iter().zip(vs) {
                        vectors.insert(h.clone(), v);
                    }
                }
                Err(e) => {
                    // Un embedder caído no debe llenar el log con un aviso en cada pasada de mantenimiento.
                    tracing::debug!(error = %e, "embed_pending interrumpido");
                    break;
                }
            }
        }
        let now_ms = Utc::now().timestamp_millis();
        let mut tx = self.pool.begin().await?;
        let mut done: Vec<(i64, i64, u128, &Vec<f32>)> = Vec::new();
        for p in &pend {
            if let Some(v) = vectors.get(&p.text_hash) {
                store::insert_embedding(&mut tx, p.chunk_id, &self.model, v, now_ms).await?;
                done.push((p.chunk_id, p.document_id, hash128(&p.text), v));
            }
        }
        tx.commit().await?;
        let mut ix = self.index_write();
        for (cid, did, th, v) in &done {
            if ix.doc_by_id(*did).is_some()
                && let Err(e) = ix.add(*cid, *did, *th, v)
            {
                tracing::warn!(error = %e, "vector fuera del índice en memoria");
            }
        }
        Ok(done.len())
    }

    // --------------------------------------------------------------- búsqueda

    /// Embebe la consulta y busca (vectorial + léxica si está activa). Si el embedder falla y hay
    /// búsqueda léxica, sigue solo con BM25; si no, devuelve el error: el llamador debe seguir
    /// SIN conocimiento y decirlo, no inventar.
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResults> {
        let q = normalize_text(&req.query);
        if q.is_empty() {
            return Err(KnowledgeError::Invalid("consulta vacía".into()));
        }
        let lexical_ok = self.lexical_enabled() && fts_query(&q).is_some();
        match self.embed_texts(std::slice::from_ref(&q), EmbedKind::Query).await {
            Ok(mut vs) => {
                let v = vs.pop().ok_or_else(|| KnowledgeError::Internal("sin vector de consulta".into()))?;
                self.search_inner(req, Some(&v), &q).await
            }
            Err(e) if lexical_ok => {
                tracing::warn!(error = %e, "embedder no disponible: búsqueda solo léxica");
                self.search_inner(req, None, &q).await
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Búsqueda con un vector de consulta ya calculado (normalizado).
    pub async fn search_vector(&self, req: &SearchRequest, qv: &[f32]) -> Result<SearchResults> {
        self.search_inner(req, Some(qv), &normalize_text(&req.query)).await
    }

    async fn search_inner(&self, req: &SearchRequest, qv: Option<&[f32]>, qtext: &str) -> Result<SearchResults> {
        let k = req.k.clamp(1, 50);
        let max_per_doc = req.max_per_doc.max(1);
        let now = req.now.unwrap_or_else(Utc::now);
        let n = (k * 5).max(25);
        let filter = Filter {
            assets: req.assets.iter().map(|a| a.trim().to_string()).collect(),
            include_general: req.include_general,
            kinds: req.kinds.clone(),
            issuer_key: req.issuer.as_deref().map(fold_key).filter(|s| !s.is_empty()),
            since_ms: req.since.map(|t| t.timestamp_millis()),
            until_ms: req.until.map(|t| t.timestamp_millis()),
        };
        // Candidatos léxicos (BM25), ya con los filtros de SQL; el resto del filtro (emisor) se aplica abajo.
        let mut lex: Vec<store::LexRow> = Vec::new();
        if self.lexical_enabled()
            && let Some(expr) = fts_query(qtext)
        {
            let q = store::LexQuery {
                expr: &expr,
                kinds: &filter.kinds,
                assets: &filter.assets,
                include_general: filter.include_general,
                since_ms: filter.since_ms,
                until_ms: filter.until_ms,
                limit: n as i64,
            };
            match store::lexical_search(&self.pool, &q).await {
                Ok(rows) => lex = rows,
                Err(e) if qv.is_some() => tracing::warn!(error = %e, "búsqueda léxica fallida: se usa solo la vectorial"),
                Err(e) => return Err(e),
            }
        }
        struct Ranked {
            chunk_id: i64,
            doc_id: i64,
            text_hash: u128,
            similarity: f32,
            lexical: bool,
            score: f32,
            recency: f32,
            srcw: f32,
        }
        let (ranked, scanned) = {
            let ix = self.index_read();
            let (vcands, vscanned) = match qv {
                Some(qv) => ix.search(qv, &filter, n, req.min_similarity),
                None => (Vec::new(), 0),
            };
            let lex: Vec<&store::LexRow> = lex.iter().filter(|r| ix.accepts(&filter, r.doc_id)).collect();
            let hybrid = !lex.is_empty();
            // Fusión RRF: posición en cada lista (0 = la mejor). Sin candidatos léxicos, la puntuación es la de siempre.
            // chunk -> (documento, hash del texto, coseno, posición vectorial, posición léxica)
            type Pooled = (i64, u128, f32, Option<usize>, Option<usize>);
            let mut pool: HashMap<i64, Pooled> = HashMap::new();
            for (rank, c) in vcands.iter().enumerate() {
                pool.insert(c.chunk_id, (c.doc_id, c.text_hash, c.similarity, Some(rank), None));
            }
            for (rank, r) in lex.iter().enumerate() {
                let Some(th) = parse_hash128(&r.text_hash) else { continue };
                pool.entry(r.chunk_id).and_modify(|e| e.4 = Some(rank)).or_insert((r.doc_id, th, 0.0, None, Some(rank)));
            }
            let mut ranked: Vec<Ranked> = pool
                .into_iter()
                .filter_map(|(chunk_id, (doc_id, text_hash, sim, vr, lr))| {
                    let d = ix.doc_by_id(doc_id)?;
                    let age_days = (now.timestamp_millis() - d.published_ms) as f64 / 86_400_000.0;
                    let recency = recency(d.kind, age_days);
                    let srcw = self.cfg.source_weight(d.kind, &d.source);
                    let relevance = if hybrid {
                        (vr.map_or(0.0, rrf_term) + lr.map_or(0.0, rrf_term)) / rrf_max()
                    } else {
                        sim.clamp(0.0, 1.0)
                    };
                    let score = 0.80 * relevance + 0.12 * recency + 0.08 * srcw;
                    Some(Ranked { chunk_id, doc_id, text_hash, similarity: sim, lexical: lr.is_some(), score, recency, srcw })
                })
                .collect();
            ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.chunk_id.cmp(&b.chunk_id)));
            (ranked, vscanned + lex.len())
        };
        // Colapsa fragmentos idénticos (S-1 y su enmienda S-1/A) y limita por documento.
        let mut seen_text: HashSet<u128> = HashSet::new();
        let mut per_doc: HashMap<i64, usize> = HashMap::new();
        let mut picked: Vec<Ranked> = Vec::new();
        for r in ranked {
            if picked.len() >= k {
                break;
            }
            if seen_text.contains(&r.text_hash) {
                continue;
            }
            let n = per_doc.entry(r.doc_id).or_default();
            if *n >= max_per_doc {
                continue;
            }
            *n += 1;
            seen_text.insert(r.text_hash);
            picked.push(r);
        }
        let ids: Vec<i64> = picked.iter().map(|r| r.chunk_id).collect();
        let mut rows = store::chunks_with_docs(&self.pool, &ids).await?;
        let hits = picked
            .into_iter()
            .filter_map(|r| {
                let (section, text, doc) = rows.remove(&r.chunk_id)?;
                Some(SearchHit { chunk_id: r.chunk_id, section, text, score: r.score, similarity: r.similarity, lexical: r.lexical, recency: r.recency, source_weight: r.srcw, doc })
            })
            .collect();
        Ok(SearchResults { hits, scanned, model: self.model.clone() })
    }

    // -------------------------------------------------------------- retención

    /// Aplica la retención por antigüedad y el tope de fragmentos. Los fijados y los análisis
    /// (retención 0) nunca se tocan.
    pub async fn purge(&self, now: DateTime<Utc>) -> Result<PurgeReport> {
        let _gate = self.write_gate.lock().await;
        let mut rep = PurgeReport::default();
        let mut ids: Vec<i64> = Vec::new();
        let mut gone: HashSet<i64> = HashSet::new();
        let mut removed_chunks_est = 0i64;
        for kind in [DocKind::News, DocKind::Filing, DocKind::Analysis] {
            let days = self.cfg.retention_days(kind);
            if days == 0 {
                continue;
            }
            let cutoff = (now - chrono::Duration::days(i64::from(days))).timestamp_millis();
            for (id, cc) in store::expired(&self.pool, kind, cutoff).await? {
                if gone.insert(id) {
                    ids.push(id);
                    removed_chunks_est += cc;
                    rep.documents_by_age += 1;
                }
            }
        }
        let total = store::total_chunks(&self.pool).await? - removed_chunks_est;
        let mut overflow = total - self.cfg.max_chunks as i64;
        if overflow > 0 {
            for (id, cc) in store::eviction_order(&self.pool).await? {
                if overflow <= 0 {
                    break;
                }
                if gone.insert(id) {
                    ids.push(id);
                    overflow -= cc;
                    rep.documents_by_cap += 1;
                }
            }
        }
        if ids.is_empty() {
            return Ok(rep);
        }
        let mut tx = self.pool.begin().await?;
        rep.chunks_removed = store::delete_documents(&mut tx, &ids, self.lexical_enabled()).await?;
        tx.commit().await?;
        self.index_write().remove_docs(&gone);
        Ok(rep)
    }

    /// Fija (o suelta) un documento: no se purga por antigüedad ni por tope.
    pub async fn pin(&self, document_id: i64, pinned: bool) -> Result<bool> {
        store::set_pinned(&self.pool, document_id, pinned).await
    }

    /// Fusiona campos en `documents.meta` (p. ej. el resultado de un análisis). No toca el texto
    /// ni los embeddings.
    pub async fn merge_meta(&self, document_id: i64, patch: serde_json::Value) -> Result<()> {
        let cur = store::get_meta(&self.pool, document_id).await?.ok_or_else(|| KnowledgeError::Invalid("documento inexistente".into()))?;
        let mut base: serde_json::Value = serde_json::from_str(&cur).unwrap_or_else(|_| serde_json::json!({}));
        if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                b.insert(k.clone(), v.clone());
            }
        }
        store::set_meta(&self.pool, document_id, &base.to_string()).await
    }

    /// Borra un documento por su clave lógica.
    pub async fn delete_by_key(&self, doc_key: &str) -> Result<bool> {
        let _gate = self.write_gate.lock().await;
        let Some(ex) = store::find_by_key(&self.pool, doc_key).await? else { return Ok(false) };
        let mut tx = self.pool.begin().await?;
        store::delete_documents(&mut tx, &[ex.id], self.lexical_enabled()).await?;
        tx.commit().await?;
        self.index_write().remove_docs(&HashSet::from([ex.id]));
        Ok(true)
    }

    pub async fn stats(&self) -> Result<Stats> {
        let (documents, text_bytes, embedding_bytes) = store::sizes(&self.pool).await?;
        let chunks = store::total_chunks(&self.pool).await? as usize;
        let pending = store::count_pending(&self.pool, &self.model).await? as usize;
        let lexical_indexed = if self.lexical_enabled() { store::fts_count(&self.pool).await.unwrap_or(0).max(0) as usize } else { 0 };
        let ix = self.index_read();
        Ok(Stats {
            documents: documents as usize,
            chunks,
            indexed: ix.len(),
            pending,
            text_bytes,
            embedding_bytes,
            index_memory_bytes: ix.memory_bytes(),
            model: self.model.clone(),
            lexical: self.lexical_enabled(),
            lexical_indexed,
        })
    }
}

#[async_trait]
impl KnowledgeSearch for KnowledgeBase {
    async fn search(&self, req: &SearchRequest) -> Result<SearchResults> {
        KnowledgeBase::search(self, req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedder_failure_pauses_it_and_the_permanent_ones_longer() {
        // N3: `Bad` no pausaba (`None`): con el archivado por defecto, una llamada inútil por documento y por pasada.
        let unavailable = embed_pause(&EmbedError::Unavailable("x".into()));
        let missing = embed_pause(&EmbedError::ModelMissing("m".into()));
        let bad = embed_pause(&EmbedError::Bad("respuesta".into()));
        assert_eq!(unavailable, Duration::from_secs(30));
        assert_eq!((missing, bad), (Duration::from_secs(300), Duration::from_secs(300)));
        // Más larga que el intervalo del mantenimiento (120 s): al menos una pasada completa sin llamar.
        assert!(bad > Duration::from_secs(120));
    }

    #[test]
    fn incoherent_vectors_are_bad_responses_and_good_ones_come_out_normalised() {
        assert!(matches!(checked_vectors(vec![vec![1.0, 0.0]], 2), Err(EmbedError::Bad(_))), "faltan vectores");
        assert!(matches!(checked_vectors(vec![vec![1.0, 0.0], vec![1.0]], 2), Err(EmbedError::Bad(_))), "dimensiones desiguales");
        assert!(matches!(checked_vectors(vec![vec![f32::NAN, 1.0]], 1), Err(EmbedError::Bad(_))), "NaN");
        assert!(matches!(checked_vectors(vec![vec![0.0, 0.0]], 1), Err(EmbedError::Bad(_))), "norma cero");
        let ok = checked_vectors(vec![vec![3.0, 4.0]], 1).unwrap();
        assert!((ok[0][0] - 0.6).abs() < 1e-6 && (ok[0][1] - 0.8).abs() < 1e-6);
        assert!(checked_vectors(vec![], 0).unwrap().is_empty());
    }
}
