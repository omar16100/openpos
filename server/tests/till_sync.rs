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
use openpos_core::sync::driver::{Driver, Next};
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_bindings::TillHandle;
use openpos_core::till::Till;
use openpos_server::http::{router, AppState};
use openpos_server::repo::{MemoryRepo, Repository};
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
        vat_on_undiscounted: false,
        barcodes: vec![format!("869000000{id:04}")],
        on_hand_milli: 40_000,
        active: true,
    }
}

/// A server with a shop, a terminal, a small catalogue, and the credential the
/// terminal was issued at enrolment.
fn shop() -> (Router, String) {
    let repo = MemoryRepo::new();
    let token = repo.enrol_with_token(TENANT, TERMINAL);
    repo.upsert_item(TENANT, item(1, 43_000));
    repo.upsert_item(TENANT, item(2, 47_500));
    (router(AppState::new(repo)), token.into_string())
}

async fn call<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    app: &Router,
    path: &str,
    body: &T,
    token: &str,
) -> (StatusCode, R) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
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
    let (server, token) = shop();
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
        &token,
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
        &token,
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
        &token,
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
    let (server, token) = shop();
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
        &token,
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
    let (_, first): (_, PushResponse) = call(&server, "/v1/sync/push", &request, &token).await;
    let (_, second): (_, PushResponse) = call(&server, "/v1/sync/push", &request, &token).await;

    assert_eq!(first.accepted, second.accepted, "a replay is acknowledged identically");

    let settled: Vec<Ulid> = second.settled().into_iter().map(Ulid::from_u128).collect();
    till.acknowledge(&settled).unwrap();
    assert_eq!(till.status().unwrap().unsynced_sales, 0);
}

#[tokio::test]
async fn a_cold_start_mid_day_keeps_the_sales_and_the_numbers() {
    let (server, token) = shop();
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
            &token,
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
        &token,
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
    repo.upsert_item(TENANT, item(1, 43_000));
    let token = repo.enrol_with_token(TENANT, TERMINAL).into_string();
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
        &token,
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

/// A day's trading drained by the driver rather than by a test calling the
/// endpoints in the order it already knows is right.
///
/// The point is that nothing here decides what to do next. The driver is asked,
/// the answer is carried out, and the loop only ends when the driver says there
/// is nothing left. A driver that stopped early, or asked for the wrong thing
/// first, fails this and would otherwise fail in a shop.
#[tokio::test]
async fn the_driver_drains_a_days_trading_without_being_told_the_order() {
    let (app, token) = shop();
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    // The shop's catalogue and a block of numbers, as enrolment would leave it.
    let pulled: PullResponse = call(
        &app,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 100,
        },
        &token,
    )
    .await
    .1;
    till.apply_pull(&deltas_from_pull(&pulled)).unwrap();
    till.grant_lease(&Lease::new(Ulid::from_u128(TERMINAL), 1, "T1", 100, 599))
        .unwrap();

    // Twelve sales rung with the internet down.
    for index in 0..12_u128 {
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 100_000);
        till.checkout(Ulid::from_u128(9_000 + index), 1_788_600_000_000).unwrap();
    }
    assert_eq!(till.status().unwrap().unsynced_sales, 12);

    let mut driver = Driver::new();
    let mut more_to_pull = pulled.more;
    let mut pushes = 0_usize;

    // A bound, not a schedule: the loop ends when the driver says to wait, and
    // this only stops a broken driver from hanging the suite. The clock advances
    // a millisecond a round, which is enough for a driver that is not backing
    // off and far too little for one that is.
    for now_ms in 0..50_u64 {
        let situation = till.situation(true, more_to_pull).unwrap();
        match driver.next(&situation, now_ms) {
            Next::Push { limit } => {
                let batch = till.pending_sales(limit).unwrap();
                let response: PushResponse = call(
                    &app,
                    "/v1/sync/push",
                    &PushRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                        sales: batch.iter().map(envelope_for).collect(),
                    },
                    &token,
                )
                .await
                .1;
                let settled: Vec<Ulid> =
                    response.settled().into_iter().map(Ulid::from_u128).collect();
                till.acknowledge(&settled).unwrap();
                driver.succeeded(now_ms);
                pushes += 1;
            }
            Next::Pull { cursor, limit } => {
                let response: PullResponse = call(
                    &app,
                    "/v1/sync/pull",
                    &PullRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                        cursor,
                        limit,
                    },
                    &token,
                )
                .await
                .1;
                more_to_pull = response.more;
                till.apply_pull(&deltas_from_pull(&response)).unwrap();
                driver.succeeded(now_ms);
            }
            Next::RenewLease { count } => {
                let response: LeaseResponse = call(
                    &app,
                    "/v1/lease",
                    &LeaseRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                        count,
                    },
                    &token,
                )
                .await
                .1;
                till.grant_lease(&Lease::new(
                    Ulid::from_u128(TERMINAL),
                    response.epoch,
                    &response.prefix,
                    response.first,
                    response.last,
                ))
                .unwrap();
                driver.succeeded(now_ms);
            }
            Next::Wait { for_ms } => {
                // Nothing outstanding. A real till sleeps here; this test is
                // finished.
                assert!(for_ms > 0);
                break;
            }
        }
    }

    assert_eq!(
        till.status().unwrap().unsynced_sales,
        0,
        "the driver has to finish the day, not most of it"
    );
    assert_eq!(pushes, 1, "twelve sales fit in one batch of twenty five");
    assert_eq!(driver.failures(), 0);
}

/// The server is unreachable, and the till keeps its sales and keeps trying.
#[tokio::test]
async fn a_failed_push_backs_off_and_loses_nothing() {
    let (app, token) = shop();
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    let pulled: PullResponse = call(
        &app,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 100,
        },
        &token,
    )
    .await
    .1;
    till.apply_pull(&deltas_from_pull(&pulled)).unwrap();
    till.grant_lease(&Lease::new(Ulid::from_u128(TERMINAL), 1, "T1", 100, 599))
        .unwrap();

    till.scan("8690000000001", Milli::ONE).unwrap();
    pay_cash(&mut till, 100_000);
    till.checkout(Ulid::from_u128(9_100), 1_788_600_000_000).unwrap();

    let mut driver = Driver::new();
    let situation = till.situation(true, false).unwrap();
    assert!(matches!(driver.next(&situation, 0), Next::Push { .. }));

    // The push does not happen: the shop's connection dropped.
    driver.failed(0);
    assert!(matches!(
        driver.next(&situation, 500),
        Next::Wait { for_ms: 500 }
    ));
    assert_eq!(
        till.status().unwrap().unsynced_sales,
        1,
        "a failed attempt must not lose the sale it was carrying"
    );

    // And once the wait is over it tries again, rather than having given up.
    assert!(matches!(driver.next(&situation, 1_000), Next::Push { .. }));

    // Now it works, and the sale lands.
    let batch = till.pending_sales(25).unwrap();
    let response: PushResponse = call(
        &app,
        "/v1/sync/push",
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sales: batch.iter().map(envelope_for).collect(),
        },
        &token,
    )
    .await
    .1;
    let settled: Vec<Ulid> = response.settled().into_iter().map(Ulid::from_u128).collect();
    till.acknowledge(&settled).unwrap();
    driver.succeeded(1_000);

    assert_eq!(till.status().unwrap().unsynced_sales, 0);
    assert_eq!(driver.failures(), 0, "one success clears the backoff");
}

/// The whole loop as a platform would drive it: JSON in, hex bodies out, and no
/// knowledge of the protocol anywhere but the core.
///
/// This is the shape a browser worker and an Android service both use. If it
/// takes more than "ask, post, hand back" here, it takes more than that there,
/// twice, in two languages.
#[tokio::test]
async fn a_platform_syncs_a_day_knowing_nothing_about_the_protocol() {
    let (app, token) = shop();
    let mut till = TillHandle::open_in_memory(
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a till opens");

    // Nothing can be rung before the catalogue arrives, so the day is rung
    // inside the loop below, once the driver has pulled it.
    let mut rounds = 0;
    let mut sold = 0_u128;

    // Ask, post, hand back. Fifty rounds is a bound against a broken driver,
    // not a schedule.
    for now_ms in 0..50_u64 {
        rounds += 1;
        let view: serde_json::Value = serde_json::from_str(
            &till.run_json(&format!(r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#)),
        )
        .expect("the till answers with a view");

        let step = view.get("step").cloned().unwrap_or(serde_json::Value::Null);
        let action = step.get("action").and_then(|a| a.as_str()).unwrap_or("wait");

        if action == "wait" {
            // Once the catalogue is in, ring the day's sales and go round again.
            if sold == 0 {
                for index in 0..12_u128 {
                    till.run_json(r#"{"op":"scan","barcode":"8690000000001","qty_milli":1000}"#);
                    till.run_json(r#"{"op":"add_cash","amount_minor":100000}"#);
                    let done = till.run_json(&format!(
                        r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1788600000000}}"#,
                        Ulid::from_u128(9_000 + index).encode()
                    ));
                    assert!(
                        done.contains("\"error\":null"),
                        "a sale must ring once the catalogue is in: {done}"
                    );
                    sold += 1;
                }
                continue;
            }
            break;
        }

        let path = step.get("path").and_then(|p| p.as_str()).expect("a path");
        let body = step.get("body").and_then(|b| b.as_str()).expect("a body");
        let kind = step.get("kind").and_then(|k| k.as_str()).expect("a kind");

        // The only thing the platform does: post the bytes it was handed.
        let reply_hex = post_hex(&app, path, body, &token).await;
        let applied = till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"{kind}","body":"{reply_hex}","now_ms":{now_ms}}}"#
        ));
        assert!(
            applied.contains("\"error\":null"),
            "applying a reply must not fail: {applied}"
        );
    }

    let view: serde_json::Value =
        serde_json::from_str(&till.run_json(r#"{"op":"view"}"#)).unwrap();
    assert_eq!(sold, 12, "the day was rung");
    assert_eq!(
        view["unsynced_sales"], 0,
        "and the driver delivered all of it in {rounds} rounds"
    );
    assert!(view["receipt_numbers_left"].as_u64().unwrap() > 0, "numbers were leased");
}

/// Hex in, hex out. The platform never sees a decoded protocol type.
async fn post_hex(app: &Router, path: &str, body_hex: &str, token: &str) -> String {
    let bytes: Vec<u8> = (0..body_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&body_hex[i..i + 2], 16).unwrap())
        .collect();

    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(bytes))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path} refused the request");
    let out = response.into_body().collect().await.unwrap().to_bytes();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Enrolment through the same two commands as everything else, and a credential
/// that outlives the process it was fetched in.
#[tokio::test]
async fn a_device_enrols_and_keeps_the_credential() {
    // A shop with a code waiting, as the demo server prints one.
    let repo = MemoryRepo::new();
    repo.enrol(TENANT, TERMINAL);
    let code = openpos_server::auth::EnrolmentCode::generate();
    repo.issue_enrolment_code(
        openpos_server::auth::Caller {
            tenant: TENANT,
            terminal: TERMINAL,
            role: openpos_server::auth::Role::Owner,
        },
        &code.hash(),
        std::time::Duration::from_secs(900),
    )
    .await
    .unwrap();
    repo.upsert_item(TENANT, item(1, 43_000));
    let app = router(AppState::new(repo));

    let backend = MemoryBackend::new();
    let (tenant_text, terminal_text) = (
        Ulid::from_u128(TENANT).encode(),
        Ulid::from_u128(TERMINAL).encode(),
    );

    let stored_backend = {
        let mut till =
            TillHandle::open_on(backend, &tenant_text, &terminal_text).expect("a till opens");

        // The device holds no credential yet, and says so.
        assert!(till.token().is_none());

        let stepped: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"enrol","code":"{}"}}"#,
            code.as_str()
        )))
        .unwrap();
        let step = &stepped["step"];
        assert_eq!(step["kind"], "enrol");
        assert!(
            step.get("token").is_none(),
            "the one request that carries no credential must not carry one"
        );

        let reply = post_hex(
            &app,
            step["path"].as_str().unwrap(),
            step["body"].as_str().unwrap(),
            "",
        )
        .await;
        let applied: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"enrol","body":"{reply}","now_ms":0}}"#
        )))
        .unwrap();

        assert_eq!(applied["error"], serde_json::Value::Null);
        // The code decides which shop and terminal this device is, not the
        // device: this is the first moment a till learns its own identity.
        assert_eq!(applied["applied"]["enrolled"]["tenant"], tenant_text);
        assert_eq!(applied["applied"]["enrolled"]["terminal"], terminal_text);
        assert!(till.token().is_some());

        till.backend().expect("a memory till").clone()
    };

    // A new process, reading only what is on disk. A credential the platform
    // had to keep somewhere of its own would be gone here.
    let till = TillHandle::open_on(stored_backend, &tenant_text, &terminal_text)
        .expect("the till reopens");
    assert!(
        till.token().is_some(),
        "the credential belongs with the ledger and has to survive with it"
    );

    // And every request it builds now carries that credential, rather than the
    // platform being trusted to remember one.
    let mut till = till;
    let stepped: serde_json::Value =
        serde_json::from_str(&till.run_json(r#"{"op":"sync_step","online":true,"now_ms":0}"#))
            .unwrap();
    assert!(
        stepped["step"]["token"].is_string(),
        "a step must carry the credential: {stepped}"
    );
}
