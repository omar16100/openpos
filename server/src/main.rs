//! openpos server binary.
//!
//! Ships as a single static binary so self-hosting is a small image plus
//! Postgres, rather than a runtime and a dependency tree.

use std::net::SocketAddr;

use openpos_server::http::{router, AppState};
use openpos_server::repo::MemoryRepo;

/// Startup failures are returned rather than panicked, so an operator reading
/// `docker logs` sees one readable line instead of a backtrace.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openpos_server=info".into()),
        )
        .init();

    // The Postgres repository is next. Until then this runs entirely in memory,
    // which is useful for a demo and useless for a shop, so it says so loudly.
    let repo = MemoryRepo::new();
    repo.enrol(1, 1);
    tracing::warn!("running with an in-memory store: nothing survives a restart");

    let listen = std::env::var("OPENPOS_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let address: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "openpos server listening");

    axum::serve(listener, router(AppState::new(repo)))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Finish in-flight requests on SIGINT rather than dropping a sync mid-batch.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
