//! Validación de símbolos de usuario y URLs públicas de verificación de cada dato.
//!
//! Todo símbolo que llega de fuera pasa por `validate` antes de tocar un proveedor; después los
//! proveedores lo codifican (percent-encoding) al meterlo en una ruta o consulta. Nunca se
//! construye una URL a partir de texto del usuario más allá del símbolo.

use domain::{AssetId, DomainError};

/// Caracteres que pueden aparecer en un símbolo real: letras, dígitos y `. - ^ = _ /`.
pub fn validate_symbol(symbol: &str) -> Result<(), DomainError> {
    let ok = !symbol.is_empty()
        && symbol.len() <= 40
        && symbol.chars().all(|c| c.is_ascii_alphanumeric() || ".-^=_/".contains(c))
        // Sin segmentos de ruta especiales (`..`, `.`) ni barras al borde o repetidas.
        && !symbol.contains("..")
        && !symbol.contains("//")
        && !symbol.starts_with(['/', '.'])
        && !symbol.ends_with('/');
    if ok {
        Ok(())
    } else {
        Err(DomainError::InvalidInput(format!(
            "símbolo no válido: {}",
            symbol.chars().take(20).collect::<String>()
        )))
    }
}

pub fn validate(id: &AssetId) -> Result<(), DomainError> {
    validate_symbol(&id.symbol)
}

/// Percent-encoding de un segmento de ruta.
pub fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// Página pública donde el usuario puede comprobar el dato de `provider` sobre `id`.
/// `None` si el proveedor no tiene una página por activo.
pub fn verify_url(provider: &str, id: &AssetId) -> Option<String> {
    let sym = &id.symbol;
    match provider {
        "yahoo" => {
            let s = crate::providers::yahoo::to_yahoo_symbol(id)?;
            Some(format!("https://finance.yahoo.com/quote/{}", enc(&s)))
        }
        "binance" => {
            let base = sym.to_ascii_uppercase();
            Some(format!(
                "https://www.binance.com/en/trade/{}_USDT",
                enc(&base)
            ))
        }
        "coingecko" => {
            let coin = crate::providers::coingecko::known_coin_id(sym).map_or_else(
                || format!("https://www.coingecko.com/en/search?query={}", enc(sym)),
                |c| format!("https://www.coingecko.com/en/coins/{c}"),
            );
            Some(coin)
        }
        "sec" => Some(format!(
            "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={}&type=&dateb=&owner=include&count=40",
            enc(&crate::providers::plain_ticker(sym))
        )),
        // Finnhub no tiene página pública por activo (`finnhub.io/quote/X` redirige a la portada,
        // medido el 14-oct-2026) y su API exige clave: no hay enlace que el usuario pueda abrir.
        "finnhub" => None,
        "fred" => Some(format!("https://fred.stlouisfed.org/series/{}", enc(sym))),
        // `twelvedata.com/markets/<símbolo>` da 404 (medido el 14-oct-2026) y no hay otra página
        // pública por activo; la API exige clave. Sin enlace antes que un enlace roto.
        "twelvedata" => None,
        "fmp" => Some(format!(
            "https://site.financialmodelingprep.com/financial-summary/{}",
            enc(sym)
        )),
        "alphavantage" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::AssetClass;

    #[test]
    fn accepts_real_symbols() {
        for s in [
            "AAPL",
            "SAN.MC",
            "BRK-B",
            "^GSPC",
            "EURUSD=X",
            "GC=F",
            "BTC",
            "AAPL261218C00200000",
        ] {
            assert!(validate_symbol(s).is_ok(), "{s}");
        }
    }

    #[test]
    fn rejects_injection() {
        for s in [
            "",
            "AAPL?x=1",
            "a b",
            "../../etc",
            "AAPL\n",
            "http://evil",
            "a#b",
            "a%2fb",
            "é",
            &"A".repeat(41),
        ] {
            assert!(validate_symbol(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn verify_urls_are_encoded() {
        let id = AssetId::new(AssetClass::Index, "^GSPC");
        assert_eq!(
            verify_url("yahoo", &id).unwrap(),
            "https://finance.yahoo.com/quote/%5EGSPC"
        );
        let btc = AssetId::new(AssetClass::Crypto, "btc");
        assert_eq!(
            verify_url("binance", &btc).unwrap(),
            "https://www.binance.com/en/trade/BTC_USDT"
        );
        assert_eq!(
            verify_url("coingecko", &btc).unwrap(),
            "https://www.coingecko.com/en/coins/bitcoin"
        );
    }
}
