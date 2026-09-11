//! What the server needs to remember, and an in-memory implementation.
//!
//! The trait exists so ingest can be tested without a database. Postgres is the
//! real implementation; the in-memory one keeps the test suite fast enough to
//! run on every save, which is what makes anybody actually run it.
//!
//! Every method takes a tenant. There is no way to ask this trait a question
//! that is not scoped to one shop, which is the first line of defence against
//! cross-tenant leakage; row-level security in Postgres is the second.

use std::future::Future;
use std::time::Duration;

use openpos_core::protocol::ItemWire;

use crate::auth::{Caller, Role, TokenHash};

mod memory;
mod records;

pub use memory::MemoryRepo;
pub use records::*;
// The row shape only a store itself names.
use records::Decision;

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

    /// Whether the shop's catalogue has ever named this item.
    ///
    /// For the one route a till writes items through. A till writes an item
    /// down when a delivery arrives during an outage carrying a barcode in
    /// nobody's catalogue, and that is the whole of its business with the
    /// catalogue: the roles exist because a shop with six tills had six devices
    /// that could reprice everything, and any one of them left on a counter was
    /// the whole shop. An item the shop already knows is the owner's to change.
    ///
    /// Ever, rather than now: an item the shop has withdrawn is one it decided
    /// about, and a till must not put it back on sale by sending its old copy.
    fn catalogue_holds(&self, tenant: u128, item: u128) -> impl Future<Output = Result<bool>> + Send;

    /// Write somebody down as a till does, which is less than the back office
    /// does.
    ///
    /// A till writes a person down so a sale on account can be rung during an
    /// outage, and it may correct a name or a phone number afterwards. What it
    /// may not touch is what the owner decided about that person: how much they
    /// may owe, and whether they may buy at all. A till sends no cap, so a
    /// plain save wrote a zero over one, and zero means no cap: any till in the
    /// shop could take an owner's credit limit off anybody.
    fn write_customer_from_a_till(
        &self,
        tenant: u128,
        customer: &CustomerRecord,
    ) -> impl Future<Output = Result<()>> + Send;

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

    /// How much of that window's cash belongs to sales the shop struck out.
    ///
    /// The figure above leaves them out, and the drawer's own figures keep
    /// them, deliberately: what a till expected and what a person counted are a
    /// record of one evening, and a duplicate that inflated the expectation is
    /// exactly what the shortfall that evening was. So the two disagree for
    /// good once a sale is struck out, and the disagreement is meant to be
    /// read. This is what it takes to read it. Without it the screen names one
    /// cause, a till that has not finished sending, and an owner whose till has
    /// finished sending is pointed at the person who counted the drawer.
    ///
    /// None on the same terms as the figure above: a sale from before the cash
    /// on one was recorded cannot be added up.
    fn struck_out_takings(
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
    /// A supplier's own statement: what came in and what was paid over a
    /// period, and what the whole account comes to.
    ///
    /// The total comes back with the lines rather than being asked for
    /// separately, because it is the number the two people are arguing about
    /// and it has to be the number those lines belong to. Read apart, a
    /// delivery landing between the two reads left a statement whose lines did
    /// not add up to what was printed under them, which is the one thing a
    /// document like this must never do.
    fn supplier_statement(
        &self,
        tenant: u128,
        supplier_id: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> impl Future<Output = Result<(Vec<SupplierEntry>, i64)>> + Send;

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

    /// Say every item again, so a till that is behind catches up.
    ///
    /// A till follows the catalogue by a cursor, and a row it could not read
    /// when it passed is a row it will never be offered again: the cursor moved
    /// on, which is the price of not stopping every till in the shop over one
    /// bad row. That happened for real, to seven rows, and the shop was left
    /// selling those items at whatever price each till already held with
    /// nothing on any screen to say which ones.
    ///
    /// This is the shop's way out, and it is the advice that screen already
    /// gives made into one press: each item's current state is written again,
    /// under new sequence numbers, so every till receives it on its next pull.
    /// The bytes are copied rather than rebuilt, so a row says exactly what it
    /// said before.
    ///
    /// Returns how many were sent again, which is what the shop is told.
    fn resend_catalogue(&self, tenant: u128) -> impl Future<Output = Result<u64>> + Send;

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
