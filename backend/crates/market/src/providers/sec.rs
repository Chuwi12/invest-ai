//! SEC EDGAR. Sin clave, pero la SEC exige un `User-Agent` con contacto y un máximo de 10
//! peticiones por segundo. Se exponen los últimos filings relevantes (10-K, 10-Q, 8-K...) como
//! noticias del proveedor `sec`. Solo empresas con ticker en EE. UU.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{AssetClass, AssetId, IpoEntry, IpoStatus, NewsItem, Prospectus};
use moka::future::Cache;
use reqwest::{Client, header};
use serde_json::Value;

use super::{
    Capability, NewsQuery, PResult, Provider, ProviderError, ProviderInfo, from_ymd,
    text as json_text,
};
use crate::{
    http::{parse_json, send_ok},
    prospectus,
    ratelimit::RateLimiter,
};

const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const FUND_TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers_mf.json";

/// Formularios que interesan a un inversor (se descarta el ruido de Form 4, 144, etc.).
const FORMS: &[&str] = &[
    "10-K", "10-K/A", "10-Q", "10-Q/A", "8-K", "8-K/A", "20-F", "40-F", "6-K", "S-1", "S-3",
    "424B5", "DEF 14A", "SC 13D", "SC 13G",
];

#[derive(Debug, Clone)]
pub struct Company {
    pub cik: u64,
    pub name: String,
}

pub struct Sec {
    http: Client,
    info: ProviderInfo,
    user_agent: String,
    tickers: Cache<(), Arc<HashMap<String, Company>>>,
    /// `company_tickers_mf.json`: ticker de ETF/fondo -> CIK, serie y clase.
    fund_tickers: Cache<(), Arc<HashMap<String, FundSeries>>>,
    /// Límite por petición HTTP (la SEC permite 10/s; un método puede encadenar varias).
    limiter: RateLimiter,
}

impl Sec {
    /// `user_agent` debe llevar un contacto (`"invest-ai contacto@dominio.com"`).
    pub fn new(http: Client, user_agent: String) -> Self {
        Self {
            http,
            user_agent,
            limiter: RateLimiter::new(8.0, 3),
            tickers: Cache::builder()
                .time_to_live(Duration::from_secs(86_400))
                .max_capacity(1)
                .build(),
            fund_tickers: Cache::builder()
                .time_to_live(Duration::from_secs(86_400))
                .max_capacity(1)
                .build(),
            info: ProviderInfo {
                name: "sec",
                requires_key: false,
                has_key: false,
                capabilities: vec![Capability::News],
                classes: vec![AssetClass::Stock],
                // El límite oficial es 10/s; se deja margen.
                rate_per_sec: 8.0,
                burst: 8,
            },
        }
    }

    async fn get(&self, url: String) -> PResult<String> {
        self.limiter
            .acquire(Duration::from_secs(20))
            .await
            .map_err(|_| ProviderError::RateLimited { retry_after: None })?;
        send_ok(
            self.http
                .get(url)
                .header(header::USER_AGENT, &self.user_agent),
        )
        .await
    }

    async fn company(&self, id: &AssetId) -> PResult<Company> {
        self.company_by_symbol(&id.symbol).await
    }

    async fn company_by_symbol(&self, symbol: &str) -> PResult<Company> {
        let map = self
            .tickers
            .try_get_with((), async {
                let text = self.get(TICKERS_URL.into()).await?;
                parse_tickers(&text).map(Arc::new)
            })
            .await
            .map_err(|e: Arc<ProviderError>| match &*e {
                ProviderError::RateLimited { retry_after } => ProviderError::RateLimited {
                    retry_after: *retry_after,
                },
                other => ProviderError::Upstream(other.to_string()),
            })?;
        map.get(&symbol.to_ascii_uppercase())
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(format!("{symbol} no figura en la SEC")))
    }

    /// Últimos filings (más recientes primero).
    pub async fn filings(&self, id: &AssetId, limit: usize) -> PResult<Vec<NewsItem>> {
        if id.class != AssetClass::Stock {
            return Err(ProviderError::Unsupported);
        }
        let co = self.company(id).await?;
        let text = self
            .get(format!(
                "https://data.sec.gov/submissions/CIK{:010}.json",
                co.cik
            ))
            .await?;
        parse_submissions(&text, limit, Utc::now())
    }
}


// ---------------------------------------------------------------------------------------------
// Folletos de OPV
// ---------------------------------------------------------------------------------------------

/// Qué folleto se quiere de un emisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProspectusPick {
    /// El folleto definitivo más antiguo (el de la OPV).
    #[default]
    Initial,
    /// El definitivo más reciente (puede ser de una oferta posterior).
    Latest,
}

/// Formularios que contienen el folleto definitivo de una OPV: 424B4/424B1 (S-1, F-1) y 253G2
/// (circular de oferta de la Regulation A+).
pub const FINAL_FORMS: &[&str] = &["424B4", "424B1", "253G2"];
/// Borradores con precio orientativo, solo si aún no hay definitivo.
pub const DRAFT_FORMS: &[&str] = &["S-1/A", "F-1/A", "S-1", "F-1", "253G1"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilingRow {
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
    pub document: String,
}

/// Filas de un bloque columnar de `submissions` (`filings.recent` o un fichero de `filings.files`).
pub fn parse_filing_rows(block: &Value) -> Vec<FilingRow> {
    let col = |k: &str| block[k].as_array();
    let (Some(acc), Some(form), Some(date), Some(doc)) = (
        col("accessionNumber"),
        col("form"),
        col("filingDate"),
        col("primaryDocument"),
    ) else {
        return Vec::new();
    };
    (0..acc.len())
        .filter_map(|i| {
            let accession = acc[i].as_str()?.to_string();
            let form = form.get(i)?.as_str()?.to_string();
            let filed = NaiveDate::parse_from_str(date.get(i)?.as_str()?, "%Y-%m-%d").ok()?;
            let document = doc.get(i)?.as_str()?.to_string();
            let clean = accession.chars().all(|c| c.is_ascii_digit() || c == '-');
            (clean && safe_path(&document)).then_some(FilingRow { accession, form, filed, document })
        })
        .collect()
}

/// Elige el folleto. `(fila, preliminar)`.
pub fn pick_prospectus(rows: &[FilingRow], pick: ProspectusPick) -> Option<(FilingRow, bool)> {
    let mut finals: Vec<&FilingRow> = rows.iter().filter(|r| FINAL_FORMS.contains(&r.form.as_str())).collect();
    finals.sort_by(|a, b| (a.filed, &a.accession).cmp(&(b.filed, &b.accession)));
    let chosen = match pick {
        ProspectusPick::Initial => finals.first(),
        ProspectusPick::Latest => finals.last(),
    };
    if let Some(r) = chosen {
        return Some(((*r).clone(), false));
    }
    rows.iter()
        .filter(|r| DRAFT_FORMS.contains(&r.form.as_str()))
        .max_by(|a, b| (a.filed, &a.accession).cmp(&(b.filed, &b.accession)))
        .map(|r| (r.clone(), true))
}

/// Resultado de buscar un emisor en el índice de texto completo de EDGAR.
#[derive(Debug, Clone)]
pub struct FtsHit {
    pub cik: u64,
    pub name: String,
    pub form: String,
    pub filed: NaiveDate,
    pub accession: String,
    /// El índice trae SIC 6770 («Blank Checks»): señal fiable de SPAC, sin peticiones extra.
    pub blank_check: bool,
}

/// Respuesta de `efts.sec.gov/LATEST/search-index` (sin el texto de los documentos).
pub fn parse_fts(text: &str) -> PResult<Vec<FtsHit>> {
    let v: Value = parse_json(text)?;
    let hits = v["hits"]["hits"]
        .as_array()
        .ok_or_else(|| ProviderError::upstream("búsqueda EDGAR sin hits"))?;
    Ok(hits
        .iter()
        .filter_map(|h| {
            let s = &h["_source"];
            let cik: u64 = s["ciks"].get(0)?.as_str()?.parse().ok()?;
            let raw = s["display_names"].get(0)?.as_str()?;
            let name = raw.split("(CIK").next().unwrap_or(raw).trim().to_string();
            Some(FtsHit {
                cik,
                name,
                form: s["form"].as_str()?.to_string(),
                filed: NaiveDate::parse_from_str(s["file_date"].as_str()?, "%Y-%m-%d").ok()?,
                accession: s["adsh"].as_str()?.to_string(),
                blank_check: s["sics"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some("6770"))),
            })
        })
        .collect())
}

fn fts_url(q: Option<&str>, forms: &str, from: Option<NaiveDate>, to: Option<NaiveDate>, offset: usize) -> String {
    let mut u = url::Url::parse("https://efts.sec.gov/LATEST/search-index").expect("url fija");
    {
        let mut qp = u.query_pairs_mut();
        if let Some(q) = q {
            qp.append_pair("q", &format!("\"{q}\""));
        }
        qp.append_pair("forms", forms);
        if let (Some(f), Some(t)) = (from, to) {
            qp.append_pair("dateRange", "custom");
            qp.append_pair("startdt", &f.to_string());
            qp.append_pair("enddt", &t.to_string());
        }
        if offset > 0 {
            qp.append_pair("from", &offset.to_string());
        }
    }
    u.to_string()
}

/// Nombre apto para una consulta de frase exacta (sin comillas ni operadores).
fn fts_name(q: &str) -> Option<String> {
    let n: String = q
        .chars()
        .filter(|c| c.is_alphanumeric() || " .&,-'".contains(*c))
        .collect();
    let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    (n.len() >= 3 && n.len() <= 80).then_some(n)
}

impl Sec {
    /// CIK de un emisor dado como CIK numérico, ticker o nombre (búsqueda en EDGAR).
    pub async fn resolve_issuer(&self, query: &str) -> PResult<(Company, Option<String>)> {
        let q = query.trim();
        if q.is_empty() || q.len() > 80 {
            return Err(ProviderError::NotFound("emisor vacío o demasiado largo".into()));
        }
        if q.chars().all(|c| c.is_ascii_digit()) {
            let cik: u64 = q.parse().map_err(|_| ProviderError::NotFound("CIK no válido".into()))?;
            return Ok((Company { cik, name: String::new() }, None));
        }
        let looks_ticker = q.len() <= 10 && q.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if looks_ticker && let Ok(c) = self.company_by_symbol(&q.replace('.', "-")).await {
            return Ok((c, Some(q.to_ascii_uppercase())));
        }
        let name = fts_name(q).ok_or_else(|| ProviderError::NotFound("nombre de emisor no válido".into()))?;
        let text = self.get(fts_url(Some(&name), "424B4,424B1,S-1,F-1,S-1/A,F-1/A", None, None, 0)).await?;
        let hits = parse_fts(&text)?;
        let want = prospectus::normalize(&name);
        let hit = hits
            .iter()
            .find(|h| prospectus::normalize(&h.name).contains(&want))
            .ok_or_else(|| ProviderError::NotFound(format!("{name} no tiene folletos en EDGAR")))?;
        Ok((Company { cik: hit.cik, name: hit.name.clone() }, None))
    }

    /// Todas las presentaciones de un CIK (`recent` + ficheros históricos si el emisor es antiguo).
    async fn filing_rows(&self, cik: u64, need_old: bool) -> PResult<(String, Option<String>, Vec<FilingRow>)> {
        let text = self.get(format!("https://data.sec.gov/submissions/CIK{cik:010}.json")).await?;
        let v: Value = parse_json(&text)?;
        let name = v["name"].as_str().unwrap_or("").to_string();
        let ticker = v["tickers"].get(0).and_then(Value::as_str).map(str::to_string);
        let mut rows = parse_filing_rows(&v["filings"]["recent"]);
        let has_final = rows.iter().any(|r| FINAL_FORMS.contains(&r.form.as_str()));
        if need_old || !has_final {
            for f in v["filings"]["files"].as_array().into_iter().flatten().take(8) {
                let Some(n) = f["name"].as_str().filter(|n| safe_path(n) && !n.contains('/')) else {
                    continue;
                };
                let t = self.get(format!("https://data.sec.gov/submissions/{n}")).await?;
                rows.extend(parse_filing_rows(&parse_json::<Value>(&t)?));
            }
        }
        Ok((name, ticker, rows))
    }

    /// Localiza y descarga el folleto de un emisor y extrae sus secciones.
    pub async fn prospectus(
        &self,
        issuer: &str,
        pick: ProspectusPick,
        max_chars: usize,
    ) -> PResult<Prospectus> {
        let (co, ticker) = self.resolve_issuer(issuer).await?;
        // «Initial» necesita ver también los ficheros antiguos: el 424B4 de la OPV queda fuera de `recent`.
        let (name, listed, rows) = self.filing_rows(co.cik, pick == ProspectusPick::Initial).await?;
        let (row, preliminary) = pick_prospectus(&rows, pick)
            .ok_or_else(|| ProviderError::NotFound(format!("{issuer}: sin folleto S-1/F-1/424B4 en EDGAR")))?;
        let acc = row.accession.replace('-', "");
        let base = format!("https://www.sec.gov/Archives/edgar/data/{}/{acc}", co.cik);
        let url = format!("{base}/{}", row.document);
        let html = self.get(url.clone()).await?;
        let ex = tokio::task::spawn_blocking(move || prospectus::extract(&html, max_chars))
            .await
            .map_err(|e| ProviderError::upstream(format!("extracción interrumpida: {e}")))?;
        Ok(Prospectus {
            issuer: if name.is_empty() { co.name } else { name },
            cik: co.cik,
            ticker: ticker.or(listed),
            form: row.form,
            preliminary,
            filed_on: row.filed,
            accession: row.accession.clone(),
            url,
            index_url: format!("{base}/{}-index.htm", row.accession),
            offer_price: ex.offer_price,
            sections: ex.sections,
            missing: ex.missing,
            document_chars: ex.document_chars,
            source: "sec".into(),
            fetched_at: Utc::now(),
        })
    }

    /// Alternativa sin clave al calendario: folletos definitivos (424B4/424B1) presentados en el
    /// rango, del índice de texto completo de EDGAR. Incluye ofertas posteriores a la OPV y
    /// no trae precio ni símbolo: es un indicio de que hubo oferta, no un calendario completo.
    pub async fn recent_final_prospectuses(&self, from: NaiveDate, to: NaiveDate) -> PResult<Vec<IpoEntry>> {
        let text = self.get(fts_url(None, "424B4,424B1", Some(from), Some(to), 0)).await?;
        let now = Utc::now();
        Ok(parse_fts(&text)?
            .into_iter()
            .map(|h| IpoEntry {
                date: h.filed,
                symbol: None,
                name: h.name,
                exchange: None,
                status: IpoStatus::Priced,
                price_low: None,
                price_high: None,
                shares: None,
                total_value: None,
                spac_likely: h.blank_check,
                source: "sec".into(),
                source_url: Some(format!(
                    "https://www.sec.gov/Archives/edgar/data/{}/{}/",
                    h.cik,
                    h.accession.replace('-', "")
                )),
                as_of: now,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------------------------
// Folletos de ETF y fondos (497K, 485BPOS, 497)
// ---------------------------------------------------------------------------------------------

/// Serie y clase de un ETF en `company_tickers_mf.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FundSeries {
    pub cik: u64,
    pub series_id: String,
    pub class_id: String,
}

/// `company_tickers_mf.json`: `{"fields":["cik","seriesId","classId","symbol"],"data":[[…],…]}`.
pub fn parse_fund_tickers(text: &str) -> PResult<HashMap<String, FundSeries>> {
    let v: Value = parse_json(text)?;
    let fields: Vec<&str> = v["fields"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let col = |name: &str| fields.iter().position(|f| *f == name);
    let (Some(c_cik), Some(c_ser), Some(c_cls), Some(c_sym)) = (col("cik"), col("seriesId"), col("classId"), col("symbol")) else {
        return Err(ProviderError::upstream("company_tickers_mf sin las columnas esperadas"));
    };
    let mut out = HashMap::new();
    for row in v["data"].as_array().into_iter().flatten() {
        let (Some(cik), Some(series), Some(class), Some(sym)) = (
            row.get(c_cik).and_then(Value::as_u64),
            row.get(c_ser).and_then(Value::as_str),
            row.get(c_cls).and_then(Value::as_str),
            row.get(c_sym).and_then(Value::as_str),
        ) else {
            continue;
        };
        out.entry(sym.to_ascii_uppercase())
            .or_insert_with(|| FundSeries { cik, series_id: series.to_string(), class_id: class.to_string() });
    }
    Ok(out)
}

/// Una presentación del feed Atom de EDGAR (`browse-edgar?output=atom`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomFiling {
    pub accession: String,
    pub form: String,
    pub filed: NaiveDate,
}

fn xml_tag<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let i = s.find(&open)? + open.len();
    let j = s[i..].find(&format!("</{tag}>"))? + i;
    Some(s[i..j].trim())
}

/// (CIK del registrante, presentaciones más recientes primero y sin duplicados).
pub fn parse_filing_atom(text: &str) -> (Option<u64>, Vec<AtomFiling>) {
    let (head, entries) = text.split_once("<entry>").map_or((text, ""), |(h, rest)| (h, rest));
    let cik = xml_tag(head, "cik").and_then(|c| c.parse::<u64>().ok());
    let mut out: Vec<AtomFiling> = Vec::new();
    for e in entries.split("<entry>") {
        let (Some(acc), Some(form), Some(date)) = (xml_tag(e, "accession-number"), xml_tag(e, "filing-type"), xml_tag(e, "filing-date")) else {
            continue;
        };
        let Ok(filed) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else { continue };
        if !acc.chars().all(|c| c.is_ascii_digit() || c == '-') || out.iter().any(|f| f.accession == acc) {
            continue;
        }
        out.push(AtomFiling { accession: acc.to_string(), form: form.to_string(), filed });
    }
    out.sort_by(|a, b| b.filed.cmp(&a.filed).then_with(|| b.accession.cmp(&a.accession)));
    (cik, out)
}

/// Nombre del registrante (`conformed-name`) del feed Atom.
pub fn parse_atom_name(text: &str) -> Option<String> {
    let head = text.split("<entry>").next().unwrap_or(text);
    xml_tag(head, "conformed-name").map(|n| n.replace("&amp;", "&")).filter(|n| !n.is_empty())
}

/// Documento principal de una presentación: el `.htm` más grande de `index.json` (≤ `max_bytes`).
pub fn pick_main_document(index_json: &str, max_bytes: u64) -> Option<String> {
    let v: Value = serde_json::from_str(index_json).ok()?;
    v["directory"]["item"]
        .as_array()?
        .iter()
        .filter_map(|i| {
            let name = i["name"].as_str()?;
            let size: u64 = i["size"].as_str()?.trim().parse().ok()?;
            let lower = name.to_ascii_lowercase();
            ((lower.ends_with(".htm") || lower.ends_with(".html")) && !lower.contains("-index") && safe_path(name) && size > 0 && size <= max_bytes).then(|| (size, name.to_string()))
        })
        .max_by_key(|(size, _)| *size)
        .map(|(_, n)| n)
}

fn atom_url(key: &str, form: &str, count: usize) -> String {
    format!("https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={key}&type={form}&dateb=&owner=include&count={count}&output=atom")
}

/// Candidatos por orden: formulario y cuántos documentos se prueban. El 497K de un ETF cuelga a
/// menudo de un fideicomiso con decenas de fondos: se descartan los que no nombran el ticker.
const FUND_PLAN_SERIES: &[(&str, usize, u64)] = &[("497K", 5, 1_500_000), ("485BPOS", 1, 3_000_000)];
/// Fideicomisos de un solo fondo (SPY, QQQ...): su CIK es el del fondo.
const FUND_PLAN_DIRECT: &[(&str, usize, u64)] = &[("497K", 3, 1_500_000), ("485BPOS", 1, 3_000_000), ("497", 1, 3_000_000)];

impl Sec {
    async fn fund_series(&self, ticker: &str) -> PResult<Option<FundSeries>> {
        let map = self
            .fund_tickers
            .try_get_with((), async {
                let text = self.get(FUND_TICKERS_URL.into()).await?;
                parse_fund_tickers(&text).map(Arc::new)
            })
            .await
            .map_err(|e: Arc<ProviderError>| match &*e {
                ProviderError::RateLimited { retry_after } => ProviderError::RateLimited { retry_after: *retry_after },
                other => ProviderError::Upstream(other.to_string()),
            })?;
        Ok(map.get(ticker).cloned())
    }

    /// Folleto resumido (497K) o folleto (485BPOS / 497) de un ETF, con sus secciones clave.
    /// Ticker -> CIK, serie y clase por `company_tickers_mf.json`; sin él (SPY es un fideicomiso
    /// de unidades y no figura) se usa el CIK de `company_tickers.json`.
    pub async fn fund_prospectus(&self, id: &AssetId, max_chars: usize) -> PResult<Prospectus> {
        if !matches!(id.class, AssetClass::Etf | AssetClass::Fund) {
            return Err(ProviderError::Unsupported);
        }
        let ticker = super::plain_ticker(&id.symbol).to_ascii_uppercase();
        let (key, direct) = match self.fund_series(&ticker).await? {
            Some(s) => (s.class_id, false),
            None => (format!("{:010}", self.company_by_symbol(&ticker).await?.cik), true),
        };
        for (form, n, max_bytes) in if direct { FUND_PLAN_DIRECT } else { FUND_PLAN_SERIES } {
            let atom = self.get(atom_url(&key, form, 12)).await?;
            let (cik, filings) = parse_filing_atom(&atom);
            let registrant = parse_atom_name(&atom);
            let Some(cik) = cik else { continue };
            for f in filings.iter().filter(|f| f.form == *form).take(*n) {
                let acc = f.accession.replace('-', "");
                let base = format!("https://www.sec.gov/Archives/edgar/data/{cik}/{acc}");
                let index = match self.get(format!("{base}/index.json")).await {
                    Ok(t) => t,
                    Err(ProviderError::RateLimited { retry_after }) => return Err(ProviderError::RateLimited { retry_after }),
                    Err(_) => continue,
                };
                let Some(doc) = pick_main_document(&index, *max_bytes) else { continue };
                let url = format!("{base}/{doc}");
                let html = match self.get(url.clone()).await {
                    Ok(h) => h,
                    Err(ProviderError::RateLimited { retry_after }) => return Err(ProviderError::RateLimited { retry_after }),
                    Err(_) => continue,
                };
                let t = ticker.clone();
                let ex = tokio::task::spawn_blocking(move || crate::fund_prospectus::extract(&html, &t, max_chars))
                    .await
                    .map_err(|e| ProviderError::upstream(format!("extracción interrumpida: {e}")))?;
                if !ex.usable() || !(direct || ex.mentions_ticker) {
                    continue;
                }
                return Ok(Prospectus {
                    issuer: ex.fund_name.clone().or(registrant).unwrap_or_else(|| ticker.clone()),
                    cik,
                    ticker: Some(ticker.clone()),
                    form: f.form.clone(),
                    preliminary: false,
                    filed_on: f.filed,
                    accession: f.accession.clone(),
                    url,
                    index_url: format!("{base}/{}-index.htm", f.accession),
                    offer_price: None,
                    sections: ex.sections,
                    missing: ex.missing,
                    document_chars: ex.document_chars,
                    source: "sec".into(),
                    fetched_at: Utc::now(),
                });
            }
        }
        Err(ProviderError::NotFound(format!("{ticker}: sin folleto resumido utilizable en EDGAR")))
    }
}

#[async_trait]
impl Provider for Sec {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    async fn news(&self, q: &NewsQuery) -> PResult<Vec<NewsItem>> {
        match &q.id {
            Some(id) => self.filings(id, q.limit.clamp(1, 20)).await,
            None => Err(ProviderError::Unsupported),
        }
    }
}

pub fn parse_tickers(text: &str) -> PResult<HashMap<String, Company>> {
    let v: Value = parse_json(text)?;
    let obj = v
        .as_object()
        .ok_or_else(|| ProviderError::upstream("company_tickers no es un objeto"))?;
    let mut out = HashMap::with_capacity(obj.len());
    for e in obj.values() {
        let (Some(cik), Some(t)) = (e["cik_str"].as_u64(), e["ticker"].as_str()) else {
            continue;
        };
        out.entry(t.to_ascii_uppercase())
            .or_insert_with(|| Company {
                cik,
                name: e["title"].as_str().unwrap_or(t).to_string(),
            });
    }
    Ok(out)
}

fn safe_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 200
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

pub fn parse_submissions(text: &str, limit: usize, now: DateTime<Utc>) -> PResult<Vec<NewsItem>> {
    let v: Value = parse_json(text)?;
    let name = v["name"].as_str().unwrap_or("").to_string();
    let cik: u64 = v["cik"]
        .as_str()
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| ProviderError::upstream("submissions sin cik"))?;
    let r = &v["filings"]["recent"];
    let col = |k: &str| r[k].as_array();
    let (Some(acc), Some(date), Some(form)) =
        (col("accessionNumber"), col("filingDate"), col("form"))
    else {
        return Err(ProviderError::upstream("submissions sin filings recientes"));
    };
    let mut out = Vec::new();
    for (i, acc_v) in acc.iter().enumerate() {
        let (Some(acc_no), Some(form), Some(date_s)) = (
            acc_v.as_str(),
            form.get(i).and_then(Value::as_str),
            date.get(i).and_then(Value::as_str),
        ) else {
            continue;
        };
        if !FORMS.contains(&form) {
            continue;
        }
        let get = |k: &str| col(k).and_then(|a| a.get(i)).and_then(json_text);
        let Some(doc) = get("primaryDocument").filter(|d| safe_path(d)) else {
            continue;
        };
        let acc_clean = acc_no.replace('-', "");
        if !acc_clean.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let published = get("acceptanceDateTime")
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc))
            .or_else(|| from_ymd(date_s));
        let Some(published) = published.filter(|p| *p <= now + chrono::Duration::days(1)) else {
            continue;
        };
        let url = format!("https://www.sec.gov/Archives/edgar/data/{cik}/{acc_clean}/{doc}");
        let desc = get("primaryDocDescription").unwrap_or_else(|| form.to_string());
        let items = get("items");
        let summary = match items {
            Some(it) => format!("{desc} (apartados {it}). Presentado el {date_s}."),
            None => format!("{desc}. Presentado el {date_s}."),
        };
        out.push(NewsItem {
            id: crate::news::item_id(&url),
            title: format!("{name}: formulario {form} ante la SEC"),
            summary: Some(summary),
            url,
            publisher: Some("SEC EDGAR".into()),
            published_at: published,
            image_url: None,
            tickers: v["tickers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect(),
            sentiment: None,
            provider: "sec".into(),
            language: Some("en".into()),
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKERS: &str = include_str!("../../tests/fixtures/sec_company_tickers.json");
    const SUBS: &str = include_str!("../../tests/fixtures/sec_submissions_aapl.json");

    #[test]
    fn parses_ticker_map() {
        let m = parse_tickers(TICKERS).unwrap();
        let aapl = &m["AAPL"];
        assert_eq!(aapl.cik, 320_193);
        assert_eq!(aapl.name, "Apple Inc.");
        assert!(!m.contains_key("ZZZZZ"));
        assert!(parse_tickers("[]").is_err());
    }

    #[test]
    fn parses_filings_with_acceptance_time() {
        let now = DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let items = parse_submissions(SUBS, 10, now).unwrap();
        assert!(!items.is_empty() && items.len() <= 10);
        for n in &items {
            assert!(
                n.url
                    .starts_with("https://www.sec.gov/Archives/edgar/data/320193/"),
                "{}",
                n.url
            );
            assert_eq!(n.provider, "sec");
            assert!(n.title.starts_with("Apple Inc."));
            assert!(n.tickers.contains(&"AAPL".to_string()));
        }
        // Más recientes primero (como en el JSON de la SEC).
        assert!(
            items
                .windows(2)
                .all(|w| w[0].published_at >= w[1].published_at)
        );
        // Se filtra el ruido: el fixture empieza por formularios 4.
        assert!(!items.iter().any(|n| n.title.contains("formulario 4 ")));
    }

    #[test]
    fn rejects_hostile_paths() {
        assert!(!safe_path("../x"));
        assert!(!safe_path("a b"));
        assert!(!safe_path("x?y=1"));
        assert!(safe_path("xslF345X06/form4.xml"));
    }

    #[test]
    fn missing_sections_are_errors() {
        assert!(parse_submissions("{}", 5, Utc::now()).is_err());
    }
}
