//! What the server needs to remember, and an in-memory implementation.
//!
//! The trait exists so ingest can be tested without a database. Postgres is the
//! real implementation; the in-memory one keeps the test suite fast enough to
//! run on every save, which is what makes anybody actually run it.
//!
//! Every method takes a tenant. There is no way to ask this trait a question
//! that is not scoped to one shop, which is the first line of defence against
//! cross-tenant leakage; row-level security in Postgres is the second.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use openpos_core::protocol::{ItemWire, QuarantineReason};

/// How long a terminal credential lasts before it has to be renewed.
///
/// A year. Long enough that a shop is not re-enrolling tablets as a chore, and
/// finite so a tablet sold on, lost, or handed back by a departing employee
/// stops being a working credential for that shop without anyone having to
/// notice. The shops this is for do not have somebody whose job that is.
///
/// Renewal is not built yet, so this is a deadline the product has to meet
/// rather than a setting: see the open item in `todo.md`.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// How long a replaced credential keeps working after renewal.
///
/// A day. The renewal reply can be lost, and a till that had its old credential
/// revoked the moment the server issued a new one would be left holding nothing
/// that authenticates and no way to ask for more: a shop offline until somebody
/// re-enrols the tablet by hand. A day covers a device that renewed, lost the
/// reply, and did not come back online until the next morning.
pub const TOKEN_RENEWAL_OVERLAP: Duration = Duration::from_secs(24 * 60 * 60);

/// When a till should start asking for a replacement.
///
/// Thirty days out. Long enough that a shop offline for a fortnight still gets
/// several chances, and far enough from the expiry that renewal is never urgent.
pub const TOKEN_RENEW_WITHIN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The shape a catalogue payload is written in.
///
/// Bumped whenever `ItemWire` changes, alongside a decoder for the old number.
/// The whole point of storing it is that the old rows stay readable, so raising
/// this without adding that decoder is the mistake it exists to prevent.
///
/// Version 2 since the tax base became a per-item choice: a field added to
/// `ItemWire` cannot be read out of version 1 bytes, and without a bump every
/// stored row would have stopped decoding and every till would have stopped
/// pulling. That is the failure this column was added to prevent, and it took
/// one careless commit to walk into it.
pub const CATALOGUE_SCHEMA: u8 = 2;

use crate::auth::{Caller, Role, Token, TokenHash};

/// A sale as the server keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSale {
    pub tenant: u128,
    pub terminal: u128,
    pub id: u128,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    /// Device clock at the moment of sale. Kept for the receipt and for
    /// ordering within one terminal, never trusted across terminals.
    pub rung_at_ms: u64,
    pub total_minor: i64,
    /// The bytes exactly as the till committed them. Kept verbatim so a dispute
    /// can be settled against what the terminal actually wrote, rather than
    /// against a re-encoding of it.
    pub payload: Vec<u8>,
    /// Set when the sale needs a human. It is still stored either way.
    pub quarantine: Option<QuarantineReason>,
    /// Item id and signed milli-units.
    pub stock: Vec<(u128, i64)>,
    /// What this sale put on somebody's account, read from its tenders. Written
    /// in the same transaction as the sale, so a shop cannot end up holding a
    /// sale on account with nothing saying who owes for it.
    pub on_account: Vec<AccountCharge>,
}

/// What one ticket put on one person's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCharge {
    /// The folded name, which is what balances are summed on.
    pub person_key: String,
    /// The name as the cashier wrote it.
    pub person_name: String,
    /// Positive when the shop is owed.
    pub amount_minor: i64,
}

/// A receipt number block handed to a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseRecord {
    pub tenant: u128,
    pub terminal: u128,
    pub epoch: u64,
    pub first: u64,
    pub last: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoError {
    /// The terminal is not enrolled for this tenant.
    UnknownTerminal,
    /// The backing store refused. Ingest treats this as fatal for the batch: a
    /// till that is told a sale is stored, when it is not, will drop its only
    /// copy.
    Backend,
    /// The caller asked for something the store will not hold. Distinct from
    /// `Backend`, which means try again later: this one will fail identically
    /// forever, and a client that retries it is wasting a shop's connection.
    Invalid,
}

pub type Result<T> = std::result::Result<T, RepoError>;

/// Stock leaving or entering for a reason that is neither a sale nor a
/// delivery: breakage, spoilage, theft, a sample given away, a mistyped count.
///
/// A separate kind from both, because the question a shopkeeper asks at the end
/// of a bad month is which of these it was. Folding them into counts would make
/// every loss look like a counting error, and folding them into sales would put
/// goods nobody paid for into the day's takings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockCorrection {
    pub id: u128,
    pub item_id: u128,
    /// Signed. Negative for goods that left without being sold, positive for a
    /// count that was under.
    pub qty_milli: i64,
    /// Why. Mandatory and free text: an unexplained correction is
    /// indistinguishable from theft when the variance is read a month later,
    /// which is the same reason a cash movement demands one.
    pub reason: String,
    pub occurred_at_ms: u64,
    pub recorded_by: u128,
}

/// A person who may stand at a till, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRecord {
    pub id: u128,
    pub name: String,
    pub pin_salt: Vec<u8>,
    pub pin_rounds: u32,
    pub pin_key: Vec<u8>,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

/// A shop as it appears on its own receipts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShopDetails {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    /// The wallets this shop takes, by the name a report should read. Set once
    /// here rather than typed at a till, where a typo becomes a third wallet.
    pub wallets: Vec<String>,
}

/// Somebody the shop buys from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supplier {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    /// Business Identification Number. Absent for most neighbourhood suppliers,
    /// and a field that insisted would be filled with zeros.
    pub bin: Option<String>,
    pub active: bool,
}

/// One line of a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptLine {
    pub item_id: u128,
    pub qty_milli: i64,
    /// What this delivery cost per unit. Kept per delivery, because the price a
    /// shop paid last Tuesday is what a margin is measured against, and the
    /// item's standing cost is only the most recent guess at it.
    pub unit_cost_minor: i64,
}

/// Goods arriving from a supplier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoodsReceipt {
    pub id: u128,
    pub supplier_id: Option<u128>,
    /// The supplier's own invoice or challan number, which is what a shopkeeper
    /// has in their hand when querying a delivery.
    pub reference: Option<String>,
    /// When the goods arrived, by the clock of whoever recorded it. Decides
    /// which side of a stock count the arrival falls on.
    pub received_at_ms: u64,
    pub received_by: u128,
    pub note: Option<String>,
    pub lines: Vec<ReceiptLine>,
}

/// A count of one item, taken at a moment and superseding everything before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockCount {
    pub id: u128,
    pub item_id: u128,
    pub counted_milli: i64,
    /// Device clock at the moment of counting. Decides which sales this count
    /// should already reflect; never used to order counts between terminals.
    pub counted_at_ms: u64,
    pub counted_by: u128,
    pub note: Option<String>,
}

/// What the shelf holds, and how confident the answer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnHand {
    pub item_id: u128,
    /// The figure to show: the last count plus everything that moved after it,
    /// or the sum of every movement when the item has never been counted.
    pub qty_milli: i64,
    /// When the item was last counted, if ever.
    pub counted_at_ms: Option<u64>,
    /// Sales rung before the last count but which only reached the server after
    /// it, so nobody can say whether the person counting saw those goods.
    ///
    /// Deliberately not folded into `qty_milli`. Applying them decrements stock
    /// the counter may already have seen was gone; ignoring them silently loses
    /// real sales. Neither is detectable later, so the number is carried
    /// separately and shown.
    pub unreconciled_milli: i64,
    /// How many such sales, so a shop can tell one late till from a systemic
    /// problem.
    pub unreconciled_sales: usize,
}

/// What storing a sale actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Stored, and its receipt number is now this sale's.
    Stored,
    /// A sale with this id was already here. Nothing changed, and the caller
    /// should acknowledge it: a replay after a dropped reply is normal.
    AlreadyStored,
    /// Stored, but another sale already holds this receipt number under this
    /// epoch. Named, so a repair queue can say which one rather than only that
    /// something is wrong.
    DuplicateReceipt { held_by: u128 },
}

/// What the server needs to remember.
///
/// Asynchronous, because the real implementation talks to Postgres, and taking
/// `&self` rather than `&mut self`, because a connection pool manages its own
/// concurrency. Requiring `&mut self` would force a lock around the whole
/// server and serialise every shop behind every other one.
///
/// Futures are explicitly `Send` so the handlers can be spawned on a
/// multi-threaded runtime.
pub trait Repository: Send + Sync {
    /// Whether this sale is already stored. Ingest is idempotent, so a replay
    /// after a dropped connection must not create a second sale.
    fn has_sale(&self, tenant: u128, id: u128) -> impl Future<Output = Result<bool>> + Send;

    /// Whether a receipt number is already used, under a given epoch. Two sales
    /// sharing one number means a terminal was restored or cloned.
    fn receipt_taken(
        &self,
        tenant: u128,
        receipt_no: &str,
        epoch: u64,
    ) -> impl Future<Output = Result<bool>> + Send;

    fn store_sale(&self, sale: StoredSale) -> impl Future<Output = Result<()>> + Send;

    /// Store a sale and claim its receipt number in one transaction.
    ///
    /// Replaces asking `has_sale`, then asking `receipt_taken`, then storing:
    /// three transactions with two windows between them. The window that
    /// mattered was the second one, because the case a duplicate check exists
    /// for is a tablet restored from a backup, and a restored tablet pushes its
    /// whole backlog at once beside the device it was copied from. Both reads
    /// said the number was free and both sales stored clean.
    fn admit_sale(&self, sale: StoredSale) -> impl Future<Output = Result<Admission>> + Send;

    /// Whether this terminal belongs to this tenant.
    fn terminal_enrolled(
        &self,
        tenant: u128,
        terminal: u128,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Allocate the next block of receipt numbers for a terminal.
    fn issue_lease(
        &self,
        tenant: u128,
        terminal: u128,
        count: u32,
    ) -> impl Future<Output = Result<LeaseRecord>> + Send;

    /// Resolve a presented token to the terminal that owns it.
    ///
    /// Returns `None` for an unknown or revoked token. Deliberately not an
    /// error: an attacker probing tokens learns nothing from the difference
    /// between "no such token" and "that one was revoked".
    fn authenticate(
        &self,
        token: &TokenHash,
    ) -> impl Future<Output = Result<Option<Caller>>> + Send;

    /// Attach a freshly issued token to a terminal.
    fn store_token(
        &self,
        caller: Caller,
        token: &TokenHash,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Issue a replacement credential and set the old one to lapse shortly.
    ///
    /// Both in one transaction, and the old one deliberately not revoked
    /// outright. A reply can be lost, and a device that acted on a revocation it
    /// never received would hold nothing that authenticates and no way to ask
    /// for more. The overlap is what makes renewal safe to retry.
    fn renew_token(
        &self,
        caller: Caller,
        previous: &TokenHash,
        replacement: &TokenHash,
        overlap: Duration,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Record a count of one item. The barrier is the moment the server stores
    /// it.
    fn record_count(
        &self,
        tenant: u128,
        count: &StockCount,
    ) -> impl Future<Output = Result<()>> + Send;

    /// What the shelf holds for one item, counted from the last barrier.
    fn on_hand(&self, tenant: u128, item: u128) -> impl Future<Output = Result<OnHand>> + Send;

    /// The people who may stand at a till in this shop.
    fn operators(&self, tenant: u128) -> impl Future<Output = Result<Vec<OperatorRecord>>> + Send;

    /// Change a person without touching their PIN: their name, what they may
    /// do, and whether they may sign in at all.
    ///
    /// Refuses when nobody by that id is there, rather than quietly writing
    /// nothing: an owner who suspends the wrong person and is told it worked
    /// has been told a lie about who can open the drawer.
    fn amend_operator(
        &self,
        tenant: u128,
        amended: &AmendedOperator,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Give somebody a new PIN, touching nothing else about them.
    ///
    /// Refuses when nobody by that id is there, and refuses a round count that
    /// would make the hash cheap: a credential written with a thousandth of the
    /// work is a credential somebody can guess offline, and it would be written
    /// once and trusted for years.
    fn set_operator_pin(
        &self,
        tenant: u128,
        operator_id: u128,
        salt: &[u8],
        rounds: u32,
        key: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Add or update one.
    fn put_operator(
        &self,
        tenant: u128,
        operator: &OperatorRecord,
    ) -> impl Future<Output = Result<()>> + Send;

    /// The shop's own details, for the top of a receipt.
    fn shop_details(&self, tenant: u128) -> impl Future<Output = Result<ShopDetails>> + Send;

    /// Set them.
    fn put_shop_details(
        &self,
        tenant: u128,
        details: &ShopDetails,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Add or update a supplier.
    fn put_supplier(
        &self,
        tenant: u128,
        supplier: &Supplier,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Suppliers a shop buys from, by name.
    fn suppliers(&self, tenant: u128) -> impl Future<Output = Result<Vec<Supplier>>> + Send;

    /// Record a delivery and move its stock, in one transaction.
    ///
    /// Idempotent on the receipt id, so a back office that retries after a
    /// dropped reply does not book the same delivery twice. Returns whether
    /// anything was written.
    /// Store drawers a till has counted and closed. Returns every id the server
    /// now holds, including ones it already had: a repeat is ordinary, because a
    /// dropped reply is the usual reason a till sends one twice.
    fn put_shifts(
        &self,
        tenant: u128,
        shifts: &[ClosedShift],
    ) -> impl Future<Output = Result<Vec<u128>>> + Send;

    /// The drawers this shop has closed lately, newest first.
    fn closed_shifts(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ClosedShift>>> + Send;

    /// Take money off what somebody owes. Idempotent by payment id, because a
    /// dropped reply is the usual reason one is sent twice and a payment
    /// counted twice is money the shop believes it has been given.
    fn take_payment(
        &self,
        tenant: u128,
        payment: &AccountPayment,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Who owes the shop, most owed first. Settled accounts are not listed.
    fn owed(&self, tenant: u128, limit: u32) -> impl Future<Output = Result<Vec<Owing>>> + Send;

    /// What one person owes, asked directly. A screen that has just taken a
    /// payment needs this one number and must not get it by paging a list it
    /// might not be on.
    fn balance(&self, tenant: u128, person_key: &str) -> impl Future<Output = Result<i64>> + Send;

    /// One person's account, newest first, which is what an owner reads out
    /// when somebody disputes the total.
    fn account(
        &self,
        tenant: u128,
        person_key: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<AccountEntry>>> + Send;

    /// The most recent deliveries, newest first.
    ///
    /// Read back because a delivery filed under a supplier is only useful if
    /// somebody can ask which goods came on which challan, which is the
    /// question asked when the invoice and the shelf disagree.
    /// What the shop took between two moments, by till.
    ///
    /// From the sale headers rather than the payloads: the total and the time
    /// are columns, and decoding every ticket to add them up would make the
    /// question an owner asks most often the one that costs most to answer.
    fn takings(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<Vec<TakingsRow>>> + Send;

    fn deliveries(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<GoodsReceipt>>> + Send;

    fn receive_goods(
        &self,
        tenant: u128,
        receipt: &GoodsReceipt,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Record a stock correction and move the stock, in one transaction.
    ///
    /// Idempotent on the correction id. Returns whether anything was written.
    fn correct_stock(
        &self,
        tenant: u128,
        correction: &StockCorrection,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Create a terminal row for a device that does not exist yet.
    ///
    /// On the trait rather than inherent on each store, because issuing an
    /// enrolment code has to create the terminal the code names: a redeemed
    /// code pointing at a terminal nobody created fails at the worst possible
    /// moment, with a shop standing there holding a new tablet.
    fn register_terminal(
        &self,
        tenant: u128,
        terminal: u128,
        label: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Attach a credential carrying a stated role. Separate from `store_token`
    /// so the ordinary path cannot mint an owner by forgetting an argument.
    fn store_token_as(
        &self,
        caller: Caller,
        token: &TokenHash,
        role: Role,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Withdraw one credential. Returns whether anything was withdrawn.
    fn revoke_token(&self, token: &TokenHash) -> impl Future<Output = Result<bool>> + Send;

    /// Withdraw every credential a terminal holds, which is what a shop needs
    /// the moment a tablet is lost or stolen. Returns how many were withdrawn.
    fn revoke_all_tokens(&self, caller: Caller) -> impl Future<Output = Result<usize>> + Send;

    /// Offer a short code that can be exchanged for a credential.
    /// Issue a code that will grant `grants` when redeemed.
    ///
    /// The parameter is the identity the code hands out, not the identity of
    /// whoever asked for it. Those were the same thing when a code only ever
    /// re-enrolled the device that asked, and conflating them now would mean a
    /// new tablet inheriting the identity of the one that requested its code.
    fn issue_enrolment_code(
        &self,
        grants: Caller,
        code: &TokenHash,
        valid_for: Duration,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Exchange a code for the terminal it names, consuming it.
    ///
    /// Returns `None` for a code that is unknown, expired or already used. The
    /// three are indistinguishable to the caller on purpose: an attacker
    /// guessing codes learns nothing from being told which of those it hit.
    fn redeem_enrolment_code(
        &self,
        code: &TokenHash,
    ) -> impl Future<Output = Result<Option<Caller>>> + Send;

    /// Catalogue changes after `cursor`, oldest first.
    ///
    /// Returns the upserts, the ids of deleted items, the cursor after this
    /// batch, and whether more is waiting. Tombstones travel explicitly: without
    /// them a deleted item lingers on every till that already has it.
    fn items_since(
        &self,
        tenant: u128,
        cursor: u64,
        limit: u32,
    ) -> impl Future<Output = Result<CataloguePage>> + Send;

    // -- Bulk read, for taking a shop out ----------------------------------
    //
    // Every reader is paged and takes the key it left off at rather than an
    // offset. A shop with a year of sales must not be one query, and an offset
    // would make the last page re-scan everything before it.

    /// The shop's own row, or `None` if there is no such shop.
    fn tenant_record(
        &self,
        tenant: u128,
    ) -> impl Future<Output = Result<Option<TenantRecord>>> + Send;

    /// Every terminal, credentials excluded. Unpaged, because a shop has a
    /// counter's worth of them and never a year's worth.
    fn terminal_records(
        &self,
        tenant: u128,
    ) -> impl Future<Output = Result<Vec<TerminalRecord>>> + Send;

    /// Catalogue changes after `after_seq`, oldest first.
    fn catalogue_after(
        &self,
        tenant: u128,
        after_seq: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<CatalogueRecord>>> + Send;

    /// Sales after `after_id`, in id order.
    ///
    /// Ordered by id rather than by arrival, because id order is stable: a page
    /// boundary cannot shift under a concurrent write the way an ordering by
    /// timestamp can, which would skip or repeat a sale mid-export.
    fn sales_after(
        &self,
        tenant: u128,
        after_id: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<SaleRecord>>> + Send;

    /// Stock movements after the given (sale, item) pair, in that order.
    fn stock_after(
        &self,
        tenant: u128,
        after: (u128, u128),
        limit: u32,
    ) -> impl Future<Output = Result<Vec<StockRecord>>> + Send;

    /// Counted drawers, in id order, for an export.
    fn shifts_after(
        &self,
        tenant: u128,
        after: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ClosedShift>>> + Send;

    /// The account book, in key order, for an export.
    fn account_after(
        &self,
        tenant: u128,
        after: (u128, String),
        limit: u32,
    ) -> impl Future<Output = Result<Vec<AccountRecord>>> + Send;

    // -- Bulk write, for putting one back ----------------------------------
    //
    // Every writer is idempotent. Import is a thing operators run twice, once
    // because the first attempt appeared to hang, so a second run must not
    // double a shop's takings.

    /// Create the shop, or raise an existing row to cover this bundle.
    fn put_tenant(&self, record: &TenantRecord) -> impl Future<Output = Result<()>> + Send;

    /// Returns how many terminal rows were written.
    fn put_terminals(
        &self,
        tenant: u128,
        records: &[TerminalRecord],
    ) -> impl Future<Output = Result<usize>> + Send;

    /// Returns how many changes were new. Also raises the shop's catalogue
    /// counter past everything written, so a later edit cannot mint a sequence
    /// number an imported row already holds.
    fn put_catalogue(
        &self,
        tenant: u128,
        records: &[CatalogueRecord],
    ) -> impl Future<Output = Result<usize>> + Send;

    /// Returns how many sales were new, which is zero on a second import.
    fn put_sales(
        &self,
        tenant: u128,
        records: &[SaleRecord],
    ) -> impl Future<Output = Result<usize>> + Send;

    /// Returns how many movements were new.
    fn put_stock(
        &self,
        tenant: u128,
        records: &[StockRecord],
    ) -> impl Future<Output = Result<usize>> + Send;

    /// Put account entries back, exactly as they were written.
    fn put_account(
        &self,
        tenant: u128,
        records: &[AccountRecord],
    ) -> impl Future<Output = Result<usize>> + Send;

    // -- Back office -------------------------------------------------------

    /// Sales still waiting on a human, oldest first.
    ///
    /// Oldest first because the queue is worked from the top and the oldest
    /// entry is the one whose evidence is decaying: the customer who disputes a
    /// receipt is remembered for a week, not a quarter.
    fn repair_queue(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RepairItem>>> + Send;

    /// Take one sale out of the queue, recording what was decided.
    ///
    /// Returns whether anything moved. Resolving twice is not an error, because
    /// two people working the same queue is the normal case and the second one
    /// should be told "already done" rather than shown a failure.
    ///
    /// The sale itself is never altered or removed. It happened, and the stored
    /// bytes are what a dispute is settled against.
    fn resolve_quarantine(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Every terminal in the shop, with what support needs to triage it.
    fn terminal_health(
        &self,
        tenant: u128,
    ) -> impl Future<Output = Result<Vec<TerminalHealth>>> + Send;

    /// Record that this terminal was heard from just now.
    ///
    /// Separate from the work of a sync rather than folded into it, so a till
    /// that syncs an empty batch still counts as alive. A device that stopped
    /// selling and a device that stopped talking need different visits, and one
    /// timestamp per successful sync is what tells them apart.
    fn mark_terminal_seen(
        &self,
        tenant: u128,
        terminal: u128,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Record a catalogue upsert, returning the sequence it landed at.
    fn upsert_item(
        &self,
        tenant: u128,
        item: &ItemWire,
    ) -> impl Future<Output = Result<u64>> + Send;

    /// Record a catalogue deletion, returning the sequence it landed at.
    fn delete_item(&self, tenant: u128, item_id: u128) -> impl Future<Output = Result<u64>> + Send;
}

/// One page of catalogue changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CataloguePage {
    /// Rows this build could not decode and passed over.
    ///
    /// Surfaced rather than swallowed: a shop whose catalogue is quietly
    /// missing changes needs somebody to know, and failing the page instead
    /// would stop every till in that shop syncing at all.
    pub skipped: usize,
    pub upserts: Vec<ItemWire>,
    pub tombstones: Vec<u128>,
    pub cursor: u64,
    pub more: bool,
}

/// A shop's own row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantRecord {
    pub id: u128,
    pub name: String,
    /// Where the shop's catalogue counter stands. It travels because a till's
    /// pull cursor is a position in this sequence, and a restore that reset the
    /// counter would hand the next edit a number some till already believes it
    /// has seen.
    pub catalogue_seq: u64,
}

/// A terminal as the shop's own record of it.
///
/// None of these rows carry the tenant they belong to. The tenant is a
/// parameter of every call that reads or writes one, so a field repeating it
/// could only ever disagree with the transaction it travelled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRecord {
    pub id: u128,
    pub label: String,
    pub epoch: u64,
    pub next_receipt: u64,
}

/// One catalogue change exactly as stored, payload bytes and all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueRecord {
    pub seq: u64,
    /// 1 upsert, 2 delete, matching the column.
    pub kind: i16,
    pub item_id: u128,
    /// postcard-encoded `ItemWire` for an upsert, `None` for a delete. Carried
    /// as bytes rather than decoded and re-encoded, so a change written by a
    /// newer build survives a round trip through an older one.
    pub payload: Option<Vec<u8>>,
    /// Which shape those bytes are in. Travels with them through export and
    /// import, so a bundle taken from a newer build does not arrive claiming to
    /// be something this one wrote.
    pub schema: u8,
}

/// A sale as stored, for bulk read and bulk write.
///
/// Distinct from [`StoredSale`] in one way that matters: the quarantine reason
/// is the rendered text the database holds, not the enum. The text cannot be
/// parsed back into a [`QuarantineReason`], so an export that carried the enum
/// would have to drop the reason, and a restore would silently empty a shop's
/// repair queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleRecord {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: Option<String>,
    pub receipt_epoch: Option<u64>,
    pub rung_at_ms: u64,
    pub total_minor: i64,
    pub payload: Vec<u8>,
    pub quarantine: Option<String>,
}

/// Why an entry came off somebody's account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// Money handed over.
    Paid,
    /// Taken off without money: a sale rung twice on a till restored from a
    /// backup, goods brought back, an argument settled. Told apart from a
    /// payment so that money taken and money written off are never added
    /// together, and never allowed without a note.
    WrittenOff,
}

/// Money taken off what somebody owes.
///
/// Its own act rather than an edit to the sale that created the debt: the sale
/// happened and does not change, and a shop that settles half an account on
/// Friday and the rest on Monday has two payments to show for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPayment {
    /// Minted by whoever took the payment, so a resent one is not counted twice.
    pub id: u128,
    /// Money handed over, or a debt struck off.
    pub kind: Settlement,
    pub person_key: String,
    pub person_name: String,
    /// What was handed over. Positive.
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: Option<String>,
}

/// What one person owes, and what it is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owing {
    pub person_key: String,
    /// The most recent spelling of the name.
    pub person_name: String,
    /// Positive is owed to the shop. Negative means they are in credit, which
    /// happens when somebody pays more than they owed and is worth showing
    /// rather than hiding.
    pub owed_minor: i64,
    /// When this account was first written in. Not when the current balance
    /// started: an account settled in March and used again in July still says
    /// March, because the entries either side of the zero are one person's
    /// history rather than two.
    pub since_ms: u64,
    pub last_at_ms: u64,
    pub entries: u32,
}

/// One line of somebody's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEntry {
    pub source_id: u128,
    /// True when this is a sale, false when it came off the account.
    pub is_sale: bool,
    /// True when it came off without money changing hands. Money taken and
    /// money written off are never added together.
    pub written_off: bool,
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: String,
}

/// Put a sale's account entries in the book.
///
/// Keyed on the sale and the person, so a till resending a sale it was not told
/// about does not double what somebody owes.
fn charge_accounts(inner: &mut Inner, sale: &StoredSale) {
    for charge in &sale.on_account {
        inner
            .accounts
            .entry((sale.tenant, sale.id, charge.person_key.clone()))
            .or_insert_with(|| AccountEntryRow {
                person_key: charge.person_key.clone(),
                person_name: charge.person_name.clone(),
                source_id: sale.id,
                is_sale: true,
                written_off: false,
                amount_minor: charge.amount_minor,
                at_ms: sale.rung_at_ms,
                note: String::new(),
            });
    }
}

/// What a row of the book is, in the numbers the table uses.
fn kind_of(row: &AccountEntryRow) -> i16 {
    if row.is_sale {
        1
    } else if row.written_off {
        3
    } else {
        2
    }
}

/// One row of the account book as the memory store holds it, matching the
/// table: the person, what put it there, and how much.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountEntryRow {
    person_key: String,
    person_name: String,
    source_id: u128,
    is_sale: bool,
    written_off: bool,
    amount_minor: i64,
    at_ms: u64,
    note: String,
}

/// A drawer that was counted and closed.
///
/// Immutable once stored: it is a statement about a period that has ended, and
/// a correction belongs in the next period as a cash movement rather than as an
/// edit to this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedShift {
    pub id: u128,
    pub terminal: u128,
    /// Who counted it, and what they were called at the time.
    pub closed_by: u128,
    pub closed_by_name: String,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
}

/// What may be changed about a person without knowing their PIN.
///
/// Deliberately not `OperatorRecord`: that one carries the derived key, and a
/// caller holding this cannot produce one. The type is the reason the PIN
/// cannot be touched here rather than a comment asking nobody to touch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmendedOperator {
    pub id: u128,
    pub name: String,
    pub max_discount_bp: u32,
    pub may_override_price: bool,
    pub may_refund: bool,
    pub may_void_line: bool,
    pub may_authorise: bool,
    pub may_open_drawer: bool,
    pub may_close_shift: bool,
    pub active: bool,
}

/// One till's part of a period's takings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakingsRow {
    pub terminal: u128,
    pub sales: u64,
    pub total_minor: i64,
    /// Sales the server quarantined, which are in the totals: the goods left the
    /// shop and the money changed hands, and a figure that omitted them would
    /// disagree with the drawer.
    pub needing_attention: u64,
    /// How many of the sales were refunds, and what they came to. A refund is a
    /// sale with the signs turned round, so it is already in the total; counted
    /// separately because a quiet day and a busy day with returns are not the
    /// same day.
    pub refunds: u64,
    pub refunded_minor: i64,
}

/// One line of the account book, as it travels in an export.
///
/// Carried whole rather than re-read from the sale payloads: a payment is not in
/// any payload, and a shop restored onto another machine that arrives with its
/// sales and none of what anybody owes it has lost the part it cannot
/// reconstruct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub person_key: String,
    pub person_name: String,
    /// The sale that created the debt, or the payment or write-off that reduced
    /// it.
    pub source: u128,
    /// 1 sale on account, 2 payment taken, 3 written off.
    pub kind: i16,
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: String,
}

/// One stock movement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockRecord {
    /// What caused the movement: a sale, a goods receipt, or a correction.
    pub source: u128,
    /// Which of those it was. See `stock_movement.source_kind`.
    pub source_kind: i16,
    pub item: u128,
    pub qty_milli: i64,
    /// When it happened, by the clock of whoever recorded it. Carried on the
    /// movement rather than fetched from the sale, because a goods receipt has
    /// no sale to fetch it from.
    pub occurred_at_ms: u64,
}

/// One sale in the repair queue.
///
/// Carries the payload's summary rather than the payload. The queue is a list a
/// person scans; whoever needs the bytes fetches the sale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairItem {
    pub id: u128,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    pub received_at_ms: u64,
    pub reason: String,
}

/// One terminal, as support sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalHealth {
    pub terminal: u128,
    pub label: String,
    pub epoch: u64,
    pub enrolled_at_ms: u64,
    /// `None` for a terminal not heard from since the column existed. Absent
    /// rather than zero, because zero would render as 1970 and read as a fault.
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    pub open_repairs: u64,
}

/// Turn a quarantine reason into the sentence a shopkeeper reads.
///
/// Stored and returned as text rather than as a structured code. It is read by a
/// human deciding what to do about a sale, never queried on, and text cannot
/// drift out of step with the enum the way a numeric code would after a release
/// that adds a variant. Both repositories call this, so the queue reads the same
/// whether it is served from Postgres or from memory.
#[must_use]
pub fn describe_quarantine(reason: &QuarantineReason) -> String {
    match reason {
        QuarantineReason::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        } => format!(
            "totals mismatch: the till stored {stored_minor} and the server recomputed {recomputed_minor}"
        ),
        QuarantineReason::DuplicateReceiptNumber { receipt_no } => {
            format!("receipt number {receipt_no} was already used by another sale")
        }
        QuarantineReason::Undecodable => {
            "the payload could not be decoded under the schema it claimed".to_owned()
        }
        QuarantineReason::CarriedIn => {
            "carried in by hand from a device that could not send it".to_owned()
        }
    }
}

/// In-memory store for tests.
///
/// Interior mutability, so it satisfies the same `&self` interface Postgres
/// does. The lock lives inside one shop's store rather than around the whole
/// server.
#[derive(Debug, Default)]
pub struct MemoryRepo {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Shops, by id, with their names. A shop exists here for the same reason
    /// it has a row in Postgres: so asking to export one that was never created
    /// is answerable with "no such shop" rather than with an empty bundle.
    tenants: HashMap<u128, String>,
    sales: HashMap<(u128, u128), StoredSale>,
    /// Quarantine reasons in the form the database keeps them, rendered text.
    /// Kept beside the sale rather than inside it because a sale that arrives by
    /// import has text and no enum, and losing it would empty a repair queue.
    quarantine: HashMap<(u128, u128), String>,
    /// When each sale arrived, keyed as the sales are. Kept beside them rather
    /// than inside `StoredSale`, because that struct is what ingest builds from
    /// a till's own bytes and arrival is the server's fact, not the till's.
    received: HashMap<(u128, u128), u64>,
    /// Notes left on resolved quarantines, keyed by tenant and sale. Presence is
    /// what takes an entry out of the queue; the sale itself is never touched.
    resolutions: HashMap<(u128, u128), String>,
    receipts: HashSet<(u128, String, u64)>,
    /// People, by tenant and operator id.
    operators: HashMap<(u128, u128), OperatorRecord>,
    /// Shop details, by tenant.
    shops: HashMap<u128, ShopDetails>,
    /// Suppliers, by tenant and supplier id.
    suppliers: HashMap<(u128, u128), Supplier>,
    /// Deliveries, by tenant and receipt id.
    deliveries: HashMap<(u128, u128), GoodsReceipt>,
    shifts: HashMap<(u128, u128), ClosedShift>,
    /// The account book, keyed as the table is: one row per person per source,
    /// so a replayed sale and a resent payment both cost nothing.
    accounts: HashMap<(u128, u128, String), AccountEntryRow>,
    /// Corrections, by tenant and correction id.
    corrections: HashMap<(u128, u128), StockCorrection>,
    /// Counts taken, by tenant and count id.
    counts: HashMap<(u128, u128), StockCount>,
    /// Which sale holds each receipt number, under which epoch. Mirrors the
    /// `receipt_claim` primary key: the claim is what decides a duplicate, and
    /// naming the holder lets a repair queue say which other sale rather than
    /// only that something is wrong.
    claims: HashMap<(u128, String, u64), u128>,
    /// Enrolled terminals and what the back office knows about each. A map
    /// rather than a set plus a parallel label table, because two collections
    /// keyed the same way can fall out of step and leave a terminal that is
    /// enrolled but nameless in the health list.
    terminals: HashMap<(u128, u128), TerminalState>,
    /// Next unissued number per terminal, and its epoch.
    counters: HashMap<(u128, u128), (u64, u64)>,
    /// Catalogue changes by sequence number, which is what a till replays.
    /// Keyed by the sequence rather than held in a vector, because an imported
    /// log need not start at one or be contiguous, and a positional store would
    /// answer a till's cursor with the wrong change.
    changes: HashMap<u128, BTreeMap<u64, CatalogueChange>>,
    /// Where each shop's catalogue counter stands.
    catalogue_seq: HashMap<u128, u64>,
    tokens: HashMap<TokenHash, Caller>,
    codes: HashMap<TokenHash, (Caller, SystemTime)>,
}

/// One catalogue change, as the server records it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CatalogueChange {
    Upsert(Box<ItemWire>),
    Delete(u128),
}

/// What the in-memory store keeps about an enrolled terminal.
///
/// Deliberately not [`TerminalRecord`]: that one is the shape a shop travels in
/// and carries receipt counters, while this one carries the dates support reads.
/// Keeping them apart stops an export from shipping a machine's last-seen clock
/// as if it were part of the shop's books.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalState {
    label: String,
    enrolled_at_ms: u64,
    last_seen_ms: Option<u64>,
}

/// Wall clock in milliseconds.
///
/// Saturates instead of failing. A clock set before 1970 is a misconfigured
/// machine, and refusing to answer a health question over it would hide the very
/// state an operator is trying to see.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl MemoryRepo {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A poisoned lock means a test panicked while holding it. Recover the data
    /// rather than cascading the panic: the store itself is still coherent.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Enrol a terminal, as the back office would.
    pub fn enrol(&self, tenant: u128, terminal: u128) {
        self.enrol_labelled(tenant, terminal, "");
    }

    /// Enrol a terminal under a name a person would recognise.
    ///
    /// The label is what the health list is read by. A support call starts with
    /// "the one by the door", not with a 128-bit identifier.
    pub fn enrol_labelled(&self, tenant: u128, terminal: u128, label: &str) {
        let mut inner = self.lock();
        // The shop itself is recorded too, so exporting a shop that was only
        // ever enrolled into answers with its row rather than "no such shop".
        inner.tenants.entry(tenant).or_default();
        // Enrolling again keeps the original date, matching the `on conflict do
        // nothing` the Postgres store uses. A terminal that re-enrols has not
        // become a new device, and rewriting the date would erase how long it
        // has been in the shop.
        let record = inner
            .terminals
            .entry((tenant, terminal))
            .or_insert_with(|| TerminalState {
                label: String::new(),
                enrolled_at_ms: now_ms(),
                last_seen_ms: None,
            });
        if !label.is_empty() {
            record.label = label.to_owned();
        }
        inner.counters.entry((tenant, terminal)).or_insert((1, 1));
    }

    /// Enrol a terminal and hand back its credential, as the back office does.
    pub fn enrol_with_token(&self, tenant: u128, terminal: u128) -> Token {
        self.enrol(tenant, terminal);
        let token = Token::generate();
        self.lock()
            .tokens
            // The first credential a shop gets is an owner's: somebody has to
            // be able to mint the rest.
            .insert(
                token.hash(),
                Caller {
                    tenant,
                    terminal,
                    role: Role::Owner,
                },
            );
        token
    }

    /// Bump a terminal's epoch, as the back office does when it believes a
    /// device was replaced or restored from a backup.
    pub fn bump_epoch(&self, tenant: u128, terminal: u128) {
        if let Some((_, epoch)) = self.lock().counters.get_mut(&(tenant, terminal)) {
            *epoch = epoch.saturating_add(1);
        }
    }

    /// Record a catalogue change, as the back office would.
    ///
    /// Shadows the trait method of the same name, on purpose. Tests build a
    /// shop's catalogue synchronously before a runtime exists, and an inherent
    /// method wins method resolution, so those call sites keep working while the
    /// asynchronous trait method serves the HTTP route.
    pub fn upsert_item(&self, tenant: u128, item: ItemWire) -> u64 {
        self.append_change(tenant, CatalogueChange::Upsert(Box::new(item)))
    }

    /// Record a deletion. Shadows the trait method, for the reason above.
    pub fn delete_item(&self, tenant: u128, id: u128) -> u64 {
        self.append_change(tenant, CatalogueChange::Delete(id))
    }

    fn append_change(&self, tenant: u128, change: CatalogueChange) -> u64 {
        let mut inner = self.lock();
        let seq = inner
            .catalogue_seq
            .entry(tenant)
            .or_default()
            .saturating_add(1);
        inner.catalogue_seq.insert(tenant, seq);
        inner.changes.entry(tenant).or_default().insert(seq, change);
        seq
    }

    #[must_use]
    pub fn sale(&self, tenant: u128, id: u128) -> Option<StoredSale> {
        self.lock().sales.get(&(tenant, id)).cloned()
    }

    #[must_use]
    pub fn sale_count(&self, tenant: u128) -> usize {
        self.lock()
            .sales
            .keys()
            .filter(|(owner, _)| *owner == tenant)
            .count()
    }

    /// Give a shop the details that head its receipts, synchronously.
    pub fn put_shop_details_for_test(
        &self,
        tenant: u128,
        name: &str,
        bin: Option<&str>,
        address: Option<&str>,
    ) {
        let mut inner = self.lock();
        inner.tenants.insert(tenant, name.to_owned());
        inner.shops.insert(
            tenant,
            ShopDetails {
                name: name.to_owned(),
                bin: bin.map(ToOwned::to_owned),
                address: address.map(ToOwned::to_owned),
                phone: None,
                wallets: Vec::new(),
            },
        );
    }

    /// Every stored sale for a tenant, oldest id first. For tests.
    #[must_use]
    pub fn sales(&self, tenant: u128) -> Vec<StoredSale> {
        let inner = self.lock();
        let mut found: Vec<StoredSale> = inner
            .sales
            .iter()
            .filter(|(key, _)| key.0 == tenant)
            .map(|(_, sale)| sale.clone())
            .collect();
        found.sort_by_key(|sale| sale.id);
        found
    }

    /// Every quarantined sale, which is what the repair queue lists.
    ///
    /// Read from the rendered reason rather than from the enum, so a sale that
    /// arrived by import is in the queue too.
    #[must_use]
    pub fn quarantined(&self, tenant: u128) -> Vec<StoredSale> {
        let inner = self.lock();
        let mut found: Vec<StoredSale> = inner
            .sales
            .iter()
            .filter(|(key, _)| key.0 == tenant && inner.quarantine.contains_key(*key))
            .map(|(_, sale)| sale.clone())
            .collect();
        found.sort_by_key(|sale| sale.id);
        found
    }
}

impl Repository for MemoryRepo {
    async fn has_sale(&self, tenant: u128, id: u128) -> Result<bool> {
        Ok(self.lock().sales.contains_key(&(tenant, id)))
    }

    async fn receipt_taken(&self, tenant: u128, receipt_no: &str, epoch: u64) -> Result<bool> {
        Ok(self
            .lock()
            .receipts
            .contains(&(tenant, receipt_no.to_owned(), epoch)))
    }

    async fn store_sale(&self, sale: StoredSale) -> Result<()> {
        let mut inner = self.lock();
        if let (Some(receipt), Some(epoch)) = (sale.receipt_no.clone(), sale.receipt_epoch) {
            inner.receipts.insert((sale.tenant, receipt, epoch));
        }
        if let Some(reason) = sale.quarantine.as_ref() {
            inner
                .quarantine
                .insert((sale.tenant, sale.id), describe_quarantine(reason));
        }
        // Arrival is recorded once. A replay stores the same sale again, and the
        // queue should keep showing when it first landed rather than moving to
        // the bottom every time a till retries.
        inner
            .received
            .entry((sale.tenant, sale.id))
            .or_insert_with(now_ms);
        charge_accounts(&mut inner, &sale);
        inner.sales.insert((sale.tenant, sale.id), sale);
        Ok(())
    }

    async fn admit_sale(&self, mut sale: StoredSale) -> Result<Admission> {
        // One lock spans the whole decision, which is what the Postgres side
        // achieves with one transaction and a primary key.
        let mut inner = self.lock();
        if inner.sales.contains_key(&(sale.tenant, sale.id)) {
            return Ok(Admission::AlreadyStored);
        }

        let mut admission = Admission::Stored;
        if let (Some(receipt), Some(epoch)) = (sale.receipt_no.clone(), sale.receipt_epoch) {
            let key = (sale.tenant, receipt.clone(), epoch);
            match inner.claims.get(&key) {
                Some(&held_by) => {
                    admission = Admission::DuplicateReceipt { held_by };
                    let reason = QuarantineReason::DuplicateReceiptNumber {
                        receipt_no: receipt,
                    };
                    inner
                        .quarantine
                        .insert((sale.tenant, sale.id), describe_quarantine(&reason));
                    sale.quarantine = Some(reason);
                }
                None => {
                    inner.claims.insert(key.clone(), sale.id);
                    inner.receipts.insert(key);
                }
            }
        }

        if let Some(reason) = sale.quarantine.as_ref() {
            inner
                .quarantine
                .insert((sale.tenant, sale.id), describe_quarantine(reason));
        }
        inner
            .received
            .entry((sale.tenant, sale.id))
            .or_insert_with(now_ms);
        charge_accounts(&mut inner, &sale);
        inner.sales.insert((sale.tenant, sale.id), sale);
        Ok(admission)
    }

    async fn terminal_enrolled(&self, tenant: u128, terminal: u128) -> Result<bool> {
        Ok(self.lock().terminals.contains_key(&(tenant, terminal)))
    }

    async fn authenticate(&self, token: &TokenHash) -> Result<Option<Caller>> {
        Ok(self.lock().tokens.get(token).copied())
    }

    async fn store_token(&self, caller: Caller, token: &TokenHash) -> Result<()> {
        self.lock().tokens.insert(token.clone(), caller);
        Ok(())
    }

    async fn renew_token(
        &self,
        caller: Caller,
        previous: &TokenHash,
        replacement: &TokenHash,
        _overlap: Duration,
    ) -> Result<()> {
        // The in-memory store keeps no expiry, so the overlap is simply that the
        // old token is left in place. Postgres is where the lapse is real.
        let mut inner = self.lock();
        inner.tokens.insert(replacement.clone(), caller);
        let _ = previous;
        Ok(())
    }

    async fn record_count(&self, tenant: u128, count: &StockCount) -> Result<()> {
        self.lock().counts.insert((tenant, count.id), count.clone());
        Ok(())
    }

    async fn on_hand(&self, tenant: u128, item: u128) -> Result<OnHand> {
        let inner = self.lock();

        // The newest count by the device clock is the one that supersedes the
        // others; a count taken later describes a later shelf.
        let latest = inner
            .counts
            .iter()
            .filter(|((owner, _), count)| *owner == tenant && count.item_id == item)
            .map(|(_, count)| count)
            .max_by_key(|count| count.counted_at_ms);

        let corrected = |from_ms: Option<u64>| -> i64 {
            inner
                .corrections
                .iter()
                .filter(|((owner, _), _)| *owner == tenant)
                .filter(|(_, entry)| entry.item_id == item)
                .filter(|(_, entry)| from_ms.is_none_or(|at| entry.occurred_at_ms >= at))
                .fold(0_i64, |total, (_, entry)| {
                    total.saturating_add(entry.qty_milli)
                })
        };

        let received = |from_ms: Option<u64>| -> i64 {
            inner
                .deliveries
                .iter()
                .filter(|((owner, _), _)| *owner == tenant)
                .filter(|(_, receipt)| from_ms.is_none_or(|at| receipt.received_at_ms >= at))
                .flat_map(|(_, receipt)| receipt.lines.iter())
                .filter(|line| line.item_id == item)
                .fold(0_i64, |total, line| total.saturating_add(line.qty_milli))
        };

        let Some(count) = latest else {
            // Never counted, so there is no barrier and the running total is the
            // best available answer.
            let sold = inner
                .sales
                .iter()
                .filter(|((owner, _), _)| *owner == tenant)
                .flat_map(|(_, sale)| sale.stock.iter())
                .filter(|(moved, _)| *moved == item)
                .fold(0_i64, |total, (_, qty)| total.saturating_add(*qty));
            let qty = sold
                .saturating_add(received(None))
                .saturating_add(corrected(None));
            return Ok(OnHand {
                item_id: item,
                qty_milli: qty,
                counted_at_ms: None,
                unreconciled_milli: 0,
                unreconciled_sales: 0,
            });
        };

        // The in-memory store has no arrival clock finer than the count's own,
        // so a sale rung before the count is treated as one the counter saw.
        // Postgres is where the late-arrival case is genuinely decided.
        let mut after = 0_i64;
        for (_, sale) in inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
        {
            let moved = sale
                .stock
                .iter()
                .filter(|(moved, _)| *moved == item)
                .fold(0_i64, |total, (_, qty)| total.saturating_add(*qty));
            if moved != 0 && sale.rung_at_ms >= count.counted_at_ms {
                after = after.saturating_add(moved);
            }
        }

        let after = after
            .saturating_add(received(Some(count.counted_at_ms)))
            .saturating_add(corrected(Some(count.counted_at_ms)));

        Ok(OnHand {
            item_id: item,
            qty_milli: count.counted_milli.saturating_add(after),
            counted_at_ms: Some(count.counted_at_ms),
            unreconciled_milli: 0,
            unreconciled_sales: 0,
        })
    }

    async fn operators(&self, tenant: u128) -> Result<Vec<OperatorRecord>> {
        let inner = self.lock();
        let mut found: Vec<OperatorRecord> = inner
            .operators
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, operator)| operator.clone())
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(found)
    }

    async fn put_operator(&self, tenant: u128, operator: &OperatorRecord) -> Result<()> {
        if operator.name.trim().is_empty() || operator.pin_rounds < 1_000 {
            // Matching what Postgres will refuse, so a store that passes tests
            // is not laxer than the one that runs.
            return Err(RepoError::Invalid);
        }
        self.lock()
            .operators
            .insert((tenant, operator.id), operator.clone());
        Ok(())
    }

    async fn set_operator_pin(
        &self,
        tenant: u128,
        operator_id: u128,
        salt: &[u8],
        rounds: u32,
        key: &[u8],
    ) -> Result<()> {
        if rounds < 1_000 || salt.is_empty() || key.is_empty() {
            // Matching what Postgres will refuse, so a store that passes tests
            // is not laxer than the one that runs.
            return Err(RepoError::Invalid);
        }
        let mut inner = self.lock();
        let Some(operator) = inner.operators.get_mut(&(tenant, operator_id)) else {
            return Err(RepoError::Invalid);
        };
        operator.pin_salt = salt.to_vec();
        operator.pin_rounds = rounds;
        operator.pin_key = key.to_vec();
        Ok(())
    }

    async fn amend_operator(&self, tenant: u128, amended: &AmendedOperator) -> Result<()> {
        if amended.name.trim().is_empty() {
            // Matching what Postgres will refuse, so a store that passes tests
            // is not laxer than the one that runs.
            return Err(RepoError::Invalid);
        }
        let mut inner = self.lock();
        let Some(operator) = inner.operators.get_mut(&(tenant, amended.id)) else {
            return Err(RepoError::Invalid);
        };
        operator.name = amended.name.clone();
        operator.max_discount_bp = amended.max_discount_bp;
        operator.may_override_price = amended.may_override_price;
        operator.may_refund = amended.may_refund;
        operator.may_void_line = amended.may_void_line;
        operator.may_authorise = amended.may_authorise;
        operator.may_open_drawer = amended.may_open_drawer;
        operator.may_close_shift = amended.may_close_shift;
        operator.active = amended.active;
        Ok(())
    }

    async fn shop_details(&self, tenant: u128) -> Result<ShopDetails> {
        let inner = self.lock();
        let name = inner
            .tenants
            .get(&tenant)
            .cloned()
            .ok_or(RepoError::UnknownTerminal)?;
        Ok(inner.shops.get(&tenant).cloned().unwrap_or(ShopDetails {
            name,
            ..ShopDetails::default()
        }))
    }

    async fn put_shop_details(&self, tenant: u128, details: &ShopDetails) -> Result<()> {
        if details.name.trim().is_empty() {
            // Matching Postgres rather than being quietly laxer: a store that
            // accepts what the other refuses is a store tests pass against and
            // production does not.
            return Err(RepoError::Invalid);
        }
        let mut inner = self.lock();
        inner.tenants.insert(tenant, details.name.clone());
        inner.shops.insert(tenant, details.clone());
        Ok(())
    }

    async fn put_supplier(&self, tenant: u128, supplier: &Supplier) -> Result<()> {
        self.lock()
            .suppliers
            .insert((tenant, supplier.id), supplier.clone());
        Ok(())
    }

    async fn suppliers(&self, tenant: u128) -> Result<Vec<Supplier>> {
        let inner = self.lock();
        let mut found: Vec<Supplier> = inner
            .suppliers
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, supplier)| supplier.clone())
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(found)
    }

    async fn takings(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<Vec<TakingsRow>> {
        let inner = self.lock();
        let mut by_till: BTreeMap<u128, TakingsRow> = BTreeMap::new();
        for sale in inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, sale)| sale)
            .filter(|sale| sale.rung_at_ms >= from_ms && sale.rung_at_ms <= to_ms)
        {
            let row = by_till.entry(sale.terminal).or_insert(TakingsRow {
                terminal: sale.terminal,
                sales: 0,
                total_minor: 0,
                needing_attention: 0,
                refunds: 0,
                refunded_minor: 0,
            });
            row.sales = row.sales.saturating_add(1);
            row.total_minor = row.total_minor.saturating_add(sale.total_minor);
            if sale.quarantine.is_some() {
                row.needing_attention = row.needing_attention.saturating_add(1);
            }
            // A refund is a sale with the signs turned round, so this is what
            // one looks like from the header alone.
            if sale.total_minor < 0 {
                row.refunds = row.refunds.saturating_add(1);
                row.refunded_minor = row.refunded_minor.saturating_add(sale.total_minor);
            }
        }
        Ok(by_till.into_values().collect())
    }

    async fn put_shifts(&self, tenant: u128, shifts: &[ClosedShift]) -> Result<Vec<u128>> {
        let mut inner = self.lock();
        let mut held = Vec::with_capacity(shifts.len());
        for shift in shifts {
            // Already there is still accepted: a till resending after a dropped
            // reply must be told it may stop, not told to try forever.
            inner
                .shifts
                .entry((tenant, shift.id))
                .or_insert_with(|| shift.clone());
            held.push(shift.id);
        }
        Ok(held)
    }

    async fn closed_shifts(&self, tenant: u128, limit: u32) -> Result<Vec<ClosedShift>> {
        let inner = self.lock();
        let mut found: Vec<ClosedShift> = inner
            .shifts
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, shift)| shift.clone())
            .collect();
        found.sort_by(|left, right| {
            right
                .closed_at_ms
                .cmp(&left.closed_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn take_payment(&self, tenant: u128, payment: &AccountPayment) -> Result<bool> {
        let mut inner = self.lock();
        let key = (tenant, payment.id, payment.person_key.clone());
        if inner.accounts.contains_key(&key) {
            // Already taken. Saying so rather than adding it again: a payment
            // counted twice is money the shop believes it has been given.
            return Ok(false);
        }
        // One minted id counts once, whoever it names. Keying only on the
        // person let the same id be sent twice under two spellings.
        if inner
            .accounts
            .keys()
            .any(|(owner, source, _)| *owner == tenant && *source == payment.id)
        {
            return Ok(false);
        }
        inner.accounts.insert(
            key,
            AccountEntryRow {
                person_key: payment.person_key.clone(),
                person_name: payment.person_name.clone(),
                source_id: payment.id,
                is_sale: false,
                written_off: payment.kind == Settlement::WrittenOff,
                // Money handed over comes off what is owed.
                amount_minor: payment.amount_minor.saturating_neg(),
                at_ms: payment.at_ms,
                note: payment.note.clone().unwrap_or_default(),
            },
        );
        Ok(true)
    }

    async fn balance(&self, tenant: u128, person_key: &str) -> Result<i64> {
        let inner = self.lock();
        Ok(inner
            .accounts
            .iter()
            .filter(|((owner, _, key), _)| *owner == tenant && key == person_key)
            .map(|(_, row)| row.amount_minor)
            .fold(0_i64, i64::saturating_add))
    }

    async fn owed(&self, tenant: u128, limit: u32) -> Result<Vec<Owing>> {
        let inner = self.lock();
        let mut totals: HashMap<String, Owing> = HashMap::new();
        for row in inner
            .accounts
            .iter()
            .filter(|((owner, _, _), _)| *owner == tenant)
            .map(|(_, row)| row)
        {
            let entry = totals.entry(row.person_key.clone()).or_insert(Owing {
                person_key: row.person_key.clone(),
                person_name: row.person_name.clone(),
                owed_minor: 0,
                since_ms: row.at_ms,
                last_at_ms: row.at_ms,
                entries: 0,
            });
            entry.owed_minor = entry.owed_minor.saturating_add(row.amount_minor);
            entry.since_ms = entry.since_ms.min(row.at_ms);
            if row.at_ms >= entry.last_at_ms {
                entry.last_at_ms = row.at_ms;
                // The most recent spelling that anybody actually wrote. A blank
                // one is not a correction, it is a field nobody filled in.
                if !row.person_name.is_empty() {
                    entry.person_name = row.person_name.clone();
                }
            }
            entry.entries = entry.entries.saturating_add(1);
        }

        let mut found: Vec<Owing> = totals
            .into_values()
            // A settled account is not a debt. It stays in the ledger and
            // leaves the list, which is what an owner wants to look at.
            .filter(|owing| owing.owed_minor != 0)
            .collect();
        found.sort_by(|left, right| {
            right
                .owed_minor
                .cmp(&left.owed_minor)
                .then_with(|| left.person_key.cmp(&right.person_key))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn account(
        &self,
        tenant: u128,
        person_key: &str,
        limit: u32,
    ) -> Result<Vec<AccountEntry>> {
        let inner = self.lock();
        let mut found: Vec<AccountEntry> = inner
            .accounts
            .iter()
            .filter(|((owner, _, key), _)| *owner == tenant && key == person_key)
            .map(|(_, row)| AccountEntry {
                source_id: row.source_id,
                is_sale: row.is_sale,
                written_off: row.written_off,
                amount_minor: row.amount_minor,
                at_ms: row.at_ms,
                note: row.note.clone(),
            })
            .collect();
        found.sort_by(|left, right| {
            right
                .at_ms
                .cmp(&left.at_ms)
                .then_with(|| right.source_id.cmp(&left.source_id))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn deliveries(&self, tenant: u128, limit: u32) -> Result<Vec<GoodsReceipt>> {
        let inner = self.lock();
        let mut found: Vec<GoodsReceipt> = inner
            .deliveries
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, receipt)| receipt.clone())
            .collect();
        // Newest first, and by id when two arrived in the same millisecond, so
        // the order is the same every time it is asked for.
        found.sort_by(|left, right| {
            right
                .received_at_ms
                .cmp(&left.received_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn receive_goods(&self, tenant: u128, receipt: &GoodsReceipt) -> Result<bool> {
        let mut inner = self.lock();
        if inner.deliveries.contains_key(&(tenant, receipt.id)) {
            // Stock booked twice is a shop ordering against goods it does not
            // have.
            return Ok(false);
        }
        inner
            .deliveries
            .insert((tenant, receipt.id), receipt.clone());
        Ok(true)
    }

    async fn correct_stock(&self, tenant: u128, correction: &StockCorrection) -> Result<bool> {
        let mut inner = self.lock();
        if inner.corrections.contains_key(&(tenant, correction.id)) {
            return Ok(false);
        }
        inner
            .corrections
            .insert((tenant, correction.id), correction.clone());
        Ok(true)
    }

    async fn register_terminal(&self, tenant: u128, terminal: u128, label: &str) -> Result<()> {
        self.enrol_labelled(tenant, terminal, label);
        Ok(())
    }

    async fn store_token_as(&self, caller: Caller, token: &TokenHash, role: Role) -> Result<()> {
        self.lock()
            .tokens
            .insert(token.clone(), Caller { role, ..caller });
        Ok(())
    }

    async fn revoke_token(&self, token: &TokenHash) -> Result<bool> {
        Ok(self.lock().tokens.remove(token).is_some())
    }

    async fn revoke_all_tokens(&self, caller: Caller) -> Result<usize> {
        let mut inner = self.lock();
        let before = inner.tokens.len();
        inner.tokens.retain(|_, owner| *owner != caller);
        Ok(before.saturating_sub(inner.tokens.len()))
    }

    async fn issue_enrolment_code(
        &self,
        grants: Caller,
        code: &TokenHash,
        valid_for: Duration,
    ) -> Result<()> {
        let expires = SystemTime::now()
            .checked_add(valid_for)
            .ok_or(RepoError::Backend)?;
        self.lock().codes.insert(code.clone(), (grants, expires));
        Ok(())
    }

    async fn redeem_enrolment_code(&self, code: &TokenHash) -> Result<Option<Caller>> {
        let mut inner = self.lock();
        // Removed rather than marked, so a code cannot be used twice even if two
        // devices race to redeem it.
        let Some((caller, expires)) = inner.codes.remove(code) else {
            return Ok(None);
        };
        if SystemTime::now() > expires {
            return Ok(None);
        }
        Ok(Some(caller))
    }

    async fn items_since(&self, tenant: u128, cursor: u64, limit: u32) -> Result<CataloguePage> {
        let inner = self.lock();
        let empty = BTreeMap::new();
        let log = inner.changes.get(&tenant).unwrap_or(&empty);
        let take = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);

        let mut page = CataloguePage {
            cursor,
            ..CataloguePage::default()
        };
        for (seq, change) in log.range(cursor.saturating_add(1)..).take(take) {
            match change {
                CatalogueChange::Upsert(item) => page.upserts.push((**item).clone()),
                CatalogueChange::Delete(id) => page.tombstones.push(*id),
            }
            page.cursor = *seq;
        }
        page.more = log.range(page.cursor.saturating_add(1)..).next().is_some();
        Ok(page)
    }

    async fn issue_lease(&self, tenant: u128, terminal: u128, count: u32) -> Result<LeaseRecord> {
        let mut inner = self.lock();
        if !inner.terminals.contains_key(&(tenant, terminal)) {
            return Err(RepoError::UnknownTerminal);
        }
        let entry = inner
            .counters
            .get_mut(&(tenant, terminal))
            .ok_or(RepoError::UnknownTerminal)?;
        let (next, epoch) = *entry;
        let span = u64::from(count.max(1));
        let last = next.saturating_add(span).saturating_sub(1);
        entry.0 = last.saturating_add(1);

        Ok(LeaseRecord {
            tenant,
            terminal,
            epoch,
            first: next,
            last,
        })
    }

    async fn tenant_record(&self, tenant: u128) -> Result<Option<TenantRecord>> {
        let inner = self.lock();
        Ok(inner.tenants.get(&tenant).map(|name| TenantRecord {
            id: tenant,
            name: name.clone(),
            catalogue_seq: inner
                .catalogue_seq
                .get(&tenant)
                .copied()
                .unwrap_or_default(),
        }))
    }

    async fn terminal_records(&self, tenant: u128) -> Result<Vec<TerminalRecord>> {
        let inner = self.lock();
        let mut found: Vec<TerminalRecord> = inner
            .terminals
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|((_, terminal), state)| {
                let (next_receipt, epoch) = inner
                    .counters
                    .get(&(tenant, *terminal))
                    .copied()
                    .unwrap_or((1, 1));
                TerminalRecord {
                    id: *terminal,
                    label: state.label.clone(),
                    epoch,
                    next_receipt,
                }
            })
            .collect();
        found.sort_by_key(|terminal| terminal.id);
        Ok(found)
    }

    async fn catalogue_after(
        &self,
        tenant: u128,
        after_seq: u64,
        limit: u32,
    ) -> Result<Vec<CatalogueRecord>> {
        let inner = self.lock();
        let empty = BTreeMap::new();
        let log = inner.changes.get(&tenant).unwrap_or(&empty);
        let take = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);

        let mut found = Vec::new();
        for (seq, change) in log.range(after_seq.saturating_add(1)..).take(take) {
            found.push(match change {
                CatalogueChange::Upsert(item) => CatalogueRecord {
                    seq: *seq,
                    kind: 1,
                    item_id: item.id,
                    payload: Some(postcard::to_allocvec(&**item).map_err(|_| RepoError::Backend)?),
                    schema: CATALOGUE_SCHEMA,
                },
                CatalogueChange::Delete(id) => CatalogueRecord {
                    seq: *seq,
                    kind: 2,
                    item_id: *id,
                    payload: None,
                    schema: CATALOGUE_SCHEMA,
                },
            });
        }
        Ok(found)
    }

    async fn sales_after(
        &self,
        tenant: u128,
        after_id: u128,
        limit: u32,
    ) -> Result<Vec<SaleRecord>> {
        let inner = self.lock();
        let mut found: Vec<SaleRecord> = inner
            .sales
            .iter()
            .filter(|(key, _)| key.0 == tenant && key.1 > after_id)
            .map(|(key, sale)| SaleRecord {
                id: sale.id,
                terminal: sale.terminal,
                receipt_no: sale.receipt_no.clone(),
                receipt_epoch: sale.receipt_epoch,
                rung_at_ms: sale.rung_at_ms,
                total_minor: sale.total_minor,
                payload: sale.payload.clone(),
                quarantine: inner.quarantine.get(key).cloned(),
            })
            .collect();
        found.sort_by_key(|sale| sale.id);
        found.truncate(usize::try_from(limit.max(1)).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn stock_after(
        &self,
        tenant: u128,
        after: (u128, u128),
        limit: u32,
    ) -> Result<Vec<StockRecord>> {
        let inner = self.lock();
        let mut found: Vec<StockRecord> = inner
            .sales
            .values()
            .filter(|sale| sale.tenant == tenant)
            .flat_map(|sale| {
                sale.stock.iter().map(|(item, qty_milli)| StockRecord {
                    source: sale.id,
                    source_kind: 1,
                    item: *item,
                    qty_milli: *qty_milli,
                    occurred_at_ms: sale.rung_at_ms,
                })
            })
            .filter(|movement| (movement.source, movement.item) > after)
            .collect();
        found.sort_by_key(|movement| (movement.source, movement.item));
        found.truncate(usize::try_from(limit.max(1)).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn put_tenant(&self, record: &TenantRecord) -> Result<()> {
        let mut inner = self.lock();
        inner.tenants.insert(record.id, record.name.clone());
        let seq = inner
            .catalogue_seq
            .get(&record.id)
            .copied()
            .unwrap_or_default()
            .max(record.catalogue_seq);
        inner.catalogue_seq.insert(record.id, seq);
        Ok(())
    }

    async fn put_terminals(&self, tenant: u128, records: &[TerminalRecord]) -> Result<usize> {
        let mut inner = self.lock();
        for record in records {
            // An imported terminal that is already here keeps the date it was
            // first seen in this shop. The bundle does not carry one, and
            // stamping "now" would tell support every till was installed the
            // morning of the restore.
            let now = now_ms();
            let state = inner
                .terminals
                .entry((tenant, record.id))
                .or_insert_with(|| TerminalState {
                    label: String::new(),
                    enrolled_at_ms: now,
                    last_seen_ms: None,
                });
            state.label = record.label.clone();
            // Never lowered. A restore from an older backup must not hand back a
            // receipt number the shop has already printed.
            let entry = inner
                .counters
                .entry((tenant, record.id))
                .or_insert((record.next_receipt, record.epoch));
            entry.0 = entry.0.max(record.next_receipt);
            entry.1 = entry.1.max(record.epoch);
        }
        Ok(records.len())
    }

    async fn put_catalogue(&self, tenant: u128, records: &[CatalogueRecord]) -> Result<usize> {
        let mut inner = self.lock();
        let mut added = 0_usize;
        let mut highest = 0_u64;
        for record in records {
            highest = highest.max(record.seq);
            let change = if record.kind == 1 {
                let bytes = record.payload.as_ref().ok_or(RepoError::Backend)?;
                let item: ItemWire = postcard::from_bytes(bytes).map_err(|_| RepoError::Backend)?;
                CatalogueChange::Upsert(Box::new(item))
            } else {
                CatalogueChange::Delete(record.item_id)
            };
            let log = inner.changes.entry(tenant).or_default();
            if log.contains_key(&record.seq) {
                continue;
            }
            log.insert(record.seq, change);
            added = added.saturating_add(1);
        }
        let seq = inner
            .catalogue_seq
            .get(&tenant)
            .copied()
            .unwrap_or_default()
            .max(highest);
        inner.catalogue_seq.insert(tenant, seq);
        Ok(added)
    }

    async fn put_sales(&self, tenant: u128, records: &[SaleRecord]) -> Result<usize> {
        let mut inner = self.lock();
        let mut added = 0_usize;
        for record in records {
            if inner.sales.contains_key(&(tenant, record.id)) {
                continue;
            }
            if let (Some(receipt), Some(epoch)) = (record.receipt_no.clone(), record.receipt_epoch)
            {
                inner.receipts.insert((tenant, receipt, epoch));
            }
            if let Some(reason) = record.quarantine.clone() {
                inner.quarantine.insert((tenant, record.id), reason);
            }
            // Arrival is the receiving server's fact, so an imported sale gets
            // the time it landed here, exactly as the Postgres column defaults
            // to `now()`. A bundle carries no arrival time, and leaving this
            // absent would show an imported repair queue as dated 1970.
            inner.received.insert((tenant, record.id), now_ms());
            inner.sales.insert(
                (tenant, record.id),
                StoredSale {
                    tenant,
                    terminal: record.terminal,
                    id: record.id,
                    receipt_no: record.receipt_no.clone(),
                    receipt_epoch: record.receipt_epoch,
                    rung_at_ms: record.rung_at_ms,
                    total_minor: record.total_minor,
                    payload: record.payload.clone(),
                    // The enum is not recoverable from the stored text. The
                    // reason survives in `quarantine`, which is what the repair
                    // queue reads.
                    quarantine: None,
                    stock: Vec::new(),
                    // An imported sale brings its own account entries with it
                    // when the bundle carries them; re-reading them out of the
                    // payload here would double every debt on a restore.
                    on_account: Vec::new(),
                },
            );
            added = added.saturating_add(1);
        }
        Ok(added)
    }

    async fn shifts_after(
        &self,
        tenant: u128,
        after: u128,
        limit: u32,
    ) -> Result<Vec<ClosedShift>> {
        let inner = self.lock();
        let mut found: Vec<ClosedShift> = inner
            .shifts
            .iter()
            .filter(|((owner, id), _)| *owner == tenant && *id > after)
            .map(|(_, shift)| shift.clone())
            .collect();
        found.sort_by_key(|shift| shift.id);
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn account_after(
        &self,
        tenant: u128,
        after: (u128, String),
        limit: u32,
    ) -> Result<Vec<AccountRecord>> {
        let inner = self.lock();
        let mut found: Vec<AccountRecord> = inner
            .accounts
            .iter()
            .filter(|((owner, source, key), _)| *owner == tenant && (*source, key.clone()) > after)
            .map(|(_, row)| AccountRecord {
                person_key: row.person_key.clone(),
                person_name: row.person_name.clone(),
                source: row.source_id,
                kind: kind_of(row),
                amount_minor: row.amount_minor,
                at_ms: row.at_ms,
                note: row.note.clone(),
            })
            .collect();
        found.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.person_key.cmp(&right.person_key))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn put_account(&self, tenant: u128, records: &[AccountRecord]) -> Result<usize> {
        let mut inner = self.lock();
        let mut added = 0_usize;
        for record in records {
            let key = (tenant, record.source, record.person_key.clone());
            if inner.accounts.contains_key(&key) {
                continue;
            }
            inner.accounts.insert(
                key,
                AccountEntryRow {
                    person_key: record.person_key.clone(),
                    person_name: record.person_name.clone(),
                    source_id: record.source,
                    is_sale: record.kind == 1,
                    written_off: record.kind == 3,
                    amount_minor: record.amount_minor,
                    at_ms: record.at_ms,
                    note: record.note.clone(),
                },
            );
            added = added.saturating_add(1);
        }
        Ok(added)
    }

    async fn put_stock(&self, tenant: u128, records: &[StockRecord]) -> Result<usize> {
        let mut inner = self.lock();
        let mut added = 0_usize;
        for record in records {
            let Some(sale) = inner.sales.get_mut(&(tenant, record.source)) else {
                // No sale to hang it on. Postgres has no such constraint, but a
                // movement with nothing to attribute it to is not stock: it is a
                // number nobody can explain.
                continue;
            };
            if sale.stock.iter().any(|(item, _)| *item == record.item) {
                continue;
            }
            sale.stock.push((record.item, record.qty_milli));
            added = added.saturating_add(1);
        }
        Ok(added)
    }

    async fn repair_queue(&self, tenant: u128, limit: u32) -> Result<Vec<RepairItem>> {
        let inner = self.lock();
        // Driven off the rendered reason rather than off the enum on the sale,
        // for the same reason the Postgres query reads the `quarantine` column:
        // a sale that arrived by import has the text and no enum, and reading
        // the enum would quietly empty a restored shop's queue.
        let mut found: Vec<RepairItem> = inner
            .quarantine
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .filter(|(key, _)| !inner.resolutions.contains_key(*key))
            .filter_map(|(key, reason)| {
                let sale = inner.sales.get(key)?;
                Some(RepairItem {
                    id: sale.id,
                    receipt_no: sale.receipt_no.clone(),
                    total_minor: sale.total_minor,
                    received_at_ms: inner.received.get(key).copied().unwrap_or_default(),
                    reason: reason.clone(),
                })
            })
            .collect();

        // Sorted by id rather than by arrival. A sale id is a ULID, whose
        // leading bits are its mint time, so this is the order the shop rang
        // them up in even when a batch of a day's offline sales all arrived in
        // the same millisecond.
        found.sort_by_key(|item| item.id);
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn resolve_quarantine(&self, tenant: u128, sale: u128, note: &str) -> Result<bool> {
        let mut inner = self.lock();
        let quarantined = inner.quarantine.contains_key(&(tenant, sale));
        if !quarantined || inner.resolutions.contains_key(&(tenant, sale)) {
            return Ok(false);
        }
        inner.resolutions.insert((tenant, sale), note.to_owned());
        Ok(true)
    }

    async fn terminal_health(&self, tenant: u128) -> Result<Vec<TerminalHealth>> {
        let inner = self.lock();
        let mut found: Vec<TerminalHealth> = inner
            .terminals
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|((_, terminal), state)| {
                let sales = inner
                    .sales
                    .values()
                    .filter(|sale| sale.tenant == tenant && sale.terminal == *terminal);
                let open_repairs = sales
                    .clone()
                    .filter(|sale| inner.quarantine.contains_key(&(tenant, sale.id)))
                    .filter(|sale| !inner.resolutions.contains_key(&(tenant, sale.id)))
                    .count();

                TerminalHealth {
                    terminal: *terminal,
                    label: state.label.clone(),
                    epoch: inner
                        .counters
                        .get(&(tenant, *terminal))
                        .map_or(1, |(_, epoch)| *epoch),
                    enrolled_at_ms: state.enrolled_at_ms,
                    last_seen_ms: state.last_seen_ms,
                    sales: u64::try_from(sales.count()).unwrap_or(u64::MAX),
                    open_repairs: u64::try_from(open_repairs).unwrap_or(u64::MAX),
                }
            })
            .collect();

        // A stable order, so the list does not shuffle between two loads of the
        // same page and make an operator doubt what they read.
        found.sort_by_key(|health| health.terminal);
        Ok(found)
    }

    async fn mark_terminal_seen(&self, tenant: u128, terminal: u128) -> Result<()> {
        let mut inner = self.lock();
        let now = now_ms();
        if let Some(state) = inner.terminals.get_mut(&(tenant, terminal)) {
            state.last_seen_ms = Some(now);
        }
        Ok(())
    }

    async fn upsert_item(&self, tenant: u128, item: &ItemWire) -> Result<u64> {
        Ok(MemoryRepo::upsert_item(self, tenant, item.clone()))
    }

    async fn delete_item(&self, tenant: u128, item_id: u128) -> Result<u64> {
        Ok(MemoryRepo::delete_item(self, tenant, item_id))
    }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use super::*;

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    #[tokio::test]
    async fn issues_blocks_that_never_overlap() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);

        let first = repo.issue_lease(TENANT, TERMINAL, 500).await.unwrap();
        let second = repo.issue_lease(TENANT, TERMINAL, 500).await.unwrap();

        assert_eq!((first.first, first.last), (1, 500));
        assert_eq!((second.first, second.last), (501, 1_000));
        assert!(second.first > first.last, "blocks must not overlap");
    }

    #[tokio::test]
    async fn refuses_to_lease_to_a_terminal_it_does_not_know() {
        let repo = MemoryRepo::new();
        assert_eq!(
            repo.issue_lease(TENANT, TERMINAL, 10).await,
            Err(RepoError::UnknownTerminal)
        );
    }

    #[tokio::test]
    async fn a_bumped_epoch_marks_later_blocks() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let before = repo.issue_lease(TENANT, TERMINAL, 10).await.unwrap();

        // The back office decides this terminal was restored from a backup.
        repo.bump_epoch(TENANT, TERMINAL);
        let after = repo.issue_lease(TENANT, TERMINAL, 10).await.unwrap();

        assert_eq!(before.epoch, 1);
        assert_eq!(after.epoch, 2, "numbers stay attributable across a restore");
    }

    #[tokio::test]
    async fn tenants_cannot_see_each_other() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 900,
            receipt_no: Some("T1-000100".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 0,
            total_minor: 49_450,
            payload: vec![],
            quarantine: None,
            stock: vec![],
            on_account: vec![],
        })
        .await
        .unwrap();

        assert!(repo.has_sale(TENANT, 900).await.unwrap());
        assert!(
            !repo.has_sale(999, 900).await.unwrap(),
            "another shop must not see it"
        );
        assert!(repo.receipt_taken(TENANT, "T1-000100", 1).await.unwrap());
        assert!(!repo.receipt_taken(999, "T1-000100", 1).await.unwrap());
        // A different epoch is a different number space.
        assert!(!repo.receipt_taken(TENANT, "T1-000100", 2).await.unwrap());
    }
}
