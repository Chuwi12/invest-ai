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
    use super::*;

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
