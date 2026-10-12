//! Prueba real contra el Ollama local con datos de fixture (sin red de mercado).
//!
//!   cargo run -p ai --example live -- models
//!   cargo run -p ai --example live -- analyze [quick|standard|deep] [stock:AAPL]
//!   cargo run -p ai --example live -- chat "¿cómo ves Apple?"

use std::{collections::HashMap, sync::{Arc, Mutex}};

use ai::{AiConfig, OllamaAnalyst, fixture::FakeMarket};
use async_trait::async_trait;
use domain::{Analyst, ChatEvent, ChatRequest, MarketData, SettingsStore};
use futures::StreamExt;

#[derive(Default)]
struct Mem(Mutex<HashMap<String, String>>);

#[async_trait]
impl SettingsStore for Mem {
    async fn get(&self, k: &str) -> domain::Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(k).cloned())
    }
    async fn set(&self, k: &str, v: &str) -> domain::Result<()> {
        self.0.lock().unwrap().insert(k.into(), v.into());
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let analyst = OllamaAnalyst::new(AiConfig::from_env(), Arc::new(Mem::default()))?;
    let market: Arc<dyn MarketData> = Arc::new(FakeMarket::new());
    match args.first().map(String::as_str) {
        Some("models") => {
            let r = analyst.models().list().await;
            println!("{}", serde_json::to_string_pretty(&r)?);
            println!("{}", serde_json::to_string_pretty(&analyst.status().await)?);
        }
        Some("analyze") => {
            let depth = args.get(1).map(|d| d.parse()).transpose()?;
            let id = args.get(2).map(String::as_str).unwrap_or("stock:AAPL").parse()?;
            let snap = market.snapshot(&id).await?;
            let t = std::time::Instant::now();
            let a = analyst.analyze_with(&snap, depth).await?;
            println!("{}", serde_json::to_string_pretty(&a)?);
            eprintln!("tiempo total: {:.0} s", t.elapsed().as_secs_f32());
        }
        Some("chat") => {
            let req = ChatRequest {
                conversation_id: "live".into(),
                history: vec![],
                user_message: args.get(1).cloned().unwrap_or_else(|| "¿cómo ves Apple?".into()),
                focus: vec![],
                market,
                language: "es".into(),
            };
            let mut s = analyst.chat(req);
            while let Some(ev) = s.next().await {
                match ev {
                    ChatEvent::Token { text } => print!("{text}"),
                    other => println!("\n[{}]", serde_json::to_string(&other)?),
                }
            }
            println!();
        }
        _ => eprintln!("uso: live models | analyze [depth] [id] | chat \"mensaje\""),
    }
    Ok(())
}
