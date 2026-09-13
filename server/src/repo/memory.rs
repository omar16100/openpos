//! The in-memory store, which is what makes the test suite fast enough to run
//! on every save, which is what makes anybody run it.
//!
//! Interior mutability, so it answers the same `&self` interface Postgres does:
//! a double that needed `&mut self` could not be handed to a handler, and the
//! tests would be testing something else.
//!
//! One file, because a trait implementation is one block. This is the largest
//! file in the crate and it cannot be split without splitting the contract it
//! honours, which would be worse.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use openpos_core::protocol::{ItemWire, QuarantineReason};

use crate::auth::{Caller, Role, Token, TokenHash};

use super::*;

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

/// Split a printed receipt number into what the till prints and the number it
/// counts, which is everything after the last dash.
fn split_receipt(receipt: &str) -> Option<(String, u64)> {
    let (prefix, digits) = receipt.rsplit_once('-')?;
    digits
        .parse()
        .ok()
        .map(|number| (prefix.to_owned(), number))
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
    /// The build this device last said it was running. None until it says.
    app_build: Option<String>,
    /// Which counter this is in its shop, 1 upward. What a receipt number is
    /// prefixed with, and handed out in order so two of them cannot share one.
    counter_no: u32,
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
        // The shop's next counter number, taken before the entry is made so the
        // closure below can have it. Spent on a re-enrolment that keeps the
        // number it already had, which costs a gap and never a clash.
        let next_counter = u32::try_from(
            inner
                .terminals
                .keys()
                .filter(|(owner, _)| *owner == tenant)
                .count()
                .saturating_add(1),
        )
        .unwrap_or(u32::MAX);
        let record = inner
            .terminals
            .entry((tenant, terminal))
            .or_insert_with(|| TerminalState {
                label: String::new(),
                enrolled_at_ms: now_ms(),
                last_seen_ms: None,
                app_build: None,
                counter_no: next_counter,
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
                // Every language this build has, which is what a shop that has
                // never said means.
                languages: Vec::new(),
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
        if operator.name.trim().is_empty() || operator.pin_rounds < openpos_core::auth::LEAST_PIN_ROUNDS {
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
        if rounds < openpos_core::auth::LEAST_PIN_ROUNDS || salt.is_empty() || key.is_empty() {
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
    ) -> Result<(Vec<SupplierEntry>, i64)> {
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

        // The whole account, under the same lock as the lines: this store holds
        // one, so reading both here is the same guarantee Postgres gets from one
        // transaction. A statement whose lines do not add up to the figure under
        // them is the thing both are avoiding.
        let mut owed_minor = 0_i64;
        for receipt in inner
            .deliveries
            .iter()
            .filter(|((owner, _), receipt)| {
                *owner == tenant && receipt.supplier_id == Some(supplier_id)
            })
            .map(|(_, receipt)| receipt)
        {
            for line in &receipt.lines {
                let amount = openpos_core::money::Minor::new(line.unit_cost_minor)
                    .mul_qty(openpos_core::money::Milli::new(line.qty_milli))
                    .map_or(0, |amount| amount.get());
                owed_minor = owed_minor.saturating_add(amount);
            }
        }
        for payment in inner
            .supplier_payments
            .iter()
            .filter(|((owner, _), payment)| *owner == tenant && payment.supplier_id == supplier_id)
            .map(|(_, payment)| payment)
        {
            owed_minor = owed_minor.saturating_sub(payment.amount_minor);
        }
        Ok((found, owed_minor))
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

    async fn write_customer_from_a_till(
        &self,
        tenant: u128,
        customer: &CustomerRecord,
    ) -> Result<()> {
        let mut inner = self.lock();
        let mut writing = customer.clone();
        if let Some(held) = inner.customers.get(&(tenant, customer.id)) {
            // The owner's two decisions about this person stay the owner's.
            writing.limit_minor = held.limit_minor;
            writing.active = held.active;
        }
        inner.customers.insert((tenant, customer.id), writing);
        bump_settings(&mut inner, tenant);
        Ok(())
    }

    async fn catalogue_holds(&self, tenant: u128, item: u128) -> Result<bool> {
        let inner = self.lock();
        // Any change naming it, including the one that withdrew it: an item the
        // shop decided about is not one a till may send back.
        Ok(inner
            .changes
            .get(&tenant)
            .into_iter()
            .flatten()
            .any(|(_, change)| match change {
                CatalogueChange::Upsert(held) => held.id == item,
                CatalogueChange::Delete(id) => *id == item,
            }))
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

    async fn struck_out_takings(
        &self,
        tenant: u128,
        terminal: u128,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Option<i64>> {
        let inner = self.lock();
        // The same window and the same sum as the takings above, over the
        // sales that one leaves out.
        Ok(Some(
            inner
                .sales
                .iter()
                .filter(|((owner, _), _)| *owner == tenant)
                .filter(|((_, id), _)| inner.struck_out.contains(&(tenant, *id)))
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
                // The receipt the debt was rung on, when the sale behind it is
                // here and carries one. A payment has no sale behind it.
                receipt_no: inner
                    .sales
                    .get(&(tenant, row.source_id))
                    .and_then(|sale| sale.receipt_no.clone())
                    .unwrap_or_default(),
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
        // And any code that would hand this device a fresh credential. An owner
        // cutting a tablet off is cutting the tablet off, and a code issued for
        // it an hour ago is a way back in.
        inner
            .codes
            .retain(|_, (grants, _)| !(grants.tenant == caller.tenant && grants.terminal == caller.terminal));
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
        let mut inner = self.lock();
        // A code that collides with one already alive is not a code to hand
        // out: it would redeem to somebody else's terminal. Refused here the
        // way the Postgres store refuses it, so a test can meet it at all.
        if inner
            .codes
            .get(code)
            .is_some_and(|(_, expires)| *expires > SystemTime::now())
        {
            return Err(RepoError::Invalid);
        }
        inner.codes.insert(code.clone(), (grants, expires));
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
        let Some(counter_no) = inner
            .terminals
            .get(&(tenant, terminal))
            .map(|record| record.counter_no)
        else {
            return Err(RepoError::UnknownTerminal);
        };
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
            counter_no,
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
                    counter_no: state.counter_no,
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
                payload_schema: sale.payload_schema,
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
        // The first number this shop has not used, for any terminal arriving
        // without one.
        let mut next_free = inner
            .terminals
            .iter()
            .filter(|((owner, _), _)| *owner == tenant)
            .map(|(_, state)| state.counter_no)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
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
                    app_build: None,
                    counter_no: 0,
                });
            state.label = record.label.clone();
            // The number it had, or the next one this shop has not used. A
            // bundle written before counters were numbered carries none, and a
            // restore that renumbered them would change what a till's receipts
            // are prefixed with while the numbers already printed keep the old
            // prefix.
            state.counter_no = if record.counter_no > 0 {
                record.counter_no
            } else if state.counter_no > 0 {
                state.counter_no
            } else {
                next_free
            };
            next_free = next_free.max(state.counter_no).saturating_add(1);
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
                    payload_schema: record.payload_schema,
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
                    build: state.app_build.clone(),
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
                    counter_no: state.counter_no,
                }
            })
            .collect();

        // A stable order, so the list does not shuffle between two loads of the
        // same page and make an operator doubt what they read.
        found.sort_by_key(|health| health.terminal);
        Ok(found)
    }

    async fn mark_terminal_seen(
        &self,
        tenant: u128,
        terminal: u128,
        build: Option<&str>,
    ) -> Result<()> {
        let mut inner = self.lock();
        let now = now_ms();
        if let Some(state) = inner.terminals.get_mut(&(tenant, terminal)) {
            state.last_seen_ms = Some(now);
            // Kept when the device did not say, for the reason the trait gives.
            if let Some(build) = build {
                state.app_build = Some(build.to_owned());
            }
        }
        Ok(())
    }

    async fn upsert_item(&self, tenant: u128, item: &ItemWire) -> Result<u64> {
        Ok(MemoryRepo::upsert_item(self, tenant, item.clone()))
    }

    async fn delete_item(&self, tenant: u128, item_id: u128) -> Result<u64> {
        Ok(MemoryRepo::delete_item(self, tenant, item_id))
    }

    async fn resend_catalogue(&self, tenant: u128) -> Result<u64> {
        // The latest change naming each item, in item order, appended again.
        // The same rule the real store follows: one row per item, whatever its
        // current state is, and the row is copied rather than rebuilt.
        let latest: BTreeMap<u128, CatalogueChange> = {
            let inner = self.lock();
            let mut newest: BTreeMap<u128, (u64, CatalogueChange)> = BTreeMap::new();
            for (seq, change) in inner.changes.get(&tenant).into_iter().flatten() {
                let id = match change {
                    CatalogueChange::Upsert(item) => item.id,
                    CatalogueChange::Delete(id) => *id,
                };
                let keep = newest.get(&id).is_none_or(|(held, _)| *seq > *held);
                if keep {
                    newest.insert(id, (*seq, change.clone()));
                }
            }
            newest
                .into_iter()
                .map(|(id, (_, change))| (id, change))
                .collect()
        };

        let mut sent = 0_u64;
        for change in latest.into_values() {
            self.append_change(tenant, change);
            sent = sent.saturating_add(1);
        }
        Ok(sent)
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
                // A sale as a shop stored one before the schema was kept.
                payload_schema: None,
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
                // A sale as a shop stored one before the schema was kept.
                payload_schema: None,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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

