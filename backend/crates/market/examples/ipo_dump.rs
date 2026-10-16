//! Herramienta de desarrollo (red real): `ipo_dump <directorio> <emisor>...` guarda el `Prospectus`
//! de cada emisor como JSON y muestra el perfil de riesgo calculado.
//! Necesita `SEC_USER_AGENT` en el entorno.
#[tokio::main]
async fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("directorio de salida");
    std::fs::create_dir_all(&dir).expect("crear directorio");
    let s = market::MarketService::new(market::MarketConfig::from_env()).expect("servicio");
    for issuer in a {
        match s.ipo_prospectus(&issuer, false, 0).await {
            Ok(p) => {
                let path = format!("{dir}/{}.json", issuer.to_ascii_lowercase());
                std::fs::write(&path, serde_json::to_string_pretty(&p).expect("json")).expect("escribir");
                println!("{issuer}: {} {} {} -> {path}", p.issuer, p.form, p.filed_on);
            }
            Err(e) => println!("{issuer}: ERROR {e}"),
        }
    }
}
