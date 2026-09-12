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
    AccountCharge, AccountPayment, Admission, AllowedAction, CatalogueRecord, ClosedShift,
    CustomerRecord, Decided, GoodsReceipt, OpenDrawer, OperatorRecord, ReceiptLine, RepoError,
    Repository, SaleRecord, Settlement, ShopDetails, StockCorrection, StockCount, StoredSale,
    Supplier, SupplierPayment,
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
        vat: Vec::new(),
        overrides: Vec::new(),
        on_account: Vec::new(),
        refund_of: None,
        cash_minor: 49_450,
        cost_minor: 0,
        cost_known: false,
    }
}

/// What the shop's own sales say a till took in cash while a drawer was open.
///
/// The other half of a counted drawer: until this existed, the expectation a
/// variance is measured against was the till's word for itself. This is the
/// same figure worked out from the sales the shop holds, over the drawer's own
/// window, with struck out sales left out because a sale that never happened
/// put nothing in the drawer.
#[tokio::test]
async fn what_a_drawer_took_is_answered_from_the_shops_own_sales() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Nothing rung yet, which is a drawer holding only its float.
    assert_eq!(
        repo.drawer_takings(tenant, terminal, 0, u64::MAX)
            .await
            .unwrap(),
        Some(0)
    );

    repo.admit_sale(sale(tenant, terminal, unique(), Some("T1-000100")))
        .await
        .unwrap();
    repo.admit_sale(sale(tenant, terminal, unique(), Some("T1-000101")))
        .await
        .unwrap();

    // A card sale: real money, and not in the drawer.
    let mut by_card = sale(tenant, terminal, unique(), Some("T1-000102"));
    by_card.cash_minor = 0;
    repo.admit_sale(by_card).await.unwrap();

    // One rung before the drawer was opened, on the same till.
    let mut earlier = sale(tenant, terminal, unique(), Some("T1-000103"));
    earlier.rung_at_ms = 1_788_500_000_000;
    repo.admit_sale(earlier).await.unwrap();

    // And one the shop held and struck out.
    let mut struck = sale(tenant, terminal, unique(), Some("T1-000104"));
    let struck_id = struck.id;
    struck.quarantine = Some(openpos_core::protocol::QuarantineReason::CarriedIn);
    repo.admit_sale(struck).await.unwrap();
    repo.resolve_quarantine(tenant, struck_id, "rung twice by mistake", false)
        .await
        .unwrap();

    // Another till's takings are not this drawer's.
    let other = unique();
    repo.enrol(tenant, other, "Test Shop").await.unwrap();
    repo.admit_sale(sale(tenant, other, unique(), Some("T2-000100")))
        .await
        .unwrap();

    assert_eq!(
        repo.drawer_takings(tenant, terminal, 1_788_590_000_000, 1_788_640_000_000)
            .await
            .unwrap(),
        Some(49_450 * 2),
        "two cash sales in the window, and nothing else"
    );

    // The other half of the same window: what the sum above left out, which is
    // what a person needs to read the gap between this figure and the till's.
    assert_eq!(
        repo.struck_out_takings(tenant, terminal, 1_788_590_000_000, 1_788_640_000_000)
            .await
            .unwrap(),
        Some(49_450),
        "the one somebody struck out, which the drawer's own figures still hold"
    );

    // And no shop reads another's drawer.
    let stranger = unique();
    assert_eq!(
        repo.drawer_takings(stranger, terminal, 0, u64::MAX)
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        repo.struck_out_takings(stranger, terminal, 0, u64::MAX)
            .await
            .unwrap(),
        Some(0),
        "nor another's strike-outs"
    );
}

/// A drawer holding a sale from before the shop computed this is not answered
/// with the sales it can see.
///
/// Every sale stored before the column existed carries nothing, and reading
/// that as an empty drawer would report every evening in the shop's history as
/// disagreeing with its own till. The shop says it cannot answer instead, which
/// is what is true about it.
#[tokio::test]
async fn a_drawer_from_before_this_existed_is_not_answered_low() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    repo.admit_sale(sale(tenant, terminal, unique(), Some("T1-000100")))
        .await
        .unwrap();
    let older = sale(tenant, terminal, unique(), Some("T1-000101"));
    let older_id = older.id;
    repo.admit_sale(older).await.unwrap();

    // What a row stored by the build before this one looks like: the sale is
    // whole and the figure was never worked out.
    // Through the migration role, because the application role reads and writes
    // sales only inside a transaction that has said which shop it is.
    let admin = std::env::var("OPENPOS_TEST_ADMIN_DATABASE_URL").expect("the macro checked it");
    let pool = sqlx::postgres::PgPool::connect(&admin)
        .await
        .expect("the migration role connects");
    let scrubbed = sqlx::query("update sale set cash_minor = null where id = $1")
        .bind(uuid::Uuid::from_u128(older_id))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(scrubbed.rows_affected(), 1, "the older sale was made older");

    assert_eq!(
        repo.drawer_takings(tenant, terminal, 0, u64::MAX)
            .await
            .unwrap(),
        None,
        "one sale nobody worked out makes the whole answer a guess"
    );
}

/// What a period made, with the part the shop cannot answer for kept apart.
#[tokio::test]
async fn what_a_period_made_leaves_out_what_it_cannot_answer_for() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let day = 1_788_600_000_000_u64;

    // A sale the shop knows the cost of: 430 net against 380 paid.
    let mut costed = sale(tenant, terminal, unique(), Some("T1-000600"));
    costed.rung_at_ms = day;
    costed.cost_minor = 38_000;
    costed.cost_known = true;
    costed.vat = vec![(1_500, 43_000, 6_450, 0)];
    repo.admit_sale(costed).await.unwrap();

    // One the shop has never said what it paid for. Not counted as free, and
    // not folded into the figure: reported beside it.
    let mut guessed = sale(tenant, terminal, unique(), Some("T1-000601"));
    guessed.rung_at_ms = day + 1_000;
    guessed.cost_minor = 0;
    guessed.cost_known = false;
    guessed.vat = vec![(1_500, 20_000, 3_000, 0)];
    repo.admit_sale(guessed).await.unwrap();

    // And one somebody struck out, which made nothing because it never was.
    let mut struck = sale(tenant, terminal, unique(), Some("T1-000602"));
    let struck_id = struck.id;
    struck.rung_at_ms = day + 2_000;
    struck.cost_minor = 10_000;
    struck.cost_known = true;
    struck.vat = vec![(1_500, 30_000, 4_500, 0)];
    struck.quarantine = Some(openpos_core::protocol::QuarantineReason::CarriedIn);
    repo.admit_sale(struck).await.unwrap();
    repo.resolve_quarantine(tenant, struck_id, "rung twice by mistake", false)
        .await
        .unwrap();

    let made = repo.made(tenant, day - 1_000, day + 10_000).await.unwrap();
    assert_eq!(made.sales, 1, "one sale the shop can answer for");
    assert_eq!(
        made.net_minor, 43_000,
        "before tax, which was never its money"
    );
    assert_eq!(made.cost_minor, 38_000);
    assert_eq!(made.made_minor, 5_000, "fifty taka on the sack");
    assert_eq!(made.sales_without_cost, 1);
    assert_eq!(
        made.net_without_cost_minor, 20_000,
        "and how much of the period the figure does not cover"
    );

    // A day the shop did not trade made nothing, and says so as nothing rather
    // than as an error.
    let quiet = repo
        .made(tenant, day - 90_000_000, day - 80_000_000)
        .await
        .unwrap();
    assert_eq!(quiet.sales, 0);
    assert_eq!(quiet.made_minor, 0);

    // And no shop reads another's margin.
    let stranger = repo
        .made(unique(), day - 1_000, day + 10_000)
        .await
        .unwrap();
    assert_eq!(stranger.sales, 0);
    assert_eq!(stranger.net_minor, 0);
}

/// What the shop holds under one receipt number, for the person at the counter.
///
/// Both sales when two carry one number, because that is the case somebody
/// comes in about, and the money given back against it so the answer is not
/// only "you were charged".
#[tokio::test]
async fn what_is_held_under_one_receipt_number_is_all_of_it() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let sold = sale(tenant, terminal, unique(), Some("T1-000400"));
    let sold_id = sold.id;
    repo.admit_sale(sold).await.unwrap();

    // The same number a second time, which is what a till restored from a
    // backup does and what the repair queue exists for.
    let mut again = sale(tenant, terminal, unique(), Some("T1-000400"));
    again.rung_at_ms += 1_000;
    again.quarantine = Some(
        openpos_core::protocol::QuarantineReason::DuplicateReceiptNumber {
            receipt_no: "T1-000400".to_owned(),
        },
    );
    let again_id = again.id;
    repo.admit_sale(again).await.unwrap();

    // And half of it given back later, under its own number.
    let mut back = sale(tenant, terminal, unique(), Some("T1-000401"));
    back.rung_at_ms += 2_000;
    back.total_minor = -24_725;
    back.refund_of = Some("T1-000400".to_owned());
    repo.admit_sale(back).await.unwrap();

    let found = repo.sales_on_receipt(tenant, "T1-000400").await.unwrap();
    assert_eq!(found.len(), 2, "both of them, oldest first");
    assert_eq!(found[0].id, sold_id);
    assert_eq!(found[1].id, again_id);
    assert!(found[0].held_for.is_none(), "the first was taken");
    assert!(
        found[1]
            .held_for
            .as_deref()
            .is_some_and(|words| words.contains("receipt")),
        "and the second was held, in words: {:?}",
        found[1].held_for
    );
    assert_eq!(
        found[0].refunded_minor, 24_725,
        "what has come back against the number, as money the shop gave"
    );
    assert!(!found[0].payload.is_empty(), "the bytes the till committed");

    // The refund itself is looked up by its own number, and nothing has come
    // back against it: it is the coming back.
    let refund = repo.sales_on_receipt(tenant, "T1-000401").await.unwrap();
    assert_eq!(refund.len(), 1);
    assert_eq!(refund[0].refund_of.as_deref(), Some("T1-000400"));
    assert_eq!(refund[0].refunded_minor, 0);

    // A number this shop does not hold is nothing, not an error.
    assert!(
        repo.sales_on_receipt(tenant, "T1-999999")
            .await
            .unwrap()
            .is_empty()
    );
    // And no shop reads another's counter.
    assert!(
        repo.sales_on_receipt(unique(), "T1-000400")
            .await
            .unwrap()
            .is_empty()
    );
}

/// What a receipt was rung for, and what has been given back against it.
///
/// The question a refund has to be answered with, and the two repositories have
/// to answer it the same way: the memory one is what the ingest tests run on,
/// and this is what a shop runs on.
#[tokio::test]
async fn what_has_been_refunded_against_a_receipt_is_answered_the_same_way() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Nothing carries that number yet, which is not a wrong on its own: a till
    // may simply not have synced.
    assert_eq!(
        repo.refunded_against(tenant, "T1-000100").await.unwrap(),
        None
    );

    let sold = sale(tenant, terminal, unique(), Some("T1-000100"));
    repo.admit_sale(sold).await.unwrap();
    assert_eq!(
        repo.refunded_against(tenant, "T1-000100").await.unwrap(),
        Some((49_450, 0)),
        "rung for its total, and nothing given back"
    );

    // Half of it back, under a receipt number of its own.
    let mut half = sale(tenant, terminal, unique(), Some("T1-000101"));
    half.total_minor = -24_725;
    half.refund_of = Some("T1-000100".to_owned());
    repo.admit_sale(half).await.unwrap();
    assert_eq!(
        repo.refunded_against(tenant, "T1-000100").await.unwrap(),
        Some((49_450, -24_725))
    );

    // And a second refund the shop held and then struck out, which is the whole
    // shape of this: it arrives beyond what the receipt was rung for, somebody
    // says it never happened, and it stops counting against the receipt.
    let mut struck = sale(tenant, terminal, unique(), Some("T1-000102"));
    let struck_id = struck.id;
    struck.total_minor = -24_725;
    struck.refund_of = Some("T1-000100".to_owned());
    struck.quarantine = Some(
        openpos_core::protocol::QuarantineReason::RefundBeyondTheSale {
            receipt_no: "T1-000100".to_owned(),
            sale_minor: 49_450,
            refunded_minor: 49_450,
        },
    );
    repo.admit_sale(struck).await.unwrap();
    repo.resolve_quarantine(tenant, struck_id, "rung twice by mistake", false)
        .await
        .unwrap();
    assert_eq!(
        repo.refunded_against(tenant, "T1-000100").await.unwrap(),
        Some((49_450, -24_725)),
        "a refund that never happened gives nothing back"
    );
}

/// What one receipt has moved, netted across the sale and its refunds.
///
/// The ledger answers it: a sale's movement is negative and a refund's is
/// positive, so anything above zero came back more than it went out.
#[tokio::test]
async fn what_came_back_against_a_receipt_is_netted_against_what_went_out() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let rice = unique();

    let mut sold = sale(tenant, terminal, unique(), Some("T1-000100"));
    sold.stock = vec![(rice, -2_000)];
    repo.admit_sale(sold).await.unwrap();
    assert_eq!(
        repo.goods_against(tenant, "T1-000100").await.unwrap(),
        vec![(rice, -2_000)],
        "two went out and nothing has come back"
    );

    let mut back = sale(tenant, terminal, unique(), Some("T1-000101"));
    back.total_minor = -24_725;
    back.refund_of = Some("T1-000100".to_owned());
    back.stock = vec![(rice, 1_000)];
    repo.admit_sale(back).await.unwrap();
    assert_eq!(
        repo.goods_against(tenant, "T1-000100").await.unwrap(),
        vec![(rice, -1_000)],
        "one of the two is back"
    );

    // A refund the shop held and struck out moved nothing.
    let mut struck = sale(tenant, terminal, unique(), Some("T1-000102"));
    let struck_id = struck.id;
    struck.total_minor = -24_725;
    struck.refund_of = Some("T1-000100".to_owned());
    struck.stock = vec![(rice, 1_000)];
    struck.quarantine = Some(openpos_core::protocol::QuarantineReason::CarriedIn);
    repo.admit_sale(struck).await.unwrap();
    repo.resolve_quarantine(tenant, struck_id, "it never happened", false)
        .await
        .unwrap();
    assert_eq!(
        repo.goods_against(tenant, "T1-000100").await.unwrap(),
        vec![(rice, -1_000)],
        "and a refund that never happened brought nothing back"
    );
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
    assert!(
        old_expiry < new_expiry,
        "{old_expiry:?} then {new_expiry:?}"
    );
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

/// A credential replaced is a credential of the same kind.
///
/// The insert wrote every column but the role, so the replacement took the
/// column's default, which is a till. An owner renewing came back as a till and
/// lost the back office: eleven months after enrolling, with nothing to connect
/// the two, and the shop's own device refused at its own shop.
///
/// Nothing caught it because the in-memory store keeps the whole caller, so the
/// two stores answered differently and only one of them was asked. That is why
/// this test is here rather than beside the memory store's.
#[tokio::test]
async fn renewing_an_owners_credential_gives_back_an_owners_credential() {
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

    let held = repo.authenticate(&new.hash()).await.unwrap().expect("it works");
    assert_eq!(held.role, Role::Owner, "the back office is still the back office");
    assert_eq!(held.tenant, tenant, "and the same shop");
    assert_eq!(held.terminal, terminal, "and the same device");

    // And a till stays a till: the role is carried, not assumed.
    let (till_tenant, till_terminal) = (unique(), unique());
    repo.enrol(till_tenant, till_terminal, "Test Shop").await.unwrap();
    let at_the_counter = Caller {
        tenant: till_tenant,
        terminal: till_terminal,
        role: Role::Till,
    };
    let first = openpos_server::auth::Token::generate();
    repo.store_token(at_the_counter, &first.hash()).await.unwrap();
    let second = openpos_server::auth::Token::generate();
    repo.renew_token(
        at_the_counter,
        &first.hash(),
        &second.hash(),
        Duration::from_secs(3_600),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.authenticate(&second.hash()).await.unwrap().expect("it works").role,
        Role::Till,
        "and a till does not become an owner by asking for a new credential"
    );
}

/// A device withdrawn while it was asking for a new credential stays withdrawn.
///
/// The old credential is authenticated before the replacement is written, and
/// an owner withdrawing the device in between left the withdrawal undone: the
/// replacement went in anyway and worked. A shopkeeper who has just told the
/// shop that a tablet is lost has been told the opposite of what happened.
#[tokio::test]
async fn a_credential_withdrawn_mid_renewal_cannot_be_replaced() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let caller = Caller {
        tenant,
        terminal,
        role: Role::Till,
    };

    let old = openpos_server::auth::Token::generate();
    repo.store_token(caller, &old.hash()).await.unwrap();
    assert!(repo.revoke_token(&old.hash()).await.unwrap());

    let new = openpos_server::auth::Token::generate();
    let refused = repo
        .renew_token(caller, &old.hash(), &new.hash(), Duration::from_secs(3_600))
        .await;
    assert!(refused.is_err(), "a withdrawn credential is not a credential");
    assert!(
        repo.authenticate(&new.hash()).await.unwrap().is_none(),
        "and nothing it asked for works either"
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

    let redeemed = repo
        .redeem_enrolment_code(&code.hash())
        .await
        .unwrap()
        .unwrap();
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
    assert_eq!(
        repo.token_last_used_for_test(&token.hash()).await.unwrap(),
        None
    );
    repo.authenticate(&token.hash()).await.unwrap();
    assert!(
        repo.token_last_used_for_test(&token.hash())
            .await
            .unwrap()
            .is_some()
    );
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
async fn an_item_that_has_moved_is_known_to_have_moved() {
    // What stands between a deletion and the name behind a shop's own figures.
    // Two implementations answer this, one over Postgres and one in memory, and
    // the http tests exercise the other one: without this the two could disagree
    // and the shop that matters would be the one nobody tested.
    let repo = database!();
    let (tenant, terminal, sold, untouched) = (unique(), unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Nothing has happened to either yet.
    assert!(!repo.item_has_history(tenant, sold).await.unwrap());
    assert!(!repo.item_has_history(tenant, untouched).await.unwrap());

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
                item_id: sold,
                qty_milli: 10_000,
                unit_cost_minor: 38_000,
            }],
        },
    )
    .await
    .unwrap();

    assert!(
        repo.item_has_history(tenant, sold).await.unwrap(),
        "a delivery is something that happened to it"
    );

    // A write-off, which is the other way a shop's record names an item without
    // anybody selling it.
    let written_off = unique();
    assert!(
        repo.correct_stock(
            tenant,
            &StockCorrection {
                id: unique(),
                item_id: written_off,
                qty_milli: -1_000,
                reason: "one broken in the crate".to_owned(),
                occurred_at_ms: 2_000,
                recorded_by: terminal,
            },
        )
        .await
        .unwrap()
    );
    assert!(
        repo.item_has_history(tenant, written_off).await.unwrap(),
        "a write-off is too"
    );

    // And a shelf somebody counted, which says the shop stocks the thing even
    // when nothing has moved.
    let counted = unique();
    repo.record_count(
        tenant,
        &StockCount {
            id: unique(),
            item_id: counted,
            counted_milli: 5_000,
            counted_at_ms: 3_000,
            counted_by: terminal,
            note: None,
        },
    )
    .await
    .unwrap();
    assert!(repo.item_has_history(tenant, counted).await.unwrap());
    assert!(
        !repo.item_has_history(tenant, untouched).await.unwrap(),
        "and it says nothing about the one beside it"
    );

    // A shop cannot see over its own boundary here either, or one shop's
    // trading would keep another shop from tidying its own catalogue.
    let next_door = unique();
    repo.enrol(next_door, unique(), "The Shop Next Door")
        .await
        .unwrap();
    assert!(!repo.item_has_history(next_door, sold).await.unwrap());
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
async fn takings_are_summed_by_the_database_and_bounded_by_the_period() {
    let repo = database!();
    let (tenant, terminal, other_till) = (unique(), unique(), unique());
    repo.enrol(tenant, terminal, "Front counter").await.unwrap();
    repo.enrol(tenant, other_till, "Second counter")
        .await
        .unwrap();

    let day = 1_788_600_000_000_u64;
    let sales = vec![
        (unique(), terminal, day + 1_000, 49_450_i64, None),
        (unique(), terminal, day + 2_000, -49_450, None),
        (
            unique(),
            terminal,
            day + 3_000,
            20_000,
            Some("undecodable".to_owned()),
        ),
        (unique(), other_till, day + 4_000, 12_500, None),
        // Outside the period. A day's figure that quietly includes yesterday is
        // worse than one that is missing.
        (unique(), terminal, day - 100_000, 99_999, None),
    ];
    for (id, till, at_ms, total, quarantine) in sales {
        repo.put_sales(
            tenant,
            &[SaleRecord {
                resolution: None,
                id,
                terminal: till,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: at_ms,
                total_minor: total,
                payload: vec![],
                quarantine,
                quarantine_kind: Vec::new(),
                vat: vec![],
                overrides: Vec::new(),
                refund_of: None,
            }],
        )
        .await
        .unwrap();
    }

    let rows = repo.takings(tenant, day, day + 86_400_000).await.unwrap();
    assert_eq!(rows.len(), 2, "one line per till");

    let front = rows.iter().find(|row| row.terminal == terminal).unwrap();
    assert_eq!(front.sales, 3);
    // A sale, its refund, and a quarantined sale: the first two cancel and the
    // third stands, which is what twenty thousand means here.
    assert_eq!(front.total_minor, 20_000);
    assert_eq!(front.refunds, 1);
    assert_eq!(front.refunded_minor, -49_450);
    // Quarantined sales are in the total: the goods left the shop and the money
    // changed hands, so leaving them out would disagree with the drawer.
    assert_eq!(front.needing_attention, 1);

    let second = rows.iter().find(|row| row.terminal == other_till).unwrap();
    assert_eq!(second.sales, 1);
    assert_eq!(second.total_minor, 12_500);
    assert_eq!(second.needing_attention, 0);

    // Another shop's takings are not this shop's, which is the policy rather
    // than the query, and worth proving is switched on.
    let outsider = unique();
    repo.enrol(outsider, unique(), "Another Shop")
        .await
        .unwrap();
    assert!(
        repo.takings(outsider, day, day + 86_400_000)
            .await
            .unwrap()
            .is_empty()
    );
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

    repo.upsert_item(tenant, &item(unique(), 43_000))
        .await
        .unwrap();
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
    repo.upsert_item(tenant, &item(unique(), 51_000))
        .await
        .unwrap();

    let page = repo.items_since(tenant, 0, 100).await.unwrap();
    assert_eq!(
        page.skipped, 1,
        "the unreadable row is counted, not swallowed"
    );
    assert_eq!(page.upserts.len(), 2, "and the readable ones still arrive");
}

#[tokio::test]
async fn two_connections_racing_for_one_receipt_number_cannot_both_win() {
    let repo = database!();
    let (tenant, first_terminal, second_terminal) = (unique(), unique(), unique());
    repo.enrol(tenant, first_terminal, "Test Shop")
        .await
        .unwrap();
    repo.enrol(tenant, second_terminal, "Test Shop")
        .await
        .unwrap();

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
        queue[0].1.contains("494.50"),
        "the repair queue must say what disagreed, in money a shop reads: {}",
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

/// Two counters in one shop are two numbers, whatever their identifiers are.
///
/// The printed prefix used to be the low sixteen bits of the terminal's
/// identifier in hex, because it has to be short enough to read aloud over the
/// phone. Two terminals sharing those bits printed the same prefix, and their
/// receipt counters are their own, so both printed T7-000001: the clash was
/// caught when the second device synced, which is after a customer is holding
/// the paper, and what the shop had was two sales with one receipt number.
///
/// The identifiers here share their low sixteen bits deliberately. Under the
/// old rule this test is the collision; under the shop's own numbering it
/// cannot be.
#[tokio::test]
async fn two_tills_in_one_shop_are_numbered_apart_even_when_their_ids_collide() {
    let repo = database!();
    let tenant = unique();
    let one = unique() & !0xFFFF | 0x0007;
    let other = (unique() & !0xFFFF | 0x0007).saturating_add(0x1_0000);
    assert_eq!(one & 0xFFFF, other & 0xFFFF, "the ids agree where they used to");
    assert_ne!(one, other);

    repo.enrol(tenant, one, "Test Shop").await.unwrap();
    repo.register_terminal(tenant, other, "The other counter")
        .await
        .unwrap();

    let first = repo.issue_lease(tenant, one, 500).await.unwrap();
    let second = repo.issue_lease(tenant, other, 500).await.unwrap();
    assert_ne!(
        first.counter_no, second.counter_no,
        "two counters in one shop cannot print the same receipt numbers"
    );
    assert!(first.counter_no > 0 && second.counter_no > 0);

    // And the number is the till's own, so it does not move under it: a shop
    // reads a receipt number back weeks later and finds the counter it names.
    let again = repo.issue_lease(tenant, one, 500).await.unwrap();
    assert_eq!(again.counter_no, first.counter_no);
    assert!(again.first > first.last, "and the block still moves on");

    // A restore puts back the numbers the shop had rather than dealing them
    // again: a till whose receipts said counter two goes on saying counter two.
    let held = repo.terminal_records(tenant).await.unwrap();
    assert_eq!(held.len(), 2);
    repo.put_terminals(tenant, &held).await.unwrap();
    let after = repo.terminal_records(tenant).await.unwrap();
    for (before, now) in held.iter().zip(after.iter()) {
        assert_eq!(before.counter_no, now.counter_no, "a restore renumbers nobody");
    }
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
    repo.upsert_item(tenant, &item(first_item, 43_000))
        .await
        .unwrap();
    repo.upsert_item(tenant, &item(second_item, 47_500))
        .await
        .unwrap();
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

    repo.upsert_item(shop_a, &item(unique(), 43_000))
        .await
        .unwrap();

    assert_eq!(
        repo.items_since(shop_a, 0, 100)
            .await
            .unwrap()
            .upserts
            .len(),
        1
    );
    assert!(
        repo.items_since(shop_b, 0, 100)
            .await
            .unwrap()
            .upserts
            .is_empty(),
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
    assert!(
        repo.authenticate(&TokenHash::of("not a real token"))
            .await
            .unwrap()
            .is_none()
    );
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

    let rows: Vec<Vec<u8>> =
        sqlx::query_scalar("select token_hash from terminal_token where tenant_id = $1")
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

/// Cutting a device off takes the way back in with it.
///
/// An owner who revokes a tablet is revoking the tablet. A code issued for it
/// an hour ago is a fresh credential for that same device, and whoever has the
/// paper it was written on, or the tab it was shown in, could have enrolled it
/// again and carried on. The code is spent rather than deleted, for the reason
/// the credential is marked rather than removed: a shop looking into what
/// happened wants to see that it existed and when it stopped working.
/// A PIN stored at a thousand rounds is not a PIN this shop will hold.
///
/// The rounds are chosen on the device that sets the PIN and travel to the shop
/// with the key, so a client that was buggy, old or hostile could have stored
/// one around a hundred times cheaper to search than the shop believes its PINs
/// are: every till would verify against it happily and nothing would say so.
/// The shop is the one place that can refuse it.
#[tokio::test]
async fn a_pin_hashed_too_cheaply_is_refused_by_the_shop() {
    let repo = database!();
    let tenant = unique();
    repo.enrol(tenant, unique(), "Test Shop").await.unwrap();

    let mut person = openpos_server::repo::OperatorRecord {
        id: unique(),
        name: "Rina".to_owned(),
        pin_salt: vec![7; 16],
        pin_rounds: 1_000,
        pin_key: vec![9; 32],
        max_discount_bp: 0,
        may_override_price: false,
        may_refund: false,
        may_void_line: false,
        may_authorise: false,
        may_open_drawer: true,
        may_close_shift: false,
        active: true,
    };
    assert_eq!(
        repo.put_operator(tenant, &person).await,
        Err(RepoError::Invalid),
        "a thousand rounds is a PIN a shop can be talked out of"
    );

    person.pin_rounds = openpos_core::auth::LEAST_PIN_ROUNDS;
    repo.put_operator(tenant, &person).await.unwrap();

    // And the same on the way in when only the PIN is being changed.
    assert_eq!(
        repo.set_operator_pin(tenant, person.id, &[7; 16], 1_000, &[9; 32])
            .await,
        Err(RepoError::Invalid)
    );
    repo.set_operator_pin(
        tenant,
        person.id,
        &[7; 16],
        openpos_core::auth::LEAST_PIN_ROUNDS,
        &[9; 32],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn revoking_a_device_spends_the_codes_that_would_let_it_back_in() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let stays = unique();
    repo.register_terminal(tenant, stays, "The other counter")
        .await
        .unwrap();

    let for_the_revoked = EnrolmentCode::generate();
    repo.issue_enrolment_code(
        Caller { tenant, terminal, role: Role::Till },
        &for_the_revoked.hash(),
        std::time::Duration::from_secs(3_600),
    )
    .await
    .unwrap();

    let for_the_other = EnrolmentCode::generate();
    repo.issue_enrolment_code(
        Caller { tenant, terminal: stays, role: Role::Till },
        &for_the_other.hash(),
        std::time::Duration::from_secs(3_600),
    )
    .await
    .unwrap();

    repo.revoke_all_tokens(Caller { tenant, terminal, role: Role::Till })
        .await
        .unwrap();

    assert!(
        repo.redeem_enrolment_code(&for_the_revoked.hash())
            .await
            .unwrap()
            .is_none(),
        "a code for a device the shop has cut off is not a way back in"
    );
    // And only that device's. Revoking one till must not lock a shop out of the
    // counter it was about to enrol beside it.
    assert!(
        repo.redeem_enrolment_code(&for_the_other.hash())
            .await
            .unwrap()
            .is_some(),
        "the other counter's code is somebody else's business"
    );
}

/// A code that collides with one already alive is refused rather than handed out.
///
/// Astronomically unlikely and unexplainable if it ever happened: the asker
/// would be shown a code that redeems to somebody else's terminal, in somebody
/// else's shop, with somebody else's role. The handler mints another.
#[tokio::test]
async fn a_code_that_is_already_somebody_elses_is_refused() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let (other_shop, other_terminal) = (unique(), unique());
    repo.enrol(other_shop, other_terminal, "The Shop Next Door")
        .await
        .unwrap();

    let code = EnrolmentCode::generate();
    repo.issue_enrolment_code(
        Caller { tenant, terminal, role: Role::Till },
        &code.hash(),
        std::time::Duration::from_secs(3_600),
    )
    .await
    .unwrap();

    assert_eq!(
        repo.issue_enrolment_code(
            Caller { tenant: other_shop, terminal: other_terminal, role: Role::Owner },
            &code.hash(),
            std::time::Duration::from_secs(3_600),
        )
        .await,
        Err(RepoError::Invalid),
        "the second asker must not be given a code that enrols them into the first shop"
    );

    // And the first shop still holds it.
    assert_eq!(
        repo.redeem_enrolment_code(&code.hash()).await.unwrap(),
        Some(Caller { tenant, terminal, role: Role::Till })
    );
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
    assert!(
        repo.redeem_enrolment_code(&code.hash())
            .await
            .unwrap()
            .is_none()
    );

    // An expired code is refused, and is indistinguishable from an unknown one.
    let stale = EnrolmentCode::generate();
    repo.issue_enrolment_code(caller, &stale.hash(), std::time::Duration::from_secs(0))
        .await
        .unwrap();
    assert!(
        repo.redeem_enrolment_code(&stale.hash())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.redeem_enrolment_code(&EnrolmentCode::generate().hash())
            .await
            .unwrap()
            .is_none()
    );
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
    let first = repo
        .enrol_with_token(tenant, terminal, "Counter")
        .await
        .unwrap();
    let second = repo
        .enrol_with_token(tenant, terminal, "Counter")
        .await
        .unwrap();

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
        assert!(
            repo.authenticate(&TokenHash::of(token.as_str()))
                .await
                .unwrap()
                .is_none()
        );
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

    assert!(
        repo.resolve_quarantine(tenant, id, "restored from a backup, receipt reissued", true)
            .await
            .unwrap()
    );
    assert!(repo.repair_queue(tenant, 50).await.unwrap().is_empty());

    // Resolving is not deleting. The sale happened, and the stored bytes are
    // what a dispute is settled against months later.
    assert!(repo.has_sale(tenant, id).await.unwrap());

    // A second person working the same queue is told nothing moved, rather than
    // overwriting the first one's note.
    assert!(
        !repo
            .resolve_quarantine(tenant, id, "second opinion", true)
            .await
            .unwrap()
    );
}

/// The morning the queue exists for: a restored tablet rang the same basket
/// twice, and one of the two did not happen.
#[tokio::test]
async fn a_sale_struck_out_stops_counting_everywhere() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();

    let (real, duplicate) = (unique(), unique());
    for (id, receipt) in [(real, "T1-000500"), (duplicate, "T1-000501")] {
        let mut one = sale(tenant, terminal, id, Some(receipt));
        one.stock = vec![(rice, -2_000)];
        one.vat = vec![(750, 45_998, 3_452, 0)];
        one.on_account = vec![AccountCharge {
            person_key: "karim".to_owned(),
            person_name: "Karim".to_owned(),
            amount_minor: 49_450,
        }];
        if id == duplicate {
            one.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000500".to_owned(),
            });
        }
        repo.store_sale(one).await.unwrap();
    }
    let (from, to) = (1_788_500_000_000, 1_788_700_000_000);
    assert_eq!(repo.balance(tenant, "karim").await.unwrap(), 98_900);
    assert_eq!(repo.on_hand(tenant, rice).await.unwrap().qty_milli, -4_000);

    assert!(
        repo.resolve_quarantine(tenant, duplicate, "rung twice after the restore", false)
            .await
            .unwrap()
    );

    let takings = repo.takings(tenant, from, to).await.unwrap();
    assert_eq!(takings.len(), 1);
    assert_eq!(takings[0].sales, 1, "one sale, not two");
    assert_eq!(takings[0].total_minor, 49_450);
    assert_eq!(
        repo.balance(tenant, "karim").await.unwrap(),
        49_450,
        "Karim owes for one basket of groceries"
    );
    assert_eq!(
        repo.account(tenant, "karim", None, 10).await.unwrap().len(),
        1
    );
    let owed = repo.owed(tenant, None, 10).await.unwrap();
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].owed_minor, 49_450);
    assert_eq!(
        repo.day_summary(tenant, from, to)
            .await
            .unwrap()
            .charged_minor,
        49_450
    );
    let vat = repo.vat_summary(tenant, from, to).await.unwrap();
    assert_eq!(vat.rows.len(), 1);
    assert_eq!(vat.rows[0].vat_minor, 3_452, "tax on what was sold once");
    assert_eq!(vat.rows[0].sales, 1);
    assert_eq!(
        vat.waiting_sales, 0,
        "nothing is waiting on a person any more"
    );
    let sold = repo.sold(tenant, from, to, 10).await.unwrap();
    assert_eq!(sold.len(), 1);
    assert_eq!(sold[0].qty_milli, 2_000);
    assert_eq!(
        repo.on_hand(tenant, rice).await.unwrap().qty_milli,
        -2_000,
        "the shelf only lost one lot of rice"
    );

    // Struck out is not deleted. The bytes are still what a dispute is settled
    // against, and the decision is carried into a bundle so a restore does not
    // put the duplicate back.
    assert!(repo.has_sale(tenant, duplicate).await.unwrap());
    // A cut far enough ahead to hold everything and still be a date Postgres
    // will convert: 2100.
    let carried = repo
        .sales_after(tenant, 0, 4_102_444_800_000, 100)
        .await
        .unwrap();
    let struck = carried
        .iter()
        .find(|record| record.id == duplicate)
        .expect("the struck-out sale is still exported");
    assert_eq!(
        struck.resolution.as_ref().map(|(_, kept)| *kept),
        Some(false)
    );
}

/// Goods brought back by somebody who took them on account come off what they
/// owe. The same field read the same way, because a refund's tender is negative.
#[tokio::test]
async fn a_refund_on_account_reduces_the_debt() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let mut took = sale(tenant, terminal, unique(), Some(&receipt()));
    took.on_account = vec![AccountCharge {
        person_key: "karim".to_owned(),
        person_name: "Karim".to_owned(),
        amount_minor: 29_450,
    }];
    repo.store_sale(took).await.unwrap();

    // Half of it back on Tuesday, rung as a refund on the same account.
    let mut brought_back = sale(tenant, terminal, unique(), Some(&receipt()));
    brought_back.total_minor = -10_000;
    brought_back.stock = vec![];
    brought_back.on_account = vec![AccountCharge {
        person_key: "karim".to_owned(),
        person_name: "Karim".to_owned(),
        amount_minor: -10_000,
    }];
    repo.store_sale(brought_back).await.unwrap();

    assert_eq!(
        repo.balance(tenant, "karim").await.unwrap(),
        19_450,
        "what he took, less what he brought back"
    );
    let owed = repo.owed(tenant, None, 10).await.unwrap();
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].owed_minor, 19_450);
    assert_eq!(
        owed[0].entries, 2,
        "both are in the book, neither is hidden"
    );

    // And the day says both sides of it rather than one net figure: a day where
    // three thousand went on and three thousand came back is not a day where
    // nothing happened.
    let day = repo
        .day_summary(tenant, 1_788_500_000_000, 1_788_700_000_000)
        .await
        .unwrap();
    assert_eq!(day.charged_minor, 29_450, "what went on the book");
    assert_eq!(day.returned_minor, 10_000, "and what came back off it");
}

/// The isolation is row level security, and the connection has to be a role it
/// applies to.
#[tokio::test]
async fn the_connection_cannot_see_past_the_shop_boundary() {
    let repo = database!();
    // What the shipped setup connects as. A role that bypasses the policies has
    // no boundary at all, and the mistake is one word in a connection string
    // that looks exactly like a server that works, so the binary refuses to
    // start on one.
    assert!(
        !repo.can_see_every_shop().await.unwrap(),
        "these tests would prove nothing about isolation on a role that sees every shop"
    );
}

/// The shelf figure only ignores a struck-out sale's own movements.
///
/// Stock moves for three reasons and they share one table. A sale struck out
/// takes its own movements out of the figure; a delivery or a correction is
/// nobody's sale and stays, whatever id it happens to carry. Contrived on
/// purpose: the guard exists for a collision that should never happen, and a
/// guard nothing tests is one the next person tidies away.
#[tokio::test]
async fn striking_out_a_sale_does_not_take_a_correction_with_it() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let (rice, oil) = (unique(), unique());
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();
    repo.upsert_item(tenant, &item(oil, 18_500)).await.unwrap();

    // One id, worn by a sale and by a correction on a different item.
    let shared = unique();
    let mut rung_twice = sale(tenant, terminal, shared, Some(&receipt()));
    rung_twice.stock = vec![(rice, -2_000)];
    rung_twice.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000900".to_owned(),
    });
    repo.store_sale(rung_twice).await.unwrap();
    repo.correct_stock(
        tenant,
        &openpos_server::repo::StockCorrection {
            id: shared,
            item_id: oil,
            qty_milli: -3_000,
            reason: "a bottle broke".to_owned(),
            occurred_at_ms: 1_788_600_000_000,
            recorded_by: unique(),
        },
    )
    .await
    .unwrap();

    repo.resolve_quarantine(tenant, shared, "rung twice after the restore", false)
        .await
        .unwrap();

    assert_eq!(
        repo.on_hand(tenant, rice).await.unwrap().qty_milli,
        0,
        "the sale's own movement goes with it"
    );
    assert_eq!(
        repo.on_hand(tenant, oil).await.unwrap().qty_milli,
        -3_000,
        "and the broken bottle is still broken"
    );

    // And again once the shelf has been counted, because a counted shelf reads
    // through a different join and needs the same guard on it. Counted at ten
    // before the bottle broke, so the figure is ten less the three.
    repo.record_count(
        tenant,
        &openpos_server::repo::StockCount {
            id: unique(),
            item_id: oil,
            counted_milli: 10_000,
            counted_at_ms: 1_788_500_000_000,
            counted_by: unique(),
            note: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo.on_hand(tenant, oil).await.unwrap().qty_milli,
        7_000,
        "the count, less the bottle that broke after it"
    );
}

/// Both stores refuse a correction with nothing said about why, because a store
/// that accepts what the other will not is a store tests pass against and
/// production does not.
#[tokio::test]
async fn a_correction_with_no_reason_is_refused() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();

    let blank = openpos_server::repo::StockCorrection {
        id: unique(),
        item_id: rice,
        qty_milli: -1_000,
        reason: "   ".to_owned(),
        occurred_at_ms: 1_788_600_000_000,
        recorded_by: unique(),
    };
    assert_eq!(
        repo.correct_stock(tenant, &blank).await,
        Err(RepoError::Invalid)
    );
    assert_eq!(
        repo.on_hand(tenant, rice).await.unwrap().qty_milli,
        0,
        "and nothing moved"
    );

    // With a reason it goes through, which is the ordinary case.
    let explained = openpos_server::repo::StockCorrection {
        reason: "a bag split on the floor".to_owned(),
        ..blank
    };
    assert!(repo.correct_stock(tenant, &explained).await.unwrap());
    assert_eq!(repo.on_hand(tenant, rice).await.unwrap().qty_milli, -1_000);
}

/// A barcode belongs to one item, or a scan rings whichever the till finds.
#[tokio::test]
async fn a_barcode_belongs_to_one_item() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let rice = item(unique(), 43_000);
    repo.upsert_item(tenant, &rice).await.unwrap();

    // Another item typed with the same barcode, which is a thumb on a keyboard
    // and not a decision anybody made.
    let mut soap = item(unique(), 9_000);
    soap.barcodes = rice.barcodes.clone();
    let holders = repo.barcode_holders(tenant, &soap.barcodes).await.unwrap();
    assert_eq!(holders.len(), 1);
    assert_eq!(holders[0].1, rice.id, "the rice holds it");

    // Correcting the rice itself is not a clash with itself.
    let its_own = repo.barcode_holders(tenant, &rice.barcodes).await.unwrap();
    assert!(its_own.iter().all(|(_, holder)| *holder == rice.id));

    // A withdrawn item gives its barcode back: a shop that stops selling
    // something can put the code on what replaces it.
    repo.delete_item(tenant, rice.id).await.unwrap();
    assert!(
        repo.barcode_holders(tenant, &rice.barcodes)
            .await
            .unwrap()
            .is_empty()
    );

    // And another shop's barcodes are not this one's.
    assert!(
        repo.barcode_holders(unique(), &rice.barcodes)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The question an inspector asks is why the numbering jumps, and until this
/// the shop had no way to look.
#[tokio::test]
async fn the_shop_can_see_where_its_numbering_jumps() {
    let repo = database!();
    let (tenant, counter) = (unique(), unique());
    let kiosk = unique();
    repo.enrol(tenant, counter, "Test Shop").await.unwrap();
    repo.enrol(tenant, kiosk, "Test Shop").await.unwrap();

    // One till rings 100, 101 and 104: two numbers went with something. The
    // other rings 100 and 101 under its own prefix, which is a different series
    // and not a hole in anybody's numbering.
    for receipt in ["T1-000100", "T1-000101", "T1-000104"] {
        repo.store_sale(sale(tenant, counter, unique(), Some(receipt)))
            .await
            .unwrap();
    }
    for receipt in ["T2-000100", "T2-000101"] {
        repo.store_sale(sale(tenant, kiosk, unique(), Some(receipt)))
            .await
            .unwrap();
    }

    let found = repo.receipt_gaps(tenant, 50).await.unwrap();
    assert_eq!(found.len(), 1, "one gap, on one till");
    assert_eq!(found[0].terminal, counter);
    assert_eq!(found[0].after, "T1-000101");
    assert_eq!(found[0].before, "T1-000104");
    assert_eq!(found[0].missing, 2);

    // The sales that were missing arrive, and the gap closes by itself. That is
    // the ordinary case: a till that had not synced yet.
    for receipt in ["T1-000102", "T1-000103"] {
        repo.store_sale(sale(tenant, counter, unique(), Some(receipt)))
            .await
            .unwrap();
    }
    assert!(repo.receipt_gaps(tenant, 50).await.unwrap().is_empty());

    // And another shop's numbering is not this one's.
    assert!(repo.receipt_gaps(unique(), 50).await.unwrap().is_empty());
}

/// A count is an event: counting again is a new count, not an edit to the last
/// one. Both stores have to agree on that or a shelf figure depends on which
/// one a shop is running.
#[tokio::test]
async fn a_count_sent_twice_keeps_what_arrived_first() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();

    let id = unique();
    let counted = openpos_server::repo::StockCount {
        id,
        item_id: rice,
        counted_milli: 31_000,
        counted_at_ms: 1_788_700_000_000,
        counted_by: unique(),
        note: None,
    };
    repo.record_count(tenant, &counted).await.unwrap();

    // The same count sent again after a dropped reply, which is ordinary, and
    // then the same id carrying a different number, which is not: correcting a
    // count means counting again.
    repo.record_count(tenant, &counted).await.unwrap();
    repo.record_count(
        tenant,
        &openpos_server::repo::StockCount {
            counted_milli: 99_000,
            ..counted.clone()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        repo.on_hand(tenant, rice).await.unwrap().qty_milli,
        31_000,
        "the first answer stands"
    );
}

/// Who allowed what: the record that answers the question asked after a
/// variance, which used to live in a tab's memory and die with it.
#[tokio::test]
async fn saying_the_list_again_writes_one_row_per_item_after_everything_else() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let one = unique();
    let two = unique();
    repo.upsert_item(tenant, &item(one, 43_000)).await.unwrap();
    repo.upsert_item(tenant, &item(two, 21_000)).await.unwrap();
    // Corrected twice, so the count is items and not changes.
    let mut dearer = item(one, 43_000);
    dearer.price_minor = 45_000;
    let before = repo.upsert_item(tenant, &dearer).await.unwrap();

    let sent = repo.resend_catalogue(tenant).await.unwrap();
    assert_eq!(sent, 2, "one row per item, whatever its state");

    // Everything lands after the old cursor, which is what makes a till that
    // had passed those rows receive them.
    let page = repo.items_since(tenant, before, 50).await.unwrap();
    assert_eq!(page.upserts.len(), 2);
    assert_eq!(
        page.upserts.iter().find(|found| found.id == one).map(|found| found.price_minor),
        Some(45_000),
        "at the price the shop holds now"
    );
    assert!(page.cursor > before);

    // And the rows are copied rather than rebuilt: the payload a shop already
    // holds is what goes out, so a row written by a build this one cannot fully
    // read still travels.
    assert_eq!(page.skipped, 0);
}

#[tokio::test]
async fn what_a_till_allowed_reaches_the_shop_and_is_stored_once() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let (cashier, supervisor) = (unique(), unique());

    let discount = AllowedAction {
        terminal,
        seq: 1,
        at_ms: 1_788_600_000_000,
        action: 1,
        bp: 1_000,
        operator: cashier,
        operator_name: "Rahima".to_owned(),
        authorised_by: supervisor,
        authorised_by_name: "Karim".to_owned(),
        // A discount is of no receipt.
        receipt_no: None,
    };
    let drawer = AllowedAction {
        terminal,
        seq: 2,
        at_ms: 1_788_600_100_000,
        action: 5,
        bp: 0,
        operator: cashier,
        operator_name: "Rahima".to_owned(),
        // Nobody had to allow it: their own permission covered it, which is a
        // different fact from a supervisor standing at the counter.
        authorised_by: 0,
        authorised_by_name: String::new(),
        receipt_no: None,
    };
    // A receipt printed a second time, which is the one kind of entry that
    // names one. A second copy is a second piece of paper somebody can hand
    // over, so the shop is told which.
    let reprint = AllowedAction {
        terminal,
        seq: 3,
        at_ms: 1_788_600_150_000,
        action: 14,
        bp: 0,
        operator: cashier,
        operator_name: "Rahima".to_owned(),
        authorised_by: 0,
        authorised_by_name: String::new(),
        receipt_no: Some("T1-000104".to_owned()),
    };

    let stored = repo
        .put_allowed(
            tenant,
            terminal,
            &[discount.clone(), drawer.clone(), reprint.clone()],
        )
        .await
        .unwrap();
    assert_eq!(stored, vec![1, 2, 3]);

    // Sent again, because the reply was dropped. That is ordinary, and it must
    // not rewrite what the shop already holds about who allowed what.
    let mut rewritten = discount.clone();
    rewritten.authorised_by_name = "Somebody Else".to_owned();
    let again = repo
        .put_allowed(tenant, terminal, &[rewritten])
        .await
        .unwrap();
    assert_eq!(again, vec![1], "the till may drop it either way");

    let trail = repo
        .allowed(tenant, 0, 1_799_999_999_999, 50)
        .await
        .unwrap();
    assert_eq!(trail.len(), 3, "stored once, not twice");
    // Newest first: what is being asked about is usually recent.
    assert_eq!(trail[0].action, 14, "the reprint");
    assert_eq!(
        trail[0].receipt_no.as_deref(),
        Some("T1-000104"),
        "which receipt was printed again survives the round trip through the shop's own store, or \
         the trail sends a shop back to lining times up against its sales by hand"
    );
    assert_eq!(trail[1].action, 5);
    assert_eq!(
        trail[1].receipt_no, None,
        "a drawer opening is of no receipt and must not borrow one"
    );
    assert_eq!(trail[2].action, 1);
    assert_eq!(trail[2].bp, 1_000);
    assert_eq!(trail[2].operator_name, "Rahima");
    assert_eq!(
        trail[2].authorised_by_name, "Karim",
        "the first answer stands"
    );
    assert_eq!(trail[1].authorised_by, 0);

    // A device that died between bumping its count and writing it down comes
    // back and reuses the count for something else. Keyed on the count alone
    // that record would be dropped as a duplicate, which is the one failure
    // this table exists to prevent.
    let reused = AllowedAction {
        terminal,
        seq: 2,
        at_ms: 1_788_600_200_000,
        action: 3,
        bp: 0,
        operator: cashier,
        operator_name: "Rahima".to_owned(),
        authorised_by: supervisor,
        authorised_by_name: "Karim".to_owned(),
        receipt_no: None,
    };
    repo.put_allowed(tenant, terminal, &[reused]).await.unwrap();
    let trail = repo
        .allowed(tenant, 0, 1_799_999_999_999, 50)
        .await
        .unwrap();
    assert_eq!(
        trail.len(),
        4,
        "both survive: a refund is not a drawer opening"
    );
    assert_eq!(trail[0].action, 3, "and the newest is the refund");

    // A window that ends before it happened holds nothing, which is what a day
    // report is.
    assert!(
        repo.allowed(tenant, 0, 1_788_599_999_999, 50)
            .await
            .unwrap()
            .is_empty()
    );
    // And another shop sees none of it.
    assert!(
        repo.allowed(unique(), 0, u64::MAX, 50)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A shop with more people on account than one page holds reads the rest,/// A shop with more people on account than one page holds reads the rest,
/// rather than being shown the first page as though it were the whole list.
#[tokio::test]
async fn the_owed_list_is_read_a_page_at_a_time_without_repeating_or_skipping() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Seven people, two of them owing exactly the same, which is the case a
    // cursor on the amount alone would either repeat or skip.
    let owed = [900_i64, 800, 700, 700, 600, 500, 400];
    for (index, amount) in owed.iter().enumerate() {
        let mut one = sale(tenant, terminal, unique(), Some(&receipt()));
        one.on_account = vec![AccountCharge {
            person_key: format!("person{index}"),
            person_name: format!("Person {index}"),
            amount_minor: *amount,
        }];
        repo.store_sale(one).await.unwrap();
    }

    let mut seen: Vec<(String, i64)> = Vec::new();
    let mut cursor: Option<(i64, String)> = None;
    loop {
        let page = repo.owed(tenant, cursor.clone(), 3).await.unwrap();
        if page.is_empty() {
            break;
        }
        for one in &page {
            seen.push((one.person_key.clone(), one.owed_minor));
        }
        let last = page
            .last()
            .expect("a page that is not empty has a last row");
        cursor = Some((last.owed_minor, last.person_key.clone()));
    }

    assert_eq!(seen.len(), 7, "every account, and none of them twice");
    let amounts: Vec<i64> = seen.iter().map(|(_, amount)| *amount).collect();
    assert_eq!(amounts, vec![900, 800, 700, 700, 600, 500, 400]);
    let mut keys: Vec<&str> = seen.iter().map(|(key, _)| key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 7, "no row is served on two pages");
}

/// A year of somebody's shopping is more than one page, and the old lines are
/// what a dispute is about.
#[tokio::test]
async fn one_account_is_read_a_page_at_a_time() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Five sales, two of them rung in the same millisecond by two tills, which
    // is what a cursor on the clock alone would repeat or skip.
    for index in 0..5_u64 {
        let mut one = sale(tenant, terminal, unique(), Some(&receipt()));
        one.rung_at_ms = 1_788_600_000_000 + (index / 2) * 1_000;
        one.on_account = vec![AccountCharge {
            person_key: "karim".to_owned(),
            person_name: "Karim".to_owned(),
            amount_minor: 1_000,
        }];
        repo.store_sale(one).await.unwrap();
    }

    let mut seen: Vec<u128> = Vec::new();
    let mut cursor: Option<(u64, u128)> = None;
    loop {
        let page = repo.account(tenant, "karim", cursor, 2).await.unwrap();
        if page.is_empty() {
            break;
        }
        for entry in &page {
            seen.push(entry.source_id);
        }
        let last = page
            .last()
            .expect("a page that is not empty has a last row");
        cursor = Some((last.at_ms, last.source_id));
    }

    assert_eq!(seen.len(), 5, "every line, and none of them twice");
    let mut unique_lines = seen.clone();
    unique_lines.sort_unstable();
    unique_lines.dedup();
    assert_eq!(unique_lines.len(), 5, "no line is served on two pages");
}

/// A strike-out takes a real debt off somebody's account, so a wrong one has to
/// be findable and reversible.
#[tokio::test]
async fn a_strike_out_made_in_error_can_be_taken_back() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let mut one = sale(tenant, terminal, id, Some("T1-000700"));
    one.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000700".to_owned(),
    });
    one.on_account = vec![AccountCharge {
        person_key: "karim".to_owned(),
        person_name: "Karim".to_owned(),
        amount_minor: 49_450,
    }];
    repo.store_sale(one).await.unwrap();

    repo.resolve_quarantine(tenant, id, "rung twice", false)
        .await
        .unwrap();
    assert_eq!(repo.balance(tenant, "karim").await.unwrap(), 0);
    assert!(repo.repair_queue(tenant, 50).await.unwrap().is_empty());

    let decided = repo.decided(tenant, 50).await.unwrap();
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].id, id);
    assert!(!decided[0].kept);
    assert_eq!(decided[0].decisions, 1);
    assert!(
        decided[0].decided_at_ms > 1_700_000_000_000,
        "when it was decided must be a real wall clock time: {}",
        decided[0].decided_at_ms
    );

    assert_eq!(
        repo.decide_again(
            tenant,
            id,
            "wrong one: the other was the duplicate",
            true,
            1
        )
        .await
        .unwrap(),
        Decided::Changed
    );
    assert_eq!(
        repo.balance(tenant, "karim").await.unwrap(),
        49_450,
        "the debt comes back"
    );
    let decided = repo.decided(tenant, 50).await.unwrap();
    assert!(decided[0].kept);
    assert_eq!(decided[0].decisions, 2, "both answers are kept");
    assert_eq!(decided[0].note, "wrong one: the other was the duplicate");

    // A second owner whose screen still showed one answer is refused rather
    // than allowed to put its stale view back.
    assert_eq!(
        repo.decide_again(tenant, id, "no, strike it out", false, 1)
            .await
            .unwrap(),
        Decided::Stale
    );
    assert!(repo.decided(tenant, 50).await.unwrap()[0].kept);

    // And it does not reappear in the queue: it has been answered, twice.
    assert!(repo.repair_queue(tenant, 50).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_sale_nobody_has_decided_about_cannot_be_decided_again() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let mut one = sale(tenant, terminal, id, Some("T1-000701"));
    one.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000701".to_owned(),
    });
    repo.store_sale(one).await.unwrap();

    assert_eq!(
        repo.decide_again(tenant, id, "changed my mind about nothing", false, 0)
            .await
            .unwrap(),
        Decided::Unanswered
    );
    assert_eq!(repo.repair_queue(tenant, 50).await.unwrap().len(), 1);
    assert!(repo.decided(tenant, 50).await.unwrap().is_empty());
}

/// A shop that arrives in a file can still find what it decided and change it.
#[tokio::test]
async fn a_restored_decision_can_be_found_and_changed() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Restored Shop").await.unwrap();

    let id = unique();
    repo.put_sales(
        tenant,
        &[SaleRecord {
            id,
            terminal,
            receipt_no: Some(receipt()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![1, 2, 3, 4],
            quarantine: Some("rang twice after a restore".to_owned()),
            quarantine_kind: Vec::new(),
            resolution: Some(("the tablet rang it again".to_owned(), false)),
            vat: Vec::new(),
            overrides: Vec::new(),
            refund_of: None,
        }],
    )
    .await
    .unwrap();

    let decided = repo.decided(tenant, 50).await.unwrap();
    assert_eq!(decided.len(), 1, "the decision came in the bundle");
    assert!(!decided[0].kept);
    assert_eq!(decided[0].decisions, 1);

    assert_eq!(
        repo.decide_again(tenant, id, "no, that one was real", true, 1)
            .await
            .unwrap(),
        Decided::Changed
    );
    let decided = repo.decided(tenant, 50).await.unwrap();
    assert!(decided[0].kept);
    assert_eq!(
        decided[0].decisions, 2,
        "what arrived in the file is the first answer, not a lost one"
    );
}

/// A sale that was never held is not the queue's business, however it arrived.
#[tokio::test]
async fn a_sale_that_was_never_held_cannot_be_decided() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Restored Shop").await.unwrap();

    let id = unique();
    repo.put_sales(
        tenant,
        &[SaleRecord {
            id,
            terminal,
            receipt_no: Some(receipt()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![1, 2, 3, 4],
            // Never quarantined, yet carrying a note. A bundle can say this and
            // it must not become a way to strike out an ordinary sale.
            quarantine: None,
            quarantine_kind: Vec::new(),
            resolution: Some(("a note from nowhere".to_owned(), true)),
            vat: Vec::new(),
            overrides: Vec::new(),
            refund_of: None,
        }],
    )
    .await
    .unwrap();

    assert!(repo.decided(tenant, 50).await.unwrap().is_empty());
    assert_eq!(
        repo.decide_again(tenant, id, "strike it out", false, 0)
            .await
            .unwrap(),
        Decided::Unanswered
    );
    assert_eq!(
        repo.takings(tenant, 1_788_500_000_000, 1_788_700_000_000)
            .await
            .unwrap()[0]
            .sales,
        1,
        "and it still counts"
    );
}

/// Another shop's decisions are not this one's to see or to change.
#[tokio::test]
async fn deciding_again_stops_at_the_shop_boundary() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    let other = unique();
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    repo.enrol(other, unique(), "Another Shop").await.unwrap();

    let id = unique();
    let mut one = sale(tenant, terminal, id, Some("T1-000702"));
    one.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000702".to_owned(),
    });
    repo.store_sale(one).await.unwrap();
    repo.resolve_quarantine(tenant, id, "rung twice", false)
        .await
        .unwrap();

    assert!(repo.decided(other, 50).await.unwrap().is_empty());
    assert_eq!(
        repo.decide_again(other, id, "not mine to change", true, 0)
            .await
            .unwrap(),
        Decided::Unanswered
    );
    assert!(
        !repo.decided(tenant, 50).await.unwrap()[0].kept,
        "and the shop that decided still has its answer"
    );
}

/// A restore must not put a struck-out duplicate back into the takings.
#[tokio::test]
async fn a_restored_shop_keeps_what_was_decided() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Restored Shop").await.unwrap();

    let id = unique();
    repo.put_sales(
        tenant,
        &[SaleRecord {
            id,
            terminal,
            receipt_no: Some(receipt()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![1, 2, 3, 4],
            quarantine: Some("rang twice after a restore".to_owned()),
            quarantine_kind: Vec::new(),
            resolution: Some((
                "the tablet was restored and rang it again".to_owned(),
                false,
            )),
            vat: Vec::new(),
            overrides: Vec::new(),
            refund_of: None,
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        repo.takings(tenant, 1_788_500_000_000, 1_788_700_000_000)
            .await
            .unwrap(),
        vec![],
        "a duplicate somebody struck out does not come back in a bundle"
    );
    assert!(
        repo.repair_queue(tenant, 50).await.unwrap().is_empty(),
        "nor back into the queue somebody already worked"
    );
}

/// A sale somebody looked at and kept counts exactly as it did before.
#[tokio::test]
async fn a_sale_that_stands_still_counts_after_it_is_looked_at() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let mut one = sale(tenant, terminal, id, Some("T1-000600"));
    one.vat = vec![(750, 45_998, 3_452, 0)];
    one.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000600".to_owned(),
    });
    repo.store_sale(one).await.unwrap();

    repo.resolve_quarantine(tenant, id, "checked against the paper receipt", true)
        .await
        .unwrap();

    let (from, to) = (1_788_500_000_000, 1_788_700_000_000);
    assert_eq!(repo.takings(tenant, from, to).await.unwrap()[0].sales, 1);
    let vat = repo.vat_summary(tenant, from, to).await.unwrap();
    assert_eq!(vat.rows[0].vat_minor, 3_452);
    assert_eq!(
        vat.waiting_vat_minor, 0,
        "a sale somebody has looked at is not waiting on anybody"
    );
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

    assert!(
        !repo
            .resolve_quarantine(tenant, id, "nothing to fix", true)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .resolve_quarantine(tenant, unique(), "no such sale", true)
            .await
            .unwrap()
    );
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
        !repo
            .resolve_quarantine(shop_b, id, "not mine to close", true)
            .await
            .unwrap(),
        "the update names no tenant, so this only fails if the policy is inert"
    );
    assert!(repo.repair_queue(shop_b, 50).await.unwrap().is_empty());
    assert_eq!(repo.repair_queue(shop_a, 50).await.unwrap().len(), 1);
}

#[tokio::test]
async fn terminal_health_reports_what_a_support_call_starts_with() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Counter by the door")
        .await
        .unwrap();

    // A device enrolled and not yet heard from. Absent, not zero: a zero would
    // render as 1970 and read as a fault rather than as silence.
    let health = repo.terminal_health(tenant).await.unwrap();
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].terminal, terminal);
    assert_eq!(health[0].label, "Counter by the door");
    assert_eq!(health[0].epoch, 1);
    assert_eq!(health[0].last_seen_ms, None);
    assert_eq!(
        health[0].sales, 0,
        "an unused till appears, rather than vanishing"
    );
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
    let seen = health[0]
        .last_seen_ms
        .expect("a till that synced must show it");
    assert!(
        seen > 1_700_000_000_000,
        "last seen must be a wall clock time: {seen}"
    );
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
    assert_eq!(
        health[0].sales, 0,
        "the join must not count another shop's sales"
    );
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
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use openpos_core::protocol::{
        CatalogueEditResponse, PROTOCOL_VERSION, PullRequest, PullResponse, RepairQueueRequest,
        RepairQueueResponse, ResolveRepairRequest, ResolveRepairResponse, TerminalHealthRequest,
        TerminalHealthResponse, UpsertItemRequest,
    };
    use openpos_server::http::{AppState, CONTENT_TYPE, router};
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
            expected_seq: 0,
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
    assert!(queue.entries[0].reason.contains("494.50"));

    let (status, resolved) = call::<_, ResolveRepairResponse>(
        &app,
        "/v1/back-office/repairs/resolve",
        &ResolveRepairRequest {
            protocol: PROTOCOL_VERSION,
            tenant,
            terminal,
            sale: quarantined,
            note: "till was restored from a backup, receipt reissued".to_owned(),
            kept: true,
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

#[tokio::test]
async fn a_counted_drawer_keeps_its_variance_and_the_person_who_counted_it() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let counter = unique();
    let closing = ClosedShift {
        id,
        terminal,
        closed_by: counter,
        closed_by_name: "Rahima".to_owned(),
        opened_at_ms: 1_788_600_000_000,
        closed_at_ms: 1_788_640_000_000,
        opening_float_minor: 50_000,
        sales: 37,
        cash_sales_minor: 124_500,
        non_cash_sales_minor: 30_000,
        cash_in_minor: 0,
        cash_out_minor: 20_000,
        expected_cash_minor: 154_500,
        counted_cash_minor: 150_500,
        variance_minor: -4_000,
    };
    assert_eq!(
        repo.put_shifts(tenant, std::slice::from_ref(&closing))
            .await
            .unwrap(),
        vec![id]
    );

    // A dropped reply is the usual reason a till sends twice, and it has to be
    // told it may stop rather than told to try forever.
    assert_eq!(repo.put_shifts(tenant, &[closing]).await.unwrap(), vec![id]);

    let found = repo.closed_shifts(tenant, 10).await.unwrap();
    assert_eq!(found.len(), 1, "and the second send left one row, not two");
    assert_eq!(
        found[0].variance_minor, -4_000,
        "forty taka short, and it says so"
    );
    assert_eq!(found[0].counted_cash_minor, 150_500);
    // Who counted it, which is the other half of what an owner wants to know.
    assert_eq!(found[0].closed_by, counter);
    assert_eq!(found[0].closed_by_name, "Rahima");

    // And it belongs to the shop that sent it, like everything else here.
    assert!(repo.closed_shifts(unique(), 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_drawer_counted_before_the_till_named_the_counter_is_still_kept() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // What an older till sends: a count, and nobody's name against it. The
    // shop is told this drawer was short and cannot be told by whom, which is
    // the truth about it and better than a guess.
    let id = unique();
    repo.put_shifts(
        tenant,
        &[ClosedShift {
            id,
            terminal,
            closed_by: 0,
            closed_by_name: String::new(),
            opened_at_ms: 1_788_600_000_000,
            closed_at_ms: 1_788_640_000_000,
            opening_float_minor: 50_000,
            sales: 1,
            cash_sales_minor: 49_450,
            non_cash_sales_minor: 0,
            cash_in_minor: 0,
            cash_out_minor: 0,
            expected_cash_minor: 99_450,
            counted_cash_minor: 95_450,
            variance_minor: -4_000,
        }],
    )
    .await
    .unwrap();

    let found = repo.closed_shifts(tenant, 10).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].closed_by, 0);
    assert!(found[0].closed_by_name.is_empty());
}

#[tokio::test]
async fn what_somebody_owes_is_summed_from_the_book_and_settled_by_paying() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // Two sales on account for one person, written two ways, plus a sale to
    // somebody else so the grouping has to do something.
    let first = unique();
    let mut sale_one = sale(tenant, terminal, first, Some("T1-000200"));
    sale_one.on_account = vec![AccountCharge {
        person_key: "karim, flat 3".to_owned(),
        person_name: "Karim, flat 3".to_owned(),
        amount_minor: 29_450,
    }];
    repo.store_sale(sale_one.clone()).await.unwrap();

    let second = unique();
    let mut sale_two = sale(tenant, terminal, second, Some("T1-000201"));
    sale_two.rung_at_ms = 1_788_700_000_000;
    sale_two.on_account = vec![AccountCharge {
        person_key: "karim, flat 3".to_owned(),
        // A later spelling, which is what should be shown back.
        person_name: "Karim (flat 3)".to_owned(),
        amount_minor: 10_000,
    }];
    repo.store_sale(sale_two).await.unwrap();

    let other = unique();
    let mut sale_three = sale(tenant, terminal, other, Some("T1-000202"));
    sale_three.on_account = vec![AccountCharge {
        person_key: "rina".to_owned(),
        person_name: "Rina".to_owned(),
        amount_minor: 5_000,
    }];
    repo.store_sale(sale_three).await.unwrap();

    // A till resending a sale it was not told about must not double the debt.
    repo.store_sale(sale_one).await.unwrap();

    let owing = repo.owed(tenant, None, 50).await.unwrap();
    assert_eq!(owing.len(), 2);
    assert_eq!(owing[0].person_key, "karim, flat 3", "most owed first");
    assert_eq!(owing[0].owed_minor, 39_450, "and the replay added nothing");
    assert_eq!(
        owing[0].person_name, "Karim (flat 3)",
        "the latest spelling"
    );
    assert_eq!(owing[0].entries, 2);
    assert_eq!(owing[1].owed_minor, 5_000);

    // Friday. He pays most of it, and the reply is dropped, so it is sent again.
    let payment = AccountPayment {
        id: unique(),
        kind: Settlement::Paid,
        person_key: "karim, flat 3".to_owned(),
        person_name: "Karim (flat 3)".to_owned(),
        amount_minor: 30_000,
        at_ms: 1_788_900_000_000,
        note: Some("in cash".to_owned()),
    };
    assert!(repo.take_payment(tenant, &payment).await.unwrap());
    assert!(
        !repo.take_payment(tenant, &payment).await.unwrap(),
        "a payment counted twice is money the shop believes it has been given"
    );

    let owing = repo.owed(tenant, None, 50).await.unwrap();
    assert_eq!(owing[0].owed_minor, 9_450, "what is left of it");
    assert_eq!(
        owing[1].person_key, "rina",
        "and she is untouched by any of it"
    );
    assert_eq!(owing[1].owed_minor, 5_000);

    // What it is made of, newest first, which is what gets read out in an
    // argument about the total.
    let entries = repo
        .account(tenant, "karim, flat 3", None, 50)
        .await
        .unwrap();
    assert_eq!(entries.len(), 3);
    assert!(!entries[0].is_sale);
    assert_eq!(entries[0].amount_minor, -30_000);
    assert_eq!(entries[0].note, "in cash");
    assert!(entries[1].is_sale && entries[2].is_sale);

    // Settled, and off the list. The entries stay where they are.
    repo.take_payment(
        tenant,
        &AccountPayment {
            id: unique(),
            kind: Settlement::Paid,
            person_key: "karim, flat 3".to_owned(),
            person_name: "Karim (flat 3)".to_owned(),
            amount_minor: 9_450,
            at_ms: 1_789_000_000_000,
            note: None,
        },
    )
    .await
    .unwrap();
    let owing = repo.owed(tenant, None, 50).await.unwrap();
    assert_eq!(owing.len(), 1, "only Rina is still in the book");
    assert_eq!(
        repo.account(tenant, "karim, flat 3", None, 50)
            .await
            .unwrap()
            .len(),
        4
    );

    // And none of it belongs to the shop next door.
    assert!(repo.owed(unique(), None, 50).await.unwrap().is_empty());
}

#[tokio::test]
async fn one_minted_id_counts_once_whoever_it_names() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    assert!(
        repo.take_payment(
            tenant,
            &AccountPayment {
                id,
                kind: Settlement::Paid,
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor: 10_000,
                at_ms: 1_788_900_000_000,
                note: None,
            },
        )
        .await
        .unwrap()
    );

    // The same payment sent again against a different spelling. Keying only on
    // the person let one payment count twice, which is money the shop believes
    // it has been given.
    assert!(
        !repo
            .take_payment(
                tenant,
                &AccountPayment {
                    id,
                    kind: Settlement::Paid,
                    person_key: "rina".to_owned(),
                    person_name: "Rina".to_owned(),
                    amount_minor: 10_000,
                    at_ms: 1_788_900_000_000,
                    note: None,
                },
            )
            .await
            .unwrap()
    );

    assert_eq!(repo.balance(tenant, "karim").await.unwrap(), -10_000);
    assert_eq!(repo.balance(tenant, "rina").await.unwrap(), 0);
}

#[tokio::test]
async fn a_debt_struck_off_is_told_apart_from_money_taken() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    // A till restored from a backup rang the same goods onto one account twice.
    let mut doubled = sale(tenant, terminal, unique(), Some("T1-000300"));
    doubled.on_account = vec![AccountCharge {
        person_key: "karim".to_owned(),
        person_name: "Karim".to_owned(),
        amount_minor: 29_450,
    }];
    repo.store_sale(doubled.clone()).await.unwrap();
    let mut again = doubled.clone();
    again.id = unique();
    again.receipt_no = Some("T1-000301".to_owned());
    repo.store_sale(again).await.unwrap();
    assert_eq!(repo.balance(tenant, "karim").await.unwrap(), 58_900);

    // The owner strikes one of them off. Until this existed the only way to
    // correct it was to record a payment nobody made.
    repo.take_payment(
        tenant,
        &AccountPayment {
            id: unique(),
            kind: Settlement::WrittenOff,
            person_key: "karim".to_owned(),
            person_name: "Karim".to_owned(),
            amount_minor: 29_450,
            at_ms: 1_788_900_000_000,
            note: Some("rung twice after the tablet was restored".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(repo.balance(tenant, "karim").await.unwrap(), 29_450);
    let entries = repo.account(tenant, "karim", None, 50).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert!(
        entries[0].written_off,
        "and it says money never changed hands"
    );
    assert!(!entries[0].is_sale);
    assert!(entries[0].note.contains("restored"));
}

#[tokio::test]
async fn a_replayed_sale_with_a_different_name_adds_no_debt() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let id = unique();
    let mut first = sale(tenant, terminal, id, Some("T1-000400"));
    first.on_account = vec![AccountCharge {
        person_key: "karim".to_owned(),
        person_name: "Karim".to_owned(),
        amount_minor: 29_450,
    }];
    repo.store_sale(first).await.unwrap();

    // The same sale id, naming somebody else. The sale insert does nothing, so
    // the book must do nothing too: a debt with no sale behind it is a debt
    // nobody can be shown the reason for.
    let mut tampered = sale(tenant, terminal, id, Some("T1-000400"));
    tampered.on_account = vec![AccountCharge {
        person_key: "rina".to_owned(),
        person_name: "Rina".to_owned(),
        amount_minor: 29_450,
    }];
    repo.store_sale(tampered).await.unwrap();

    assert_eq!(repo.balance(tenant, "rina").await.unwrap(), 0);
    assert_eq!(repo.balance(tenant, "karim").await.unwrap(), 29_450);
}

#[tokio::test]
async fn a_count_filed_in_batches_and_retried_lands_once() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();

    // A shop counting its shelves over an afternoon files what it has as it
    // goes. The line ids are minted on the device and kept, so a batch whose
    // reply was dropped can be sent again.
    let first = unique();
    let count = StockCount {
        id: first,
        item_id: rice,
        counted_milli: 37_000,
        counted_at_ms: 1_788_600_000_000,
        counted_by: terminal,
        note: None,
    };
    repo.record_count(tenant, &count).await.unwrap();
    repo.record_count(tenant, &count).await.unwrap();

    let found = repo.on_hand(tenant, rice).await.unwrap();
    assert_eq!(
        found.qty_milli, 37_000,
        "the same count twice is one count, not two shelves"
    );

    // Later in the afternoon somebody recounts the same shelf and finds one
    // more. The newer count is the one that describes the shelf.
    repo.record_count(
        tenant,
        &StockCount {
            id: unique(),
            item_id: rice,
            counted_milli: 38_000,
            counted_at_ms: 1_788_600_100_000,
            counted_by: terminal,
            note: Some("counted again after the delivery went out".to_owned()),
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.on_hand(tenant, rice).await.unwrap().qty_milli, 38_000);

    // And the earlier one, resent because a device was still retrying it, does
    // not put the shelf back to what it was before.
    repo.record_count(tenant, &count).await.unwrap();
    assert_eq!(
        repo.on_hand(tenant, rice).await.unwrap().qty_milli,
        38_000,
        "a late retry of an older count must not overwrite a newer one"
    );
}

#[tokio::test]
async fn an_open_drawer_is_a_position_and_a_counted_one_ends_it() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let shift = unique();
    let drawer = OpenDrawer {
        terminal,
        shift,
        opened_at_ms: 1_788_600_000_000,
        reported_at_ms: 1_788_620_000_000,
        opening_float_minor: 50_000,
        sales: 12,
        cash_sales_minor: 74_500,
        non_cash_sales_minor: 10_000,
        cash_in_minor: 0,
        cash_out_minor: 20_000,
        expected_cash_minor: 104_500,
    };
    repo.put_open_drawer(tenant, &drawer).await.unwrap();
    repo.put_open_drawer(
        tenant,
        &OpenDrawer {
            reported_at_ms: 1_788_621_200_000,
            sales: 15,
            expected_cash_minor: 120_000,
            ..drawer.clone()
        },
    )
    .await
    .unwrap();

    let open = repo.open_drawers(tenant).await.unwrap();
    assert_eq!(open.len(), 1, "one till, one open drawer");
    assert_eq!(open[0].expected_cash_minor, 120_000, "the later figure");
    assert_eq!(open[0].reported_at_ms, 1_788_621_200_000);

    // Somebody counts it. An open list still showing a drawer counted an hour
    // ago is a list an owner learns to ignore.
    repo.put_shifts(
        tenant,
        &[ClosedShift {
            id: shift,
            terminal,
            closed_by: unique(),
            closed_by_name: "Rahima".to_owned(),
            opened_at_ms: 1_788_600_000_000,
            closed_at_ms: 1_788_640_000_000,
            opening_float_minor: 50_000,
            sales: 15,
            cash_sales_minor: 90_000,
            non_cash_sales_minor: 10_000,
            cash_in_minor: 0,
            cash_out_minor: 20_000,
            expected_cash_minor: 120_000,
            counted_cash_minor: 119_000,
            variance_minor: -1_000,
        }],
    )
    .await
    .unwrap();
    assert!(repo.open_drawers(tenant).await.unwrap().is_empty());

    // And a shop next door sees none of it.
    repo.put_open_drawer(tenant, &drawer).await.unwrap();
    assert!(repo.open_drawers(unique()).await.unwrap().is_empty());
}

#[tokio::test]
async fn who_buys_on_account_is_written_down_and_corrected_in_place() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let karim = unique();
    repo.put_customer(
        tenant,
        &CustomerRecord {
            id: karim,
            name: "Karim, flat 3".to_owned(),
            phone: Some("01711000000".to_owned()),
            active: true,
            bin: None,
            limit_minor: 0,
        },
    )
    .await
    .unwrap();
    repo.put_customer(
        tenant,
        &CustomerRecord {
            id: unique(),
            name: "Rina".to_owned(),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        },
    )
    .await
    .unwrap();

    let found = repo.customers(tenant).await.unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(
        found[0].name, "Karim, flat 3",
        "by name, as a screen offers"
    );

    // A correction lands on the same person rather than making a second one,
    // which matters here more than anywhere: the account is keyed on the id.
    repo.put_customer(
        tenant,
        &CustomerRecord {
            id: karim,
            name: "Karim Uddin, flat 3".to_owned(),
            phone: Some("01711000001".to_owned()),
            active: false,
            bin: None,
            limit_minor: 0,
        },
    )
    .await
    .unwrap();
    let found = repo.customers(tenant).await.unwrap();
    assert_eq!(found.len(), 2, "corrected, not duplicated");
    let corrected = found
        .iter()
        .find(|one| one.id == karim)
        .expect("still there");
    assert_eq!(corrected.name, "Karim Uddin, flat 3");
    assert!(!corrected.active, "and the shop has stopped their account");

    // A shop next door sees none of them.
    assert!(repo.customers(unique()).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_sale_naming_a_customer_lands_on_that_customer_not_on_the_spelling() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let karim = unique();
    let key = openpos_core::accounts::customer_key(karim);

    // Two sales on account for one written-down person, spelled differently at
    // the till on the two days. Keyed on the customer, they are one account.
    for (id, spelling, amount, at_ms) in [
        (unique(), "Karim", 29_450_i64, 1_788_600_000_000_u64),
        // The next day, spelled differently. Distinct clocks on purpose: this
        // is about which spelling is the latest, not about how a tie is broken.
        (unique(), "karim uddin", 10_000, 1_788_700_000_000),
    ] {
        let mut on_account = sale(tenant, terminal, id, None);
        on_account.rung_at_ms = at_ms;
        on_account.on_account = vec![AccountCharge {
            person_key: key.clone(),
            person_name: spelling.to_owned(),
            amount_minor: amount,
        }];
        repo.store_sale(on_account).await.unwrap();
    }

    assert_eq!(repo.balance(tenant, &key).await.unwrap(), 39_450);
    let owing = repo.owed(tenant, None, 10).await.unwrap();
    assert_eq!(owing.len(), 1, "one person, however it was typed");
    assert_eq!(owing[0].person_key, key);
    // The spelling still shows: it is what is on the receipt in their hand.
    assert_eq!(owing[0].person_name, "karim uddin");
}

#[tokio::test]
async fn a_days_drawers_and_account_movement_are_summed_in_the_database() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let day = 1_788_600_000_000_u64;

    // Two drawers counted in the day, one short and one over, and one counted
    // yesterday that belongs to nobody's question today.
    for (closed_at, expected, counted, variance) in [
        (day + 5_000, 87_450_i64, 83_450_i64, -4_000_i64),
        (day + 6_000, 40_000, 41_000, 1_000),
        (day - 100_000, 10_000, 10_000, 0),
    ] {
        repo.put_shifts(
            tenant,
            &[ClosedShift {
                id: unique(),
                terminal,
                closed_by: unique(),
                closed_by_name: "Rahima".to_owned(),
                opened_at_ms: day,
                closed_at_ms: closed_at,
                opening_float_minor: 50_000,
                sales: 3,
                cash_sales_minor: 37_450,
                non_cash_sales_minor: 0,
                cash_in_minor: 0,
                cash_out_minor: 0,
                expected_cash_minor: expected,
                counted_cash_minor: counted,
                variance_minor: variance,
            }],
        )
        .await
        .unwrap();
    }

    // A sale on account, a payment against an older one, and a debt struck off.
    let mut on_account = sale(tenant, terminal, unique(), None);
    on_account.rung_at_ms = day + 1_000;
    on_account.on_account = vec![AccountCharge {
        person_key: "karim".to_owned(),
        person_name: "Karim".to_owned(),
        amount_minor: 30_000,
    }];
    repo.store_sale(on_account).await.unwrap();

    for (kind, amount) in [
        (Settlement::Paid, 5_000_i64),
        (Settlement::WrittenOff, 2_000),
    ] {
        repo.take_payment(
            tenant,
            &AccountPayment {
                id: unique(),
                kind,
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor: amount,
                at_ms: day + 4_000,
                note: Some("rung twice after the tablet was restored".to_owned()),
            },
        )
        .await
        .unwrap();
    }

    let seen = repo.day_summary(tenant, day, day + 10_000).await.unwrap();
    assert_eq!(seen.drawers_counted, 2, "yesterday's is not today's");
    assert_eq!(seen.expected_cash_minor, 127_450);
    assert_eq!(seen.counted_cash_minor, 124_450);
    assert_eq!(
        seen.variance_minor, -3_000,
        "a short drawer and an over one do not cancel into nothing"
    );
    // Three numbers, because money the shop was given and money it gave up are
    // not the same thing.
    assert_eq!(seen.charged_minor, 30_000);
    assert_eq!(seen.paid_minor, 5_000);
    assert_eq!(seen.written_off_minor, 2_000);

    // And a quiet day answers zeroes rather than failing to answer.
    let quiet = repo
        .day_summary(tenant, day - 1_000_000, day - 500_000)
        .await
        .unwrap();
    assert_eq!(quiet.drawers_counted, 0);
    assert_eq!(quiet.charged_minor, 0);

    // None of it belongs to the shop next door.
    let elsewhere = repo.day_summary(unique(), day, day + 10_000).await.unwrap();
    assert_eq!(elsewhere.drawers_counted, 0);
}

#[tokio::test]
async fn what_the_shop_owes_the_revenue_is_grouped_by_rate_and_by_the_day_it_sold() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let day = 1_788_600_000_000_u64;

    // Two sales in the month at two rates, and one the month before.
    for (at_ms, rows) in [
        (
            day,
            vec![(1_500_u32, 43_000_i64, 6_450_i64, 0_u8), (0, 20_000, 0, 2)],
        ),
        (day + 1_000, vec![(1_500, 7_000, 1_050, 0)]),
        (day - 40_000_000_000, vec![(1_500, 99_000, 14_850, 0)]),
    ] {
        let mut sold = sale(tenant, terminal, unique(), None);
        sold.rung_at_ms = at_ms;
        sold.vat = rows;
        repo.store_sale(sold).await.unwrap();
    }

    let month = repo
        .vat_summary(tenant, day - 1_000_000, day + 1_000_000)
        .await
        .unwrap();
    assert_eq!(month.rows.len(), 2, "one row per rate, smallest first");
    assert_eq!(month.rows[0].vat_bp, 0);
    assert_eq!(month.rows[0].net_minor, 20_000, "exempt is still declared");
    assert_eq!(month.rows[0].supply, 2, "and it says which nothing it was");
    assert_eq!(month.rows[0].vat_minor, 0);
    assert_eq!(month.rows[1].vat_bp, 1_500);
    assert_eq!(
        month.rows[1].net_minor, 50_000,
        "both sales, not last month's"
    );
    assert_eq!(month.rows[1].vat_minor, 7_500);
    assert_eq!(month.rows[1].sales, 2);
    assert_eq!(
        month.waiting_sales, 0,
        "and none of it is waiting on anybody"
    );

    // A refund in the period takes it back down, which is what makes a refund a
    // refund rather than a second sale.
    let mut refunded = sale(tenant, terminal, unique(), None);
    refunded.rung_at_ms = day + 2_000;
    refunded.vat = vec![(1_500, -43_000, -6_450, 0)];
    repo.store_sale(refunded).await.unwrap();

    let month = repo
        .vat_summary(tenant, day - 1_000_000, day + 1_000_000)
        .await
        .unwrap();
    assert_eq!(month.rows[1].net_minor, 7_000);
    assert_eq!(month.rows[1].vat_minor, 1_050);

    // And the shop next door declares its own.
    assert!(
        repo.vat_summary(unique(), day - 1_000_000, day + 1_000_000)
            .await
            .unwrap()
            .rows
            .is_empty()
    );
}

#[tokio::test]
async fn what_the_shop_owes_a_supplier_is_the_deliveries_less_what_it_paid() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();
    let distributor = unique();
    repo.put_supplier(
        tenant,
        &Supplier {
            id: distributor,
            name: "Mirpur Distributors".to_owned(),
            phone: Some("01711000000".to_owned()),
            bin: None,
            active: true,
        },
    )
    .await
    .unwrap();

    // Two deliveries in the week. The second has a quantity and a cost whose
    // product lands on half a poisha, which is where the database's arithmetic
    // and the core's have to agree.
    for (qty, cost) in [(40_000_i64, 34_400_i64), (1_500, 4_333)] {
        repo.receive_goods(
            tenant,
            &GoodsReceipt {
                id: unique(),
                supplier_id: Some(distributor),
                reference: Some("CH-1".to_owned()),
                received_at_ms: 1_788_600_000_000,
                received_by: terminal,
                note: None,
                lines: vec![ReceiptLine {
                    item_id: rice,
                    qty_milli: qty,
                    unit_cost_minor: cost,
                }],
            },
        )
        .await
        .unwrap();
    }

    // 40 x 344.00 is 13,760.00, and 1.5 x 43.33 is 64.995, which rounds away
    // from zero to 65.00 exactly as `Minor::mul_qty` does on a device.
    let owing = repo.supplier_owing(tenant).await.unwrap();
    assert_eq!(owing.len(), 1);
    assert_eq!(owing[0].name, "Mirpur Distributors");
    assert_eq!(owing[0].owed_minor, 1_376_000 + 6_500);
    assert_eq!(owing[0].deliveries, 2);

    // Saturday: the distributor's man is paid most of it, and the reply is
    // dropped, so it is sent again.
    let payment = SupplierPayment {
        id: unique(),
        supplier_id: distributor,
        amount_minor: 1_000_000,
        paid_at_ms: 1_788_900_000_000,
        note: Some("in cash, Saturday".to_owned()),
    };
    assert!(repo.pay_supplier(tenant, &payment).await.unwrap());
    assert!(
        !repo.pay_supplier(tenant, &payment).await.unwrap(),
        "money the shop believes it has paid and has not is the same mistake"
    );

    let owing = repo.supplier_owing(tenant).await.unwrap();
    assert_eq!(owing[0].owed_minor, 1_376_000 + 6_500 - 1_000_000);

    // Settled, and off the list. Paying more than is owed shows as paid ahead
    // rather than as nothing at all.
    repo.pay_supplier(
        tenant,
        &SupplierPayment {
            id: unique(),
            supplier_id: distributor,
            amount_minor: 382_500,
            paid_at_ms: 1_789_000_000_000,
            note: None,
        },
    )
    .await
    .unwrap();
    assert!(repo.supplier_owing(tenant).await.unwrap().is_empty());

    repo.pay_supplier(
        tenant,
        &SupplierPayment {
            id: unique(),
            supplier_id: distributor,
            amount_minor: 10_000,
            paid_at_ms: 1_789_100_000_000,
            note: None,
        },
    )
    .await
    .unwrap();
    let ahead = repo.supplier_owing(tenant).await.unwrap();
    assert_eq!(ahead.len(), 1);
    assert_eq!(ahead[0].owed_minor, -10_000, "paid ahead, and it says so");

    // And none of it belongs to the shop next door.
    assert!(repo.supplier_owing(unique()).await.unwrap().is_empty());
}

#[tokio::test]
async fn what_sold_comes_from_the_movements_and_the_day_it_was_rung() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let day = 1_788_600_000_000_u64;

    let rice = unique();
    let oil = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();
    repo.upsert_item(tenant, &item(oil, 47_500)).await.unwrap();

    for (at_ms, stock) in [
        (day + 1_000, vec![(rice, -2_000_i64)]),
        (day + 2_000, vec![(rice, -1_000), (oil, -3_000)]),
        // A bag brought back, which a shop does not need to reorder.
        (day + 3_000, vec![(rice, 1_000)]),
        // And last month, which this question is not about.
        (day - 40_000_000_000, vec![(rice, -9_000)]),
    ] {
        let mut sold = sale(tenant, terminal, unique(), None);
        sold.rung_at_ms = at_ms;
        sold.stock = stock;
        repo.store_sale(sold).await.unwrap();
    }

    // A delivery of the same item in the same window, which is stock moving the
    // other way and is not something that sold.
    repo.receive_goods(
        tenant,
        &GoodsReceipt {
            id: unique(),
            supplier_id: None,
            reference: None,
            received_at_ms: day + 4_000,
            received_by: terminal,
            note: None,
            lines: vec![ReceiptLine {
                item_id: rice,
                qty_milli: 50_000,
                unit_cost_minor: 34_400,
            }],
        },
    )
    .await
    .unwrap();

    let rows = repo.sold(tenant, day, day + 10_000, 50).await.unwrap();
    assert_eq!(
        rows.len(),
        2,
        "most sold first, and last month is not in it"
    );
    assert_eq!(rows[0].item_id, oil);
    assert_eq!(rows[0].qty_milli, 3_000);
    assert_eq!(rows[1].item_id, rice);
    assert_eq!(
        rows[1].qty_milli, 2_000,
        "three sold, one brought back, and a delivery is not a sale"
    );
    assert_eq!(rows[1].sales, 3);

    // And the shop next door sells its own.
    assert!(
        repo.sold(unique(), day, day + 10_000, 50)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn cutting_a_device_off_stops_every_credential_it_holds() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let caller = Caller {
        tenant,
        terminal,
        role: Role::Till,
    };

    // A device that has renewed twice holds more than one working credential,
    // because a renewal overlaps rather than cutting a till off mid-day. All of
    // them have to stop, or a stolen tablet keeps trading on the older one.
    let first = openpos_server::auth::Token::generate();
    repo.store_token(caller, &first.hash()).await.unwrap();
    let second = openpos_server::auth::Token::generate();
    repo.renew_token(
        caller,
        &first.hash(),
        &second.hash(),
        std::time::Duration::from_secs(3_600),
    )
    .await
    .unwrap();
    assert!(repo.authenticate(&first.hash()).await.unwrap().is_some());
    assert!(repo.authenticate(&second.hash()).await.unwrap().is_some());

    let withdrawn = repo.revoke_all_tokens(caller).await.unwrap();
    assert!(withdrawn >= 2, "every one of them, not the newest");
    assert!(repo.authenticate(&first.hash()).await.unwrap().is_none());
    assert!(repo.authenticate(&second.hash()).await.unwrap().is_none());

    // Doing it twice is ordinary: an owner presses again because the first
    // press did not visibly do anything. Nothing is left to withdraw.
    assert_eq!(repo.revoke_all_tokens(caller).await.unwrap(), 0);

    // And the terminal is still there. Its sales are still its sales, and a
    // shop looking into a theft wants to see the device existed.
    assert!(repo.terminal_enrolled(tenant, terminal).await.unwrap());
}

#[tokio::test]
async fn a_supplier_statement_puts_goods_in_and_money_out_in_one_list() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();
    let distributor = unique();
    repo.put_supplier(
        tenant,
        &Supplier {
            id: distributor,
            name: "Mirpur Distributors".to_owned(),
            phone: None,
            bin: None,
            active: true,
        },
    )
    .await
    .unwrap();

    let monday = 1_788_600_000_000_u64;
    repo.receive_goods(
        tenant,
        &GoodsReceipt {
            id: unique(),
            supplier_id: Some(distributor),
            reference: Some("CH-1".to_owned()),
            received_at_ms: monday,
            received_by: terminal,
            note: None,
            lines: vec![ReceiptLine {
                item_id: rice,
                qty_milli: 10_000,
                unit_cost_minor: 34_400,
            }],
        },
    )
    .await
    .unwrap();
    repo.pay_supplier(
        tenant,
        &SupplierPayment {
            id: unique(),
            supplier_id: distributor,
            amount_minor: 200_000,
            paid_at_ms: monday + 86_400_000,
            note: Some("part payment, Tuesday".to_owned()),
        },
    )
    .await
    .unwrap();
    // Outside the window, so not in this statement even though it is in the
    // balance: a period that opens owing and closes owing says so either way.
    repo.pay_supplier(
        tenant,
        &SupplierPayment {
            id: unique(),
            supplier_id: distributor,
            amount_minor: 100_000,
            paid_at_ms: monday + 30 * 86_400_000,
            note: None,
        },
    )
    .await
    .unwrap();

    let (week, owed) = repo
        .supplier_statement(tenant, distributor, monday - 1_000, monday + 7 * 86_400_000)
        .await
        .unwrap();
    assert_eq!(
        week.len(),
        2,
        "the later payment is another week's business"
    );
    // The whole account, which is the number the two people argue about, read
    // with the lines rather than after them: 344,000 delivered less 200,000 and
    // the later 100,000 paid.
    assert_eq!(owed, 44_000, "what is owed comes back with the period");
    assert!(week[0].delivered, "goods arrive, then they are paid for");
    assert_eq!(week[0].amount_minor, 344_000);
    assert_eq!(week[0].reference.as_deref(), Some("CH-1"));
    assert!(!week[1].delivered);
    assert_eq!(week[1].amount_minor, 200_000);
    assert_eq!(week[1].reference.as_deref(), Some("part payment, Tuesday"));

    // And a supplier the shop has never dealt with has an empty statement
    // rather than an error.
    let (nothing, owed_nothing) = repo
        .supplier_statement(tenant, unique(), 0, u64::MAX)
        .await
        .unwrap();
    assert!(nothing.is_empty());
    assert_eq!(owed_nothing, 0, "and nothing is owed to somebody never dealt with");
}

#[tokio::test]
async fn a_catalogue_change_this_build_cannot_read_is_passed_over_and_named() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let rice = unique();
    repo.upsert_item(tenant, &item(rice, 43_000)).await.unwrap();

    // A change written by a build this one does not understand: the schema is
    // one nothing here can decode. This is what a downgrade leaves behind, and
    // what a shop's own database looks like after a rollback.
    repo.put_catalogue(
        tenant,
        &[CatalogueRecord {
            seq: 9_000,
            kind: 1,
            item_id: unique(),
            payload: Some(vec![9, 9, 9, 9]),
            schema: 99,
        }],
    )
    .await
    .unwrap();

    // A till pulling past it gets the readable change and not the other, and
    // its cursor moves past both: failing the page would stop every till in the
    // shop syncing for ever over one bad row.
    let page = repo.items_since(tenant, 0, 100).await.unwrap();
    assert_eq!(page.upserts.len(), 1, "the one it can read");
    assert_eq!(page.skipped, 1, "and it counted the one it could not");
    assert!(page.cursor >= 9_000, "the cursor moved past it regardless");

    // Which is only defensible because somebody can be told. Until this reader
    // existed the count went into a field the pull handler ignored.
    let lost = repo.unreadable_changes(tenant, 100).await.unwrap();
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].seq, 9_000);
    assert_eq!(lost[0].schema, 99, "the build that wrote it, by number");

    // A shop whose catalogue is entirely readable gets an empty list, which is
    // the ordinary answer rather than a special case.
    let (healthy, its_till) = (unique(), unique());
    repo.enrol(healthy, its_till, "Another Shop").await.unwrap();
    repo.upsert_item(healthy, &item(unique(), 10_000))
        .await
        .unwrap();
    assert!(
        repo.unreadable_changes(healthy, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_return_says_how_much_of_itself_is_waiting_on_somebody() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let day = 1_788_600_000_000_u64;

    // An ordinary sale, and one the server put in the queue: a receipt number
    // another sale already carries, which is what a restored tablet produces.
    let clean = unique();
    let mut sold = sale(tenant, terminal, clean, Some("T1-000700"));
    sold.rung_at_ms = day;
    sold.vat = vec![(1_500, 43_000, 6_450, 0)];
    repo.store_sale(sold).await.unwrap();

    let suspect = unique();
    let mut doubtful = sale(tenant, terminal, suspect, Some("T1-000700"));
    doubtful.rung_at_ms = day + 1_000;
    doubtful.vat = vec![(1_500, 43_000, 6_450, 0)];
    doubtful.quarantine = Some(QuarantineReason::DuplicateReceiptNumber {
        receipt_no: "T1-000700".to_owned(),
    });
    repo.store_sale(doubtful).await.unwrap();

    // Both are in the figure, because goods may well have left the shop twice
    // and a machine cannot know. What it can do is say how much is uncertain.
    let month = repo
        .vat_summary(tenant, day - 1_000, day + 10_000)
        .await
        .unwrap();
    assert_eq!(month.rows.len(), 1);
    assert_eq!(month.rows[0].vat_minor, 12_900, "both, for now");
    assert_eq!(month.waiting_sales, 1);
    assert_eq!(
        month.waiting_vat_minor, 6_450,
        "and this much of it is a sale nobody has looked at"
    );

    // Somebody looks at it and writes down what they decided. It stops being
    // uncertain; whether it was kept or not is in the note they left.
    repo.resolve_quarantine(
        tenant,
        suspect,
        "rung twice after the tablet was restored",
        true,
    )
    .await
    .unwrap();
    let month = repo
        .vat_summary(tenant, day - 1_000, day + 10_000)
        .await
        .unwrap();
    assert_eq!(month.waiting_sales, 0);
    assert_eq!(month.waiting_vat_minor, 0);
}

#[tokio::test]
async fn the_settings_counter_moves_when_the_people_or_the_shop_change() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let start = repo.settings_seq(tenant).await.unwrap();

    // Everything a till holds because it must work with the line down: the
    // people, the shop's own details, and who buys on account. Each one moves
    // the counter, because a till that has to re-read one may as well re-read
    // all three.
    repo.put_operator(
        tenant,
        &OperatorRecord {
            id: unique(),
            name: "Rahima".to_owned(),
            pin_salt: vec![7; 16],
            pin_rounds: openpos_core::auth::LEAST_PIN_ROUNDS,
            pin_key: vec![9; 32],
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
    let after_person = repo.settings_seq(tenant).await.unwrap();
    assert!(after_person > start, "somebody was added");

    repo.put_shop_details(
        tenant,
        &ShopDetails {
            name: "Karim General Store".to_owned(),
            ..ShopDetails::default()
        },
    )
    .await
    .unwrap();
    let after_shop = repo.settings_seq(tenant).await.unwrap();
    assert!(after_shop > after_person, "the shop's own details changed");

    repo.put_customer(
        tenant,
        &CustomerRecord {
            id: unique(),
            name: "Karim, flat 3".to_owned(),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        },
    )
    .await
    .unwrap();
    let after_customer = repo.settings_seq(tenant).await.unwrap();
    assert!(after_customer > after_shop, "somebody may buy on account");

    // A sale does not move it. This is the whole point: the counter is asked
    // for every half minute by every till, and a shop that is merely trading
    // must not make all of them re-read three lists.
    repo.store_sale(sale(tenant, terminal, unique(), None))
        .await
        .unwrap();
    assert_eq!(
        repo.settings_seq(tenant).await.unwrap(),
        after_customer,
        "trading is not a settings change"
    );

    // And the shop next door has its own counter.
    assert_eq!(repo.settings_seq(unique()).await.unwrap(), 0);
}

#[tokio::test]
async fn what_a_supervisor_waived_survives_the_trip_and_can_be_asked_about() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();
    let day = 1_788_600_000_000_u64;

    let id = unique();
    let mut allowed = sale(tenant, terminal, id, None);
    allowed.rung_at_ms = day + 1_000;
    allowed.overrides = vec![
        "Karim allowed a discount of 1000 basis points".to_owned(),
        "Karim allowed a price to be typed over the catalogue's".to_owned(),
    ];
    repo.store_sale(allowed.clone()).await.unwrap();

    // A replay writes the same rows rather than a second set: a waiver counted
    // twice would read as a supervisor who allowed the same thing twice.
    repo.store_sale(allowed).await.unwrap();

    let seen = repo.waived(tenant, day, day + 10_000, 50).await.unwrap();
    assert_eq!(seen.len(), 2, "both, and only once each");
    assert_eq!(seen[0].sale_id, id);
    assert!(seen[0].reason.contains("Karim"));
    assert_eq!(seen[0].terminal, terminal);

    // Yesterday's shift is not this week's question.
    assert!(
        repo.waived(tenant, day - 100_000, day - 1, 50)
            .await
            .unwrap()
            .is_empty()
    );

    // And the shop next door allows its own.
    assert!(
        repo.waived(unique(), day, day + 10_000, 50)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Asking about many items at once gives the same answer as asking one at a
/// time, item for item.
///
/// The batched query exists because asking one at a time is a transaction and
/// three statements per item, so a till refreshing two hundred items made six
/// hundred round trips and a shop with eight hundred lines took twenty minutes
/// to get round its own catalogue. The figure behind a stock refusal at the far
/// end could be twenty minutes old, and a cashier told the shelf is empty when
/// it is not is a cashier who stops trusting the till.
///
/// It is a widening of one question, not a second question, and this is what
/// makes that true rather than a claim in a comment. Every shape the single
/// answer has to get right is in this shop at once: an item nobody has counted,
/// one counted with sales after it, one counted with a sale that arrived late,
/// one counted and never touched again, one that has never moved at all, and
/// one the shop does not have.
#[tokio::test]
async fn asking_about_many_items_answers_the_same_as_asking_one_at_a_time() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let never_counted = unique();
    let counted_then_sold = unique();
    let counted_with_a_late_sale = unique();
    let counted_and_still = unique();
    let never_moved = unique();
    let not_in_this_shop = unique();

    // Never counted: the running total from the day it appeared.
    let mut early = sale(tenant, terminal, unique(), Some(&receipt()));
    early.rung_at_ms = 1_000;
    early.stock = vec![(never_counted, -10_000)];
    repo.admit_sale(early).await.unwrap();

    for (item, counted_milli) in [
        (counted_then_sold, 40_000_i64),
        (counted_with_a_late_sale, 40_000),
        (counted_and_still, 25_000),
    ] {
        repo.record_count(
            tenant,
            &StockCount {
                id: unique(),
                item_id: item,
                counted_milli,
                counted_at_ms: 5_000,
                counted_by: terminal,
                note: None,
            },
        )
        .await
        .unwrap();
    }

    // Rung after the count, so it moves the figure.
    let mut after = sale(tenant, terminal, unique(), Some(&receipt()));
    after.rung_at_ms = 9_000;
    after.stock = vec![(counted_then_sold, -3_000)];
    repo.admit_sale(after).await.unwrap();

    // Rung before the count and arriving after it, so nobody can say and it is
    // held apart rather than guessed at.
    let mut stranded = sale(tenant, terminal, unique(), Some(&receipt()));
    stranded.rung_at_ms = 1_000;
    stranded.stock = vec![(counted_with_a_late_sale, -2_000)];
    repo.admit_sale(stranded).await.unwrap();

    let wanted = [
        never_counted,
        counted_then_sold,
        counted_with_a_late_sale,
        counted_and_still,
        never_moved,
        not_in_this_shop,
    ];

    let mut one_at_a_time = Vec::new();
    for item in wanted {
        one_at_a_time.push(repo.on_hand(tenant, item).await.unwrap());
    }
    let all_at_once = repo.on_hand_many(tenant, &wanted).await.unwrap();

    assert_eq!(
        all_at_once, one_at_a_time,
        "the batched answer is the same answer, item for item and figure for figure"
    );

    // And it says something worth saying, or the comparison above is two ways
    // of computing nothing.
    assert_eq!(one_at_a_time[0].qty_milli, -10_000);
    assert_eq!(one_at_a_time[1].qty_milli, 37_000);
    assert_eq!(one_at_a_time[2].qty_milli, 40_000);
    assert_eq!(one_at_a_time[2].unreconciled_milli, -2_000);
    assert_eq!(one_at_a_time[2].unreconciled_sales, 1);
    assert_eq!(one_at_a_time[3].qty_milli, 25_000);
    assert_eq!(one_at_a_time[3].counted_at_ms, Some(5_000));
    assert_eq!(one_at_a_time[4].qty_milli, 0);
    assert_eq!(one_at_a_time[4].counted_at_ms, None);

    // Answers come back in the order asked, whatever order the database found
    // them in. A caller matching by position against a reordered answer would
    // put every shelf figure against the wrong item, which is a refusal against
    // the wrong item.
    for (at, item) in wanted.iter().enumerate() {
        assert_eq!(all_at_once[at].item_id, *item);
    }

    // Nothing asked is nothing answered, rather than a query with an empty
    // array in it.
    assert!(repo.on_hand_many(tenant, &[]).await.unwrap().is_empty());
}

/// What the batching bought, measured rather than asserted.
///
/// 200 items, each counted once and sold once, against Postgres in release:
/// one at a time 140.5 ms, all at once 3.5 ms. Forty times, on a database on
/// the same machine with no network between them, which is the flattering case:
/// the loop is six hundred round trips and the batch is two, so the gap widens
/// with every millisecond of latency between the server and its database.
///
/// Ignored by default because it is a measurement and a number that moves with
/// the machine is not something to fail a build on. Run it with
/// `cargo test --release -p openpos-server --test postgres_repo measure_on_hand
/// -- --ignored --nocapture`.
///
/// What it does not buy, and is worth saying plainly: a till still asks about
/// two hundred items every five minutes, so a shop with eight hundred lines
/// still takes twenty minutes to get round its catalogue. That bound is the
/// page size and the cadence, and both are bandwidth decisions on mobile data
/// in Bangladesh rather than database ones. What changed is that the database
/// is no longer the reason they cannot move.
#[tokio::test]
#[ignore = "a measurement, not an assertion"]
async fn measure_on_hand_batching() {
    let repo = database!();
    let (tenant, terminal) = (unique(), unique());
    repo.enrol(tenant, terminal, "Test Shop").await.unwrap();

    let mut items = Vec::new();
    for _ in 0..200 {
        let item = unique();
        let mut one = sale(tenant, terminal, unique(), Some(&receipt()));
        one.rung_at_ms = 1_000;
        one.stock = vec![(item, -1_000)];
        repo.admit_sale(one).await.unwrap();
        repo.record_count(
            tenant,
            &StockCount {
                id: unique(),
                item_id: item,
                counted_milli: 40_000,
                counted_at_ms: 5_000,
                counted_by: terminal,
                note: None,
            },
        )
        .await
        .unwrap();
        items.push(item);
    }

    let began = std::time::Instant::now();
    for item in &items {
        let _ = repo.on_hand(tenant, *item).await.unwrap();
    }
    let looped = began.elapsed();

    let began = std::time::Instant::now();
    let _ = repo.on_hand_many(tenant, &items).await.unwrap();
    let batched = began.elapsed();

    println!("MEASURE 200 items: one at a time {looped:?}, all at once {batched:?}");
}
