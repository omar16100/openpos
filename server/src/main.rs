//! openpos server binary.
//!
//! Ships as a single static binary so self-hosting is a small image plus
//! Postgres, rather than a runtime and a dependency tree.
//!
//! Configuration is three environment variables:
//!
//! - `OPENPOS_LISTEN`, default `0.0.0.0:8080`
//! - `OPENPOS_DATABASE_URL`, the application role. Without it the server runs in
//!   memory, which is a demo and says so.
//! - `OPENPOS_ADMIN_DATABASE_URL`, optional, used once at startup to migrate.
//!   Separate because migrating needs rights the running application must not
//!   have: the app role can read and write rows and cannot alter the schema it
//!   is audited against.

use std::net::SocketAddr;

use openpos_server::http::{router, AppState};
use openpos_server::pg::PgRepo;
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

    let listen = std::env::var("OPENPOS_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let address: SocketAddr = listen.parse()?;

    match std::env::var("OPENPOS_DATABASE_URL") {
        Ok(url) => {
            if let Ok(admin) = std::env::var("OPENPOS_ADMIN_DATABASE_URL") {
                tracing::info!("running migrations");
                PgRepo::migrate(&admin).await?;
            } else {
                tracing::info!(
                    "no OPENPOS_ADMIN_DATABASE_URL, assuming the schema is already current"
                );
            }
            let repo = PgRepo::connect(&url, 16).await?;
            tracing::info!(%address, "openpos server listening, backed by postgres");
            serve(address, router(AppState::new(repo))).await?;
        }
        Err(_) => {
            let repo = MemoryRepo::new();
            repo.enrol(1, 1);
            tracing::warn!(
                "no OPENPOS_DATABASE_URL: running with an in-memory store, nothing survives a restart"
            );
            tracing::info!(%address, "openpos server listening");
            serve(address, router(AppState::new(repo))).await?;
        }
    }
    Ok(())
}

async fn serve(address: SocketAddr, app: axum::Router) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    // Connect info is what lets enrolment be rate limited per client. Without
    // it every caller shares one bucket, so one machine guessing codes would
    // lock out every shop trying to enrol a tablet.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

/// Finish in-flight requests on SIGINT rather than dropping a sync mid-batch.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
