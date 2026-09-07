//! Taking a shop out of one install and putting it into another, against a real
//! Postgres.
//!
//! The in-memory tests in `export.rs` prove the shape of the bundle and the file
//! format. These prove the part only a database can: that the writes are
//! idempotent under real primary keys, that row-level security still holds while
//! a whole shop is being written, and that an import does not disturb the shop
//! next to it.
//!
//! Skipped unless the database URLs are set, so the everyday suite stays fast
//! and needs no infrastructure. Run them with:
//!
//! ```text
//! docker compose up -d
//! OPENPOS_TEST_ADMIN_DATABASE_URL=postgres://postgres:postgres@localhost:5433/openpos \
//! OPENPOS_TEST_DATABASE_URL=postgres://openpos_app:openpos_app@localhost:5433/openpos \
//!     cargo test -p openpos-server --test export_import
//! ```
//!
//! The application URL must name a role that is **not** a superuser, for the
//! same reason as the other database tests: a superuser bypasses row-level
//! security, and these would then pass while proving nothing.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::protocol::{ItemWire, QuarantineReason};
use openpos_server::export::{
    ExportBundle, ExportError, IdentityPolicy, export_tenant, import_tenant, stream_tenant,
};
use openpos_server::pg::PgRepo;
use openpos_server::repo::{AccountCharge, AccountPayment, Repository, Settlement, StoredSale};

/// A fresh identifier, unique within and across runs. Copied in spirit from
/// `postgres_repo.rs`: the clock alone hands two shops the same id, because its
/// granularity is coarser than the time it takes to read it.
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

/// The one test in this file that runs without a database, and fails without
/// one.
///
/// Everything else here returns early when the two URLs are unset, and a test
/// that returns early passes. So a run with no database was a green suite that
/// had tested nothing about Postgres, and every total quoted in todo.md before
/// the sixth of September counted forty eight tests that never ran.
///
/// A skip that reports itself as a pass is worse than a failure: it is a
/// failure nobody will look for. This one says what to set and what would have
/// run, so `cargo test --workspace` on a machine with no database is one
/// obvious failure rather than a false all-clear.
#[test]
fn these_tests_need_a_database_and_say_so_when_they_have_none() {
    let admin = std::env::var("OPENPOS_TEST_ADMIN_DATABASE_URL").ok();
    let app = std::env::var("OPENPOS_TEST_DATABASE_URL").ok();
    assert!(
        admin.is_some() && app.is_some(),
        "the Postgres tests in this file did not run. Start the database with `docker compose up \
         -d db` and set OPENPOS_TEST_ADMIN_DATABASE_URL and OPENPOS_TEST_DATABASE_URL as \
         docs/running.md gives them. Everything else in this file returns early without them, \
         which the harness reports as a pass."
    );
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
        from_a_till: false,
        supply: 0,
        category: String::new(),
    }
}

fn sale(tenant: u128, terminal: u128, id: u128, item_id: u128, receipt: &str) -> StoredSale {
    StoredSale {
        tenant,
        terminal,
        id,
        receipt_no: Some(receipt.to_owned()),
        receipt_epoch: Some(1),
        rung_at_ms: 1_788_600_000_000,
        total_minor: 49_450,
        payload: vec![1, 2, 3, 4],
        quarantine: None,
        stock: vec![(item_id, -1_000)],
        vat: Vec::new(),
        overrides: Vec::new(),
        on_account: Vec::new(),
        refund_of: None,
        cash_minor: 49_450,
        cost_minor: 0,
        cost_known: false,
    }
}

/// A shop with a day behind it: two terminals, a catalogue with a deletion in
/// it, three sales and one of them in the repair queue.
async fn shop(repo: &PgRepo) -> (u128, u128, u128) {
    let (tenant, counter, kiosk) = (unique(), unique(), unique());
    repo.enrol(tenant, counter, "Karim General Store")
        .await
        .unwrap();
    repo.enrol(tenant, kiosk, "Karim General Store")
        .await
        .unwrap();

    let rice = unique();
    let oil = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();
    repo.upsert_item(tenant, &item(oil, 47_500)).await.unwrap();
    repo.delete_item(tenant, oil).await.unwrap();

    repo.store_sale(sale(tenant, counter, unique(), rice, "T1-000100"))
        .await
        .unwrap();
    repo.store_sale(sale(tenant, kiosk, unique(), rice, "T2-000100"))
        .await
        .unwrap();

    let mut suspect = sale(tenant, counter, unique(), rice, "T1-000100");
    suspect.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000100".to_owned(),
    });
    repo.store_sale(suspect).await.unwrap();

    // Somebody the shop wrote down, and the book that is keyed on them. A shop
    // arriving with its balances and none of the people they belong to has a
    // book of ids nobody can put a face to.
    let karim = unique();
    repo.put_customer(
        tenant,
        &openpos_server::repo::CustomerRecord {
            id: karim,
            name: "Karim, flat 3".to_owned(),
            phone: Some("01711000000".to_owned()),
            active: true,
            // A buyer that is a business, so a restore that lost this would be
            // a shop that cannot write them a tax invoice again.
            bin: Some("009876543-0202".to_owned()),
        },
    )
    .await
    .unwrap();
    let key = openpos_core::accounts::customer_key(karim);

    // He took goods on account and paid half of it, which is the part of a shop
    // that cannot be reconstructed from anything else in the file: a payment is
    // in no sale payload.
    let mut on_account = sale(tenant, counter, unique(), rice, "T1-000101");
    on_account.on_account = vec![AccountCharge {
        person_key: key.clone(),
        person_name: "Karim, flat 3".to_owned(),
        amount_minor: 29_450,
    }];
    repo.store_sale(on_account).await.unwrap();
    repo.take_payment(
        tenant,
        &AccountPayment {
            id: unique(),
            kind: Settlement::Paid,
            person_key: key.clone(),
            person_name: "Karim, flat 3".to_owned(),
            amount_minor: 10_000,
            at_ms: 1_788_900_000_000,
            note: Some("in cash".to_owned()),
        },
    )
    .await
    .unwrap();

    // And the evening was counted. A shop that moves machine and arrives unable
    // to say a single drawer was ever reconciled has lost its accountability
    // record, which is the whole reason a drawer is counted by one person and
    // read by another.
    repo.put_shifts(
        tenant,
        &[openpos_server::repo::ClosedShift {
            id: unique(),
            terminal: counter,
            closed_by: unique(),
            closed_by_name: "Rahima".to_owned(),
            opened_at_ms: 1_788_600_000_000,
            closed_at_ms: 1_788_640_000_000,
            opening_float_minor: 50_000,
            sales: 3,
            cash_sales_minor: 148_350,
            non_cash_sales_minor: 0,
            cash_in_minor: 0,
            cash_out_minor: 0,
            expected_cash_minor: 198_350,
            counted_cash_minor: 194_350,
            variance_minor: -4_000,
        }],
    )
    .await
    .unwrap();

    // Somebody who may stand at a till, and somebody the shop buys from. A shop
    // that arrives with neither cannot sell and cannot say where its goods came
    // from.
    repo.put_operator(
        tenant,
        &openpos_server::repo::OperatorRecord {
            id: unique(),
            name: "Rahima".to_owned(),
            pin_salt: vec![1, 2, 3, 4],
            pin_rounds: 100_000,
            pin_key: vec![9; 32],
            max_discount_bp: 500,
            may_override_price: false,
            may_refund: true,
            may_void_line: true,
            may_authorise: false,
            may_open_drawer: true,
            may_close_shift: true,
            active: true,
        },
    )
    .await
    .unwrap();
    // Goods in on credit, and half of it paid. This is the part of a shop that
    // no sale and no movement can rebuild: what it owes the people who supply
    // it.
    let distributor = unique();
    repo.put_supplier(
        tenant,
        &openpos_server::repo::Supplier {
            id: distributor,
            name: "Mirpur Distributors".to_owned(),
            phone: None,
            bin: None,
            active: true,
        },
    )
    .await
    .unwrap();
    repo.receive_goods(
        tenant,
        &openpos_server::repo::GoodsReceipt {
            id: unique(),
            supplier_id: Some(distributor),
            reference: Some("CH-1".to_owned()),
            received_at_ms: 1_788_500_000_000,
            received_by: unique(),
            note: Some("forty bags".to_owned()),
            lines: vec![openpos_server::repo::ReceiptLine {
                item_id: rice,
                qty_milli: 40_000,
                unit_cost_minor: 34_400,
            }],
        },
    )
    .await
    .unwrap();
    repo.pay_supplier(
        tenant,
        &openpos_server::repo::SupplierPayment {
            id: unique(),
            supplier_id: distributor,
            amount_minor: 500_000,
            paid_at_ms: 1_788_900_000_000,
            note: Some("in cash, Saturday".to_owned()),
        },
    )
    .await
    .unwrap();

    // A shelf counted, which is the barrier every stock figure after it is
    // worked from, and a bag of rice that split on the floor.
    repo.record_count(
        tenant,
        &openpos_server::repo::StockCount {
            id: unique(),
            item_id: rice,
            counted_milli: 31_000,
            counted_at_ms: 1_788_700_000_000,
            counted_by: unique(),
            note: Some("Friday morning".to_owned()),
        },
    )
    .await
    .unwrap();
    repo.correct_stock(
        tenant,
        &openpos_server::repo::StockCorrection {
            id: unique(),
            item_id: rice,
            qty_milli: -1_000,
            reason: "a bag split on the floor".to_owned(),
            occurred_at_ms: 1_788_710_000_000,
            recorded_by: unique(),
        },
    )
    .await
    .unwrap();

    // And a discount a supervisor allowed, which is the record that answers the
    // question asked after a variance.
    repo.put_allowed(
        tenant,
        counter,
        &[openpos_server::repo::AllowedAction {
            terminal: counter,
            seq: 1,
            at_ms: 1_788_600_060_000,
            action: 1,
            bp: 1_000,
            operator: unique(),
            operator_name: "Rahima".to_owned(),
            authorised_by: unique(),
            authorised_by_name: "Karim".to_owned(),
        }],
    )
    .await
    .unwrap();

    // And what it prints at the top of a receipt, which in this country is a
    // tax invoice and needs the BIN on it.
    repo.put_shop_details(
        tenant,
        &openpos_server::repo::ShopDetails {
            name: "Karim General Store".to_owned(),
            bin: Some("000000000-0000".to_owned()),
            address: Some("Mirpur 10, Dhaka".to_owned()),
            phone: Some("01711000000".to_owned()),
            wallets: vec!["bKash".to_owned()],
            // Refuse a basket past the shelf, so a restore that lost this
            // would be a shop that quietly stopped refusing.
            stock_rule: 2,
        },
    )
    .await
    .unwrap();

    // The counter has been selling, so its lease has moved on.
    repo.issue_lease(tenant, counter, 500).await.unwrap();

    (tenant, counter, rice)
}

/// The whole point of the feature: a shop leaves one install and arrives in
/// another, complete, and arriving twice does not double its takings.
#[tokio::test]
async fn a_shop_moves_install_through_a_file_and_arrives_intact() {
    let repo = database!();
    let (tenant, counter, rice) = shop(&repo).await;

    // What the shop is handed on the way out.
    let mut file = Vec::new();
    stream_tenant(&repo, tenant, &mut file).await.unwrap();
    let bundle = ExportBundle::read_jsonl(file.as_slice()).unwrap();
    assert_eq!(bundle.sales.len(), 4);
    assert_eq!(bundle.terminals.len(), 2);
    assert_eq!(bundle.catalogue.len(), 3);
    // Four from sales, one from the delivery and one from the correction: goods
    // in and goods lost move stock too.
    assert_eq!(bundle.movements.len(), 6);
    // The debt and the payment against it. A shop that arrives with its sales
    // and none of what anybody owes it has lost the part it cannot rebuild.
    assert_eq!(bundle.accounts.len(), 2);
    assert_eq!(bundle.shifts.len(), 1);
    assert_eq!(bundle.customers.len(), 1);
    // Without these a restored shop cannot sell and cannot print a tax invoice.
    assert_eq!(bundle.operators.len(), 1);
    assert_eq!(bundle.suppliers.len(), 1);
    // What the shop owes the people who supply it, which no sale and no
    // movement can rebuild.
    assert_eq!(bundle.deliveries.len(), 1);
    assert_eq!(bundle.deliveries[0].lines.len(), 1);
    assert_eq!(bundle.deliveries[0].lines[0].unit_cost_minor, 34_400);
    assert_eq!(bundle.deliveries[0].reference.as_deref(), Some("CH-1"));
    assert_eq!(bundle.supplier_payments.len(), 1);
    assert_eq!(bundle.supplier_payments[0].amount_minor, 500_000);
    // The barrier the shelf figures are worked from, and the reason a bag left
    // without being sold.
    assert_eq!(bundle.counts.len(), 1);
    assert_eq!(bundle.counts[0].counted_milli, 31_000);
    assert_eq!(bundle.corrections.len(), 1);
    assert_eq!(bundle.corrections[0].reason, "a bag split on the floor");
    // And who allowed what, which a shop that moves machine would otherwise be
    // unable to answer about anything before the move.
    assert_eq!(bundle.allowed.len(), 1);
    assert_eq!(bundle.allowed[0].bp, 1_000);
    assert_eq!(bundle.allowed[0].authorised_by_name, "Karim");
    assert_eq!(bundle.shop.bin.as_deref(), Some("000000000-0000"));
    assert_eq!(bundle.shop.wallets, vec!["bKash".to_owned()]);
    // And what does not travel, on purpose: a four-digit PIN behind any number
    // of rounds is a few thousand guesses to whoever holds the file.
    let written = String::from_utf8(file.clone()).unwrap();
    assert!(
        !written.contains("pin"),
        "no PIN material of any kind is in the file"
    );

    // And what arrives at the other end. The install already holds the original,
    // which is why the copy is re-homed rather than restored.
    let policy = IdentityPolicy::mint();
    let outcome = import_tenant(&repo, &bundle, policy).await.unwrap();
    assert_ne!(outcome.tenant, tenant);
    assert_eq!(outcome.sales_added, 4);
    assert_eq!(outcome.catalogue_added, 3);
    assert_eq!(outcome.movements_added, 6);
    assert_eq!(outcome.accounts_added, 2);
    assert_eq!(outcome.shifts_taken, 1);
    assert_eq!(outcome.customers_taken, 1);
    assert_eq!(outcome.operators_taken, 1);
    assert_eq!(outcome.suppliers_taken, 1);
    assert_eq!(outcome.deliveries_taken, 1);
    assert_eq!(outcome.supplier_payments_taken, 1);
    assert_eq!(outcome.counts_taken, 1);
    assert_eq!(outcome.corrections_taken, 1);
    assert_eq!(outcome.allowed_taken, 1);
    let trail = repo
        .allowed(outcome.tenant, 0, 1_799_999_999_999, 50)
        .await
        .unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].operator_name, "Rahima");
    assert_eq!(trail[0].authorised_by_name, "Karim");

    // And the copy's shelf says what the original's says: counted at 31.000,
    // less the bag that split.
    let shelf = repo.on_hand(outcome.tenant, rice).await.unwrap();
    let original = repo.on_hand(tenant, rice).await.unwrap();
    assert_eq!(shelf.qty_milli, original.qty_milli);
    assert_eq!(shelf.counted_at_ms, Some(1_788_700_000_000));
    assert_eq!(
        shelf.qty_milli, 30_000,
        "the count is the barrier, and the split bag came off it"
    );

    // And the copy owes exactly what the original owes: forty bags at 344.00,
    // less the five thousand handed over on Saturday.
    let owing = repo.supplier_owing(outcome.tenant).await.unwrap();
    assert_eq!(owing.len(), 1);
    assert_eq!(owing[0].name, "Mirpur Distributors");
    assert_eq!(
        owing[0].owed_minor,
        // Forty bags at 344.00, less the five thousand handed over.
        1_376_000 - 500_000,
        "the delivery less the payment"
    );

    // The people came back with their permissions and their ids, so the history
    // written against them still names somebody, and with a PIN nobody can
    // type: the shop sets one before they can sign in.
    let people = repo.operators(outcome.tenant).await.unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].name, "Rahima");
    assert!(people[0].may_refund);
    assert_eq!(people[0].max_discount_bp, 500);
    assert_ne!(
        people[0].pin_key,
        vec![9; 32],
        "the PIN that was in the shop is not the PIN that came back"
    );
    // The shop sets a PIN, and then somebody runs the import again because the
    // first attempt looked stuck. That must not lock the shop out of its own
    // tills by writing another unguessable PIN over the one just set.
    let set_by_the_shop = vec![7; 32];
    repo.put_operator(
        outcome.tenant,
        &openpos_server::repo::OperatorRecord {
            pin_key: set_by_the_shop.clone(),
            ..people[0].clone()
        },
    )
    .await
    .unwrap();
    let again = import_tenant(&repo, &bundle, IdentityPolicy::Rehome(outcome.tenant))
        .await
        .unwrap();
    assert_eq!(again.operators_taken, 0, "nobody was written over");

    let printed = repo.shop_details(outcome.tenant).await.unwrap();
    assert_eq!(printed.bin.as_deref(), Some("000000000-0000"));
    assert_eq!(printed.address.as_deref(), Some("Mirpur 10, Dhaka"));
    assert_eq!(
        printed.stock_rule, 2,
        "a restored shop still refuses a basket past the shelf"
    );
    assert_eq!(repo.suppliers(outcome.tenant).await.unwrap().len(), 1);

    let copy = export_tenant(&repo, outcome.tenant).await.unwrap();
    assert_eq!(copy.sales, bundle.sales, "every sale, byte for byte");
    assert_eq!(copy.catalogue, bundle.catalogue, "prices and tombstones");
    assert_eq!(copy.movements, bundle.movements);
    assert_eq!(copy.accounts, bundle.accounts, "the book, entry for entry");
    assert_eq!(
        copy.shifts, bundle.shifts,
        "and every drawer that was counted"
    );
    assert_eq!(copy.shifts[0].closed_by_name, "Rahima");
    assert_eq!(copy.terminals, bundle.terminals);

    // And the balance at the far end is the one the shop left with, rather than
    // a debt rebuilt out of the sales with every payment forgotten.
    let owing = repo.owed(outcome.tenant, None, 50).await.unwrap();
    assert_eq!(owing.len(), 1);
    assert_eq!(owing[0].owed_minor, 19_450);
    assert_eq!(copy.tenant.name, bundle.tenant.name);
    assert_eq!(copy.tenant.catalogue_seq, bundle.tenant.catalogue_seq);
    assert_eq!(copy.tenant.id, outcome.tenant);

    // Run it again, as an operator does when the first attempt looked stuck.
    let again = import_tenant(&repo, &bundle, policy).await.unwrap();
    assert!(!again.changed_anything(), "{again:?}");
    assert_eq!(
        export_tenant(&repo, outcome.tenant).await.unwrap(),
        copy,
        "a second import must leave the shop exactly as it was"
    );

    // And the shop it was copied from is untouched.
    assert_eq!(export_tenant(&repo, tenant).await.unwrap(), bundle);
    assert!(repo.terminal_enrolled(tenant, counter).await.unwrap());
}

/// Restoring a shop over itself, which is what a support restore is.
#[tokio::test]
async fn a_restore_over_a_live_shop_adds_nothing_and_removes_nothing() {
    let repo = database!();
    let (tenant, _, _) = shop(&repo).await;
    let bundle = export_tenant(&repo, tenant).await.unwrap();

    let outcome = import_tenant(&repo, &bundle, IdentityPolicy::Preserve)
        .await
        .unwrap();

    assert_eq!(outcome.tenant, tenant, "a restore keeps the shop's own id");
    assert!(!outcome.changed_anything(), "{outcome:?}");
    assert_eq!(export_tenant(&repo, tenant).await.unwrap(), bundle);
}

/// A shop is restored from a backup taken before this morning's trading. The
/// backup must not roll the receipt counter back onto numbers already printed,
/// and must not renumber the catalogue under a till that is mid-pull.
#[tokio::test]
async fn an_older_backup_never_rewinds_a_counter() {
    let repo = database!();
    let (tenant, counter, _) = shop(&repo).await;
    let backup = export_tenant(&repo, tenant).await.unwrap();

    // The shop carries on trading after the backup was taken.
    let sold = repo.issue_lease(tenant, counter, 500).await.unwrap();
    let new_item = unique();
    let seq_before = repo
        .upsert_item(tenant, &item(new_item, 51_000))
        .await
        .unwrap();

    let _ = import_tenant(&repo, &backup, IdentityPolicy::Preserve)
        .await
        .unwrap();

    let next = repo.issue_lease(tenant, counter, 10).await.unwrap();
    assert_eq!(
        next.first,
        sold.last + 1,
        "a restore must not hand back a receipt number already printed"
    );

    let seq_after = repo
        .upsert_item(tenant, &item(unique(), 52_000))
        .await
        .unwrap();
    assert!(
        seq_after > seq_before,
        "the catalogue sequence must move forward, not back onto imported rows"
    );

    // The change made after the backup is still there and still where it was, so
    // a till holding a cursor is not asked to re-pull the shop.
    let page = repo.items_since(tenant, seq_before - 1, 100).await.unwrap();
    assert!(page.upserts.iter().any(|upsert| upsert.id == new_item));
}

/// Writing a whole shop is still a tenant-scoped operation.
#[tokio::test]
async fn importing_one_shop_does_not_disturb_the_shop_next_to_it() {
    let repo = database!();
    let (donor, _, _) = shop(&repo).await;
    let (neighbour, _, _) = shop(&repo).await;
    let before = export_tenant(&repo, neighbour).await.unwrap();

    let bundle = export_tenant(&repo, donor).await.unwrap();
    let outcome = import_tenant(&repo, &bundle, IdentityPolicy::mint())
        .await
        .unwrap();

    assert_eq!(
        export_tenant(&repo, neighbour).await.unwrap(),
        before,
        "an import must not reach into another shop"
    );

    // And the copy is invisible from anywhere else, which is the row-level
    // security policy doing its job on rows that were written in bulk.
    for sale in &bundle.sales {
        assert!(repo.has_sale(outcome.tenant, sale.id).await.unwrap());
        assert!(
            !repo.has_sale(neighbour, sale.id).await.unwrap(),
            "row-level security did not isolate the imported rows. Check that the \
             connecting role is not a superuser"
        );
    }
}

/// An export names a shop that has to exist, so a mistyped id is an error rather
/// than a plausible-looking empty file.
#[tokio::test]
async fn exporting_a_shop_that_was_never_created_is_refused() {
    let repo = database!();
    assert_eq!(
        export_tenant(&repo, unique()).await,
        Err(ExportError::UnknownTenant)
    );
}

/// The file a shop is handed must not also be a way into it.
#[tokio::test]
async fn an_export_file_contains_no_credential() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    let token = repo
        .enrol_with_token(tenant, terminal, "Karim General Store")
        .await
        .unwrap();
    repo.store_sale(sale(tenant, terminal, unique(), unique(), "T1-000100"))
        .await
        .unwrap();

    let mut file = Vec::new();
    stream_tenant(&repo, tenant, &mut file).await.unwrap();
    let text = String::from_utf8(file).unwrap();

    assert!(!text.contains(token.as_str()), "an export is not a key");
    assert!(!text.contains("token"));

    // The copy therefore has no way in until its devices are enrolled again,
    // which is the trade being made.
    let bundle = ExportBundle::read_jsonl(text.as_bytes()).unwrap();
    let outcome = import_tenant(&repo, &bundle, IdentityPolicy::mint())
        .await
        .unwrap();
    assert_eq!(outcome.terminals, 1);
    assert!(
        repo.terminal_enrolled(outcome.tenant, terminal)
            .await
            .unwrap()
    );
}

/// A shop trading while its own backup is being taken.
#[tokio::test]
async fn a_sale_that_lands_mid_export_is_left_out_whole_rather_than_half_in() {
    let repo = database!();
    let (tenant, counter, rice) = shop(&repo).await;

    // The cut this export describes, taken as the drain takes it.
    let cut = repo.now_ms().await.unwrap();
    // And a wait for the database's own clock to pass it. The cut includes what
    // arrived at the cut, which is right: a sale stored in that millisecond is
    // at the cut, not after it. Without this pause the sale below can land in
    // the same millisecond on a fast machine, and the test fails for being
    // ambiguous rather than for being wrong.
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        if repo.now_ms().await.unwrap() > cut {
            break;
        }
    }

    // A till syncs a moment later, which is what a shop does all day. Its sale
    // carries stock movements and, because it was on account, an entry in the
    // book: three tables, and an export that caught some of them would restore
    // a shop with stock that moved for no reason and a debt with no sale.
    let late = unique();
    let mut arriving = sale(tenant, counter, late, rice, "T1-000900");
    arriving.on_account = vec![AccountCharge {
        person_key: "karim, flat 3".to_owned(),
        person_name: "Karim, flat 3".to_owned(),
        amount_minor: 10_000,
    }];
    repo.store_sale(arriving).await.unwrap();

    // Read as the drain reads, at the cut. None of the three tables has it.
    let sales = repo.sales_after(tenant, 0, cut, 500).await.unwrap();
    assert!(
        !sales.iter().any(|one| one.id == late),
        "a sale that arrived after the cut is not in this export"
    );
    let movements = repo.stock_after(tenant, (0, 0), cut, 500).await.unwrap();
    assert!(
        !movements.iter().any(|one| one.source == late),
        "and neither are its movements"
    );
    let entries = repo
        .account_after(tenant, (0, String::new()), cut, 500)
        .await
        .unwrap();
    assert!(
        !entries.iter().any(|one| one.source == late),
        "nor what it put on somebody's account"
    );

    // And the next export has all of it, which is what makes the miss harmless:
    // running again converges rather than leaving a hole.
    let after = repo.now_ms().await.unwrap();
    assert!(
        repo.sales_after(tenant, 0, after, 500)
            .await
            .unwrap()
            .iter()
            .any(|one| one.id == late)
    );
    assert!(
        repo.stock_after(tenant, (0, 0), after, 500)
            .await
            .unwrap()
            .iter()
            .any(|one| one.source == late)
    );
}
