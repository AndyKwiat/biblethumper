//! Bible Thumper server (radify app contract: listen on 127.0.0.1:$PORT,
//! answer /healthz, log to stdout).

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_ansi(false) // journald, not a terminal
        .init();
    let t = std::time::Instant::now();
    let engine = tokio::task::spawn_blocking(bt_server::assets::load_engine).await??;
    tracing::info!(
        "engine ready in {:?} ({})",
        t.elapsed(),
        engine.embedder_id()
    );

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    tracing::info!("listening on http://{host}:{port}");
    // ConnectInfo gives the rate limiter the peer address when no proxy headers exist.
    let app = bt_server::router(Arc::new(engine))
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app).await?;
    Ok(())
}
