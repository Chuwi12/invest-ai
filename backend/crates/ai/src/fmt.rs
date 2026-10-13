//! Formato numérico compacto y estable para prompts y razones. Siempre punto decimal (el
//! contexto se compara textualmente con lo que dice el modelo, ver `fidelity`).

/// Precio con decimales adaptados a la magnitud (BTC 64231.5, EURUSD 1.0842, SHIB 0.0000089).
pub fn price(x: f64) -> String {
    if !x.is_finite() {
        return "n/d".into();
    }
    let a = x.abs();
    if a >= 10.0 {
        trim_zeros(format!("{x:.2}"))
    } else if a >= 0.01 {
        trim_zeros(format!("{x:.4}"))
    } else if a == 0.0 {
        "0".into()
    } else {
        trim_zeros(format!("{x:.8}"))
    }
}

/// Precio para el usuario: en castellano usa coma decimal y punto de millares (81.059,79); en
/// inglés es igual que `price`. Solo para texto que ve el usuario: el contexto del modelo usa `price`.
pub fn price_l(lang: crate::lang::Lang, x: f64) -> String {
    let p = price(x);
    if !matches!(lang, crate::lang::Lang::Es) || !x.is_finite() {
        return p;
    }
    let (neg, body) = p.strip_prefix('-').map_or((false, p.as_str()), |b| (true, b));
    let (int, frac) = body.split_once('.').map_or((body, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (k, c) in int.chars().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 && int.len() > 4 {
            grouped.push('.');
        }
        grouped.push(c);
    }
    format!("{}{}{}", if neg { "-" } else { "" }, grouped, frac.map(|f| format!(",{f}")).unwrap_or_default())
}

/// Nombre de visualización de un proveedor de datos (los mismos que muestra la UI en
/// `frontend/src/lib/sources.ts`): `binance` → «Binance», `yahoo` → «Yahoo Finance». Un id que no se
/// conoce se deja como viene salvo que sea todo minúsculas, en cuyo caso se capitaliza («fixture» →
/// «Fixture»). Solo para texto que ve el usuario: `SourceRef.provider` conserva el id, que la UI usa
/// para elegir el enlace de verificación.
pub fn provider_name(id: &str) -> String {
    let key = id.trim().to_lowercase();
    let key = key.strip_prefix("provider:").unwrap_or(&key);
    let known = match key {
        "yahoo" | "yahoo_rss" => "Yahoo Finance",
        "binance" => "Binance",
        "coingecko" => "CoinGecko",
        "finnhub" => "Finnhub",
        "alphavantage" => "Alpha Vantage",
        "fmp" => "Financial Modeling Prep",
        "twelvedata" => "Twelve Data",
        "fred" => "FRED",
        "sec" | "edgar" => "SEC EDGAR",
        "gdelt" => "GDELT",
        "google_news" => "Google News",
        "newsapi" => "NewsAPI",
        "marketaux" => "Marketaux",
        "cryptopanic" => "CryptoPanic",
        "ecb" => "BCE",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let id = id.trim();
    if id.chars().any(char::is_uppercase) {
        return id.to_string();
    }
    let mut cs = id.chars();
    cs.next().map(|c| c.to_uppercase().chain(cs).collect()).unwrap_or_default()
}

/// Fecha corta legible: «12 oct 2026» / «12 Oct 2026».
pub fn date_l(lang: crate::lang::Lang, d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    const ES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
    const EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = d.month0() as usize;
    format!("{} {} {}", d.day(), if matches!(lang, crate::lang::Lang::Es) { ES[m] } else { EN[m] }, d.year())
}

/// Número genérico con 2 decimales (1 si ≥ 100) sin ceros finales.
pub fn num(x: f64) -> String {
    if !x.is_finite() {
        return "n/d".into();
    }
    if x.abs() >= 100.0 { trim_zeros(format!("{x:.1}")) } else { trim_zeros(format!("{x:.2}")) }
}

/// Fracción → porcentaje con un decimal (0.1234 → "12.3%").
pub fn pct(frac: f64) -> String {
    if !frac.is_finite() {
        return "n/d".into();
    }
    format!("{:.1}%", frac * 100.0)
}

/// Fracción → porcentaje con DOS decimales (0.0003 → "0.03%"): para ratios pequeños (gastos de un
/// ETF, rentabilidad por dividendo) que con un decimal se redondean a «0.0%».
pub fn pct2(frac: f64) -> String {
    if !frac.is_finite() {
        return "n/d".into();
    }
    format!("{:.2}%", frac * 100.0)
}

/// Igual que `pct` pero con signo explícito (+1.2% / -3.4%).
pub fn pct_signed(frac: f64) -> String {
    if !frac.is_finite() {
        return "n/d".into();
    }
    format!("{:+.1}%", frac * 100.0)
}

/// Cifras grandes: 1.23T, 4.5B, 120M, 35K.
pub fn big(x: f64) -> String {
    if !x.is_finite() {
        return "n/d".into();
    }
    let a = x.abs();
    let (v, s) = if a >= 1e12 {
        (x / 1e12, "T")
    } else if a >= 1e9 {
        (x / 1e9, "B")
    } else if a >= 1e6 {
        (x / 1e6, "M")
    } else if a >= 1e3 {
        (x / 1e3, "K")
    } else {
        return trim_zeros(format!("{x:.2}"));
    };
    let body = if v.abs() >= 100.0 { format!("{v:.0}") } else { trim_zeros(format!("{v:.2}")) };
    format!("{body}{s}")
}

fn trim_zeros(s: String) -> String {
    if s.contains('.') {
        let t = s.trim_end_matches('0').trim_end_matches('.');
        if t.is_empty() || t == "-" { "0".into() } else { t.to_string() }
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spanish_prices_use_dot_thousands_and_decimal_comma() {
        use crate::lang::Lang;
        assert_eq!(super::price_l(Lang::Es, 81059.79), "81.059,79");
        assert_eq!(super::price_l(Lang::Es, 2057.45), "2057,45");
        assert_eq!(super::price_l(Lang::Es, 1234567.0), "1.234.567");
        assert_eq!(super::price_l(Lang::Es, 0.5), "0,5");
        assert_eq!(super::price_l(Lang::En, 81059.79), "81059.79");
    }

    use super::*;

    #[test]
    fn provider_names_are_capitalised_for_the_user() {
        assert_eq!(provider_name("binance"), "Binance");
        assert_eq!(provider_name("yahoo"), "Yahoo Finance");
        assert_eq!(provider_name("Yahoo"), "Yahoo Finance");
        assert_eq!(provider_name("yahoo_rss"), "Yahoo Finance");
        assert_eq!(provider_name("alphavantage"), "Alpha Vantage");
        assert_eq!(provider_name("provider:marketaux"), "Marketaux");
        assert_eq!(provider_name("fixture"), "Fixture");
        // Lo que ya trae mayúsculas (un medio como Reuters) no se toca.
        assert_eq!(provider_name("Reuters"), "Reuters");
        assert_eq!(provider_name("Financial Times"), "Financial Times");
        assert_eq!(provider_name(""), "");
    }

    #[test]
    fn prices_adapt_decimals() {
        assert_eq!(price(64231.5), "64231.5");
        assert_eq!(price(212.3), "212.3");
        assert_eq!(price(212.0), "212");
        assert_eq!(price(1.08421), "1.0842");
        assert_eq!(price(5.1), "5.1");
        assert_eq!(price(0.0842), "0.0842");
        assert_eq!(price(0.0000089), "0.0000089");
        assert_eq!(price(f64::NAN), "n/d");
        assert_eq!(price(0.0), "0");
    }

    #[test]
    fn percentages() {
        assert_eq!(pct(0.1234), "12.3%");
        assert_eq!(pct_signed(0.05), "+5.0%");
        assert_eq!(pct_signed(-0.0312), "-3.1%");
        assert_eq!(pct(f64::INFINITY), "n/d");
        assert_eq!(pct2(0.0003), "0.03%");
        assert_eq!(pct2(f64::NAN), "n/d");
    }

    #[test]
    fn big_numbers() {
        assert_eq!(big(3.21e12), "3.21T");
        assert_eq!(big(4.5e9), "4.5B");
        assert_eq!(big(120.4e6), "120M");
        assert_eq!(big(35_000.0), "35K");
        assert_eq!(big(12.0), "12");
        assert_eq!(big(-2.5e9), "-2.5B");
    }

    #[test]
    fn generic_numbers() {
        assert_eq!(num(28.456), "28.46");
        assert_eq!(num(150.26), "150.3");
        assert_eq!(num(1.0), "1");
    }
}
