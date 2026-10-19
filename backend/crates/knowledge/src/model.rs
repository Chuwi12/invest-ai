//! Tipos públicos: documentos, consultas y resultados.

use chrono::{DateTime, Utc};
use domain::{AssetId, NewsItem};
use serde::{Deserialize, Serialize};

use crate::{
    chunk::Section,
    text::{fold_key, normalize_text, sha256_hex},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    News,
    Filing,
    Analysis,
}

impl DocKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::News => "news",
            Self::Filing => "filing",
            Self::Analysis => "analysis",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "news" => Some(Self::News),
            "filing" => Some(Self::Filing),
            "analysis" => Some(Self::Analysis),
            _ => None,
        }
    }
}

/// Documento a ingerir. Para folletos (S-1/F-1/424B) pasa `sections` ya separadas (o usa
/// [`crate::sections_from_text`]); para noticias, ver [`NewDocument::from_news_item`].
#[derive(Debug, Clone)]
pub struct NewDocument {
    /// Identidad lógica estable (`news:<id>`, `filing:<accession>`, `analysis:<uuid>`).
    pub doc_key: String,
    pub kind: DocKind,
    /// Formulario (`S-1`, `424B4`, `8-K`…).
    pub subtype: Option<String>,
    /// Proveedor/origen: `sec`, `google_news`, `invest-ai`…
    pub source: String,
    pub publisher: Option<String>,
    pub title: String,
    /// Enlace verificable al original (obligatorio salvo en análisis propios).
    pub url: Option<String>,
    /// Emisor (nombre) para folletos de OPV aún sin ticker.
    pub issuer: Option<String>,
    pub language: Option<String>,
    pub published_at: DateTime<Utc>,
    /// Ids de activo (`stock:AAPL`). Vacío = documento general.
    pub assets: Vec<String>,
    pub sections: Vec<Section>,
    /// JSON libre (tickers, resultado de un análisis…); no entra en el hash.
    pub meta: serde_json::Value,
}

/// «Titular - Medio» -> «Titular» (Google News añade el medio al título).
fn strip_publisher_suffix(title: &str) -> &str {
    match title.rsplit_once(" - ") {
        Some((head, tail)) if tail.split_whitespace().count() <= 4 && !head.is_empty() => head,
        _ => title,
    }
}

impl NewDocument {
    /// Convierte una noticia que la app YA ha descargado (sin pedir nada a ningún proveedor).
    /// `None` si no hay contenido útil (título casi vacío).
    pub fn from_news_item(item: &NewsItem, asset: Option<&AssetId>) -> Option<Self> {
        let title = normalize_text(&item.title);
        if title.split_whitespace().count() < 3 {
            return None;
        }
        let mut body = title.clone();
        if let Some(s) = item.summary.as_deref().map(normalize_text).filter(|s| !s.is_empty() && fold_key(s) != fold_key(&title)) {
            body.push_str("\n\n");
            body.push_str(&s);
        }
        Some(Self {
            doc_key: format!("news:{}", item.id),
            kind: DocKind::News,
            subtype: None,
            source: item.provider.clone(),
            publisher: item.publisher.clone(),
            title,
            url: Some(item.url.clone()),
            issuer: None,
            language: item.language.clone(),
            published_at: item.published_at,
            assets: asset.map(|a| vec![a.to_string()]).unwrap_or_default(),
            sections: vec![Section { heading: None, text: body }],
            meta: serde_json::json!({ "tickers": item.tickers }),
        })
    }

    /// Hash de deduplicación. Noticias: título sin «- Medio» + día (el mismo titular de dos
    /// proveedores colapsa); el resto: el texto normalizado completo.
    pub fn content_hash(&self) -> String {
        let basis = match self.kind {
            DocKind::News => {
                format!("news\n{}\n{}", fold_key(strip_publisher_suffix(&self.title)), self.published_at.format("%Y-%m-%d"))
            }
            DocKind::Filing | DocKind::Analysis => {
                let body: Vec<String> =
                    self.sections.iter().map(|s| format!("{}\n{}", s.heading.as_deref().unwrap_or(""), normalize_text(&s.text))).collect();
                let key = if self.kind == DocKind::Analysis { self.doc_key.as_str() } else { "" };
                format!("{}\n{key}\n{}", self.kind.as_str(), body.join("\n\n"))
            }
        };
        sha256_hex(basis.as_bytes())
    }
}

/// Metadatos de un documento almacenado (lo que se cita).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocumentInfo {
    pub id: i64,
    pub doc_key: String,
    pub kind: DocKind,
    pub subtype: Option<String>,
    pub source: String,
    pub publisher: Option<String>,
    pub title: String,
    pub url: Option<String>,
    pub issuer: Option<String>,
    pub language: Option<String>,
    pub published_at: DateTime<Utc>,
    pub assets: Vec<String>,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestOutcome {
    Added,
    /// Misma `doc_key` con contenido distinto: se reemplazaron sus fragmentos.
    Updated,
    /// Ya estaba exactamente igual (solo se fusionaron activos nuevos).
    Unchanged,
    /// Mismo contenido bajo otra `doc_key` (p. ej. el mismo titular de otro proveedor).
    Duplicate { of: i64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestReport {
    pub outcome: IngestOutcome,
    pub document_id: i64,
    pub chunks: usize,
    /// Fragmentos cuyo embedding se calculó ahora.
    pub embedded: usize,
    /// Fragmentos que reutilizaron un embedding existente (mismo texto).
    pub reused: usize,
    /// Fragmentos guardados SIN embedding (embedder caído): `embed_pending` los completa.
    pub pending: usize,
    /// El texto contiene órdenes al modelo; se guarda tal cual y solo se avisa en el log.
    pub suspicious: bool,
    /// El documento superó el tope de fragmentos y se recortó.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatchReport {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub duplicates: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Consulta. `Default` da una consulta vacía: rellena `query`.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub k: usize,
    /// Cualquiera de estos activos (vacío = sin filtro por activo).
    pub assets: Vec<String>,
    /// Con `assets` no vacío, incluir también documentos generales (sin activo).
    pub include_general: bool,
    /// Vacío = todos los tipos.
    pub kinds: Vec<DocKind>,
    /// Emisor (comparación sin mayúsculas ni signos).
    pub issuer: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Similitud coseno mínima (depende del modelo: calibrar).
    pub min_similarity: Option<f32>,
    /// Máximo de fragmentos por documento.
    pub max_per_doc: usize,
    /// Reloj para la recencia (tests). `None` = ahora.
    pub now: Option<DateTime<Utc>>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            k: 6,
            assets: Vec::new(),
            include_general: false,
            kinds: Vec::new(),
            issuer: None,
            since: None,
            until: None,
            min_similarity: None,
            max_per_doc: 2,
            now: None,
        }
    }
}

impl SearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self { query: query.into(), ..Self::default() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub chunk_id: i64,
    pub section: Option<String>,
    /// Texto original del fragmento, sin alterar.
    pub text: String,
    /// Puntuación final tras el reranking (mayor = mejor).
    pub score: f32,
    /// Coseno con la consulta; 0 si el fragmento solo salió de la búsqueda léxica (sin vector o fuera del top vectorial).
    pub similarity: f32,
    /// `true` si BM25 (FTS5) también lo devolvió.
    pub lexical: bool,
    pub recency: f32,
    pub source_weight: f32,
    pub doc: DocumentInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// Fragmentos que pasaron los filtros y se compararon.
    pub scanned: usize,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub documents: usize,
    pub chunks: usize,
    /// Fragmentos con embedding del modelo activo (están en el índice en memoria).
    pub indexed: usize,
    pub pending: usize,
    pub text_bytes: i64,
    pub embedding_bytes: i64,
    /// Memoria del índice en RAM (vectores + filas), estimada.
    pub index_memory_bytes: usize,
    pub model: String,
    /// La búsqueda léxica (FTS5) está activa.
    pub lexical: bool,
    /// Fragmentos presentes en la tabla FTS5.
    pub lexical_indexed: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PurgeReport {
    pub documents_by_age: usize,
    pub documents_by_cap: usize,
    pub chunks_removed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn news(title: &str, day: u32) -> NewsItem {
        NewsItem {
            id: format!("id-{title}"),
            title: title.into(),
            summary: None,
            url: format!("https://x.test/{title}"),
            publisher: None,
            published_at: DateTime::from_timestamp(1_760_000_000 + i64::from(day) * 86_400, 0).unwrap(),
            image_url: None,
            tickers: vec![],
            sentiment: None,
            provider: "yahoo".into(),
            language: Some("en".into()),
        }
    }

    #[test]
    fn same_headline_from_two_providers_has_one_hash() {
        let a = NewDocument::from_news_item(&news("Apple beats estimates", 0), None).unwrap();
        let mut b = NewDocument::from_news_item(&news("Apple beats estimates - Reuters", 0), None).unwrap();
        b.doc_key = "news:other".into();
        assert_eq!(a.content_hash(), b.content_hash());
        let c = NewDocument::from_news_item(&news("Apple beats estimates", 3), None).unwrap();
        assert_ne!(a.content_hash(), c.content_hash(), "otro día = otra noticia");
    }

    #[test]
    fn junk_titles_are_skipped() {
        assert!(NewDocument::from_news_item(&news("Hola", 0), None).is_none());
    }
}
