//! The catalogue this device holds, and selling out of it.
//!
//! The hot path. A scan is a lookup in memory with no I/O and no allocation,
//! because it happens with a customer waiting and it happens all day.

use super::*;

impl<B: Backend> Till<B> {
    // -- catalogue -----------------------------------------------------------

    #[must_use]
    pub fn catalogue(&self) -> &Replica {
        &self.replica
    }

    /// Look up a scanned barcode. The hot path: no I/O, no allocation.
    #[must_use]
    pub fn lookup(&self, barcode: &str) -> Option<&Item> {
        self.replica.by_barcode(barcode)
    }

    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Item> {
        self.replica.search(query, limit)
    }

    // -- selling -------------------------------------------------------------

    #[must_use]
    pub fn cart(&self) -> &Cart {
        &self.cart
    }

    /// The catalogue this device holds.
    ///
    /// Read-only on purpose: a caller can look items up and search them, and
    /// changes still arrive the one way they always have, through a pull.
    #[must_use]
    pub fn replica(&self) -> &Replica {
        &self.replica
    }

    /// Scan a barcode straight onto the ticket.
    pub fn scan(&mut self, barcode: &str, qty: Milli) -> Result<usize> {
        let item = self
            .replica
            .by_barcode(barcode)
            .ok_or(TillError::UnknownBarcode)?
            .clone();
        self.ring(item, qty)
    }

    /// Put an item on the ticket by its id, for a cashier who looked it up
    /// rather than scanned it.
    ///
    /// A barcode that will not read, loose goods that carry none, a label torn
    /// off: the shop still has to sell the thing. Shares the rules below with
    /// scanning rather than repeating them, because a second way in that forgot
    /// one of them would be a way to sell what the shop has withdrawn.
    pub fn add(&mut self, id: crate::replica::ItemId, qty: Milli) -> Result<usize> {
        let item = self
            .replica
            .by_id(id)
            .ok_or(TillError::UnknownBarcode)?
            .clone();
        self.ring(item, qty)
    }

    pub(super) fn ring(&mut self, item: Item, qty: Milli) -> Result<usize> {
        // A discontinued item cannot be sold and must still be refundable: the
        // shop sold it last week and the customer is standing there with it.
        // Until this existed the flag was honoured by search and ignored by the
        // one lookup that takes money.
        if !item.active && !self.cart.is_refund() {
            return Err(TillError::NoLongerSold);
        }
        self.refuse_beyond_the_shelf(&item, self.wanted_of(item.id).saturating_add(qty.get()))?;
        Ok(self.cart.add_item(&item, qty)?)
    }

    /// Bring goods back at what the customer was charged for them.
    ///
    /// The item is looked up because the tax treatment, the unit and what the
    /// shop paid belong to the item; the money comes from the paper in the
    /// customer's hand. A refund rung by scanning the goods again prices them
    /// out of today's catalogue, which is the wrong money twice: a basket sold
    /// with something off it comes back at full price, and an item whose price
    /// has moved since comes back at the new one.
    ///
    /// An item the shop has stopped selling is still refundable, which the
    /// lookup below allows on purpose: it was sold last week and the customer
    /// is standing here with it.
    ///
    /// `came_off_the_line` and `was_on_the_line` are the whole line as the
    /// paper has it, not the part coming back: half of a discounted line
    /// returned brings half of what came off it, and the halving is done here.
    /// The till screen used to do that division itself, in a language where the
    /// only rounding to hand rounds halves towards the even number and negative
    /// halves the other way from positive ones, so a refund of half a line was
    /// a poisha the shop's own arithmetic did not agree with.
    pub fn return_line(
        &mut self,
        id: crate::replica::ItemId,
        qty: Milli,
        charged_each: Minor,
        came_off_the_line: Minor,
        was_on_the_line: Milli,
    ) -> Result<usize> {
        let item = self
            .replica
            .by_id(id)
            .ok_or(TillError::UnknownBarcode)?
            .clone();
        let came_off = came_off_the_line
            .share_of(qty, was_on_the_line)
            .map_err(|error| TillError::Cart(CartError::Money(error)))?;
        Ok(self.cart.return_line(&item, qty, charged_each, came_off)?)
    }

    pub fn set_qty(&mut self, line: usize, qty: Milli) -> Result<()> {
        // Typing ten where the shelf holds three is the same act as scanning it
        // ten times, and until this was here it was the way around the rule.
        if let Some(line) = self.cart.lines().get(line) {
            let item = self.replica.by_id(line.item_id).cloned();
            if let Some(item) = item {
                let others = self.wanted_of(item.id).saturating_sub(line.qty.get());
                self.refuse_beyond_the_shelf(&item, others.saturating_add(qty.get()))?;
            }
        }
        Ok(self.cart.set_qty(line, qty)?)
    }

    /// The same rule, against a basket that is coming back rather than one on
    /// the screen.
    ///
    /// Refunds are exempt here too: a parked refund is goods coming back, and
    /// the shelf has nothing to say about it.
    pub(super) fn refuse_a_parked_basket_past_the_shelf(&self, held: &wire::HeldTicketV1) -> Result<()> {
        if self.stock_rule != StockRule::Block || self.beyond_stock_allowed {
            return Ok(());
        }
        if !self.knows_the_shelf() {
            return Ok(());
        }
        for line in &held.lines {
            if line.qty_milli < 0 {
                continue;
            }
            let wanted = held
                .lines
                .iter()
                .filter(|other| other.item_id == line.item_id)
                .fold(0_i64, |sum, other| sum.saturating_add(other.qty_milli));
            let Some(item) = self.replica.by_id(Ulid::from_u128(line.item_id)) else {
                continue;
            };
            if wanted > item.on_hand.get() {
                return Err(TillError::MoreThanTheShelfHolds {
                    name: item.name_en.to_string(),
                    on_hand_milli: item.on_hand.get(),
                    wanted_milli: wanted,
                });
            }
        }
        Ok(())
    }

    /// How much of an item this basket already asks for, in thousandths.
    ///
    /// Summed across the lines, because one basket can hold the same item three
    /// times: scanned twice and then keyed once, or split to give one of them a
    /// discount.
    pub(super) fn wanted_of(&self, item: crate::replica::ItemId) -> i64 {
        self.cart
            .lines()
            .iter()
            .filter(|line| line.item_id == item)
            .fold(0_i64, |sum, line| sum.saturating_add(line.qty.get()))
    }

    /// Stop a basket that asks for more than the shop believes it has, when the
    /// shop has asked to be stopped.
    ///
    /// A refund is never stopped: goods coming back put stock in, and a customer
    /// standing at the counter with something they bought is not a stock
    /// question. Nor is a shop that has not set a rule, which is every shop
    /// until one says otherwise.
    pub(super) fn refuse_beyond_the_shelf(&self, item: &Item, wanted_milli: i64) -> Result<()> {
        if self.stock_rule != StockRule::Block || self.cart.is_refund() {
            return Ok(());
        }
        if !self.knows_the_shelf() {
            return Ok(());
        }
        // A supervisor already said yes for this basket. The ticket carries the
        // words, so what was allowed is on the customer's paper and in the
        // shop's copy.
        if self.beyond_stock_allowed {
            return Ok(());
        }
        if wanted_milli <= item.on_hand.get() {
            return Ok(());
        }
        Err(TillError::MoreThanTheShelfHolds {
            name: item.name_en.to_string(),
            on_hand_milli: item.on_hand.get(),
            wanted_milli,
        })
    }

    /// The lines this basket holds more of than the shop believes it has.
    ///
    /// For the shop that wants to be told rather than stopped, and for the one
    /// that stopped and then allowed it: either way the screen should still say
    /// which line the shelf disagrees about. Empty when the shop set no rule,
    /// because a figure nobody maintains is not worth a warning on every line.
    #[must_use]
    pub fn beyond_the_shelf(&self) -> Vec<ShortOfStock> {
        if self.stock_rule == StockRule::Off || self.cart.is_refund() {
            return Vec::new();
        }
        if !self.knows_the_shelf() {
            return Vec::new();
        }
        let mut short = Vec::new();
        for (index, line) in self.cart.lines().iter().enumerate() {
            let Some(item) = self.replica.by_id(line.item_id) else {
                continue;
            };
            let wanted = self.wanted_of(line.item_id);
            if wanted > item.on_hand.get() {
                short.push(ShortOfStock {
                    line: index,
                    name: line.name.to_string(),
                    on_hand_milli: item.on_hand.get(),
                    wanted_milli: wanted,
                });
            }
        }
        short
    }

    /// What this shop does about the shelf.
    #[must_use]
    pub fn stock_rule(&self) -> StockRule {
        self.stock_rule
    }

    /// Whether the figures this device holds are worth acting on.
    ///
    /// A till learns the shelf two hundred items at a time, and an item whose
    /// turn has not come holds whatever its catalogue row carried, which is
    /// usually nothing. Nothing and none look the same from here, so until the
    /// first lap is done the shelf says nothing: no warning, and above all no
    /// refusal. Anything else is a till refusing to sell what the shop has,
    /// which is what a new one did for its first hour.
    #[must_use]
    pub(super) fn knows_the_shelf(&self) -> bool {
        self.shelf_swept
    }

    /// Whether this device has been round the shelf once, for a screen to say
    /// so while a shop's rule is waiting on it.
    #[must_use]
    pub fn shelf_known(&self) -> bool {
        self.shelf_swept
    }

    /// Say that this device has been round the whole shelf.
    ///
    /// Called when the window of figures that just arrived was the last of a
    /// lap. Not written down: see the field. A reload costs one lap of silence
    /// about the shelf, and the screen says so while it lasts, which is the end
    /// of this a shop can see rather than the end where a till refuses sales on
    /// figures it does not have.
    pub fn shelf_swept(&mut self) {
        self.shelf_swept = true;
    }

    /// Take a line off the basket being rung.
    ///
    /// Free while nobody has paid anything: that is a mis-scan, the basket is
    /// not yet a record of anything, and a queue that needs a supervisor for
    /// every double scan is a till nobody uses.
    ///
    /// Once money has been entered against the basket it is `VoidLine`, which
    /// is the shape the permission was written for: goods rung up, cash taken
    /// for them, and the line taken off so the sale is smaller than what left
    /// the shop. A cashier without the permission is refused and the screen
    /// asks for a supervisor, by the same route as a price typed over the
    /// catalogue's. Either way the till writes down who took what off, because
    /// until now it left no trace at all.
    pub fn remove_line(&mut self, line: usize, now_ms: u64) -> Result<()> {
        if !self.cart.tenders().is_empty() {
            let outcome = self.auth.check(Action::VoidLine, now_ms);
            // Written down before the refusal goes back, as everywhere else: a
            // cashier who tried is the record a shop wants most.
            self.keep_what_was_allowed()?;
            if outcome.is_err() {
                // The auth book writes down wrong PINs and allowed actions; an
                // action somebody was simply not permitted is neither, and went
                // unrecorded. It is the one an owner would want to be told
                // about, so it is written here rather than left to the book.
                if let Some(who) = self.auth.signed_in().map(|who| who.id) {
                    self.write_down_allowed(now_ms, 11, 0, who, None, None);
                    self.persist_terminal_state()?;
                }
            }
            outcome?;
        }
        self.cart.remove_line(line)?;
        Ok(())
    }

    /// Sell one line at a different price.
    ///
    /// For damaged goods, a short weight, a price a customer was quoted. The
    /// cart has enforced the permission since it was written and the facade did
    /// not forward it, so a supervisor who may override a price had no way to.
    pub fn set_unit_price(&mut self, line: usize, price: Minor) -> Result<()> {
        Ok(self.cart.set_unit_price(line, price)?)
    }

    pub fn set_line_discount(&mut self, line: usize, discount: Discount) -> Result<()> {
        Ok(self.cart.set_line_discount(line, discount)?)
    }

    pub fn set_ticket_discount(&mut self, discount: Discount) -> Result<()> {
        Ok(self.cart.set_ticket_discount(discount)?)
    }

    /// Take back the money entered so far.
    ///
    /// For a mis-keyed amount: five thousand typed instead of five hundred
    /// cannot be unwound by entering more. The cart has been able to do this
    /// since it was written and the facade did not forward it, so nothing on any
    /// screen could reach it.
    pub fn clear_tenders(&mut self) {
        self.cart.clear_tenders();
    }

    /// Take money, or a promise of it.
    ///
    /// A credit tender naming somebody the shop has written down, on a basket
    /// that is not pointed at them, is refused. It reads as harmless and is
    /// not: what somebody owes is added up against their record, and a typed
    /// name is added up against itself, so the two live in different places.
    /// The shop then has a customer who owes for what they took and a phantom
    /// of the same name holding what they brought back, which is exactly what
    /// happened the first time a return was rung on account here.
    pub fn add_tender(&mut self, tender: Tender, now_ms: u64) -> Result<()> {
        if tender.kind == TenderKind::Credit
            && self.cart.customer().is_none()
            && let Some(named) = tender.reference.as_deref()
            && let Some(known) = self.customer_called(named)
        {
            return Err(TillError::WriteItAgainstThem { name: known });
        }
        // And refused here as well as at the close, because here is where the
        // cashier can still see what they typed. A promise or a card that goes
        // past the basket cannot be given back as change, and finding that out
        // only when the sale is closed means retyping the whole tender with a
        // customer waiting.
        self.cart.would_overpay(&tender)?;
        self.refuse_beyond_their_limit(&tender, now_ms)?;
        self.cart.add_tender(tender)?;
        Ok(())
    }

    /// Stop a sale on account going past what the shop said this person may
    /// owe.
    ///
    /// A shop that sells on account all day and never says stop is a shop whose
    /// cash is on somebody else's shelf. The cap is the shop's own, per person,
    /// and zero is what everybody has until somebody sets one.
    ///
    /// Measured against what the shop last told this device, which on a till
    /// that has not synced since morning is the morning's figure. That is the
    /// honest position for a device that has to keep selling with the internet
    /// down: it refuses on what it knows, says how old that is, and a
    /// supervisor standing there can allow it.
    pub(super) fn refuse_beyond_their_limit(&mut self, tender: &Tender, now_ms: u64) -> Result<()> {
        if tender.kind != TenderKind::Credit {
            return Ok(());
        }
        let Some(id) = self.cart.customer() else {
            return Ok(());
        };
        let Some(known) = self.customers.iter().find(|known| known.id == id.to_u128()) else {
            return Ok(());
        };
        if known.limit_minor <= 0 {
            return Ok(());
        }
        let (owed, as_of) = match self.owed_by(id) {
            Some((owed, at_ms)) => (owed.get(), at_ms),
            // Nothing the shop has said. A limit measured against a figure
            // nobody has sent is a limit measured against nothing, and refusing
            // on that would stop a new device selling to anybody on account.
            None => return Ok(()),
        };
        let wanted = owed.saturating_add(tender.amount.get());
        if wanted <= known.limit_minor {
            return Ok(());
        }
        let name = known.name.clone();
        let limit_minor = known.limit_minor;

        // A supervisor standing there may allow this one, like a basket past
        // the shelf. Written down either way, because the question afterwards
        // is never whether it was allowed but who allowed it.
        match self.auth.check(Action::BeyondTheirLimit, now_ms) {
            Ok(()) => {
                self.keep_what_was_allowed()?;
                Ok(())
            }
            Err(_) => {
                self.keep_what_was_allowed()?;
                Err(TillError::BeyondTheirLimit {
                    name,
                    owed_minor: owed,
                    owed_as_of_ms: as_of,
                    limit_minor,
                    wanted_minor: wanted,
                })
            }
        }
    }

    /// The name as the shop wrote it, when a typed one means somebody on the
    /// list. Folded the way the account book folds: case and spacing do not
    /// make two people.
    pub(super) fn customer_called(&self, typed: &str) -> Option<alloc::string::String> {
        let wanted = crate::accounts::account_key(typed);
        self.customers
            .iter()
            .filter(|known| known.active)
            .find(|known| crate::accounts::account_key(&known.name) == wanted)
            .map(|known| known.name.clone())
    }

    pub fn totals(&self) -> Result<TicketTotals> {
        Ok(self.cart.totals()?)
    }

    pub fn balance_due(&self) -> Result<Minor> {
        Ok(self.cart.balance_due()?)
    }

    pub fn change_due(&self) -> Result<Minor> {
        Ok(self.cart.change_due()?)
    }

    /// What still has to change hands, with its sign. See `Cart::outstanding`.
    pub fn outstanding(&self) -> Result<Minor> {
        Ok(self.cart.outstanding()?)
    }

    /// Whether the money covers the basket, by the rule the close uses.
    pub fn settled(&self) -> Result<bool> {
        Ok(self.cart.settled()?)
    }

    /// Abandon the sale in progress.
    pub fn cancel_sale(&mut self) {
        self.lower_limits_to_the_cashier();
        self.beyond_stock_allowed = false;
        self.cart = Cart::new(self.limits);
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

    use alloc::vec;

    use super::*;
    use crate::cart::TenderKind;
    
    
    use crate::storage::backend::{Fault, FaultyBackend, MemoryBackend};
    use crate::storage::wire::ItemV1;

    use super::super::proof::*;

    #[test]
    fn an_item_the_shop_stopped_selling_cannot_be_rung_and_can_still_be_refunded() {
        let (mut till, _) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        // A supervisor, because starting a refund needs the permission and this
        // test is about the item, not about who may refund.
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        let mut retired = item(1, 43_000);
        retired.active = false;
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&retired)],
            tombstones: vec![],
        })
        .unwrap();
        let barcode = alloc::string::String::from(&*retired.barcodes[0]);

        // The flag was honoured by search and ignored by the lookup that takes
        // money, so a discontinued item went on selling to anyone holding a box
        // of it.
        assert_eq!(
            till.scan(&barcode, Milli::ONE),
            Err(TillError::NoLongerSold)
        );

        // And refusing it outright would be worse: the shop sold this last week
        // and the customer is standing there with it.
        till.start_refund(Some("T1-000100"), 0).unwrap();
        assert!(till.scan(&barcode, Milli::ONE).is_ok());
    }

    #[test]
    fn rings_a_sale_from_scan_to_receipt() {
        let mut till = stocked_till(MemoryBackend::new());

        till.scan("8690000000001", Milli::ONE).unwrap();
        assert_eq!(till.totals().unwrap().total, Minor::new(49_450));
        pay_cash(&mut till, 50_000);

        let sale = till
            .checkout(Ulid::from_u128(900), 1_788_600_000_000)
            .unwrap();
        assert_eq!(sale.receipt_no.as_deref(), Some("T1-000100"));
        assert_eq!(sale.ticket.change, Minor::new(550));

        // Stock moved, cart cleared, sale queued for the server.
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(39_000))
        );
        assert!(till.cart().is_empty());
        assert_eq!(till.status().unwrap().unsynced_sales, 1);
    }

    #[test]
    fn an_unknown_barcode_is_reported_not_guessed() {
        let mut till = stocked_till(MemoryBackend::new());
        assert_eq!(
            till.scan("0000000000000", Milli::ONE),
            Err(TillError::UnknownBarcode)
        );
    }

    #[test]
    fn a_failed_commit_leaves_the_basket_and_the_numbers_untouched() {
        // Write operations, in order: the pull appends and flushes, the lease
        // grant appends and flushes, then the sale appends and flushes. Break
        // that last flush, which is the worst case: the bytes are written but
        // never made durable, so a naive implementation would leave a phantom
        // sale behind and burn a receipt number for it.
        let backend = FaultyBackend::new().with_fault(5, Fault::Fail);
        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
            tombstones: vec![],
        })
        .unwrap();
        till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 599))
            .unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let numbers_before = till.leases().remaining();

        let result = till.checkout(Ulid::from_u128(900), 0);
        assert!(
            result.is_err(),
            "a sale that cannot be made durable must not succeed"
        );

        // The cashier can retry: nothing was consumed and nothing was lost.
        assert_eq!(
            till.cart().lines().len(),
            1,
            "the basket survives a failed commit"
        );
        assert_eq!(
            till.leases().remaining(),
            numbers_before,
            "no number was burned"
        );
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(40_000)),
            "stock does not move for a sale that did not happen"
        );
        assert_eq!(
            till.pending_sales(10).unwrap().len(),
            0,
            "a sale that failed to commit must never reach the server"
        );
    }

    /// The same promise across a power cut, which is the case that matters.
    ///
    /// A failed commit gives the number back in memory, and a device that dies
    /// has no memory to give anything back from. What the till offers after it
    /// is switched on again is decided entirely by what survived on disk, so the
    /// question is whether a number taken for a sale that never committed comes
    /// back with it.
    ///
    /// It matters more than the arithmetic suggests. A tax invoice in this
    /// country is required to carry an unbroken sequence, so a number that is
    /// simply skipped is a question the shop has to answer for, and the shop
    /// cannot answer it: nothing anywhere would record that the number was ever
    /// taken.
    #[test]
    fn a_number_taken_by_a_sale_that_never_committed_comes_back_after_a_power_cut() {
        // The same fifth write as the test above, cut rather than failed: the
        // bytes never reach the disk and the device stops existing.
        let backend = FaultyBackend::new().with_fault(5, Fault::PowerCut);
        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
            tombstones: vec![],
        })
        .unwrap();
        till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 599))
            .unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        assert!(
            till.checkout(Ulid::from_u128(900), 0).is_err(),
            "the device died before the sale was durable"
        );

        // Switched on again the next morning, from whatever reached the disk.
        let (again, _) = Till::open(
            FaultyBackend::from_durable(till.journal().backend().durable()),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();

        assert_eq!(
            again.leases().remaining(),
            500,
            "the whole block is still there: no sale used any of it"
        );
        assert_eq!(
            again.pending_sales(10).unwrap().len(),
            0,
            "and no sale came back either, which is the other half"
        );
    }

    #[test]
    fn keeps_selling_when_the_numbers_run_out() {
        let (mut till, _) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&item(1, 43_000))],
            tombstones: vec![],
        })
        .unwrap();
        // A block with exactly one number in it.
        till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 100))
            .unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let first = till.checkout(Ulid::from_u128(901), 0).unwrap();
        assert_eq!(first.receipt_no.as_deref(), Some("T1-000100"));

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let second = till.checkout(Ulid::from_u128(902), 0).unwrap();

        assert!(second.receipt_no.is_none(), "the sale still completes");
        assert_eq!(till.status().unwrap().unnumbered_sales, 1);
        assert_eq!(till.status().unwrap().unsynced_sales, 2, "both sales sync");
    }

    #[test]
    fn parks_a_basket_and_brings_it_back_unchanged() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        let parked_total = till.totals().unwrap().total;

        till.hold(
            Ulid::from_u128(500),
            1_788_600_000_000,
            "Rahim, gone for cash",
        )
        .unwrap();
        assert!(
            till.cart().is_empty(),
            "the counter is free for the next customer"
        );

        let waiting = till.held_tickets().unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].lines, 1);
        assert_eq!(waiting[0].total, parked_total);
        assert_eq!(&waiting[0].label, "Rahim, gone for cash");

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(till.totals().unwrap().total, parked_total);
        assert!(
            till.held_tickets().unwrap().is_empty(),
            "and it is no longer parked"
        );
    }

    #[test]
    fn a_parked_basket_survives_the_outbox_draining() {
        // Same hazard as the receipt numbers: a full acknowledgement empties the
        // critical log, and a customer who stepped out for cash has not stopped
        // existing because the shop got its internet back.
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();

            till.scan("8690000000001", Milli::ONE).unwrap();
            till.hold(Ulid::from_u128(500), 0, "gone for cash").unwrap();
            till.acknowledge(&[sale.ticket.id]).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(till.held_tickets().unwrap().len(), 1);
    }

    #[test]
    fn a_parked_basket_survives_the_tablet_dying() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            till.hold(Ulid::from_u128(500), 1_788_600_000_000, "Karim")
                .unwrap();
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let waiting = till.held_tickets().unwrap();
        assert_eq!(waiting.len(), 1, "the customer is still standing there");

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(till.cart().lines().len(), 1);
    }

    #[test]
    fn a_resumed_basket_keeps_the_price_it_was_parked_at() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        let quoted = till.totals().unwrap().total;
        till.hold(Ulid::from_u128(500), 0, "waiting").unwrap();

        // A price rise arrives from the server while the basket is parked.
        let mut repriced = item(1, 99_000);
        repriced.barcodes = vec!["8690000000001".into()];
        till.apply_pull(&ItemDeltasV1 {
            cursor: 2,
            upserts: vec![ItemV1::from_domain(&repriced)],
            tombstones: vec![],
        })
        .unwrap();

        till.resume(Ulid::from_u128(500)).unwrap();
        assert_eq!(
            till.totals().unwrap().total,
            quoted,
            "the customer pays what they were quoted before they walked off"
        );
    }

    #[test]
    fn refuses_to_resume_over_a_basket_in_progress() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.hold(Ulid::from_u128(500), 0, "first").unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // Two baskets on one screen is how a customer pays for somebody else's
        // shopping.
        assert_eq!(
            till.resume(Ulid::from_u128(500)),
            Err(TillError::TicketInProgress)
        );
    }

    #[test]
    fn refuses_to_park_nothing_and_to_resume_what_is_not_there() {
        let mut till = stocked_till(MemoryBackend::new());
        assert_eq!(
            till.hold(Ulid::from_u128(500), 0, ""),
            Err(TillError::NothingToHold)
        );
        assert_eq!(
            till.resume(Ulid::from_u128(999)),
            Err(TillError::NoSuchHeldTicket)
        );
    }

    #[test]
    fn a_refund_carries_the_receipt_it_reverses_to_the_server() {
        let mut till = stocked_till(MemoryBackend::new());
        till.start_refund(Some("T1-000100"), 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.add_tender(
            Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(-49_450),
                reference: None,
            },
            0,
        )
        .unwrap();
        till.checkout(Ulid::from_u128(902), 0).unwrap();

        let pending = till.pending_sales(10).unwrap();
        let stored = crate::storage::wire::decode_sale(
            crate::storage::wire::SALE_SCHEMA,
            &pending[0].payload,
        )
        .unwrap();

        assert_eq!(stored.refund_of.as_deref(), Some("T1-000100"));
        // Stock moves the other way, which is what the server will post.
        assert_eq!(stored.stock, alloc::vec![(1_u128, 1_000_i64)]);
    }

    #[test]
    fn two_lines_of_one_item_move_the_stock_once_for_the_full_amount() {
        let mut till = stocked_till(MemoryBackend::new());
        // The cart deliberately opens a second line when the first is
        // discounted, so one item on two lines is the normal case, not an edge.
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.set_line_discount(0, Discount::Rate(crate::money::Bp::new(1_000).unwrap()))
            .unwrap();
        till.scan("8690000000001", Milli::new(2_000)).unwrap();
        assert_eq!(till.cart().lines().len(), 2, "two lines, one item");

        pay_cash(&mut till, 200_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
        let stored = wire::sale_commit(&sale.ticket, None, None);

        // The server keys a movement on the sale and the item, so a second entry
        // for the same pair is discarded and the ledger undercounts for good.
        assert_eq!(
            stored.stock,
            alloc::vec![(1_u128, -3_000_i64)],
            "one entry per item, carrying every line's quantity"
        );
    }

    #[test]
    fn a_basket_can_only_be_for_somebody_the_shop_wrote_down() {
        let mut till = stocked_till(MemoryBackend::new());
        till.set_customers(alloc::vec![
            wire::CustomerV1 {
                id: 21,
                name: "Karim, flat 3".into(),
                phone: Some("01711000000".into()),
                active: true,
                bin: None,
                limit_minor: 0,
                address: None,
            },
            wire::CustomerV1 {
                id: 22,
                name: "Rina".into(),
                phone: None,
                // Stopped: what she already owes is still owed, and nothing new
                // goes on the account.
                active: false,
                bin: None,
                limit_minor: 0,
                address: None,
            },
        ])
        .unwrap();

        assert!(till.set_customer(Some(Ulid::from_u128(21))).is_ok());
        assert_eq!(till.customer(), Some(Ulid::from_u128(21)));

        // A stopped account, and somebody this till has never heard of. Both
        // are a cashier about to write a debt nobody can chase.
        assert!(matches!(
            till.set_customer(Some(Ulid::from_u128(22))),
            Err(TillError::UnknownCustomer)
        ));
        assert!(matches!(
            till.set_customer(Some(Ulid::from_u128(99))),
            Err(TillError::UnknownCustomer)
        ));
        assert_eq!(till.customer(), Some(Ulid::from_u128(21)), "and unchanged");

        // Nobody is always allowed: a shop that has written nobody down still
        // sells on account against a name typed at the till.
        assert!(till.set_customer(None).is_ok());
        assert_eq!(till.customer(), None);
    }

    /// A shop that has not said anything sells whatever is asked for.
    ///
    /// The default, and it matters: a shop that has never counted holds zero of
    /// everything as far as this till knows, and a till that refused on that
    /// basis is a till that cannot sell.
    #[test]
    fn a_shop_that_set_no_rule_sells_past_the_shelf_without_a_word() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Off);
        till.scan("8690000000001", Milli::new(5_000)).unwrap();
        assert!(
            till.beyond_the_shelf().is_empty(),
            "and says nothing about it"
        );
    }

    /// A till that has not been round the shelf says nothing about the shelf.
    ///
    /// The figures arrive two hundred items at a time, five minutes apart, so a
    /// device holds a figure for the items whose turn has come and nothing for
    /// the rest. Nothing reads as none. Under the rule that stops a sale, a
    /// till enrolled this morning and put on the counter refuses everything
    /// scanned at it until its own figures catch up, which is its first hour,
    /// with a queue in front of it. Seen on a real till: it warned "the shop
    /// has 0" about an item the shop had sixty-one of, two minutes after enrolment.
    #[test]
    fn a_till_that_has_not_learned_the_shelf_sells_and_says_nothing() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        // As it was before the first lap closed.
        till.shelf_swept = false;

        till.scan("8690000000001", Milli::new(9_000))
            .expect("three times what the shelf says, and the shelf has not spoken");
        assert!(
            till.beyond_the_shelf().is_empty(),
            "and no warning either: the figure it would quote is one nobody sent"
        );
    }

    /// And the moment it has been round, the rule means something.
    #[test]
    fn once_it_has_been_round_the_shelf_the_rule_bites() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.shelf_swept = false;
        till.scan("8690000000001", Milli::new(9_000)).unwrap();
        till.cancel_sale();

        till.shelf_swept();
        let refusal = till.scan("8690000000001", Milli::new(9_000)).unwrap_err();
        assert!(
            matches!(refusal, TillError::MoreThanTheShelfHolds { .. }),
            "refused with {refusal:?}"
        );
    }

    /// Typing the quantity is the same act as scanning it again.
    #[test]
    fn a_quantity_typed_past_the_shelf_is_stopped_too() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.scan("8690000000001", Milli::ONE).unwrap();

        assert!(
            till.set_qty(0, Milli::new(9_000)).is_err(),
            "nine where the shop has three"
        );
        assert_eq!(
            till.cart().lines()[0].qty,
            Milli::ONE,
            "and the line is left alone"
        );
        till.set_qty(0, Milli::new(3_000))
            .expect("three is what it holds");
    }

    /// Goods coming back are never a stock question.
    #[test]
    fn a_refund_is_not_stopped_by_the_shelf() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.start_refund(None, 0).unwrap();
        till.scan("8690000000001", Milli::new(9_000))
            .expect("they are standing there with it");
        assert!(
            till.beyond_the_shelf().is_empty(),
            "and nothing is said about a shelf a return puts stock back on"
        );
    }

    /// A sale past somebody's cap says so on the paper as well as in the trail.
    ///
    /// The trail had it and the receipt did not, so the customer walked out
    /// with a copy that said nothing about the one thing that was unusual about
    /// the sale: that the shop let them past a cap it had set itself. Three of
    /// the four things a supervisor can allow on a ticket went onto the paper
    /// and this one did not.
    ///
    /// And it is written down once. It is checked through the auth book when
    /// the money goes on the ticket, which writes its own entry, so an entry
    /// here as well would have a shop counting one waiver as two.
    #[test]
    fn a_sale_past_a_cap_is_on_the_paper_and_in_the_trail_once() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();

        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        // Somebody the shop lets owe a hundred taka and no more, who already
        // owes ninety-five of it.
        till.set_customers(alloc::vec![crate::storage::wire::CustomerV1 {
            id: 21,
            name: alloc::string::String::from("the man in flat 3"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 10_000,
                address: None,
        }])
        .unwrap();
        till.set_balances(alloc::vec![(21, 9_500)], 1_000);
        till.scan("8690000000001", Milli::ONE).unwrap();
        till.set_customer(Some(Ulid::from_u128(21))).unwrap();

        let owed = till.totals().unwrap().total;
        let on_account = Tender {
            kind: TenderKind::Credit,
            amount: owed,
            reference: None,
        };
        till.add_tender(on_account.clone(), 1_000)
            .expect_err("that is past what the shop lets them owe");

        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::BeyondTheirLimit,
            1_000,
            60_000,
        )
        .expect("a supervisor standing there may allow this one");
        till.add_tender(on_account, 1_100)
            .expect("and then it goes on their account");

        let sold = till.checkout(Ulid::from_u128(902), 2_000).unwrap();
        assert!(
            sold.ticket
                .overrides
                .iter()
                .any(|note| note.contains("Owner") && note.contains("may owe")),
            "the customer's copy says the shop let them past their own cap: {:?}",
            sold.ticket.overrides
        );
        // The whole trail, not just a count of one kind: writing an entry here
        // as well as at the tender would either double this allowance or file
        // it under another number, and counting only twelves would miss the
        // second of those. Nine is the cashier taking the till.
        // Everything the trail holds about this basket, rather than a count of
        // one kind: writing an entry here as well as at the tender would either
        // double this allowance or file it under another number, and counting
        // only twelves would miss the second of those. Sign-ins are left out
        // because the fixture makes two of its own.
        let written: alloc::vec::Vec<u8> = till
            .unsent_allowed()
            .iter()
            .map(|one| one.action)
            .filter(|action| *action != 9)
            .collect();
        assert_eq!(
            written,
            alloc::vec![12],
            "one sale allowed past a cap, written down once and under its own number"
        );
    }

    /// A refund parked as a refund comes back as one.
    ///
    /// Nothing written down said which way round a parked basket was, so a
    /// refund came back as a sale with negative lines on it: money going the
    /// wrong way with nothing on the screen to say so. The screen has offered
    /// "park it" during a refund since refunds existed.
    #[test]
    fn a_refund_parked_is_a_refund_when_it_comes_back() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();

        till.start_refund(Some("T1-000104"), 0).unwrap();
        till.scan("8690000000001", Milli::ONE)
            .expect("they are standing there with it");
        assert!(till.cart().is_refund());
        let owed = till.totals().unwrap().total;

        till.hold(Ulid::from_u128(501), 1_000, "the returned rice")
            .unwrap();
        till.resume(Ulid::from_u128(501)).unwrap();

        assert!(
            till.cart().is_refund(),
            "it went on the list as a refund and it comes back as one"
        );
        assert_eq!(
            till.cart().refund_of(),
            Some("T1-000104"),
            "against the receipt it named, which is what a second refund is checked against"
        );
        assert_eq!(till.totals().unwrap().total, owed, "and for what it was");
    }

    /// A basket parked while the shelf agreed, coming back to a shelf that no
    /// longer does.
    ///
    /// Another till sold the last of it, or somebody wrote off a broken box.
    /// The check is at the moment a line is added, so without this the basket
    /// comes back whole and goes through the till without a word.
    #[test]
    fn a_parked_basket_survives_the_shop_deleting_what_is_in_it() {
        // Deleting an item is now something a back office can do, and what it
        // sends every till is a tombstone. A basket parked with that item in it
        // is a customer's shopping sitting on the counter: it has to come back
        // and be sellable, at the price it was parked at, or somebody rings the
        // lot again from memory.
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.scan("8690000000001", Milli::new(1_000)).unwrap();
        till.hold(Ulid::from_u128(501), 1_000, "Karim").unwrap();

        till.apply_pull(&ItemDeltasV1 {
            cursor: 2,
            upserts: vec![],
            tombstones: vec![Ulid::from_u128(1).to_u128()],
        })
        .unwrap();
        assert!(
            till.replica().by_barcode("8690000000001").is_none(),
            "the shop has taken it off this till"
        );

        let parked = till.held_tickets().unwrap();
        let id = parked[0].id;
        till.resume(id).expect("the basket comes back");
        let lines = till.cart().lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0].unit_price,
            Minor::new(43_000),
            "at what it was parked at: the ticket carries its own prices, and the \
             catalogue it was read from is gone"
        );
    }

    #[test]
    fn a_parked_basket_is_checked_against_the_shelf_when_it_comes_back() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.scan("8690000000001", Milli::new(3_000)).unwrap();
        till.hold(Ulid::from_u128(500), 1_000, "Karim").unwrap();

        // The shelf moves under it: two of the three are gone, which the shop
        // says in an answer about the shelf. A catalogue change would say
        // nothing about it, because a catalogue change is about names, prices
        // and tax and the shelf is a different question.
        till.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(1_000))]);

        let parked = till.held_tickets().unwrap();
        let id = parked[0].id;
        let refusal = till.resume(id).unwrap_err();
        assert!(
            matches!(refusal, TillError::MoreThanTheShelfHolds { .. }),
            "refused with {refusal:?}"
        );
        assert_eq!(
            till.held_tickets().unwrap().len(),
            1,
            "and it is still parked, not in nobody's hands"
        );

        // A supervisor says bring it back anyway.
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::SellBeyondStock,
            1_000,
            60_000,
        )
        .unwrap();
        till.resume(id).expect("the supervisor said so");
        assert_eq!(till.cart().lines().len(), 1);
    }

    /// What a till refuses to write down.
    #[test]
    fn an_item_with_no_name_or_no_barcode_is_refused() {
        let mut till = stocked_till(MemoryBackend::new());

        let mut nameless = item(9, 12_000);
        nameless.name_en = "  ".into();
        nameless.barcodes = vec!["8690000000099".into()];
        assert!(matches!(
            till.quick_add(nameless),
            Err(TillError::NamelessItem)
        ));

        let mut unfindable = item(9, 12_000);
        unfindable.name_en = "Biscuits".into();
        unfindable.barcodes = vec![];
        assert!(matches!(
            till.quick_add(unfindable),
            Err(TillError::NoBarcodeToFindItBy)
        ));
        assert!(till.unsent_items().is_empty(), "and neither is held");
    }

    /// The shop says it has them, and only then does the till let them go.
    #[test]
    fn items_the_shop_has_are_dropped_and_the_rest_are_kept() {
        let mut till = stocked_till(MemoryBackend::new());
        for (seed, barcode) in [(9_u128, "8690000000099"), (10, "8690000000100")] {
            let mut arrived = item(seed, 12_000);
            arrived.barcodes = vec![barcode.into()];
            till.quick_add(arrived).unwrap();
        }
        assert_eq!(till.unsent_items().len(), 2);

        // A reply that named one of them. The other is still owed, and a reply
        // that never arrived would leave both.
        till.items_accepted(&[Ulid::from_u128(9).to_u128()])
            .unwrap();
        assert_eq!(till.unsent_items().len(), 1);
        assert_eq!(till.unsent_items()[0].id, Ulid::from_u128(10).to_u128());

        let backend = till.journal().backend().clone();
        let (again, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(again.unsent_items().len(), 1, "across a restart");
    }

    /// A sale that closed with no receipt number left, taken by the shop while
    /// the drawer is still open.
    ///
    /// The count of sales waiting for a number is read from the log, and used to
    /// rely on an acknowledged sale being gone from it. Now that an open drawer
    /// holds the log down, the sale is still there and would be counted a second
    /// time: the till would ask for numbers it does not owe.
    #[test]
    fn a_numbered_sale_the_shop_took_is_not_still_waiting() {
        let backend;
        {
            let (mut till, sold) = a_till_whose_numbers_ran_out();
            till.acknowledge(&sold).unwrap();
            assert_eq!(
                till.status().unwrap().unnumbered_sales,
                0,
                "the screen says so before the restart, not only after it"
            );
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.status().unwrap().unnumbered_sales,
            0,
            "the shop has it, so numbering it is the shop's job"
        );
    }

    /// A log that has gone bad in the middle keeps every byte it has.
    ///
    /// What the outbox can see is what reads through: a frame that rots in a
    /// live log hides every sale behind it, and an outbox that cannot see them
    /// says there is nothing left to send. The log was then emptied, and the
    /// sales behind the bad frame were on this device and nowhere else.
    ///
    /// Keeping the bytes is what makes them recoverable: the next open reads
    /// the good prefix and puts the rest in the salvage file, where a person
    /// can be pointed at it. Raised by a review of the path that keeps sales
    /// safe.
    /// Nothing after the commit turns a sale into a failure.
    ///
    /// The commit's own note says it: durable, and only then may in-memory
    /// state move and a receipt print. What came after it could still return an
    /// error, and the sale was already on the disk and would sync. The cashier
    /// would be told the sale failed, ring the basket again, and the shop would
    /// have two of it with one customer.
    ///
    /// Only the drawer's arithmetic can fail there, at figures no shop reaches,
    /// and what it costs is this device's running drawer figure until the next
    /// boot, which rebuilds it from these same frames. The till says so rather
    /// than swallowing it, because an evening's count against a figure that is
    /// behind is an argument nobody can see the cause of.
    #[test]
    fn a_sale_that_is_already_durable_is_never_reported_as_failed() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(500), Minor::new(50_000), 0)
            .unwrap();
        // A drawer that cannot take another paisa without overflowing.
        till.shift
            .as_mut()
            .expect("the drawer is open")
            .set_cash_for_test(Minor::new(i64::MAX));

        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();

        assert_eq!(
            sale.ticket.totals.total,
            Minor::new(49_450),
            "the sale went through, because it was already on the disk"
        );
        assert!(
            till.status().unwrap().drawer_is_behind,
            "and the till says the drawer figure is behind the sales"
        );
    }

    #[test]
    fn acknowledged_sales_leave_the_outbox() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();

        assert_eq!(till.acknowledge(&[sale.ticket.id]).unwrap(), 1);
        assert_eq!(till.status().unwrap().unsynced_sales, 0);
    }
}
