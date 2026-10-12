//! Configuración del crate. `MarketConfig::from_env()` lee las variables de `.env.example`.
//! Las claves son `Secret`: ni `Debug` ni `Display` las muestran.

use std::time::Duration;

use crate::secret::Secret;

/// User-Agent de la SEC por defecto. La SEC exige un contacto real: si no se configura
/// `SEC_USER_AGENT` se avisa en el arranque.
pub const DEFAULT_SEC_UA: &str = "invest-ai contacto@example.com";

#[derive(Debug, Clone)]
pub struct MarketConfig {
    pub finnhub_key: Option<Secret>,
    pub alphavantage_key: Option<Secret>,
    pub fmp_key: Option<Secret>,
    pub twelvedata_key: Option<Secret>,
    pub fred_key: Option<Secret>,
    pub coingecko_key: Option<Secret>,
    pub newsapi_key: Option<Secret>,
    pub marketaux_key: Option<Secret>,
    pub cryptopanic_key: Option<Secret>,
    /// `User-Agent` con contacto para SEC EDGAR.
    pub sec_user_agent: String,
    /// Idiomas de las noticias de Google News (`en`, `es`).
    pub news_languages: Vec<String>,
    /// Tiempo máximo de cada petición HTTP.
    pub request_timeout: Duration,
    /// Activa los WebSockets (Binance, Finnhub). Con `false` todo se sondea.
    pub realtime: bool,
    /// Bases de Binance (sobrescribibles para pruebas).
    pub binance_rest_base: String,
    pub binance_ws_base: String,
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            finnhub_key: None,
            alphavantage_key: None,
            fmp_key: None,
            twelvedata_key: None,
            fred_key: None,
            coingecko_key: None,
            newsapi_key: None,
            marketaux_key: None,
            cryptopanic_key: None,
            sec_user_agent: DEFAULT_SEC_UA.into(),
            news_languages: vec!["en".into(), "es".into()],
            request_timeout: Duration::from_secs(10),
            realtime: true,
            binance_rest_base: crate::providers::binance::REST_BASE.into(),
            binance_ws_base: crate::providers::binance::WS_BASE.into(),
        }
    }
}

impl MarketConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Igual que `from_env` con una fuente de variables arbitraria (tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let key = |k: &str| get(k).and_then(Secret::new);
        let mut c = Self {
            finnhub_key: key("FINNHUB_API_KEY"),
            alphavantage_key: key("ALPHAVANTAGE_API_KEY"),
            fmp_key: key("FMP_API_KEY"),
            twelvedata_key: key("TWELVEDATA_API_KEY"),
            fred_key: key("FRED_API_KEY"),
            coingecko_key: key("COINGECKO_API_KEY"),
            newsapi_key: key("NEWSAPI_API_KEY"),
            marketaux_key: key("MARKETAUX_API_KEY"),
            cryptopanic_key: key("CRYPTOPANIC_API_KEY"),
            ..Self::default()
        };
        if let Some(ua) = get("SEC_USER_AGENT") {
            let ua = ua.trim().trim_matches(['"', '\'']).to_string();
            // Cabecera válida: ASCII imprimible, sin saltos de línea.
            if !ua.is_empty()
                && ua.len() <= 200
                && ua.chars().all(|ch| ch.is_ascii() && !ch.is_ascii_control())
            {
                c.sec_user_agent = ua;
            }
        }
        // Noticias en el idioma del LLM además de inglés.
        if let Some(l) = get("LLM_LANGUAGE") {
            let l = l.trim().to_ascii_lowercase();
            let l = l.split(['-', '_']).next().unwrap_or("").to_string();
            c.news_languages = if l == "en" || l.is_empty() {
                vec!["en".into()]
            } else {
                vec!["en".into(), l]
            };
        }
        if let Some(s) = get("MARKET_HTTP_TIMEOUT_SECS").and_then(|s| s.trim().parse::<u64>().ok())
        {
            c.request_timeout = Duration::from_secs(s.clamp(2, 60));
        }
        if let Some(v) = get("MARKET_REALTIME") {
            c.realtime = !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            );
        }
        c
    }

    /// Todas las claves presentes (para ocultarlas de mensajes de error).
    pub fn secrets(&self) -> Vec<Secret> {
        [
            &self.finnhub_key,
            &self.alphavantage_key,
            &self.fmp_key,
            &self.twelvedata_key,
            &self.fred_key,
            &self.coingecko_key,
            &self.newsapi_key,
            &self.marketaux_key,
            &self.cryptopanic_key,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }

    pub fn uses_default_sec_contact(&self) -> bool {
        self.sec_user_agent == DEFAULT_SEC_UA || self.sec_user_agent.contains("example.com")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> MarketConfig {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        MarketConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn empty_keys_are_none_and_debug_hides_values() {
        let c = env(&[
            ("FINNHUB_API_KEY", ""),
            ("FRED_API_KEY", "abcd1234efgh"),
            ("FMP_API_KEY", "   "),
        ]);
        assert!(c.finnhub_key.is_none() && c.fmp_key.is_none());
        assert!(c.fred_key.is_some());
        assert!(!format!("{c:?}").contains("abcd1234"));
        assert_eq!(c.secrets().len(), 1);
    }

    #[test]
    fn sec_ua_and_language() {
        let c = env(&[
            ("SEC_USER_AGENT", "\"invest-ai yo@correo.es\""),
            ("LLM_LANGUAGE", "es"),
        ]);
        assert_eq!(c.sec_user_agent, "invest-ai yo@correo.es");
        assert_eq!(c.news_languages, ["en", "es"]);
        assert!(!c.uses_default_sec_contact());
        // Un User-Agent con saltos de línea se descarta (inyección de cabeceras).
        let c = env(&[("SEC_USER_AGENT", "x\r\nHost: evil")]);
        assert!(c.uses_default_sec_contact());
        assert_eq!(env(&[("LLM_LANGUAGE", "en")]).news_languages, ["en"]);
    }

    #[test]
    fn realtime_switch_and_timeout() {
        assert!(!env(&[("MARKET_REALTIME", "off")]).realtime);
        assert!(env(&[]).realtime);
        assert_eq!(
            env(&[("MARKET_HTTP_TIMEOUT_SECS", "999")]).request_timeout,
            Duration::from_secs(60)
        );
    }
}
