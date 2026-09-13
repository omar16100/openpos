//! What a shop's own rows look like on the way in and out of a store.
//!
//! Plain records rather than the wire shapes: what travels between a till and a
//! shop is positional and frozen, and what a repository hands back is neither.
//! Keeping them apart is what lets a stored row gain a field without a protocol
//! version, and a protocol version without touching a table.

use std::time::Duration;

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
    /// Which schema those bytes were written under, as the till said.
    ///
    /// `None` for a sale stored before this was written down, which is every
    /// sale in every shop until this release. It matters because the bytes do
    /// not say: postcard is positional and has no tags, so a payload written
    /// under an older schema can parse under a newer one and be read as
    /// something else entirely. One in a real shop's backup did exactly that,
    /// and what changed was the tax: a sale that had declared 430.00 and 64.50
    /// came back declaring nothing, with the total still reading 494.50 so that
    /// nothing else noticed. The till has always sent this with the sale.
    pub payload_schema: Option<u16>,
    /// Who was signed in at the till when it was rung, as the till recorded it.
    ///
    /// `None` for a sale rung before a till recorded it, which is every sale in
    /// every shop before this release, and for one rung with nobody signed in.
    /// Both are true rather than missing: a sale is never refused for want of a
    /// sign-in, and inventing a name for a sale that has none is worse than
    /// saying nobody.
    ///
    /// Beside the sale as well as inside its bytes, so that asking who served a
    /// customer is a query rather than a decode of every payload in the shop.
    /// Not carried in a bundle: it is recomputed from the payload on the way in,
    /// like the tax rows, because a bundle that asserts it could assert somebody
    /// else.
    pub operator: Option<u128>,
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
    /// Which counter this is in its shop, 1 upward, and what the printed
    /// receipt number is prefixed with.
    ///
    /// The shop's own number rather than anything derived from the terminal's
    /// identifier. The prefix used to be the low sixteen bits of that
    /// identifier in hex, because it has to be short enough to read aloud over
    /// the phone, and two terminals sharing those bits printed the same
    /// numbers: the clash was caught when the second device synced, by which
    /// time a customer was holding the paper.
    pub counter_no: u32,
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
    /// The languages this shop offers its own staff, by the codes the screens
    /// use: `en`, `bn`. Empty means every language the device has, which is
    /// what every shop meant before a shop could say.
    ///
    /// A setting about the words this product chose, never about the words the
    /// shop chose: what a shop typed into its own catalogue is its own.
    pub languages: Vec<String>,
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
    /// Which counter this is in its shop, and so what its receipts are
    /// prefixed with. Zero in a bundle written before a shop numbered its own
    /// counters, and the shop gives it one on the way in.
    pub counter_no: u32,
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
    /// Which schema those bytes were written under, when the shop knows.
    ///
    /// Carried through a bundle so a restore reads them as they were written.
    /// Absent for a sale stored before the shop kept it, and the export works
    /// that out rather than leaving the reader to guess: see `export.rs`.
    pub payload_schema: Option<u16>,
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
    /// The receipt this debt was rung on. Empty for a payment or a write-off,
    /// which have no paper, and for a sale from before devices printed numbers.
    pub receipt_no: String,
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
pub(super) type Decision = (u64, String, bool);

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
    /// The build this device last said it was running. `None` until it says,
    /// which is any device not yet upgraded to a build that carries one, and
    /// any browser that refuses the service worker that knows it.
    pub build: Option<String>,
    /// Which counter this is in its shop, and the prefix on every receipt it
    /// prints. Zero for a device enrolled before the shop handed these out.
    pub counter_no: u32,
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

