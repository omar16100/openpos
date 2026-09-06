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
//! - `OPENPOS_DEV_ALLOW_ORIGIN`, unset in production. One browser origin allowed
//!   to call this server, for running the till from `npm run dev`.
//! - `OPENPOS_TRUSTED_PROXY_HOPS`, default 0. How many reverse proxies sit in
//!   front. Set to 1 behind Caddy or Cloudflare, or enrolment rate limiting
//!   sees every client as the proxy and throttles the whole world as one.
//!   Separate because migrating needs rights the running application must not
//!   have: the app role can read and write rows and cannot alter the schema it
//!   is audited against.

use std::net::SocketAddr;
use std::time::Duration;

use openpos_core::protocol::ItemWire;
use openpos_server::auth::{Caller, EnrolmentCode, Role};
use openpos_core::auth::PinHash;
use openpos_server::repo::{OperatorRecord, Repository, ShopDetails};

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

    // Zero unless an operator says otherwise. Both documented deployments put a
    // proxy in front, and behind one every request shares a single rate-limit
    // bucket, so this needs setting to 1 for Caddy or for Cloudflare. It is not
    // defaulted to 1 because a server that trusts a forwarded header nobody
    // overwrites lets a caller invent an address and mint a fresh budget per
    // request, which is worse than one shared bucket.
    // Development only. In the shipped image the server serves the till and the
    // admin app itself, so nothing is cross-origin and no browser asks.
    let dev_origin = std::env::var("OPENPOS_DEV_ALLOW_ORIGIN").ok();
    if let Some(origin) = dev_origin.as_deref() {
        tracing::warn!(%origin, "allowing one cross-origin caller: this is a development setting");
    }

    let proxy_hops: usize = std::env::var("OPENPOS_TRUSTED_PROXY_HOPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if proxy_hops == 0 {
        tracing::info!(
            "no OPENPOS_TRUSTED_PROXY_HOPS: rate limiting by socket address. Set it to 1 behind Caddy or Cloudflare, or every client shares one bucket"
        );
    }

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
            serve(
                address,
                router(
                    AppState::new(repo)
                        .with_trusted_proxy_hops(proxy_hops)
                        .with_dev_allow_origin(dev_origin.clone()),
                ),
            )
            .await?;
        }
        Err(_) => {
            let repo = MemoryRepo::new();
            let (tenant, terminal) = (1_u128, 1_u128);
            repo.enrol(tenant, terminal);

            // A demo that cannot be reached is not a demo. Every route but one
            // needs a credential, and until now this mode enrolled a terminal
            // whose token nobody could retrieve and issued no code to get one,
            // so the only thing a person could do with it was read the health
            // endpoint.
            //
            // An enrolment code rather than a token, because that is what the
            // product already has for exactly this: short, single use, short
            // lived, and meant to be read off a screen and typed into a device.
            // Printing a bearer token instead would put a long-lived credential
            // in a log file and teach the habit.
            let code = EnrolmentCode::generate();
            repo.issue_enrolment_code(
                Caller {
                    tenant,
                    terminal,
                    role: Role::Owner,
                },
                &code.hash(),
                Duration::from_secs(60 * 60),
            )
            .await
            .map_err(|_| "the in-memory store refused an enrolment code")?;

            // What goes at the top of a receipt. Without it a till enrols, sells,
            // and prints paper with an empty line where the shop should be.
            repo.put_shop_details(
                tenant,
                &ShopDetails {
                    name: "Demo General Store".to_owned(),
                    bin: Some("000000000-0000".to_owned()),
                    address: Some("Demo data, not a real shop".to_owned()),
                    phone: None,
                },
            )
            .await
            .map_err(|_| "the in-memory store refused the shop details")?;

            // Somebody to stand at the till. Without a person, nobody can sign
            // in, and every permission check refuses: the demo would enrol,
            // sell nothing that needs authority, and give no clue why.
            let owner_id = 1_u128;
            let owner_pin = PinHash::derive("1234", DEMO_SALT, PIN_ROUNDS);
            repo.put_operator(
                tenant,
                &OperatorRecord {
                    id: owner_id,
                    name: "Demo Owner".to_owned(),
                    pin_salt: DEMO_SALT.to_vec(),
                    pin_rounds: PIN_ROUNDS,
                    pin_key: owner_pin.key().to_vec(),
                    max_discount_bp: 10_000,
                    may_override_price: true,
                    may_refund: true,
                    may_void_line: true,
                    may_authorise: true,
                    may_open_drawer: true,
                    may_close_shift: true,
                    active: true,
                },
            )
            .await
            .map_err(|_| "the in-memory store refused an operator")?;

            // A catalogue, so a till that enrols has something to sell.
            for item in demo_catalogue() {
                repo.upsert_item(tenant, item);
            }

            tracing::warn!(
                "no OPENPOS_DATABASE_URL: running with an in-memory store, nothing survives a restart"
            );
            tracing::info!(
                "demo operator: Demo Owner, PIN 1234. Demo data only; a real shop sets its own"
            );
            tracing::info!(
                enrolment_code = %code.as_str(),
                "demo shop ready. Enrol a till with this code; it expires in an hour and works once"
            );
            tracing::info!(%address, "openpos server listening");
            serve(
                address,
                router(
                    AppState::new(repo)
                        .with_trusted_proxy_hops(proxy_hops)
                        .with_dev_allow_origin(dev_origin.clone()),
                ),
            )
            .await?;
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

/// A fixed salt, because this is demo data that lives for one process and is
/// printed in the log beside the PIN it protects. A real operator's salt is
/// random and comes from the owner's device.
const DEMO_SALT: [u8; openpos_core::auth::SALT_LEN] = [7; openpos_core::auth::SALT_LEN];

/// Low on purpose: this runs at startup on whatever machine is demonstrating,
/// and the PIN is in the log anyway.
const PIN_ROUNDS: u32 = 1_000;

/// A few things to sell, so a demo till has a catalogue rather than an empty
/// screen and no way to fill it.
///
/// Prices in poisha and VAT in basis points, as everything else here is.
fn demo_catalogue() -> Vec<ItemWire> {
    [
        (1_u128, "RICE5", "Rice Miniket 5kg", "মিনিকেট চাল ৫ কেজি", 43_000_i64, "8690000000001"),
        (2, "OIL1", "Soybean Oil 1L", "সয়াবিন তেল ১ লিটার", 18_500, "8690000000002"),
        (3, "DAL1", "Masoor Dal 1kg", "মসুর ডাল ১ কেজি", 14_000, "8690000000003"),
        (4, "SUG1", "Sugar 1kg", "চিনি ১ কেজি", 12_500, "8690000000004"),
        (5, "TEA400", "Tea 400g", "চা ৪০০ গ্রাম", 22_000, "8690000000005"),
    ]
    .into_iter()
    .map(|(id, code, name_en, name_bn, price_minor, barcode)| ItemWire {
        id,
        code: code.to_owned(),
        name_en: name_en.to_owned(),
        name_bn: name_bn.to_owned(),
        unit: "Nos".to_owned(),
        price_minor,
        // Eighty percent of the price, so a margin is visible without
        // inventing a second column of made-up numbers.
        cost_minor: price_minor.saturating_mul(4).saturating_div(5),
        vat_bp: 1_500,
        price_inclusive: false,
        vat_on_undiscounted: false,
        barcodes: vec![barcode.to_owned()],
        on_hand_milli: 40_000,
        active: true,
    })
    .collect()
}

/// Finish in-flight requests on SIGINT rather than dropping a sync mid-batch.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
