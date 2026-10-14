//! Convierte un `AssetSnapshot` (más su puntuación) en el bloque de texto que ve el LLM.
//!
//! Objetivos: que sea **compacto** (cifras redondeadas, nada que no aporte), **trazable** (cada
//! sección con su fuente y marca de tiempo; noticias numeradas `[N1]…` con fecha, medio y
//! sentimiento) y que **quepa** en el presupuesto de tokens: si no cabe, se degrada por niveles
//! (menos noticias, sin resúmenes, sin extras) antes de recortar a la fuerza.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use domain::{
    AssetClass, AssetSnapshot, Fundamentals, Indicators, NewsItem, Quote, SourceRef, Trend,
};

use crate::{
    derived, fmt,
    lang::{Lang, tr},
    scoring::{Scorecard, verdict_name},
    sentiment,
    tokens::{estimate_tokens, truncate_chars, truncate_to_tokens},
};

#[derive(Debug, Clone)]
pub struct ContextOptions {
    /// Presupuesto máximo del bloque en tokens (estimados).
    pub max_tokens: usize,
    pub lang: Lang,
}

/// Noticias que el modelo puede citar, en orden de numeración (`N1` = primera).
#[derive(Debug, Clone, Default)]
pub struct NewsRegistry {
    items: Vec<NewsItem>,
}

impl NewsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Etiqueta de la noticia (`N3`); la registra si es nueva. Deduplica por id o URL.
    pub fn tag_of(&mut self, item: &NewsItem) -> String {
        let pos = self.items.iter().position(|i| i.id == item.id || i.url == item.url);
        let idx = match pos {
            Some(p) => p,
            None => {
                self.items.push(item.clone());
                self.items.len() - 1
            }
        };
        format!("N{}", idx + 1)
    }

    /// Busca por etiqueta: acepta `N3`, `[N3]` o `n3`.
    pub fn get(&self, tag: &str) -> Option<&NewsItem> {
        let n = parse_tag(tag)?;
        self.items.get(n.checked_sub(1)?)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[NewsItem] {
        &self.items
    }

    /// Referencias de fuente (para `Analysis.sources`) de las etiquetas dadas.
    pub fn source_refs<'a>(&self, tags: impl IntoIterator<Item = &'a str>) -> Vec<SourceRef> {
        tags.into_iter().filter_map(|t| self.get(t)).map(news_source).collect()
    }
}

pub fn news_source(n: &NewsItem) -> SourceRef {
    SourceRef {
        title: n.title.clone(),
        url: Some(n.url.clone()),
        provider: n.publisher.clone().unwrap_or_else(|| n.provider.clone()),
        published_at: Some(n.published_at),
    }
}

/// `N12` / `[N12]` / `n12` → 12.
pub fn parse_tag(tag: &str) -> Option<usize> {
    let t = tag.trim().trim_matches(['[', ']']).trim();
    let rest = t.strip_prefix('N').or_else(|| t.strip_prefix('n'))?;
    if rest.is_empty() || rest.len() > 4 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// Citas `[N3]`, `[N1, N4]` o `[N1][N2]` presentes en un texto (números de noticia).
pub fn extract_citations(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else { break };
        let inner = &after[..end];
        // Solo contenido con pinta de cita: «N3», «N1, N2», «N1-N3» no se interpreta.
        if inner.len() <= 40 {
            for part in inner.split([',', ';', ' ']).filter(|p| !p.is_empty()) {
                if let Some(n) = parse_tag(part) {
                    out.push(n);
                }
            }
        }
        rest = &after[end + 1..];
    }
    out
}

/// Resultado de construir el contexto.
#[derive(Debug, Clone)]
pub struct BuiltContext {
    pub text: String,
    /// Noticias citables en este bloque (`N1…`).
    pub news: NewsRegistry,
    pub tokens: usize,
    /// Qué se recortó para caber (vacío si cupo entero).
    pub trimmed: Vec<String>,
}

struct Level {
    max_news: usize,
    summary_chars: usize,
    extras: bool,
    max_macro: usize,
    max_reasons: usize,
}

const LEVELS: [Level; 6] = [
    Level { max_news: 10, summary_chars: 200, extras: true, max_macro: usize::MAX, max_reasons: 8 },
    Level { max_news: 10, summary_chars: 100, extras: true, max_macro: usize::MAX, max_reasons: 8 },
    Level { max_news: 8, summary_chars: 0, extras: true, max_macro: usize::MAX, max_reasons: 5 },
    Level { max_news: 6, summary_chars: 0, extras: false, max_macro: 8, max_reasons: 4 },
    Level { max_news: 4, summary_chars: 0, extras: false, max_macro: 6, max_reasons: 3 },
    Level { max_news: 3, summary_chars: 0, extras: false, max_macro: 4, max_reasons: 2 },
];

/// Qué partes del snapshot se incluyen. Cada etapa del análisis pide solo lo que necesita.
#[derive(Debug, Clone, Copy)]
pub struct Sections {
    pub quote: bool,
    pub indicators: bool,
    pub fundamentals: bool,
    pub news: bool,
    pub macro_ctx: bool,
    pub gaps: bool,
    /// Máscara de filas de la tabla de puntuaciones (`Sections::S_*`); 0 = sin tabla.
    pub scores: u8,
    /// Veredicto cuantitativo provisional y sus razones.
    pub verdict: bool,
    /// Grupos de «Cifras derivadas» (`derived::*`); 0 = sin sección.
    pub derived: u8,
}

impl Sections {
    pub const S_TECHNICAL: u8 = 1;
    pub const S_MOMENTUM: u8 = 2;
    pub const S_VALUATION: u8 = 4;
    pub const S_FUNDAMENTAL: u8 = 8;
    pub const S_SENTIMENT: u8 = 16;
    pub const S_MACRO: u8 = 32;
    pub const S_RISK: u8 = 64;
    pub const S_ALL: u8 = 127;

    pub const ALL: Self = Self {
        quote: true,
        indicators: true,
        fundamentals: true,
        news: true,
        macro_ctx: true,
        gaps: true,
        scores: Self::S_ALL,
        verdict: true,
        derived: derived::ALL,
    };

    const NONE: Self = Self {
        quote: false,
        indicators: false,
        fundamentals: false,
        news: false,
        macro_ctx: false,
        gaps: true,
        scores: 0,
        verdict: false,
        derived: 0,
    };

    /// Lo mínimo que necesita el juez y los abogados: cotización, puntuaciones y huecos.
    pub fn judge() -> Self {
        Self {
            quote: true,
            scores: Self::S_ALL,
            verdict: true,
            derived: derived::ANALYST | derived::LEVELS | derived::RANGE | derived::VALUATION,
            ..Self::NONE
        }
    }
    pub fn technical() -> Self {
        Self {
            quote: true,
            indicators: true,
            scores: Self::S_TECHNICAL | Self::S_MOMENTUM,
            derived: derived::DETAIL | derived::RANGE | derived::TECH,
            ..Self::NONE
        }
    }
    pub fn fundamental() -> Self {
        Self {
            quote: true,
            fundamentals: true,
            scores: Self::S_FUNDAMENTAL | Self::S_VALUATION,
            derived: derived::ANALYST | derived::RANGE | derived::VALUATION,
            ..Self::NONE
        }
    }
    pub fn sentiment() -> Self {
        Self { news: true, scores: Self::S_SENTIMENT, ..Self::NONE }
    }
    pub fn macro_risk() -> Self {
        Self {
            quote: true,
            indicators: true,
            fundamentals: true,
            macro_ctx: true,
            scores: Self::S_MACRO | Self::S_RISK,
            derived: derived::RANGE | derived::TECH,
            ..Self::NONE
        }
    }
}

/// Construye el contexto del activo. `registry` es la numeración de noticias ya en uso (vacía
/// para un análisis; compartida entre activos y herramientas en el chat).
pub fn build_context(
    snap: &AssetSnapshot,
    card: &Scorecard,
    opts: &ContextOptions,
    registry: &mut NewsRegistry,
) -> BuiltContext {
    build_context_sections(snap, card, opts, registry, &Sections::ALL)
}

/// Como `build_context`, solo con las partes de `sections`.
pub fn build_context_sections(
    snap: &AssetSnapshot,
    card: &Scorecard,
    opts: &ContextOptions,
    registry: &mut NewsRegistry,
    sections: &Sections,
) -> BuiltContext {
    let mut last: Option<(String, NewsRegistry)> = None;
    for (n, level) in LEVELS.iter().enumerate() {
        let mut reg = registry.clone();
        let text = render(snap, card, opts.lang, level, &mut reg, sections);
        let tokens = estimate_tokens(&text);
        if tokens <= opts.max_tokens {
            *registry = reg.clone();
            let trimmed = if n == 0 {
                vec![]
            } else {
                vec![tr!(
                    opts.lang,
                    "contexto reducido al nivel {} de {} por presupuesto de tokens",
                    "context reduced to level {} of {} to fit the token budget",
                    n,
                    LEVELS.len() - 1
                )]
            };
            return BuiltContext { text, news: reg, tokens, trimmed };
        }
        last = Some((text, reg));
    }
    // Ni el nivel mínimo cabe: recorte duro (se pierde el final: noticias y huecos de datos).
    let (text, reg) = last.expect("LEVELS no está vacío");
    let text = truncate_to_tokens(&text, opts.max_tokens);
    let tokens = estimate_tokens(&text);
    *registry = reg.clone();
    let trimmed = vec![tr!(
        opts.lang,
        "contexto recortado a la fuerza a {} tokens",
        "context hard-truncated to {} tokens",
        opts.max_tokens
    )];
    BuiltContext { text, news: reg, tokens, trimmed }
}

// ---------------------------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------------------------

fn ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn trend_name(t: Trend) -> &'static str {
    match t {
        Trend::StrongUp => "strong_up",
        Trend::Up => "up",
        Trend::Sideways => "sideways",
        Trend::Down => "down",
        Trend::StrongDown => "strong_down",
    }
}

/// Filas `| etiqueta | valor |`; las filas sin ningún dato se omiten.
#[derive(Default)]
struct Table {
    rows: Vec<(String, String)>,
}

impl Table {
    fn row(&mut self, label: &str, pieces: Vec<Option<String>>) {
        let vals: Vec<String> = pieces.into_iter().flatten().collect();
        if !vals.is_empty() {
            self.rows.push((label.to_string(), vals.join(", ")));
        }
    }

    fn one(&mut self, label: &str, v: Option<String>) {
        self.row(label, vec![v]);
    }

    fn render(&self, out: &mut String, header: (&str, &str)) {
        if self.rows.is_empty() {
            return;
        }
        let _ = writeln!(out, "| {} | {} |\n|---|---|", header.0, header.1);
        for (k, v) in &self.rows {
            let _ = writeln!(out, "| {k} | {v} |");
        }
    }
}

fn lab(label: &str, v: Option<f64>, f: impl Fn(f64) -> String) -> Option<String> {
    v.filter(|x| x.is_finite()).map(|x| format!("{label} {}", f(x)))
}

fn val(v: Option<f64>, f: impl Fn(f64) -> String) -> Option<String> {
    v.filter(|x| x.is_finite()).map(f)
}

fn section(out: &mut String, title: &str) {
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    let _ = writeln!(out, "## {title}");
}

fn render(
    snap: &AssetSnapshot,
    card: &Scorecard,
    l: Lang,
    lv: &Level,
    reg: &mut NewsRegistry,
    sec: &Sections,
) -> String {
    let mut out = String::with_capacity(4096);
    let s = &snap.summary;

    // Cabecera
    let _ = writeln!(out, "# {} ({}) — {}", s.name, s.id, s.class.as_str());
    let mut meta = Vec::new();
    if let Some(e) = &s.exchange {
        meta.push(e.clone());
    }
    if let Some(c) = &s.currency {
        meta.push(format!("{} {c}", l.t("moneda", "currency")));
    }
    if let Some(c) = &s.country {
        meta.push(c.clone());
    }
    if let Some(f) = &snap.fundamentals {
        if let Some(x) = &f.sector {
            meta.push(format!("{} {x}", l.t("sector", "sector")));
        }
        if let Some(x) = &f.industry {
            meta.push(x.clone());
        }
    }
    if !meta.is_empty() {
        let _ = writeln!(out, "{}", meta.join(" · "));
    }
    let _ = writeln!(
        out,
        "{}",
        tr!(
            l,
            "Datos generados el {} (todas las horas en UTC).",
            "Data generated at {} (all times UTC).",
            ts(snap.generated_at)
        )
    );

    if sec.quote
        && let Some(q) = &snap.quote
    {
        render_quote(&mut out, q, snap.generated_at, l);
    }
    if sec.indicators
        && let Some(i) = &snap.indicators
    {
        render_indicators(&mut out, i, snap.quote.as_ref().map(|q| q.price), l, lv);
    }
    if sec.fundamentals
        && let Some(f) = &snap.fundamentals
    {
        render_fundamentals(&mut out, f, snap, l, lv);
    }
    if sec.derived != 0 {
        // Con la tabla de indicadores delante no se repiten ATR %, volatilidad ni volumen relativo.
        let d = derived::render(snap, l, sec.derived, !lv.extras, sec.indicators && snap.indicators.is_some());
        if !d.is_empty() {
            out.push('\n');
            out.push_str(&d);
        }
    }
    if sec.scores != 0 || sec.verdict {
        render_scores(&mut out, card, l, lv, sec);
    }
    if sec.news {
        render_news(&mut out, &snap.news, snap.generated_at, l, lv, reg);
    }
    if sec.macro_ctx
        && let Some(m) = &snap.macro_context
    {
        render_macro(&mut out, m, l, lv);
    }

    // Huecos de datos: obligan al modelo a decirlo en lugar de inventar.
    let mut gaps: Vec<&str> = snap.missing.iter().map(|s| s.as_str()).collect();
    gaps.extend(card.gaps.iter().map(|s| s.as_str()));
    if sec.gaps && !gaps.is_empty() {
        section(&mut out, l.t("Datos no disponibles", "Missing data"));
        for g in gaps {
            let _ = writeln!(out, "- {g}");
        }
    }
    out
}

fn render_quote(out: &mut String, q: &Quote, now: DateTime<Utc>, l: Lang) {
    let cur = q.currency.as_deref().unwrap_or("");
    let age = (now - q.timestamp).num_seconds().max(0);
    let state = format!("{:?}", q.market_state).to_lowercase();
    let delay = q
        .delay_secs
        .map(|d| format!("{} {d} s", l.t("retraso", "delay")))
        .unwrap_or_else(|| l.t("retraso desconocido", "delay unknown").to_string());
    section(
        out,
        &format!(
            "{} [{}: {} · {} · {} · {} {}]",
            l.t("Cotización", "Quote"),
            l.t("fuente", "source"),
            q.source,
            ts(q.timestamp),
            delay,
            l.t("mercado", "market"),
            state
        ),
    );
    let mut t = Table::default();
    t.one(
        l.t("Precio", "Price"),
        Some(format!("{} {cur}", fmt::price(q.price)).trim().to_string()),
    );
    t.row(
        l.t("Variación del día", "Day change"),
        // `change_pct` está en puntos porcentuales (1.2 = +1,2 %), `pct_signed` espera una fracción.
        vec![val(q.change_pct, |c| fmt::pct_signed(c / 100.0)), val(q.change, |c| format!("({})", fmt::price(c)))],
    );
    t.row(
        "Open / High / Low / Prev close",
        vec![
            lab("open", q.open, fmt::price),
            lab("high", q.high, fmt::price),
            lab("low", q.low, fmt::price),
            lab("prev", q.prev_close, fmt::price),
        ],
    );
    t.one("Volume", val(q.volume, fmt::big));
    t.one(l.t("Capitalización", "Market cap"), val(q.market_cap, fmt::big));
    t.one(l.t("Antigüedad del dato", "Data age"), Some(format!("{age} s")));
    t.render(out, (l.t("Dato", "Item"), l.t("Valor", "Value")));
}

fn render_indicators(out: &mut String, i: &Indicators, price: Option<f64>, l: Lang, lv: &Level) {
    let asof = i.as_of.map(|t| format!(" [{}]", ts(t))).unwrap_or_default();
    section(
        out,
        &format!(
            "{}{asof}",
            l.t("Indicadores técnicos (velas diarias)", "Technical indicators (daily candles)")
        ),
    );
    let mut t = Table::default();
    t.one(l.t("Tendencia", "Trend"), i.trend.map(|x| trend_name(x).to_string()));
    t.row(
        "SMA20 / SMA50 / SMA200",
        vec![
            lab("SMA20", i.sma20, fmt::price),
            lab("SMA50", i.sma50, fmt::price),
            lab("SMA200", i.sma200, fmt::price),
        ],
    );
    if lv.extras {
        t.row(
            "EMA12 / EMA26",
            vec![lab("EMA12", i.ema12, fmt::price), lab("EMA26", i.ema26, fmt::price)],
        );
        t.row(
            "Bollinger (20, 2)",
            vec![
                lab("low", i.bb_lower, fmt::price),
                lab("mid", i.bb_middle, fmt::price),
                lab("up", i.bb_upper, fmt::price),
            ],
        );
    }
    t.one("RSI14", val(i.rsi14, |v| format!("{v:.1}")));
    t.row(
        "MACD",
        vec![
            lab("macd", i.macd, fmt::num),
            lab("signal", i.macd_signal, fmt::num),
            lab("hist", i.macd_hist, fmt::num),
        ],
    );
    let atr_pct = i
        .atr14
        .zip(price)
        .filter(|(_, p)| *p > 0.0)
        .map(|(a, p)| format!("({} of price)", fmt::pct(a / p)));
    t.row("ATR14", vec![val(i.atr14, fmt::price), atr_pct]);
    t.one(
        l.t("Volatilidad 30d (anualizada)", "Volatility 30d (annualised)"),
        val(i.volatility30d, fmt::pct),
    );
    t.one(
        l.t("Caída máxima 1 año", "Max drawdown 1y"),
        val(i.max_drawdown1y, |v| fmt::pct_signed(-v.abs())),
    );
    t.row(
        l.t("Rendimientos", "Returns"),
        vec![
            lab("1w", i.return1w, fmt::pct_signed),
            lab("1m", i.return1m, fmt::pct_signed),
            lab("3m", i.return3m, fmt::pct_signed),
            lab("YTD", i.return_ytd, fmt::pct_signed),
            lab("1y", i.return1y, fmt::pct_signed),
        ],
    );
    let below = i
        .dist_from_high52w
        .map(|d| format!("({} {})", fmt::pct(d.abs()), l.t("bajo el máximo", "below the high")));
    t.row(
        l.t("Rango 52 semanas", "52-week range"),
        vec![lab("low", i.low52w, fmt::price), lab("high", i.high52w, fmt::price), below],
    );
    if lv.extras {
        t.row(
            "Volume",
            vec![
                lab("avg20", i.volume_avg20, fmt::big),
                lab("relative", i.relative_volume, fmt::num),
            ],
        );
    }
    let list = |v: &[f64]| {
        let s: Vec<String> = v.iter().filter(|x| x.is_finite()).map(|x| fmt::price(*x)).collect();
        (!s.is_empty()).then(|| s.join(" / "))
    };
    t.one(
        l.t("Soportes (de mayor a menor)", "Supports (high to low)"),
        list(&sorted_desc(&i.supports)),
    );
    t.one(
        l.t("Resistencias (de menor a mayor)", "Resistances (low to high)"),
        list(&sorted_asc(&i.resistances)),
    );
    t.render(out, (l.t("Indicador", "Indicator"), l.t("Valor", "Value")));
}

fn sorted_desc(v: &[f64]) -> Vec<f64> {
    let mut v = v.to_vec();
    v.sort_by(|a, b| b.total_cmp(a));
    v
}

fn sorted_asc(v: &[f64]) -> Vec<f64> {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

fn render_fundamentals(
    out: &mut String,
    f: &Fundamentals,
    snap: &AssetSnapshot,
    l: Lang,
    lv: &Level,
) {
    let src = if f.sources.is_empty() {
        String::new()
    } else {
        format!("{}: {}", l.t("fuente", "source"), f.sources.join(", "))
    };
    let asof = f.as_of.map(|t| format!(" · {}", ts(t))).unwrap_or_default();
    let head = l.t("Fundamentales", "Fundamentals");
    if src.is_empty() && asof.is_empty() {
        section(out, head);
    } else {
        section(out, &format!("{head} [{src}{asof}]"));
    }
    if lv.extras
        && let Some(d) = &f.description {
            let _ = writeln!(out, "{}", truncate_chars(d, 240));
        }
    // Las cuentas (ingresos, flujo de caja, caja, deuda) van en `financialCurrency`, que en un ADR
    // no es la de cotización: se rotulan con su moneda para que no se comparen con el precio (R-12).
    let price_cur = f
        .currency
        .as_deref()
        .or(snap.quote.as_ref().and_then(|q| q.currency.as_deref()))
        .or(snap.summary.currency.as_deref());
    let fin_cur = f
        .extra
        .get("financialCurrency")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty() && price_cur.is_some_and(|p| !p.eq_ignore_ascii_case(c)));
    let amount = |v: f64| match fin_cur {
        Some(c) => format!("{} {c}", fmt::big(v)),
        None => fmt::big(v),
    };
    let mut t = Table::default();
    if let (Some(fc), Some(pc)) = (fin_cur, price_cur) {
        t.one(
            l.t("Monedas", "Currencies"),
            Some(tr!(
                l,
                "ingresos, flujo de caja, caja y deuda en {}; precio, capitalización y objetivos de analistas en {}",
                "revenue, free cash flow, cash and debt in {}; price, market cap and analyst targets in {}",
                fc,
                pc
            )),
        );
    }
    t.row(
        l.t("Valoración", "Valuation"),
        vec![
            lab("EV", f.enterprise_value, fmt::big),
            lab("P/E trailing", f.pe_trailing, fmt::num),
            lab("P/E forward", f.pe_forward, fmt::num),
            lab("PEG", f.peg, fmt::num),
            lab("P/B", f.price_to_book, fmt::num),
            lab("P/S", f.price_to_sales, fmt::num),
            lab("EV/EBITDA", f.ev_to_ebitda, fmt::num),
        ],
    );
    t.row(
        "EPS",
        vec![lab("trailing", f.eps_trailing, fmt::num), lab("forward", f.eps_forward, fmt::num)],
    );
    t.row(
        l.t("Resultados", "Earnings"),
        vec![
            lab("revenue TTM", f.revenue_ttm, amount),
            lab("revenue growth YoY", f.revenue_growth_yoy, fmt::pct_signed),
            lab("earnings growth YoY", f.earnings_growth_yoy, fmt::pct_signed),
        ],
    );
    t.row(
        l.t("Márgenes y rentabilidad", "Margins and returns"),
        vec![
            lab("gross", f.gross_margin, fmt::pct),
            lab("operating", f.operating_margin, fmt::pct),
            lab("net", f.profit_margin, fmt::pct),
            lab("ROE", f.roe, fmt::pct),
            lab("ROA", f.roa, fmt::pct),
        ],
    );
    t.row(
        l.t("Caja y deuda", "Cash and debt"),
        vec![
            lab("FCF", f.free_cash_flow, amount),
            lab("cash", f.total_cash, amount),
            lab("debt", f.total_debt, amount),
            lab("debt/equity", f.debt_to_equity, fmt::num),
            lab("current ratio", f.current_ratio, fmt::num),
        ],
    );
    t.row(
        l.t("Accionista", "Shareholders"),
        vec![
            lab("dividend yield", f.dividend_yield, fmt::pct2),
            lab("payout", f.payout_ratio, fmt::pct),
            lab("beta", f.beta, fmt::num),
            lab("short % float", f.short_pct_float, fmt::pct),
        ],
    );
    t.row(
        l.t("Consenso de analistas", "Analyst consensus"),
        vec![
            lab("rating (1=buy..5=sell)", f.analyst_rating_mean, |v| format!("{v:.1}")),
            f.analyst_count.map(|n| format!("{n} analysts")),
            lab("target mean", f.analyst_target_mean, fmt::price),
            lab("low", f.analyst_target_low, fmt::price),
            lab("high", f.analyst_target_high, fmt::price),
        ],
    );
    if let Some(d) = f.next_earnings_date {
        let days = (d - snap.generated_at.date_naive()).num_days();
        t.one(
            l.t("Próximos resultados", "Next earnings"),
            Some(tr!(l, "{} (en {} días)", "{} (in {} days)", d, days)),
        );
    }
    if snap.summary.class == AssetClass::Crypto || f.circulating_supply.is_some() {
        t.row(
            l.t("Oferta", "Supply"),
            vec![
                lab("circulating", f.circulating_supply, fmt::big),
                lab("total", f.total_supply, fmt::big),
                lab("max", f.max_supply, fmt::big),
                lab("FDV", f.fully_diluted_valuation, fmt::big),
            ],
        );
        let ath_dist = f
            .all_time_high
            .zip(snap.quote.as_ref().map(|q| q.price))
            .filter(|(a, _)| *a > 0.0)
            .map(|(a, p)| format!("(price at {} of the ATH)", fmt::pct(p / a)));
        t.row(
            "ATH",
            vec![
                val(f.all_time_high, fmt::price),
                f.all_time_high_date.map(|d| d.to_string()),
                ath_dist,
            ],
        );
    }
    if matches!(snap.summary.class, AssetClass::Etf | AssetClass::Fund) {
        t.row(
            l.t("Fondo", "Fund"),
            vec![
                lab("expense ratio", f.expense_ratio, fmt::pct2),
                lab("assets", f.total_assets, fmt::big),
            ],
        );
        if lv.extras && !f.top_holdings.is_empty() {
            let h: Vec<String> = f
                .top_holdings
                .iter()
                .take(8)
                .map(|h| {
                    format!("{} {}", h.symbol.as_deref().unwrap_or(&h.name), fmt::pct(h.weight))
                })
                .collect();
            t.one(l.t("Mayores posiciones", "Top holdings"), Some(h.join(", ")));
        }
    }
    if snap.summary.class == AssetClass::Bond {
        t.row(
            l.t("Bono", "Bond"),
            vec![
                lab("yield", f.yield_to_maturity, |v| format!("{v:.2}%")),
                lab("coupon", f.coupon, |v| format!("{v:.2}%")),
                f.maturity_date.map(|d| format!("maturity {d}")),
            ],
        );
    }
    t.render(out, (l.t("Concepto", "Item"), l.t("Valor", "Value")));
}

fn render_scores(out: &mut String, card: &Scorecard, l: Lang, lv: &Level, sec: &Sections) {
    section(
        out,
        l.t(
            "Puntuaciones cuantitativas (calculadas en código, 0-100; en Riesgo más alto = más riesgo)",
            "Quantitative scores (computed in code, 0-100; for Risk higher = riskier)",
        ),
    );
    let s = &card.scores;
    let _ = writeln!(
        out,
        "| {} | {} | {} |\n|---|---|---|",
        l.t("Componente", "Component"),
        l.t("Puntos", "Score"),
        l.t("Razones", "Reasons")
    );
    let join = |v: &[String]| v.iter().take(lv.max_reasons).cloned().collect::<Vec<_>>().join("; ");
    let rows: [(u8, &str, Option<f32>, &[String]); 7] = [
        (Sections::S_TECHNICAL, l.t("Técnico", "Technical"), s.technical, &card.reasons.technical),
        (Sections::S_MOMENTUM, "Momentum", s.momentum, &card.reasons.momentum),
        (Sections::S_VALUATION, l.t("Valoración", "Valuation"), s.valuation, &card.reasons.valuation),
        (Sections::S_FUNDAMENTAL, l.t("Fundamentales", "Fundamentals"), s.fundamental, &card.reasons.fundamental),
        (Sections::S_SENTIMENT, l.t("Sentimiento", "Sentiment"), s.sentiment, &card.reasons.sentiment),
        (Sections::S_MACRO, "Macro", s.macro_score, &card.reasons.macro_score),
        (Sections::S_RISK, l.t("Riesgo", "Risk"), s.risk, &card.reasons.risk),
    ];
    for (bit, name, score, reasons) in rows {
        if sec.scores & bit != 0
            && let Some(v) = score
        {
            let _ = writeln!(out, "| {name} | {v:.0} | {} |", join(reasons));
        }
    }
    if sec.scores == Sections::S_ALL {
        let _ = writeln!(out, "| **Global** | **{:.0}** | |", s.overall);
    }
    if sec.verdict {
        let _ = writeln!(
            out,
            "{}",
            tr!(
                l,
                "Veredicto cuantitativo provisional: {} (confianza {:.2}, cobertura de datos {:.0} %). {}",
                "Provisional quantitative verdict: {} (confidence {:.2}, data coverage {:.0}%). {}",
                verdict_name(l, card.verdict),
                card.confidence,
                card.coverage * 100.0,
                card.verdict_reasons.join("; ")
            )
        );
    }
}

fn render_news(
    out: &mut String,
    news: &[NewsItem],
    _now: DateTime<Utc>,
    l: Lang,
    lv: &Level,
    reg: &mut NewsRegistry,
) {
    if news.is_empty() {
        return;
    }
    let mut sorted: Vec<&NewsItem> = news.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.published_at));
    section(
        out,
        l.t(
            "Noticias (más recientes primero; cítalas como [N1], [N2]…)",
            "News (most recent first; cite them as [N1], [N2]…)",
        ),
    );
    for item in sorted.into_iter().take(lv.max_news) {
        let tag = reg.tag_of(item);
        out.push_str(&news_line(&tag, item, lv.summary_chars));
        out.push('\n');
    }
}

/// Línea de una noticia: `[N3] fecha · medio · sentimiento: titular — resumen`.
pub fn news_line(tag: &str, item: &NewsItem, summary_chars: usize) -> String {
    let sent = match &item.sentiment {
        Some(s) => s.clone(),
        None => sentiment::sentiment_of(item),
    };
    let label = format!("{:?}", sent.label).to_lowercase();
    let mut line = format!(
        "[{tag}] {} · {} · {:+.2} {label} ({}): {}",
        item.published_at.format("%Y-%m-%d %H:%M"),
        item.publisher.as_deref().unwrap_or(&item.provider),
        sent.score,
        sent.method,
        item.title.trim()
    );
    if summary_chars > 0
        && let Some(sm) = item.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let _ = write!(line, " — {}", truncate_chars(sm, summary_chars));
        }
    line
}

fn render_macro(out: &mut String, m: &domain::MacroContext, l: Lang, lv: &Level) {
    if m.indicators.is_empty() {
        return;
    }
    let asof = m.as_of.map(|t| format!(" [{}]", ts(t))).unwrap_or_default();
    section(out, &format!("{}{asof}", l.t("Entorno macro", "Macro environment")));
    // Prioridad a lo que usa la puntuación; el resto después, hasta el límite del nivel.
    const PRIORITY: [&str; 8] =
        ["vix", "us10y", "us2y", "curve_10y2y", "fed_funds", "cpi_yoy", "unemployment", "dxy"];
    let mut inds: Vec<&domain::MacroIndicator> = m.indicators.iter().collect();
    inds.sort_by_key(|i| PRIORITY.iter().position(|k| *k == i.key).unwrap_or(usize::MAX));
    let _ = writeln!(
        out,
        "| {} | {} | {} | {} |\n|---|---|---|---|",
        l.t("Indicador", "Indicator"),
        l.t("Valor", "Value"),
        l.t("Cambio", "Change"),
        l.t("Fuente", "Source")
    );
    for i in inds.into_iter().take(lv.max_macro) {
        let ch = i.change.map(|c| format!("{c:+.2}")).unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} {} | {ch} | {} {} |",
            i.name,
            fmt::num(i.value),
            i.unit,
            i.source,
            i.as_of.format("%Y-%m-%d")
        );
    }
}

/// Parte suelta del contexto, para los resultados de las herramientas del chat.
#[derive(Debug, Clone, Copy)]
pub enum Part {
    Quote,
    Indicators,
    Fundamentals,
    Macro,
}

/// Renderiza una sola sección (misma tabla y misma cabecera con fuente y hora que en el análisis).
/// `snap` solo necesita lo que usa la sección pedida.
pub fn render_part(snap: &AssetSnapshot, part: Part, lang: Lang) -> String {
    let lv = &LEVELS[0];
    let mut out = String::new();
    match part {
        Part::Quote => {
            if let Some(q) = &snap.quote {
                render_quote(&mut out, q, snap.generated_at, lang);
            }
        }
        Part::Indicators => {
            if let Some(i) = &snap.indicators {
                render_indicators(&mut out, i, snap.quote.as_ref().map(|q| q.price), lang, lv);
            }
        }
        Part::Fundamentals => {
            if let Some(f) = &snap.fundamentals {
                render_fundamentals(&mut out, f, snap, lang, lv);
            }
        }
        Part::Macro => {
            if let Some(m) = &snap.macro_context {
                render_macro(&mut out, m, lang, lv);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixture, scoring};

    fn build(snap: &AssetSnapshot, max_tokens: usize) -> BuiltContext {
        let card = scoring::score(snap, Lang::Es);
        build_context(
            snap,
            &card,
            &ContextOptions { max_tokens, lang: Lang::Es },
            &mut NewsRegistry::new(),
        )
    }

    #[test]
    fn full_context_has_every_section_with_sources_and_times() {
        let snap = fixture::stock_snapshot();
        let c = build(&snap, 6000);
        let t = &c.text;
        assert!(c.trimmed.is_empty(), "{:?}", c.trimmed);
        assert!(t.starts_with("# Apple Inc. (stock:AAPL) — stock"));
        for needle in [
            "## Cotización [fuente: fixture · 2026-10-09 15:29 UTC",
            "| Precio | 228.4 USD |",
            "## Indicadores técnicos",
            "| RSI14 | 61.3 |",
            "| SMA20 / SMA50 / SMA200 | SMA20 226.1, SMA50 221.5, SMA200 205.7 |",
            "Soportes (de mayor a menor) | 221.5 / 214 / 205.7",
            "## Fundamentales [fuente: fixture",
            "P/E forward 29.6",
            "Próximos resultados | 2026-10-30 (en 21 días)",
            "## Puntuaciones cuantitativas",
            "Veredicto cuantitativo provisional:",
            "## Noticias (más recientes primero",
            "[N1] 2026-10-09 12:30 · Reuters",
            "## Entorno macro",
            "VIX",
        ] {
            assert!(t.contains(needle), "falta «{needle}» en:\n{t}");
        }
        assert_eq!(c.news.len(), 6);
        assert_eq!(c.tokens, estimate_tokens(t));
    }

    /// R-10: `Quote.change_pct` viene en PUNTOS porcentuales (Yahoo `(p/prev-1)*100`, Binance
    /// `priceChangePercent`: 0.201 = +0,2 %), pero el contexto lo pasaba por `pct_signed`, que espera
    /// una fracción: el modelo veía «+20.1%» donde el dato real es +0,2 %.
    #[test]
    fn day_change_is_shown_in_percent_points_not_multiplied_by_100() {
        let mut snap = fixture::stock_snapshot();
        {
            let q = snap.quote.as_mut().unwrap();
            q.change_pct = Some(0.201);
            q.change = Some(0.46);
        }
        let c = build(&snap, 6000);
        assert!(c.text.contains("| Variación del día | +0.2%, (0.46) |"), "{}", c.text);
        snap.quote.as_mut().unwrap().change_pct = Some(-3.456);
        snap.quote.as_mut().unwrap().change = Some(-8.2);
        let c = build(&snap, 6000);
        assert!(c.text.contains("| Variación del día | -3.5%, (-8.2) |"), "{}", c.text);
        // Y la fuente de verdad de la cifra (la herramienta del chat) dice lo mismo.
        let tool = render_part(&snap, Part::Quote, Lang::Es);
        assert!(tool.contains("-3.5%") && !tool.contains("-345.6%"), "{tool}");
    }

    /// R-12: en un ADR las cuentas están en otra moneda (`extra.financialCurrency`, p. ej. TWD en TSM)
    /// que el precio y la capitalización (USD). El contexto mostraba ingresos, caja y deuda sin moneda,
    /// justo al lado de una capitalización en USD: el modelo los comparaba como si fueran la misma.
    #[test]
    fn statement_amounts_carry_their_currency_when_it_differs_from_the_price_currency() {
        let mut snap = fixture::stock_snapshot();
        let f = snap.fundamentals.as_mut().unwrap();
        f.extra.insert("financialCurrency".into(), serde_json::json!("TWD"));
        let (rev, fcf, cash, debt) = (f.revenue_ttm.unwrap(), f.free_cash_flow.unwrap(), f.total_cash.unwrap(), f.total_debt.unwrap());
        let c = build(&snap, 6000);
        for (label, v) in [("revenue TTM", rev), ("FCF", fcf), ("cash", cash), ("debt", debt)] {
            let needle = format!("{label} {} TWD", fmt::big(v));
            assert!(c.text.contains(&needle), "falta «{needle}» en:\n{}", c.text);
        }
        assert!(c.text.contains("| Monedas | "), "{}", c.text);
        // Los objetivos de analistas siguen en la moneda de cotización: sin sufijo.
        let line = c.text.lines().find(|l| l.contains("target mean")).expect("consenso");
        assert!(!line.contains("TWD"), "{line}");
        // Misma moneda (o desconocida): sin ruido.
        let f = snap.fundamentals.as_mut().unwrap();
        f.extra.insert("financialCurrency".into(), serde_json::json!("USD"));
        let c = build(&snap, 6000);
        assert!(!c.text.contains("TWD") && !c.text.contains("| Monedas |"), "{}", c.text);
        snap.fundamentals.as_mut().unwrap().extra.clear();
        assert!(!build(&snap, 6000).text.contains("| Monedas |"));
    }

    #[test]
    fn dump_context() {
        let c = build(&fixture::stock_snapshot(), 6000);
        println!("{}\n--- {} tokens est., {} chars", c.text, c.tokens, c.text.len());
    }

    #[test]
    fn numbers_are_rounded() {
        let mut snap = fixture::stock_snapshot();
        snap.quote.as_mut().unwrap().price = 228.4012345;
        snap.indicators.as_mut().unwrap().rsi14 = Some(61.34567);
        let c = build(&snap, 6000);
        assert!(c.text.contains("| Precio | 228.4 USD |"));
        assert!(c.text.contains("| RSI14 | 61.3 |"));
        assert!(!c.text.contains("228.4012") && !c.text.contains("61.345"));
    }

    #[test]
    fn news_are_numbered_most_recent_first_with_sentiment() {
        let mut snap = fixture::stock_snapshot();
        snap.news.reverse(); // el orden de entrada no debe importar
        let c = build(&snap, 6000);
        let lines: Vec<&str> = c.text.lines().filter(|l| l.starts_with("[N")).collect();
        assert_eq!(lines.len(), 6);
        assert!(lines[0].starts_with("[N1]") && lines[0].contains("Apple shares rise"));
        assert!(lines[1].starts_with("[N2]") && lines[1].contains("antitrust probe"));
        assert!(lines[0].contains("lexicon") && lines[0].contains("positive"));
        assert!(lines[1].contains("negative"));
        // La numeración corresponde con el registro.
        assert!(c.news.get("N1").unwrap().title.contains("Apple shares rise"));
        assert!(c.news.get("[N2]").unwrap().title.contains("antitrust"));
        assert!(c.news.get("N7").is_none());
    }

    #[test]
    fn provider_sentiment_is_kept() {
        let mut snap = fixture::stock_snapshot();
        snap.news[0].sentiment = Some(domain::Sentiment {
            score: -0.77,
            label: domain::SentimentLabel::VeryNegative,
            method: "provider:marketaux".into(),
        });
        let c = build(&snap, 6000);
        assert!(c.text.contains("-0.77 verynegative (provider:marketaux)"));
    }

    #[test]
    fn degrades_by_levels_to_fit_budget() {
        let snap = fixture::stock_snapshot();
        let full = build(&snap, 100_000);
        assert!(full.trimmed.is_empty());
        let budget = full.tokens * 3 / 4;
        let small = build(&snap, budget);
        assert!(small.tokens <= budget, "{} > {budget}", small.tokens);
        assert!(!small.trimmed.is_empty());
        assert!(small.news.len() <= full.news.len());
        // Sigue teniendo lo esencial.
        assert!(
            small.text.contains("| Precio |") && small.text.contains("Puntuaciones cuantitativas")
        );
        // Más presupuesto nunca da menos contenido.
        let mid = build(&snap, full.tokens - 1);
        assert!(mid.tokens < full.tokens);
        assert!(mid.tokens >= small.tokens);
    }

    #[test]
    fn hard_truncates_when_even_minimum_does_not_fit() {
        let snap = fixture::stock_snapshot();
        let c = build(&snap, 300);
        assert!(c.tokens <= 301);
        assert!(c.text.ends_with('…'));
        assert!(c.trimmed[0].contains("recortado a la fuerza"));
        assert!(c.text.starts_with("# Apple Inc."));
    }

    #[test]
    fn registry_is_only_updated_with_the_news_actually_included() {
        let snap = fixture::stock_snapshot();
        let full = build(&snap, 100_000);
        let card = scoring::score(&snap, Lang::Es);
        let mut reg = NewsRegistry::new();
        let c = build_context(
            &snap,
            &card,
            &ContextOptions { max_tokens: full.tokens * 2 / 3, lang: Lang::Es },
            &mut reg,
        );
        assert_eq!(reg.len(), c.news.len());
        let shown = c.text.lines().filter(|l| l.starts_with("[N")).count();
        assert_eq!(reg.len(), shown);
    }

    #[test]
    fn missing_data_is_listed() {
        let snap = fixture::sparse_snapshot();
        let c = build(&snap, 6000);
        assert!(c.text.contains("## Datos no disponibles"));
        assert!(c.text.contains("- fundamentals: proveedor sin datos"));
        assert!(c.text.contains("Técnico: sin datos suficientes"));
        assert!(!c.text.contains("## Indicadores técnicos"));
        assert!(!c.text.contains("## Noticias"));
    }

    #[test]
    fn crypto_and_etf_specific_rows() {
        let c = build(&fixture::crypto_snapshot(), 6000);
        assert!(c.text.contains("FDV 2.07T"), "{}", c.text);
        assert!(
            c.text.contains("ATH | 109000, 2026-01-20, (price at 90.3% of the ATH)"),
            "{}",
            c.text
        );
        let mut etf = fixture::stock_snapshot();
        etf.summary.class = AssetClass::Etf;
        etf.fundamentals.as_mut().unwrap().expense_ratio = Some(0.0003);
        etf.fundamentals.as_mut().unwrap().top_holdings = vec![domain::Holding {
            symbol: Some("AAPL".into()),
            name: "Apple".into(),
            weight: 0.07,
        }];
        let c = build(&etf, 6000);
        // R-13: con un decimal, 0,03 % salía «0.0%» (un ETF casi gratis parecía gratis del todo).
        assert!(c.text.contains("expense ratio 0.03%"), "{}", c.text);
        etf.fundamentals.as_mut().unwrap().expense_ratio = Some(0.000945);
        etf.fundamentals.as_mut().unwrap().dividend_yield = Some(0.0032);
        let c = build(&etf, 6000);
        assert!(c.text.contains("expense ratio 0.09%") && c.text.contains("dividend yield 0.32%"), "{}", c.text);
        assert!(c.text.contains("Mayores posiciones | AAPL 7.0%"));
    }

    #[test]
    fn english_headings() {
        let snap = fixture::stock_snapshot();
        let card = scoring::score(&snap, Lang::En);
        let c = build_context(
            &snap,
            &card,
            &ContextOptions { max_tokens: 6000, lang: Lang::En },
            &mut NewsRegistry::new(),
        );
        assert!(c.text.contains("## Quote [source: fixture"));
        assert!(c.text.contains("## Technical indicators"));
        assert!(c.text.contains("Provisional quantitative verdict"));
    }

    #[test]
    fn typical_context_leaves_room_in_an_8k_window() {
        let c = build(&fixture::stock_snapshot(), 6000);
        // Debe caber holgadamente en num_ctx = 8192 junto al prompt de sistema y la respuesta.
        assert!(c.tokens < 3500, "{} tokens estimados", c.tokens);
    }

    #[test]
    fn citation_parsing() {
        assert_eq!(parse_tag("N3"), Some(3));
        assert_eq!(parse_tag("[n12]"), Some(12));
        assert_eq!(parse_tag("N"), None);
        assert_eq!(parse_tag("X3"), None);
        assert_eq!(parse_tag("N3x"), None);
        assert_eq!(
            extract_citations("Sube [N3] y cae [N1, N4]; ver [N2][N5] y [1] o [nota]"),
            vec![3, 1, 4, 2, 5]
        );
        assert_eq!(extract_citations("sin citas"), Vec::<usize>::new());
        assert_eq!(extract_citations("abierto [N3"), Vec::<usize>::new());
    }

    #[test]
    fn registry_dedups_and_maps_sources() {
        let snap = fixture::stock_snapshot();
        let mut reg = NewsRegistry::new();
        let a = reg.tag_of(&snap.news[0]);
        let b = reg.tag_of(&snap.news[1]);
        let a2 = reg.tag_of(&snap.news[0]);
        assert_eq!((a.as_str(), b.as_str(), a2.as_str()), ("N1", "N2", "N1"));
        let refs = reg.source_refs(["N2", "[N1]", "N9"]);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].provider, "Financial Times");
        assert!(refs[0].url.as_deref().unwrap().starts_with("https://"));
    }
}
