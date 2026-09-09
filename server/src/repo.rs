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
///
/// Version 3 because it happened anyway, three times over. `from_a_till`,
/// `supply` and `category` were each appended to `ItemWire` while this stayed
/// at 2, so rows stamped 2 exist in four lengths and this build could read only
/// the newest. In the shop this was found in, seven rows written on its first
/// day stopped decoding: the back office reported them as written by a version
/// it cannot read, every till went on selling those items at the price it
/// already held, and the advice on the screen was to type the prices in again.
/// The vintages are frozen in `openpos_core::protocol` and the decoder tries
/// them longest first, so those rows read again; this number moves from here on
/// so the ambiguity stops growing.
pub const CATALOGUE_SCHEMA: u8 = 3;

/// How many fields `ItemWire` has, as of the schema above.
///
/// Checked by a test against the source, because the comment above has been
/// read and ignored three times. A field appended without moving the schema is
/// a shop's catalogue quietly becoming unreadable, and the test is the only
/// thing that has ever noticed.
pub const ITEM_WIRE_FIELDS: usize = 16;

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
    /// What the goods on this sale cost the shop, from the cost each line
    /// carried when it was rung.
    ///
    /// Frozen by the till and summed here rather than looked up against the
    /// item's cost today, for the same reason the price is: what a day made is
    /// a fact about that day, and a supplier's price moving next month must not
    /// rewrite it.
    pub cost_minor: i64,
    /// Whether every line on it carried a cost. A shop that has never entered
    /// what it pays would otherwise read a margin equal to its whole turnover.
    pub cost_known: bool,
    /// What this sale left in the drawer: cash tenders less change given back.
    ///
    /// Computed here from the tenders rather than believed from a field, like
    /// the stock movements and the tax rows. It is what lets a shop check a
    /// counted drawer against its own sales instead of against the till's word
    /// for them.
    pub cash_minor: i64,
    /// For a refund, the receipt it reverses, as the till wrote it.
    ///
    /// Beside the sale as well as inside its bytes, because the question asked
    /// of it is "how much has been refunded against this receipt", and nothing
    /// could answer that without decoding every sale in the shop.
    pub refund_of: Option<String>,
    /// Item id and signed milli-units.
    pub stock: Vec<(u128, i64)>,
    /// What a supervisor waived on this sale, in the order it was waived.
    ///
    /// Read from the ticket the till wrote, because that is the same text the
    /// customer's receipt was printed from: a waiver the shop's copy words
    /// differently from the customer's is a waiver nobody can settle.
    pub overrides: Vec<String>,
    /// What this sale owed the revenue, by rate: basis points, net, tax.
    /// Recomputed by the server rather than read from the payload, because what
    /// a shop declares must not be something a device could assert.
    pub vat: Vec<(u32, i64, i64, u8)>,
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
    /// What a till should do when a basket asks for more than the shop believes
    /// it has: 0 nothing, 1 say so, 2 refuse it and let a supervisor allow it.
    ///
    /// Nothing by default, because the figure is only as good as the shop's
    /// stock keeping and a shop that has never counted holds zero of
    /// everything. Turning it on is a statement that the figures mean
    /// something.
    pub stock_rule: u8,
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
    /// Whether this sale is already stored.
    ///
    /// Nothing in the product asks any more: the duplicate check moved into the
    /// write, where a primary key decides it and two connections cannot both be
    /// told a sale is new. It is kept because it is what the tests observe with,
    /// and what they observe is that one shop cannot see another's rows after a
    /// bulk import. Deleting it would leave that assertion nothing to make it
    /// through, which is a worse trade than an unused reader.
    fn has_sale(&self, tenant: u128, id: u128) -> impl Future<Output = Result<bool>> + Send;

    /// Whether a receipt number is already used, under a given epoch. Two sales
    /// sharing one number means a terminal was restored or cloned.
    ///
    /// A test observer, like `has_sale` above and for the same reason: the claim
    /// is now made by the insert that stores the sale.
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
    /// When this terminal was enrolled, or `None` if the shop has no such
    /// terminal.
    ///
    /// The time comes back rather than a bare yes because the enrolment date is
    /// worth having beside a sale's own clock. It is not the bound on how old a
    /// sale may be: a device re-enrolled after a wipe is a new terminal row
    /// holding perfectly good sales rung yesterday.
    fn terminal_enrolled_at(
        &self,
        tenant: u128,
        terminal: u128,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;

    /// Whether the shop has this terminal at all.
    fn terminal_enrolled(
        &self,
        tenant: u128,
        terminal: u128,
    ) -> impl Future<Output = Result<bool>> + Send {
        async move { Ok(self.terminal_enrolled_at(tenant, terminal).await?.is_some()) }
    }

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

    /// The same question about many items at once.
    ///
    /// The same answer, not a second one: the store that overrides this owes a
    /// test that runs both and compares, and `postgres_repo.rs` has it. That is
    /// the whole reason this exists as a widening of one question rather than as
    /// its own query with its own idea of what a barrier means.
    ///
    /// It exists because a till refreshing what the shelves hold asks about two
    /// hundred items at a time, and asking one at a time is two hundred
    /// transactions and six hundred round trips. A shop with eight hundred lines
    /// takes twenty minutes to get round its own catalogue that way, so the
    /// figure behind a refusal at the far end of the alphabet can be twenty
    /// minutes old. The refusal is the point: a cashier told the shelf is empty
    /// when it is not is a cashier who stops trusting the till.
    ///
    /// The default is the loop, so a store that has not been widened is correct
    /// by construction and merely slow.
    fn on_hand_many(
        &self,
        tenant: u128,
        items: &[u128],
    ) -> impl Future<Output = Result<Vec<OnHand>>> + Send {
        async move {
            let mut found = Vec::with_capacity(items.len());
            for item in items {
                found.push(self.on_hand(tenant, *item).await?);
            }
            Ok(found)
        }
    }

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

    /// Store what a till allowed. Returns every count the server now holds,
    /// including ones it already had, for the same reason the drawers do.
    ///
    /// The device's own count is what makes one storable exactly once: two
    /// identical actions in one millisecond are possible and are two different
    /// things, so the clock cannot be the key.
    fn put_allowed(
        &self,
        tenant: u128,
        terminal: u128,
        allowed: &[AllowedAction],
    ) -> impl Future<Output = Result<Vec<u64>>> + Send;

    /// When this shop was created, if it exists.
    ///
    /// The bound on how old a sale can be. Nothing rung in a shop can predate
    /// the shop, and a device whose clock says 2010 is a device that has been
    /// switched off long enough to forget what year it is, which is ordinary
    /// for a cheap tablet and not ordinary for a figure on a tax return.
    fn tenant_created_at(&self, tenant: u128) -> impl Future<Output = Result<Option<u64>>> + Send;

    /// Which item holds each of these barcodes, if any item does.
    ///
    /// A barcode belongs to one item. Two items carrying the same one means a
    /// scan rings whichever the index happened to keep: the wrong price, the
    /// wrong tax, the wrong thing off the shelf. The replica's own comment has
    /// said "the back office is responsible for not issuing one" since it was
    /// written, and nothing was.
    ///
    /// Withdrawn items are not counted. A shop that stops selling something has
    /// its barcode back.
    fn barcode_holders(
        &self,
        tenant: u128,
        barcodes: &[String],
    ) -> impl Future<Output = Result<Vec<(String, u128)>>> + Send;

    /// What one receipt was rung for, and what has been refunded against it.
    ///
    /// `None` when this shop has no sale carrying that number, which is an
    /// ordinary thing and not on its own a wrong: a till whose sales have not
    /// arrived yet, or a receipt from before the shop kept records here. What it
    /// is for is the refund that reverses a sale nobody has, and the receipt
    /// refunded twice.
    ///
    /// Both figures as the ledger holds them: a sale is positive and a refund
    /// negative, and the caller decides what "beyond" means rather than being
    /// handed a judgement.
    fn refunded_against(
        &self,
        tenant: u128,
        receipt_no: &str,
    ) -> impl Future<Output = Result<Option<(i64, i64)>>> + Send;

    /// What one receipt has moved, per item, netted across the sale and every
    /// refund against it.
    ///
    /// A sale's movement is negative: the goods left. A refund's is positive:
    /// they came back. So a net above zero for an item is more of that item
    /// coming back than that receipt ever sold, which is the money being right
    /// and the goods being wrong.
    ///
    /// Read out of the movements the shop already keeps rather than by decoding
    /// sales: the ledger is the answer, and a second way of working it out is a
    /// second answer to disagree with it.
    fn goods_against(
        &self,
        tenant: u128,
        receipt_no: &str,
    ) -> impl Future<Output = Result<Vec<(u128, i64)>>> + Send;

    /// What the shop's own sales say one till took in cash between two moments.
    ///
    /// The other half of a counted drawer. What a till reported it expected is
    /// the till's word, and the variance an owner acts on is the difference
    /// between that word and a count: nothing asked whether the shop's own
    /// sales came to the same figure. A till reporting a lower expectation
    /// hides a shortfall, and until this nothing could see it.
    ///
    /// Cash less change, per sale, over the window the drawer was open. Struck
    /// out sales are left out: a sale somebody said never happened put nothing
    /// in the drawer. A sale merely held is counted, because the money for it
    /// is as likely to be in the drawer as not and the figure exists to be
    /// compared rather than to be relied on alone.
    ///
    /// None where the shop cannot answer: a sale stored before it worked this
    /// out carries no figure, and treating that as nothing in the drawer would
    /// report every drawer in the shop's history as disagreeing with its till.
    fn drawer_takings(
        &self,
        tenant: u128,
        terminal: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<Option<i64>>> + Send;

    /// Runs of receipt numbers with no sale against them, oldest first.
    ///
    /// The question an inspector asks is why the numbering jumps, and until
    /// this the shop had no way to look. Oldest first because the old ones are
    /// the ones that will never close: a gap from this morning is probably a
    /// till that has not synced since lunch.
    fn receipt_gaps(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ReceiptGap>>> + Send;

    /// What the shop allowed in a window, newest first.
    ///
    /// The question this answers is "who allowed it", asked a week after a
    /// variance. Newest first because the thing being asked about is usually
    /// recent, and the older it gets the less anybody can remember about it.
    fn allowed(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<AllowedAction>>> + Send;

    /// Catalogue changes this build cannot read, which every till has passed
    /// over. Oldest first, and a shop with none gets an empty list.
    fn unreadable_changes(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<UnreadableChange>>> + Send;

    /// What passed between the shop and one supplier over a period, oldest
    /// first: deliveries in, payments out. The statement two people put side by
    /// side when their figures disagree.
    fn supplier_statement(
        &self,
        tenant: u128,
        supplier_id: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<Vec<SupplierEntry>>> + Send;

    /// What sold over a period, most sold first. The figure a shop buys
    /// against, so it is what left the shelf rather than what was charged.
    fn sold(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<SoldRow>>> + Send;

    /// Record money paid to a supplier. Idempotent by payment id, because a
    /// dropped reply is the usual reason one is sent twice and a payment
    /// counted twice is money the shop believes it has paid.
    fn pay_supplier(
        &self,
        tenant: u128,
        payment: &SupplierPayment,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// What the shop owes each supplier: the deliveries less what has been
    /// paid. Settled suppliers are not listed, most owed first.
    fn supplier_owing(
        &self,
        tenant: u128,
    ) -> impl Future<Output = Result<Vec<SupplierOwing>>> + Send;

    /// What supervisors waived over a period, newest first. The question an
    /// owner asks when the takings are light and everybody was on shift.
    fn waived(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<WaivedRow>>> + Send;

    /// What was sold at each rate over a period, smallest rate first.
    fn vat_summary(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<VatSummary>> + Send;

    /// What a period looked like beyond its sales.
    fn day_summary(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<DaySummary>> + Send;

    /// The item as the shop now holds it, and the sequence it last changed at.
    ///
    /// What somebody about to edit an item should be looking at, rather than
    /// their device's copy of the catalogue, which is up to half a minute
    /// behind and may be missing a change another device made a moment ago.
    fn item_now(
        &self,
        tenant: u128,
        item_id: u128,
    ) -> impl Future<Output = Result<Option<(ItemWire, u64)>>> + Send;

    /// Where the shop's settings counter stands: the people, the shop's own
    /// details and who buys on account, as one number.
    ///
    /// A till asks for this on the cadence it pulls the catalogue at, and asks
    /// for the three lists themselves only when it has moved. Suspending
    /// somebody then reaches every till in half a minute rather than ten,
    /// without three large replies a minute per till for data nobody touched.
    fn settings_seq(&self, tenant: u128) -> impl Future<Output = Result<u64>> + Send;

    /// Add or correct somebody who buys on account.
    fn put_customer(
        &self,
        tenant: u128,
        customer: &CustomerRecord,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Everybody the shop lets buy on account, stopped accounts included: a
    /// till showing only the active ones is right, and a back office that
    /// cannot see the rest has nowhere to let anybody back in.
    fn customers(&self, tenant: u128) -> impl Future<Output = Result<Vec<CustomerRecord>>> + Send;

    /// Say what a till currently has open. Replaces whatever that terminal said
    /// before: this is a position, not a history.
    fn put_open_drawer(
        &self,
        tenant: u128,
        drawer: &OpenDrawer,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Drawers open right now, oldest first, which is the order an owner cares
    /// about: the one open longest is the one somebody forgot.
    fn open_drawers(&self, tenant: u128) -> impl Future<Output = Result<Vec<OpenDrawer>>> + Send;

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
    ///
    /// `after` is where the last page ended: what that person owed and their
    /// key. `None` starts at the top. A keyset rather than an offset, because
    /// the list is ordered by what is owed and a payment taken between two
    /// pages would make an offset skip somebody.
    fn owed(
        &self,
        tenant: u128,
        after: Option<(i64, String)>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Owing>>> + Send;

    /// What every written-down customer owes, in one answer. Only those who owe
    /// something: a shop with two hundred names and four debts sends four rows.
    fn customer_balances(
        &self,
        tenant: u128,
    ) -> impl Future<Output = Result<Vec<(u128, i64)>>> + Send;

    /// What one person owes, asked directly. A screen that has just taken a
    /// payment needs this one number and must not get it by paging a list it
    /// might not be on.
    fn balance(&self, tenant: u128, person_key: &str) -> impl Future<Output = Result<i64>> + Send;

    /// One person's account, newest first, which is what an owner reads out
    /// when somebody disputes the total.
    ///
    /// `after` is where the last page ended: when that entry was and what made
    /// it. `None` starts at the newest. Every entry a person has is one row per
    /// source, so that pair is unique and the page cannot repeat or skip a
    /// line.
    fn account(
        &self,
        tenant: u128,
        person_key: &str,
        after: Option<(u64, u128)>,
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

    /// The store's own clock, in milliseconds, for taking a cut.
    ///
    /// An export reads eight tables and a shop is trading while it does. Every
    /// append-only read is filtered to what had arrived when the export
    /// started, so a sale that lands mid-export is left out of it whole rather
    /// than half in: its stock movements and its account entries go with it,
    /// and a movement with no sale behind it is stock that moved for no reason
    /// anybody can point at.
    ///
    /// The store's clock rather than the caller's, because the two drift and
    /// the comparison happens in the store.
    fn now_ms(&self) -> impl Future<Output = Result<u64>> + Send;

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
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<SaleRecord>>> + Send;

    /// Deliveries after `after_id`, in id order, with their lines.
    ///
    /// In a bundle because the movements alone are not the record: what a
    /// delivery cost and which supplier it came from is what the shop pays
    /// against, and a restored shop that knows its stock moved and not what it
    /// owes for it has lost the half nobody can rebuild.
    fn deliveries_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<GoodsReceipt>>> + Send;

    /// Money handed to suppliers, after `after_id`, in id order.
    ///
    /// The other half of the payables book, and the half that exists nowhere
    /// else: a payment is in no delivery and in no sale.
    fn supplier_payments_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<SupplierPayment>>> + Send;

    /// Counts after `after_id`, in id order.
    ///
    /// A count is a barrier, not a movement: it says what a shelf held at a
    /// moment and supersedes everything before it. A restored shop without its
    /// barriers works its figures out from the movements alone, which is the
    /// answer the shop counted the shelf to correct.
    fn counts_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<StockCount>>> + Send;

    /// Corrections after `after_id`, in id order.
    ///
    /// The movements they caused are already in a bundle. The reason is not,
    /// and an unexplained correction is indistinguishable from theft when the
    /// variance is read a month later.
    fn corrections_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<StockCorrection>>> + Send;

    /// What was allowed, after the given (terminal, count) pair, in that order.
    ///
    /// In a bundle because it is the record that answers "who allowed this"
    /// after a variance, and a shop that moves machine and arrives without it
    /// cannot answer that about anything before the move.
    fn allowed_after(
        &self,
        tenant: u128,
        after: (u128, u64),
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<AllowedAction>>> + Send;

    /// Stock movements after the given (sale, item) pair, in that order.
    fn stock_after(
        &self,
        tenant: u128,
        after: (u128, u128),
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<StockRecord>>> + Send;

    /// Counted drawers, in id order, for an export.
    fn shifts_after(
        &self,
        tenant: u128,
        after: u128,
        cut_ms: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ClosedShift>>> + Send;

    /// The account book, in key order, for an export.
    fn account_after(
        &self,
        tenant: u128,
        after: (u128, String),
        cut_ms: u64,
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

    /// What the shop made over a period, and how much of it it can answer for.
    ///
    /// Turnover before tax, less what the goods cost, from the cost each line
    /// carried when it was rung. A shop knows what it took; until this existed
    /// nothing could say what it made, which is the question that decides what
    /// to put on the shelf.
    ///
    /// The uncosted sales are counted apart rather than left out or quietly
    /// treated as free. A shop that has never entered what it pays would
    /// otherwise read a margin equal to its whole turnover and believe it.
    fn made(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<MadeSummary>> + Send;

    /// What the shop holds under one receipt number.
    ///
    /// The question asked across the counter: somebody comes back with a piece
    /// of paper. A list rather than one sale, because two sales carrying one
    /// number is exactly what gets asked about, and answering with whichever
    /// arrived first would hide the second from the person owed it.
    ///
    /// Quarantined and struck-out sales are in the answer. This is not a
    /// figure the shop declares; it is a record of what was rung, and leaving
    /// out the sale somebody says never happened is leaving out the answer.
    fn sales_on_receipt(
        &self,
        tenant: u128,
        receipt_no: &str,
    ) -> impl Future<Output = Result<Vec<SaleOnPaper>>> + Send;

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

    /// Take one sale out of the queue, saying what was decided and whether it
    /// stands.
    ///
    /// `kept` false means it was not a sale: a till restored from a backup rang
    /// the same goods twice, and one of them did not happen. Everything that
    /// counted it stops counting it, the money and the tax and what left the
    /// shelf and anything it put on somebody's account. Nothing is deleted: the
    /// figures filter, and the sale stays exactly as it arrived.
    ///
    /// Returns whether anything moved. Answering twice is not an error, because
    /// two people working the same queue is the normal case and the second one
    /// should be told "already done" rather than shown a failure. Changing the
    /// answer is a different act with its own method, so it cannot happen by
    /// pressing twice.
    fn resolve_quarantine(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
        kept: bool,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// What has been decided lately, newest first.
    ///
    /// The queue only shows what is waiting, so a decision made in error left no
    /// screen it could be reached from. A strike-out takes a real debt off
    /// somebody's account, so somebody who has just made the wrong one has to be
    /// able to find it.
    fn decided(
        &self,
        tenant: u128,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<DecidedSale>>> + Send;

    /// Decide a sale again.
    ///
    /// Separate from resolving because it is a different act: this one changes
    /// an answer somebody already gave, and a screen that let that happen by
    /// pressing the same button twice would be a way to lose a debt quietly.
    /// Every answer is kept; the latest is the one the figures read.
    ///
    /// `expected` is how many answers the caller saw. Zero means it did not
    /// look, which an older screen or a script sends and which is accepted.
    fn decide_again(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
        kept: bool,
        expected: u32,
    ) -> impl Future<Output = Result<Decided>> + Send;

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

    /// Whether anything has ever happened to this item: sold, delivered,
    /// written off or counted.
    ///
    /// Asked before a deletion, because a deletion is a tombstone and every till
    /// drops the item on the next pull. For a line typed by mistake that is
    /// exactly right. For anything the shop has traded it takes the name off
    /// figures still in the books, and the act that was wanted is withdrawing
    /// it, which keeps the record and takes it off the tills just the same.
    fn item_has_history(
        &self,
        tenant: u128,
        item_id: u128,
    ) -> impl Future<Output = Result<bool>> + Send;
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
    /// The same reason as the enum, so a restored shop can still say why a sale
    /// is held in its own language. Empty for a sale nobody held, and for a
    /// bundle written before the shop kept it.
    pub quarantine_kind: Vec<u8>,
    /// What the shop decided about it, and whether it stands. Carried, unlike
    /// the tax figures, because it is not derivable from the payload: it is a
    /// person's decision about the sale rather than anything the sale says. A
    /// restore that dropped it would put a struck-out duplicate back into the
    /// takings and back into the queue, which is the exact morning this whole
    /// thing exists for.
    pub resolution: Option<(String, bool)>,
    /// What it owed the revenue, by rate. Not carried in a bundle: it is
    /// recomputed from the payload on the way in, from the same crate that
    /// computed it the first time, so a restored shop declares what the
    /// original one did rather than what a file claimed.
    pub vat: Vec<(u32, i64, i64, u8)>,
    /// For a refund, the receipt it reverses. Read back out of the payload on
    /// the way in for the same reason the tax figures are: a bundle that
    /// asserted what a refund reversed would be a way to point a refund at a
    /// different sale by editing a text file.
    pub refund_of: Option<String>,
    /// What a supervisor waived on it, read back out of the payload on the way
    /// in for the same reason: the ticket is the record, and a bundle that
    /// asserted its own would be a way to rewrite what somebody allowed.
    pub overrides: Vec<String>,
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
fn record_vat(inner: &mut Inner, sale: &StoredSale) {
    for (bp, net, vat, supply) in &sale.vat {
        inner
            .sale_vat
            .entry((sale.tenant, sale.id, *bp, *supply))
            .or_insert((*net, *vat));
    }
}

fn charge_accounts(inner: &mut Inner, sale: &StoredSale) {
    for charge in &sale.on_account {
        inner
            .accounts
            .entry((sale.tenant, sale.id, charge.person_key.clone()))
            .or_insert_with(|| AccountEntryRow {
                received_ms: now_ms(),
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

/// The customer a key names, when it names one.
///
/// The book holds two kinds of key: a folded name, and a written-down customer
/// written as `#` and their id. Only the second can be shown against a record.
pub(crate) fn customer_from_key(key: &str) -> Option<u128> {
    let rest = key.strip_prefix('#')?;
    openpos_core::ids::Ulid::decode(rest)
        .ok()
        .map(|id| id.to_u128())
}

/// Move the settings counter on, so tills learn something changed.
///
/// One counter for the people, the shop and the account customers together: a
/// till that has to re-read one of them may as well re-read all three, and three
/// counters would be three chances to forget to move one.
fn bump_settings(inner: &mut Inner, tenant: u128) {
    let seq = inner.settings_seq.entry(tenant).or_default();
    *seq = seq.saturating_add(1);
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
    /// When this arrived here. The server's fact, as with a sale.
    received_ms: u64,
    person_key: String,
    person_name: String,
    source_id: u128,
    is_sale: bool,
    written_off: bool,
    amount_minor: i64,
    at_ms: u64,
    note: String,
}

/// A catalogue change this build cannot read.
///
/// The cursor moves past one of these, because failing the page would stop every
/// till in the shop syncing for ever over one bad row. That trade is only
/// defensible if somebody can be told, and this is what tells them: a shop whose
/// price change never reached its tills has no other way to find out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadableChange {
    pub seq: u64,
    pub item_id: u128,
    /// The schema its payload was written under, which is the useful part: one
    /// number names the build that wrote it.
    pub schema: u8,
}

/// One line of what passed between the shop and a supplier: goods in, or money
/// out.
///
/// What the distributor's man wants to see when the shop's figure and his own
/// disagree, which is the conversation this exists for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplierEntry {
    pub at_ms: u64,
    /// True when goods came in, false when money went out.
    pub delivered: bool,
    /// Positive either way: what arrived, or what was handed over.
    pub amount_minor: i64,
    /// The supplier's own challan or invoice number for a delivery, or whatever
    /// was written against a payment.
    pub reference: Option<String>,
}

/// How much of one item left the shelf over a period, and on how many sales.
///
/// Read from the stock movements a sale wrote rather than from its payload: the
/// movements are already the server's own recomputation of what the lines said,
/// which is the figure a shop should buy against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoldRow {
    pub item_id: u128,
    /// Positive is what left the shop. A period with more refunds than sales of
    /// one thing shows negative, which is a fact worth seeing.
    pub qty_milli: i64,
    pub sales: u64,
}

/// Money the shop paid a supplier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplierPayment {
    /// Minted by whoever recorded it, so a resent one is not counted twice.
    pub id: u128,
    pub supplier_id: u128,
    /// What was handed over. Positive.
    pub amount_minor: i64,
    pub paid_at_ms: u64,
    pub note: Option<String>,
}

/// What the shop owes one supplier, and what that is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplierOwing {
    pub supplier_id: u128,
    pub name: String,
    /// Positive is owed by the shop. Negative means the shop has paid ahead,
    /// which happens and is worth showing rather than hiding.
    pub owed_minor: i64,
    pub deliveries: u32,
    /// When the oldest delivery still in this balance arrived, which is what
    /// tells an owner this has been running since March.
    pub since_ms: u64,
}

/// What a period owes the revenue: one row per rate, and how much of it is
/// still waiting on somebody to look.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VatSummary {
    pub rows: Vec<VatRow>,
    /// Sales in the period that are in the repair queue and not yet dealt with,
    /// and the tax they account for.
    ///
    /// Counted, not removed. A duplicate receipt over-declares and a sale a
    /// person has not looked at yet may be either, and this is a figure a shop
    /// signs its name to: the machine says how much of it is uncertain and the
    /// person filing decides, exactly as they do with a drawer that came up
    /// short.
    pub waiting_sales: u64,
    pub waiting_vat_minor: i64,
}

/// One thing a supervisor waived, and the sale it was waived on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaivedRow {
    pub sale_id: u128,
    pub terminal: u128,
    pub rung_at_ms: u64,
    pub total_minor: i64,
    pub reason: String,
}

/// What was sold at one rate over a period, and the tax on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VatRow {
    pub vat_bp: u32,
    /// 0 standard rated, 1 zero rated, 2 exempt. A rate of zero cannot say
    /// which of the last two a shop meant, and the two are declared in
    /// different places.
    pub supply: u8,
    pub net_minor: i64,
    pub vat_minor: i64,
    /// How many sales carried a line at this rate. Not a count of lines: a
    /// shop reading a return wants to know how much of its trading this is.
    pub sales: u64,
}

/// What a period looked like beyond the sales: the drawers that were counted in
/// it, and what moved on the account book.
///
/// Summed in the store rather than pulled and added, for the same reason the
/// takings are: a shop's day is thousands of rows and the answer is a handful of
/// numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DaySummary {
    pub drawers_counted: u32,
    /// What those drawers were expected to hold, and what was in them. The
    /// difference is not the sum of the variances by accident: it is the same
    /// arithmetic, and showing both is what lets an owner see a long day made
    /// of small shortages rather than one big one.
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
    /// Put on somebody's account in the period, taken off it, and struck off
    /// without money. Three numbers rather than one, because money the shop was
    /// given and money it gave up are not the same thing.
    pub charged_minor: i64,
    /// Goods brought back by somebody who took them on account, shown as what
    /// came off the book.
    pub returned_minor: i64,
    pub paid_minor: i64,
    pub written_off_minor: i64,
}

/// Somebody the shop lets buy on account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerRecord {
    pub id: u128,
    pub name: String,
    pub phone: Option<String>,
    /// False when the shop has stopped their account. Kept rather than deleted:
    /// what they already owe does not stop being owed.
    pub active: bool,
    /// Their Business Identification Number, when the buyer is a business. What
    /// a tax invoice here has to name when a shop sells to one.
    pub bin: Option<String>,
    /// The most they may owe at once, in poisha. Zero is no cap, which is what
    /// everybody has until an owner says otherwise.
    pub limit_minor: i64,
}

/// A drawer a till has open right now, as it last reported.
///
/// Not a record of anything that happened: the record is the counted drawer,
/// written when it closes. This is what a till last said about one that has not
/// closed yet, so an owner can see a drawer left open overnight before the
/// tablet holding it is wiped in the morning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDrawer {
    pub terminal: u128,
    pub shift: u128,
    pub opened_at_ms: u64,
    /// When the till last said this. A figure from four hours ago and one from
    /// four minutes ago are different things, and only the shop can say which
    /// matters.
    pub reported_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
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
/// What a period made, and how much of it the shop can answer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MadeSummary {
    /// Turnover before tax, which is what a margin is taken on: the tax was
    /// never the shop's money.
    pub net_minor: i64,
    /// What those goods cost, over the sales that carry a cost.
    pub cost_minor: i64,
    /// Turnover less cost, over the same sales.
    pub made_minor: i64,
    /// How many sales are in the figure.
    pub sales: u64,
    /// And how many of the period's sales are not, because something on them
    /// has no cost recorded. Their turnover is not in `net_minor` either: half
    /// a margin is worse than none.
    pub sales_without_cost: u64,
    /// What those uncosted sales came to before tax, so an owner can see how
    /// much of the period this figure does not cover.
    pub net_without_cost_minor: i64,
}

/// One sale as the shop holds it, for the person at the counter.
///
/// The bytes are carried rather than decoded here, because decoding is the http
/// layer's job everywhere else in this file and a repository that decoded would
/// be two things at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleOnPaper {
    pub id: u128,
    pub terminal: u128,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub total_minor: i64,
    pub payload: Vec<u8>,
    /// What it was held for, in the words the repair queue uses. Words rather
    /// than the enum, because a sale that arrived by import has words and no
    /// enum, and a person reading this is owed the same sentence either way.
    pub held_for: Option<String>,
    /// The same reason as the enum, for a screen wording it in the shop's
    /// language. Empty for a sale the shop took, and for one held before the
    /// column existed.
    pub held_for_bytes: Vec<u8>,
    /// What somebody decided about it, and whether it still counts.
    pub decided: Option<(String, bool)>,
    /// What has been given back against this receipt, as a positive amount.
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairItem {
    pub id: u128,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    pub received_at_ms: u64,
    /// The sentence the server decided on when it held the sale. What an
    /// operator read then, and what any screen falls back to.
    pub reason: String,
    /// The reason itself, as postcard, for a screen wording it in the shop's
    /// language. Empty for a sale held before the column existed: those can
    /// only ever be shown as the sentence above.
    pub reason_bytes: Vec<u8>,
}

/// One answer somebody gave about one sale: when, what they wrote, and whether
/// the sale stood.
type Decision = (u64, String, bool);

/// What became of an attempt to change an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decided {
    /// The answer changed and every figure has moved with it.
    Changed,
    /// Nobody had answered about this sale, so it is still in the queue, which
    /// is where a first answer is given.
    Unanswered,
    /// Somebody else answered between the list being read and this arriving.
    /// Nothing was changed: a stale view must not become the current one.
    Stale,
}

/// A sale somebody has already decided about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecidedSale {
    pub id: u128,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    /// Why it was held in the first place, in the words the server used then.
    pub reason: String,
    /// What the person wrote when they decided.
    pub note: String,
    /// Whether the sale stands. False means every figure is ignoring it.
    pub kept: bool,
    pub decided_at_ms: u64,
    /// How many times it has been decided. Two or more is a shop that changed
    /// its mind, which is worth showing rather than hiding.
    pub decisions: u32,
}

/// A run of receipt numbers the shop has no sale for.
///
/// A shop's numbering is meant to be unbroken. A gap is one of two things and
/// the shop is the only one who can tell which: numbers rung on a device that
/// has not synced yet, which close by themselves, or numbers that went with a
/// device that was wiped or lost, which never will.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptGap {
    pub terminal: u128,
    /// The series. A terminal the shop declared replaced starts a new one, so
    /// numbers under two epochs are two sequences rather than one with a hole.
    pub epoch: u64,
    /// The number before the gap and the number after it, as they are printed.
    pub after: String,
    pub before: String,
    /// How many numbers are missing between them.
    pub missing: u64,
}

/// Split a printed receipt number into what the till prints and the number it
/// counts, which is everything after the last dash.
fn split_receipt(receipt: &str) -> Option<(String, u64)> {
    let (prefix, digits) = receipt.rsplit_once('-')?;
    digits
        .parse()
        .ok()
        .map(|number| (prefix.to_owned(), number))
}

/// The same shape the till prints, so a gap is reported in the numbers a person
/// is looking at rather than in bare integers.
pub(crate) fn format_receipt(prefix: &str, number: u64) -> String {
    format!("{prefix}-{number:06}")
}

/// A privileged action a till allowed, and on whose authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedAction {
    pub terminal: u128,
    /// The device's own count of what it has allowed, ever.
    pub seq: u64,
    /// The device's clock, shown as what the till thought the time was, which
    /// is what the person standing at it saw.
    pub at_ms: u64,
    /// 1 discount, 2 price override, 3 refund, 4 void a line, 5 open the
    /// drawer, 6 close the drawer, 7 a PIN typed wrongly, 8 a PIN typed wrongly
    /// that locked that person out, 9 somebody signing in, 10 more sold than
    /// the shop has, 11 tried to take a line off a basket that had been paid
    /// towards, 12 sold to somebody already past what they may owe, 13 tried to
    /// open the drawer, 14 a receipt printed again.
    ///
    /// Stored as the number rather than as words, because the words are the
    /// screen's business and a shop reading its trail in Bangla reads the same
    /// rows as one reading it in English.
    pub action: u8,
    pub bp: u32,
    pub operator: u128,
    pub operator_name: String,
    /// Zero when nobody had to allow it.
    pub authorised_by: u128,
    pub authorised_by_name: String,
    /// The receipt a reprint was of. `None` for every other kind, and for
    /// anything written before a device carried one.
    pub receipt_no: Option<String>,
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
    /// The highest role any live credential of this device holds: 2 for one
    /// that is the back office as well, 1 for a till, 0 for a device holding
    /// no credential at all because the shop withdrew it.
    ///
    /// On the credential rather than on the terminal, which is where roles have
    /// always lived here. It is on this list because the shop has to be able to
    /// give the back office a new code when the tablet running it is lost, and
    /// a screen that cannot tell which device that is can only offer a till's.
    pub role: u8,
}

/// Turn a quarantine reason into the sentence a shopkeeper reads.
///
/// Stored and returned as text rather than as a structured code. It is read by a
/// human deciding what to do about a sale, never queried on, and text cannot
/// drift out of step with the enum the way a numeric code would after a release
/// that adds a variant. Both repositories call this, so the queue reads the same
/// whether it is served from Postgres or from memory.
///
/// Every figure in it is written the way the shop writes one. This used to print
/// poisha as a bare integer, thousandths as "thousandths", and a moment as the
/// milliseconds since 1970: an owner deciding whether a sale is real was reading
/// "rung at 1788600000000", which is a number nobody outside this repository can
/// act on. A clock that is wrong is described by how far out it is, because that
/// is the fact, and because the shop's own hour is the screen's to know and not
/// this server's.
#[must_use]
pub fn describe_quarantine(reason: &QuarantineReason) -> String {
    use openpos_core::receipt::{money_of, quantity_of};

    /// A gap between two moments, in the largest unit that says something.
    fn how_far_out(from_ms: u64, to_ms: u64) -> String {
        let apart = from_ms.abs_diff(to_ms);
        let minutes = apart / 60_000;
        let hours = minutes / 60;
        let days = hours / 24;
        if days > 0 {
            format!("{days} day{}", if days == 1 { "" } else { "s" })
        } else if hours > 0 {
            format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
        } else if minutes > 0 {
            format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
        } else {
            String::from("under a minute")
        }
    }

    match reason {
        QuarantineReason::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        } => format!(
            "totals mismatch: the till stored {} and the shop recomputed {}",
            money_of(*stored_minor),
            money_of(*recomputed_minor)
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
        QuarantineReason::ClockOutOfRange {
            rung_at_ms,
            received_at_ms,
        } => format!(
            "the till says this was rung {} {} it reached the shop: that device's clock is \
             wrong, so which day this belongs to needs a person",
            how_far_out(*rung_at_ms, *received_at_ms),
            if rung_at_ms > received_at_ms {
                "after"
            } else {
                "before"
            }
        ),
        QuarantineReason::RefundAgainstNothing { receipt_no } => format!(
            "this reverses receipt {receipt_no}, and no sale here carries that number: it may be \
             on a till whose sales have not arrived, or it may be a refund against nothing"
        ),
        QuarantineReason::RefundBeyondTheSale {
            receipt_no,
            sale_minor,
            refunded_minor,
        } => format!(
            "receipt {receipt_no} was rung for {} and {} has now been refunded against it",
            money_of(*sale_minor),
            money_of(*refunded_minor)
        ),
        QuarantineReason::TendersDoNotAddUp {
            total_minor,
            tendered_minor,
            change_minor,
        } => format!(
            "this says it was for {} and carries {} handed over with {} given back: nobody paid \
             what the ticket says it was for",
            money_of(*total_minor),
            money_of(*tendered_minor),
            money_of(*change_minor)
        ),
        QuarantineReason::MoreCameBackThanWentOut {
            receipt_no,
            item_id,
            over_by_milli,
        } => format!(
            "more of item {item_id} has come back against receipt {receipt_no} than that receipt \
             sold, by {}: the money may be right and the goods are not",
            quantity_of(*over_by_milli)
        ),
    }
}

#[cfg(test)]
mod described {
    // Tests assert with plain arithmetic and panic on failure, which is the
    // point of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use openpos_core::protocol::QuarantineReason;

    use super::describe_quarantine;

    #[test]
    fn every_figure_is_written_the_way_the_shop_writes_one() {
        // An owner deciding whether a sale is real was reading "the till stored
        // 21275" and "rung at 1788600000000". Both are numbers nobody outside
        // this repository can act on.
        let said = describe_quarantine(&QuarantineReason::TotalsMismatch {
            stored_minor: 21_275,
            recomputed_minor: 21_300,
        });
        assert!(said.contains("212.75") && said.contains("213.00"), "{said}");

        let said = describe_quarantine(&QuarantineReason::ClockOutOfRange {
            rung_at_ms: 1_788_600_000_000,
            received_at_ms: 1_788_600_000_000 + 3 * 24 * 60 * 60 * 1_000,
        });
        assert!(said.contains("3 days before"), "{said}");
        assert!(!said.contains("1788600000000"), "{said}");

        // The other way round: a device whose clock runs ahead says it rang a
        // sale after the shop had already been handed it.
        let said = describe_quarantine(&QuarantineReason::ClockOutOfRange {
            rung_at_ms: 1_788_600_000_000 + 90 * 60 * 1_000,
            received_at_ms: 1_788_600_000_000,
        });
        assert!(said.contains("1 hour after"), "{said}");

        let said = describe_quarantine(&QuarantineReason::MoreCameBackThanWentOut {
            receipt_no: "T1-000001".into(),
            item_id: 1,
            over_by_milli: 2_500,
        });
        assert!(said.contains("2.5"), "{said}");
        assert!(!said.contains("thousandths"), "{said}");
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
    /// The same reason as the enum, beside the sentence, for a screen wording
    /// it in the shop's language. Keyed the same way, and absent for a sale
    /// held by anything that only knew the words.
    quarantine_kind: HashMap<(u128, u128), Vec<u8>>,
    /// When each sale arrived, keyed as the sales are. Kept beside them rather
    /// than inside `StoredSale`, because that struct is what ingest builds from
    /// a till's own bytes and arrival is the server's fact, not the till's.
    received: HashMap<(u128, u128), u64>,
    /// Every answer anybody gave about a sale, oldest first. The last one is
    /// what counts; the rest are how a shop shows it changed its mind.
    decisions: HashMap<(u128, u128), Vec<Decision>>,
    /// Sales somebody looked at and said were not sales. Kept as a set rather
    /// than a flag on the sale, for the same reason the quarantine reasons are
    /// beside the sales rather than inside them: what a person decided is the
    /// shop's fact, and the sale is the till's.
    struck_out: HashSet<(u128, u128)>,
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
    /// What each till allowed, keyed by shop, terminal, the device's own count
    /// and its clock. The clock is in the key beside the count for the reason
    /// the migration gives: a device that dies between bumping the count and
    /// writing it down comes back and reuses it, and keyed on the count alone
    /// the second record would be dropped as a duplicate.
    allowed: HashMap<(u128, u128, u64, u64), AllowedAction>,
    /// When each of those arrived here, as Postgres records with a default.
    /// Kept beside them rather than inside, because arrival is the server's
    /// fact and a counted drawer is the till's.
    shifts_received: HashMap<(u128, u128), u64>,
    /// Who the shop lets buy on account, by id.
    customers: HashMap<(u128, u128), CustomerRecord>,
    /// Where each shop's settings counter stands.
    settings_seq: HashMap<u128, u64>,
    /// What each till says it has open, by terminal. A position rather than a
    /// history, which is why one terminal has one of these.
    open_drawers: HashMap<(u128, u128), OpenDrawer>,
    /// Money paid to suppliers, by payment id.
    supplier_payments: HashMap<(u128, u128), SupplierPayment>,
    /// What each sale owed the revenue, by rate, keyed as the table is.
    sale_vat: HashMap<(u128, u128, u32, u8), (i64, i64)>,
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

    /// Enrol as of a moment, for a test that then rings sales at a fixed clock.
    ///
    /// A shop enrols a device and then sells on it. A fixture that enrols now
    /// and rings a sale timestamped last week describes a device that sold
    /// before it existed, which the server holds for a person to look at, and
    /// rightly.
    pub fn enrol_at(&self, tenant: u128, terminal: u128, at_ms: u64) {
        self.enrol_labelled(tenant, terminal, "");
        if let Some(record) = self.lock().terminals.get_mut(&(tenant, terminal)) {
            record.enrolled_at_ms = at_ms;
        }
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
        // Enrolled well before the clock the fixtures ring sales at, because a
        // shop enrols a device and then sells on it. A terminal created now and
        // handed a sale timestamped last week is a device that sold before it
        // existed, and the server holds those for a person to look at.
        self.enrol_at(tenant, terminal, 1_700_000_000_000);
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
                stock_rule: 0,
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
            if let Ok(bytes) = postcard::to_allocvec(reason) {
                inner.quarantine_kind.insert((sale.tenant, sale.id), bytes);
            }
        }
        // Arrival is recorded once. A replay stores the same sale again, and the
        // queue should keep showing when it first landed rather than moving to
        // the bottom every time a till retries.
        inner
            .received
            .entry((sale.tenant, sale.id))
            .or_insert_with(now_ms);
        charge_accounts(&mut inner, &sale);
        record_vat(&mut inner, &sale);
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
        record_vat(&mut inner, &sale);
        inner.sales.insert((sale.tenant, sale.id), sale);
        Ok(admission)
    }

    async fn terminal_enrolled_at(&self, tenant: u128, terminal: u128) -> Result<Option<u64>> {
        Ok(self
            .lock()
            .terminals
            .get(&(tenant, terminal))
            .map(|record| record.enrolled_at_ms))
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
        // First writer wins, as Postgres does: a count is an event, and
        // counting again is a new count with a later clock rather than an edit
        // to the last one. This store used to overwrite, which meant a resend
        // carrying different numbers changed a barrier here and was ignored
        // there: a figure that depended on which store a shop was running.
        self.lock()
            .counts
            .entry((tenant, count.id))
            .or_insert_with(|| count.clone());
        Ok(())
    }

    async fn on_hand(&self, tenant: u128, item: u128) -> Result<OnHand> {
        let inner = self.lock();
        let stands = |sale: u128| !inner.struck_out.contains(&(tenant, sale));

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
                .filter(|((owner, id), _)| *owner == tenant && stands(*id))
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
            .filter(|((owner, id), _)| *owner == tenant && stands(*id))
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
        let mut inner = self.lock();
        inner
            .operators
            .insert((tenant, operator.id), operator.clone());
        bump_settings(&mut inner, tenant);
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
        bump_settings(&mut inner, tenant);
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
        bump_settings(&mut inner, tenant);
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
        bump_settings(&mut inner, tenant);
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
            .filter(|((owner, id), _)| {
                *owner == tenant && !inner.struck_out.contains(&(tenant, *id))
            })
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
            inner
                .shifts_received
                .entry((tenant, shift.id))
                .or_insert_with(now_ms);
            // Closed is closed: whatever that till was reporting as open is no
            // longer open, and an open list that still shows it is a list an
            // owner learns to ignore.
            if inner
                .open_drawers
                .get(&(tenant, shift.terminal))
                .is_some_and(|open| open.shift == shift.id)
            {
                inner.open_drawers.remove(&(tenant, shift.terminal));
            }
            held.push(shift.id);
        }
        Ok(held)
    }

    async fn unreadable_changes(&self, tenant: u128, limit: u32) -> Result<Vec<UnreadableChange>> {
        // The memory store holds items rather than encoded payloads, so nothing
        // here can be unreadable. Answering an empty list is the truth for this
        // store rather than a stub: what it holds, it can read.
        let _ = (tenant, limit);
        Ok(Vec::new())
    }

    async fn supplier_statement(
        &self,
        tenant: u128,
        supplier_id: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Vec<SupplierEntry>> {
        let inner = self.lock();
        let mut found: Vec<SupplierEntry> = inner
            .deliveries
            .iter()
            .filter(|((owner, _), receipt)| {
                *owner == tenant
                    && receipt.supplier_id == Some(supplier_id)
                    && receipt.received_at_ms >= from_ms
                    && receipt.received_at_ms <= to_ms
            })
            .map(|(_, receipt)| SupplierEntry {
                at_ms: receipt.received_at_ms,
                delivered: true,
                amount_minor: receipt
                    .lines
                    .iter()
                    .map(|line| {
                        openpos_core::money::Minor::new(line.unit_cost_minor)
                            .mul_qty(openpos_core::money::Milli::new(line.qty_milli))
                            .map_or(0, |amount| amount.get())
                    })
                    .fold(0_i64, i64::saturating_add),
                reference: receipt.reference.clone(),
            })
            .collect();

        found.extend(
            inner
                .supplier_payments
                .iter()
                .filter(|((owner, _), payment)| {
                    *owner == tenant
                        && payment.supplier_id == supplier_id
                        && payment.paid_at_ms >= from_ms
                        && payment.paid_at_ms <= to_ms
                })
                .map(|(_, payment)| SupplierEntry {
                    at_ms: payment.paid_at_ms,
                    delivered: false,
                    amount_minor: payment.amount_minor,
                    reference: payment.note.clone(),
                }),
        );

        // Oldest first, and a delivery before a payment made in the same
        // millisecond: goods arrive and are paid for, not the other way round.
        found.sort_by(|left, right| {
            left.at_ms
                .cmp(&right.at_ms)
                .then_with(|| right.delivered.cmp(&left.delivered))
        });
        Ok(found)
    }

    async fn sold(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> Result<Vec<SoldRow>> {
        let inner = self.lock();
        let mut totals: HashMap<u128, SoldRow> = HashMap::new();
        for sale in inner
            .sales
            .iter()
            .filter(|((owner, id), sale)| {
                *owner == tenant
                    && sale.rung_at_ms >= from_ms
                    && sale.rung_at_ms <= to_ms
                    && !inner.struck_out.contains(&(tenant, *id))
            })
            .map(|(_, sale)| sale)
        {
            for (item, qty_milli) in &sale.stock {
                let row = totals.entry(*item).or_insert(SoldRow {
                    item_id: *item,
                    qty_milli: 0,
                    sales: 0,
                });
                // Stock moves the opposite way to a sale: what left the shelf is
                // the negative of the movement.
                row.qty_milli = row.qty_milli.saturating_sub(*qty_milli);
                row.sales = row.sales.saturating_add(1);
            }
        }
        let mut found: Vec<SoldRow> = totals
            .into_values()
            .filter(|row| row.qty_milli != 0)
            .collect();
        found.sort_by(|left, right| {
            right
                .qty_milli
                .cmp(&left.qty_milli)
                .then_with(|| left.item_id.cmp(&right.item_id))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn pay_supplier(&self, tenant: u128, payment: &SupplierPayment) -> Result<bool> {
        let mut inner = self.lock();
        if inner.supplier_payments.contains_key(&(tenant, payment.id)) {
            // Already recorded. A dropped reply is the usual reason one is sent
            // twice, and counting it twice is money the shop believes it paid.
            return Ok(false);
        }
        inner
            .supplier_payments
            .insert((tenant, payment.id), payment.clone());
        Ok(true)
    }

    async fn supplier_owing(&self, tenant: u128) -> Result<Vec<SupplierOwing>> {
        let inner = self.lock();
        let mut totals: HashMap<u128, SupplierOwing> = HashMap::new();

        for receipt in inner
            .deliveries
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, receipt)| receipt)
        {
            // A delivery from nobody is a delivery nobody can be asked about,
            // and it is already in the list of what came in.
            let Some(supplier) = receipt.supplier_id else {
                continue;
            };
            // The same arithmetic a line on a receipt uses, from the same
            // crate: a delivery total worked out one way here and another way
            // on a screen is two answers to one question.
            let total = receipt
                .lines
                .iter()
                .map(|line| {
                    openpos_core::money::Minor::new(line.unit_cost_minor)
                        .mul_qty(openpos_core::money::Milli::new(line.qty_milli))
                        .map_or(0, |amount| amount.get())
                })
                .fold(0_i64, i64::saturating_add);
            let name = inner
                .suppliers
                .get(&(tenant, supplier))
                .map(|known| known.name.clone())
                .unwrap_or_default();
            let entry = totals.entry(supplier).or_insert(SupplierOwing {
                supplier_id: supplier,
                name,
                owed_minor: 0,
                deliveries: 0,
                since_ms: receipt.received_at_ms,
            });
            entry.owed_minor = entry.owed_minor.saturating_add(total);
            entry.deliveries = entry.deliveries.saturating_add(1);
            entry.since_ms = entry.since_ms.min(receipt.received_at_ms);
        }

        for payment in inner
            .supplier_payments
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, payment)| payment)
        {
            let name = inner
                .suppliers
                .get(&(tenant, payment.supplier_id))
                .map(|known| known.name.clone())
                .unwrap_or_default();
            let entry = totals.entry(payment.supplier_id).or_insert(SupplierOwing {
                supplier_id: payment.supplier_id,
                name,
                owed_minor: 0,
                deliveries: 0,
                since_ms: payment.paid_at_ms,
            });
            entry.owed_minor = entry.owed_minor.saturating_sub(payment.amount_minor);
        }

        let mut found: Vec<SupplierOwing> = totals
            .into_values()
            .filter(|owing| owing.owed_minor != 0)
            .collect();
        found.sort_by(|left, right| {
            right
                .owed_minor
                .cmp(&left.owed_minor)
                .then_with(|| left.supplier_id.cmp(&right.supplier_id))
        });
        Ok(found)
    }

    async fn waived(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> Result<Vec<WaivedRow>> {
        let inner = self.lock();
        let mut found: Vec<WaivedRow> = inner
            .sales
            .values()
            .filter(|sale| {
                sale.tenant == tenant
                    && sale.rung_at_ms >= from_ms
                    && sale.rung_at_ms <= to_ms
                    // What a supervisor allowed on a sale that never happened
                    // is not something anybody gave away.
                    && !inner.struck_out.contains(&(tenant, sale.id))
            })
            .flat_map(|sale| {
                sale.overrides.iter().map(|reason| WaivedRow {
                    sale_id: sale.id,
                    terminal: sale.terminal,
                    rung_at_ms: sale.rung_at_ms,
                    total_minor: sale.total_minor,
                    reason: reason.clone(),
                })
            })
            .collect();
        found.sort_by(|left, right| {
            right
                .rung_at_ms
                .cmp(&left.rung_at_ms)
                .then_with(|| right.sale_id.cmp(&left.sale_id))
        });
        found.truncate(usize::try_from(limit.max(1)).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn made(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<MadeSummary> {
        let inner = self.lock();
        let mut summary = MadeSummary::default();
        for ((owner, id), sale) in inner.sales.iter() {
            if *owner != tenant || sale.rung_at_ms < from_ms || sale.rung_at_ms > to_ms {
                continue;
            }
            // Struck out: somebody said it was not a sale, so it made nothing.
            if inner.struck_out.contains(&(tenant, *id)) {
                continue;
            }
            // Before tax, which is what a margin is taken on: the tax was never
            // the shop's money.
            let net: i64 = inner
                .sale_vat
                .iter()
                .filter(|((held_owner, held_sale, _, _), _)| {
                    *held_owner == tenant && held_sale == id
                })
                .fold(0_i64, |sum, (_, (net, _))| sum.saturating_add(*net));
            if sale.cost_known {
                summary.sales = summary.sales.saturating_add(1);
                summary.net_minor = summary.net_minor.saturating_add(net);
                summary.cost_minor = summary.cost_minor.saturating_add(sale.cost_minor);
            } else {
                summary.sales_without_cost = summary.sales_without_cost.saturating_add(1);
                summary.net_without_cost_minor = summary.net_without_cost_minor.saturating_add(net);
            }
        }
        summary.made_minor = summary.net_minor.saturating_sub(summary.cost_minor);
        Ok(summary)
    }

    async fn vat_summary(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<VatSummary> {
        let inner = self.lock();
        let mut rows: HashMap<(u32, u8), VatRow> = HashMap::new();
        let mut summary = VatSummary::default();
        let mut waiting: Vec<u128> = Vec::new();
        for ((owner, sale_id, bp, supply), (net, vat)) in inner.sale_vat.iter() {
            if *owner != tenant {
                continue;
            }
            // A sale outside the period is not in the return, and one this
            // store has forgotten is not either.
            let Some(sale) = inner.sales.get(&(tenant, *sale_id)) else {
                continue;
            };
            // Struck out: somebody looked at this and said it was not a sale,
            // so it is not tax the shop collected either.
            if inner.struck_out.contains(&(tenant, *sale_id)) {
                continue;
            }
            if sale.rung_at_ms < from_ms || sale.rung_at_ms > to_ms {
                continue;
            }
            let row = rows.entry((*bp, *supply)).or_insert(VatRow {
                vat_bp: *bp,
                supply: *supply,
                net_minor: 0,
                vat_minor: 0,
                sales: 0,
            });
            row.net_minor = row.net_minor.saturating_add(*net);
            row.vat_minor = row.vat_minor.saturating_add(*vat);
            row.sales = row.sales.saturating_add(1);

            // In the figure, and counted separately: a sale nobody has looked
            // at yet may be a duplicate that over-declares, and the person
            // signing the return decides rather than the machine.
            let unresolved = inner.quarantine.contains_key(&(tenant, *sale_id))
                && !inner.resolutions.contains_key(&(tenant, *sale_id));
            if unresolved {
                summary.waiting_vat_minor = summary.waiting_vat_minor.saturating_add(*vat);
                if !waiting.contains(sale_id) {
                    waiting.push(*sale_id);
                }
            }
        }
        summary.waiting_sales = u64::try_from(waiting.len()).unwrap_or_default();
        summary.rows = rows.into_values().collect();
        summary.rows.sort_by_key(|row| (row.vat_bp, row.supply));
        Ok(summary)
    }

    async fn day_summary(&self, tenant: u128, from_ms: u64, to_ms: u64) -> Result<DaySummary> {
        let inner = self.lock();
        let mut summary = DaySummary::default();

        // A drawer is not adjusted by a sale struck out afterwards. What a till
        // expected and what a person counted are a record of one evening, and a
        // duplicate cash sale that inflated the expectation is exactly what the
        // shortfall that evening was. Rewriting the expectation now would erase
        // the evidence and make an evening that did not reconcile look as
        // though it had. So the takings can be lower than the cash a drawer
        // expected in the same report, and the difference is the thing somebody
        // is meant to read.
        for shift in inner
            .shifts
            .iter()
            .filter(|((owner, _), shift)| {
                *owner == tenant && shift.closed_at_ms >= from_ms && shift.closed_at_ms <= to_ms
            })
            .map(|(_, shift)| shift)
        {
            summary.drawers_counted = summary.drawers_counted.saturating_add(1);
            summary.expected_cash_minor = summary
                .expected_cash_minor
                .saturating_add(shift.expected_cash_minor);
            summary.counted_cash_minor = summary
                .counted_cash_minor
                .saturating_add(shift.counted_cash_minor);
            summary.variance_minor = summary.variance_minor.saturating_add(shift.variance_minor);
        }

        for row in inner
            .accounts
            .iter()
            .filter(|((owner, _, _), row)| {
                *owner == tenant
                    && row.at_ms >= from_ms
                    && row.at_ms <= to_ms
                    && !(row.is_sale && inner.struck_out.contains(&(tenant, row.source_id)))
            })
            .map(|(_, row)| row)
        {
            if row.is_sale {
                // Split rather than netted: goods taken on account and goods
                // brought back are different things, and a day that nets to
                // zero because one balanced the other is a day somebody should
                // look at.
                if row.amount_minor < 0 {
                    summary.returned_minor =
                        summary.returned_minor.saturating_sub(row.amount_minor);
                } else {
                    summary.charged_minor = summary.charged_minor.saturating_add(row.amount_minor);
                }
            } else if row.written_off {
                // Stored negative, shown as what was given up.
                summary.written_off_minor =
                    summary.written_off_minor.saturating_sub(row.amount_minor);
            } else {
                summary.paid_minor = summary.paid_minor.saturating_sub(row.amount_minor);
            }
        }
        Ok(summary)
    }

    async fn item_now(&self, tenant: u128, item_id: u128) -> Result<Option<(ItemWire, u64)>> {
        let inner = self.lock();
        // The newest change naming that item, which is where it stands.
        let found = inner.changes.get(&tenant).and_then(|changes| {
            changes
                .iter()
                .rev()
                .find(|(_, change)| match change {
                    CatalogueChange::Upsert(item) => item.id == item_id,
                    CatalogueChange::Delete(id) => *id == item_id,
                })
                .map(|(seq, change)| (*seq, change.clone()))
        });
        Ok(match found {
            Some((seq, CatalogueChange::Upsert(item))) => Some((*item, seq)),
            // Withdrawn: it stands at that sequence and there is nothing to
            // show, which is different from never having existed.
            Some((_, CatalogueChange::Delete(_))) | None => None,
        })
    }

    async fn settings_seq(&self, tenant: u128) -> Result<u64> {
        Ok(self
            .lock()
            .settings_seq
            .get(&tenant)
            .copied()
            .unwrap_or_default())
    }

    async fn put_customer(&self, tenant: u128, customer: &CustomerRecord) -> Result<()> {
        let mut inner = self.lock();
        inner
            .customers
            .insert((tenant, customer.id), customer.clone());
        bump_settings(&mut inner, tenant);
        Ok(())
    }

    async fn customers(&self, tenant: u128) -> Result<Vec<CustomerRecord>> {
        let inner = self.lock();
        let mut found: Vec<CustomerRecord> = inner
            .customers
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, customer)| customer.clone())
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(found)
    }

    async fn put_open_drawer(&self, tenant: u128, drawer: &OpenDrawer) -> Result<()> {
        self.lock()
            .open_drawers
            .insert((tenant, drawer.terminal), drawer.clone());
        Ok(())
    }

    async fn open_drawers(&self, tenant: u128) -> Result<Vec<OpenDrawer>> {
        let inner = self.lock();
        let mut found: Vec<OpenDrawer> = inner
            .open_drawers
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, drawer)| drawer.clone())
            .collect();
        found.sort_by_key(|drawer| drawer.opened_at_ms);
        Ok(found)
    }

    async fn put_allowed(
        &self,
        tenant: u128,
        terminal: u128,
        allowed: &[AllowedAction],
    ) -> Result<Vec<u64>> {
        let mut inner = self.lock();
        let mut held = Vec::with_capacity(allowed.len());
        for one in allowed {
            // First writer wins. A resend after a dropped reply must not
            // rewrite what the shop already holds about who allowed what.
            inner
                .allowed
                .entry((tenant, terminal, one.seq, one.at_ms))
                .or_insert_with(|| AllowedAction {
                    terminal,
                    ..one.clone()
                });
            held.push(one.seq);
        }
        Ok(held)
    }

    async fn tenant_created_at(&self, tenant: u128) -> Result<Option<u64>> {
        let inner = self.lock();
        // This store keeps no creation date, so it answers with the oldest
        // enrolment it holds: the same bound, from the same shop's own records.
        Ok(inner
            .terminals
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, record)| record.enrolled_at_ms)
            .min())
    }

    async fn sales_on_receipt(&self, tenant: u128, receipt_no: &str) -> Result<Vec<SaleOnPaper>> {
        let inner = self.lock();
        // What has been given back against this number, worked out once for
        // whatever carries it: a refund names the receipt it reverses, and its
        // own total is negative.
        let refunded: i64 = inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .filter(|(_, sale)| sale.refund_of.as_deref() == Some(receipt_no))
            .filter(|((_, id), _)| !inner.struck_out.contains(&(tenant, *id)))
            .fold(0_i64, |sum, (_, sale)| {
                sum.saturating_add(sale.total_minor.saturating_neg())
            });

        let mut found: Vec<SaleOnPaper> = inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .filter(|(_, sale)| sale.receipt_no.as_deref() == Some(receipt_no))
            .map(|((_, id), sale)| SaleOnPaper {
                id: *id,
                terminal: sale.terminal,
                receipt_no: receipt_no.to_owned(),
                rung_at_ms: sale.rung_at_ms,
                total_minor: sale.total_minor,
                payload: sale.payload.clone(),
                held_for: inner.quarantine.get(&(tenant, *id)).cloned(),
                held_for_bytes: inner
                    .quarantine_kind
                    .get(&(tenant, *id))
                    .cloned()
                    .unwrap_or_default(),
                decided: inner
                    .resolutions
                    .get(&(tenant, *id))
                    .map(|said| (said.clone(), !inner.struck_out.contains(&(tenant, *id)))),
                // Only against the sale itself. A refund does not have money
                // given back against it; it is the money given back.
                refunded_minor: if sale.refund_of.is_none() {
                    refunded
                } else {
                    0
                },
                refund_of: sale.refund_of.clone(),
            })
            .collect();
        // Oldest first, which is the order they were rung and the order the two
        // of them have to be read in when there are two.
        found.sort_by_key(|one| (one.rung_at_ms, one.id));
        Ok(found)
    }

    async fn refunded_against(&self, tenant: u128, receipt_no: &str) -> Result<Option<(i64, i64)>> {
        let inner = self.lock();
        // The sale that carries the number, which is the one that is not itself
        // a refund of it: a refund has its own receipt number and names this one
        // as what it reverses.
        let sold = inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .find(|(_, sale)| {
                sale.receipt_no.as_deref() == Some(receipt_no) && sale.refund_of.is_none()
            })
            .map(|(_, sale)| sale.total_minor);
        let Some(sold) = sold else {
            return Ok(None);
        };
        let refunded = inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .filter(|(_, sale)| sale.refund_of.as_deref() == Some(receipt_no))
            .map(|(_, sale)| sale.total_minor)
            .sum();
        Ok(Some((sold, refunded)))
    }

    async fn goods_against(&self, tenant: u128, receipt_no: &str) -> Result<Vec<(u128, i64)>> {
        let inner = self.lock();
        let about: Vec<u128> = inner
            .sales
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .filter(|(_, sale)| {
                (sale.receipt_no.as_deref() == Some(receipt_no) && sale.refund_of.is_none())
                    || sale.refund_of.as_deref() == Some(receipt_no)
            })
            .filter(|((_, id), _)| !inner.struck_out.contains(&(tenant, *id)))
            .map(|(_, sale)| sale.id)
            .collect();

        let mut net: Vec<(u128, i64)> = Vec::new();
        for sale in about {
            let Some(stored) = inner.sales.get(&(tenant, sale)) else {
                continue;
            };
            for (item, qty) in &stored.stock {
                match net.iter_mut().find(|(known, _)| known == item) {
                    Some((_, total)) => *total = total.saturating_add(*qty),
                    None => net.push((*item, *qty)),
                }
            }
        }
        Ok(net)
    }

    async fn drawer_takings(
        &self,
        tenant: u128,
        terminal: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Option<i64>> {
        let inner = self.lock();
        // Every sale this store holds was computed on the way in, so it always
        // has an answer. The store a shop runs on holds sales from before.
        Ok(Some(
            inner
                .sales
                .iter()
                .filter(|((owner, _), _)| *owner == tenant)
                .filter(|((_, id), _)| !inner.struck_out.contains(&(tenant, *id)))
                .filter(|(_, sale)| {
                    sale.terminal == terminal
                        && sale.rung_at_ms >= from_ms
                        && sale.rung_at_ms <= to_ms
                })
                .fold(0_i64, |sum, (_, sale)| sum.saturating_add(sale.cash_minor)),
        ))
    }

    async fn barcode_holders(
        &self,
        tenant: u128,
        barcodes: &[String],
    ) -> Result<Vec<(String, u128)>> {
        let inner = self.lock();
        // Where each item stands, which is its newest change. A withdrawn item
        // holds nothing: a shop that stops selling something has its barcode
        // back.
        let mut current: HashMap<u128, Option<ItemWire>> = HashMap::new();
        for (seq, change) in inner.changes.get(&tenant).into_iter().flatten() {
            let _ = seq;
            match change {
                CatalogueChange::Upsert(item) => {
                    current.insert(item.id, Some((**item).clone()));
                }
                CatalogueChange::Delete(id) => {
                    current.insert(*id, None);
                }
            }
        }

        let mut found = Vec::new();
        for item in current.into_values().flatten().filter(|item| item.active) {
            for code in &item.barcodes {
                if barcodes.iter().any(|wanted| wanted == code) {
                    found.push((code.clone(), item.id));
                }
            }
        }
        Ok(found)
    }

    async fn receipt_gaps(&self, tenant: u128, limit: u32) -> Result<Vec<ReceiptGap>> {
        let inner = self.lock();
        // Grouped by the series a number belongs to: the terminal, the epoch,
        // and the prefix the till prints. Two tills counting from one hundred
        // are not a hole in each other's numbering.
        let mut series: HashMap<(u128, u64, String), Vec<u64>> = HashMap::new();
        for sale in inner.sales.values().filter(|sale| sale.tenant == tenant) {
            let (Some(receipt), Some(epoch)) = (sale.receipt_no.as_deref(), sale.receipt_epoch)
            else {
                // A sale rung with no numbers left is numbered by the back
                // office later. It is not a gap; it is a sale waiting for one.
                continue;
            };
            let Some((prefix, number)) = split_receipt(receipt) else {
                continue;
            };
            series
                .entry((sale.terminal, epoch, prefix))
                .or_default()
                .push(number);
        }

        let mut found = Vec::new();
        for ((terminal, epoch, prefix), mut numbers) in series {
            numbers.sort_unstable();
            numbers.dedup();
            for pair in numbers.windows(2) {
                let (before, after) = (pair.first().copied(), pair.get(1).copied());
                let (Some(before), Some(after)) = (before, after) else {
                    continue;
                };
                let missing = after.saturating_sub(before).saturating_sub(1);
                if missing == 0 {
                    continue;
                }
                found.push(ReceiptGap {
                    terminal,
                    epoch,
                    after: format_receipt(&prefix, before),
                    before: format_receipt(&prefix, after),
                    missing,
                });
            }
        }
        found.sort_by(|left, right| {
            left.after
                .cmp(&right.after)
                .then_with(|| left.terminal.cmp(&right.terminal))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn allowed(
        &self,
        tenant: u128,
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    ) -> Result<Vec<AllowedAction>> {
        let inner = self.lock();
        let mut found: Vec<AllowedAction> = inner
            .allowed
            .iter()
            .filter(|((owner, _, _, _), one)| {
                *owner == tenant && one.at_ms >= from_ms && one.at_ms <= to_ms
            })
            .map(|(_, one)| one.clone())
            .collect();
        // Newest first, and by terminal and count when two land in the same
        // millisecond, so this store answers the same way Postgres does.
        found.sort_by(|left, right| {
            right
                .at_ms
                .cmp(&left.at_ms)
                .then_with(|| right.terminal.cmp(&left.terminal))
                .then_with(|| right.seq.cmp(&left.seq))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
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
                received_ms: now_ms(),
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

    async fn customer_balances(&self, tenant: u128) -> Result<Vec<(u128, i64)>> {
        let inner = self.lock();
        let mut totals: HashMap<u128, i64> = HashMap::new();
        for ((owner, _, key), row) in inner.accounts.iter() {
            if *owner != tenant {
                continue;
            }
            // A charge from a sale somebody struck out is not a debt: the goods
            // never left, so nothing is owed for them.
            if row.is_sale && inner.struck_out.contains(&(tenant, row.source_id)) {
                continue;
            }
            // Only entries keyed on somebody the shop wrote down. A debt against
            // a name typed at a till belongs to no record and cannot be shown
            // against one.
            let Some(id) = customer_from_key(key) else {
                continue;
            };
            *totals.entry(id).or_default() = totals
                .get(&id)
                .copied()
                .unwrap_or_default()
                .saturating_add(row.amount_minor);
        }
        Ok(totals.into_iter().filter(|(_, owed)| *owed != 0).collect())
    }

    async fn balance(&self, tenant: u128, person_key: &str) -> Result<i64> {
        let inner = self.lock();
        Ok(inner
            .accounts
            .iter()
            .filter(|((owner, _, key), _)| *owner == tenant && key == person_key)
            .filter(|(_, row)| {
                !(row.is_sale && inner.struck_out.contains(&(tenant, row.source_id)))
            })
            .map(|(_, row)| row.amount_minor)
            .fold(0_i64, i64::saturating_add))
    }

    async fn owed(
        &self,
        tenant: u128,
        after: Option<(i64, String)>,
        limit: u32,
    ) -> Result<Vec<Owing>> {
        let inner = self.lock();
        let mut totals: HashMap<String, Owing> = HashMap::new();
        // The source the shown name came from, per person, so a tie on the
        // clock is broken the same way every time.
        let mut spelled_by: HashMap<String, u128> = HashMap::new();
        for row in inner
            .accounts
            .iter()
            .filter(|((owner, _, _), _)| *owner == tenant)
            .map(|(_, row)| row)
            .filter(|row| !(row.is_sale && inner.struck_out.contains(&(tenant, row.source_id))))
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
            // Latest by the till's clock, and the larger source id when two
            // land in the same millisecond, so this store answers the same way
            // Postgres does rather than however the map happened to iterate.
            let latest_source = spelled_by.get(&row.person_key).copied().unwrap_or_default();
            if (row.at_ms, row.source_id) >= (entry.last_at_ms, latest_source) {
                entry.last_at_ms = row.at_ms;
                spelled_by.insert(row.person_key.clone(), row.source_id);
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
        // Everything after where the last page ended, in that same order.
        if let Some((owed, key)) = after {
            found.retain(|one| {
                one.owed_minor < owed || (one.owed_minor == owed && one.person_key > key)
            });
        }
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn account(
        &self,
        tenant: u128,
        person_key: &str,
        after: Option<(u64, u128)>,
        limit: u32,
    ) -> Result<Vec<AccountEntry>> {
        let inner = self.lock();
        let mut found: Vec<AccountEntry> = inner
            .accounts
            .iter()
            .filter(|((owner, _, key), _)| *owner == tenant && key == person_key)
            .filter(|(_, row)| {
                !(row.is_sale && inner.struck_out.contains(&(tenant, row.source_id)))
            })
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
        if let Some((at, source)) = after {
            found.retain(|one| one.at_ms < at || (one.at_ms == at && one.source_id < source));
        }
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn allowed_after(
        &self,
        tenant: u128,
        after: (u128, u64),
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<AllowedAction>> {
        let inner = self.lock();
        let mut found: Vec<AllowedAction> = inner
            .allowed
            .iter()
            .filter(|((owner, terminal, seq, _), _)| *owner == tenant && (*terminal, *seq) > after)
            .filter(|(_, one)| one.at_ms <= cut_ms)
            .map(|(_, one)| one.clone())
            .collect();
        found.sort_by_key(|one| (one.terminal, one.seq));
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn counts_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<StockCount>> {
        let inner = self.lock();
        let mut found: Vec<StockCount> = inner
            .counts
            .iter()
            .filter(|((owner, id), _)| *owner == tenant && *id > after_id)
            // This store has no arrival clock of its own, so the cut is taken on
            // the counter's own clock. Postgres decides the late-arrival case.
            .filter(|(_, count)| count.counted_at_ms <= cut_ms)
            .map(|(_, count)| count.clone())
            .collect();
        found.sort_by_key(|count| count.id);
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn corrections_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<StockCorrection>> {
        let inner = self.lock();
        let mut found: Vec<StockCorrection> = inner
            .corrections
            .iter()
            .filter(|((owner, id), _)| *owner == tenant && *id > after_id)
            .filter(|(_, entry)| entry.occurred_at_ms <= cut_ms)
            .map(|(_, entry)| entry.clone())
            .collect();
        found.sort_by_key(|entry| entry.id);
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn deliveries_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<GoodsReceipt>> {
        let inner = self.lock();
        let mut found: Vec<GoodsReceipt> = inner
            .deliveries
            .iter()
            .filter(|((owner, id), _)| *owner == tenant && *id > after_id)
            // This store has no arrival clock of its own for a delivery, so the
            // cut is taken on when the goods came in. Postgres is where the
            // late-arrival case is genuinely decided.
            .filter(|(_, receipt)| receipt.received_at_ms <= cut_ms)
            .map(|(_, receipt)| receipt.clone())
            .collect();
        found.sort_by_key(|receipt| receipt.id);
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn supplier_payments_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<SupplierPayment>> {
        let inner = self.lock();
        let mut found: Vec<SupplierPayment> = inner
            .supplier_payments
            .iter()
            .filter(|((owner, id), _)| *owner == tenant && *id > after_id)
            .filter(|(_, payment)| payment.paid_at_ms <= cut_ms)
            .map(|(_, payment)| payment.clone())
            .collect();
        found.sort_by_key(|payment| payment.id);
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
        // Refused here as Postgres refuses it, rather than being laxer: a store
        // that accepts what the other will not is a store tests pass against
        // and production does not. An unexplained correction is stock that left
        // for no reason anybody wrote down.
        if correction.reason.trim().is_empty() {
            return Err(RepoError::Invalid);
        }
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

    async fn now_ms(&self) -> Result<u64> {
        Ok(now_ms())
    }

    async fn sales_after(
        &self,
        tenant: u128,
        after_id: u128,
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<SaleRecord>> {
        let inner = self.lock();
        let mut found: Vec<SaleRecord> = inner
            .sales
            .iter()
            // Arrived before the export started. A sale that lands mid-export
            // is left out of it whole rather than half in.
            .filter(|(key, _)| inner.received.get(key).is_none_or(|at| *at <= cut_ms))
            .filter(|(key, _)| key.0 == tenant && key.1 > after_id)
            .map(|(key, sale)| SaleRecord {
                vat: Vec::new(),
                overrides: Vec::new(),
                resolution: inner
                    .resolutions
                    .get(key)
                    .map(|note| (note.clone(), !inner.struck_out.contains(key))),
                id: sale.id,
                terminal: sale.terminal,
                receipt_no: sale.receipt_no.clone(),
                receipt_epoch: sale.receipt_epoch,
                rung_at_ms: sale.rung_at_ms,
                total_minor: sale.total_minor,
                payload: sale.payload.clone(),
                quarantine: inner.quarantine.get(key).cloned(),
                quarantine_kind: inner.quarantine_kind.get(key).cloned().unwrap_or_default(),
                refund_of: None,
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
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<StockRecord>> {
        let inner = self.lock();
        let mut found: Vec<StockRecord> = inner
            .sales
            .values()
            .filter(|sale| sale.tenant == tenant)
            // A movement belongs to its sale, so it is in the cut when the sale
            // is: a movement with no sale behind it is stock that moved for no
            // reason anybody can point at.
            .filter(|sale| {
                inner
                    .received
                    .get(&(sale.tenant, sale.id))
                    .is_none_or(|at| *at <= cut_ms)
            })
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
                // And the reason itself when the bundle carried it, so a
                // restored shop can still say why in its own language rather
                // than dropping to the English it was stored in.
                if !record.quarantine_kind.is_empty() {
                    inner
                        .quarantine_kind
                        .insert((tenant, record.id), record.quarantine_kind.clone());
                }
            }
            // What somebody decided about it, so a restored shop does not put a
            // struck-out duplicate back into the queue and back into its
            // takings.
            if let Some((note, kept)) = record.resolution.clone() {
                // Only for a sale that was held. Deciding is answering the
                // queue, and a sale that never reached it has nothing to
                // answer.
                if record.quarantine.is_some() {
                    inner
                        .decisions
                        .entry((tenant, record.id))
                        .or_default()
                        .push((now_ms(), note.clone(), kept));
                }
                inner.resolutions.insert((tenant, record.id), note);
                if !kept {
                    inner.struck_out.insert((tenant, record.id));
                }
            }
            // Arrival is the receiving server's fact, so an imported sale gets
            // the time it landed here, exactly as the Postgres column defaults
            // to `now()`. A bundle carries no arrival time, and leaving this
            // absent would show an imported repair queue as dated 1970.
            inner.received.insert((tenant, record.id), now_ms());
            // What it left in a drawer and what its goods cost, read out of the
            // bytes the till committed rather than left at zero.
            let (cash, cost, costed) = crate::ingest::figures_from_payload(&record.payload);
            for (bp, net, vat, supply) in &record.vat {
                inner
                    .sale_vat
                    .entry((tenant, record.id, *bp, *supply))
                    .or_insert((*net, *vat));
            }
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
                    vat: Vec::new(),
                    overrides: Vec::new(),
                    on_account: Vec::new(),
                    refund_of: record.refund_of.clone(),
                    // Read back out of the bytes the till committed, like the
                    // tax rows beside them. A restore that left these at zero
                    // would tell a shop its own history made nothing and that
                    // every drawer it ever counted cannot be checked.
                    cash_minor: cash,
                    cost_minor: cost,
                    cost_known: costed,
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
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClosedShift>> {
        let inner = self.lock();
        let mut found: Vec<ClosedShift> = inner
            .shifts
            .iter()
            .filter(|((owner, id), _)| *owner == tenant && *id > after)
            .filter(|(key, _)| {
                inner
                    .shifts_received
                    .get(key)
                    .is_none_or(|at| *at <= cut_ms)
            })
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
        cut_ms: u64,
        limit: u32,
    ) -> Result<Vec<AccountRecord>> {
        let inner = self.lock();
        let mut found: Vec<AccountRecord> = inner
            .accounts
            .iter()
            .filter(|((owner, source, key), _)| *owner == tenant && (*source, key.clone()) > after)
            .filter(|(_, row)| row.received_ms <= cut_ms)
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
                    received_ms: now_ms(),
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
                    reason_bytes: inner.quarantine_kind.get(key).cloned().unwrap_or_default(),
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

    async fn resolve_quarantine(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
        kept: bool,
    ) -> Result<bool> {
        let mut inner = self.lock();
        let quarantined = inner.quarantine.contains_key(&(tenant, sale));
        if !quarantined || inner.resolutions.contains_key(&(tenant, sale)) {
            return Ok(false);
        }
        let at = now_ms();
        inner.resolutions.insert((tenant, sale), note.to_owned());
        inner
            .decisions
            .entry((tenant, sale))
            .or_default()
            .push((at, note.to_owned(), kept));
        if !kept {
            inner.struck_out.insert((tenant, sale));
        }
        Ok(true)
    }

    async fn decided(&self, tenant: u128, limit: u32) -> Result<Vec<DecidedSale>> {
        let inner = self.lock();
        let mut found: Vec<DecidedSale> = inner
            .decisions
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .filter_map(|(key, answers)| {
                let sale = inner.sales.get(key)?;
                let (at, note, kept) = answers.last()?;
                Some(DecidedSale {
                    id: sale.id,
                    receipt_no: sale.receipt_no.clone(),
                    total_minor: sale.total_minor,
                    // Why it was held, in the words the server used then.
                    reason: inner.quarantine.get(key).cloned().unwrap_or_default(),
                    note: note.clone(),
                    kept: *kept,
                    decided_at_ms: *at,
                    decisions: u32::try_from(answers.len()).unwrap_or(u32::MAX),
                })
            })
            .collect();
        // Newest first, and by id when two land in the same millisecond, so
        // somebody looking for the answer they just gave finds it at the top.
        found.sort_by(|left, right| {
            right
                .decided_at_ms
                .cmp(&left.decided_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(found)
    }

    async fn decide_again(
        &self,
        tenant: u128,
        sale: u128,
        note: &str,
        kept: bool,
        expected: u32,
    ) -> Result<Decided> {
        let mut inner = self.lock();
        let Some(answers) = inner.decisions.get(&(tenant, sale)) else {
            // Never decided, so there is nothing to change. The queue is where
            // a first answer is given.
            return Ok(Decided::Unanswered);
        };
        let seen = u32::try_from(answers.len()).unwrap_or(u32::MAX);
        if expected != 0 && expected != seen {
            return Ok(Decided::Stale);
        }
        let at = now_ms();
        inner.resolutions.insert((tenant, sale), note.to_owned());
        inner
            .decisions
            .entry((tenant, sale))
            .or_default()
            .push((at, note.to_owned(), kept));
        if kept {
            inner.struck_out.remove(&(tenant, sale));
        } else {
            inner.struck_out.insert((tenant, sale));
        }
        Ok(Decided::Changed)
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
                    // The highest role this device still holds a credential
                    // for. Zero when the shop has withdrawn every one of them,
                    // which is a device that cannot come back as anything until
                    // somebody gives it a code.
                    role: inner
                        .tokens
                        .values()
                        .filter(|held| held.tenant == tenant && held.terminal == *terminal)
                        .map(|held| held.role as u8)
                        .max()
                        .unwrap_or_default(),
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

    async fn item_has_history(&self, tenant: u128, item_id: u128) -> Result<bool> {
        let inner = self.lock();
        // Every way an item can have been part of the shop's trading. A sale
        // that was later struck out still counts: it happened, somebody
        // answered for it, and the answer names this item.
        let sold = inner
            .sales
            .values()
            .filter(|sale| sale.tenant == tenant)
            .any(|sale| sale.stock.iter().any(|(item, _)| *item == item_id));
        let delivered = inner
            .deliveries
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .flat_map(|(_, receipt)| receipt.lines.iter())
            .any(|line| line.item_id == item_id);
        let corrected = inner
            .corrections
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .any(|(_, entry)| entry.item_id == item_id);
        let counted = inner
            .counts
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .any(|(_, count)| count.item_id == item_id);
        Ok(sold || delivered || corrected || counted)
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
    async fn goods_brought_back_on_account_are_counted_apart_from_what_went_on() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let charge = |id: u128, amount_minor: i64| StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id,
            receipt_no: None,
            receipt_epoch: None,
            rung_at_ms: 1_788_600_000_000,
            total_minor: amount_minor,
            payload: vec![],
            quarantine: None,
            stock: vec![],
            vat: vec![],
            overrides: Vec::new(),
            on_account: vec![AccountCharge {
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor,
            }],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        };
        repo.store_sale(charge(910, 29_450)).await.unwrap();
        // Half of it brought back, which is a negative charge and not a payment
        // nobody made.
        repo.store_sale(charge(911, -10_000)).await.unwrap();

        assert_eq!(repo.balance(TENANT, "karim").await.unwrap(), 19_450);
        let day = repo
            .day_summary(TENANT, 1_788_500_000_000, 1_788_700_000_000)
            .await
            .unwrap();
        assert_eq!(day.charged_minor, 29_450, "what went on the book");
        assert_eq!(day.returned_minor, 10_000, "and what came back off it");
        assert_eq!(day.paid_minor, 0, "nobody handed over any money");
    }

    #[tokio::test]
    async fn the_numbering_gaps_read_the_same_as_postgres() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let sale = |id: u128, receipt: &str| StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id,
            receipt_no: Some(receipt.to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![],
            quarantine: None,
            stock: vec![],
            vat: vec![],
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        };
        for (id, receipt) in [(920, "T1-000100"), (921, "T1-000101"), (922, "T1-000104")] {
            repo.store_sale(sale(id, receipt)).await.unwrap();
        }

        let found = repo.receipt_gaps(TENANT, 50).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].after, "T1-000101");
        assert_eq!(found[0].before, "T1-000104");
        assert_eq!(found[0].missing, 2);

        // A sale rung with no numbers left is waiting for one, not a hole.
        let mut unnumbered = sale(923, "T1-000109");
        unnumbered.receipt_no = None;
        unnumbered.receipt_epoch = None;
        repo.store_sale(unnumbered).await.unwrap();
        assert_eq!(repo.receipt_gaps(TENANT, 50).await.unwrap().len(), 1);

        // And they close when the sales arrive.
        repo.store_sale(sale(924, "T1-000102")).await.unwrap();
        repo.store_sale(sale(925, "T1-000103")).await.unwrap();
        assert!(repo.receipt_gaps(TENANT, 50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_correction_with_no_reason_is_refused_here_too() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let blank = StockCorrection {
            id: 700,
            item_id: 1,
            qty_milli: -1_000,
            reason: String::new(),
            occurred_at_ms: 1_788_600_000_000,
            recorded_by: 70,
        };
        assert_eq!(
            repo.correct_stock(TENANT, &blank).await,
            Err(RepoError::Invalid),
            "as Postgres refuses it"
        );
        assert_eq!(repo.on_hand(TENANT, 1).await.unwrap().qty_milli, 0);
    }

    #[tokio::test]
    async fn a_count_sent_twice_keeps_what_arrived_first() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let counted = StockCount {
            id: 500,
            item_id: 1,
            counted_milli: 31_000,
            counted_at_ms: 1_788_700_000_000,
            counted_by: 70,
            note: None,
        };
        repo.record_count(TENANT, &counted).await.unwrap();
        // A resend, then the same id carrying a different number. Correcting a
        // count means counting again, which is a new id and a later clock.
        repo.record_count(TENANT, &counted).await.unwrap();
        repo.record_count(
            TENANT,
            &StockCount {
                counted_milli: 99_000,
                ..counted.clone()
            },
        )
        .await
        .unwrap();

        assert_eq!(
            repo.on_hand(TENANT, 1).await.unwrap().qty_milli,
            31_000,
            "the first answer stands, as it does in Postgres"
        );
    }

    #[tokio::test]
    async fn what_a_till_allowed_is_stored_once_and_read_newest_first() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let one = |seq: u64, at_ms: u64, action: u8| AllowedAction {
            terminal: TERMINAL,
            seq,
            at_ms,
            action,
            bp: 0,
            operator: 71,
            operator_name: "Rahima".to_owned(),
            authorised_by: 0,
            authorised_by_name: String::new(),
            receipt_no: None,
        };

        let stored = repo
            .put_allowed(
                TENANT,
                TERMINAL,
                &[one(1, 1_788_600_000_000, 5), one(2, 1_788_600_100_000, 3)],
            )
            .await
            .unwrap();
        assert_eq!(stored, vec![1, 2]);

        // Sent again after a dropped reply, which is ordinary.
        repo.put_allowed(TENANT, TERMINAL, &[one(1, 1_788_600_000_000, 5)])
            .await
            .unwrap();
        // And a count reused after the device forgot the bump, which is not the
        // same record and must not be dropped as one.
        repo.put_allowed(TENANT, TERMINAL, &[one(2, 1_788_600_200_000, 1)])
            .await
            .unwrap();

        let trail = repo.allowed(TENANT, 0, u64::MAX, 50).await.unwrap();
        assert_eq!(trail.len(), 3);
        assert_eq!(trail[0].action, 1, "newest first");
        assert_eq!(trail[2].action, 5);
        // Another shop's trail is not this one's.
        assert!(repo.allowed(999, 0, u64::MAX, 50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_sale_struck_out_stops_counting_everywhere() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);

        // The tablet was restored from Thursday's backup and rang Karim's
        // groceries again on Friday. Two sales, same goods, same debt, and only
        // one of them happened.
        for id in [900_u128, 901] {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal: TERMINAL,
                id,
                receipt_no: Some(format!("T1-{id}")),
                receipt_epoch: Some(1),
                rung_at_ms: 1_788_600_000_000,
                total_minor: 49_450,
                payload: vec![],
                // The second one came in from a restored tablet and the server
                // held it for a person to look at, which is how it reaches the
                // queue at all.
                quarantine: (id == 901).then(|| QuarantineReason::DuplicateReceiptNumber {
                    receipt_no: "T1-900".to_owned(),
                }),
                stock: vec![(5_001, -2_000)],
                vat: vec![(750, 45_998, 3_452, 0)],
                overrides: Vec::new(),
                on_account: vec![AccountCharge {
                    person_key: "karim".to_owned(),
                    person_name: "Karim".to_owned(),
                    amount_minor: 49_450,
                }],
                refund_of: None,
                cash_minor: 0,
                cost_minor: 0,
                cost_known: false,
            })
            .await
            .unwrap();
        }
        let (from, to) = (1_788_500_000_000, 1_788_700_000_000);

        assert_eq!(repo.takings(TENANT, from, to).await.unwrap()[0].sales, 2);
        assert_eq!(repo.balance(TENANT, "karim").await.unwrap(), 98_900);

        // The owner works the queue: the second one was never a sale.
        assert!(
            repo.resolve_quarantine(TENANT, 901, "rung twice after the restore", false)
                .await
                .unwrap()
        );

        let takings = repo.takings(TENANT, from, to).await.unwrap();
        assert_eq!(takings[0].sales, 1, "one sale, not two");
        assert_eq!(takings[0].total_minor, 49_450);
        assert_eq!(
            repo.balance(TENANT, "karim").await.unwrap(),
            49_450,
            "Karim owes for one basket of groceries"
        );
        assert_eq!(
            repo.account(TENANT, "karim", None, 10).await.unwrap().len(),
            1
        );
        assert_eq!(
            repo.owed(TENANT, None, 10).await.unwrap()[0].owed_minor,
            49_450
        );
        assert_eq!(
            repo.day_summary(TENANT, from, to)
                .await
                .unwrap()
                .charged_minor,
            49_450
        );
        let vat = repo.vat_summary(TENANT, from, to).await.unwrap();
        assert_eq!(vat.rows[0].vat_minor, 3_452, "tax on what was sold once");
        assert_eq!(vat.rows[0].sales, 1);
        assert_eq!(
            repo.sold(TENANT, from, to, 10).await.unwrap()[0].qty_milli,
            2_000
        );
        assert_eq!(repo.on_hand(TENANT, 5_001).await.unwrap().qty_milli, -2_000);
    }

    #[tokio::test]
    async fn the_owed_list_pages_the_way_postgres_does() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        // Two people owing the same, which is the tie a cursor on the amount
        // alone would repeat or skip.
        for (index, amount) in [900_i64, 700, 700, 400].iter().enumerate() {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal: TERMINAL,
                id: 1_000 + index as u128,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: 1_788_600_000_000,
                total_minor: *amount,
                payload: vec![],
                quarantine: None,
                stock: vec![],
                vat: vec![],
                overrides: Vec::new(),
                on_account: vec![AccountCharge {
                    person_key: format!("person{index}"),
                    person_name: format!("Person {index}"),
                    amount_minor: *amount,
                }],
                refund_of: None,
                cash_minor: 0,
                cost_minor: 0,
                cost_known: false,
            })
            .await
            .unwrap();
        }

        let mut seen: Vec<(String, i64)> = Vec::new();
        let mut cursor: Option<(i64, String)> = None;
        loop {
            let page = repo.owed(TENANT, cursor.clone(), 2).await.unwrap();
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

        assert_eq!(
            seen.iter().map(|(_, owed)| *owed).collect::<Vec<i64>>(),
            vec![900, 700, 700, 400]
        );
        let mut keys: Vec<&str> = seen.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 4, "no row is served on two pages");
    }

    #[tokio::test]
    async fn a_strike_out_made_in_error_can_be_taken_back() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 902,
            receipt_no: Some("T1-000102".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![],
            quarantine: Some(QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000102".to_owned(),
            }),
            stock: vec![],
            vat: vec![],
            overrides: Vec::new(),
            on_account: vec![AccountCharge {
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor: 49_450,
            }],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();

        // Struck out in error: this was the real sale, not the duplicate, and
        // Karim's debt has just disappeared.
        repo.resolve_quarantine(TENANT, 902, "rung twice", false)
            .await
            .unwrap();
        assert_eq!(repo.balance(TENANT, "karim").await.unwrap(), 0);

        // It is not in the queue any more, so the list of what was decided is
        // the only way back to it.
        assert!(repo.repair_queue(TENANT, 50).await.unwrap().is_empty());
        let decided = repo.decided(TENANT, 50).await.unwrap();
        assert_eq!(decided.len(), 1);
        assert_eq!(decided[0].id, 902);
        assert!(!decided[0].kept);
        assert_eq!(decided[0].decisions, 1);

        assert_eq!(
            repo.decide_again(
                TENANT,
                902,
                "wrong one: the other was the duplicate",
                true,
                1
            )
            .await
            .unwrap(),
            Decided::Changed
        );
        assert_eq!(
            repo.balance(TENANT, "karim").await.unwrap(),
            49_450,
            "the debt comes back"
        );
        let decided = repo.decided(TENANT, 50).await.unwrap();
        assert!(decided[0].kept);
        assert_eq!(
            decided[0].decisions, 2,
            "a shop that changed its mind shows that it did"
        );
        assert_eq!(decided[0].note, "wrong one: the other was the duplicate");

        // The other owner's screen still shows one answer. Pressing there now
        // would put its stale view back as the current one, so it is refused.
        assert_eq!(
            repo.decide_again(TENANT, 902, "no, strike it out", false, 1)
                .await
                .unwrap(),
            Decided::Stale
        );
        assert!(repo.decided(TENANT, 50).await.unwrap()[0].kept);
    }

    #[tokio::test]
    async fn a_sale_nobody_has_decided_about_cannot_be_decided_again() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 903,
            receipt_no: Some("T1-000103".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![],
            quarantine: Some(QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000103".to_owned(),
            }),
            stock: vec![],
            vat: vec![],
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();

        // Still waiting. A first answer is given in the queue, and letting this
        // route give it would be a way past the note the queue asks for.
        assert_eq!(
            repo.decide_again(TENANT, 903, "changed my mind about nothing", false, 0)
                .await
                .unwrap(),
            Decided::Unanswered
        );
        assert_eq!(repo.repair_queue(TENANT, 50).await.unwrap().len(), 1);
        assert!(repo.decided(TENANT, 50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_sale_that_stands_still_counts_after_it_is_looked_at() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 900,
            receipt_no: Some("T1-000100".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![],
            quarantine: Some(QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000100".to_owned(),
            }),
            stock: vec![(5_001, -2_000)],
            vat: vec![(750, 45_998, 3_452, 0)],
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();

        // The common answer: somebody checked, it is a real sale, the note says
        // what was checked. Nothing about the figures moves.
        assert!(
            repo.resolve_quarantine(TENANT, 900, "checked against the paper receipt", true)
                .await
                .unwrap()
        );
        let (from, to) = (1_788_500_000_000, 1_788_700_000_000);
        assert_eq!(repo.takings(TENANT, from, to).await.unwrap()[0].sales, 1);
        assert_eq!(
            repo.vat_summary(TENANT, from, to).await.unwrap().rows[0].vat_minor,
            3_452
        );
    }

    #[tokio::test]
    async fn a_restored_shop_keeps_what_was_decided() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 901,
            receipt_no: Some("T1-000101".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![],
            quarantine: Some(QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000100".to_owned(),
            }),
            stock: vec![],
            vat: vec![],
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();
        repo.resolve_quarantine(TENANT, 901, "rung twice after the restore", false)
            .await
            .unwrap();

        // Out of one shop and into another, which is what a restore is.
        let carried = repo
            .sales_after(TENANT, 0, 4_102_444_800_000, 10)
            .await
            .unwrap();
        assert_eq!(
            carried[0].resolution.as_ref().map(|(_, kept)| *kept),
            Some(false)
        );
        let fresh = MemoryRepo::new();
        fresh.put_sales(TENANT, &carried).await.unwrap();
        assert_eq!(
            fresh
                .takings(TENANT, 1_788_500_000_000, 1_788_700_000_000)
                .await
                .unwrap(),
            vec![],
            "a duplicate somebody struck out does not come back in a bundle"
        );
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
            vat: Vec::new(),
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
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
