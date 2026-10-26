//! Perfil de riesgo determinista de una OPV: indicadores leídos del folleto (S-1/F-1/424B4) y del
//! calendario, y tasa base de las OPV recientes.
//!
//! Reglas de diseño:
//! - **Sin LLM, sin estimaciones.** Cada indicador sale de una regla explícita sobre el texto del
//!   folleto y lleva la frase literal que lo sustenta (`evidence`) y su fuente. Si la regla no
//!   encuentra el dato con fiabilidad, el indicador queda `NotExtracted` con el motivo.
//! - Las cifras de folletos son frágiles (tablas de columnas variables, notas al pie, varias
//!   clases de acciones). Donde el orden de columnas no se puede deducir, **no se interpreta**:
//!   se da lo que es independiente del orden (p. ej. el signo del resultado en todos los periodos)
//!   y la fila literal.
//! - La tasa base (`compute_base_rate`) describe lo ocurrido con OPV recientes; no predice una OPV
//!   concreta.
//!
//! Sin dependencias de regex (igual que `prospectus.rs`).

use chrono::{DateTime, Utc};
use domain::{
    IpoBaseRate, IpoBaseRateTier, IpoEntry, IpoIndicatorSource, IpoIndicatorStatus, IpoPerformance, IpoProspectusRef, IpoRiskFlag,
    IpoRiskIndicator, IpoRiskProfile, IpoStatus, Prospectus,
};

pub const DISCLAIMER: &str = "Perfil de riesgo, no una recomendación ni una predicción. Resume hechos leídos del \
folleto y del calendario con reglas fijas; lo que no se pudo leer figura como «no extraído». La tasa base de OPV \
recientes describe lo ocurrido con otras OPV y no predice cómo irá esta.";

/// Datos de entrada del perfil.
pub struct ProfileInput<'a> {
    pub symbol: &'a str,
    pub entry: Option<&'a IpoEntry>,
    pub prospectus: Option<&'a Prospectus>,
    pub base_rate: Option<IpoBaseRate>,
    pub now: DateTime<Utc>,
}

/// Construye el perfil completo. Función pura: mismo folleto, mismo perfil.
pub fn build_profile(input: ProfileInput<'_>) -> IpoRiskProfile {
    let ctx = Ctx { p: input.prospectus, entry: input.entry };
    let offer_price = ctx.offer_price();
    let indicators = vec![
        offer_size(&ctx),
        use_of_proceeds(&ctx),
        secondary_sale(&ctx),
        float_offered(&ctx),
        lock_up(&ctx),
        underwriters(&ctx),
        dilution(&ctx, offer_price),
        voting(&ctx),
        controlled_company(&ctx),
        net_result(&ctx),
        revenue(&ctx),
    ];

    let mut warnings = Vec::new();
    match input.prospectus {
        None => warnings.push(
            "No se pudo obtener el folleto: los indicadores que dependen de él quedan como «no extraído».".to_string(),
        ),
        Some(p) if p.preliminary => warnings.push(format!(
            "El folleto disponible es preliminar ({}): el precio, el número de acciones y las cifras pueden cambiar en el definitivo (424B4).",
            p.form
        )),
        Some(_) => {}
    }
    if let Some(e) = input.entry {
        if e.spac_likely {
            warnings.push(
                "Probable SPAC (sociedad vehículo sin negocio): este perfil está pensado para empresas operativas; \
                 en una SPAC no hay ingresos ni resultados y su riesgo es otro (fusión posterior)."
                    .to_string(),
            );
        }
        if matches!(e.status, IpoStatus::Expected | IpoStatus::Filed) {
            warnings.push("La OPV aún no cotiza: precio y acciones pueden cambiar hasta la fijación del precio.".to_string());
        }
    } else {
        warnings.push("No se encontró la OPV en el calendario: faltan los datos de tamaño y fecha.".to_string());
    }
    let size = input.entry.and_then(offer_size_usd);
    let base_rate_tier: Option<IpoBaseRateTier> = input.base_rate.as_ref().zip(size).and_then(|(b, sz)| tier_for(b, sz).cloned());
    if input.base_rate.is_some() && base_rate_tier.is_none() {
        warnings.push("No se pudo calcular el tamaño de la oferta: se muestra solo la tasa base global.".to_string());
    }
    if input.base_rate.is_none() {
        warnings.push("La tasa base de OPV recientes no está disponible ahora.".to_string());
    }

    IpoRiskProfile {
        symbol: input.symbol.to_ascii_uppercase(),
        name: input.entry.map(|e| e.name.clone()).or_else(|| input.prospectus.map(|p| p.issuer.clone())),
        entry: input.entry.cloned(),
        prospectus: input.prospectus.map(|p| IpoProspectusRef {
            form: p.form.clone(),
            preliminary: p.preliminary,
            filed_on: p.filed_on,
            url: p.url.clone(),
            index_url: p.index_url.clone(),
            document_chars: p.document_chars,
            missing: p.missing.clone(),
        }),
        indicators,
        base_rate: input.base_rate,
        base_rate_tier,
        warnings,
        disclaimer: DISCLAIMER.to_string(),
        generated_at: input.now,
    }
}

// ---------------------------------------------------------------------------------------------
// Contexto y utilidades de texto
// ---------------------------------------------------------------------------------------------

struct Ctx<'a> {
    p: Option<&'a Prospectus>,
    entry: Option<&'a IpoEntry>,
}

impl<'a> Ctx<'a> {
    fn section(&self, key: &str) -> Option<&'a domain::ProspectusSection> {
        self.p?.sections.iter().find(|s| s.key == key && !s.text.trim().is_empty())
    }

    fn source(&self, key: &str) -> IpoIndicatorSource {
        match (self.p, self.section(key)) {
            (Some(p), Some(s)) => IpoIndicatorSource {
                kind: "prospectus".into(),
                section: Some(key.to_string()),
                label: format!("Folleto {} ({}), sección «{}»", p.form, p.filed_on, s.title),
                url: Some(p.url.clone()),
            },
            (Some(p), None) => IpoIndicatorSource {
                kind: "prospectus".into(),
                section: Some(key.to_string()),
                label: format!("Folleto {} ({}): sección no localizada", p.form, p.filed_on),
                url: Some(p.url.clone()),
            },
            (None, _) => IpoIndicatorSource {
                kind: "prospectus".into(),
                section: Some(key.to_string()),
                label: "Folleto no disponible".into(),
                url: None,
            },
        }
    }

    fn calendar_source(&self) -> IpoIndicatorSource {
        match self.entry {
            Some(e) => IpoIndicatorSource {
                kind: "calendar".into(),
                section: None,
                label: if e.source == "finnhub" { "Calendario de OPV de Finnhub".into() } else { format!("Calendario de OPV ({})", e.source) },
                url: e.source_url.clone(),
            },
            None => IpoIndicatorSource { kind: "calendar".into(), section: None, label: "Calendario de OPV: sin entrada".into(), url: None },
        }
    }

    /// Precio de la OPV: el definitivo del calendario; si no, el de la portada del folleto (solo
    /// si es verosímil: algunos folletos dejan solo el valor nominal, p. ej. 0,0001 USD).
    fn offer_price(&self) -> Option<f64> {
        if let Some(e) = self.entry
            && e.status == IpoStatus::Priced
            && let (Some(lo), Some(hi)) = (e.price_low, e.price_high)
            && lo == hi
            && lo > 0.0
        {
            return Some(lo);
        }
        self.p.and_then(|p| p.offer_price).filter(|v| *v >= 0.5)
    }

    fn missing_note(&self, key: &str) -> String {
        if self.p.is_none() {
            "Folleto no disponible.".to_string()
        } else {
            format!("La sección «{key}» no se localizó en el folleto.")
        }
    }
}

fn not_extracted(key: &str, label: &str, source: IpoIndicatorSource, note: impl Into<String>) -> IpoRiskIndicator {
    IpoRiskIndicator {
        key: key.into(),
        label: label.into(),
        status: IpoIndicatorStatus::NotExtracted,
        value: None,
        numeric: None,
        unit: None,
        flags: Vec::new(),
        evidence: None,
        note: Some(note.into()),
        source,
    }
}

#[allow(clippy::too_many_arguments)]
fn extracted(
    key: &str,
    label: &str,
    source: IpoIndicatorSource,
    value: String,
    numeric: Option<f64>,
    unit: Option<&str>,
    flags: Vec<IpoRiskFlag>,
    evidence: Option<String>,
    note: Option<String>,
) -> IpoRiskIndicator {
    IpoRiskIndicator {
        key: key.into(),
        label: label.into(),
        status: IpoIndicatorStatus::Extracted,
        value: Some(value),
        numeric,
        unit: unit.map(str::to_string),
        flags,
        evidence,
        note,
        source,
    }
}

fn flag(code: &str, label: &str, attention: bool, evidence: &str) -> IpoRiskFlag {
    IpoRiskFlag { code: code.into(), label: label.into(), attention, evidence: clip(evidence, 320) }
}

/// Recorta a `max` caracteres en un límite de palabra y colapsa los espacios.
fn clip(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let cut: String = one.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).trim_end_matches([',', ';']).to_string();
    format!("{cut}…")
}

/// Posición válida (límite de carácter) a la izquierda de `i`.
fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Abreviaturas tras las que un punto no termina la frase.
fn is_abbrev(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '.').trim_end_matches('.').to_ascii_lowercase();
    t.len() <= 1
        || t.contains('.')
        || matches!(t.as_str(), "inc" | "corp" | "co" | "ltd" | "llc" | "lp" | "no" | "nos" | "mr" | "dr" | "mrs" | "sec" | "approx" | "vs" | "etc" | "plc")
}

/// Divide en frases por líneas y por «. » seguido de mayúscula (sin cortar «J.P. Morgan», «Inc.»).
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Algunos folletos traen el párrafo cortado en líneas: se une una línea con la siguiente si la
    // primera no termina en puntuación y la segunda empieza en minúscula, cifra o «$».
    let mut logical: Vec<String> = Vec::new();
    // ¿La última línea lógica contiene « | »? Se arrastra como booleano: releer la línea acumulada
    // en cada línea nueva costaba O(n²) con un folleto hostil de líneas cortas sin puntuación.
    let mut last_has_pipe = false;
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let raw_has_pipe = raw.contains(" | ");
        let cont = logical.last().is_some_and(|prev| {
            !last_has_pipe
                && !raw_has_pipe
                && !prev.ends_with(['.', ':', ';', '!', '?', '”', '"'])
                && raw.chars().next().is_some_and(|c| c.is_lowercase() || c.is_ascii_digit() || c == '$' || c == '(')
        });
        if cont {
            let last = logical.last_mut().expect("hay");
            // Ni `last` ni `raw` traen « | »; la unión (`last` + ' ' + `raw`) solo puede formarlo si
            // `last` termina en « |» (la línea de `raw` nunca empieza por «|»).
            last_has_pipe = last.ends_with(" |");
            last.push(' ');
            last.push_str(raw);
        } else {
            last_has_pipe = raw_has_pipe;
            logical.push(raw.to_string());
        }
    }
    for line in logical.iter().map(String::as_str) {
        if line.is_empty() {
            continue;
        }
        let chars: Vec<(usize, char)> = line.char_indices().collect();
        let mut start = 0usize;
        for k in 0..chars.len() {
            let (i, c) = chars[k];
            if !matches!(c, '.' | '!' | '?' | ';') {
                continue;
            }
            let Some(&(_, next)) = chars.get(k + 1) else { continue };
            if !next.is_whitespace() {
                continue;
            }
            let after = chars[k + 1..].iter().map(|&(_, ch)| ch).find(|ch| !ch.is_whitespace());
            if !after.is_some_and(|a| a.is_uppercase() || matches!(a, '“' | '"' | '•' | '(')) {
                continue;
            }
            let tok = line[start..i + c.len_utf8()].rsplit(char::is_whitespace).next().unwrap_or("");
            if c == '.' && is_abbrev(tok) {
                continue;
            }
            let s = line[start..i + c.len_utf8()].trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            start = i + c.len_utf8();
        }
        let tail = line[start..].trim();
        if !tail.is_empty() {
            out.push(tail.to_string());
        }
    }
    out
}

/// Fila de tabla del texto limpio: «Etiqueta | celda | celda».
struct Row {
    label: String,
    cells: Vec<String>,
    raw: String,
}

fn rows(text: &str) -> Vec<Row> {
    text.lines()
        .filter(|l| l.contains(" | "))
        .map(|l| {
            let mut it = l.split(" | ").map(|c| c.trim().to_string());
            let label = it.next().unwrap_or_default();
            Row { label, cells: it.collect(), raw: l.trim().to_string() }
        })
        .collect()
}

/// Entero al comienzo de una celda («8,333,334 shares.»). Rechaza porcentajes y decimales.
fn leading_count(cell: &str) -> Option<u64> {
    let c = cell.trim().trim_start_matches(['$', '~']);
    let digits: String = c.chars().take_while(|ch| ch.is_ascii_digit() || *ch == ',').collect();
    let rest = &c[digits.len()..];
    if rest.starts_with('%') || (rest.starts_with('.') && rest[1..].starts_with(|ch: char| ch.is_ascii_digit())) {
        return None;
    }
    let n: u64 = digits.replace(',', "").parse().ok()?;
    (n >= 100).then_some(n)
}

/// Importe de una celda: «$(11.36)», «US$4.3801», «(3,144)». `None` si no hay número.
fn parse_amount(cell: &str) -> Option<f64> {
    let c = cell.trim();
    let neg = c.contains('(') || c.starts_with('-') || c.contains("$-");
    let start = c.find(|ch: char| ch.is_ascii_digit())?;
    let num: String = c[start..].chars().take_while(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '.')).collect();
    let num = num.trim_end_matches([',', '.']).replace(',', "");
    let v: f64 = num.parse().ok()?;
    Some(if neg { -v } else { v })
}

fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(ch);
    }
    out
}

fn fmt_dec(v: f64, dec: usize) -> String {
    format!("{v:.dec$}").replace('.', ",")
}

fn fmt_usd(v: f64) -> String {
    if v.abs() >= 1e9 {
        format!("{} mil millones de USD", fmt_dec(v / 1e9, 2))
    } else if v.abs() >= 1e6 {
        format!("{} M USD", fmt_dec(v / 1e6, 1))
    } else {
        format!("{} USD", fmt_int(v.round() as u64))
    }
}

// ---------------------------------------------------------------------------------------------
// Indicadores
// ---------------------------------------------------------------------------------------------

const L_SIZE: &str = "Tamaño de la oferta";
const L_PROCEEDS: &str = "Destino de los fondos";
const L_SECONDARY: &str = "Venta de accionistas existentes";
const L_FLOAT: &str = "Capital ofrecido frente al total";
const L_LOCKUP: &str = "Lock-up";
const L_UNDERWRITERS: &str = "Colocadores principales";
const L_DILUTION: &str = "Dilución por acción";
const L_VOTING: &str = "Clases de acciones y voto";
const L_CONTROLLED: &str = "Empresa controlada";
const L_NET: &str = "Resultado neto";
const L_REVENUE: &str = "Ingresos";

fn offer_size(c: &Ctx) -> IpoRiskIndicator {
    let src = c.calendar_source();
    let Some(e) = c.entry else {
        return not_extracted("offer_size", L_SIZE, src, "La OPV no figura en el calendario consultado.");
    };
    let (Some(total), Some(shares)) = (e.total_value, e.shares) else {
        return not_extracted("offer_size", L_SIZE, src, "El calendario no da importe ni número de acciones (suele ocurrir en presentaciones sin precio).");
    };
    let price = match (e.price_low, e.price_high) {
        (Some(lo), Some(hi)) if lo == hi => format!("a {} USD", fmt_dec(lo, 2)),
        (Some(lo), Some(hi)) => format!("a {}–{} USD", fmt_dec(lo, 2), fmt_dec(hi, 2)),
        _ => "sin precio".to_string(),
    };
    let note = if e.status == IpoStatus::Priced {
        "Acciones × precio definitivo, según el calendario."
    } else {
        "Con la OPV aún sin precio definitivo, el calendario da el máximo posible (límite alto × acciones × 1,15 de sobreasignación)."
    };
    extracted(
        "offer_size",
        L_SIZE,
        src,
        format!("{} ({} acciones {})", fmt_usd(total), fmt_int(shares), price),
        Some(total),
        Some("USD"),
        Vec::new(),
        None,
        Some(note.to_string()),
    )
}

fn use_of_proceeds(c: &Ctx) -> IpoRiskIndicator {
    let Some(sec) = c.section("use_of_proceeds") else {
        return not_extracted("use_of_proceeds", L_PROCEEDS, c.source("use_of_proceeds"), c.missing_note("use_of_proceeds"));
    };
    const DEBT_ACT: &[&str] = &["repay", "repayment", "pay down", "paydown", "prepay", "retire the", "redeem"];
    const DEBT_OBJ: &[&str] = &[
        "indebtedness", "debt", "borrowings", "term loan", "credit facility", "credit agreement", "senior notes", "notes due",
        "revolving", "loan", "convertible notes",
    ];
    let mut debt: Option<String> = None;
    let mut general: Option<String> = None;
    let mut working: Option<String> = None;
    let mut sellers: Option<String> = None;
    let mut acquisitions: Option<String> = None;
    let mut rnd: Option<String> = None;
    let mut capex: Option<String> = None;
    let mut redeem: Option<String> = None;
    for s in sentences(&sec.text) {
        let l = s.to_ascii_lowercase();
        let negated = l.contains("no portion") || l.contains("not use") || l.contains("not intend to use") || l.contains("will not repay");
        if !negated && DEBT_ACT.iter().any(|a| l.contains(a)) && DEBT_OBJ.iter().any(|o| l.contains(o)) && debt.is_none() {
            debt = Some(s.clone());
        }
        if l.contains("general corporate purposes") && general.is_none() {
            general = Some(s.clone());
        }
        if l.contains("working capital") && working.is_none() {
            working = Some(s.clone());
        }
        if l.contains("will not receive any")
            && (l.contains("selling stockholder") || l.contains("selling shareholder") || l.contains("selling securityholder") || l.contains("selling holder"))
            && sellers.is_none()
        {
            sellers = Some(s.clone());
        }
        if (l.contains("acquisitions") || l.contains("acquire businesses") || l.contains("strategic acquisition")) && acquisitions.is_none() {
            acquisitions = Some(s.clone());
        }
        if (l.contains("clinical") || l.contains("research and development")) && rnd.is_none() {
            rnd = Some(s.clone());
        }
        if l.contains("capital expenditures") && capex.is_none() {
            capex = Some(s.clone());
        }
        if l.contains("redeem") && (l.contains("units") || l.contains("llc interests") || l.contains("preferred")) && redeem.is_none() {
            redeem = Some(s.clone());
        }
    }
    let mut flags = Vec::new();
    if let Some(e) = &debt {
        flags.push(flag("debt_repayment", "Devolver deuda", true, e));
    }
    if let Some(e) = &sellers {
        flags.push(flag("selling_holders", "Accionistas vendedores (la empresa no recibe esa parte)", true, e));
    }
    if let Some(e) = &redeem {
        flags.push(flag("redeem_holders", "Rescate de títulos de accionistas existentes", true, e));
    }
    if let Some(e) = &general {
        flags.push(flag("general_corporate", "Fines corporativos generales", false, e));
    }
    if let Some(e) = &working {
        flags.push(flag("working_capital", "Capital circulante", false, e));
    }
    if let Some(e) = &rnd {
        flags.push(flag("rnd", "Desarrollo clínico o I+D", false, e));
    }
    if let Some(e) = &acquisitions {
        flags.push(flag("acquisitions", "Adquisiciones", false, e));
    }
    if let Some(e) = &capex {
        flags.push(flag("capex", "Inversión de capital", false, e));
    }
    let value = if flags.is_empty() {
        "Ninguna de las categorías buscadas aparece en la sección (léela entera)".to_string()
    } else {
        flags.iter().map(|f| f.label.split(" (").next().unwrap_or(&f.label).to_string()).collect::<Vec<_>>().join(" · ")
    };
    let evidence = sentences(&sec.text).into_iter().find(|s| s.to_ascii_lowercase().contains("use") && s.to_ascii_lowercase().contains("proceeds")).map(|s| clip(&s, 320));
    extracted(
        "use_of_proceeds",
        L_PROCEEDS,
        c.source("use_of_proceeds"),
        value,
        None,
        None,
        flags,
        evidence,
        Some("Categorías detectadas por palabras clave en la sección; no mide importes ni su peso relativo.".into()),
    )
}

fn is_ads(text: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|t| t.eq_ignore_ascii_case("ads") || t.eq_ignore_ascii_case("adss"))
        || text.to_ascii_lowercase().contains("american depositary")
}

/// Acciones nuevas (ofrecidas por la empresa) y de accionistas vendedores, de «The Offering».
struct OfferedShares {
    new: Option<(u64, String)>,
    selling: Option<(u64, String)>,
    ads: bool,
}

fn offered_shares(c: &Ctx) -> Option<OfferedShares> {
    let sec = c.section("offering")?;
    let mut new = None;
    let mut selling = None;
    let mut ads = false;
    for r in rows(&sec.text) {
        let l = r.label.to_ascii_lowercase();
        if !l.contains("offered") {
            continue;
        }
        let Some(n) = r.cells.first().and_then(|c| leading_count(c)) else { continue };
        let blocked = ["concurrent", "option", "directed", "reserved", "private placement", "over-allotment", "prior to"];
        if blocked.iter().any(|b| l.contains(b)) {
            continue;
        }
        if l.contains("selling") {
            if selling.is_none() {
                selling = Some((n, r.raw.clone()));
            }
        } else if new.is_none() {
            ads = is_ads(&r.label) || r.cells.first().is_some_and(|c| is_ads(c));
            new = Some((n, r.raw.clone()));
        }
    }
    Some(OfferedShares { new, selling, ads })
}

fn secondary_sale(c: &Ctx) -> IpoRiskIndicator {
    let src = c.source("offering");
    let Some(off) = offered_shares(c) else {
        return not_extracted("secondary_sale", L_SECONDARY, src, c.missing_note("offering"));
    };
    let Some((new, new_raw)) = off.new else {
        return not_extracted(
            "secondary_sale",
            L_SECONDARY,
            src,
            "No se encontró la fila «offered by us» con el número de acciones en «The Offering».",
        );
    };
    let unit = if off.ads { "ADS" } else { "acciones" };
    if let Some((sell, sell_raw)) = off.selling {
        let total = new + sell;
        let pct = sell as f64 / total as f64 * 100.0;
        return extracted(
            "secondary_sale",
            L_SECONDARY,
            src,
            format!(
                "{} % de lo ofrecido lo venden accionistas existentes ({} de {} {unit}; la empresa vende {})",
                fmt_dec(pct, 1),
                fmt_int(sell),
                fmt_int(total),
                fmt_int(new)
            ),
            Some(pct),
            Some("%"),
            vec![flag("secondary", "Venta secundaria: los de dentro venden", true, &sell_raw)],
            Some(format!("{new_raw} || {sell_raw}")),
            Some("Sin contar la opción de sobreasignación. Lo que venden los accionistas no entra en la caja de la empresa.".into()),
        );
    }
    // Sin fila de vendedores: solo se afirma «0 %» si el folleto no menciona vendedores en las
    // secciones leídas.
    let mentions = ["offering", "use_of_proceeds"].iter().filter_map(|k| c.section(k)).any(|s| {
        let l = s.text.to_ascii_lowercase();
        l.contains("selling stockholder") || l.contains("selling shareholder") || l.contains("selling securityholder")
    });
    if mentions {
        return not_extracted(
            "secondary_sale",
            L_SECONDARY,
            src,
            "El folleto menciona accionistas vendedores pero no se leyó su número de acciones.",
        );
    }
    extracted(
        "secondary_sale",
        L_SECONDARY,
        src,
        format!("0 % (solo se ofrecen {} {unit} nuevas; el folleto no menciona accionistas vendedores)", fmt_int(new)),
        Some(0.0),
        Some("%"),
        Vec::new(),
        Some(new_raw),
        Some("Sin contar la opción de sobreasignación.".into()),
    )
}

fn float_offered(c: &Ctx) -> IpoRiskIndicator {
    let src = c.source("offering");
    let Some(off) = offered_shares(c) else {
        return not_extracted("float_offered", L_FLOAT, src, c.missing_note("offering"));
    };
    let Some((new, new_raw)) = off.new else {
        return not_extracted("float_offered", L_FLOAT, src, "No se encontró el número de acciones ofrecidas.");
    };
    if off.ads {
        return not_extracted("float_offered", L_FLOAT, src, "La oferta es en ADS y las acciones en circulación se dan en acciones ordinarias: unidades no comparables sin la razón ADS/acción.");
    }
    let offered = new + off.selling.as_ref().map_or(0, |s| s.0);
    let sec = c.section("offering").expect("offering existe si hay acciones");
    let mut totals: Vec<(String, u64, String)> = Vec::new();
    let mut classes: Vec<(String, u64, String)> = Vec::new();
    // Etiquetas de clase ya vistas: buscarlas en `classes` era O(n²) con miles de filas distintas.
    let mut seen_classes: HashSet<String> = HashSet::new();
    for r in rows(&sec.text) {
        let l = r.label.to_ascii_lowercase();
        let after = l.contains("after this offering") || l.contains("immediately after") || l.contains("following this offering") || l.contains("after the offering");
        if !l.contains("outstanding") || !after {
            continue;
        }
        if ["option", "warrant", "reserved", "issuable", "exercise"].iter().any(|b| l.contains(b)) {
            continue;
        }
        let Some(n) = r.cells.first().and_then(|c| leading_count(c)) else { continue };
        if l.contains("total") {
            totals.push((l.clone(), n, r.raw.clone()));
        } else if seen_classes.insert(l.clone()) {
            classes.push((l, n, r.raw.clone()));
        }
    }
    let (outstanding, raw) = if let Some((_, n, raw)) = totals.first() {
        (*n, raw.clone())
    } else if !classes.is_empty() {
        (classes.iter().map(|x| x.1).sum(), classes.iter().map(|x| x.2.clone()).collect::<Vec<_>>().join(" || "))
    } else {
        return not_extracted("float_offered", L_FLOAT, src, "No se encontró la fila de acciones en circulación tras la oferta.");
    };
    if outstanding < offered {
        return not_extracted("float_offered", L_FLOAT, src, "Las cifras leídas no cuadran (acciones ofrecidas > acciones tras la oferta): no se usa.");
    }
    let pct = offered as f64 / outstanding as f64 * 100.0;
    let mut note = String::from(
        "Acciones ofrecidas (sin sobreasignación) entre todas las acciones en circulación tras la oferta (suma de clases). \
         No es el flotante real: el resto está en manos de los de dentro y sujeto a lock-up o restricciones.",
    );
    if raw.to_ascii_lowercase().contains("concurrent") {
        note.push_str(" El total incluye la colocación o emisión concurrente.");
    }
    extracted(
        "float_offered",
        L_FLOAT,
        src,
        format!("{} % ({} de {} acciones)", fmt_dec(pct, 1), fmt_int(offered), fmt_int(outstanding)),
        Some(pct),
        Some("%"),
        Vec::new(),
        Some(format!("{new_raw} || {raw}")),
        Some(note),
    )
}

/// Contexto de un hallazgo de plazo de lock-up.
struct LockHit {
    n: u64,
    months: bool,
    section: &'static str,
    snippet: String,
}

fn lock_up_hits(c: &Ctx) -> Vec<LockHit> {
    const KEYS: &[&str] = &["lock_up", "underwriting", "shares_eligible", "offering", "summary"];
    const LOCK_WORDS: &[&str] = &["lock-up", "lock up", "lockup", "market stand-off", "market standoff", "market stand off"];
    const TAIL_OK: &[&str] = &["date of this prospectus", "date of the prospectus", "of this prospectus", "of the final prospectus", "closing of this offering", "closing of the offering", "date of the underwriting agreement", "pricing of this offering", "following the offering", "following this offering", "after the offering"];
    const COMMIT: &[&str] = &["agreed", "will agree", "not to sell", "not to offer", "not to dispose", "may not sell", "may not offer", "will not, ", "restrict", "prevent", "prohibit", "bound by"];
    let mut hits = Vec::new();
    for key in KEYS {
        let Some(sec) = c.section(key) else { continue };
        let text = sec.text.as_str();
        for (i, k, n, months) in duration_candidates(text) {
            let before_start = floor(text, i.saturating_sub(350));
            let after_end = ceil(text, (k + 100).min(text.len()));
            let before = text[before_start..i].to_ascii_lowercase();
            let after = text[k..after_end].to_ascii_lowercase();
            let near = floor(text, i.saturating_sub(120));
            let near_before = text[near..i].to_ascii_lowercase();
            let lock_ctx = *key == "lock_up" || LOCK_WORDS.iter().any(|w| before.contains(w) || after.contains(w));
            let anchored = TAIL_OK.iter().any(|t| after.contains(t));
            let committed = COMMIT.iter().any(|v| before.contains(v) || after.contains(v));
            let excluded = !committed || near_before.contains("rule 144") || near_before.contains("three-month period") || near_before.contains("rule 701") || near_before.contains("holding period") || near_before.contains("within any") || after.chars().take(40).collect::<String>().contains("period beginning") || after.contains("effective date");
            if lock_ctx && anchored && !excluded {
                let s = floor(text, i.saturating_sub(180));
                let e = ceil(text, (k + 120).min(text.len()));
                hits.push(LockHit { n, months, section: key, snippet: clip(&text[s..e], 300) });
            }
        }
    }
    hits
}

/// Plazos «N días / N-day / 180th day / N meses / six months» del texto: (inicio, fin tras la
/// unidad, N, ¿meses?). Solo rangos verosímiles de lock-up (30-1100 días, 1-36 meses).
fn duration_candidates(text: &str) -> Vec<(usize, usize, u64, bool)> {
    let b = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b',' || b[i - 1] == b'.')) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let n: u64 = text[i..j].parse().unwrap_or(0);
        let mut k = j;
        // «180th day», «1st month»
        if lower[k..].starts_with("th ") || lower[k..].starts_with("st ") || lower[k..].starts_with("nd ") || lower[k..].starts_with("rd ") {
            k += 2;
        }
        while k < b.len() && matches!(b[k], b' ' | b')' | b'-') {
            k += 1;
        }
        let tail = &lower[k..floor(&lower, k + 8)];
        let months = tail.starts_with("month");
        let days = tail.starts_with("day");
        if (days && (30..=1100).contains(&n)) || (months && (1..=36).contains(&n)) {
            out.push((i, k, n, months));
        }
        i = j;
    }
    // «six months», «six-month»: cifra en letra (las formas «six (6) months» ya las da la cifra).
    const WORDS: &[(&str, u64)] = &[
        ("one", 1), ("two", 2), ("three", 3), ("four", 4), ("five", 5), ("six", 6), ("seven", 7), ("eight", 8),
        ("nine", 9), ("ten", 10), ("eleven", 11), ("twelve", 12), ("eighteen", 18),
    ];
    // La palabra más larga de `WORDS` es «eighteen»: no hace falta mirar más atrás de eso. Retroceder
    // por todas las letras previas era O(n²) con «monthmonthmonth…». Se trabaja en bytes: una letra
    // ASCII es siempre un carácter entero, así que `word_start` cae en un límite de carácter aunque
    // lo anterior sea multibyte (comillas tipográficas, acentos).
    const MAX_WORD: usize = 8;
    let mut from = 0;
    while let Some(p) = lower[from..].find("month") {
        let at = from + p;
        from = at + 5;
        let head = lower[..at].trim_end_matches([' ', '-']);
        let letters = head.as_bytes().iter().rev().take(MAX_WORD + 1).take_while(|c| c.is_ascii_alphabetic()).count();
        if letters == 0 || letters > MAX_WORD {
            continue;
        }
        let word_start = head.len() - letters;
        let w = &head[word_start..];
        if let Some(&(_, n)) = WORDS.iter().find(|(name, _)| *name == w) {
            out.push((word_start, at, n, true));
        }
    }
    out
}

fn lock_up(c: &Ctx) -> IpoRiskIndicator {
    let hits = lock_up_hits(c);
    if hits.is_empty() {
        let note = if c.p.is_none() {
            c.missing_note("lock_up")
        } else {
            "No se encontró un plazo de lock-up de los suscriptores (N días o meses desde la fecha del folleto) en las secciones leídas. \
             Puede estar redactado de otra forma."
                .to_string()
        };
        let key = if c.section("lock_up").is_some() { "lock_up" } else { "underwriting" };
        return not_extracted("lock_up", L_LOCKUP, c.source(key), note);
    }
    // Plazos distintos que aparecen (p. ej. 180 días para los de dentro y 90 para la empresa, o
    // una salida escalonada). Con uno solo se da el número; con varios, la lista ordenada.
    let mut distinct: Vec<(u64, bool, usize)> = Vec::new();
    for h in &hits {
        if let Some(d) = distinct.iter_mut().find(|d| d.0 == h.n && d.1 == h.months) {
            d.2 += 1;
        } else {
            distinct.push((h.n, h.months, 1));
        }
    }
    let label = |n: u64, m: bool| if m { format!("{n} meses") } else { format!("{n} días") };
    let key_days = |d: &(u64, bool, usize)| if d.1 { d.0 * 30 } else { d.0 };
    let main_key = distinct.iter().max_by_key(|d| (d.2, key_days(d))).map(|d| (d.0, d.1)).expect("hay");
    let main = hits.iter().find(|h| (h.n, h.months) == main_key).expect("existe");
    distinct.sort_by_key(key_days);
    if distinct.len() == 1 {
        let (n, months, _) = distinct[0];
        return extracted(
            "lock_up",
            L_LOCKUP,
            c.source(main.section),
            label(n, months),
            (!months).then_some(n as f64),
            (!months).then_some("días"),
            Vec::new(),
            Some(main.snippet.clone()),
            Some("Plazo que firman los suscriptores con la empresa y los accionistas de siempre; suele tener excepciones y salidas anticipadas.".into()),
        );
    }
    let list = distinct.iter().map(|d| label(d.0, d.1)).collect::<Vec<_>>().join(", ");
    extracted(
        "lock_up",
        L_LOCKUP,
        c.source(main.section),
        format!("Varios plazos en el texto: {list}"),
        None,
        None,
        Vec::new(),
        Some(main.snippet.clone()),
        Some("Cada plazo corresponde a un grupo (empresa, directivos, accionistas) o a una liberación escalonada; el folleto lo detalla. La evidencia citada es la del plazo que más se repite.".into()),
    )
}

fn underwriters(c: &Ctx) -> IpoRiskIndicator {
    let src = c.source("underwriting");
    let Some(sec) = c.section("underwriting") else {
        return not_extracted("underwriters", L_UNDERWRITERS, src, c.missing_note("underwriting"));
    };
    let all = rows(&sec.text);
    let header = all.iter().position(|r| {
        let l = r.label.to_ascii_lowercase();
        let h = r.cells.first().map(|c| c.to_ascii_lowercase()).unwrap_or_default();
        matches!(l.as_str(), "name" | "underwriter" | "underwriters" | "name of underwriter" | "name of underwriters" | "underwriter name") && (h.contains("number of") || h.contains("shares") || h.contains("ads"))
    });
    let Some(h) = header else {
        return not_extracted("underwriters", L_UNDERWRITERS, src, "No se encontró la tabla de suscriptores («Name | Number of Shares») en la sección.");
    };
    let mut list: Vec<(String, u64)> = Vec::new();
    let mut total: Option<u64> = None;
    for r in &all[h + 1..] {
        let l = r.label.to_ascii_lowercase();
        let Some(n) = r.cells.first().and_then(|c| leading_count(c)) else { break };
        if l.starts_with("total") {
            total = Some(n);
            break;
        }
        list.push((r.label.trim().to_string(), n));
    }
    if list.is_empty() {
        return not_extracted("underwriters", L_UNDERWRITERS, src, "La tabla de suscriptores no tiene filas legibles.");
    }
    let sum: u64 = list.iter().map(|x| x.1).sum();
    // Sin fila «Total» (p. ej. un único colocador) solo se acepta si la suma coincide con lo ofrecido.
    let offered = offered_shares(c).and_then(|o| o.new.map(|n| n.0));
    let total = match total {
        Some(t) => t,
        None if offered == Some(sum) => sum,
        None => {
            return not_extracted("underwriters", L_UNDERWRITERS, src, "La tabla de suscriptores no termina en una fila «Total» legible ni su suma coincide con las acciones ofrecidas.");
        }
    };
    if sum != total {
        return not_extracted("underwriters", L_UNDERWRITERS, src, format!("La tabla no cuadra (suma {} frente a total {}): no se usa.", fmt_int(sum), fmt_int(total)));
    }
    let n = list.len();
    list.sort_by_key(|a| std::cmp::Reverse(a.1));
    let top: Vec<String> = list.iter().take(3).map(|(name, k)| format!("{name} ({} %)", fmt_dec(*k as f64 / total as f64 * 100.0, 0))).collect();
    let more = if n > 3 { format!(" y {} más", n - 3) } else { String::new() };
    extracted(
        "underwriters",
        L_UNDERWRITERS,
        src,
        format!("{}{more}", top.join(", ")),
        Some(n as f64),
        Some("colocadores"),
        Vec::new(),
        Some(format!("{} colocadores en la tabla; total {}", n, fmt_int(total))),
        Some("Porcentaje = acciones que cada colocador se compromete a comprar sobre el total. No se valora su reputación.".into()),
    )
}

fn dilution(c: &Ctx, offer_price: Option<f64>) -> IpoRiskIndicator {
    let src = c.source("dilution");
    let Some(sec) = c.section("dilution") else {
        return not_extracted("dilution", L_DILUTION, src, c.missing_note("dilution"));
    };
    for r in rows(&sec.text) {
        let l = r.label.to_ascii_lowercase();
        if !l.contains("dilution") || !(l.contains("new investors") || l.contains("investors in this offering") || l.contains("purchasers") || l.contains("investors purchasing")) {
            continue;
        }
        let Some(v) = r.cells.first().filter(|c| c.contains('$')).and_then(|c| parse_amount(c)) else { continue };
        let mut note = String::from("Diferencia entre lo que paga el nuevo inversor por acción y el valor contable tangible pro forma por acción tras la oferta.");
        let value = match offer_price {
            Some(p) if p > 0.0 && v > 0.0 => {
                format!("{} USD por acción ({} % del precio de la OPV, {} USD)", fmt_dec(v, 2), fmt_dec(v / p * 100.0, 0), fmt_dec(p, 2))
            }
            _ => format!("{} USD por acción", fmt_dec(v, 2)),
        };
        if v < 0.0 {
            note.push_str(" Valor negativo en el folleto: el nuevo inversor paga menos que el valor contable (consulta la evidencia; puede deberse a una colocación privada concurrente).");
        }
        if r.cells.len() > 1 {
            note.push_str(" La tabla tiene varias columnas; se usa la primera.");
        }
        return extracted("dilution", L_DILUTION, src, value, Some(v), Some("USD/acción"), Vec::new(), Some(clip(&r.raw, 260)), Some(note));
    }
    not_extracted("dilution", L_DILUTION, src, "No se encontró la fila «Dilution per share to new investors» con un importe en dólares.")
}

fn vote_count(sentence: &str) -> Option<u64> {
    let l = sentence.to_ascii_lowercase();
    let mut best: Option<u64> = None;
    for pat in ["votes per share", "vote per share", "votes for each share", "votes for each"] {
        let mut from = 0;
        while let Some(p) = l[from..].find(pat) {
            let at = from + p;
            let head = &l[..at];
            let tok = head.split(|c: char| c.is_whitespace() || c == '(' || c == ')').rfind(|t| !t.is_empty()).unwrap_or("");
            let n = match tok.trim_matches(|c: char| !c.is_alphanumeric()) {
                "one" | "1" => Some(1),
                "two" => Some(2),
                "three" => Some(3),
                "five" => Some(5),
                "ten" => Some(10),
                "twenty" => Some(20),
                "fifty" => Some(50),
                "hundred" => Some(100),
                t => t.parse::<u64>().ok(),
            };
            if let Some(n) = n {
                best = Some(best.map_or(n, |b| b.max(n)));
            }
            from = at + pat.len();
        }
    }
    best
}

const VOTE_SECTIONS: &[&str] = &["summary", "offering", "principal_stockholders", "dividend_policy", "risk_factors"];

fn voting(c: &Ctx) -> IpoRiskIndicator {
    let available: Vec<&str> = VOTE_SECTIONS.iter().copied().filter(|k| c.section(k).is_some()).collect();
    if available.is_empty() {
        return not_extracted("voting", L_VOTING, c.source("offering"), c.missing_note("offering"));
    }
    let mut best: Option<(u64, String, &str)> = None;
    let mut dual_mention: Option<(String, &str)> = None;
    for k in &available {
        for s in sentences(&c.section(k).expect("existe").text) {
            if let Some(n) = vote_count(&s)
                && n >= 2
                && best.as_ref().is_none_or(|b| n > b.0)
            {
                best = Some((n, s.clone(), k));
            }
            let l = s.to_ascii_lowercase();
            if dual_mention.is_none() && (l.contains("dual class") || l.contains("dual-class") || l.contains("multi-class") || l.contains("multiple classes of")) {
                dual_mention = Some((s, k));
            }
        }
    }
    let classes_in_offering = c.section("offering").is_some_and(|s| {
        let l = s.text.to_ascii_lowercase();
        l.contains("class b common stock") || l.contains("class b ordinary shares") || l.contains("class b shares")
    });
    let searched = format!("Buscado en: {}.", available.iter().map(|k| section_es(k)).collect::<Vec<_>>().join(", "));
    if let Some((n, ev, sec)) = best {
        return extracted(
            "voting",
            L_VOTING,
            c.source(sec),
            format!("Voto múltiple: una clase tiene hasta {n} votos por acción"),
            Some(n as f64),
            Some("votos/acción"),
            vec![flag("multi_vote", "Acciones con voto múltiple (los fundadores pueden controlar con menos capital)", true, &ev)],
            Some(clip(&ev, 320)),
            Some(searched),
        );
    }
    if let Some((ev, sec)) = dual_mention {
        return extracted(
            "voting",
            L_VOTING,
            c.source(sec),
            "El folleto menciona varias clases de acciones; la relación de votos no se extrajo".into(),
            None,
            None,
            vec![flag("multi_class", "Varias clases de acciones", true, &ev)],
            Some(clip(&ev, 320)),
            Some(searched),
        );
    }
    if classes_in_offering {
        let class_b_row = c
            .section("offering")
            .and_then(|s| rows(&s.text).into_iter().find(|r| r.label.to_ascii_lowercase().contains("class b")))
            .map_or_else(|| "Class B (en «The Offering»)".to_string(), |r| r.raw);
        return extracted(
            "voting",
            L_VOTING,
            c.source("offering"),
            "Hay una Clase B además de la oferta; la relación de votos no se extrajo".into(),
            None,
            None,
            vec![flag("multi_class", "Varias clases de acciones", true, &class_b_row)],
            Some(clip(&class_b_row, 260)),
            Some(searched),
        );
    }
    extracted("voting", L_VOTING, c.source("offering"), "No se detectó voto múltiple".into(), None, None, Vec::new(), None, Some(format!("{searched} La ausencia de coincidencias no prueba que no exista.")))
}

fn section_es(key: &str) -> &'static str {
    match key {
        "summary" => "resumen",
        "offering" => "la oferta",
        "principal_stockholders" => "accionistas principales",
        "dividend_policy" => "política de dividendos",
        "risk_factors" => "factores de riesgo",
        _ => "otras secciones",
    }
}

fn controlled_company(c: &Ctx) -> IpoRiskIndicator {
    let available: Vec<&str> = VOTE_SECTIONS.iter().copied().filter(|k| c.section(k).is_some()).collect();
    if available.is_empty() {
        return not_extracted("controlled", L_CONTROLLED, c.source("offering"), c.missing_note("offering"));
    }
    for k in &available {
        let sents = sentences(&c.section(k).expect("existe").text);
        for (i, s) in sents.iter().enumerate() {
            if !s.to_ascii_lowercase().contains("controlled company") {
                continue;
            }
            // Un título corto («Status as a Controlled Company») se completa con las frases siguientes.
            let mut ev = if s.chars().count() < 80 { sents[i..sents.len().min(i + 3)].join(" ") } else { s.clone() };
            // El porcentaje de voto suele venir en la frase siguiente.
            if percent_near(&ev, "voting power").is_none()
                && let Some(next) = sents.get(i + 1)
                && next.to_ascii_lowercase().contains("voting power")
            {
                ev = format!("{ev} {next}");
            }
            let pct = percent_near(&ev, "voting power");
            let value = match pct {
                Some(p) => format!("Sí: se presenta como «controlled company» (un accionista controla ≈{} % de los votos)", fmt_dec(p, 0)),
                None => "Sí: el folleto la menciona como «controlled company» (real o posible)".to_string(),
            };
            return extracted(
                "controlled",
                L_CONTROLLED,
                c.source(k),
                value,
                pct,
                pct.map(|_| "%"),
                vec![flag("controlled_company", "Mencionada como «controlled company»", true, &ev)],
                Some(clip(&ev, 320)),
                Some("Una «empresa controlada» puede acogerse a exenciones de gobierno corporativo de la bolsa. Lee la frase: algunos folletos solo dicen que podría llegar a serlo.".into()),
            );
        }
    }
    extracted(
        "controlled",
        L_CONTROLLED,
        c.source("offering"),
        "No se detectó".into(),
        None,
        None,
        Vec::new(),
        None,
        Some(format!("Buscado en: {}. La ausencia de coincidencias no prueba que no lo sea.", available.iter().map(|k| section_es(k)).collect::<Vec<_>>().join(", "))),
    )
}

/// Primer «N %» o «N%» de la frase si ésta contiene `anchor`.
fn percent_near(s: &str, anchor: &str) -> Option<f64> {
    if !s.to_ascii_lowercase().contains(anchor) {
        return None;
    }
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'%' {
            let head = s[..i].trim_end();
            let num: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
            if let Ok(v) = num.parse::<f64>()
                && (0.0..=100.0).contains(&v)
            {
                return Some(v);
            }
        }
    }
    None
}

fn strip_footnote(label: &str) -> String {
    let lower = label.trim().to_ascii_lowercase();
    // Se recorta con trozos (`&str`) en vez de reasignar un `String` en cada vuelta: con
    // «net loss(1)(1)(1)…» copiar la etiqueta entera por cada nota era O(n²).
    let mut l: &str = &lower;
    // «net loss(1)» / «net loss (1)»
    while l.ends_with(')') {
        match l.rfind('(') {
            Some(p) if l[p + 1..l.len() - 1].chars().all(|c| c.is_ascii_digit() || c == ',') && !l[p + 1..l.len() - 1].is_empty() => {
                l = l[..p].trim_end();
            }
            _ => break,
        }
    }
    l.trim_end_matches([':', ',']).to_string()
}

fn units_of(text: &str) -> Option<&'static str> {
    let l = text.to_ascii_lowercase();
    if l.contains("in thousands") {
        Some("miles de USD")
    } else if l.contains("in millions") {
        Some("millones de USD")
    } else {
        None
    }
}

fn period_values(r: &Row) -> Vec<(f64, bool)> {
    // (valor, entre paréntesis). «—» y celdas vacías no cuentan como dato.
    r.cells
        .iter()
        .filter_map(|c| {
            let v = parse_amount(c)?;
            Some((v.abs(), c.contains('(')))
        })
        .collect()
}

fn net_result(c: &Ctx) -> IpoRiskIndicator {
    let src = c.source("summary_financial_data");
    let Some(sec) = c.section("summary_financial_data") else {
        return not_extracted("net_result", L_NET, src, c.missing_note("summary_financial_data"));
    };
    const LABELS: &[&str] = &[
        "net loss", "net income (loss)", "net (loss) income", "net income/(loss)", "net (loss)/income", "net loss and comprehensive loss",
        "net income", "net income and comprehensive income", "net loss and comprehensive loss attributable", "net (loss) income and comprehensive (loss) income",
        "net income (loss) and comprehensive income (loss)", "net profit (loss)",
    ];
    let Some(r) = rows(&sec.text).into_iter().find(|r| LABELS.contains(&strip_footnote(&r.label).as_str()) && !period_values(r).is_empty()) else {
        return not_extracted("net_result", L_NET, src, "No se encontró una fila «Net loss» o «Net income (loss)» con cifras en el resumen financiero.");
    };
    let vals = period_values(&r);
    let l = strip_footnote(&r.label);
    let both = l.contains("income") && l.contains("loss");
    let loss_only = l.contains("loss") && !l.contains("income");
    let n = vals.len();
    let paren = vals.iter().filter(|v| v.1).count();
    let unit = units_of(&sec.text).map_or("unidad no indicada en la sección".to_string(), |u| format!("cifras en {u}"));
    let loss_flag = || vec![flag("net_loss", "Pérdidas en todos los periodos", true, &r.raw)];
    let (value, flags): (String, Vec<IpoRiskFlag>) = if loss_only {
        // «Net loss»: se escribe con o sin paréntesis, pero siempre es pérdida; solo se rechaza
        // la mezcla de ambas convenciones en la misma fila.
        if paren == n || paren == 0 {
            (format!("Pérdida neta en los {n} periodos de la tabla"), loss_flag())
        } else {
            return not_extracted("net_result", L_NET, src, "La fila «Net loss» mezcla cifras con y sin paréntesis: el signo es ambiguo y no se interpreta.");
        }
    } else {
        // «Net income (loss)» (both) o «Net income»: paréntesis = pérdida, sin paréntesis = beneficio.
        let _ = both;
        if paren == n {
            (format!("Pérdida neta en los {n} periodos de la tabla"), loss_flag())
        } else if paren == 0 {
            (format!("Beneficio neto en los {n} periodos de la tabla"), Vec::new())
        } else {
            (format!("Resultado mixto: pérdida en {paren} y beneficio en {} de los {n} periodos de la tabla", n - paren), Vec::new())
        }
    };
    extracted(
        "net_result",
        L_NET,
        src,
        value,
        None,
        None,
        flags,
        Some(clip(&r.raw, 260)),
        Some(format!("{unit}. Los periodos mezclan años completos y trimestres o semestres y su orden varía según el folleto: no se interpretan las columnas, solo el signo.")),
    )
}

fn revenue(c: &Ctx) -> IpoRiskIndicator {
    let src = c.source("summary_financial_data");
    let Some(sec) = c.section("summary_financial_data") else {
        return not_extracted("revenue", L_REVENUE, src, c.missing_note("summary_financial_data"));
    };
    const LABELS: &[&str] = &[
        "revenue", "revenues", "total revenue", "total revenues", "net revenue", "net revenues", "revenues, net", "revenue, net",
        "net sales", "sales", "total net revenue", "total net revenues", "total net sales",
    ];
    let Some(r) = rows(&sec.text).into_iter().find(|r| LABELS.contains(&strip_footnote(&r.label).as_str()) && r.cells.iter().any(|c| c.chars().any(|ch| ch.is_ascii_digit() || ch == '—' || ch == '–'))) else {
        return not_extracted("revenue", L_REVENUE, src, "No se encontró una fila «Revenue» / «Total revenues» en el resumen financiero (las filas con otro nombre, como «Collaboration revenue», no se usan).");
    };
    let cells: Vec<&String> = r.cells.iter().filter(|c| !c.trim().is_empty() && **c != "$").collect();
    let zero = cells.iter().filter(|c| !c.chars().any(|ch| ch.is_ascii_digit()) || parse_amount(c) == Some(0.0)).count();
    let n = cells.len();
    let value = if zero == n {
        format!("Sin ingresos en los {n} periodos de la tabla")
    } else if zero > 0 {
        format!("Ingresos en {} de {n} periodos de la tabla ({zero} sin ingresos)", n - zero)
    } else {
        format!("Ingresos en los {n} periodos de la tabla")
    };
    let flags = if zero == n { vec![flag("no_revenue", "Sin ingresos", true, &r.raw)] } else { Vec::new() };
    let unit = units_of(&sec.text).map_or("unidad no indicada en la sección".to_string(), |u| format!("cifras en {u}"));
    extracted(
        "revenue",
        L_REVENUE,
        src,
        value,
        None,
        None,
        flags,
        Some(clip(&r.raw, 260)),
        Some(format!("{unit}. Fila tal como aparece; el orden de las columnas (años, semestres) no se interpreta.")),
    )
}

// ---------------------------------------------------------------------------------------------
// Tasa base
// ---------------------------------------------------------------------------------------------

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 })
}

fn pct_of(count: usize, n: usize) -> Option<f64> {
    (n > 0).then(|| count as f64 / n as f64 * 100.0)
}

/// Cortes de tamaño (importe de la oferta, USD) de la tasa base. Fijados ANTES de mirar resultados,
/// con criterio de mercado: por debajo de 50 M es micro-OPV (flotante diminuto, colocadores
/// pequeños); de 50 a 250 M, pequeña; desde 250 M, mediana y grande (con cobertura institucional).
pub const TIER_CUTS_USD: [f64; 2] = [50e6, 250e6];
/// Muestra mínima (con dato del primer día) para que un tramo se publique solo; si no, se fusiona.
pub const MIN_TIER_SAMPLE: usize = 10;

/// Importe de la oferta en USD para asignar el tramo: con precio fijado, el del calendario o
/// acciones × precio; si aún no hay precio definitivo, acciones × punto medio del rango (sin el
/// 15 % de sobreasignación que el calendario suma al máximo). `None` si no se puede calcular.
pub fn offer_size_usd(e: &IpoEntry) -> Option<f64> {
    let v = if e.status == IpoStatus::Priced {
        e.total_value.or_else(|| Some(e.shares? as f64 * e.price_low?))
    } else {
        let mid = match (e.price_low, e.price_high) {
            (Some(l), Some(h)) => Some((l + h) / 2.0),
            (Some(p), None) | (None, Some(p)) => Some(p),
            _ => None,
        };
        Some(e.shares? as f64 * mid?)
    };
    v.filter(|x| x.is_finite() && *x > 0.0)
}

/// Índice del tramo original (0 = < 50 M, 1 = 50–250 M, 2 = ≥ 250 M).
pub fn tier_index(size_usd: f64) -> usize {
    TIER_CUTS_USD.iter().filter(|c| size_usd >= **c).count()
}

fn tier_label(lo: usize, hi: usize) -> String {
    let (a, b) = (TIER_CUTS_USD[0] / 1e6, TIER_CUTS_USD[1] / 1e6);
    match (lo, hi) {
        (0, 0) => format!("< {a:.0} M USD"),
        (1, 1) => format!("{a:.0}–{b:.0} M USD"),
        (2, 2) => format!("≥ {b:.0} M USD"),
        (0, 1) => format!("< {b:.0} M USD"),
        (1, 2) => format!("≥ {a:.0} M USD"),
        _ => "todos los tamaños".to_string(),
    }
}

/// Una OPV de la muestra con el importe de su oferta (si se conoce).
#[derive(Debug, Clone)]
pub struct SizedPerf {
    pub size_usd: Option<f64>,
    pub perf: IpoPerformance,
}

struct Stats {
    sample: usize,
    n_day1: usize,
    n_3m: usize,
    median_day1: Option<f64>,
    positive_day1: Option<f64>,
    median_3m: Option<f64>,
    median_3m_excess: Option<f64>,
    beat_3m: Option<f64>,
    symbols: Vec<String>,
}

fn stats(perfs: &[&IpoPerformance]) -> Stats {
    let mut d1: Vec<f64> = perfs.iter().map(|p| p.first_day_return_pct).collect();
    let positive_d1 = d1.iter().filter(|v| **v > 0.0).count();
    let n_day1 = d1.len();
    let p3: Vec<&domain::IpoReturnPoint> = perfs.iter().filter_map(|p| p.points.iter().find(|x| x.horizon == "3m")).collect();
    let mut r3: Vec<f64> = p3.iter().filter_map(|x| x.return_pct).collect();
    let mut x3: Vec<f64> = p3.iter().filter_map(|x| x.excess_pct).collect();
    let beat = x3.iter().filter(|v| **v > 0.0).count();
    let n_3m = r3.len();
    Stats {
        sample: perfs.len(),
        n_day1,
        n_3m,
        median_day1: median(&mut d1),
        positive_day1: pct_of(positive_d1, n_day1),
        median_3m: median(&mut r3),
        median_3m_excess: median(&mut x3),
        beat_3m: pct_of(beat, n_3m),
        symbols: perfs.iter().map(|p| p.symbol.clone()).collect(),
    }
}

/// Tramos por tamaño con fusión: mientras algún tramo tenga menos de `MIN_TIER_SAMPLE` OPV con
/// dato del primer día, se fusiona con el contiguo (el de menor muestra si hay dos). Devuelve los
/// tramos finales y las notas de fusión.
fn build_tiers(items: &[SizedPerf]) -> (Vec<IpoBaseRateTier>, Vec<String>) {
    // (lo, hi, miembros) con lo/hi = índices de tramo original.
    let mut groups: Vec<(usize, usize, Vec<&IpoPerformance>)> = (0..=TIER_CUTS_USD.len()).map(|k| (k, k, Vec::new())).collect();
    let mut any = false;
    for it in items {
        if let Some(sz) = it.size_usd {
            groups[tier_index(sz)].2.push(&it.perf);
            any = true;
        }
    }
    if !any {
        return (Vec::new(), Vec::new());
    }
    let mut notes = Vec::new();
    while groups.len() > 1 {
        let Some(idx) = (0..groups.len()).filter(|&k| groups[k].2.len() < MIN_TIER_SAMPLE).min_by_key(|&k| groups[k].2.len()) else { break };
        let nb = if idx == 0 {
            1
        } else if idx == groups.len() - 1 || groups[idx - 1].2.len() <= groups[idx + 1].2.len() {
            idx - 1
        } else {
            idx + 1
        };
        let (a, b) = (idx.min(nb), idx.max(nb));
        let (la, lb) = (tier_label(groups[a].0, groups[a].1), tier_label(groups[b].0, groups[b].1));
        let (na, nb_) = (groups[a].2.len(), groups[b].2.len());
        let right = groups.remove(b);
        groups[a].1 = right.1;
        groups[a].2.extend(right.2);
        notes.push(format!(
            "El tramo «{la}» (n={na}) y el «{lb}» (n={nb_}) se han fusionado en «{}» porque alguno tenía menos de {MIN_TIER_SAMPLE} OPV en la ventana.",
            tier_label(groups[a].0, groups[a].1)
        ));
    }
    let tiers = groups
        .into_iter()
        .map(|(lo, hi, members)| {
            let st = stats(&members);
            let merged_from = if lo == hi { Vec::new() } else { (lo..=hi).map(|k| tier_label(k, k)).collect() };
            IpoBaseRateTier {
                label: tier_label(lo, hi),
                min_usd: if lo == 0 { 0.0 } else { TIER_CUTS_USD[lo - 1] },
                max_usd: TIER_CUTS_USD.get(hi).copied(),
                merged_from,
                sample: st.sample,
                n_day1: st.n_day1,
                n_3m: st.n_3m,
                median_day1_pct: st.median_day1,
                positive_day1_pct: st.positive_day1,
                median_3m_pct: st.median_3m,
                median_3m_excess_pct: st.median_3m_excess,
                beat_benchmark_3m_pct: st.beat_3m,
                symbols: st.symbols,
            }
        })
        .collect();
    (tiers, notes)
}

/// Tramo de la tasa base al que pertenece una oferta de `size_usd`.
pub fn tier_for(base: &IpoBaseRate, size_usd: f64) -> Option<&IpoBaseRateTier> {
    base.tiers.iter().find(|t| size_usd >= t.min_usd && t.max_usd.is_none_or(|m| size_usd < m))
}

/// Tasa base global de las OPV recientes (sin tramos). Las que aún no llegan a 3 meses cuentan
/// para el primer día pero no para el horizonte de 3 meses.
pub fn compute_base_rate(perfs: &[IpoPerformance], months: u32, benchmark: &str, now: DateTime<Utc>) -> IpoBaseRate {
    let items: Vec<SizedPerf> = perfs.iter().map(|p| SizedPerf { size_usd: None, perf: p.clone() }).collect();
    compute_base_rate_sized(&items, months, benchmark, now)
}

/// Como `compute_base_rate`, más la misma estadística por tramo de tamaño de la oferta.
pub fn compute_base_rate_sized(items: &[SizedPerf], months: u32, benchmark: &str, now: DateTime<Utc>) -> IpoBaseRate {
    let all: Vec<&IpoPerformance> = items.iter().map(|i| &i.perf).collect();
    let st = stats(&all);
    let (tiers, tier_notes) = build_tiers(items);
    IpoBaseRate {
        months,
        benchmark: benchmark.to_string(),
        sample: st.sample,
        n_day1: st.n_day1,
        n_3m: st.n_3m,
        median_day1_pct: st.median_day1,
        positive_day1_pct: st.positive_day1,
        median_3m_pct: st.median_3m,
        median_3m_excess_pct: st.median_3m_excess,
        beat_benchmark_3m_pct: st.beat_3m,
        symbols: st.symbols,
        source: "Calendario de OPV (Finnhub) y cierres de Yahoo Finance".to_string(),
        as_of: now,
        caveats: vec![
            "Rentabilidad de precio desde el precio de la OPV: solo la obtiene quien recibe acciones en la colocación, no quien compra al abrir.".into(),
            "Muestra limitada y sin SPAC (detección heurística); las OPV que se deslistaron pueden faltar (sesgo de supervivencia).".into(),
            "En micro-OPV (< 50 M USD) el capital flotante es muy pequeño y la volatilidad del primer día no es representativa.".into(),
            "Mediana de otras OPV: describe el momento del mercado, no predice cómo irá una OPV concreta.".into(),
        ],
        tiers,
        tier_notes,
    }
}

// ---------------------------------------------------------------------------------------------
// Servicio: tasa base con caché y adaptador al trait `IpoData`
// ---------------------------------------------------------------------------------------------

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures::{FutureExt, future::{BoxFuture, Shared}};
use moka::sync::Cache;
use sha2::{Digest, Sha256};

use crate::service::MarketService;

/// Vida de una tasa base calculada.
const BASE_TTL: Duration = Duration::from_secs(6 * 3600);
/// OPV máximas que entran en la muestra (cada una cuesta 2-3 peticiones la primera vez).
const BASE_SAMPLE: usize = 60;

type BaseFlight = Shared<BoxFuture<'static, Result<Arc<IpoBaseRate>, String>>>;

/// Caché de la tasa base y cálculo único en vuelo por ventana: peticiones simultáneas comparten
/// el mismo cálculo y, si el cliente se va, el cálculo sigue y deja el resultado en caché.
#[derive(Default)]
pub struct BaseRates {
    done: Mutex<HashMap<u32, (Instant, Arc<IpoBaseRate>)>>,
    flights: Mutex<HashMap<u32, BaseFlight>>,
}

impl MarketService {
    /// Tasa base de las OPV (con precio definitivo, sin SPAC) de los últimos `months` meses
    /// frente a SPY: medianas del primer día y de 3 meses, con el tamaño de muestra de cada una.
    pub async fn ipo_base_rate(self: &Arc<Self>, months: u32) -> domain::Result<IpoBaseRate> {
        let months = months.clamp(1, 24);
        if let Some((at, v)) = self.ipo_base.done.lock().expect("lock").get(&months)
            && at.elapsed() < BASE_TTL
        {
            return Ok((**v).clone());
        }
        let flight = {
            let mut flights = self.ipo_base.flights.lock().expect("lock");
            if let Some(f) = flights.get(&months) {
                f.clone()
            } else {
                let this = self.clone();
                let fut: BoxFuture<'static, Result<Arc<IpoBaseRate>, String>> = async move {
                    let res = async {
                        let items = this.sampled_ipo_performance(months, BASE_SAMPLE, "SPY").await.map_err(|e| e.to_string())?;
                        Ok::<_, String>(Arc::new(compute_base_rate_sized(&items, months, "SPY", Utc::now())))
                    }
                    .await;
                    if let Ok(v) = &res
                        && v.sample > 0
                    {
                        this.ipo_base.done.lock().expect("lock").insert(months, (Instant::now(), v.clone()));
                    }
                    this.ipo_base.flights.lock().expect("lock").remove(&months);
                    res
                }
                .boxed();
                let shared = fut.shared();
                // Se empuja en una tarea propia: sigue aunque el solicitante se desconecte.
                tokio::spawn(shared.clone());
                flights.insert(months, shared.clone());
                shared
            }
        };
        match flight.await {
            Ok(v) => Ok((*v).clone()),
            Err(e) => {
                tracing::warn!(error = %e, "no se pudo calcular la tasa base de OPV");
                Err(domain::DomainError::Unavailable("no se pudo calcular la tasa base de OPV recientes".into()))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Caché de perfiles
// ---------------------------------------------------------------------------------------------

/// Vida de un perfil en caché. Los datos de los que depende (folleto, calendario, tasa base) van
/// en la clave, así que caducar no es cuestión de frescura sino de no retener memoria sin uso.
const PROFILE_TTL: Duration = Duration::from_secs(30 * 60);
/// Tope de la caché en bytes (JSON serializado de los perfiles guardados). Un perfil normal pesa
/// 10-40 KB; uno que supere el tope entero no se guarda.
const PROFILE_CACHE_BYTES: u64 = 16 * 1024 * 1024;

/// Escribe en un `Sha256` lo que serializa `serde_json`: solo evita materializar el TEXTO JSON final
/// de la huella (una `String` del tamaño del folleto serializado). No evita la copia del folleto que
/// hace `profile_key` con `serde_json::to_value` (ver allí).
struct HashWriter<'a>(&'a mut Sha256);

impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Huella de TODO lo que determina el perfil (símbolo, calendario, folleto completo y tasa base;
/// no `now`): mismas entradas, mismo perfil. `None` si no se pudo serializar (entonces no se cachea).
///
/// Del folleto se descarta `fetchedAt` (cuándo se descargó): el perfil no lo usa, y una descarga
/// nueva del mismo documento no debe invalidar el perfil ya calculado. Si el campo cambiara de
/// nombre, el único efecto sería no reconocer esas descargas (más fallos de caché, nunca perfiles
/// equivocados).
///
/// Coste: para quitar `fetchedAt`, `serde_json::to_value(p)` COPIA el folleto entero en un
/// `serde_json::Value` temporal (proporcional al texto de sus secciones, unos MB en un folleto
/// grande) en cada llamada, también con la caché caliente. Solo se evita el texto JSON intermedio,
/// no esa copia.
fn profile_key(input: &ProfileInput<'_>) -> Option<[u8; 32]> {
    let mut hasher = Sha256::new();
    let prospectus = match input.prospectus {
        Some(p) => {
            let mut v = serde_json::to_value(p).ok()?;
            if let Some(o) = v.as_object_mut() {
                o.remove("fetchedAt");
            }
            Some(v)
        }
        None => None,
    };
    let value = (input.symbol.to_ascii_uppercase(), input.entry, prospectus, &input.base_rate);
    serde_json::to_writer(HashWriter(&mut hasher), &value).ok()?;
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    Some(key)
}

/// Peso de un perfil en la caché: su tamaño serializado.
fn profile_weight(p: &IpoRiskProfile) -> u32 {
    serde_json::to_vec(p).map_or(1 << 20, |b| b.len()).saturating_add(256).try_into().unwrap_or(u32::MAX)
}

/// Caché de perfiles de riesgo ya construidos, acotada en bytes y con caducidad.
///
/// `build_profile` es puro pero recorre todo el texto del folleto: cachearlo evita repetir ese
/// trabajo en cada recarga de la ficha, y `get_with` de moka hace que dos peticiones simultáneas
/// con las mismas entradas compartan un único cálculo (la segunda espera, no recalcula). El
/// cálculo es síncrono: llámese desde `spawn_blocking`, no desde una tarea async.
///
/// `generated_at` es el momento en que se construyó el perfil, que puede ser de hace hasta
/// `PROFILE_TTL` si salió de la caché.
pub struct ProfileCache {
    cache: Cache<[u8; 32], Arc<IpoRiskProfile>>,
    builds: AtomicU64,
}

impl Default for ProfileCache {
    fn default() -> Self {
        Self::with_limits(PROFILE_CACHE_BYTES, PROFILE_TTL)
    }
}

impl ProfileCache {
    /// Caché con tope de `max_bytes` (JSON serializado) y caducidad `ttl`.
    pub fn with_limits(max_bytes: u64, ttl: Duration) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_bytes)
            .weigher(|_key: &[u8; 32], value: &Arc<IpoRiskProfile>| profile_weight(value))
            .time_to_live(ttl)
            .build();
        Self { cache, builds: AtomicU64::new(0) }
    }

    /// Perfil de `input`: el guardado si ya se construyó con exactamente estas entradas; si no, lo
    /// construye y lo guarda.
    pub fn get_or_build(&self, input: ProfileInput<'_>) -> Arc<IpoRiskProfile> {
        let Some(key) = profile_key(&input) else {
            self.builds.fetch_add(1, Ordering::Relaxed);
            return Arc::new(build_profile(input));
        };
        self.cache.get_with(key, || {
            self.builds.fetch_add(1, Ordering::Relaxed);
            Arc::new(build_profile(input))
        })
    }

    /// Cuántas veces se ha construido un perfil de verdad (las lecturas de caché no cuentan).
    pub fn builds(&self) -> u64 {
        self.builds.load(Ordering::Relaxed)
    }

    /// Perfiles guardados y su peso total en bytes, tras aplicar las expulsiones pendientes.
    pub fn stats(&self) -> (u64, u64) {
        self.cache.run_pending_tasks();
        (self.cache.entry_count(), self.cache.weighted_size())
    }
}

/// Adaptador de `MarketService` al contrato `IpoData` del servidor.
#[derive(Clone)]
pub struct MarketIpo(pub Arc<MarketService>);

#[async_trait::async_trait]
impl domain::IpoData for MarketIpo {
    async fn calendar(&self, from: chrono::NaiveDate, to: chrono::NaiveDate) -> domain::Result<Vec<IpoEntry>> {
        self.0.ipo_calendar(from, to).await
    }

    async fn prospectus(&self, issuer: &str) -> domain::Result<Prospectus> {
        self.0.ipo_prospectus(issuer, false, 0).await
    }

    async fn performance(
        &self,
        symbol: &str,
        ipo_date: chrono::NaiveDate,
        offer_price: Option<f64>,
        benchmark: &str,
    ) -> domain::Result<IpoPerformance> {
        self.0.ipo_performance(symbol, ipo_date, offer_price, benchmark).await
    }

    async fn base_rate(&self, months: u32) -> domain::Result<IpoBaseRate> {
        self.0.ipo_base_rate(months).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_split_keeps_abbreviations() {
        let s = sentences("J.P. Morgan Securities LLC and Evercore Group L.L.C. are acting as managers. We will use the proceeds. Inc. owns it.");
        assert_eq!(s.len(), 3, "{s:?}");
        assert!(s[0].ends_with("managers."));
    }

    #[test]
    fn amounts_and_counts() {
        assert_eq!(parse_amount("$(11.36)"), Some(-11.36));
        assert_eq!(parse_amount("US$4.3801"), Some(4.3801));
        assert_eq!(parse_amount("$ 19.67"), Some(19.67));
        assert_eq!(parse_amount("$—"), None);
        assert_eq!(leading_count("8,333,334 shares."), Some(8_333_334));
        assert_eq!(leading_count("5% of the shares"), None);
        assert_eq!(leading_count("Immediately subsequent"), None);
        assert_eq!(leading_count("None."), None);
        assert_eq!(fmt_int(8_333_334), "8.333.334");
    }

    #[test]
    fn vote_words() {
        assert_eq!(vote_count("Holders of Class B are entitled to ten votes per share."), Some(10));
        assert_eq!(vote_count("Class A has one vote per share."), Some(1));
        assert_eq!(vote_count("no votes here"), None);
    }

    #[test]
    fn footnotes_in_labels() {
        assert_eq!(strip_footnote("Net loss(1)"), "net loss");
        assert_eq!(strip_footnote("Net loss (2)"), "net loss");
        assert_eq!(strip_footnote("Net income (loss)"), "net income (loss)");
    }

    // ---- Equivalencia con el código anterior a B1 ------------------------------------------------
    //
    // Los arreglos de coste cuadrático no deben cambiar ningún resultado. Cada función tocada se
    // compara con una copia literal de la versión original (el oráculo, con su coste cuadrático) sobre
    // entradas aleatorias deterministas, acotadas y con texto no ASCII, y sobre los folletos reales.

    /// Generador pseudoaleatorio determinista (xorshift64): mismas entradas en cada ejecución.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[(self.next() % items.len() as u64) as usize]
        }

        fn soup(&mut self, items: &[&str], pieces: usize) -> String {
            (0..pieces).map(|_| self.pick(items)).collect()
        }
    }

    /// `sentences` tal como era antes de B1.
    fn sentences_reference(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut logical: Vec<String> = Vec::new();
        for raw in text.lines() {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let cont = logical.last().is_some_and(|prev| {
                !prev.contains(" | ")
                    && !raw.contains(" | ")
                    && !prev.ends_with(['.', ':', ';', '!', '?', '”', '"'])
                    && raw.chars().next().is_some_and(|c| c.is_lowercase() || c.is_ascii_digit() || c == '$' || c == '(')
            });
            if cont {
                let last = logical.last_mut().expect("hay");
                last.push(' ');
                last.push_str(raw);
            } else {
                logical.push(raw.to_string());
            }
        }
        for line in logical.iter().map(String::as_str) {
            if line.is_empty() {
                continue;
            }
            let chars: Vec<(usize, char)> = line.char_indices().collect();
            let mut start = 0usize;
            for k in 0..chars.len() {
                let (i, c) = chars[k];
                if !matches!(c, '.' | '!' | '?' | ';') {
                    continue;
                }
                let Some(&(_, next)) = chars.get(k + 1) else { continue };
                if !next.is_whitespace() {
                    continue;
                }
                let after = chars[k + 1..].iter().map(|&(_, ch)| ch).find(|ch| !ch.is_whitespace());
                if !after.is_some_and(|a| a.is_uppercase() || matches!(a, '“' | '"' | '•' | '(')) {
                    continue;
                }
                let tok = line[start..i + c.len_utf8()].rsplit(char::is_whitespace).next().unwrap_or("");
                if c == '.' && is_abbrev(tok) {
                    continue;
                }
                let s = line[start..i + c.len_utf8()].trim();
                if !s.is_empty() {
                    out.push(s.to_string());
                }
                start = i + c.len_utf8();
            }
            let tail = line[start..].trim();
            if !tail.is_empty() {
                out.push(tail.to_string());
            }
        }
        out
    }

    /// `duration_candidates` tal como era antes de B1 (retroceso sin límite por las letras previas).
    fn duration_candidates_reference(text: &str) -> Vec<(usize, usize, u64, bool)> {
        let b = text.as_bytes();
        let lower = text.to_ascii_lowercase();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if !b[i].is_ascii_digit() || (i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b',' || b[i - 1] == b'.')) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let n: u64 = text[i..j].parse().unwrap_or(0);
            let mut k = j;
            if lower[k..].starts_with("th ") || lower[k..].starts_with("st ") || lower[k..].starts_with("nd ") || lower[k..].starts_with("rd ") {
                k += 2;
            }
            while k < b.len() && matches!(b[k], b' ' | b')' | b'-') {
                k += 1;
            }
            let tail = &lower[k..floor(&lower, k + 8)];
            let months = tail.starts_with("month");
            let days = tail.starts_with("day");
            if (days && (30..=1100).contains(&n)) || (months && (1..=36).contains(&n)) {
                out.push((i, k, n, months));
            }
            i = j;
        }
        const WORDS: &[(&str, u64)] = &[
            ("one", 1), ("two", 2), ("three", 3), ("four", 4), ("five", 5), ("six", 6), ("seven", 7), ("eight", 8),
            ("nine", 9), ("ten", 10), ("eleven", 11), ("twelve", 12), ("eighteen", 18),
        ];
        let mut from = 0;
        while let Some(p) = lower[from..].find("month") {
            let at = from + p;
            from = at + 5;
            let head = lower[..at].trim_end_matches([' ', '-']);
            let word_start = head.char_indices().rev().find(|(_, c)| !c.is_ascii_alphabetic()).map_or(0, |(x, c)| x + c.len_utf8());
            let w = &head[word_start..];
            if let Some(&(_, n)) = WORDS.iter().find(|(name, _)| *name == w) {
                out.push((word_start, at, n, true));
            }
        }
        out
    }

    /// `strip_footnote` tal como era antes de B1.
    fn strip_footnote_reference(label: &str) -> String {
        let mut l = label.trim().to_ascii_lowercase();
        while l.ends_with(')') {
            match l.rfind('(') {
                Some(p) if l[p + 1..l.len() - 1].chars().all(|c| c.is_ascii_digit() || c == ',') && !l[p + 1..l.len() - 1].is_empty() => {
                    l.truncate(p);
                    l = l.trim_end().to_string();
                }
                _ => break,
            }
        }
        l.trim_end_matches([':', ',']).to_string()
    }

    /// Texto de las secciones de los cinco folletos reales de los tests de integración.
    fn fixture_section_texts() -> Vec<String> {
        const FIXTURES: [&str; 5] = [
            include_str!("../tests/fixtures/sec_prospectus_trxb_excerpt.json"),
            include_str!("../tests/fixtures/sec_prospectus_lynx_excerpt.json"),
            include_str!("../tests/fixtures/sec_prospectus_oig_excerpt.json"),
            include_str!("../tests/fixtures/sec_prospectus_crwv_excerpt.json"),
            include_str!("../tests/fixtures/sec_prospectus_snsc_excerpt.json"),
        ];
        FIXTURES
            .iter()
            .flat_map(|j| serde_json::from_str::<Prospectus>(j).expect("fixture válido").sections)
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn sentence_split_matches_the_original_on_real_and_random_text() {
        let texts = fixture_section_texts();
        assert!(texts.len() >= 20, "los fixtures aportan secciones reales");
        for t in &texts {
            assert_eq!(sentences(t), sentences_reference(t));
        }
        let pieces = [
            "a", "b c", "foo |", "| y", " | ", "x | y", "Foo.", "J.P.", "Inc.", "end.", "The", "(1)", "$5", "5%", "…", "“q”", "é", ";", "!", "?", ".", ". ",
            "  ", " ",
        ];
        let seps = ["\n", "\n", " ", "\n\n", "  \n", "\t"];
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        for _ in 0..3_000 {
            let n = 1 + (rng.next() % 40) as usize;
            let mut t = String::new();
            for _ in 0..n {
                t.push_str(rng.pick(&pieces));
                t.push_str(rng.pick(&seps));
            }
            assert_eq!(sentences(&t), sentences_reference(&t), "{t:?}");
        }
        // Una unión que forma « | » (la línea acumulada termina en « |»): la siguiente ya no se pega.
        let t = "foo |\nbar\nbaz\nqux";
        assert_eq!(sentences(t), sentences_reference(t));
        assert_eq!(sentences(t), vec!["foo | bar", "baz qux"]);
    }

    #[test]
    fn duration_candidates_match_the_original_on_real_and_random_text() {
        for t in &fixture_section_texts() {
            assert_eq!(duration_candidates(t), duration_candidates_reference(t));
        }
        let pieces = [
            "six", "Six", "months", "month", "Month", "-", " ", "  ", "“", "”", "é", "seventeen", "eighteen", "neighteen", "eightee", "nineteen", "twelve",
            "eleven", "ten", "one", "two", "180", "days", "30", "day", "1", "3", "36", "37", "th ", "st ", "(", ")", "x", "abcdefgh", "abcdefghi", ",", ".", "5",
            "of this prospectus", "\n", "—", "日本",
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..6_000 {
            let n = 1 + (rng.next() % 30) as usize;
            let t = rng.soup(&pieces, n);
            assert_eq!(duration_candidates(&t), duration_candidates_reference(&t), "{t:?}");
        }
    }

    /// Los plazos reales de un lock-up siguen saliendo igual: en letra, en cifra, en meses y en días,
    /// y con comillas tipográficas (multibyte) pegadas a la palabra.
    #[test]
    fn real_lockup_phrasings_are_still_found() {
        let found = |t: &str| -> Vec<(u64, bool)> { duration_candidates(t).into_iter().map(|(_, _, n, m)| (n, m)).collect() };
        assert_eq!(found("for a period of six months from the date of this prospectus"), vec![(6, true)]);
        assert_eq!(found("for 180 days after the date of this prospectus"), vec![(180, false)]);
        assert_eq!(found("for a period of 3 months from the date"), vec![(3, true)]);
        assert_eq!(found("on the 180th day following the date"), vec![(180, false)]);
        assert_eq!(found("the 1st month after, “six months” and a twelve-month, eighteen months term"), vec![(1, true), (6, true), (12, true), (18, true)]);
        assert_eq!(found("xxxxxxxxxxxxsix months and neighteen months"), Vec::new(), "una palabra más larga que «eighteen» no coincide");
    }

    #[test]
    fn footnote_stripping_matches_the_original() {
        let pieces = ["Net loss", "net income (loss)", "(1)", "(2,3)", "()", "(a)", " (4)", "(1", "1)", ":", ",", " ", "é", ")", "(", "Revenue"];
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for _ in 0..4_000 {
            let n = 1 + (rng.next() % 12) as usize;
            let t = rng.soup(&pieces, n);
            assert_eq!(strip_footnote(&t), strip_footnote_reference(&t), "{t:?}");
        }
        assert_eq!(strip_footnote(&format!("Net loss{}", "(1)".repeat(5_000))), "net loss");
    }
}
