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
    export_tenant, import_tenant, stream_tenant, ExportBundle, ExportError, IdentityPolicy,
};
use openpos_server::pg::PgRepo;
use openpos_server::repo::{Repository, StoredSale};

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
        barcodes: vec![format!("869{:010}", id % 1_000_000)],
        on_hand_milli: 40_000,
        active: true,
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
    }
}

/// A shop with a day behind it: two terminals, a catalogue with a deletion in
/// it, three sales and one of them in the repair queue.
async fn shop(repo: &PgRepo) -> (u128, u128, u128) {
    let (tenant, counter, kiosk) = (unique(), unique(), unique());
    repo.enrol(tenant, counter, "Karim General Store").await.unwrap();
    repo.enrol(tenant, kiosk, "Karim General Store").await.unwrap();

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

    // The counter has been selling, so its lease has moved on.
    repo.issue_lease(tenant, counter, 500).await.unwrap();

    (tenant, counter, rice)
}

/// The whole point of the feature: a shop leaves one install and arrives in
/// another, complete, and arriving twice does not double its takings.
#[tokio::test]
async fn a_shop_moves_install_through_a_file_and_arrives_intact() {
    let repo = database!();
    let (tenant, counter, _) = shop(&repo).await;

    // What the shop is handed on the way out.
    let mut file = Vec::new();
    stream_tenant(&repo, tenant, &mut file).await.unwrap();
    let bundle = ExportBundle::read_jsonl(file.as_slice()).unwrap();
    assert_eq!(bundle.sales.len(), 3);
    assert_eq!(bundle.terminals.len(), 2);
    assert_eq!(bundle.catalogue.len(), 3);
    assert_eq!(bundle.movements.len(), 3);

    // And what arrives at the other end. The install already holds the original,
    // which is why the copy is re-homed rather than restored.
    let policy = IdentityPolicy::mint();
    let outcome = import_tenant(&repo, &bundle, policy).await.unwrap();
    assert_ne!(outcome.tenant, tenant);
    assert_eq!(outcome.sales_added, 3);
    assert_eq!(outcome.catalogue_added, 3);
    assert_eq!(outcome.movements_added, 3);

    let copy = export_tenant(&repo, outcome.tenant).await.unwrap();
    assert_eq!(copy.sales, bundle.sales, "every sale, byte for byte");
    assert_eq!(copy.catalogue, bundle.catalogue, "prices and tombstones");
    assert_eq!(copy.movements, bundle.movements);
    assert_eq!(copy.terminals, bundle.terminals);
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
    let seq_before = repo.upsert_item(tenant, &item(new_item, 51_000)).await.unwrap();

    let _ = import_tenant(&repo, &backup, IdentityPolicy::Preserve)
        .await
        .unwrap();

    let next = repo.issue_lease(tenant, counter, 10).await.unwrap();
    assert_eq!(
        next.first,
        sold.last + 1,
        "a restore must not hand back a receipt number already printed"
    );

    let seq_after = repo.upsert_item(tenant, &item(unique(), 52_000)).await.unwrap();
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
    assert!(repo.terminal_enrolled(outcome.tenant, terminal).await.unwrap());
}
