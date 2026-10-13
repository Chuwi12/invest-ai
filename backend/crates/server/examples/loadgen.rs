//! Cliente de carga mínimo (no hay oha/wrk/ab en la máquina). Solo para medir en local.
//!
//! ```text
//! loadgen http  http://127.0.0.1:8791 -n 2000 -c 50 [--gzip] /api/dashboard [otra/ruta ...]
//! loadgen ws    http://127.0.0.1:8791 --clients 20 --secs 30 crypto:BTC crypto:ETH ...
//! ```
//! `http`: conexiones keep-alive, una por cliente; las rutas se reparten en round-robin.
//! `ws`: cada cliente se suscribe a todos los ids; mide ticks/s y la latencia entre la marca de
//! tiempo del proveedor (`ts`) y la recepción en el cliente.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tokio_tungstenite::tungstenite::Message;

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_default();
    let base = args.get(1).cloned().unwrap_or_default();
    let addr = base.trim_start_matches("http://").trim_end_matches('/').to_owned();
    let (mut n, mut c, mut clients, mut secs, mut gzip) = (100usize, 1usize, 20usize, 30u64, false);
    let mut rest = Vec::new();
    let mut it = args.into_iter().skip(2);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-n" => n = it.next().and_then(|v| v.parse().ok()).unwrap_or(n),
            "-c" => c = it.next().and_then(|v| v.parse().ok()).unwrap_or(c),
            "--clients" => clients = it.next().and_then(|v| v.parse().ok()).unwrap_or(clients),
            "--secs" => secs = it.next().and_then(|v| v.parse().ok()).unwrap_or(secs),
            "--gzip" => gzip = true,
            _ => rest.push(a),
        }
    }
    match mode.as_str() {
        "http" => http(addr, n, c, gzip, rest).await,
        "ws" => ws(addr, clients, secs, rest).await,
        _ => eprintln!("uso: loadgen http|ws BASE ..."),
    }
}

/// Una petición GET sobre una conexión keep-alive. Devuelve (estado, bytes del cuerpo).
async fn get(
    rd: &mut BufReader<TcpStream>,
    addr: &str,
    path: &str,
    gzip: bool,
) -> std::io::Result<(u16, usize)> {
    let enc = if gzip { "gzip, br" } else { "identity" };
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nAccept-Encoding: {enc}\r\n\r\n");
    rd.get_mut().write_all(req.as_bytes()).await?;
    let mut line = String::new();
    rd.read_line(&mut line).await?;
    let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let (mut len, mut chunked) = (None::<usize>, false);
    loop {
        line.clear();
        rd.read_line(&mut line).await?;
        let l = line.trim_end().to_ascii_lowercase();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("content-length:") {
            len = v.trim().parse().ok();
        } else if l.starts_with("transfer-encoding:") && l.contains("chunked") {
            chunked = true;
        }
    }
    let mut total = 0usize;
    if chunked {
        loop {
            line.clear();
            rd.read_line(&mut line).await?;
            let sz = usize::from_str_radix(line.trim(), 16).unwrap_or(0);
            let mut buf = vec![0u8; sz + 2];
            rd.read_exact(&mut buf).await?;
            if sz == 0 {
                break;
            }
            total += sz;
        }
    } else if let Some(n) = len {
        let mut buf = vec![0u8; n];
        rd.read_exact(&mut buf).await?;
        total = n;
    }
    Ok((status, total))
}

async fn http(addr: String, n: usize, c: usize, gzip: bool, paths: Vec<String>) {
    let paths = Arc::new(paths);
    let next = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..c {
        let (addr, paths, next) = (addr.clone(), paths.clone(), next.clone());
        tasks.push(tokio::spawn(async move {
            let mut lat = Vec::new();
            let (mut errs, mut bytes, mut codes) = (0usize, 0usize, std::collections::BTreeMap::<u16, usize>::new());
            let Ok(s) = TcpStream::connect(&addr).await else { return (lat, 1, 0, codes) };
            s.set_nodelay(true).ok();
            let mut rd = BufReader::new(s);
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let t = Instant::now();
                match get(&mut rd, &addr, &paths[i % paths.len()], gzip).await {
                    Ok((st, b)) => {
                        lat.push(t.elapsed().as_secs_f64() * 1000.0);
                        bytes += b;
                        *codes.entry(st).or_default() += 1;
                    }
                    Err(_) => {
                        errs += 1;
                        let Ok(s) = TcpStream::connect(&addr).await else { break };
                        rd = BufReader::new(s);
                    }
                }
            }
            (lat, errs, bytes, codes)
        }));
    }
    let (mut lat, mut errs, mut bytes) = (Vec::new(), 0, 0usize);
    let mut codes = std::collections::BTreeMap::<u16, usize>::new();
    for t in tasks {
        let (l, e, b, cs) = t.await.unwrap();
        lat.extend(l);
        errs += e;
        bytes += b;
        for (k, v) in cs {
            *codes.entry(k).or_default() += v;
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "{{\"path\":{:?},\"n\":{},\"c\":{},\"errs\":{},\"codes\":{},\"rps\":{:.1},\"p50_ms\":{:.2},\"p95_ms\":{:.2},\"p99_ms\":{:.2},\"max_ms\":{:.2},\"avg_bytes\":{}}}",
        paths.first().cloned().unwrap_or_default(),
        lat.len(),
        c,
        errs,
        format!("{{{}}}", codes.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect::<Vec<_>>().join(",")),
        lat.len() as f64 / wall,
        pct(&lat, 0.50),
        pct(&lat, 0.95),
        pct(&lat, 0.99),
        pct(&lat, 1.0),
        bytes / lat.len().max(1)
    );
}

async fn ws(addr: String, clients: usize, secs: u64, ids: Vec<String>) {
    let end = Instant::now() + Duration::from_secs(secs);
    let mut tasks = Vec::new();
    for _ in 0..clients {
        let (addr, ids) = (addr.clone(), ids.clone());
        tasks.push(tokio::spawn(async move {
            let url = format!("ws://{addr}/api/ws");
            let mut lat = Vec::new();
            let Ok((mut sock, _)) = tokio_tungstenite::connect_async(url).await else { return (lat, 1usize) };
            let sub = serde_json::json!({ "type": "subscribe", "ids": ids }).to_string();
            sock.send(Message::text(sub)).await.ok();
            while Instant::now() < end {
                let left = end.saturating_duration_since(Instant::now());
                match tokio::time::timeout(left, sock.next()).await {
                    Ok(Some(Ok(Message::Text(t)))) => {
                        let now = chrono::Utc::now();
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t.as_str())
                            && let Some(ts) = v["tick"]["ts"].as_str()
                            && let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts)
                        {
                            lat.push((now - ts.with_timezone(&chrono::Utc)).num_milliseconds() as f64);
                        }
                    }
                    Ok(Some(Ok(_))) => {}
                    _ => break,
                }
            }
            (lat, 0)
        }));
    }
    let (mut lat, mut failed) = (Vec::new(), 0);
    for t in tasks {
        let (l, f) = t.await.unwrap();
        lat.extend(l);
        failed += f;
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "{{\"clients\":{},\"failed\":{},\"secs\":{},\"ticks\":{},\"ticks_per_sec_total\":{:.1},\"ticks_per_sec_per_client\":{:.2},\"ts_to_recv_ms\":{{\"p50\":{:.0},\"p95\":{:.0},\"p99\":{:.0}}}}}",
        clients,
        failed,
        secs,
        lat.len(),
        lat.len() as f64 / secs as f64,
        lat.len() as f64 / secs as f64 / clients as f64,
        pct(&lat, 0.5),
        pct(&lat, 0.95),
        pct(&lat, 0.99)
    );
}
