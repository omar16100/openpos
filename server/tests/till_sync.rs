//! A real till against a real server.
//!
//! Every layer below this has its own tests, which is exactly why this one
//! matters: the failures that survive unit testing live at the seams. Here the
//! till's storage, its outbox, the protocol encoding, the HTTP surface and the
//! server's ingest all have to agree at once.
//!
//! The shape of the test is the shape of a shop's day: pull the catalogue, take
//! a block of receipt numbers, sell with no connection, then sync when the
//! network returns.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::lease::Lease;
use openpos_core::money::{Milli, Minor};
use openpos_core::protocol::{
    ItemWire, LeaseRequest, LeaseResponse, PullRequest, PullResponse, PushRequest, PushResponse,
    PROTOCOL_VERSION,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_core::till::Till;
use openpos_server::http::{router, AppState};
use openpos_server::repo::MemoryRepo;
use tower::ServiceExt;

const TENANT: u128 = 42;
const TERMINAL: u128 = 7;

fn item(id: u128, price_minor: i64) -> ItemWire {
    ItemWire {
        id,
        code: format!("SKU{id:03}"),
        name_en: format!("Item {id}"),
        name_bn: format!("পণ্য {id}"),
        unit: "Nos".to_owned(),
        price_minor,
        cost_minor: price_minor / 2,
        vat_bp: 1_500,
        price_inclusive: false,
        barcodes: vec![format!("869000000{id:04}")],
        on_hand_milli: 40_000,
        active: true,
    }
}

/// A server with a shop, a terminal and a small catalogue.
fn shop() -> Router {
    let repo = MemoryRepo::new();
    repo.enrol(TENANT, TERMINAL);
    repo.upsert_item(TENANT, item(1, 43_000));
    repo.upsert_item(TENANT, item(2, 47_500));
    router(AppState::new(repo))
}

async fn call<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    app: &Router,
    path: &str,
    body: &T,
) -> (StatusCode, R) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .body(Body::from(postcard::to_allocvec(body).unwrap()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, postcard::from_bytes::<R>(&bytes).unwrap())
}

fn pay_cash(till: &mut Till<MemoryBackend>, amount: i64) {
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(amount),
        reference: None,
    });
}

#[tokio::test]
async fn a_shop_opens_sells_offline_and_syncs_when_the_network_returns() {
    let server = shop();
    let (mut till, boot) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();
    assert_eq!(boot.items, 0, "a new terminal knows nothing yet");

    // Morning: fetch the catalogue and a block of receipt numbers.
    let (status, page): (_, PullResponse) = call(
        &server,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 100,
        },
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    till.apply_pull(&deltas_from_pull(&page)).unwrap();
    assert_eq!(till.catalogue().len(), 2);

    let (_, granted): (_, LeaseResponse) = call(
        &server,
        "/v1/lease",
        &LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 500,
        },
    )
    .await;
    till.grant_lease(&Lease::new(
        Ulid::from_u128(TERMINAL),
        granted.epoch,
        &granted.prefix,
        granted.first,
        granted.last,
    ))
    .unwrap();

    // Daytime: the connection is gone. Nothing below touches the server.
    let mut expected_total = 0_i64;
    for index in 0..3_u128 {
        till.scan("8690000000001", Milli::ONE).unwrap();
        let total = till.totals().unwrap().total;
        expected_total += total.get();
        pay_cash(&mut till, 60_000);
        let sale = till
            .checkout(Ulid::from_u128(900 + index), 1_788_600_000_000)
            .unwrap();
        assert!(sale.receipt_no.is_some(), "numbers were leased in advance");
    }
    assert_eq!(till.status().unwrap().unsynced_sales, 3);
    assert_eq!(
        till.catalogue().by_id(Ulid::from_u128(1)).unwrap().on_hand,
        Milli::new(37_000),
        "stock moved locally while offline"
    );

    // Evening: the network is back.
    let pending = till.pending_sales(100).unwrap();
    assert_eq!(pending.len(), 3);
    let (status, receipt): (_, PushResponse) = call(
        &server,
        "/v1/sync/push",
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sales: pending.iter().map(envelope_for).collect(),
        },
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt.accepted.len(), 3);
    assert!(receipt.quarantined.is_empty(), "the server recomputed the same totals");

    let settled: Vec<Ulid> = receipt.settled().into_iter().map(Ulid::from_u128).collect();
    assert_eq!(till.acknowledge(&settled).unwrap(), 3);
    assert_eq!(till.status().unwrap().unsynced_sales, 0, "the outbox is empty");

    // The money agrees on both sides, which is the whole point of one crate.
    let total_pushed: i64 = pending.iter().map(|sale| sale.total_minor).sum();
    assert_eq!(total_pushed, expected_total);
}

#[tokio::test]
async fn a_retry_after_a_dropped_reply_does_not_duplicate_the_day() {
    let server = shop();
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    let (_, page): (_, PullResponse) = call(
        &server,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 100,
        },
    )
    .await;
    till.apply_pull(&deltas_from_pull(&page)).unwrap();

    till.scan("8690000000001", Milli::ONE).unwrap();
    pay_cash(&mut till, 60_000);
    till.checkout(Ulid::from_u128(900), 0).unwrap();

    let pending = till.pending_sales(100).unwrap();
    let request = PushRequest {
        protocol: PROTOCOL_VERSION,
        tenant: TENANT,
        terminal: TERMINAL,
        sales: pending.iter().map(envelope_for).collect(),
    };

    // The server stored it, then the reply was lost on a flaky connection, so
    // the till sends the same batch again.
    let (_, first): (_, PushResponse) = call(&server, "/v1/sync/push", &request).await;
    let (_, second): (_, PushResponse) = call(&server, "/v1/sync/push", &request).await;

    assert_eq!(first.accepted, second.accepted, "a replay is acknowledged identically");

    let settled: Vec<Ulid> = second.settled().into_iter().map(Ulid::from_u128).collect();
    till.acknowledge(&settled).unwrap();
    assert_eq!(till.status().unwrap().unsynced_sales, 0);
}

#[tokio::test]
async fn a_cold_start_mid_day_keeps_the_sales_and_the_numbers() {
    let server = shop();
    let backend;
    let sold_ids: Vec<Ulid>;

    {
        let (mut till, _) = Till::open(
            MemoryBackend::new(),
            TENANT,
            Ulid::from_u128(TERMINAL),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();

        let (_, page): (_, PullResponse) = call(
            &server,
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                cursor: 0,
                limit: 100,
            },
        )
        .await;
        till.apply_pull(&deltas_from_pull(&page)).unwrap();
        till.grant_lease(&Lease::new(Ulid::from_u128(TERMINAL), 1, "T7", 100, 599))
            .unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 60_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
        sold_ids = vec![sale.ticket.id];

        // The tablet dies here, with the sale unsynced.
        backend = till.journal().backend().clone();
    }

    let (mut till, boot) = Till::open(
        backend,
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    assert_eq!(boot.items, 2, "the catalogue survived");
    assert_eq!(boot.unsynced_sales, 1, "so did the sale nobody has seen yet");
    assert_eq!(boot.receipt_numbers_left, 499, "and the number it used is not reissued");

    // It still syncs, after the reboot, exactly once.
    let pending = till.pending_sales(100).unwrap();
    assert_eq!(pending[0].id, sold_ids[0]);

    let (_, receipt): (_, PushResponse) = call(
        &server,
        "/v1/sync/push",
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sales: pending.iter().map(envelope_for).collect(),
        },
    )
    .await;
    assert_eq!(receipt.accepted.len(), 1);

    let settled: Vec<Ulid> = receipt.settled().into_iter().map(Ulid::from_u128).collect();
    till.acknowledge(&settled).unwrap();
    assert_eq!(till.status().unwrap().unsynced_sales, 0);
}

#[tokio::test]
async fn a_price_change_reaches_the_till_without_repricing_an_open_basket() {
    let repo = MemoryRepo::new();
    repo.enrol(TENANT, TERMINAL);
    repo.upsert_item(TENANT, item(1, 43_000));
    let server = router(AppState::new(repo));

    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    let (_, page): (_, PullResponse) = call(
        &server,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 100,
        },
    )
    .await;
    till.apply_pull(&deltas_from_pull(&page)).unwrap();

    // A basket is open at the old price when a repricing arrives.
    till.scan("8690000000001", Milli::ONE).unwrap();
    let quoted = till.totals().unwrap().total;

    let repriced = openpos_core::storage::wire::ItemDeltasV1 {
        cursor: 99,
        upserts: vec![openpos_core::storage::wire::ItemV1 {
            price_minor: 99_000,
            ..openpos_core::storage::wire::ItemV1::from_domain(
                till.catalogue().by_id(Ulid::from_u128(1)).unwrap(),
            )
        }],
        tombstones: vec![],
    };
    till.apply_pull(&repriced).unwrap();

    assert_eq!(
        till.totals().unwrap().total,
        quoted,
        "the customer pays what the cashier quoted aloud"
    );
    assert_eq!(
        till.catalogue().by_id(Ulid::from_u128(1)).unwrap().price,
        Minor::new(99_000),
        "the next basket gets the new price"
    );
}
