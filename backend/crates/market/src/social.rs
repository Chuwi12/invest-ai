//! Redes sociales (Reddit y X): configuración, emparejado de menciones, presupuesto de lecturas
//! y utilidades comunes de los proveedores `providers::reddit` y `providers::x`.
//!
//! Las redes son OPINIÓN, no hechos: aquí solo se recogen publicaciones con su fecha real para
//! medir atención y tono. Todo está APAGADO por defecto: sin credenciales no se crea ningún
//! proveedor y no se hace ninguna petición. Ver `docs/notes/social.md`.

use std::{sync::Mutex, time::Duration};

use async_trait::async_trait;
use domain::{AssetClass, SocialPost, SocialSource};
use reqwest::RequestBuilder;
use sha2::{Digest, Sha256};

use crate::{
    http::{Body, send},
    providers::{PResult, ProviderError, plain_ticker},
    secret::Secret,
};

// ---------------------------------------------------------------------------------------------
// Configuración
// ---------------------------------------------------------------------------------------------

/// Subreddits por defecto. Los nombres en inglés son comunidades grandes y conocidas; los
/// españoles están ESCRITOS DE MEMORIA y no se han podido comprobar (no se hacen peticiones
/// reales al desarrollar): revísalos y ajústalos con `REDDIT_SUBREDDITS`.
pub const SUBS_GENERAL: &[&str] =
    &["stocks", "investing", "StockMarket", "wallstreetbets", "SecurityAnalysis", "ValueInvesting", "Dividends"];
pub const SUBS_ETF: &[&str] = &["ETFs", "Bogleheads"];
pub const SUBS_CRYPTO: &[&str] = &["CryptoCurrency", "Bitcoin", "ethereum", "CryptoMarkets"];
pub const SUBS_ES: &[&str] = &["SpainFIRE", "inversiones", "finanzas"];

pub const DEFAULT_REDDIT_BASE: &str = "https://oauth.reddit.com";
pub const DEFAULT_REDDIT_AUTH_BASE: &str = "https://www.reddit.com";
pub const DEFAULT_X_BASE: &str = "https://api.x.com";
/// Precio orientativo por post leído (tarifa de pago por uso de feb-2026; configurable).
pub const DEFAULT_X_COST_PER_READ_USD: f64 = 0.005;
/// La API de búsqueda reciente de X exige entre 10 y 100 resultados por petición.
pub const X_MIN_RESULTS: u32 = 10;
pub const X_MAX_RESULTS: u32 = 100;
/// La búsqueda reciente de X solo cubre los últimos 7 días.
pub const X_MAX_DAYS: u32 = 7;

#[derive(Debug, Clone)]
pub struct RedditConfig {
    pub client_id: Secret,
    pub client_secret: Secret,
    /// Reddit exige un User-Agent descriptivo con el usuario de Reddit.
    pub user_agent: String,
    /// `None` = lista por defecto según la clase del activo.
    pub subreddits: Option<Vec<String>>,
    pub api_base: String,
    pub auth_base: String,
}

#[derive(Debug, Clone)]
pub struct XConfig {
    pub bearer: Secret,
    /// Máximo de posts leídos al mes. Obligatorio: sin él X no se activa.
    pub monthly_read_budget: u32,
    pub accounts: Vec<String>,
    pub search_all: bool,
    pub langs: Vec<String>,
    /// Resultados que se piden por petición (10-100).
    pub results_per_request: u32,
    pub cost_per_read_usd: f64,
    pub api_base: String,
}

#[derive(Debug, Clone)]
pub struct SocialConfig {
    pub reddit: Option<RedditConfig>,
    pub x: Option<XConfig>,
    /// Ventana de consulta en días.
    pub days: u32,
    /// Extractos que se enseñan.
    pub top_n: usize,
    /// No se vuelve a consultar un activo antes de estos minutos (caché + ahorro de presupuesto de X).
    pub refresh_minutes: u64,
    /// Avisos de configuración (p. ej. «X no se activa: falta X_MONTHLY_READ_BUDGET»).
    pub notes: Vec<String>,
}

impl Default for SocialConfig {
    fn default() -> Self {
        Self { reddit: None, x: None, days: 7, top_n: 5, refresh_minutes: 360, notes: Vec::new() }
    }
}

fn list(v: &str) -> Vec<String> {
    v.split([',', ';', ' ']).map(|s| s.trim().trim_start_matches('@').trim_start_matches("r/")).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn valid_name(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

impl SocialConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        let key = |k: &str| get(k).and_then(Secret::new);
        if let Some(d) = get("SOCIAL_DAYS").and_then(|s| s.trim().parse::<u32>().ok()) {
            c.days = d.clamp(1, 30);
        }
        if let Some(n) = get("SOCIAL_TOP_N").and_then(|s| s.trim().parse::<usize>().ok()) {
            c.top_n = n.clamp(1, 5);
        }
        if let Some(m) = get("SOCIAL_REFRESH_MINUTES").and_then(|s| s.trim().parse::<u64>().ok()) {
            c.refresh_minutes = m.clamp(15, 10_080);
        }

        // Reddit: hacen falta las tres variables.
        let (id, secret) = (key("REDDIT_CLIENT_ID"), key("REDDIT_CLIENT_SECRET"));
        let ua = get("REDDIT_USER_AGENT").map(|u| u.trim().trim_matches(['"', '\'']).to_string()).filter(|u| !u.is_empty());
        match (id, secret, ua) {
            (Some(client_id), Some(client_secret), Some(user_agent)) => {
                if user_agent.len() > 200 || !user_agent.chars().all(|ch| ch.is_ascii() && !ch.is_ascii_control()) {
                    c.notes.push("Reddit no se activa: REDDIT_USER_AGENT no es una cabecera válida".into());
                } else {
                    let subreddits = get("REDDIT_SUBREDDITS").map(|s| list(&s).into_iter().filter(|s| valid_name(s, 21)).collect::<Vec<_>>()).filter(|v| !v.is_empty());
                    c.reddit = Some(RedditConfig {
                        client_id,
                        client_secret,
                        user_agent,
                        subreddits,
                        api_base: DEFAULT_REDDIT_BASE.into(),
                        auth_base: DEFAULT_REDDIT_AUTH_BASE.into(),
                    });
                }
            }
            (None, None, None) => {}
            _ => c.notes.push("Reddit no se activa: hacen falta REDDIT_CLIENT_ID, REDDIT_CLIENT_SECRET y REDDIT_USER_AGENT".into()),
        }

        // X: token + presupuesto obligatorio + (cuentas o búsqueda general).
        if let Some(bearer) = key("X_BEARER_TOKEN") {
            let budget = get("X_MONTHLY_READ_BUDGET").and_then(|s| s.trim().parse::<u32>().ok()).filter(|b| *b > 0);
            let accounts: Vec<String> = get("X_ACCOUNTS").map(|s| list(&s)).unwrap_or_default().into_iter().filter(|a| valid_name(a, 15)).collect();
            let search_all = get("X_SEARCH_ALL").is_some_and(|v| truthy(&v));
            match budget {
                None => c.notes.push("X no se activa: X_MONTHLY_READ_BUDGET es obligatorio (nº máximo de posts leídos al mes)".into()),
                Some(_) if accounts.is_empty() && !search_all => {
                    c.notes.push("X no se activa: define X_ACCOUNTS (cuentas que eliges tú) o X_SEARCH_ALL=1".into());
                }
                Some(monthly_read_budget) => {
                    let langs = get("X_LANGS").map(|s| list(&s)).filter(|v| !v.is_empty()).unwrap_or_else(|| vec!["en".into(), "es".into()]);
                    let langs = langs.into_iter().filter(|l| l.len() <= 3 && l.chars().all(|ch| ch.is_ascii_lowercase())).collect();
                    let results_per_request = get("X_RESULTS_PER_REQUEST")
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(20)
                        .clamp(X_MIN_RESULTS, X_MAX_RESULTS);
                    let cost_per_read_usd = get("X_COST_PER_READ_USD")
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .filter(|x| x.is_finite() && *x >= 0.0)
                        .unwrap_or(DEFAULT_X_COST_PER_READ_USD);
                    c.x = Some(XConfig {
                        bearer,
                        monthly_read_budget,
                        accounts,
                        search_all,
                        langs,
                        results_per_request,
                        cost_per_read_usd,
                        api_base: DEFAULT_X_BASE.into(),
                    });
                }
            }
        }
        c
    }

    pub fn enabled(&self) -> bool {
        self.reddit.is_some() || self.x.is_some()
    }

    pub fn secrets(&self) -> Vec<Secret> {
        let mut v = Vec::new();
        if let Some(r) = &self.reddit {
            v.push(r.client_id.clone());
            v.push(r.client_secret.clone());
        }
        if let Some(x) = &self.x {
            v.push(x.bearer.clone());
        }
        v
    }
}

// ---------------------------------------------------------------------------------------------
// Emparejado de menciones
// ---------------------------------------------------------------------------------------------

/// Tickers que además son palabras corrientes: se exige cashtag (`$ALL`) o el nombre.
const COMMON_WORD_TICKERS: &[&str] = &[
    "ALL", "ARE", "BIG", "CAN", "CAR", "DD", "FOR", "FUN", "GO", "HAS", "KEY", "LOW", "NEW", "NOW", "ONE", "OPEN", "OUT", "PLAY", "REAL",
    "RUN", "SEE", "TRUE", "WELL", "YOU", "LINK", "EDIT", "BEST", "CASH", "COST", "FAST", "FREE", "GOOD", "HOME", "LOVE", "MAIN", "MORE",
    "SAFE", "SUN", "TOP", "UP", "WORK", "PEAK", "RARE", "TECH", "GAIN", "BILL", "FUND", "GAME", "HUGE", "LIFE", "LIVE", "NEXT", "POOL",
];

const LEGAL_SUFFIXES: &[&str] = &[
    "inc", "incorporated", "corp", "corporation", "co", "company", "ltd", "limited", "plc", "sa", "nv", "ag", "se", "holdings", "holding",
    "group", "the", "class", "a", "b", "c", "common", "stock", "ordinary", "shares", "adr", "etf", "trust", "fund", "usd", "lp",
];

/// Cómo se reconoce un activo en el texto de una publicación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matcher {
    pub ticker: String,
    /// Ticker corto o palabra común: solo cuenta con cashtag o por el nombre.
    pub ambiguous: bool,
    /// Nombre del activo sin sufijos legales, en minúsculas (≥ 4 caracteres) o `None`.
    pub name: Option<String>,
}

impl Matcher {
    pub fn new(class: AssetClass, symbol: &str, name: Option<&str>) -> Self {
        let mut ticker = plain_ticker(symbol).to_ascii_uppercase();
        if class == AssetClass::Crypto {
            ticker = ticker.strip_suffix("-USD").unwrap_or(&ticker).to_string();
        }
        let ambiguous = ticker.chars().count() <= 2 || COMMON_WORD_TICKERS.contains(&ticker.as_str());
        Self { name: name.and_then(clean_name), ambiguous, ticker }
    }

    /// `true` si hay algo con lo que buscar sin ruido: un ticker no ambiguo, o un nombre.
    pub fn searchable(&self) -> bool {
        !self.ambiguous || self.name.is_some()
    }

    /// Texto de la consulta a Reddit (lenguaje de búsqueda por defecto de Reddit).
    pub fn reddit_query(&self) -> Option<String> {
        if !self.searchable() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.ambiguous {
            parts.push(format!("\"${}\"", self.ticker));
            parts.push(format!("\"{}\"", self.ticker));
        } else {
            parts.push(format!("\"${}\"", self.ticker));
        }
        if let Some(n) = &self.name {
            parts.push(format!("\"{n}\""));
        }
        Some(parts.join(" OR "))
    }

    /// ¿Menciona el texto este activo? Reglas (conservadoras, para no inflar las menciones):
    /// 1. cashtag `$TICKER` (cualquier capitalización) siempre vale;
    /// 2. el nombre como palabra(s) completa(s), sin distinguir mayúsculas;
    /// 3. el ticker suelto SOLO si no es ambiguo: con ≥ 4 letras en cualquier capitalización, con 3 en mayúsculas.
    pub fn matches(&self, text: &str) -> bool {
        self.has_cashtag(text) || self.name_in(text) || (!self.ambiguous && self.bare_ticker_in(text))
    }

    pub fn has_cashtag(&self, text: &str) -> bool {
        let t = text.to_ascii_uppercase();
        let needle = format!("${}", self.ticker);
        word_find(&t, &needle, true)
    }

    fn name_in(&self, text: &str) -> bool {
        self.name.as_ref().is_some_and(|n| word_find(&fold(text), n, false))
    }

    fn bare_ticker_in(&self, text: &str) -> bool {
        if self.ticker.chars().count() >= 4 {
            word_find(&text.to_ascii_uppercase(), &self.ticker, false)
        } else {
            word_find(text, &self.ticker, false)
        }
    }
}

fn fold(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `needle` aparece en `hay` con límites de palabra. `dollar`: la aguja empieza por `$`.
fn word_find(hay: &str, needle: &str, dollar: bool) -> bool {
    let _ = dollar;
    let mut start = 0;
    while let Some(i) = hay[start..].find(needle) {
        let a = start + i;
        let b = a + needle.len();
        let before_ok = hay[..a].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '$'));
        let after_ok = hay[b..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = a + needle.chars().next().map_or(1, char::len_utf8);
        if start >= hay.len() {
            break;
        }
    }
    false
}

/// «Apple Inc.» → «apple»; «Ford Motor Company» → «ford motor». `None` si queda < 4 caracteres.
pub fn clean_name(name: &str) -> Option<String> {
    let folded = fold(name);
    let mut words: Vec<&str> = folded.split(' ').collect();
    while words.len() > 1 && words.last().is_some_and(|w| LEGAL_SUFFIXES.contains(w) || w.chars().count() == 1) {
        words.pop();
    }
    let n = words.join(" ");
    (n.chars().count() >= 4 && n.chars().any(char::is_alphabetic)).then_some(n)
}

// ---------------------------------------------------------------------------------------------
// Utilidades comunes
// ---------------------------------------------------------------------------------------------

/// Seudónimo estable de un autor (hash truncado con sal fija). No guarda el nombre de usuario.
pub fn hash_author(source: SocialSource, author: &str) -> String {
    let d = Sha256::digest(format!("invest-ai-social-v1|{}|{}", source.as_str(), author.trim().to_lowercase()).as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Recorta a `max` caracteres (por caracteres, no por bytes) añadiendo «…».
pub fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let mut t: String = one.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// Resultado de consultar una red para un activo.
#[derive(Debug, Clone, Default)]
pub struct SocialBatch {
    pub posts: Vec<SocialPost>,
    /// Hay más resultados de los recogidos: las menciones son una cota inferior.
    pub truncated: bool,
    /// Peticiones HTTP realizadas (0 si salió de la caché).
    pub requests: u32,
    /// Posts leídos facturables (solo X).
    pub reads: u32,
    /// X: se alcanzó el presupuesto mensual y no se hizo (todo) lo pedido.
    pub budget_exhausted: bool,
}

#[derive(Debug, Clone)]
pub struct SocialQuery {
    pub class: AssetClass,
    pub symbol: String,
    pub name: Option<String>,
    pub days: u32,
}

impl SocialQuery {
    pub fn matcher(&self) -> Matcher {
        Matcher::new(self.class, &self.symbol, self.name.as_deref())
    }
}

#[async_trait]
pub trait SocialProvider: Send + Sync {
    fn source(&self) -> SocialSource;
    async fn fetch(&self, q: &SocialQuery) -> PResult<SocialBatch>;
}

/// Envía con reintentos ante 429: respeta `Retry-After` (tope 60 s) y, sin él, espera
/// `base · 2^intento`. Tras `retries` reintentos devuelve el `RateLimited`.
pub async fn send_with_backoff(build: impl Fn() -> RequestBuilder, base: Duration, retries: u32) -> PResult<Body> {
    let mut attempt = 0u32;
    loop {
        match send(build()).await {
            Err(ProviderError::RateLimited { retry_after }) if attempt < retries => {
                let wait = retry_after.unwrap_or_else(|| base * 2u32.saturating_pow(attempt)).min(Duration::from_secs(60));
                tracing::debug!(espera_ms = wait.as_millis() as u64, "429: se reintenta tras esperar");
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Presupuesto de lecturas de X
// ---------------------------------------------------------------------------------------------

/// Contador mensual de posts leídos. Lo implementa el servidor sobre SQLite (sobrevive a reinicios).
#[async_trait]
pub trait ReadBudget: Send + Sync {
    /// Reserva hasta `want` lecturas del mes `month` (`YYYY-MM`) sin pasar de `cap`. Devuelve las
    /// concedidas (0 = agotado). La reserva es atómica: dos llamadas concurrentes no pueden pasarse del tope.
    async fn reserve(&self, month: &str, want: u32, cap: u32) -> Result<u32, String>;
    /// Ajusta una reserva al consumo real (`actual <= reserved`).
    async fn settle(&self, month: &str, reserved: u32, actual: u32);
    async fn used(&self, month: &str) -> u32;
}

/// Presupuesto en memoria (tests y utilidades).
#[derive(Default)]
pub struct MemoryBudget(Mutex<std::collections::HashMap<String, u32>>);

#[async_trait]
impl ReadBudget for MemoryBudget {
    async fn reserve(&self, month: &str, want: u32, cap: u32) -> Result<u32, String> {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let used = m.entry(month.to_string()).or_insert(0);
        let grant = want.min(cap.saturating_sub(*used));
        *used += grant;
        Ok(grant)
    }

    async fn settle(&self, month: &str, reserved: u32, actual: u32) {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(u) = m.get_mut(month) {
            *u = u.saturating_sub(reserved.saturating_sub(actual));
        }
    }

    async fn used(&self, month: &str) -> u32 {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).get(month).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> SocialConfig {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        SocialConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn everything_is_off_by_default() {
        let c = env(&[]);
        assert!(!c.enabled() && c.notes.is_empty());
    }

    #[test]
    fn reddit_needs_all_three_variables() {
        let c = env(&[("REDDIT_CLIENT_ID", "abcdef"), ("REDDIT_CLIENT_SECRET", "ghijkl")]);
        assert!(c.reddit.is_none() && c.notes.len() == 1);
        let c = env(&[("REDDIT_CLIENT_ID", "abcdef"), ("REDDIT_CLIENT_SECRET", "ghijkl"), ("REDDIT_USER_AGENT", "linux:invest-ai:0.1 (by /u/alguien)")]);
        assert!(c.reddit.is_some() && c.enabled());
        assert!(!format!("{c:?}").contains("ghijkl"), "las claves no salen en Debug");
    }

    #[test]
    fn x_requires_budget_and_accounts_or_search_all() {
        let base = [("X_BEARER_TOKEN", "tok123456")];
        let c = env(&base);
        assert!(c.x.is_none() && c.notes[0].contains("X_MONTHLY_READ_BUDGET"));
        let c = env(&[("X_BEARER_TOKEN", "tok123456"), ("X_MONTHLY_READ_BUDGET", "0")]);
        assert!(c.x.is_none(), "presupuesto 0 = no se activa");
        let c = env(&[("X_BEARER_TOKEN", "tok123456"), ("X_MONTHLY_READ_BUDGET", "500")]);
        assert!(c.x.is_none() && c.notes[0].contains("X_ACCOUNTS"));
        let c = env(&[("X_BEARER_TOKEN", "tok123456"), ("X_MONTHLY_READ_BUDGET", "500"), ("X_ACCOUNTS", "@uno, dos;tres x-mal!")]);
        let x = c.x.unwrap();
        assert_eq!(x.accounts, ["uno", "dos", "tres"], "x-mal! no es un usuario válido");
        assert!(!x.search_all);
        let c = env(&[("X_BEARER_TOKEN", "tok123456"), ("X_MONTHLY_READ_BUDGET", "500"), ("X_SEARCH_ALL", "1")]);
        assert!(c.x.unwrap().search_all);
        // Sin token, el presupuesto solo no activa nada.
        assert!(!env(&[("X_MONTHLY_READ_BUDGET", "500"), ("X_ACCOUNTS", "uno")]).enabled());
    }

    #[test]
    fn ambiguous_tickers_need_cashtag_or_name() {
        let f = Matcher::new(AssetClass::Stock, "F", Some("Ford Motor Company"));
        assert!(f.ambiguous && f.name.as_deref() == Some("ford motor"));
        assert!(!f.matches("I think F is going to rally"), "F suelta no cuenta");
        assert!(!f.matches("Press F to pay respects"));
        assert!(f.matches("Bought more $F today"));
        assert!(f.matches("bought more $f today"));
        assert!(f.matches("Ford Motor reports earnings"));
        assert!(!f.matches("the ford is a river crossing"), "«ford» solo no es el nombre completo");
        let t = Matcher::new(AssetClass::Stock, "T", Some("AT&T Inc."));
        assert!(!t.matches("T is flat") && t.matches("$T dividend"));
        // Sin nombre y ambiguo: no hay forma segura de buscar.
        let ai = Matcher::new(AssetClass::Stock, "AI", None);
        assert!(!ai.searchable() && ai.reddit_query().is_none());
        assert!(!ai.matches("AI is eating the world") && ai.matches("$AI guidance"));
        let all = Matcher::new(AssetClass::Stock, "ALL", Some("Allstate Corp"));
        assert!(all.ambiguous && !all.matches("buy ALL the dips") && all.matches("Allstate raised rates"));
    }

    #[test]
    fn unambiguous_tickers_match_as_words() {
        let a = Matcher::new(AssetClass::Stock, "AAPL", Some("Apple Inc."));
        assert!(!a.ambiguous);
        assert!(a.matches("AAPL earnings tomorrow") && a.matches("aapl to the moon") && a.matches("Apple is a great company"));
        assert!(!a.matches("AAPLX fund") && !a.matches("pineapple prices"), "límites de palabra");
        let amd = Matcher::new(AssetClass::Stock, "AMD", Some("Advanced Micro Devices, Inc."));
        assert!(amd.matches("AMD up 5%") && !amd.matches("amd is a word here"), "3 letras: solo en mayúsculas");
        assert!(amd.matches("$amd"));
        let san = Matcher::new(AssetClass::Stock, "SAN.MC", Some("Banco Santander, S.A."));
        assert_eq!(san.ticker, "SAN");
        assert_eq!(san.name.as_deref(), Some("banco santander"));
        let btc = Matcher::new(AssetClass::Crypto, "BTC-USD", Some("Bitcoin"));
        assert_eq!(btc.ticker, "BTC");
        assert!(btc.matches("Bitcoin rompe máximos") && btc.matches("BTC a 100k"));
        let link = Matcher::new(AssetClass::Crypto, "LINK", Some("Chainlink"));
        assert!(link.ambiguous && !link.matches("click the link below") && link.matches("Chainlink oracles"));
    }

    #[test]
    fn query_for_reddit() {
        let a = Matcher::new(AssetClass::Stock, "AAPL", Some("Apple Inc."));
        assert_eq!(a.reddit_query().unwrap(), "\"$AAPL\" OR \"AAPL\" OR \"apple\"");
        let f = Matcher::new(AssetClass::Stock, "F", Some("Ford Motor Company"));
        assert_eq!(f.reddit_query().unwrap(), "\"$F\" OR \"ford motor\"");
    }

    #[test]
    fn author_hash_is_stable_and_not_the_name() {
        let h = hash_author(SocialSource::Reddit, "Some_User");
        assert_eq!(h, hash_author(SocialSource::Reddit, "some_user"));
        assert_ne!(h, hash_author(SocialSource::X, "some_user"));
        assert_eq!(h.len(), 16);
        assert!(!h.contains("user"));
    }

    #[test]
    fn clip_counts_characters() {
        assert_eq!(clip("ñandú  ñandú", 5), "ñandú…");
        assert_eq!(clip("corto", 50), "corto");
    }

    #[tokio::test]
    async fn memory_budget_never_exceeds_cap() {
        let b = MemoryBudget::default();
        assert_eq!(b.reserve("2026-10", 60, 100).await.unwrap(), 60);
        assert_eq!(b.reserve("2026-10", 60, 100).await.unwrap(), 40);
        assert_eq!(b.reserve("2026-10", 10, 100).await.unwrap(), 0);
        b.settle("2026-10", 40, 15).await;
        assert_eq!(b.used("2026-10").await, 75);
        assert_eq!(b.reserve("2026-11", 10, 100).await.unwrap(), 10, "cada mes cuenta aparte");
    }
}
