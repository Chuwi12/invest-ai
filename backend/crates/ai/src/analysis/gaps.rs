//! `dataWarnings`: solo carencias REALES del snapshot (lo que `snapshot.missing` declara y los
//! campos que de verdad llegaron vacíos). Nunca sugerencias del modelo del tipo «no hay datos de
//! competidores»: eso no es una carencia de los datos sino una lista de deseos.

use domain::{AssetClass, AssetSnapshot};

use crate::{
    lang::{Lang, tr},
    scoring::Scorecard,
};

pub fn real_gaps(snap: &AssetSnapshot, card: &Scorecard, lang: Lang) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        let s = s.trim().to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    for m in &snap.missing {
        push(m.clone());
    }
    match &snap.quote {
        None => push(tr!(lang, "Sin cotización en vivo del activo.", "No live quote for the asset.")),
        Some(q) => {
            if q.delay_secs.is_some_and(|d| d >= 60) {
                push(tr!(
                    lang,
                    "La cotización llega con {} s de retraso ({}).",
                    "The quote is delayed by {} s ({}).",
                    q.delay_secs.unwrap_or(0),
                    crate::fmt::provider_name(&q.source)
                ));
            }
            if q.delay_secs.is_none() {
                push(tr!(lang, "No se conoce el retraso de la cotización ({}).", "The quote delay is unknown ({}).", crate::fmt::provider_name(&q.source)));
            }
        }
    }
    match &snap.indicators {
        None => push(tr!(
            lang,
            "Sin indicadores técnicos: no hay histórico de velas diarias.",
            "No technical indicators: no daily candle history."
        )),
        Some(i) => {
            if i.sma200.is_none() {
                push(tr!(lang, "Sin SMA200: historial de precios insuficiente.", "No SMA200: insufficient price history."));
            }
            if i.atr14.is_none() {
                push(tr!(lang, "Sin ATR14: no se puede medir la volatilidad diaria.", "No ATR14: daily volatility cannot be measured."));
            }
            if i.supports.is_empty() && i.resistances.is_empty() {
                push(tr!(lang, "Sin soportes ni resistencias detectados.", "No supports or resistances detected."));
            }
        }
    }
    match &snap.fundamentals {
        None => {
            if matches!(snap.summary.class, AssetClass::Stock | AssetClass::Etf | AssetClass::Fund) {
                push(tr!(lang, "Sin datos fundamentales del activo.", "No fundamentals for the asset."));
            }
        }
        Some(f) => {
            if snap.summary.class == AssetClass::Stock {
                let checks: [(bool, &str, &str); 5] = [
                    (f.pe_trailing.is_none() && f.pe_forward.is_none(), "PER", "P/E"),
                    (f.free_cash_flow.is_none(), "flujo de caja libre", "free cash flow"),
                    (f.profit_margin.is_none() && f.operating_margin.is_none(), "márgenes", "margins"),
                    (f.revenue_growth_yoy.is_none(), "crecimiento de ingresos", "revenue growth"),
                    (f.analyst_target_mean.is_none(), "consenso de analistas", "analyst consensus"),
                ];
                for (missing, es, en) in checks {
                    if missing {
                        push(tr!(lang, "Los fundamentales no incluyen {}.", "The fundamentals do not include {}.", lang.t(es, en)));
                    }
                }
            }
        }
    }
    if snap.news.is_empty() {
        push(tr!(lang, "Sin noticias recientes del activo.", "No recent news for the asset."));
    }
    if snap.macro_context.as_ref().is_none_or(|m| m.indicators.is_empty()) {
        push(tr!(lang, "Sin contexto macroeconómico.", "No macroeconomic context."));
    }
    for g in &card.gaps {
        push(g.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixture, scoring};

    #[test]
    fn only_real_gaps_never_wishlists() {
        let snap = fixture::stock_snapshot();
        let card = scoring::score(&snap, Lang::Es);
        let g = real_gaps(&snap, &card, Lang::Es);
        assert!(g.iter().all(|x| !x.to_lowercase().contains("competidores")), "{g:?}");

        let mut snap = fixture::stock_snapshot();
        snap.missing.push("fundamentals: yahoo 429".into());
        snap.news.clear();
        snap.fundamentals.as_mut().unwrap().free_cash_flow = None;
        snap.indicators.as_mut().unwrap().atr14 = None;
        let card = scoring::score(&snap, Lang::Es);
        let g = real_gaps(&snap, &card, Lang::Es);
        assert!(g.contains(&"fundamentals: yahoo 429".to_string()));
        assert!(g.iter().any(|x| x.contains("noticias")));
        assert!(g.iter().any(|x| x.contains("flujo de caja libre")));
        assert!(g.iter().any(|x| x.contains("ATR14")));
        assert!(g.iter().all(|x| !x.contains("Método")), "la nota de método va aparte");
    }
}
