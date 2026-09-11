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

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use openpos_bindings::TillHandle;
use openpos_core::auth::{PinHash, SALT_LEN};
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::domain::Discount;
use openpos_core::ids::Ulid;
use openpos_core::lease::Lease;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::protocol::{
    AccountRequest, AccountResponse, AllowedWire, CatalogueEditResponse, ClosedShiftWire,
    CustomersRequest, CustomersResponse, ItemWire, LeaseRequest, LeaseResponse, OperatorWire,
    OperatorsRequest, OperatorsResponse, OwedRequest, OwedResponse, PROTOCOL_VERSION, PullRequest,
    PullResponse, PushAllowedRequest, PushAllowedResponse, PushRequest, PushResponse,
    PushShiftsRequest, PushShiftsResponse, PutOperatorRequest, PutShopRequest, ShopRequest,
    ShopResponse, TakePaymentRequest, TakePaymentResponse, UpsertItemRequest,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::driver::{Driver, Next};
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_core::till::Till;
use openpos_server::http::{AppState, router};
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
        from_a_till: false,
        supply: 0,
        category: String::new(),
    }
}

/// A server with a shop, a terminal, a small catalogue, and the credential the
/// terminal was issued at enrolment.
fn shop() -> (Router, String) {
    let repo = MemoryRepo::new();
    let token = repo.enrol_with_token(TENANT, TERMINAL);
    // A shop with a name, because a shop without one cannot head a receipt and
    // a till now refuses to pretend otherwise.
    repo.put_shop_details_for_test(
        TENANT,
        "Karim General Store",
        Some("001234567-0101"),
        Some("12 Mirpur Road, Dhaka"),
    );
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
    till.add_tender(
        Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(amount),
            reference: None,
        },
        0,
    )
    .unwrap();
}

#[tokio::test]
async fn a_till_replaces_its_credential_before_the_shop_stops_accepting_it() {
    let (server, first) = shop();
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();
    // Enrolled a month ago, as far as this device knows.
    let enrolled_at = 1_788_600_000_000_u64;
    till.take_credential(&first, enrolled_at, 0).unwrap();

    // A month later the driver says to renew. Nothing else about the till has
    // changed: this is the step that had no client at all, so every device
    // would have stopped a year after it was enrolled.
    let driver = Driver::default();
    let now = enrolled_at + openpos_core::sync::driver::RENEW_CREDENTIAL_MS;
    let situation = till.situation(true, false).unwrap();
    assert_eq!(driver.next(&situation, now), Next::RenewCredential);

    let (status, renewed): (_, openpos_core::protocol::RenewResponse) = call(
        &server,
        "/v1/renew",
        &openpos_core::protocol::RenewRequest {
            protocol: PROTOCOL_VERSION,
        },
        &first,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(renewed.token, first, "a fresh one, not the same one back");
    assert!(
        renewed.expires_in_seconds > 0,
        "and it says how long it lasts"
    );
    assert!(
        renewed.previous_valid_for_seconds > 0,
        "the old one keeps working, or a lost reply strands the device"
    );

    till.take_credential(&renewed.token, now, renewed.expires_in_seconds * 1_000)
        .unwrap();

    // The new credential works.
    let (status, _): (_, PullResponse) = call(
        &server,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        },
        &renewed.token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // And so does the old one, for now. A device whose reply was lost still has
    // the credential it went in with, and a shop that cut it off the instant a
    // new one was issued would have a till that cannot ask for anything.
    let (status, _): (_, PullResponse) = call(
        &server,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        },
        &first,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the overlap is the whole point");

    // Having renewed, the till is not due again immediately, and it knows the
    // shop's own policy rather than a number compiled into this build.
    let situation = till.situation(true, false).unwrap();
    assert_ne!(driver.next(&situation, now + 1_000), Next::RenewCredential);
    assert_eq!(
        till.credential_age().map(|(_, lifetime)| lifetime),
        Some(renewed.expires_in_seconds * 1_000)
    );
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
    assert!(
        receipt.quarantined.is_empty(),
        "the server recomputed the same totals"
    );

    let settled: Vec<Ulid> = receipt.settled().into_iter().map(Ulid::from_u128).collect();
    assert_eq!(till.acknowledge(&settled).unwrap(), 3);
    assert_eq!(
        till.status().unwrap().unsynced_sales,
        0,
        "the outbox is empty"
    );

    // The money agrees on both sides, which is the whole point of one crate.
    let total_pushed: i64 = pending.iter().map(|sale| sale.total_minor).sum();
    assert_eq!(total_pushed, expected_total);
}

#[tokio::test]
async fn a_sale_on_account_becomes_a_debt_the_owner_can_settle() {
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

    // Karim takes rice on Tuesday and pays half of it in cash. This is the
    // ordinary shape of it: part now, the rest on Friday.
    till.scan("8690000000001", Milli::ONE).unwrap();
    let total = till.totals().unwrap().total.get();
    pay_cash(&mut till, 20_000);
    till.add_tender(
        Tender {
            kind: TenderKind::Credit,
            amount: Minor::new(total - 20_000),
            reference: Some("Karim, flat 3".into()),
        },
        0,
    )
    .unwrap();
    till.checkout(Ulid::from_u128(900), 1_788_600_000_000)
        .unwrap();

    let pending = till.pending_sales(10).unwrap();
    let (status, _): (_, PushResponse) = call(
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

    // The owner opens the back office and finds what the notebook used to say.
    let (status, book): (_, OwedResponse) = call(
        &server,
        "/v1/back-office/owed",
        &OwedRequest {
            protocol: PROTOCOL_VERSION,
            limit: 50,
            after_owed_minor: 0,
            after_person_key: String::new(),
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(book.owing.len(), 1);
    assert_eq!(book.owing[0].person_name, "Karim, flat 3");
    assert_eq!(
        book.owing[0].owed_minor,
        total - 20_000,
        "only the part on account, not the whole ticket"
    );

    // Friday: he pays two hundred of it.
    let (status, taken): (_, TakePaymentResponse) = call(
        &server,
        "/v1/back-office/owed/payment",
        &TakePaymentRequest {
            protocol: PROTOCOL_VERSION,
            id: 5_000,
            person_key: book.owing[0].person_key.clone(),
            person_name: book.owing[0].person_name.clone(),
            amount_minor: 20_000,
            at_ms: 1_788_900_000_000,
            note: Some("in cash".to_owned()),
            written_off: false,
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(taken.taken);
    assert_eq!(taken.owed_minor, total - 40_000);

    // The reply was dropped and the owner presses it again. A payment counted
    // twice is money the shop believes it has been given and has not.
    let (status, again): (_, TakePaymentResponse) = call(
        &server,
        "/v1/back-office/owed/payment",
        &TakePaymentRequest {
            protocol: PROTOCOL_VERSION,
            id: 5_000,
            person_key: book.owing[0].person_key.clone(),
            person_name: book.owing[0].person_name.clone(),
            amount_minor: 20_000,
            at_ms: 1_788_900_000_000,
            note: Some("in cash".to_owned()),
            written_off: false,
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!again.taken, "already recorded, and said so");
    assert_eq!(again.owed_minor, total - 40_000);

    // What it is made of, which is what gets read out when somebody argues.
    let (status, account): (_, AccountResponse) = call(
        &server,
        "/v1/back-office/owed/account",
        &AccountRequest {
            protocol: PROTOCOL_VERSION,
            person_key: book.owing[0].person_key.clone(),
            limit: 50,
            after_at_ms: 0,
            after_source_id: 0,
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(account.entries.len(), 2);
    assert!(!account.entries[0].is_sale, "the payment is the newest");
    assert_eq!(account.entries[0].amount_minor, -20_000);
    assert!(account.entries[1].is_sale);

    // He settles the rest, and leaves the list. The entries stay in the book.
    let (_, cleared): (_, TakePaymentResponse) = call(
        &server,
        "/v1/back-office/owed/payment",
        &TakePaymentRequest {
            protocol: PROTOCOL_VERSION,
            id: 5_001,
            person_key: book.owing[0].person_key.clone(),
            person_name: book.owing[0].person_name.clone(),
            amount_minor: total - 40_000,
            at_ms: 1_789_000_000_000,
            note: None,
            written_off: false,
        },
        &token,
    )
    .await;
    assert_eq!(cleared.owed_minor, 0);

    let (_, book): (_, OwedResponse) = call(
        &server,
        "/v1/back-office/owed",
        &OwedRequest {
            protocol: PROTOCOL_VERSION,
            limit: 50,
            after_owed_minor: 0,
            after_person_key: String::new(),
        },
        &token,
    )
    .await;
    assert!(
        book.owing.is_empty(),
        "a settled account is not a debt, and an owner reads a list of debts"
    );
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
    // A real clock, because the server now holds a sale whose timestamp cannot
    // be true, and a till that rang one at the epoch is a device whose clock was
    // never set.
    till.checkout(Ulid::from_u128(900), 1_788_600_000_000)
        .unwrap();

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

    assert_eq!(
        first.accepted, second.accepted,
        "a replay is acknowledged identically"
    );

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
        let sale = till
            .checkout(Ulid::from_u128(900), 1_788_600_000_000)
            .unwrap();
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
    assert_eq!(
        boot.unsynced_sales, 1,
        "so did the sale nobody has seen yet"
    );
    assert_eq!(
        boot.receipt_numbers_left, 499,
        "and the number it used is not reissued"
    );

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

/// A shop's afternoon: the drawer is open, the network comes back, the server
/// takes every sale, and then the tablet restarts.
///
/// The till empties its log once the server holds everything in it, and the open
/// drawer is rebuilt by replaying that same log. So the float the owner counted
/// in that morning, the change fetched from the safe, and the day's takings all
/// went, and the cashier met it at the evening count: a drawer that began at
/// nothing, holding a day's cash.
#[tokio::test]
async fn a_drawer_open_when_the_server_takes_the_day_is_still_open_after_a_restart() {
    let (server, token) = shop();
    let backend;

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

        // Somebody has to be standing at it: a cash movement is signed for.
        till.set_operators(vec![openpos_core::auth::Operator {
            id: Ulid::from_u128(1),
            name: "Karim".into(),
            pin: PinHash::derive("0000", [1; SALT_LEN], 1_000),
            permissions: openpos_core::auth::Permissions::supervisor(),
            active: true,
        }])
        .unwrap();
        till.sign_in(Ulid::from_u128(1), "0000", 1_788_600_000_000)
            .unwrap();

        // Morning: two thousand taka counted into the drawer, and five hundred
        // more fetched from the safe when the small notes ran low.
        till.open_shift(Ulid::from_u128(80), Minor::new(200_000), 1_788_600_000_000)
            .unwrap();
        till.cash_in(
            Minor::new(50_000),
            "change from the safe",
            1_788_601_000_000,
        )
        .unwrap();

        for index in 0..3_u128 {
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 60_000);
            till.checkout(Ulid::from_u128(900 + index), 1_788_602_000_000)
                .unwrap();
        }

        // Afternoon: the connection is back and the shop takes the lot.
        let pending = till.pending_sales(100).unwrap();
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
        assert_eq!(receipt.accepted.len(), 3);
        let settled: Vec<Ulid> = receipt.settled().into_iter().map(Ulid::from_u128).collect();
        assert_eq!(till.acknowledge(&settled).unwrap(), 3);
        assert_eq!(till.status().unwrap().unsynced_sales, 0);

        // And the tablet is unplugged.
        backend = till.journal().backend().clone();
    }

    let (till, _) = Till::open(
        backend,
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    let report = till.x_report().expect("the drawer is still open");
    assert_eq!(report.opening_float, Minor::new(200_000), "the float");
    assert_eq!(report.cash_in, Minor::new(50_000), "the safe");
    assert_eq!(report.sales, 3, "the day's sales");
    assert_eq!(
        report.expected_cash,
        Minor::new(398_350),
        "2000 float, 500 in, three baskets of 494.50"
    );
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
        till.checkout(Ulid::from_u128(9_000 + index), 1_788_600_000_000)
            .unwrap();
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
            // A drawer standing open is a position, not a record, and this
            // test is about the records. It is answered so the loop moves on.
            Next::ReportDrawer => driver.reported_drawer(now_ms),
            // Not this shop: it does nothing about the shelf, so the driver
            // should never ask. Answered rather than ignored so a driver that
            // starts asking fails here instead of looping.
            Next::FetchStock { .. } => panic!("a shop with no stock rule was asked about stock"),
            // Nothing was written down at this till, so nothing is owed.
            Next::PushItems => panic!("a till that wrote nothing down was asked to send items"),
            Next::PushCustomers => panic!("a till that wrote nobody down was asked to send people"),
            Next::PushShifts => {
                // A drawer somebody counted. It goes ahead of the catalogue for
                // the same reason sales do: it exists nowhere else.
                let (_, response): (_, PushShiftsResponse) = call(
                    &app,
                    "/v1/sync/shifts",
                    &PushShiftsRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                        shifts: till
                            .unsent_shifts()
                            .iter()
                            .map(|shift| ClosedShiftWire {
                                id: shift.id,
                                terminal: TERMINAL,
                                closed_by: shift.closed_by,
                                closed_by_name: shift.closed_by_name.clone(),
                                opened_at_ms: shift.opened_at_ms,
                                closed_at_ms: shift.closed_at_ms,
                                opening_float_minor: shift.opening_float_minor,
                                sales: shift.sales,
                                cash_sales_minor: shift.cash_sales_minor,
                                non_cash_sales_minor: shift.non_cash_sales_minor,
                                cash_in_minor: shift.cash_in_minor,
                                cash_out_minor: shift.cash_out_minor,
                                expected_cash_minor: shift.expected_cash_minor,
                                counted_cash_minor: shift.counted_cash_minor,
                                variance_minor: shift.variance_minor,
                                expected_from_sales_minor: None,
                                struck_out_cash_minor: None,
                            })
                            .collect(),
                    },
                    &token,
                )
                .await;
                till.shifts_accepted(&response.accepted).unwrap();
            }
            Next::PushAllowed => {
                // Who allowed what. Beside the counted drawer and ahead of the
                // catalogue for the same reason: it exists nowhere else until
                // the shop has it.
                let (_, response): (_, PushAllowedResponse) = call(
                    &app,
                    "/v1/sync/allowed",
                    &PushAllowedRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                        allowed: till
                            .unsent_allowed()
                            .iter()
                            .map(|one| AllowedWire {
                                seq: one.seq,
                                at_ms: one.at_ms,
                                action: one.action,
                                bp: one.bp,
                                operator: one.operator,
                                operator_name: one.operator_name.clone(),
                                authorised_by: one.authorised_by,
                                authorised_by_name: one.authorised_by_name.clone(),
                                receipt_no: one.receipt_no.clone(),
                            })
                            .collect(),
                    },
                    &token,
                )
                .await;
                till.allowed_accepted(&response.stored).unwrap();
            }
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
                let settled: Vec<Ulid> = response
                    .settled()
                    .into_iter()
                    .map(Ulid::from_u128)
                    .collect();
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
            // A credential a month old, which this loop's clock never reaches.
            // Answered so the loop moves on rather than panicking, and so a
            // day's trading is not spent renewing.
            Next::RenewCredential => {
                let response: openpos_core::protocol::RenewResponse = call(
                    &app,
                    "/v1/renew",
                    &openpos_core::protocol::RenewRequest {
                        protocol: PROTOCOL_VERSION,
                    },
                    &token,
                )
                .await
                .1;
                till.take_credential(&response.token, now_ms, 0).unwrap();
            }
            // One number, asked often, which is what keeps the three lists
            // rare. A shop that has changed nothing answers the same number and
            // the till asks for nothing else.
            Next::CheckSettings => {
                let response: openpos_core::protocol::SettingsResponse = call(
                    &app,
                    "/v1/settings",
                    &openpos_core::protocol::SettingsRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                    },
                    &token,
                )
                .await
                .1;
                driver.settings_seq(response.seq, now_ms);
            }
            // Nobody is written down in this shop, so the driver asks for no
            // balances. Answered anyway rather than left to panic, because a
            // step this loop cannot handle is a test that hangs.
            Next::FetchBalances => driver.fetched_balances(now_ms),
            Next::FetchCustomers => {
                // Nobody buys on account in this shop yet, and the till still
                // counts it as asked: otherwise it asks forever.
                let response: openpos_core::protocol::CustomersResponse = call(
                    &app,
                    "/v1/customers",
                    &openpos_core::protocol::CustomersRequest {
                        protocol: PROTOCOL_VERSION,
                        tenant: TENANT,
                        terminal: TERMINAL,
                    },
                    &token,
                )
                .await
                .1;
                assert!(response.customers.is_empty());
                driver.fetched_customers(now_ms);
            }
            Next::FetchOperators => {
                // Nobody has been added to this shop, so the reply is empty and
                // the till still counts it as asked: otherwise it asks forever.
                let response: OperatorsResponse = call(
                    &app,
                    "/v1/operators",
                    &OperatorsRequest {
                        protocol: PROTOCOL_VERSION,
                    },
                    &token,
                )
                .await
                .1;
                assert!(response.operators.is_empty());
                till.set_operators(vec![openpos_core::auth::Operator {
                    id: Ulid::from_u128(1),
                    name: "Owner".into(),
                    pin: PinHash::derive("0000", [1; SALT_LEN], 1_000),
                    permissions: openpos_core::auth::Permissions::supervisor(),
                    active: true,
                }])
                .unwrap();
                driver.succeeded(now_ms);
            }
            Next::FetchShop => {
                let response: ShopResponse = call(
                    &app,
                    "/v1/shop",
                    &ShopRequest {
                        protocol: PROTOCOL_VERSION,
                    },
                    &token,
                )
                .await
                .1;
                till.set_shop(
                    openpos_core::receipt::Shop {
                        name: response.name,
                        bin: response.bin,
                        address: response.address,
                        phone: response.phone,
                    },
                    response.wallets.into_iter().map(Into::into).collect(),
                    openpos_core::domain::StockRule::from_u8(response.stock_rule),
                )
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
    till.checkout(Ulid::from_u128(9_100), 1_788_600_000_000)
        .unwrap();

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
    let settled: Vec<Ulid> = response
        .settled()
        .into_iter()
        .map(Ulid::from_u128)
        .collect();
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
    // A shop that wants its tills told about the shelf, so the loop below has to
    // carry that exchange too without knowing what it is.
    let _: ShopResponse = call(
        &app,
        "/v1/back-office/shop",
        &PutShopRequest {
            protocol: PROTOCOL_VERSION,
            name: "Karim General Store".to_owned(),
            bin: None,
            address: None,
            phone: None,
            wallets: vec![],
            stock_rule: 1,
        },
        &token,
    )
    .await
    .1;
    let mut till = TillHandle::open_in_memory(
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a till opens");

    // Nothing can be rung before the catalogue arrives, so the day is rung
    // inside the loop below, once the driver has pulled it.
    let mut rounds = 0;
    let mut sold = 0_u128;
    let mut saw_stock = false;

    // Ask, post, hand back. Fifty rounds is a bound against a broken driver,
    // not a schedule.
    for now_ms in 0..50_u64 {
        rounds += 1;
        let view: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#
        )))
        .expect("the till answers with a view");

        let step = view.get("step").cloned().unwrap_or(serde_json::Value::Null);
        let action = step
            .get("action")
            .and_then(|a| a.as_str())
            .unwrap_or("wait");

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
        if kind == "stock" {
            saw_stock = true;
        }

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

    let view: serde_json::Value = serde_json::from_str(&till.run_json(r#"{"op":"view"}"#)).unwrap();
    assert_eq!(sold, 12, "the day was rung");
    assert_eq!(
        view["unsynced_sales"], 0,
        "and the driver delivered all of it in {rounds} rounds"
    );
    assert!(
        view["receipt_numbers_left"].as_u64().unwrap() > 0,
        "numbers were leased"
    );
    assert!(
        saw_stock,
        "a shop that watches the shelf has its till ask what is on it"
    );
    // And the answer landed. This shop has had no delivery, so its shelves hold
    // nothing whatever the catalogue records say, and the next thing rung is
    // beyond them: named by line, so a screen can put it under the line it is
    // about rather than as a banner about the basket.
    let rung: serde_json::Value = serde_json::from_str(
        &till.run_json(r#"{"op":"scan","barcode":"8690000000001","qty_milli":1000}"#),
    )
    .unwrap();
    let short = rung["beyond_the_shelf"]
        .as_array()
        .expect("the view says what the shelf disagrees about");
    assert_eq!(short.len(), 1, "one line, not a banner: {short:?}");
    assert_eq!(short[0]["line"], 0);
    assert_eq!(
        short[0]["on_hand_milli"], -12_000,
        "no delivery has ever arrived and this till sold twelve out of it"
    );
    assert_eq!(short[0]["wanted_milli"], 1_000);
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
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{path} refused the request"
    );
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

        let stepped: serde_json::Value = serde_json::from_str(
            &till.run_json(&format!(r#"{{"op":"enrol","code":"{}"}}"#, code.as_str())),
        )
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

/// The tax base set in the back office reaches a till and changes what a
/// customer pays.
///
/// A flag nothing can set is not a feature, and a flag that stops somewhere in
/// the middle is worse than one that does not exist: it looks set.
#[tokio::test]
async fn a_listed_price_item_set_in_the_back_office_prices_that_way_at_the_till() {
    let (app, token) = shop();

    // The owner marks an item as taxed on its listed price.
    let mut listed = item(9, 10_000);
    listed.vat_on_undiscounted = true;
    listed.barcodes = vec!["8690000000009".to_owned()];
    let _: CatalogueEditResponse = call(
        &app,
        "/v1/back-office/catalogue/upsert",
        &UpsertItemRequest {
            expected_seq: 0,
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            item: listed,
        },
        &token,
    )
    .await
    .1;

    // A till pulls it, exactly as it pulls anything else.
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

    // 100.00 listed, ten percent off the line, five percent off the ticket.
    till.scan("8690000000009", Milli::ONE).unwrap();
    till.set_line_discount(0, Discount::Rate(Bp::new(1_000).unwrap()))
        .unwrap();
    till.set_ticket_discount(Discount::Rate(Bp::new(500).unwrap()))
        .unwrap();

    let totals = till.totals().unwrap();
    assert_eq!(
        totals.net_total,
        Minor::new(8_550),
        "the goods, discounted twice"
    );
    assert_eq!(
        totals.vat_total,
        Minor::new(1_500),
        "and the tax fixed to the listed price, all the way from the back office"
    );
    assert_eq!(totals.total, Minor::new(10_050));
}

/// A shop says what to do about the shelf, and a till three miles away does it.
///
/// The whole of the setting: an owner picks it in the back office, it reaches a
/// device through the ordinary shop fetch, and the device enforces it with the
/// internet down, which is where it will be enforced.
#[tokio::test]
async fn a_shop_that_says_refuse_has_its_till_refuse() {
    let (app, token) = shop();

    let _: ShopResponse = call(
        &app,
        "/v1/back-office/shop",
        &PutShopRequest {
            protocol: PROTOCOL_VERSION,
            name: "Karim General Store".to_owned(),
            bin: None,
            address: None,
            phone: None,
            wallets: vec![],
            // Refuse it and let a supervisor allow it.
            stock_rule: 2,
        },
        &token,
    )
    .await
    .1;

    let (_, shop_now): (_, ShopResponse) = call(
        &app,
        "/v1/shop",
        &ShopRequest {
            protocol: PROTOCOL_VERSION,
        },
        &token,
    )
    .await;
    assert_eq!(shop_now.stock_rule, 2, "the till is told what to do");

    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();
    let (_, page): (_, PullResponse) = call(
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
    .await;
    till.apply_pull(&deltas_from_pull(&page)).unwrap();
    till.set_shop(
        openpos_core::receipt::Shop {
            name: shop_now.name,
            bin: shop_now.bin,
            address: shop_now.address,
            phone: shop_now.phone,
        },
        shop_now.wallets.into_iter().map(Into::into).collect(),
        openpos_core::domain::StockRule::from_u8(shop_now.stock_rule),
    )
    .unwrap();

    // What the shop actually holds, which is not what the catalogue record says:
    // that number is whatever somebody last typed on the item and never moves.
    // A till that has been told to refuse has to ask.
    let (_, shelves): (_, openpos_core::protocol::OnHandResponse) = call(
        &app,
        "/v1/stock",
        &openpos_core::protocol::OnHandRequest {
            protocol: PROTOCOL_VERSION,
            item_ids: till
                .item_window(0, 200)
                .into_iter()
                .map(|id| id.to_u128())
                .collect(),
        },
        &token,
    )
    .await;
    assert_eq!(shelves.on_hand.len(), 2, "a figure for each item it holds");
    let taken = till.apply_on_hand(
        &shelves
            .on_hand
            .iter()
            .map(|entry| (Ulid::from_u128(entry.item_id), Milli::new(entry.qty_milli)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(taken, 2);
    // Two items answered out of a window of two hundred, so that window was the
    // whole shelf and the lap is closed. The driver says this in the app; here
    // the test is the driver. Until it is said, the till holds a figure for
    // some items and nothing for the rest, and it says nothing about the shelf
    // at all rather than refusing on an absence.
    till.shelf_swept();

    // This shop has never had a delivery, so its shelves hold nothing and the
    // first scan is refused. Which is the rule doing exactly what the shop
    // asked, and why the rule is off until a shop turns it on.
    let refusal = till.scan("8690000000001", Milli::ONE).unwrap_err();
    assert!(
        format!("{refusal}").contains("the shop has 0"),
        "refused with {refusal}"
    );

    // Goods arrive. The till asks again and sells what came in.
    let _: openpos_core::protocol::ReceiveGoodsResponse = call(
        &app,
        "/v1/back-office/stock/receive",
        &openpos_core::protocol::ReceiveGoodsRequest {
            protocol: PROTOCOL_VERSION,
            id: Ulid::from_u128(600).to_u128(),
            supplier_id: None,
            reference: Some("a delivery".to_owned()),
            received_at_ms: 1_788_600_000_000,
            note: None,
            lines: vec![openpos_core::protocol::ReceiptLineWire {
                item_id: 1,
                qty_milli: 3_000,
                unit_cost_minor: 38_000,
            }],
        },
        &token,
    )
    .await
    .1;
    let (_, shelves): (_, openpos_core::protocol::OnHandResponse) = call(
        &app,
        "/v1/stock",
        &openpos_core::protocol::OnHandRequest {
            protocol: PROTOCOL_VERSION,
            item_ids: vec![1],
        },
        &token,
    )
    .await;
    till.apply_on_hand(
        &shelves
            .on_hand
            .iter()
            .map(|entry| (Ulid::from_u128(entry.item_id), Milli::new(entry.qty_milli)))
            .collect::<Vec<_>>(),
    );

    till.scan("8690000000001", Milli::new(3_000))
        .expect("three arrived and three may be sold");
    let refusal = till.scan("8690000000001", Milli::ONE).unwrap_err();
    assert!(
        format!("{refusal}").contains("the shop has 3"),
        "refused with {refusal}"
    );
}

/// The back office reads the shop back before it offers to change it.
///
/// The form that sets the shop's name, its BIN, the wallets it takes and what a
/// till does about the shelf used to open empty every time, so an owner who set
/// a rule and came back tomorrow could not tell what the shop was doing without
/// overwriting it. Read from the route a till reads, so what the screen shows
/// and what a till obeys are one answer.
#[tokio::test]
async fn the_back_office_reads_the_shop_back_before_it_offers_to_change_it() {
    let (app, token) = shop();
    let _: ShopResponse = call(
        &app,
        "/v1/back-office/shop",
        &PutShopRequest {
            protocol: PROTOCOL_VERSION,
            name: "Karim General Store".to_owned(),
            bin: Some("001234567-0101".to_owned()),
            address: Some("12 Mirpur Road, Dhaka".to_owned()),
            phone: None,
            // Two spellings of one wallet, which the server tidies. The screen
            // has to show what the shop holds, not what somebody typed.
            wallets: vec!["bKash".to_owned(), " bKash ".to_owned(), "Nagad".to_owned()],
            stock_rule: 2,
        },
        &token,
    )
    .await
    .1;

    let mut office = TillHandle::open_on(
        MemoryBackend::new(),
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a back office opens");
    office.set_token_for_test(&token);

    let stepped: serde_json::Value =
        serde_json::from_str(&office.run_json(r#"{"op":"admin","request":{"what":"shop_now"}}"#))
            .unwrap();
    let step = &stepped["step"];
    assert_eq!(step["kind"], "admin_shop_now");
    let reply = post_hex(
        &app,
        step["path"].as_str().unwrap(),
        step["body"].as_str().unwrap(),
        &token,
    )
    .await;
    let applied: serde_json::Value = serde_json::from_str(&office.run_json(&format!(
        r#"{{"op":"sync_apply","kind":"admin_shop_now","body":"{reply}","now_ms":0}}"#
    )))
    .unwrap();

    let shop = &applied["applied"]["shop"];
    assert_eq!(shop["name"], "Karim General Store");
    assert_eq!(shop["bin"], "001234567-0101");
    assert_eq!(shop["address"], "12 Mirpur Road, Dhaka");
    assert!(shop["phone"].is_null(), "a shop with no phone shows none");
    assert_eq!(
        shop["wallets"].as_array().expect("the wallets").len(),
        2,
        "as the shop holds them, tidied: {shop:?}"
    );
    assert_eq!(shop["stock_rule"], 2, "and what it does about the shelf");
}

/// A delivery arrives during an outage with a barcode in nobody's catalogue.
///
/// The cold-start promise turns on this: a till that can only say "no such
/// item" loses the sale, and the shop sells it off the paper and reconciles
/// nothing. So the cashier writes it down, sells it, and the shop gets both.
#[tokio::test]
async fn something_the_shop_never_heard_of_is_sold_and_then_reaches_the_shop() {
    let (app, token) = shop();
    let mut till = TillHandle::open_on(
        MemoryBackend::new(),
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a till opens");
    till.set_token_for_test(&token);

    // Nobody has heard of it.
    let refused = till.run_json(r#"{"op":"scan","barcode":"8690000009999","qty_milli":1000}"#);
    assert!(
        refused.contains("no item in the catalogue has that barcode"),
        "{refused}"
    );

    // The cashier says what it is. A hundred and twenty taka, the ordinary rate.
    let made = Ulid::from_u128(4_242);
    let written = till.run_json(&format!(
        r#"{{"op":"quick_add","id":"{}","barcode":"8690000009999",
             "name":"Biscuits, the new ones","price_minor":12000,"vat_bp":1500}}"#,
        made.encode()
    ));
    assert!(written.contains("\"error\":null"), "{written}");

    let rung = till.run_json(r#"{"op":"scan","barcode":"8690000009999","qty_milli":2000}"#);
    let view: serde_json::Value = serde_json::from_str(&rung).unwrap();
    assert_eq!(view["net_minor"], 24_000, "two at a hundred and twenty");
    assert_eq!(view["vat_minor"], 3_600);
    till.run_json(r#"{"op":"add_cash","amount_minor":30000}"#);
    let sold = till.run_json(&format!(
        r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1788600000000}}"#,
        Ulid::from_u128(900).encode()
    ));
    assert!(sold.contains("\"error\":null"), "{sold}");

    // The line comes back. The item goes to the shop ahead of the catalogue
    // pull, because the sales already sent name it.
    let mut sent_items = false;
    for now_ms in 0..30_u64 {
        let stepped: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#
        )))
        .unwrap();
        let step = &stepped["step"];
        if step["action"] == "wait" {
            break;
        }
        let kind = step["kind"].as_str().unwrap();
        let reply = post_hex(
            &app,
            step["path"].as_str().unwrap(),
            step["body"].as_str().unwrap(),
            &token,
        )
        .await;
        let applied = till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"{kind}","body":"{reply}","now_ms":{now_ms}}}"#
        ));
        assert!(applied.contains("\"error\":null"), "{kind}: {applied}");
        if kind == "items" {
            sent_items = true;
            assert!(
                applied.contains("\"items_taken\":1"),
                "the shop said it has it: {applied}"
            );
        }
    }
    assert!(sent_items, "a till that wrote an item down has to send it");

    // The shop holds it, marked as a till's work: a price typed to get a queue
    // moving is not a price the owner agreed to.
    let (_, page): (_, PullResponse) = call(
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
    .await;
    let written = page
        .upserts
        .iter()
        .find(|item| item.id == made.to_u128())
        .expect("the shop has the item the till wrote down");
    assert_eq!(written.name_en, "Biscuits, the new ones");
    assert_eq!(written.price_minor, 12_000);
    assert_eq!(written.vat_bp, 1_500);
    assert_eq!(written.barcodes, ["8690000009999"]);
    assert!(written.from_a_till, "and knows where it came from");

    // And the till has stopped owing it.
    let view: serde_json::Value = serde_json::from_str(&till.run_json(r#"{"op":"view"}"#)).unwrap();
    assert_eq!(view["unsynced_sales"], 0);
}

/// Somebody buys on account who is in nobody's list.
///
/// The sale used to be written against whatever name was typed and added up
/// under that spelling, which is how the second Karim pays for the first one's
/// rice. Writing them down at the till gives the debt a person, and the shop
/// comes to hold them.
#[tokio::test]
async fn somebody_written_down_at_the_till_reaches_the_shop_and_the_paper() {
    let (app, token) = shop();
    let mut till = TillHandle::open_on(
        MemoryBackend::new(),
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a till opens");
    till.set_token_for_test(&token);
    till.run_json(
        r#"{"op":"apply_items","items":[{"id":"00000000000000000000000001","code":"RICE5",
           "name":"Rice Miniket 5kg","price_minor":43000,"vat_bp":1500,"price_inclusive":false,
           "barcodes":["8690000000001"],"on_hand_milli":40000}]}"#,
    );

    // A shop with a name, or the receipt below cannot print at all.
    let (_, details): (_, ShopResponse) = call(
        &app,
        "/v1/shop",
        &ShopRequest {
            protocol: PROTOCOL_VERSION,
        },
        &token,
    )
    .await;
    assert!(!details.name.is_empty());

    let buyer = Ulid::from_u128(21);
    let written = till.run_json(&format!(
        r#"{{"op":"write_customer","id":"{}","name":"Karim, flat 3",
             "phone":"01711000000","bin":"009876543-0202"}}"#,
        buyer.encode()
    ));
    assert!(written.contains("\"error\":null"), "{written}");

    // Everything the till needs before it can print: its shop's name, and the
    // person it just wrote down on their way to the shop.
    let mut sent_people = false;
    for now_ms in 0..30_u64 {
        let stepped: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#
        )))
        .unwrap();
        let step = &stepped["step"];
        if step["action"] == "wait" {
            break;
        }
        let kind = step["kind"].as_str().unwrap();
        let reply = post_hex(
            &app,
            step["path"].as_str().unwrap(),
            step["body"].as_str().unwrap(),
            &token,
        )
        .await;
        let applied = till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"{kind}","body":"{reply}","now_ms":{now_ms}}}"#
        ));
        assert!(applied.contains("\"error\":null"), "{kind}: {applied}");
        if kind == "people" {
            sent_people = true;
            assert!(applied.contains("\"people_taken\":1"), "{applied}");
        }
    }
    assert!(
        sent_people,
        "a till that wrote somebody down has to send them"
    );

    // The sale is theirs, and goes on their account rather than against a
    // spelling.
    let pointed = till.run_json(&format!(
        r#"{{"op":"set_customer","customer":"{}"}}"#,
        buyer.encode()
    ));
    assert!(pointed.contains("\"error\":null"), "{pointed}");
    till.run_json(r#"{"op":"scan","barcode":"8690000000001","qty_milli":1000}"#);
    let taken = till.run_json(
        r#"{"op":"add_tender","kind":"credit","amount_minor":49450,"reference":"Karim, flat 3"}"#,
    );
    assert!(taken.contains("\"error\":null"), "{taken}");
    let sold = till.run_json(&format!(
        r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1788600000000}}"#,
        Ulid::from_u128(900).encode()
    ));
    assert!(sold.contains("\"error\":null"), "{sold}");

    // The paper names them and their BIN, which is what a tax invoice to
    // another business has to carry.
    let asked = till.run_json(r#"{"op":"receipt","width":32,"rung_at":"07 Sep 2026 19:00"}"#);
    let printed: serde_json::Value = serde_json::from_str(&asked).unwrap();
    assert!(
        printed["receipt"].is_array(),
        "a receipt, and instead: {asked}"
    );
    let paper: String = printed["receipt"]
        .as_array()
        .expect("a receipt")
        .iter()
        .map(|line| line["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(paper.contains("Karim, flat 3"), "{paper}");
    assert!(paper.contains("009876543-0202"), "the buyer's BIN: {paper}");

    // And the sale itself reaches the shop, so the debt is on the book.
    for now_ms in 30..60_u64 {
        let stepped: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#
        )))
        .unwrap();
        let step = &stepped["step"];
        if step["action"] == "wait" {
            break;
        }
        let kind = step["kind"].as_str().unwrap();
        let reply = post_hex(
            &app,
            step["path"].as_str().unwrap(),
            step["body"].as_str().unwrap(),
            &token,
        )
        .await;
        let applied = till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"{kind}","body":"{reply}","now_ms":{now_ms}}}"#
        ));
        assert!(applied.contains("\"error\":null"), "{kind}: {applied}");
    }

    let (_, known): (_, CustomersResponse) = call(
        &app,
        "/v1/customers",
        &CustomersRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
        },
        &token,
    )
    .await;
    let held = known
        .customers
        .iter()
        .find(|one| one.id == buyer.to_u128())
        .expect("the shop has them now");
    assert_eq!(held.name, "Karim, flat 3");
    assert_eq!(held.phone.as_deref(), Some("01711000000"));
    assert_eq!(held.bin.as_deref(), Some("009876543-0202"));

    // What they owe is against the person, not the spelling.
    let (_, owed): (_, OwedResponse) = call(
        &app,
        "/v1/back-office/owed",
        &OwedRequest {
            protocol: PROTOCOL_VERSION,
            limit: 10,
            after_owed_minor: 0,
            after_person_key: String::new(),
        },
        &token,
    )
    .await;
    assert_eq!(owed.owing.len(), 1, "one person owes: {owed:?}");
    assert_eq!(owed.owing[0].owed_minor, 49_450);
}

/// A till learns what shop it is, and prints a receipt that says so.
///
/// The whole point of holding the details on the device: this receipt is
/// printed with the internet down, and a customer cannot take a nameless one
/// back to anybody.
#[tokio::test]
async fn a_till_prints_a_receipt_naming_the_shop_it_learned_from_the_server() {
    let (app, token) = shop();

    // The owner fills in what goes at the top of every receipt.
    let _: ShopResponse = call(
        &app,
        "/v1/back-office/shop",
        &PutShopRequest {
            protocol: PROTOCOL_VERSION,
            name: "Karim General Store".to_owned(),
            bin: Some("001234567-0101".to_owned()),
            address: Some("12 Mirpur Road, Dhaka".to_owned()),
            phone: None,
            wallets: vec!["bKash".to_owned(), "Nagad".to_owned()],
            stock_rule: 0,
        },
        &token,
    )
    .await
    .1;

    let mut till = TillHandle::open_on(
        MemoryBackend::new(),
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a till opens");
    till.set_token_for_test(&token);

    // A sale, rung before this device has ever heard of its shop.
    till.run_json(
        r#"{"op":"apply_items","items":[{"id":"00000000000000000000000001","code":"RICE5",
           "name":"Rice Miniket 5kg","price_minor":43000,"vat_bp":1500,"price_inclusive":false,
           "barcodes":["8690000000001"],"on_hand_milli":40000}]}"#,
    );
    till.run_json(r#"{"op":"scan","barcode":"8690000000001","qty_milli":1000}"#);
    till.run_json(r#"{"op":"add_cash","amount_minor":50000}"#);
    till.run_json(&format!(
        r#"{{"op":"checkout","ticket_id":"{}","rung_at_ms":1788600000000}}"#,
        Ulid::from_u128(900).encode()
    ));

    // The sale is fine. The receipt is not, and the till says which: printing a
    // nameless one is worse, because a customer cannot take it back to anybody.
    assert!(
        till.run_json(r#"{"op":"receipt","width":32,"rung_at":"06 Sep 2026 15:42"}"#)
            .contains("does not know its shop"),
        "a nameless receipt must be refused, not printed"
    );

    // Now let the driver work. The sale goes first, because a sale exists
    // nowhere else; the shop follows, before the catalogue.
    let mut asked_for_the_shop = false;
    for now_ms in 0..10_u64 {
        let stepped: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#
        )))
        .unwrap();
        let step = &stepped["step"];
        if step["action"] == "wait" {
            break;
        }
        let kind = step["kind"].as_str().unwrap();
        if kind == "shop" {
            asked_for_the_shop = true;
        }
        let reply = post_hex(
            &app,
            step["path"].as_str().unwrap(),
            step["body"].as_str().unwrap(),
            &token,
        )
        .await;
        let applied = till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"{kind}","body":"{reply}","now_ms":{now_ms}}}"#
        ));
        assert!(applied.contains("\"error\":null"), "{kind}: {applied}");
        if asked_for_the_shop {
            break;
        }
    }
    assert!(asked_for_the_shop, "a till has to learn what shop it is");

    // And the same sale, which happened before any of this, now prints.
    let printed: serde_json::Value = serde_json::from_str(
        &till.run_json(r#"{"op":"receipt","width":32,"rung_at":"06 Sep 2026 15:42"}"#),
    )
    .unwrap();
    let paper: String = printed["receipt"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| line["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(paper.contains("Karim General Store"), "{paper}");
    assert!(paper.contains("BIN 001234567-0101"), "{paper}");
    assert!(paper.contains("494.50"), "{paper}");

    // And the same sale as bytes a thermal printer understands, which is what
    // the Android till will write to a socket.
    let job: serde_json::Value = serde_json::from_str(
        &till.run_json(r#"{"op":"escpos","width":32,"rung_at":"06 Sep 2026 15:42"}"#),
    )
    .unwrap();
    let hex = job["job"]["bytes"].as_str().expect("printer bytes");
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect();

    assert!(
        bytes.starts_with(&[0x1B, 0x40]),
        "a job wakes the printer first"
    );
    assert!(
        bytes.ends_with(&[0x1D, 0x56, 0x42, 0x00]),
        "and ends by cutting"
    );
    assert!(
        bytes.windows(19).any(|w| w == b"Karim General Store"),
        "the shop is on the paper"
    );
    assert!(
        job["job"]["unprintable"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "an English receipt prints as written"
    );
}

/// An owner adds a cashier, a till learns who they are, and that cashier signs
/// in and is refused what they may not do.
///
/// The permission model existed for a long time with no way to put a person in
/// it, which made every check in it unreachable. This is the path that was
/// missing.
#[tokio::test]
async fn an_owner_adds_a_cashier_who_then_signs_in_at_the_till() {
    let (app, token) = shop();

    // The PIN is hashed here, on the owner's device, by the same code the till
    // will verify with. It never crosses the network.
    let cashier_id = Ulid::from_u128(70);
    let pin = PinHash::derive("1234", [9; SALT_LEN], 1_000);
    let _: OperatorsResponse = call(
        &app,
        "/v1/back-office/operators",
        &PutOperatorRequest {
            protocol: PROTOCOL_VERSION,
            operator: OperatorWire {
                id: cashier_id.to_u128(),
                name: "Rahim".to_owned(),
                pin_salt: [9_u8; SALT_LEN].to_vec(),
                pin_rounds: 1_000,
                pin_key: pin.key().to_vec(),
                max_discount_bp: 500,
                may_override_price: false,
                may_refund: false,
                may_void_line: false,
                may_authorise: false,
                may_open_drawer: true,
                may_close_shift: false,
                active: true,
            },
        },
        &token,
    )
    .await
    .1;

    let mut till = TillHandle::open_on(
        MemoryBackend::new(),
        &Ulid::from_u128(TENANT).encode(),
        &Ulid::from_u128(TERMINAL).encode(),
    )
    .expect("a till opens");
    till.set_token_for_test(&token);

    // Nobody can sign in yet, and the till says how many people it knows rather
    // than only that the PIN was wrong.
    let before: serde_json::Value =
        serde_json::from_str(&till.run_json(r#"{"op":"view"}"#)).unwrap();
    assert!(
        before["people"].as_array().is_some_and(Vec::is_empty),
        "nobody has been added to this device yet, which is a different problem \
         from a forgotten PIN and a screen should be able to say which"
    );

    // The driver fetches people, before the catalogue.
    let mut fetched = false;
    for now_ms in 0..10_u64 {
        let stepped: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
            r#"{{"op":"sync_step","online":true,"now_ms":{now_ms}}}"#
        )))
        .unwrap();
        if stepped["step"]["action"] == "wait" {
            break;
        }
        let kind = stepped["step"]["kind"].as_str().unwrap().to_owned();
        let reply = post_hex(
            &app,
            stepped["step"]["path"].as_str().unwrap(),
            stepped["step"]["body"].as_str().unwrap(),
            &token,
        )
        .await;
        let applied = till.run_json(&format!(
            r#"{{"op":"sync_apply","kind":"{kind}","body":"{reply}","now_ms":{now_ms}}}"#
        ));
        assert!(applied.contains("\"error\":null"), "{kind}: {applied}");
        if kind == "operators" {
            fetched = true;
            break;
        }
    }
    assert!(fetched, "a till has to learn who may use it");

    // A wrong PIN is refused and says how many tries remain.
    let wrong: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
        r#"{{"op":"sign_in","operator_id":"{}","pin":"0000","now_ms":0}}"#,
        cashier_id.encode()
    )))
    .unwrap();
    assert!(
        wrong["error"]
            .as_str()
            .unwrap_or_default()
            .contains("wrong PIN"),
        "{wrong}"
    );
    assert!(wrong["operator"].is_null());

    // The right one signs them in, with the permissions the owner set.
    let signed: serde_json::Value = serde_json::from_str(&till.run_json(&format!(
        r#"{{"op":"sign_in","operator_id":"{}","pin":"1234","now_ms":0}}"#,
        cashier_id.encode()
    )))
    .unwrap();
    assert_eq!(signed["error"], serde_json::Value::Null, "{signed}");
    assert_eq!(signed["operator"]["name"], "Rahim");
    assert_eq!(signed["operator"]["may_open_drawer"], true);
    assert_eq!(
        signed["operator"]["may_refund"], false,
        "a cashier the owner did not trust with refunds must not have them"
    );
}

/// A drawer carries the name the shop holds, not the name the device typed.
///
/// The id on a count is the till's word and has to be: the server knows which
/// device holds a credential and can never know who is standing at it. The name
/// is not the same thing, because the shop issued every id it has and holds its
/// own answer for each one. Until this, a device could report any name at all
/// against a count, and it was written down and shown to an owner as fact,
/// months later, by somebody deciding whether to trust a person with the till.
#[tokio::test]
async fn a_counted_drawer_carries_the_name_the_shop_holds_for_whoever_counted_it() {
    const RAHIM: u128 = 0x8001;
    const NOBODY: u128 = 0x9009;

    let repo = MemoryRepo::new();
    let token = repo.enrol_with_token(TENANT, TERMINAL).into_string();
    repo.put_operator(
        TENANT,
        &openpos_server::repo::OperatorRecord {
            id: RAHIM,
            name: "Rahim".to_owned(),
            pin_salt: vec![0; SALT_LEN],
            pin_rounds: 100_000,
            pin_key: vec![0; 32],
            max_discount_bp: 0,
            may_override_price: false,
            may_refund: false,
            may_void_line: false,
            may_authorise: false,
            may_open_drawer: true,
            may_close_shift: true,
            active: true,
        },
    )
    .await
    .unwrap();
    let state = AppState::new(repo);
    let repo = std::sync::Arc::clone(&state.repo);
    let app = router(state);

    let counted = |id: u128, by: u128, name: &str| ClosedShiftWire {
        id,
        terminal: TERMINAL,
        closed_by: by,
        closed_by_name: name.to_owned(),
        opened_at_ms: 1_000,
        closed_at_ms: 2_000,
        opening_float_minor: 100_000,
        sales: 3,
        cash_sales_minor: 150_000,
        non_cash_sales_minor: 0,
        cash_in_minor: 0,
        cash_out_minor: 0,
        expected_cash_minor: 250_000,
        counted_cash_minor: 249_000,
        variance_minor: -1_000,
        expected_from_sales_minor: None,
        struck_out_cash_minor: None,
    };

    let (status, _): (_, PushShiftsResponse) = call(
        &app,
        "/v1/sync/shifts",
        &PushShiftsRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            shifts: vec![
                // A real person under somebody else's name.
                counted(0x01, RAHIM, "Fatima"),
                // An id this shop never issued.
                counted(0x02, NOBODY, "Karim"),
                // A build from before anybody was named.
                counted(0x03, 0, ""),
            ],
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "every count is kept");

    let mut kept = repo.shifts_after(TENANT, 0, u64::MAX, 10).await.unwrap();
    kept.sort_by_key(|shift| shift.id);
    assert_eq!(kept.len(), 3, "a name the shop cannot vouch for loses no money");

    assert_eq!(kept[0].closed_by, RAHIM);
    assert_eq!(
        kept[0].closed_by_name, "Rahim",
        "the shop's own name for that id, not the one the device typed"
    );

    assert_eq!(
        kept[1].closed_by, NOBODY,
        "the id is still written down: it is what the device claimed and the \
         claim is evidence about the device"
    );
    assert!(
        kept[1].closed_by_name.is_empty(),
        "and no name at all, because a blank reads as nobody and a wrong name \
         reads as a person"
    );

    assert_eq!(kept[2].closed_by, 0);
    assert!(
        kept[2].closed_by_name.is_empty(),
        "an older build names nobody and that is the truth about it"
    );

    // The variance is what an owner rings up about, and it is untouched by any
    // of this.
    for shift in &kept {
        assert_eq!(shift.variance_minor, -1_000);
        assert_eq!(shift.counted_cash_minor, 249_000);
    }
}
