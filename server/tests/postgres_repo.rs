//! The Postgres repository, against a real Postgres.
//!
//! Skipped unless the database URLs are set, so the everyday suite stays fast
//! and needs no infrastructure. Run them with:
//!
//! ```text
//! docker compose up -d
//! OPENPOS_TEST_ADMIN_DATABASE_URL=postgres://postgres:postgres@localhost:5433/openpos \
//! OPENPOS_TEST_DATABASE_URL=postgres://openpos_app:openpos_app@localhost:5433/openpos \
//!     cargo test -p openpos-server --test postgres_repo
//! ```
//!
//! Two URLs, because they are two different jobs. Migrating needs rights to
//! create tables; the application needs only to read and write rows. The
//! application URL must name a role that is **not** a superuser: superusers
//! bypass row-level security, so running these as `postgres` would make the
//! isolation tests pass while proving nothing at all.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::protocol::{ItemWire, QuarantineReason};
use openpos_server::auth::{Caller, EnrolmentCode, Role, TokenHash};
use std::time::Duration;

use openpos_server::pg::PgRepo;
use openpos_server::repo::{
    Admission, CatalogueRecord, GoodsReceipt, ReceiptLine, RepoError, Repository, StockCorrection,
    StockCount, StoredSale, Supplier,
};

/// A receipt number no other test will pick.
fn receipt() -> String {
    format!("T1-{:06}", unique() % 1_000_000)
}

/// A fresh identifier, unique within and across runs.
///
/// The clock alone is not enough: measured on this machine, eight consecutive
/// reads of the system clock produced three distinct values, because the
/// granularity is coarser than the time it takes to call it. An earlier version
/// of this helper used only the clock, which handed two different shops the same
/// id and made the isolation test below compare a shop with itself. The counter
/// is what makes the values actually distinct; the clock and pid only keep
/// separate runs apart.
fn unique() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let seq = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    (nanos << 32) ^ (u128::from(std::process::id()) << 96) ^ seq
}

async fn repo() -> Option<PgRepo> {
    let admin = std::env::var("OPENPOS_TEST_ADMIN_DATABASE_URL").ok()?;
    let url = std::env::var("OPENPOS_TEST_DATABASE_URL").ok()?;

    PgRepo::migrate(&admin)
        .await
        .expect("cannot migrate the test database");
    Some(
        PgRepo::connect(&url, 5)
            .await
            .expect("cannot reach the test database"),
    )
}

/// Skip rather than fail when no database is configured.
macro_rules! database {
    () => {
        match repo().await {
            Some(repo) => repo,
            None => {
                eprintln!("skipping: set OPENPOS_TEST_ADMIN_DATABASE_URL and OPENPOS_TEST_DATABASE_URL to run these");
                return;
            }
        }
    };
}

fn item(id: u128, price_minor: i64) -> ItemWire {
    ItemWire {
        id,
        code: format!("SKU{}", id % 1_000),
        name_en: "Rice Miniket 5kg".to_owned(),
        name_bn: "মিনিকেট চাল ৫ কেজি".to_owned(),
        unit: "Nos".to_owned(),
        price_minor,
        cost_minor: price_minor / 2,
        vat_bp: 1_500,
        price_inclusive: false,
        vat_on_undiscounted: false,
        barcodes: vec![format!("869{:010}", id % 1_000_000)],
        on_hand_milli: 40_000,
        active: true,
    }
}

fn sale(tenant: u128, terminal: u128, id: u128, receipt: Option<&str>) -> StoredSale {
    StoredSale {
        tenant,
        terminal,
        id,
        receipt_no: receipt.map(ToOwned::to_owned),
        receipt_epoch: receipt.map(|_| 1),
        rung_at_ms: 1_788_600_000_000,
        total_minor: 49_450,
        payload: vec![1, 2, 3, 4],
        quarantine: None,
        stock: vec![(id, -1_000)],
    }
}

#[tokio::test]
async fn migrations_run_and_a_shop_can_be_enrolled() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());

    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    assert!(repo.terminal_enrolled(tenant, terminal).await.unwrap());
    assert!(!repo.terminal_enrolled(tenant, unique()).await.unwrap());
}

#[tokio::test]
async fn stores_a_sale_and_recognises_a_replay() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    assert!(!repo.has_sale(tenant, id).await.unwrap());

    repo.store_sale(sale(tenant, terminal, id, Some("T1-000100")))
        .await
        .unwrap();
    assert!(repo.has_sale(tenant, id).await.unwrap());
    assert!(repo.receipt_taken(tenant, "T1-000100", 1).await.unwrap());

    // A replay of the same sale is a no-op rather than a duplicate row.
    repo.store_sale(sale(tenant, terminal, id, Some("T1-000100")))
        .await
        .unwrap();
    assert!(repo.has_sale(tenant, id).await.unwrap());
}

#[tokio::test]
async fn renewal_overlaps_rather_than_cutting_a_till_off() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let caller = Caller {
        tenant,
        terminal,
        role: Role::Owner,
    };

    let old = openpos_server::auth::Token::generate();
    repo.store_token(caller, &old.hash()).await.unwrap();

    let new = openpos_server::auth::Token::generate();
    repo.renew_token(caller, &old.hash(), &new.hash(), Duration::from_secs(3_600))
        .await
        .unwrap();

    assert!(repo.authenticate(&new.hash()).await.unwrap().is_some());
    assert!(
        repo.authenticate(&old.hash()).await.unwrap().is_some(),
        "the reply can be lost, so the old credential must outlive the new one's arrival"
    );

    // The old one is on a deadline, though, and it is the near one.
    let old_expiry = repo.token_expiry_for_test(&old.hash()).await.unwrap();
    let new_expiry = repo.token_expiry_for_test(&new.hash()).await.unwrap();
    assert!(old_expiry < new_expiry, "{old_expiry:?} then {new_expiry:?}");
}

#[tokio::test]
async fn renewing_in_a_loop_cannot_keep_an_old_credential_alive() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let caller = Caller {
        tenant,
        terminal,
        role: Role::Owner,
    };

    let old = openpos_server::auth::Token::generate();
    repo.store_token(caller, &old.hash()).await.unwrap();
    repo.renew_token(
        caller,
        &old.hash(),
        &openpos_server::auth::Token::generate().hash(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let first = repo.token_expiry_for_test(&old.hash()).await.unwrap();

    // A second renewal naming the same old token must not push its deadline
    // back, or a device could hold one alive indefinitely.
    repo.renew_token(
        caller,
        &old.hash(),
        &openpos_server::auth::Token::generate().hash(),
        Duration::from_secs(86_400),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.token_expiry_for_test(&old.hash()).await.unwrap(),
        first,
        "renewal must never extend a credential"
    );
}

#[tokio::test]
async fn a_credentials_role_survives_a_round_trip_through_the_database() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let till = openpos_server::auth::Token::generate();
    repo.store_token_as(
        Caller {
            tenant,
            terminal,
            role: Role::Till,
        },
        &till.hash(),
        Role::Till,
    )
    .await
    .unwrap();

    let caller = repo.authenticate(&till.hash()).await.unwrap().unwrap();
    assert_eq!(
        caller.role,
        Role::Till,
        "a role that does not survive storage is no role at all"
    );
    assert!(!caller.role.covers(Role::Owner));
}

#[tokio::test]
async fn an_enrolment_code_carries_the_role_the_device_will_get() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let code = EnrolmentCode::generate();
    repo.issue_enrolment_code(
        Caller {
            tenant,
            terminal,
            role: Role::Till,
        },
        &code.hash(),
        Duration::from_secs(900),
    )
    .await
    .unwrap();

    let redeemed = repo.redeem_enrolment_code(&code.hash()).await.unwrap().unwrap();
    assert_eq!(
        redeemed.role,
        Role::Till,
        "a code issued for a till must not hand back an owner"
    );
}

#[tokio::test]
async fn a_credential_stops_working_when_it_expires() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let token = openpos_server::auth::Token::generate();
    repo.store_token(
        Caller {
            tenant,
            terminal,
            role: Role::Owner,
        },
        &token.hash(),
    )
        .await
        .unwrap();
    assert!(
        repo.authenticate(&token.hash()).await.unwrap().is_some(),
        "a fresh credential works"
    );

    // A tablet sold on, or lost, or handed back by a departing employee. The
    // shops this is for do not have somebody whose job it is to notice.
    repo.expire_token_for_test(&token.hash()).await.unwrap();
    assert!(
        repo.authenticate(&token.hash()).await.unwrap().is_none(),
        "an expired credential must stop working on its own"
    );
}

#[tokio::test]
async fn using_a_credential_records_that_it_was_used() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let token = openpos_server::auth::Token::generate();
    repo.store_token(
        Caller {
            tenant,
            terminal,
            role: Role::Owner,
        },
        &token.hash(),
    )
        .await
        .unwrap();

    // issued_at says a credential was created, not that anything ever presented
    // it, and "used from two places today" is otherwise unanswerable.
    assert_eq!(repo.token_last_used_for_test(&token.hash()).await.unwrap(), None);
    repo.authenticate(&token.hash()).await.unwrap();
    assert!(repo.token_last_used_for_test(&token.hash()).await.unwrap().is_some());
}

#[tokio::test]
async fn breakage_moves_stock_and_stays_distinguishable_from_a_count() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    repo.receive_goods(
        tenant,
        &GoodsReceipt {
            id: unique(),
            supplier_id: None,
            reference: None,
            received_at_ms: 1_000,
            received_by: terminal,
            note: None,
            lines: vec![ReceiptLine {
                item_id: sku,
                qty_milli: 60_000,
                unit_cost_minor: 38_000,
            }],
        },
    )
    .await
    .unwrap();

    let correction = StockCorrection {
        id: unique(),
        item_id: sku,
        qty_milli: -5_000,
        reason: "five broken in the crate".to_owned(),
        occurred_at_ms: 2_000,
        recorded_by: terminal,
    };
    assert!(repo.correct_stock(tenant, &correction).await.unwrap());
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 55_000);

    // A retry after a dropped reply must not write the loss off twice.
    assert!(!repo.correct_stock(tenant, &correction).await.unwrap());
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 55_000);
}

#[tokio::test]
async fn a_correction_without_a_reason_is_refused() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // An unexplained correction is indistinguishable from theft when the
    // variance is read a month later.
    let outcome = repo
        .correct_stock(
            tenant,
            &StockCorrection {
                id: unique(),
                item_id: sku,
                qty_milli: -5_000,
                reason: "   ".to_owned(),
                occurred_at_ms: 2_000,
                recorded_by: terminal,
            },
        )
        .await;

    assert!(matches!(outcome, Err(RepoError::Invalid)));
}

#[tokio::test]
async fn a_correction_before_a_count_is_superseded_by_it() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    repo.correct_stock(
        tenant,
        &StockCorrection {
            id: unique(),
            item_id: sku,
            qty_milli: -5_000,
            reason: "spoiled".to_owned(),
            occurred_at_ms: 1_000,
            recorded_by: terminal,
        },
    )
    .await
    .unwrap();

    repo.record_count(
        tenant,
        &StockCount {
            id: unique(),
            item_id: sku,
            counted_milli: 40_000,
            counted_at_ms: 5_000,
            counted_by: terminal,
            note: None,
        },
    )
    .await
    .unwrap();

    // The barrier does not care what kind of thing moved the stock.
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 40_000);

    repo.correct_stock(
        tenant,
        &StockCorrection {
            id: unique(),
            item_id: sku,
            qty_milli: -2_000,
            reason: "broken after the count".to_owned(),
            occurred_at_ms: 9_000,
            recorded_by: terminal,
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 38_000);
}

#[tokio::test]
async fn deliveries_read_back_newest_first_with_their_lines() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let supplier_id = unique();
    repo.put_supplier(
        tenant,
        &Supplier {
            id: supplier_id,
            name: "Karim Traders".to_owned(),
            phone: None,
            bin: None,
            active: true,
        },
    )
    .await
    .unwrap();

    let (first, second) = (unique(), unique());
    for (id, at_ms, reference) in [(first, 1_000_u64, "CH-1"), (second, 2_000, "CH-2")] {
        repo.receive_goods(
            tenant,
            &GoodsReceipt {
                id,
                supplier_id: Some(supplier_id),
                reference: Some(reference.to_owned()),
                received_at_ms: at_ms,
                received_by: terminal,
                note: None,
                lines: vec![ReceiptLine {
                    item_id: sku,
                    qty_milli: 12_000,
                    unit_cost_minor: 38_000,
                }],
            },
        )
        .await
        .unwrap();
    }

    let found = repo.deliveries(tenant, 20).await.unwrap();

    // Newest first: what a shop asks is what came in this week.
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, second);
    assert_eq!(found[1].id, first);

    // The challan number and the goods together, which is the whole reason to
    // file a delivery: the invoice and the shelf can be put side by side.
    assert_eq!(found[0].reference.as_deref(), Some("CH-2"));
    assert_eq!(found[0].supplier_id, Some(supplier_id));
    assert_eq!(found[0].lines.len(), 1);
    assert_eq!(found[0].lines[0].item_id, sku);
    assert_eq!(found[0].lines[0].qty_milli, 12_000);
    assert_eq!(found[0].lines[0].unit_cost_minor, 38_000);

    // The limit is honoured, or a shop open for five years gets every delivery
    // it has ever taken in one page.
    assert_eq!(repo.deliveries(tenant, 1).await.unwrap().len(), 1);

    // And another shop's deliveries are not this shop's. Row-level security is
    // what enforces it, and this is what proves the policy is on.
    let other = unique();
    repo.enrol(other, unique(), "Another Shop").await.unwrap();
    assert!(repo.deliveries(other, 20).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_delivery_puts_stock_in_and_is_not_booked_twice() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let supplier_id = unique();
    repo.put_supplier(
        tenant,
        &Supplier {
            id: supplier_id,
            name: "Karim Traders".to_owned(),
            phone: Some("01700000000".to_owned()),
            bin: None,
            active: true,
        },
    )
    .await
    .unwrap();

    let delivery = GoodsReceipt {
        id: unique(),
        supplier_id: Some(supplier_id),
        reference: Some("CHALLAN-4471".to_owned()),
        received_at_ms: 3_000,
        received_by: terminal,
        note: None,
        lines: vec![ReceiptLine {
            item_id: sku,
            qty_milli: 60_000,
            unit_cost_minor: 38_000,
        }],
    };

    assert!(repo.receive_goods(tenant, &delivery).await.unwrap());
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 60_000);

    // A back office retrying after a dropped reply. Stock booked twice is a shop
    // ordering against goods it does not have.
    assert!(
        !repo.receive_goods(tenant, &delivery).await.unwrap(),
        "a repeated delivery must be recognised, not booked again"
    );
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 60_000);

    let listed = repo.suppliers(tenant).await.unwrap();
    assert!(listed.iter().any(|s| s.id == supplier_id));
}

#[tokio::test]
async fn a_delivery_before_a_count_is_superseded_by_it() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    repo.receive_goods(
        tenant,
        &GoodsReceipt {
            id: unique(),
            supplier_id: None,
            reference: None,
            received_at_ms: 1_000,
            received_by: terminal,
            note: None,
            lines: vec![ReceiptLine {
                item_id: sku,
                qty_milli: 60_000,
                unit_cost_minor: 38_000,
            }],
        },
    )
    .await
    .unwrap();

    // Then somebody counts and finds fifty. The count is what the shelf holds,
    // whatever the delivery note said.
    repo.record_count(
        tenant,
        &StockCount {
            id: unique(),
            item_id: sku,
            counted_milli: 50_000,
            counted_at_ms: 5_000,
            counted_by: terminal,
            note: Some("ten short on the delivery".to_owned()),
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 50_000);

    // A delivery after the count moves the figure, exactly as a sale does. The
    // barrier does not care what kind of thing moved the stock.
    repo.receive_goods(
        tenant,
        &GoodsReceipt {
            id: unique(),
            supplier_id: None,
            reference: None,
            received_at_ms: 9_000,
            received_by: terminal,
            note: None,
            lines: vec![ReceiptLine {
                item_id: sku,
                qty_milli: 20_000,
                unit_cost_minor: 38_000,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 70_000);
}

#[tokio::test]
async fn a_count_supersedes_everything_before_it() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Ten sold before anybody counted.
    let mut early = sale(tenant, terminal, unique(), Some(&receipt()));
    early.rung_at_ms = 1_000;
    early.stock = vec![(sku, -10_000)];
    repo.admit_sale(early).await.unwrap();

    // Then the shelf is counted at forty, whatever the ledger believed.
    repo.record_count(
        tenant,
        &StockCount {
            id: unique(),
            item_id: sku,
            counted_milli: 40_000,
            counted_at_ms: 5_000,
            counted_by: terminal,
            note: Some("Friday count".to_owned()),
        },
    )
    .await
    .unwrap();

    let on_hand = repo.on_hand(tenant, sku).await.unwrap();
    assert_eq!(
        on_hand.qty_milli, 40_000,
        "a count is an assertion about the shelf, not another movement"
    );
    assert_eq!(on_hand.counted_at_ms, Some(5_000));
    assert_eq!(on_hand.unreconciled_sales, 0);

    // A sale after the count moves the figure normally.
    let mut later = sale(tenant, terminal, unique(), Some(&receipt()));
    later.rung_at_ms = 9_000;
    later.stock = vec![(sku, -3_000)];
    repo.admit_sale(later).await.unwrap();
    assert_eq!(repo.on_hand(tenant, sku).await.unwrap().qty_milli, 37_000);
}

#[tokio::test]
async fn a_sale_that_arrives_after_the_count_that_should_have_seen_it_is_raised_not_guessed() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Counted at noon.
    repo.record_count(
        tenant,
        &StockCount {
            id: unique(),
            item_id: sku,
            counted_milli: 40_000,
            counted_at_ms: 5_000,
            counted_by: terminal,
            note: None,
        },
    )
    .await
    .unwrap();

    // A till that was offline all morning finally syncs a sale rung at nine.
    // Applying it decrements goods the counter may already have seen were gone;
    // ignoring it silently loses a real sale. Neither is detectable later.
    let mut stranded = sale(tenant, terminal, unique(), Some(&receipt()));
    stranded.rung_at_ms = 1_000;
    stranded.stock = vec![(sku, -2_000)];
    repo.admit_sale(stranded).await.unwrap();

    let on_hand = repo.on_hand(tenant, sku).await.unwrap();
    assert_eq!(
        on_hand.qty_milli, 40_000,
        "the counted figure stands; the late sale must not quietly rewrite it"
    );
    assert_eq!(on_hand.unreconciled_milli, -2_000);
    assert_eq!(
        on_hand.unreconciled_sales, 1,
        "and somebody is told, rather than the ledger picking an answer"
    );
}

#[tokio::test]
async fn the_newest_count_wins_whatever_order_the_counts_arrived_in() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // The later count is recorded first, as happens when one terminal syncs
    // before another that was offline.
    for (counted_at_ms, counted_milli) in [(9_000_u64, 12_000_i64), (5_000, 40_000)] {
        repo.record_count(
            tenant,
            &StockCount {
                id: unique(),
                item_id: sku,
                counted_milli,
                counted_at_ms,
                counted_by: terminal,
                note: None,
            },
        )
        .await
        .unwrap();
    }

    assert_eq!(
        repo.on_hand(tenant, sku).await.unwrap().qty_milli,
        12_000,
        "a count taken later describes a later shelf, whatever order they landed in"
    );
}

#[tokio::test]
async fn an_item_never_counted_falls_back_to_its_running_total() {
    let repo = database!();
    let (tenant, terminal, sku) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let mut sold = sale(tenant, terminal, unique(), Some(&receipt()));
    sold.stock = vec![(sku, -4_000)];
    repo.admit_sale(sold).await.unwrap();

    let on_hand = repo.on_hand(tenant, sku).await.unwrap();
    assert_eq!(on_hand.qty_milli, -4_000);
    assert_eq!(
        on_hand.counted_at_ms, None,
        "and the answer says it rests on no count, so a shop knows what it is looking at"
    );
}

#[tokio::test]
async fn a_catalogue_row_written_before_the_tax_base_existed_still_reads() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Exactly the bytes version 1 wrote, with no tax base in them. Every shop's
    // catalogue is full of these, and a build that could not read them would
    // stop every till pulling at once.
    let id = unique();
    let legacy = openpos_core::protocol::ItemWireV1 {
        id,
        code: "RICE5".to_owned(),
        name_en: "Rice Miniket 5kg".to_owned(),
        name_bn: "মিনিকেট চাল ৫ কেজি".to_owned(),
        unit: "Nos".to_owned(),
        price_minor: 43_000,
        cost_minor: 38_000,
        vat_bp: 1_500,
        price_inclusive: false,
        barcodes: vec!["8690000000001".to_owned()],
        on_hand_milli: 40_000,
        active: true,
    };

    repo.put_catalogue(
        tenant,
        &[CatalogueRecord {
            seq: 1,
            kind: 1,
            item_id: id,
            payload: Some(postcard::to_allocvec(&legacy).unwrap()),
            schema: 1,
        }],
    )
    .await
    .unwrap();

    let page = repo.items_since(tenant, 0, 100).await.unwrap();
    assert_eq!(page.skipped, 0, "a version 1 row is readable, not skipped");
    assert_eq!(page.upserts.len(), 1);
    assert_eq!(page.upserts[0].price_minor, 43_000);
    assert!(
        !page.upserts[0].vat_on_undiscounted,
        "an item written before the choice existed was taxed the ordinary way"
    );
}

#[tokio::test]
async fn a_catalogue_row_this_build_cannot_read_is_skipped_not_fatal() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    repo.upsert_item(tenant, &item(unique(), 43_000)).await.unwrap();
    // A row written by a newer build during a rolling upgrade, sharing this
    // database. Failing the page would make it a permanent poison pill: every
    // pull for this shop returns a backend error, the HTTP layer turns it into a
    // 503, and every till in the shop stops syncing with no way past it.
    repo.put_catalogue(
        tenant,
        &[CatalogueRecord {
            seq: 9_000_000,
            kind: 1,
            item_id: unique(),
            payload: Some(vec![0xFF, 0xFF, 0xFF]),
            schema: 99,
        }],
    )
    .await
    .unwrap();
    repo.upsert_item(tenant, &item(unique(), 51_000)).await.unwrap();

    let page = repo.items_since(tenant, 0, 100).await.unwrap();
    assert_eq!(page.skipped, 1, "the unreadable row is counted, not swallowed");
    assert_eq!(page.upserts.len(), 2, "and the readable ones still arrive");
}

#[tokio::test]
async fn two_connections_racing_for_one_receipt_number_cannot_both_win() {
    let repo = database!();
    let (tenant, first_terminal, second_terminal) = (unique(), unique(), unique());
    repo.enrol(tenant, first_terminal, "Test Shop").await.unwrap();
    repo.enrol(tenant, second_terminal, "Test Shop").await.unwrap();

    // A tablet restored from a backup, pushing its backlog beside the device it
    // was copied from. This is the one moment the duplicate check matters, and
    // it is exactly when both pushes arrive together.
    let number = format!("T1-{:06}", unique() % 1_000_000);
    let one = sale(tenant, first_terminal, unique(), Some(&number));
    let two = sale(tenant, second_terminal, unique(), Some(&number));

    let (left, right) = tokio::join!(repo.admit_sale(one), repo.admit_sale(two));
    let (left, right) = (left.unwrap(), right.unwrap());

    // Two separate connections, two real transactions. The check used to be a
    // read in its own transaction, so both saw the number free and both stored
    // clean; a primary key on the claim is what makes that impossible.
    let winners = [&left, &right]
        .iter()
        .filter(|admission| ***admission == Admission::Stored)
        .count();
    assert_eq!(winners, 1, "got {left:?} and {right:?}");
    assert!(
        matches!(left, Admission::DuplicateReceipt { .. })
            || matches!(right, Admission::DuplicateReceipt { .. }),
        "the loser must be named as a duplicate, not silently accepted"
    );
}

#[tokio::test]
async fn a_duplicate_receipt_is_stored_and_flagged_rather_than_refused() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let number = format!("T1-{:06}", unique() % 1_000_000);
    let first = unique();
    let second = unique();
    assert_eq!(
        repo.admit_sale(sale(tenant, terminal, first, Some(&number)))
            .await
            .unwrap(),
        Admission::Stored
    );
    assert_eq!(
        repo.admit_sale(sale(tenant, terminal, second, Some(&number)))
            .await
            .unwrap(),
        Admission::DuplicateReceipt { held_by: first },
        "and the reply names the sale that holds it"
    );

    // The goods left the shop and the money changed hands, so the sale is kept.
    assert!(repo.has_sale(tenant, second).await.unwrap());
    let queue = repo.repair_queue(tenant, 10).await.unwrap();
    assert!(queue.iter().any(|entry| entry.id == second));
}

#[tokio::test]
async fn a_replay_is_recognised_without_a_separate_read() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let number = format!("T1-{:06}", unique() % 1_000_000);
    repo.admit_sale(sale(tenant, terminal, id, Some(&number)))
        .await
        .unwrap();

    // A till retrying after a dropped reply. Doing the work twice would double
    // the stock movement; refusing it would strand the sale on the tablet.
    assert_eq!(
        repo.admit_sale(sale(tenant, terminal, id, Some(&number)))
            .await
            .unwrap(),
        Admission::AlreadyStored
    );
}

#[tokio::test]
async fn a_quarantined_sale_is_stored_with_its_reason() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let mut suspect = sale(tenant, terminal, id, Some("T1-000200"));
    suspect.quarantine = Some(QuarantineReason::TotalsMismatch {
        stored_minor: 1,
        recomputed_minor: 49_450,
    });
    repo.store_sale(suspect).await.unwrap();

    let queue = repo.quarantined(tenant, 10).await.unwrap();
    assert_eq!(queue.len(), 1);
    assert!(
        queue[0].1.contains("49450"),
        "the repair queue must say what disagreed: {}",
        queue[0].1
    );
}

#[tokio::test]
async fn lease_blocks_never_overlap() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let first = repo.issue_lease(tenant, terminal, 500).await.unwrap();
    let second = repo.issue_lease(tenant, terminal, 500).await.unwrap();

    assert_eq!((first.first, first.last), (1, 500));
    assert_eq!((second.first, second.last), (501, 1_000));
    assert!(second.first > first.last);
    assert_eq!(first.epoch, 1);
}

#[tokio::test]
async fn refuses_a_lease_for_a_terminal_that_is_not_enrolled() {
    let repo = database!();
    let tenant = unique();
    repo.enrol(tenant, unique(), "Test Shop").await.unwrap();

    assert_eq!(
        repo.issue_lease(tenant, unique(), 10).await,
        Err(RepoError::UnknownTerminal)
    );
}

#[tokio::test]
async fn catalogue_changes_page_in_order() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let first_item = unique();
    let second_item = unique();
    repo.upsert_item(tenant, &item(first_item, 43_000)).await.unwrap();
    repo.upsert_item(tenant, &item(second_item, 47_500)).await.unwrap();
    repo.delete_item(tenant, first_item).await.unwrap();

    let page = repo.items_since(tenant, 0, 2).await.unwrap();
    assert_eq!(page.upserts.len(), 2);
    assert_eq!(page.cursor, 2);
    assert!(page.more, "a till must know to ask again");

    let rest = repo.items_since(tenant, page.cursor, 10).await.unwrap();
    assert_eq!(rest.tombstones, vec![first_item]);
    assert_eq!(rest.cursor, 3);
    assert!(!rest.more);
}

/// The test the whole row-level security arrangement exists for.
#[tokio::test]
async fn one_shop_cannot_see_another_shops_sales() {
    let repo = database!();
    let (shop_a, terminal_a) = (unique(), unique());
    let (shop_b, terminal_b) = (unique(), unique());
    assert_ne!(shop_a, shop_b, "the fixture must actually create two shops");
    repo.enrol(shop_a, terminal_a, "Shop A").await.unwrap();
    repo.enrol(shop_b, terminal_b, "Shop B").await.unwrap();

    let sale_id = unique();
    repo.store_sale(sale(shop_a, terminal_a, sale_id, Some("T1-000100")))
        .await
        .unwrap();

    // Shop A sees its own sale.
    assert!(repo.has_sale(shop_a, sale_id).await.unwrap());

    // Shop B asks for the very same id and finds nothing.
    //
    // Note what is not happening: `has_sale` runs `where id = $1` with no tenant
    // predicate whatsoever. Nothing in the query mentions which shop is asking.
    // If row-level security were inert, this assertion would fail, which is
    // exactly what makes it a test of the policy rather than of a WHERE clause.
    assert!(
        !repo.has_sale(shop_b, sale_id).await.unwrap(),
        "row-level security did not isolate the shops. Check that the connecting \
         role is not a superuser and that the tables are marked force row level security"
    );
    assert!(!repo.receipt_taken(shop_b, "T1-000100", 1).await.unwrap());

    // And a terminal belongs to exactly one shop.
    assert!(repo.terminal_enrolled(shop_a, terminal_a).await.unwrap());
    assert!(!repo.terminal_enrolled(shop_b, terminal_a).await.unwrap());
}

/// Catalogue changes are isolated the same way, which matters because a price
/// list is commercially sensitive.
#[tokio::test]
async fn one_shop_cannot_pull_another_shops_catalogue() {
    let repo = database!();
    let (shop_a, terminal_a) = (unique(), unique());
    let (shop_b, terminal_b) = (unique(), unique());
    assert_ne!(shop_a, shop_b, "the fixture must actually create two shops");
    repo.enrol(shop_a, terminal_a, "Shop A").await.unwrap();
    repo.enrol(shop_b, terminal_b, "Shop B").await.unwrap();

    repo.upsert_item(shop_a, &item(unique(), 43_000)).await.unwrap();

    assert_eq!(repo.items_since(shop_a, 0, 100).await.unwrap().upserts.len(), 1);
    assert!(
        repo.items_since(shop_b, 0, 100).await.unwrap().upserts.is_empty(),
        "a shop must not see another shop's prices"
    );
}

/// A credential proves which terminal is calling, and nothing else can.
#[tokio::test]
async fn a_token_resolves_to_exactly_one_terminal() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());

    let token = repo
        .enrol_with_token(tenant, terminal, "Test Shop")
        .await
        .unwrap();

    let caller = repo
        .authenticate(&TokenHash::of(token.as_str()))
        .await
        .unwrap()
        .expect("the token it just issued must authenticate");
    assert_eq!(caller.tenant, tenant);
    assert_eq!(caller.terminal, terminal);

    // Anything else resolves to nobody, rather than to an error that would tell
    // an attacker whether a guess was close.
    assert!(repo
        .authenticate(&TokenHash::of("not a real token"))
        .await
        .unwrap()
        .is_none());
}

/// The database stores hashes, so a copy of it is not a set of working keys.
#[tokio::test]
async fn the_stored_credential_is_not_the_credential() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    let token = repo
        .enrol_with_token(tenant, terminal, "Test Shop")
        .await
        .unwrap();

    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "select token_hash from terminal_token where tenant_id = $1",
    )
    .bind(uuid::Uuid::from_u128(tenant))
    .fetch_all(repo.pool())
    .await
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert_ne!(
        rows[0],
        token.as_str().as_bytes(),
        "the token itself must never be stored"
    );
    assert_eq!(rows[0], TokenHash::of(token.as_str()).as_bytes());
}

#[tokio::test]
async fn an_enrolment_code_is_single_use_and_expires() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let caller = Caller {
        tenant,
        terminal,
        role: Role::Owner,
    };

    let code = EnrolmentCode::generate();
    repo.issue_enrolment_code(caller, &code.hash(), std::time::Duration::from_secs(900))
        .await
        .unwrap();

    let redeemed = repo.redeem_enrolment_code(&code.hash()).await.unwrap();
    assert_eq!(redeemed, Some(caller));

    // A second attempt finds nothing. Two devices racing cannot both win,
    // because consuming and reading happen in one statement.
    assert!(repo.redeem_enrolment_code(&code.hash()).await.unwrap().is_none());

    // An expired code is refused, and is indistinguishable from an unknown one.
    let stale = EnrolmentCode::generate();
    repo.issue_enrolment_code(caller, &stale.hash(), std::time::Duration::from_secs(0))
        .await
        .unwrap();
    assert!(repo.redeem_enrolment_code(&stale.hash()).await.unwrap().is_none());
    assert!(repo
        .redeem_enrolment_code(&EnrolmentCode::generate().hash())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_revoked_credential_no_longer_authenticates() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    let token = repo
        .enrol_with_token(tenant, terminal, "Test Shop")
        .await
        .unwrap();
    let hash = TokenHash::of(token.as_str());

    assert!(repo.authenticate(&hash).await.unwrap().is_some());
    assert!(repo.revoke_token(&hash).await.unwrap());
    assert!(repo.authenticate(&hash).await.unwrap().is_none());

    // Revoking again changes nothing, so an operator can run it twice safely.
    assert!(!repo.revoke_token(&hash).await.unwrap());
}

/// What a shop does the moment a tablet is stolen.
#[tokio::test]
async fn every_credential_for_a_terminal_can_be_withdrawn_at_once() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    let first = repo.enrol_with_token(tenant, terminal, "Counter").await.unwrap();
    let second = repo.enrol_with_token(tenant, terminal, "Counter").await.unwrap();

    let withdrawn = repo
        .revoke_all_tokens(Caller {
            tenant,
            terminal,
            role: Role::Owner,
        })
        .await
        .unwrap();
    assert_eq!(withdrawn, 2);

    for token in [first, second] {
        assert!(repo
            .authenticate(&TokenHash::of(token.as_str()))
            .await
            .unwrap()
            .is_none());
    }
}

/// The repair queue, worked through the way a shop actually works it.
#[tokio::test]
async fn a_resolved_sale_leaves_the_queue_and_the_sale_itself_stays() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let mut suspect = sale(tenant, terminal, id, Some("T1-000300"));
    suspect.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000300".to_owned(),
    });
    repo.store_sale(suspect).await.unwrap();

    let queue = repo.repair_queue(tenant, 50).await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, id);
    assert_eq!(queue[0].receipt_no.as_deref(), Some("T1-000300"));
    assert_eq!(queue[0].total_minor, 49_450);
    assert!(
        queue[0].received_at_ms > 1_700_000_000_000,
        "arrival must be a real wall clock time: {}",
        queue[0].received_at_ms
    );

    assert!(repo
        .resolve_quarantine(tenant, id, "restored from a backup, receipt reissued")
        .await
        .unwrap());
    assert!(repo.repair_queue(tenant, 50).await.unwrap().is_empty());

    // Resolving is not deleting. The sale happened, and the stored bytes are
    // what a dispute is settled against months later.
    assert!(repo.has_sale(tenant, id).await.unwrap());

    // A second person working the same queue is told nothing moved, rather than
    // overwriting the first one's note.
    assert!(!repo
        .resolve_quarantine(tenant, id, "second opinion")
        .await
        .unwrap());
}

#[tokio::test]
async fn resolving_a_sale_that_is_not_quarantined_changes_nothing() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    repo.store_sale(sale(tenant, terminal, id, Some("T1-000400")))
        .await
        .unwrap();

    assert!(!repo.resolve_quarantine(tenant, id, "nothing to fix").await.unwrap());
    assert!(!repo
        .resolve_quarantine(tenant, unique(), "no such sale")
        .await
        .unwrap());
}

/// One shop must not be able to clear another shop's queue, which is the case
/// row-level security has to cover for a write and not only for a read.
#[tokio::test]
async fn one_shop_cannot_resolve_another_shops_repair() {
    let repo = database!();
    let (shop_a, terminal_a) = (unique(), unique());
    let (shop_b, terminal_b) = (unique(), unique());
    repo.enrol(shop_a, terminal_a, "Shop A").await.unwrap();
    repo.enrol(shop_b, terminal_b, "Shop B").await.unwrap();

    let id = unique();
    let mut suspect = sale(shop_a, terminal_a, id, Some("T1-000500"));
    suspect.quarantine = Some(QuarantineReason::Undecodable);
    repo.store_sale(suspect).await.unwrap();

    assert!(
        !repo.resolve_quarantine(shop_b, id, "not mine to close").await.unwrap(),
        "the update names no tenant, so this only fails if the policy is inert"
    );
    assert!(repo.repair_queue(shop_b, 50).await.unwrap().is_empty());
    assert_eq!(repo.repair_queue(shop_a, 50).await.unwrap().len(), 1);
}

#[tokio::test]
async fn terminal_health_reports_what_a_support_call_starts_with() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Counter by the door").await.unwrap();

    // A device enrolled and not yet heard from. Absent, not zero: a zero would
    // render as 1970 and read as a fault rather than as silence.
    let health = repo.terminal_health(tenant).await.unwrap();
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].terminal, terminal);
    assert_eq!(health[0].label, "Counter by the door");
    assert_eq!(health[0].epoch, 1);
    assert_eq!(health[0].last_seen_ms, None);
    assert_eq!(health[0].sales, 0, "an unused till appears, rather than vanishing");
    assert!(health[0].enrolled_at_ms > 1_700_000_000_000);

    repo.mark_terminal_seen(tenant, terminal).await.unwrap();
    repo.store_sale(sale(tenant, terminal, unique(), Some("T1-000600")))
        .await
        .unwrap();
    let mut broken = sale(tenant, terminal, unique(), Some("T1-000601"));
    broken.quarantine = Some(QuarantineReason::Undecodable);
    repo.store_sale(broken).await.unwrap();

    let health = repo.terminal_health(tenant).await.unwrap();
    assert_eq!(health[0].sales, 2);
    assert_eq!(health[0].open_repairs, 1);
    let seen = health[0].last_seen_ms.expect("a till that synced must show it");
    assert!(seen > 1_700_000_000_000, "last seen must be a wall clock time: {seen}");
}

/// Health is per shop, like everything else. A count that leaked across tenants
/// would tell one shop how busy another is.
#[tokio::test]
async fn one_shop_cannot_see_another_shops_terminals() {
    let repo = database!();
    let (shop_a, terminal_a) = (unique(), unique());
    let (shop_b, terminal_b) = (unique(), unique());
    repo.enrol(shop_a, terminal_a, "Shop A").await.unwrap();
    repo.enrol(shop_b, terminal_b, "Shop B").await.unwrap();
    repo.store_sale(sale(shop_a, terminal_a, unique(), Some("T1-000700")))
        .await
        .unwrap();

    let health = repo.terminal_health(shop_b).await.unwrap();
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].terminal, terminal_b);
    assert_eq!(health[0].sales, 0, "the join must not count another shop's sales");
}

/// The catalogue routes go through the trait, so the trait has to record the
/// same change the inherent editor did.
#[tokio::test]
async fn an_edited_item_appears_as_a_catalogue_change() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let cursor = Repository::upsert_item(&repo, tenant, &item(id, 43_000))
        .await
        .unwrap();
    let deleted = Repository::delete_item(&repo, tenant, id).await.unwrap();
    assert!(deleted > cursor, "each edit takes its own sequence");

    let page = repo.items_since(tenant, 0, 10).await.unwrap();
    assert_eq!(page.upserts.len(), 1);
    assert_eq!(page.upserts[0].price_minor, 43_000);
    assert_eq!(page.tombstones, vec![id]);
}

/// The back office over HTTP, against the real database rather than the
/// in-memory store.
///
/// The unit tests exercise the handlers over `MemoryRepo` and these exercise the
/// queries over Postgres, and neither would catch a column named one thing in
/// the migration and another in the handler's response. This is the one test
/// that runs the whole path an owner's console actually takes.
#[tokio::test]
async fn the_back_office_works_over_http_against_postgres() {
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use openpos_core::protocol::{
        CatalogueEditResponse, PullRequest, PullResponse, RepairQueueRequest, RepairQueueResponse,
        ResolveRepairRequest, ResolveRepairResponse, TerminalHealthRequest, TerminalHealthResponse,
        UpsertItemRequest, PROTOCOL_VERSION,
    };
    use openpos_server::http::{router, AppState, CONTENT_TYPE};
    use tower::ServiceExt;

    async fn call<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        app: &axum::Router,
        path: &str,
        body: &T,
        token: &str,
    ) -> (StatusCode, Option<R>) {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, CONTENT_TYPE)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(postcard::to_allocvec(body).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, postcard::from_bytes::<R>(&bytes).ok())
    }

    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    let token = repo
        .enrol_with_token(tenant, terminal, "Counter by the door")
        .await
        .unwrap();
    let token = token.into_string();

    // A sale that needs a human, put there before the repo moves into the router.
    let quarantined = unique();
    let mut suspect = sale(tenant, terminal, quarantined, Some("T1-000800"));
    suspect.quarantine = Some(QuarantineReason::TotalsMismatch {
        stored_minor: 1,
        recomputed_minor: 49_450,
    });
    repo.store_sale(suspect).await.unwrap();

    let app = router(AppState::new(repo));

    // The owner edits a price, and a till pulls it.
    let item_id = unique();
    let (status, edit) = call::<_, CatalogueEditResponse>(
        &app,
        "/v1/back-office/catalogue/upsert",
        &UpsertItemRequest {
            protocol: PROTOCOL_VERSION,
            tenant,
            terminal,
            item: item(item_id, 51_000),
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(edit.unwrap().cursor > 0);

    let (status, page) = call::<_, PullResponse>(
        &app,
        "/v1/sync/pull",
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant,
            terminal,
            cursor: 0,
            limit: 10,
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page = page.unwrap();
    assert_eq!(page.upserts.len(), 1);
    assert_eq!(page.upserts[0].price_minor, 51_000);

    // The repair queue shows the sale, and resolving it empties the queue.
    let queue_request = RepairQueueRequest {
        protocol: PROTOCOL_VERSION,
        tenant,
        terminal,
        limit: 50,
    };
    let (status, queue) =
        call::<_, RepairQueueResponse>(&app, "/v1/back-office/repairs", &queue_request, &token)
            .await;
    assert_eq!(status, StatusCode::OK);
    let queue = queue.unwrap();
    assert_eq!(queue.entries.len(), 1);
    assert_eq!(queue.entries[0].id, quarantined);
    assert!(queue.entries[0].reason.contains("49450"));

    let (status, resolved) = call::<_, ResolveRepairResponse>(
        &app,
        "/v1/back-office/repairs/resolve",
        &ResolveRepairRequest {
            protocol: PROTOCOL_VERSION,
            tenant,
            terminal,
            sale: quarantined,
            note: "till was restored from a backup, receipt reissued".to_owned(),
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(resolved.unwrap().resolved);

    let (_, queue) =
        call::<_, RepairQueueResponse>(&app, "/v1/back-office/repairs", &queue_request, &token)
            .await;
    assert!(queue.unwrap().entries.is_empty());

    // And the health list knows the till synced, because it just pulled.
    let (status, health) = call::<_, TerminalHealthResponse>(
        &app,
        "/v1/back-office/terminals",
        &TerminalHealthRequest {
            protocol: PROTOCOL_VERSION,
            tenant,
            terminal,
        },
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let health = health.unwrap();
    assert_eq!(health.terminals.len(), 1);
    assert_eq!(health.terminals[0].label, "Counter by the door");
    assert_eq!(health.terminals[0].sales, 1);
    assert_eq!(health.terminals[0].open_repairs, 0, "the queue was worked");
    assert!(
        health.terminals[0].last_seen_ms.is_some(),
        "a till that pulled must show as heard from"
    );
}
