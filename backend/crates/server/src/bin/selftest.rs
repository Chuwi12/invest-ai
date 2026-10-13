//! Prueba de humo de una instancia en marcha (sin LLM salvo `--llm`). La lanza `./invest-ai selftest`.
//!
//! Uso: `invest-ai-selftest http://127.0.0.1:8787 [--llm]`. Sale con 0 si todo está OK y con 1 si
//! algún punto falla. Solo usa datos reales de la instancia (no simula nada).

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tokio_tungstenite::tungstenite::Message;

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|e| format!("JSON no válido: {e}"))
    }
}

/// GET/POST mínimo HTTP/1.1 sobre TCP (sin dependencias de cliente HTTP).
async fn request(addr: &str, method: &str, path: &str, host: &str, timeout: Duration) -> Result<Resp, String> {
    let work = async {
        let s = TcpStream::connect(addr).await.map_err(|e| format!("no conecta con {addr}: {e}"))?;
        let mut rd = BufReader::new(s);
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: identity\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        rd.get_mut().write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut line = String::new();
        rd.read_line(&mut line).await.map_err(|e| e.to_string())?;
        let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("respuesta HTTP no válida")?;
        let mut headers = Vec::new();
        let mut chunked = false;
        let mut len = None::<usize>;
        loop {
            line.clear();
            rd.read_line(&mut line).await.map_err(|e| e.to_string())?;
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_owned());
                if k == "content-length" {
                    len = v.parse().ok();
                }
                if k == "transfer-encoding" && v.contains("chunked") {
                    chunked = true;
                }
                headers.push((k, v));
            }
        }
        let mut body = Vec::new();
        if chunked {
            loop {
                line.clear();
                rd.read_line(&mut line).await.map_err(|e| e.to_string())?;
                let sz = usize::from_str_radix(line.trim(), 16).unwrap_or(0);
                let mut buf = vec![0u8; sz + 2];
                rd.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
                if sz == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..sz]);
            }
        } else if let Some(n) = len {
            body.resize(n, 0);
            rd.read_exact(&mut body).await.map_err(|e| e.to_string())?;
        } else {
            rd.read_to_end(&mut body).await.map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(Resp { status, headers, body })
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| format!("sin respuesta en {}s", timeout.as_secs()))?
}

async fn get(addr: &str, path: &str) -> Result<Resp, String> {
    request(addr, "GET", path, addr, Duration::from_secs(15)).await
}

fn age_secs(ts: &str) -> Result<i64, String> {
    let t: DateTime<Utc> = DateTime::parse_from_rfc3339(ts).map_err(|e| format!("marca de tiempo no válida «{ts}»: {e}"))?.into();
    Ok((Utc::now() - t).num_seconds())
}

fn human(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s} s"),
        s if s < 7200 => format!("{} min", s / 60),
        s if s < 172_800 => format!("{} h", s / 3600),
        s => format!("{} días", s / 86400),
    }
}

/// Antigüedad máxima razonable de una cotización según el estado del mercado.
fn max_age(state: &str) -> i64 {
    match state {
        "continuous" => 180,     // cripto: tiempo real
        "regular" => 20 * 60,    // sesión abierta (hay proveedores con 15 min de retraso)
        "pre" | "post" => 20 * 3600, // la quote regular es la del cierre anterior
        _ => 5 * 86400,          // cerrado: fin de semana largo incluido
    }
}

async fn check_quote(addr: &str, id: &str) -> Result<String, String> {
    let r = get(addr, &format!("/api/assets/{id}/quote")).await?;
    if r.status != 200 {
        return Err(format!("HTTP {}", r.status));
    }
    let q = r.json()?;
    let price = q["price"].as_f64().filter(|p| *p > 0.0 && p.is_finite()).ok_or("sin precio válido")?;
    let source = q["source"].as_str().filter(|s| !s.is_empty()).ok_or("sin fuente")?;
    let ts = q["timestamp"].as_str().ok_or("sin marca de tiempo")?;
    let state = q["marketState"].as_str().unwrap_or("unknown");
    let age = age_secs(ts)?;
    if age < -60 {
        return Err(format!("marca de tiempo en el futuro ({ts})"));
    }
    if age > max_age(state) {
        return Err(format!("cotización de hace {} con el mercado «{state}» (máximo razonable {})", human(age), human(max_age(state))));
    }
    Ok(format!("{price} {} fuente={source} mercado={state} hace {}", q["currency"].as_str().unwrap_or("?"), human(age.max(0))))
}

async fn check_ws(addr: &str) -> Result<String, String> {
    let work = async {
        let (mut sock, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws"))
            .await
            .map_err(|e| format!("no conecta: {e}"))?;
        sock.send(Message::text(r#"{"type":"subscribe","ids":["crypto:BTC"]}"#)).await.map_err(|e| e.to_string())?;
        while let Some(m) = sock.next().await {
            let Message::Text(t) = m.map_err(|e| e.to_string())? else { continue };
            let v: Value = serde_json::from_str(t.as_str()).map_err(|e| e.to_string())?;
            match v["type"].as_str() {
                Some("tick") => {
                    let tick = &v["tick"];
                    let price = tick["price"].as_f64().filter(|p| *p > 0.0).ok_or("tick sin precio")?;
                    let age = age_secs(tick["ts"].as_str().ok_or("tick sin ts")?)?;
                    if age > 180 {
                        return Err(format!("tick de hace {}", human(age)));
                    }
                    return Ok(format!("crypto:BTC {price} fuente={} hace {}", tick["source"].as_str().unwrap_or("?"), human(age.max(0))));
                }
                Some("error") => return Err(format!("el servidor respondió error: {}", v["message"])),
                _ => {}
            }
        }
        Err("el socket se cerró sin ticks".into())
    };
    tokio::time::timeout(Duration::from_secs(15), work).await.map_err(|_| "ningún tick en 15 s".to_string())?
}

async fn check_models(addr: &str) -> Result<String, String> {
    let r = get(addr, "/api/models").await?;
    if r.status != 200 {
        return Err(format!("HTTP {}", r.status));
    }
    let v = r.json()?;
    let hw = &v["hardware"];
    let threads = hw["cpuThreads"].as_u64().filter(|t| *t > 0).ok_or("hardware sin cpuThreads")?;
    let ram = hw["ramTotalGb"].as_f64().filter(|r| *r > 0.0).ok_or("hardware sin ramTotalGb")?;
    let gpus: Vec<String> = hw["gpus"].as_array().map(|a| a.iter().filter_map(|g| g["name"].as_str().map(String::from)).collect()).unwrap_or_default();
    let active = v["active"].as_str().filter(|s| !s.is_empty()).ok_or("sin modelo activo")?;
    if v["ollamaReachable"].as_bool() != Some(true) {
        return Err("Ollama no es alcanzable".into());
    }
    let installed = v["models"].as_array().into_iter().flatten().any(|m| m["id"].as_str() == Some(active) && m["installed"].as_bool() == Some(true));
    if !installed {
        return Err(format!("el modelo activo {active} no está instalado (ollama pull {active})"));
    }
    Ok(format!("activo={active} instalado; {threads} hilos, {ram:.0} GB RAM, GPU: {}", if gpus.is_empty() { "ninguna".into() } else { gpus.join(", ") }))
}

async fn check_headers(addr: &str) -> Result<String, String> {
    let r = get(addr, "/api/health").await?;
    let mut missing = Vec::new();
    for (name, want) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        ("cache-control", "no-store"),
    ] {
        match r.header(name) {
            Some(v) if v.eq_ignore_ascii_case(want) => {}
            other => missing.push(format!("{name}={other:?} (esperado {want})")),
        }
    }
    if r.header("permissions-policy").is_none() {
        missing.push("permissions-policy ausente".into());
    }
    // Anti DNS rebinding: un Host que no es loopback debe rechazarse.
    let evil = request(addr, "GET", "/api/health", "evil.example.com", Duration::from_secs(5)).await?;
    if evil.status != 403 {
        missing.push(format!("Host ajeno aceptado (HTTP {})", evil.status));
    }
    if missing.is_empty() { Ok("cabeceras de seguridad presentes; Host ajeno -> 403".into()) } else { Err(missing.join("; ")) }
}

async fn check_llm(addr: &str) -> Result<String, String> {
    let t0 = Instant::now();
    let r = request(addr, "POST", "/api/assets/stock:AAPL/analysis?depth=quick", addr, Duration::from_secs(240)).await?;
    if r.status != 200 {
        return Err(format!("HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(200).collect::<String>()));
    }
    let a = r.json()?;
    let verdict = a["verdict"].as_str().ok_or("sin veredicto")?;
    if a["summary"].as_str().is_none_or(str::is_empty) {
        return Err("sin resumen".into());
    }
    Ok(format!("veredicto={verdict} en {:.1} s ({} llamadas al modelo, {} tokens de salida)", t0.elapsed().as_secs_f64(), a["llmCalls"], a["completionTokens"]))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args.first().cloned().unwrap_or_else(|| "http://127.0.0.1:8787".into());
    let llm = args.iter().any(|a| a == "--llm");
    let addr = base.trim_start_matches("http://").trim_end_matches('/').to_owned();
    println!("selftest contra {base}{}", if llm { " (con análisis quick)" } else { "" });

    let t0 = Instant::now();
    let mut failed = 0;
    let mut report = |name: &str, res: Result<String, String>| match res {
        Ok(detail) => println!("  OK     {name}: {detail}"),
        Err(why) => {
            failed += 1;
            println!("  FALLO  {name}: {why}");
        }
    };

    let health = async {
        let r = get(&addr, "/api/health").await?;
        let ok = r.status == 200 && r.json()?["status"] == "ok";
        if ok { Ok(format!("versión {}", r.json()?["version"].as_str().unwrap_or("?"))) } else { Err(format!("HTTP {}", r.status)) }
    };
    // Las comprobaciones de datos van en paralelo: el conjunto cabe en pocos segundos.
    let (h, aapl, btc, ws, models, headers) = tokio::join!(
        health,
        check_quote(&addr, "stock:AAPL"),
        check_quote(&addr, "crypto:BTC"),
        check_ws(&addr),
        check_models(&addr),
        check_headers(&addr),
    );
    report("health", h);
    report("cotización stock:AAPL", aapl);
    report("cotización crypto:BTC", btc);
    report("tick por WebSocket", ws);
    report("/models (hardware y modelo activo)", models);
    report("cabeceras de seguridad", headers);
    if llm {
        report("análisis quick stock:AAPL", check_llm(&addr).await);
    }
    let secs = t0.elapsed().as_secs_f64();
    if failed == 0 {
        println!("RESULTADO: todo OK ({secs:.1} s)");
    } else {
        println!("RESULTADO: {failed} punto(s) con FALLO ({secs:.1} s)");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_limits_follow_the_market_state() {
        assert!(max_age("continuous") < max_age("regular"));
        assert!(max_age("regular") < max_age("pre"));
        assert!(max_age("pre") < max_age("closed"));
        // Un cierre de viernes visto el lunes por la mañana sigue siendo razonable.
        assert!(max_age("closed") >= 3 * 86400);
        assert_eq!(max_age("algo-raro"), max_age("closed"));
    }

    #[test]
    fn timestamps_parse_and_report_age() {
        let now = Utc::now().to_rfc3339();
        assert!(age_secs(&now).unwrap().abs() <= 2);
        assert!(age_secs("ayer").is_err());
        assert_eq!(human(30), "30 s");
        assert_eq!(human(3 * 3600), "3 h");
        assert_eq!(human(3 * 86400), "3 días");
    }
}
