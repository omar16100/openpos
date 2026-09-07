//! Copyright (C) 2026 the openpos authors.
//!
//! This program is free software: you can redistribute it and modify it under
//! the terms of the GNU Affero General Public License as published by the Free
//! Software Foundation, version 3. It is distributed in the hope that it will
//! be useful, and with no warranty: see the LICENSE file at the root of this
//! repository, or <https://www.gnu.org/licenses/>.
//!
//! Section 13 is the one that matters here and is why this licence was chosen:
//! anybody who runs a modified copy of this as a service for other people has
//! to offer those people its source. A shop's own copy, modified for its own
//! counter, is its own business.

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

use openpos_core::auth::PinHash;
use openpos_core::protocol::ItemWire;
use openpos_server::auth::{Caller, EnrolmentCode, Role};
use openpos_server::repo::{GoodsReceipt, OperatorRecord, ReceiptLine, Repository, ShopDetails};

use openpos_server::http::{AppState, router};
use openpos_server::pg::PgRepo;
use openpos_server::repo::MemoryRepo;

/// What the command line asked for, when it asked for something other than
/// serving.
enum Asked {
    /// `openpos-server export <shop>`, where the shop is the id its own bundle
    /// and its own logs use. Refused rather than guessed at: exporting the
    /// wrong shop is handing somebody a file full of another shop's takings.
    Export(u128),
    /// `openpos-server import [--as <shop>]`, reading the bundle on stdin.
    ///
    /// Without an id this is a restore: the shop keeps the id it had, because
    /// the tills still hold sales carrying it and giving the shop a new one
    /// would orphan every outbox in the building. With one it is a copy into an
    /// install that may already hold the shop.
    Import(Option<u128>),
}

fn shop_id(named: &str) -> Result<u128, Box<dyn std::error::Error>> {
    openpos_core::ids::Ulid::decode(named)
        .map(|id| id.to_u128())
        .or_else(|_| uuid::Uuid::parse_str(named).map(|id| id.as_u128()))
        .map_err(|_| format!("{named} is not a shop id").into())
}

fn asked() -> Result<Option<Asked>, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Ok(None);
    };
    match command.as_str() {
        "export" => {
            let named = args
                .next()
                .ok_or("which shop? give the id it is known by")?;
            Ok(Some(Asked::Export(shop_id(&named)?)))
        }
        "import" => match args.next().as_deref() {
            None => Ok(Some(Asked::Import(None))),
            Some("--as") => {
                let named = args.next().ok_or("--as needs the id to put it under")?;
                Ok(Some(Asked::Import(Some(shop_id(&named)?))))
            }
            Some(other) => Err(format!("import takes --as <shop>, not {other}").into()),
        },
        other => Err(format!("no such command: {other}").into()),
    }
}

/// Stop if this connection can see past the shop boundary.
///
/// Row level security is the whole of the isolation here, so a role that
/// bypasses it has none: one shop's till would read another's takings and
/// nothing anywhere would say so. The mistake is one word in a connection
/// string, `postgres` where `openpos_app` was meant, and it looks exactly like
/// a server that works.
///
/// Refused rather than warned about. A warning in a log nobody reads is what a
/// shop finds out about from its customers.
async fn refuse_a_role_that_sees_every_shop(
    repo: &PgRepo,
) -> Result<(), Box<dyn std::error::Error>> {
    if repo.can_see_every_shop().await? {
        return Err(
            "this connects as a role that can see past every shop's boundary, which turns \
                    row level security off: use the unprivileged role, openpos_app in the \
                    documented setup, and keep the superuser for migrations"
                .into(),
        );
    }
    Ok(())
}

/// Startup failures are returned rather than panicked, so an operator reading
/// `docker logs` sees one readable line instead of a backtrace.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        // To stderr, so that stdout carries only what was asked for. A shop's
        // backup is written there, and a log line in the middle of it is a
        // bundle that will not read back.
        .with_writer(std::io::stderr)
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
    // Where the built apps are, when this image carries them. Absent in
    // development, where they are served by whatever is running vite.
    let apps = std::env::var("OPENPOS_APPS").ok();
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

    // Taking a backup, rather than serving. Everything a shop owns, written to
    // a file: the export has existed since the week it was needed and nothing
    // could ask for it, which made it a library with tests rather than a thing
    // an operator can do.
    //
    // A subcommand rather than a route, because it is an operator's act on the
    // machine the database is on, and because a shop's whole ledger is not
    // something to hand out over HTTP to whoever holds a credential today.
    if let Some(command) = asked()? {
        let url = std::env::var("OPENPOS_DATABASE_URL")
            .map_err(|_| "OPENPOS_DATABASE_URL is needed to read or write a shop")?;
        let repo = PgRepo::connect(&url, 4).await?;
        // The same check as serving, and for the same reason: an export taken
        // on a role that sees every shop is a file with every shop in it.
        refuse_a_role_that_sees_every_shop(&repo).await?;
        match command {
            Asked::Export(tenant) => {
                let mut out = std::io::BufWriter::new(std::io::stdout().lock());
                openpos_server::export::stream_tenant(&repo, tenant, &mut out)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
            }
            Asked::Import(under) => {
                // Read whole before anything is written. A bundle is a shop, and
                // a half-read file that had already started writing would leave
                // an install holding half of one.
                let bundle =
                    openpos_server::export::ExportBundle::read_jsonl(std::io::stdin().lock())
                        .map_err(|error| format!("{error:?}"))?;
                let policy = match under {
                    None => openpos_server::export::IdentityPolicy::Preserve,
                    Some(id) => openpos_server::export::IdentityPolicy::Rehome(id),
                };
                let outcome = openpos_server::export::import_tenant(&repo, &bundle, policy)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                // To stderr with everything else, so a script that pipes a
                // bundle in gets nothing on stdout it did not ask for.
                tracing::info!(
                    shop = %uuid::Uuid::from_u128(outcome.tenant),
                    sales = outcome.sales_added,
                    catalogue = outcome.catalogue_added,
                    movements = outcome.movements_added,
                    accounts = outcome.accounts_added,
                    customers = outcome.customers_taken,
                    drawers = outcome.shifts_taken,
                    people = outcome.operators_taken,
                    suppliers = outcome.suppliers_taken,
                    "the shop is in"
                );
                if outcome.operators_taken > 0 {
                    // Said out loud, because nothing else will say it until a
                    // cashier is standing at a till with a queue behind them.
                    tracing::warn!(
                        people = outcome.operators_taken,
                        "no PIN travels in a bundle: set one for each of these before anybody can sign in"
                    );
                }
            }
        }
        return Ok(());
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
            refuse_a_role_that_sees_every_shop(&repo).await?;
            // Only when asked. Demo data in a shop's real database would be a
            // catalogue nobody ordered and a person nobody hired, and the PIN
            // is printed in this file.
            if std::env::var("OPENPOS_DEMO").is_ok() {
                seed_demo(&repo).await?;
            }
            tracing::info!(%address, "openpos server listening, backed by postgres");
            serve(
                address,
                with_the_apps(
                    router(
                        AppState::new(repo)
                            .with_trusted_proxy_hops(proxy_hops)
                            .with_dev_allow_origin(dev_origin.clone()),
                    ),
                    apps.as_deref(),
                ),
            )
            .await?;
        }
        Err(_) => {
            let repo = MemoryRepo::new();
            seed_demo(&repo).await?;

            tracing::warn!(
                "no OPENPOS_DATABASE_URL: running with an in-memory store, nothing survives a restart"
            );
            tracing::info!(%address, "openpos server listening");
            serve(
                address,
                with_the_apps(
                    router(
                        AppState::new(repo)
                            .with_trusted_proxy_hops(proxy_hops)
                            .with_dev_allow_origin(dev_origin.clone()),
                    ),
                    apps.as_deref(),
                ),
            )
            .await?;
        }
    }
    Ok(())
}

/// Serve the till and the back office out of this binary, when a directory of
/// them was given.
///
/// `OPENPOS_APPS` points at what the build produced: the till at the root and
/// the back office under `/admin/`. A shop that self-hosts then runs one image
/// and one database rather than a web server to configure as well, and nothing
/// is cross-origin, which is why the development origin allowance is a
/// development thing.
///
/// Anything the file service cannot find falls back to that app's own
/// `index.html`, because both are single-page apps and a reload on any path
/// inside one has to reach it rather than a 404.
fn with_the_apps(router: axum::Router, apps: Option<&str>) -> axum::Router {
    let Some(home) = apps else {
        return router;
    };
    let home = std::path::Path::new(home);
    let till = tower_http::services::ServeDir::new(home)
        .append_index_html_on_directories(true)
        .fallback(tower_http::services::ServeFile::new(
            home.join("index.html"),
        ));
    let admin = home.join("admin");
    let back_office = tower_http::services::ServeDir::new(&admin)
        .append_index_html_on_directories(true)
        .fallback(tower_http::services::ServeFile::new(
            admin.join("index.html"),
        ));
    tracing::info!(apps = %home.display(), "serving the till and the back office");
    router
        .nest_service("/admin", back_office)
        .fallback_service(till)
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

/// Put a shop in an empty store: details, a person, a catalogue, the goods that
/// catalogue claims, and two codes to enrol with.
///
/// Backend-agnostic, so the same demo can be run on Postgres and survive a
/// restart. It was memory-only, which made every check of anything that has to
/// outlive a restart impossible to do against the demo.
///
/// Two codes and two terminals, not one. Two apps on one origin keep their
/// stores in directories named for their terminal, so a back office and a till
/// enrolling with the same code fight over the same files, and the failure
/// reads as a complaint about access handles. That cost an hour today, twice.
async fn seed_demo<R: Repository>(repo: &R) -> Result<(), String> {
    // Every failure here means the same thing to whoever ran this: the store
    // would not take the demo. Which call refused is in the message.
    fn refused(what: &str) -> impl Fn(openpos_server::repo::RepoError) -> String + '_ {
        move |error| format!("the store refused {what}: {error:?}")
    }

    let tenant = 1_u128;
    let (back_office, till) = (1_u128, 2_u128);

    // Idempotent by this check, so a demo on Postgres can be restarted without
    // a second shop appearing beside the first.
    if repo.shop_details(tenant).await.is_ok() {
        tracing::info!("demo shop already here, leaving it alone");
        return Ok(());
    }

    repo.register_terminal(tenant, back_office, "Demo back office")
        .await
        .map_err(refused("a terminal"))?;
    repo.register_terminal(tenant, till, "Demo front counter")
        .await
        .map_err(refused("a terminal"))?;

    // What goes at the top of a receipt. Without it a till enrols, sells, and
    // prints paper with an empty line where the shop should be.
    repo.put_shop_details(
        tenant,
        &ShopDetails {
            name: "Demo General Store".to_owned(),
            bin: Some("000000000-0000".to_owned()),
            address: Some("Demo data, not a real shop".to_owned()),
            phone: None,
            // The two a shop here would actually take, so the demo shows the
            // till offering them by name rather than a blank dropdown.
            wallets: vec!["bKash".to_owned(), "Nagad".to_owned()],
            // Told rather than stopped, so the demo shows the rule without a
            // demo catalogue's figures stopping anybody selling.
            stock_rule: 1,
        },
    )
    .await
    .map_err(refused("the shop details"))?;

    // Somebody to stand at the till. Without a person, nobody can sign in, and
    // every permission check refuses: the demo would enrol, sell nothing that
    // needs authority, and give no clue why.
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
    .map_err(refused("an operator"))?;

    for item in demo_catalogue() {
        repo.upsert_item(tenant, &item)
            .await
            .map_err(refused("a catalogue item"))?;
    }

    // Goods actually arriving, so the demo shop has stock the same way a real
    // one does rather than asserting a number on a product record.
    repo.receive_goods(
        tenant,
        &GoodsReceipt {
            id: 1,
            supplier_id: None,
            reference: Some("demo opening delivery".to_owned()),
            // A real time, because zero renders as 1970 on every screen that
            // shows a delivery, and a shop reading that learns to distrust the
            // column rather than the one row.
            received_at_ms: now_ms(),
            received_by: owner_id,
            note: None,
            lines: demo_catalogue()
                .iter()
                .map(|item| ReceiptLine {
                    item_id: item.id,
                    qty_milli: 40_000,
                    unit_cost_minor: item.cost_minor,
                })
                .collect(),
        },
    )
    .await
    .map_err(refused("the opening delivery"))?;

    // Codes rather than tokens: short, single use, short lived, and meant to be
    // read off a screen. Printing a bearer token would put a long-lived
    // credential in a log file and teach the habit.
    for (terminal, role, what) in [
        (back_office, Role::Owner, "the back office at /admin/"),
        (till, Role::Till, "a till at /"),
    ] {
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller {
                tenant,
                terminal,
                role,
            },
            &code.hash(),
            Duration::from_secs(60 * 60),
        )
        .await
        .map_err(refused("an enrolment code"))?;
        tracing::info!(
            enrolment_code = %code.as_str(),
            "enrol {what} with this code; it expires in an hour and works once"
        );
    }

    tracing::info!("demo operator: Demo Owner, PIN 1234. Demo data only; a real shop sets its own");
    Ok(())
}

/// The wall clock, in milliseconds.
fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default(),
    )
    .unwrap_or_default()
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
    // The last column is whether tax is charged on the listed price, so a
    // discount comes out of the shop's margin instead of reducing the tax. One
    // item has it, because a demo where every item is taxed the same way cannot
    // show the difference, and the difference is the whole point of the setting.
    [
        (
            1_u128,
            "RICE5",
            "Rice Miniket 5kg",
            "মিনিকেট চাল ৫ কেজি",
            43_000_i64,
            "8690000000001",
            false,
        ),
        (
            2,
            "OIL1",
            "Soybean Oil 1L",
            "সয়াবিন তেল ১ লিটার",
            18_500,
            "8690000000002",
            false,
        ),
        (
            3,
            "DAL1",
            "Masoor Dal 1kg",
            "মসুর ডাল ১ কেজি",
            14_000,
            "8690000000003",
            false,
        ),
        (
            4,
            "SUG1",
            "Sugar 1kg",
            "চিনি ১ কেজি",
            12_500,
            "8690000000004",
            false,
        ),
        (
            5,
            "TEA400",
            "Tea 400g",
            "চা ৪০০ গ্রাম",
            22_000,
            "8690000000005",
            false,
        ),
        (
            6,
            "LISTED100",
            "Listed price 100.00",
            "তালিকা মূল্য ১০০.০০",
            10_000,
            "8690000000006",
            true,
        ),
        // One line at nothing, because a shop here sells taxed and untaxed
        // goods in the same basket all day, and every path that adds them up
        // has to meet that on an ordinary run rather than only in a test. Named
        // for what it demonstrates rather than for a real good: which goods
        // this country exempts is the revenue's word, not this file's.
        (
            7,
            "ZERO",
            "Zero-rated example",
            "শূন্য হারের উদাহরণ",
            5_000,
            "8690000000007",
            false,
        ),
    ]
    .into_iter()
    .map(
        |(id, code, name_en, name_bn, price_minor, barcode, vat_on_undiscounted)| ItemWire {
            id,
            code: code.to_owned(),
            name_en: name_en.to_owned(),
            name_bn: name_bn.to_owned(),
            unit: "Nos".to_owned(),
            price_minor,
            // Eighty percent of the price, so a margin is visible without
            // inventing a second column of made-up numbers.
            cost_minor: price_minor.saturating_mul(4).saturating_div(5),
            // Fifteen percent, except the one line that shows a shop what a
            // basket with something untaxed in it looks like.
            vat_bp: if code == "ZERO" { 0 } else { 1_500 },
            price_inclusive: false,
            vat_on_undiscounted,
            barcodes: vec![barcode.to_owned()],
            // Zero on the item record, deliberately. Stock is what deliveries,
            // sales and counts add up to, not a number typed on a product, and a
            // catalogue that asserts forty bags nobody ever delivered is a figure
            // the shop cannot explain and the stock screen contradicts.
            on_hand_milli: 0,
            active: true,
            // Seeded by the shop, not typed at a counter.
            from_a_till: false,
        },
    )
    .collect()
}

/// Finish in-flight requests on SIGINT rather than dropping a sync mid-batch.
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
