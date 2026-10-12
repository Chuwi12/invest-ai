use std::{sync::Arc, time::Duration};

use anyhow::Context;
use domain::{Analyst, MarketData, ModelService, NewsItem};
use server::{AppState, Settings, config::Config, db, fake, repo::settings::SqliteSettings};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// Tiempo máximo que se espera a las conexiones abiertas y tareas en curso al apagar.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Mercado, analista, anotador de sentimiento y, con el analista real, la gestión de modelos.
type Services = (Arc<dyn MarketData>, Arc<dyn Analyst>, fn(&mut [NewsItem]), Option<Arc<dyn ModelService>>);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `.env` es opcional; busca en el directorio actual y en los superiores.
    let dotenv = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,server=debug")))
        .init();
    match dotenv {
        Ok(path) => tracing::info!(path = %path.display(), ".env cargado"),
        Err(_) => tracing::debug!("sin fichero .env"),
    }

    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    let (market, analyst, annotate_news, models) = build_services(&config, &pool).await?;

    let mut state = AppState::new(
        market,
        analyst,
        pool.clone(),
        Settings { language: config.language.clone(), allowed_origins: config.frontend_origins.clone() },
    )
    .with_news_annotator(annotate_news);
    if let Some(models) = models {
        state = state.with_models(models);
    }

    match &config.static_dir {
        Some(dir) => tracing::info!(dir = %dir.display(), "sirviendo el frontend compilado"),
        None => tracing::info!("sin frontend/dist: solo API (usa el servidor de desarrollo de Vite)"),
    }
    let app = server::app(state.clone(), config.static_dir.clone());

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("no se pudo abrir {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, fake = config.fake, "invest-ai escuchando");

    let shutdown = state.shutdown.clone();
    let mut server = tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            axum::serve(listener, app).with_graceful_shutdown(async move { shutdown.cancelled().await }).await
        }
    });

    tokio::select! {
        res = &mut server => {
            // El servidor terminó solo (error de accept, normalmente).
            res.context("la tarea del servidor falló")?.context("error del servidor")?;
        }
        () = wait_for_signal() => {
            tracing::info!("apagando: cerrando conexiones y terminando tareas en curso");
            // Cierra WebSockets y SSE (si no, el apagado ordenado esperaría a que el cliente se vaya).
            shutdown.cancel();
            match tokio::time::timeout(SHUTDOWN_GRACE, &mut server).await {
                Ok(res) => res.context("la tarea del servidor falló")?.context("error del servidor")?,
                Err(_) => tracing::warn!("hay conexiones que no cerraron a tiempo: se fuerza el apagado"),
            }
        }
    }

    // Deja terminar de guardar los chats y análisis que estuvieran en marcha.
    state.tasks.close();
    if tokio::time::timeout(SHUTDOWN_GRACE, state.tasks.wait()).await.is_err() {
        tracing::warn!("quedaron tareas en segundo plano sin terminar");
    }
    pool.close().await;
    tracing::info!("hasta luego");
    Ok(())
}

async fn build_services(config: &Config, pool: &sqlx::SqlitePool) -> anyhow::Result<Services> {
    if config.fake {
        tracing::warn!("INVEST_AI_FAKE activo: datos y analista simulados");
        let market = fake::FakeMarket { tick_interval: Duration::from_millis(250), ..Default::default() };
        return Ok((Arc::new(market), Arc::new(fake::FakeAnalyst::default()), fake::annotate_fake, None));
    }
    // Datos reales: proveedores con fallback, caché y tiempo real (ver docs/notes/market.md).
    let market = market::MarketService::new(market::MarketConfig::from_env())
        .context("no se pudo iniciar el servicio de mercado")?;
    // LLM local: el modelo activo se guarda en la tabla `settings` (ver docs/notes/ai.md).
    let settings = Arc::new(SqliteSettings::new(pool.clone()));
    let analyst = ai::OllamaAnalyst::new(ai::AiConfig::from_env(), settings)
        .map_err(|e| anyhow::anyhow!("no se pudo iniciar el analista: {e}"))?;
    let models = analyst.models();
    Ok((market, Arc::new(analyst), ai::annotate_sentiment, Some(models)))
}

/// Espera Ctrl+C o SIGTERM.
async fn wait_for_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "no se pudo escuchar Ctrl+C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "no se pudo escuchar SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
