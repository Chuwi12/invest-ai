//! Utilidades de noticias: URL canónica e id estable, deduplicación (URL y título casi idéntico),
//! relevancia respecto al activo y ordenación. Son funciones puras; las llamadas a los
//! proveedores viven en `service.rs`.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use domain::{AssetClass, AssetId, NewsItem};
use sha2::{Digest, Sha256};
use url::Url;

use crate::providers::plain_ticker;

/// Parámetros que solo sirven para rastrear el clic y no identifican el artículo.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "dclid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "igshid",
    "ref",
    "ref_src",
    "ref_url",
    "cmpid",
    "ocid",
    "ncid",
    "soc_src",
    "soc_trk",
    "yptr",
    "guccounter",
    "taid",
    "spm",
    "ito",
    "mkt_tok",
    "_hsenc",
    "_hsmi",
    "cid",
    "rss",
    "oc",
    "output",
    "amp",
    "outputType",
    "guce_referrer",
    "guce_referrer_sig",
    "sref",
    "smid",
    "smtyp",
    "cmp",
    "campaign",
];

/// URL sin esquema, `www.`, fragmento ni parámetros de rastreo, con los restantes ordenados.
pub fn canonical_url(raw: &str) -> String {
    let Ok(url) = Url::parse(raw.trim()) else {
        return raw.trim().to_ascii_lowercase();
    };
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("amp."))
        .unwrap_or(&host);
    let mut path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    let mut query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !k.starts_with("utm_") && !TRACKING_PARAMS.iter().any(|t| t.eq_ignore_ascii_case(&k))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    query.sort();
    let mut out = format!("{host}{path}");
    if !query.is_empty() {
        out.push('?');
        out.push_str(
            &query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    out
}

/// Hash estable (SHA-256 truncado a 96 bits, en hexadecimal) de la URL canónica.
pub fn item_id(url: &str) -> String {
    let digest = Sha256::digest(canonical_url(url).as_bytes());
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// Deduplicación
// ---------------------------------------------------------------------------------------------

/// Título en minúsculas, sin signos y sin el « - Medio» final que añade Google News.
fn normalized_title(title: &str, publisher: Option<&str>) -> Vec<String> {
    let mut t = title.trim().to_string();
    if let Some((head, tail)) = t.rsplit_once(" - ") {
        let tail_is_publisher =
            publisher.is_some_and(|p| p.trim().eq_ignore_ascii_case(tail.trim()));
        // Sin publisher conocido, un sufijo corto (≤ 4 palabras) también suele ser el medio.
        if tail_is_publisher || (publisher.is_none() && tail.split_whitespace().count() <= 4) {
            t = head.to_string();
        }
    }
    t.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn similar(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    if a.len().min(b.len()) < 4 {
        return false;
    }
    let sa: HashSet<&String> = a.iter().collect();
    let sb: HashSet<&String> = b.iter().collect();
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    inter / union >= 0.8
}

fn richness(n: &NewsItem) -> u8 {
    u8::from(n.summary.is_some())
        + u8::from(n.image_url.is_some())
        + u8::from(n.sentiment.is_some())
        + u8::from(!n.tickers.is_empty())
        // Un enlace directo al medio es mejor que la redirección de Google News.
        + u8::from(n.provider != "google_news")
}

fn absorb(keep: &mut NewsItem, other: NewsItem) {
    if keep.summary.is_none() {
        keep.summary = other.summary;
    }
    if keep.image_url.is_none() {
        keep.image_url = other.image_url;
    }
    if keep.sentiment.is_none() {
        keep.sentiment = other.sentiment;
    }
    if keep.publisher.is_none() {
        keep.publisher = other.publisher;
    }
    for t in other.tickers {
        if !keep.tickers.contains(&t) {
            keep.tickers.push(t);
        }
    }
}

/// Elimina duplicados por id (URL canónica) y por título casi idéntico. Conserva el elemento más
/// completo y le incorpora los campos que solo tenía el descartado.
pub fn dedupe(items: Vec<NewsItem>) -> Vec<NewsItem> {
    let mut kept: Vec<(NewsItem, Vec<String>)> = Vec::with_capacity(items.len());
    for item in items {
        let tokens = normalized_title(&item.title, item.publisher.as_deref());
        let dup = kept
            .iter()
            .position(|(k, kt)| k.id == item.id || similar(kt, &tokens));
        match dup {
            Some(i) => {
                if richness(&item) > richness(&kept[i].0) {
                    let (old, _) = std::mem::replace(&mut kept[i], (item, tokens));
                    absorb(&mut kept[i].0, old);
                } else {
                    absorb(&mut kept[i].0, item);
                }
            }
            None => kept.push((item, tokens)),
        }
    }
    kept.into_iter().map(|(n, _)| n).collect()
}

// ---------------------------------------------------------------------------------------------
// Relevancia
// ---------------------------------------------------------------------------------------------

const LEGAL_SUFFIXES: &[&str] = &[
    "inc",
    "incorporated",
    "corp",
    "corporation",
    "co",
    "company",
    "ltd",
    "limited",
    "plc",
    "sa",
    "s.a",
    "ag",
    "nv",
    "n.v",
    "se",
    "llc",
    "lp",
    "holdings",
    "holding",
    "group",
    "the",
    "class",
    "a",
    "b",
    "c",
    "adr",
    "ordinary",
    "shares",
    "common",
    "stock",
    "trust",
    "etf",
    "fund",
    "new",
    "de",
    "banco",
    "bank",
    "motor",
    "motors",
    "international",
    "technologies",
    "technology",
    "systems",
    "usd",
];

/// Nombre comercial limpio para consultas de texto: sin sufijos legales ni signos, máx. 3
/// palabras (`"Apple Inc."` → `"Apple"`, `"Banco Santander, S.A."` → `"Santander"`).
/// Solo deja letras, dígitos y `&`: es seguro meterlo en una consulta de proveedor.
pub fn brand(name: &str) -> String {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '&')
        .filter(|w| w.chars().count() > 1 && !LEGAL_SUFFIXES.contains(w))
        .take(3)
        .collect();
    let joined = words.join(" ");
    // Recupera las mayúsculas originales palabra a palabra.
    let orig: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric() && c != '&')
        .collect();
    joined
        .split(' ')
        .map(|w| {
            orig.iter()
                .find(|o| o.to_lowercase() == w)
                .copied()
                .unwrap_or(w)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Términos con los que se reconoce que una noticia habla de un activo.
#[derive(Debug, Clone, Default)]
pub struct Relevance {
    /// Ticker en mayúsculas (se busca como palabra completa y exacta).
    ticker: Option<String>,
    /// Símbolo completo tal como aparece en `NewsItem.tickers` (p. ej. `SAN.MC`, `AAPL`).
    symbol: String,
    /// Frases en minúsculas (nombre limpio y su palabra distintiva).
    phrases: Vec<String>,
}

impl Relevance {
    pub fn new(id: &AssetId, name: Option<&str>) -> Self {
        let ticker = plain_ticker(&id.symbol).to_ascii_uppercase();
        // Un ticker de 1–2 letras (F, T, GM) casa con demasiado ruido; solo vale en cripto.
        let min_len = if id.class == AssetClass::Crypto { 2 } else { 3 };
        let mut phrases = Vec::new();
        if let Some(name) = name {
            let words: Vec<String> = name
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '&' && c != '\'')
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect();
            let core: Vec<&String> = words
                .iter()
                .filter(|w| !LEGAL_SUFFIXES.contains(&w.as_str()))
                .collect();
            if !core.is_empty() {
                phrases.push(
                    core.iter()
                        .map(|w| w.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                if core.len() > 1 {
                    // La palabra más larga suele ser la marca («santander», «nvidia»).
                    if let Some(longest) = core.iter().max_by_key(|w| w.chars().count())
                        && longest.chars().count() >= 5
                    {
                        phrases.push((*longest).clone());
                    }
                }
            }
        }
        phrases.sort();
        phrases.dedup();
        Self {
            ticker: (ticker.chars().count() >= min_len).then_some(ticker),
            symbol: id.symbol.to_ascii_uppercase(),
            phrases,
        }
    }

    /// 3 = etiquetada con el símbolo por el proveedor, 2 = aparece en el título,
    /// 1 = aparece en el resumen, 0 = sin relación aparente.
    pub fn score(&self, item: &NewsItem) -> u8 {
        let tagged = item.tickers.iter().any(|t| {
            let t = t.to_ascii_uppercase();
            t == self.symbol || self.ticker.as_deref() == Some(plain_ticker(&t).as_str())
        });
        if tagged {
            return 3;
        }
        if self.mentions(&item.title) {
            return 2;
        }
        if item.summary.as_deref().is_some_and(|s| self.mentions(s)) {
            return 1;
        }
        0
    }

    fn mentions(&self, text: &str) -> bool {
        if let Some(t) = &self.ticker
            && contains_word(text, t, true)
        {
            return true;
        }
        let lower = text.to_lowercase();
        self.phrases.iter().any(|p| contains_word(&lower, p, false))
    }
}

/// Búsqueda de palabra completa (sin depender de `regex`).
fn contains_word(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    let (h, n);
    let (h, n) = if case_sensitive {
        (haystack, needle)
    } else {
        h = haystack.to_lowercase();
        n = needle.to_lowercase();
        (h.as_str(), n.as_str())
    };
    if n.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric();
    let mut from = 0;
    while let Some(pos) = h[from..].find(n) {
        let start = from + pos;
        let end = start + n.len();
        let before_ok = h[..start].chars().next_back().is_none_or(|c| !is_word(c));
        let after_ok = h[end..].chars().next().is_none_or(|c| !is_word(c));
        if before_ok && after_ok {
            return true;
        }
        from = start + h[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Proveedores cuya consulta ya va acotada al símbolo: sus noticias se aceptan aunque no
/// repitan el ticker en el título.
const SCOPED_PROVIDERS: &[&str] = &[
    "finnhub",
    "yahoo",
    "marketaux",
    "alphavantage",
    "cryptopanic",
    "sec",
];

/// Une listas de varios proveedores: filtra por relevancia (si hay activo), deduplica, ordena por
/// fecha descendente y recorta a `limit`.
pub fn merge(
    lists: Vec<Vec<NewsItem>>,
    relevance: Option<&Relevance>,
    limit: usize,
    now: DateTime<Utc>,
) -> Vec<NewsItem> {
    let horizon = now + Duration::days(1);
    let mut all: Vec<(u8, NewsItem)> = lists
        .into_iter()
        .flatten()
        .filter(|n| n.published_at <= horizon && !n.title.trim().is_empty() && !n.url.is_empty())
        .filter_map(|n| match relevance {
            None => Some((0, n)),
            Some(r) => {
                let s = r.score(&n);
                (s > 0 || SCOPED_PROVIDERS.contains(&n.provider.as_str())).then_some((s, n))
            }
        })
        .collect();
    // Más reciente primero; a igual fecha, más relevante primero (orden estable).
    all.sort_by(|(sa, a), (sb, b)| b.published_at.cmp(&a.published_at).then(sb.cmp(sa)));
    let mut out = dedupe(all.into_iter().map(|(_, n)| n).collect());
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, url: &str, provider: &str, ts: i64) -> NewsItem {
        NewsItem {
            id: item_id(url),
            title: title.into(),
            summary: None,
            url: url.into(),
            publisher: None,
            published_at: DateTime::<Utc>::from_timestamp(ts, 0).unwrap(),
            image_url: None,
            tickers: vec![],
            sentiment: None,
            provider: provider.into(),
            language: None,
        }
    }

    #[test]
    fn canonical_url_ignores_tracking_and_noise() {
        let a =
            canonical_url("https://www.Example.com/news/a-b/?utm_source=x&id=7&fbclid=zzz#frag");
        let b = canonical_url("http://example.com/news/a-b?id=7");
        assert_eq!(a, b);
        assert_eq!(a, "example.com/news/a-b?id=7");
        assert_eq!(
            canonical_url("https://news.google.com/rss/articles/CBMi123?oc=5"),
            "news.google.com/rss/articles/CBMi123"
        );
        // Los parámetros restantes se ordenan.
        assert_eq!(
            canonical_url("https://a.com/p?b=2&a=1"),
            canonical_url("https://a.com/p?a=1&b=2")
        );
    }

    #[test]
    fn item_id_is_stable_and_url_based() {
        let id = item_id("https://example.com/a?utm_campaign=x");
        assert_eq!(id, item_id("http://www.example.com/a/"));
        assert_ne!(id, item_id("https://example.com/b"));
        assert_eq!(id.len(), 24);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // Valor fijo: si cambia el algoritmo se rompen los ids ya guardados por el cliente.
        assert_eq!(item_id("https://example.com/a"), "036cbe16b0015b0a30d733ca");
    }

    #[test]
    fn dedupes_by_url_and_near_identical_title() {
        let mut rich = item(
            "Apple sube un 3% tras resultados - Reuters",
            "https://reuters.com/a",
            "finnhub",
            1000,
        );
        rich.summary = Some("Resumen".into());
        let g = item(
            "Apple sube un 3% tras resultados - Reuters",
            "https://news.google.com/rss/articles/X?oc=5",
            "google_news",
            1010,
        );
        let same_url = item(
            "Otro titular distinto",
            "https://www.reuters.com/a?utm_source=y",
            "gdelt",
            990,
        );
        let different = item(
            "Microsoft anuncia nuevo chip",
            "https://x.com/m",
            "gdelt",
            900,
        );
        let out = dedupe(vec![g, rich, same_url, different]);
        assert_eq!(out.len(), 2, "{out:#?}");
        // Se queda el más completo (el de Finnhub, con resumen y enlace directo).
        assert!(
            out.iter()
                .any(|n| n.provider == "finnhub" && n.summary.is_some())
        );
    }

    #[test]
    fn similar_titles_need_enough_overlap() {
        let a = normalized_title("Apple supera previsiones en el tercer trimestre", None);
        let b = normalized_title("Apple supera previsiones en el tercer trimestre.", None);
        let c = normalized_title("Apple supera previsiones y sube en bolsa", None);
        assert!(similar(&a, &b));
        assert!(!similar(&a, &c));
    }

    #[test]
    fn relevance_scores() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let r = Relevance::new(&id, Some("Apple Inc."));
        let mut tagged = item("Algo", "https://a.com/1", "finnhub", 1);
        tagged.tickers = vec!["AAPL".into()];
        assert_eq!(r.score(&tagged), 3);
        assert_eq!(
            r.score(&item(
                "Apple lanza el iPhone",
                "https://a.com/2",
                "gdelt",
                1
            )),
            2
        );
        assert_eq!(
            r.score(&item("AAPL cae un 2%", "https://a.com/3", "gdelt", 1)),
            2
        );
        assert_eq!(
            r.score(&item(
                "Precio de la manzana en el mercado",
                "https://a.com/4",
                "gdelt",
                1
            )),
            0
        );
        // «Pineapple» no cuenta: palabra completa.
        assert_eq!(
            r.score(&item(
                "Pineapple prices rise",
                "https://a.com/5",
                "gdelt",
                1
            )),
            0
        );
        let mut sum = item("Titular sin pistas", "https://a.com/6", "gdelt", 1);
        sum.summary = Some("La empresa Apple presentó resultados".into());
        assert_eq!(r.score(&sum), 1);
    }

    #[test]
    fn brand_strips_legal_suffixes() {
        assert_eq!(brand("Apple Inc."), "Apple");
        assert_eq!(brand("Banco Santander, S.A."), "Santander");
        assert_eq!(brand("NVIDIA CORP"), "NVIDIA");
        assert_eq!(brand("Bitcoin"), "Bitcoin");
        assert_eq!(brand("Alphabet Inc. \"; DROP"), "Alphabet DROP");
    }

    #[test]
    fn relevance_for_spanish_stock_and_crypto() {
        let san = Relevance::new(
            &AssetId::new(AssetClass::Stock, "SAN.MC"),
            Some("Banco Santander, S.A."),
        );
        assert_eq!(
            san.score(&item(
                "Santander gana 3.000 millones",
                "https://a.com/1",
                "google_news",
                1
            )),
            2
        );
        assert_eq!(
            san.score(&item(
                "El Banco Santander amplía capital",
                "https://a.com/2",
                "google_news",
                1
            )),
            2
        );
        let btc = Relevance::new(&AssetId::new(AssetClass::Crypto, "BTC"), Some("Bitcoin"));
        assert_eq!(
            btc.score(&item(
                "Bitcoin supera los 80.000",
                "https://a.com/3",
                "gdelt",
                1
            )),
            2
        );
        assert_eq!(
            btc.score(&item("BTC rebota", "https://a.com/4", "gdelt", 1)),
            2
        );
        // Tickers de una letra no se usan como término de búsqueda.
        let ford = Relevance::new(
            &AssetId::new(AssetClass::Stock, "F"),
            Some("Ford Motor Company"),
        );
        assert_eq!(
            ford.score(&item("Plan F del gobierno", "https://a.com/5", "gdelt", 1)),
            0
        );
        assert_eq!(
            ford.score(&item(
                "Ford retira 100.000 coches",
                "https://a.com/6",
                "gdelt",
                1
            )),
            2
        );
    }

    #[test]
    fn merge_filters_sorts_and_limits() {
        let id = AssetId::new(AssetClass::Stock, "AAPL");
        let r = Relevance::new(&id, Some("Apple Inc."));
        let now = DateTime::<Utc>::from_timestamp(10_000, 0).unwrap();
        let a = vec![
            item("Apple vieja", "https://a.com/1", "gdelt", 100),
            item("Nada que ver", "https://a.com/2", "gdelt", 9_000),
            item("Apple reciente", "https://a.com/3", "gdelt", 5_000),
        ];
        let b = vec![
            // Proveedor acotado al símbolo: pasa aunque el título no mencione el ticker.
            item(
                "Resultados trimestrales",
                "https://b.com/1",
                "finnhub",
                7_000,
            ),
            // Fecha futura (>1 día): se descarta.
            item(
                "Apple del futuro",
                "https://b.com/2",
                "gdelt",
                10_000 + 3 * 86_400,
            ),
        ];
        let out = merge(vec![a, b], Some(&r), 10, now);
        let titles: Vec<_> = out.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Resultados trimestrales", "Apple reciente", "Apple vieja"]
        );
        assert_eq!(
            merge(
                vec![vec![item("x", "https://a.com/9", "gdelt", 1)]],
                None,
                0,
                now
            )
            .len(),
            0
        );
    }
}
