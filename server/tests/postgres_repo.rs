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
use openpos_server::auth::{Caller, EnrolmentCode, TokenHash};
use openpos_server::pg::PgRepo;
use openpos_server::repo::{RepoError, Repository, StoredSale};

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
    let caller = Caller { tenant, terminal };

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
        .revoke_all_tokens(Caller { tenant, terminal })
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
