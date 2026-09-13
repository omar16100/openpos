//! The drawer: opening one, the cash that crosses it, and counting it at the
//! end of the evening.
//!
//! A shift belongs to a terminal, never to a shop. Two offline terminals
//! closing one shop-wide shift is the write conflict an append-only ledger
//! cannot absorb: both counts are honest, both differ, and no rule picks a
//! winner afterwards.

use super::*;

impl<B: Backend> Till<B> {
    // -- the drawer -----------------------------------------------------------

    /// Open the drawer for the day with a counted float.
    pub fn open_shift(&mut self, id: ShiftId, opening_float: Minor, at_ms: u64) -> Result<()> {
        let shift = Shift::open(id, self.terminal, opening_float, at_ms)?;
        self.commit_shift_event(&ShiftEventV1::Opened {
            id: id.to_u128(),
            terminal: self.terminal.to_u128(),
            opening_float_minor: opening_float.get(),
            at_ms,
        })?;
        self.shift = Some(shift);
        Ok(())
    }

    #[must_use]
    pub fn shift(&self) -> Option<&Shift> {
        self.shift.as_ref()
    }

    /// Put cash in for a stated reason.
    pub fn cash_in(&mut self, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.move_cash(true, amount, reason, at_ms)
    }

    /// Take cash out for a stated reason.
    pub fn cash_out(&mut self, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.move_cash(false, amount, reason, at_ms)
    }

    /// Open the cash drawer without selling anything.
    ///
    /// A cashier gives change for something bought next door, or puts the float
    /// in at the start of a shift, and neither of those rings a sale. Without
    /// this the only way to open the drawer is to complete a sale or to reach
    /// under the counter and pull it, and a drawer that is easier to open by
    /// hand is a drawer that is left unlocked.
    ///
    /// Permission-gated on the same action a cash movement is, because it is
    /// the same act: the drawer coming open with nothing on the paper to say
    /// why. Written into the trail for the same reason, under the number that
    /// already means "the drawer opened", which every screen already words.
    ///
    /// Returns the bytes for a printer-driven drawer. Almost every drawer in a
    /// shop here is on the end of a cable in the printer's socket, so opening
    /// one is something the printer does.
    pub fn open_the_drawer(&mut self, at_ms: u64) -> Result<crate::receipt::escpos::Job> {
        let outcome = self.auth.check(Action::OpenDrawer, at_ms);
        // Written down before the refusal goes back, as everywhere else: a
        // cashier who tried to open the drawer is the record a shop wants most.
        self.keep_what_was_allowed()?;
        if outcome.is_err()
            && let Some(who) = self.auth.signed_in().map(|who| who.id)
        {
            // Thirteen, its own number. Eleven means a line taken off a basket
            // somebody had paid towards, and a trail that said that about a
            // cashier who tried to open the drawer would be accusing them of
            // something else entirely.
            self.write_down_allowed(at_ms, 13, 0, who, None, None);
            self.persist_terminal_state()?;
        }
        outcome?;
        Ok(crate::receipt::escpos::kick_the_drawer())
    }

    /// A receipt printed a second time, written down.
    ///
    /// Not permission-gated, and that is the decision rather than an omission.
    /// A customer who has lost their copy, or a printer that ate the paper, is
    /// the ordinary reason a receipt is printed again, and a till that needed a
    /// supervisor for it is a till a shop works around. What a reprint needs is
    /// a record, because a second copy of a receipt is a second piece of paper
    /// somebody can hand over: an expense claimed twice, a return made against
    /// a sale that was already returned.
    ///
    /// What a shop looks at is the shape rather than the single event. One
    /// reprint on a Tuesday is a customer who dropped their paper; six on a
    /// Thursday evening by one person is something else, and only a trail that
    /// holds them all can show the difference.
    ///
    /// Fourteen, its own number, because every other number in that trail
    /// already means something a shop would read differently.
    ///
    /// Which receipt it was is recorded beside it, when the screen knows: a
    /// trail that says only "somebody printed something again" leaves a shop
    /// lining times up against its own sales by hand, which is the work it
    /// keeps a trail to avoid. `None` where nothing was on the screen to name,
    /// and a device from before this was recorded says `None` too rather than
    /// having an answer filled in for it afterwards.
    pub fn reprinted(
        &mut self,
        at_ms: u64,
        receipt_no: Option<alloc::string::String>,
    ) -> Result<()> {
        let Some(who) = self.auth.signed_in().map(|who| who.id) else {
            // Nobody is signed in, so there is nobody to write down. A till in
            // that state has no receipt on its screen either.
            return Err(TillError::Auth(crate::auth::AuthError::UnknownOperator));
        };
        self.keep_what_was_allowed()?;
        self.write_down_allowed(at_ms, 14, 0, who, None, receipt_no);
        self.persist_terminal_state()
    }

    pub(super) fn move_cash(&mut self, inward: bool, amount: Minor, reason: &str, at_ms: u64) -> Result<()> {
        self.auth.check(Action::OpenDrawer, at_ms)?;
        self.keep_what_was_allowed()?;
        let shift = self.shift.as_mut().ok_or(TillError::NoOpenShift)?;

        // Applied to a copy first, so a movement the shift refuses is not
        // written to the log where the next boot would replay it.
        let mut next = shift.clone();
        if inward {
            next.cash_in(amount, reason, at_ms)?;
        } else {
            next.cash_out(amount, reason, at_ms)?;
        }

        self.commit_shift_event(&ShiftEventV1::CashMoved {
            inward,
            amount_minor: amount.get(),
            reason: alloc::string::String::from(reason),
            at_ms,
        })?;
        self.shift = Some(next);
        Ok(())
    }

    /// Totals so far, leaving the drawer open.
    pub fn x_report(&self) -> Result<XReport> {
        let shift = self.shift.as_ref().ok_or(TillError::NoOpenShift)?;
        Ok(shift.x_report()?)
    }

    /// Count the drawer and close the shift.
    pub fn close_shift(&mut self, counted_cash: Minor, at_ms: u64) -> Result<ZReport> {
        self.auth.check(Action::CloseShift, at_ms)?;
        self.keep_what_was_allowed()?;
        // Taken before anything else moves, so the record names whoever was
        // standing at the till when it was counted. A variance attached to a
        // terminal and a time is half of what an owner wants to know.
        let (counted_by, counted_by_name) = self
            .auth
            .signed_in()
            .map_or((0, alloc::string::String::new()), |who| {
                (who.id.to_u128(), who.name.to_string())
            });
        let shift = self.shift.as_mut().ok_or(TillError::NoOpenShift)?;

        let mut next = shift.clone();
        let report = next.close(counted_cash, at_ms)?;

        // The name goes in the frame as well as in the record below, because the
        // frame is durable first and the record is what a device rebuilds from
        // if it dies in between.
        self.commit_shift_event(&ShiftEventV1::Closed {
            counted_cash_minor: counted_cash.get(),
            at_ms,
            counted_by,
            counted_by_name: counted_by_name.clone(),
        })?;

        // Written down for sending before the caller is told it closed. The
        // count is the thing somebody who was not at the till reconciles, and a
        // device that reported "closed" and kept it to itself is the situation
        // this exists to end.
        self.unsent_shifts.push(wire::ClosedShiftV1 {
            id: report.totals.shift.to_u128(),
            closed_by: counted_by,
            closed_by_name: counted_by_name,
            opened_at_ms: report.totals.opened_at_ms,
            closed_at_ms: report.closed_at_ms,
            opening_float_minor: report.totals.opening_float.get(),
            sales: u32::try_from(report.totals.sales).unwrap_or(u32::MAX),
            cash_sales_minor: report.totals.cash_sales.get(),
            non_cash_sales_minor: report.totals.non_cash_sales.get(),
            cash_in_minor: report.totals.cash_in.get(),
            cash_out_minor: report.totals.cash_out.get(),
            expected_cash_minor: report.totals.expected_cash.get(),
            counted_cash_minor: report.counted_cash.get(),
            variance_minor: report.variance.get(),
        });
        self.persist_terminal_state()?;

        self.shift = Some(next);
        // The count is written down and sendable, so the day's frames are free
        // to go if the shop has already taken every sale in them. Here rather
        // than only on the next acknowledgement: a till that closes for the
        // night with nothing outstanding would otherwise carry the whole day
        // until tomorrow's first sale is confirmed.
        self.empty_the_log_if_nothing_needs_it()?;
        Ok(report)
    }

    /// Drawers counted and closed that the shop has not been told about.
    #[must_use]
    pub fn unsent_shifts(&self) -> &[wire::ClosedShiftV1] {
        &self.unsent_shifts
    }

    /// Forget the drawers the shop now holds.
    ///
    /// Called with what the server said it accepted, never with what was sent:
    /// a reply that did not arrive must leave the count here to be sent again.
    pub fn shifts_accepted(&mut self, accepted: &[u128]) -> Result<()> {
        let before = self.unsent_shifts.len();
        self.unsent_shifts
            .retain(|shift| !accepted.contains(&shift.id));
        if self.unsent_shifts.len() != before {
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    pub(super) fn commit_shift_event(&mut self, event: &ShiftEventV1) -> Result<()> {
        let bytes = wire::encode_shift_event(event)?;
        self.journal.commit(
            Store::Critical,
            PayloadKind::ShiftEvent,
            SHIFT_SCHEMA,
            &bytes,
        )?;
        Ok(())
    }

    /// Baskets currently set aside, newest first.
    pub fn held_tickets(&self) -> Result<Vec<HeldTicket>> {
        let mut listed = Vec::with_capacity(self.held.tickets.len());
        for held in &self.held.tickets {
            let lines = held
                .lines
                .iter()
                .cloned()
                .map(LineV1::into_domain)
                .collect::<core::result::Result<Vec<_>, WireError>>()?;
            let totals = ticket_totals(&TicketInput {
                lines: lines.iter().map(CartLine::as_input).collect(),
                ticket_discount: held.ticket_discount.clone().into_domain()?,
            })
            .map_err(|error| TillError::Cart(CartError::Money(error)))?;
            let total = totals.total;
            listed.push(HeldTicket {
                id: Ulid::from_u128(held.id),
                held_at_ms: held.held_at_ms,
                label: held.label.clone(),
                lines: held.lines.len(),
                total,
            });
        }
        listed.sort_by_key(|held| core::cmp::Reverse(held.held_at_ms));
        Ok(listed)
    }

    /// Set the basket aside so the next customer can be served.
    ///
    /// Persisted before the cart is cleared. A cashier who parks a basket and
    /// then loses power has not lost it, which is the whole reason to write it
    /// down rather than keep it in memory: the customer is still standing there.
    pub fn hold(&mut self, id: TicketId, held_at_ms: u64, label: &str) -> Result<()> {
        if self.cart.is_empty() {
            return Err(TillError::NothingToHold);
        }

        let mut next = self.held.clone();
        next.tickets.push(HeldTicketV1 {
            id: id.to_u128(),
            held_at_ms,
            customer: self.cart.customer().map(Ulid::to_u128),
            label: label.into(),
            lines: self.cart.lines().iter().map(LineV1::from_domain).collect(),
            ticket_discount: DiscountV1::from_domain(self.cart.ticket_discount()),
            // What a supervisor allowed on this basket goes with it. Without
            // this the basket came back at the approved price with nothing
            // saying who approved it, so the customer's copy and the shop's
            // both lost the one line that explains it.
            overrides: self
                .cart
                .overrides()
                .iter()
                .map(|note| alloc::string::String::from(&**note))
                .collect(),
            // And which way round it is. A refund parked and resumed came back
            // as a sale with negative lines on it.
            refund: self.cart.is_refund(),
            refund_of: self.cart.refund_of().map(alloc::string::String::from),
        });

        self.persist_held(&next)?;
        self.held = next;
        self.beyond_stock_allowed = false;
        self.cart = Cart::new(self.limits);
        Ok(())
    }

    /// Bring a parked basket back to the screen.
    ///
    /// Refuses while something is already rung, rather than silently merging or
    /// discarding it. Two baskets on one screen is how a customer ends up paying
    /// for somebody else's shopping.
    pub fn resume(&mut self, id: TicketId) -> Result<()> {
        if !self.cart.is_empty() {
            return Err(TillError::TicketInProgress);
        }
        let position = self
            .held
            .tickets
            .iter()
            .position(|held| held.id == id.to_u128())
            .ok_or(TillError::NoSuchHeldTicket)?;

        let held = self
            .held
            .tickets
            .get(position)
            .ok_or(TillError::NoSuchHeldTicket)?
            .clone();

        // A basket parked while the shelf agreed can come back to a shelf that
        // no longer does: another till sold the last of it, or somebody wrote
        // off a broken box. Checked before it is taken off the parked list, so a
        // refusal leaves it where it was rather than in nobody's hands.
        self.refuse_a_parked_basket_past_the_shelf(&held)?;

        // Built before the parked list is touched. It used to be the other way
        // round: the ticket was taken off the list and persisted, and then the
        // discount was applied through the checked setter, which refuses
        // anything above the ceiling of whoever is at the till now. A basket a
        // supervisor approved at fifteen percent, parked, and resumed by a
        // cashier whose own ceiling is nothing, was removed from the list and
        // then refused, which left the customer's basket in nobody's hands.
        let mut cart = Cart::new(self.limits);
        cart.set_customer(held.customer.map(Ulid::from_u128));
        if held.refund {
            // Restored rather than started: starting one refuses once anything
            // is rung, which is right at a counter and wrong here, where the
            // lines about to go back on are the parked refund's own.
            cart.restore_refund(held.refund_of.as_deref());
        }
        for line in held.lines {
            cart.restore_line(LineV1::into_domain(line)?);
        }
        // Put back rather than re-applied, like the lines: this basket was
        // already priced and somebody already allowed what is on it. The person
        // who resumed it is not the person who can approve it again.
        cart.restore_ticket_discount(held.ticket_discount.into_domain()?);
        cart.restore_overrides(held.overrides.into_iter().map(Into::into).collect());

        // Only now is it taken off the parked list. A basket that is both on
        // screen and in the parked list can be rung twice.
        let mut next = self.held.clone();
        next.tickets.remove(position);
        self.persist_held(&next)?;
        self.held = next;

        self.beyond_stock_allowed = false;
        self.cart = cart;
        Ok(())
    }

    /// Throw away a parked basket the customer never came back for.
    pub fn discard_held(&mut self, id: TicketId) -> Result<()> {
        let position = self
            .held
            .tickets
            .iter()
            .position(|held| held.id == id.to_u128())
            .ok_or(TillError::NoSuchHeldTicket)?;

        let mut next = self.held.clone();
        next.tickets.remove(position);
        self.persist_held(&next)?;
        self.held = next;
        Ok(())
    }

    pub(super) fn persist_held(&mut self, held: &HeldTicketsV1) -> Result<()> {
        let previous = core::mem::replace(&mut self.held, held.clone());
        // Rolled back on failure so the in-memory list can never claim a basket
        // that was not written down.
        if let Err(error) = self.persist_terminal_state() {
            self.held = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Turn the ticket in progress into a refund.
    ///
    /// Scanning then works exactly as it does for a sale: the cashier passes the
    /// goods over the same scanner and the till negates the quantities. Stock
    /// goes back on the shelf when the refund commits, by the same path a sale
    /// takes it off.
    ///
    /// Gated on the refund permission, and refused for a cashier who has none
    /// unless a supervisor has authorised this one. Money leaving the drawer is
    /// the action the permission model exists for.
    pub fn start_refund(&mut self, original_receipt: Option<&str>, now_ms: u64) -> Result<()> {
        self.auth.check(Action::Refund, now_ms)?;
        self.keep_what_was_allowed()?;
        Ok(self.cart.start_refund(original_receipt)?)
    }

    /// Close the sale.
    ///
    /// The order is deliberate and is the reason this method exists:
    ///
    /// 1. close the cart into an immutable ticket
    /// 2. take a receipt number from the leased block
    /// 3. commit the whole thing durably, in one frame
    /// 4. only then move stock and clear the cart
    ///
    /// If step 3 fails, the receipt number is put back and the cart is left
    /// exactly as it was, so the cashier can retry without re-ringing the basket
    /// and without burning a number. Moving stock before the commit would leave
    /// the on-screen quantity wrong after a failed sale.
    pub fn checkout(&mut self, id: TicketId, rung_at_ms: u64) -> Result<CompletedSale> {
        let mut ticket = self.cart.close(id, self.terminal, rung_at_ms)?;

        // Who rang it, taken from the till rather than passed in. The same rule
        // the shop's own name follows on a receipt: a platform that can pass it
        // is a platform that can pass the wrong one, and the only true answer is
        // the one this till is holding at the moment the sale closes. Nobody
        // signed in is a real state of this till and is left as nobody.
        ticket.operator = self.auth.signed_in().map(|who| who.id);

        // Take a number, but keep the book as it was in case the commit fails.
        let book_before = self.leases.clone();
        let number = self.leases.consume();
        let epoch = number.as_ref().map(|issued| issued.epoch);
        let lease_next = self.leases.active().map(|lease| lease.next);
        ticket.receipt_no = number.as_ref().map(|issued| issued.text.as_str().into());

        let payload = wire::sale_commit(&ticket, epoch, lease_next);
        let bytes = wire::encode_sale(&payload)?;

        let sequence = match self.journal.commit(
            Store::Critical,
            PayloadKind::SaleCommit,
            SALE_SCHEMA,
            &bytes,
        ) {
            Ok(sequence) => sequence,
            Err(error) => {
                // Nothing happened. Give the number back and leave the basket be.
                self.leases = book_before;
                return Err(error.into());
            }
        };

        // Durable. Only now may in-memory state move, and a receipt be printed.
        for line in &ticket.lines {
            let sold = line.qty.get().saturating_neg();
            self.replica.adjust_on_hand(line.item_id, Milli::new(sold));
        }

        // Into the drawer, if one is open. A sale is never refused for want of
        // an open shift: a till that will not sell because nobody pressed the
        // right button in the morning is a till the shop works around.
        // Recovery replays the same sale frames in the same order, so the
        // in-memory figure and the one rebuilt after a reboot agree.
        if let Some(shift) = self.shift.as_mut().filter(|shift| shift.is_open())
            && shift.record_sale(&ticket.tenders, ticket.change).is_err()
        {
            // The sale is durable and the receipt is about to print, so nothing
            // after the commit may turn it into a failure: a cashier told the
            // sale failed rings the basket again, and the shop has two.
            //
            // Only arithmetic can fail here, and only at figures no shop
            // reaches, because the drawer is already open by the filter above.
            // What it would cost is the drawer's running figure on this device
            // until the next boot, and the boot rebuilds it by replaying these
            // same frames in the same order.
            self.drawer_is_behind = true;
        }

        self.beyond_stock_allowed = false;
        self.cart = Cart::new(self.limits);

        Ok(CompletedSale {
            receipt_no: number.map(|issued| issued.text),
            ticket,
            journal_sequence: sequence,
        })
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
    
    
    use crate::storage::backend::MemoryBackend;
    use crate::storage::wire::ItemV1;

    use super::super::proof::*;

    #[test]
    fn half_a_discounted_line_comes_back_with_half_of_what_came_off_it() {
        let (mut till, _) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        let sold = item(1, 43_000);
        let id = sold.id;
        till.apply_pull(&ItemDeltasV1 {
            cursor: 1,
            upserts: vec![ItemV1::from_domain(&sold)],
            tombstones: vec![],
        })
        .unwrap();

        // Four on the paper at 430.00 each, with 100.00 off the line. Two come
        // back, so 50.00 of the discount comes back with them.
        till.start_refund(Some("T1-000100"), 0).unwrap();
        till.return_line(
            id,
            Milli::new(2_000),
            Minor::new(43_000),
            Minor::new(10_000),
            Milli::new(4_000),
        )
        .unwrap();
        let line = &till.cart().lines()[0];
        assert_eq!(
            line.discount,
            crate::domain::pricing::Discount::Amount(Minor::new(5_000)),
            "half the line back is half the discount back, and the halving is \
             the core's: the screen used to do this division itself"
        );

        // The whole line at once and the whole line in two halves come to the
        // same money, which is what the split has to be for.
        let whole = {
            let (mut other, _) = Till::open(
                MemoryBackend::new(),
                TENANT,
                terminal(),
                1,
                CartLimits::unrestricted(),
            )
            .unwrap();
            other.put_operator(supervisor_operator()).unwrap();
            other.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
            other
                .apply_pull(&ItemDeltasV1 {
                    cursor: 1,
                    upserts: vec![ItemV1::from_domain(&sold)],
                    tombstones: vec![],
                })
                .unwrap();
            other.start_refund(Some("T1-000100"), 0).unwrap();
            other
                .return_line(
                    id,
                    Milli::new(4_000),
                    Minor::new(43_000),
                    Minor::new(10_000),
                    Milli::new(4_000),
                )
                .unwrap();
            other.totals().unwrap().total
        };
        till.return_line(
            id,
            Milli::new(2_000),
            Minor::new(43_000),
            Minor::new(10_000),
            Milli::new(4_000),
        )
        .unwrap();
        assert_eq!(till.totals().unwrap().total, whole);
    }

    #[test]
    fn a_cashier_signs_in_with_the_internet_down_the_next_morning() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            let mut cashier = supervisor_operator();
            cashier.id = Ulid::from_u128(71);
            cashier.name = "Karim".into();
            cashier.pin =
                crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
            cashier.permissions = crate::auth::Permissions::cashier();
            till.put_operator(cashier).unwrap();
            backend = till.journal().backend().clone();
        }

        // Cold start, no network. The whole reason credentials sit on the
        // device.
        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(till.sign_in(Ulid::from_u128(71), "4321", 0).is_err());
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();
        assert_eq!(till.signed_in().map(|who| &*who.name), Some("Karim"));
    }

    #[test]
    fn a_cashier_cannot_refund_without_a_supervisor_standing_there() {
        let mut till = stocked_till(MemoryBackend::new());
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        assert!(
            till.start_refund(None, 0).is_err(),
            "money leaving the drawer is what the permission model is for"
        );

        till.authorise(Ulid::from_u128(70), "9999", Action::Refund, 0, 90_000)
            .unwrap();
        till.start_refund(None, 1_000).unwrap();

        // And the supervisor's name is on it afterwards.
        assert_eq!(
            till.audit().last().map(|entry| entry.authorised_by),
            Some(Some(Ulid::from_u128(70)))
        );
    }

    /// Cash is cash: a cap is on what somebody owes, not on what they pay.
    #[test]
    fn a_cap_on_an_account_does_not_stop_somebody_paying_cash() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        till.set_customers(alloc::vec![crate::storage::wire::CustomerV1 {
            id: 21,
            name: alloc::string::String::from("Karim, flat 3"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 1,
        }])
        .unwrap();
        till.set_balances(alloc::vec![(21, 40_000)], 1_000);
        till.set_customer(Some(Ulid::from_u128(21))).unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        let total = till.totals().unwrap().total;
        till.add_tender(
            Tender {
                kind: TenderKind::Cash,
                amount: total,
                reference: None,
            },
            2_000,
        )
        .expect("money in the hand is not credit");
    }

    /// The drawer opens for somebody permitted, and the trail says so either
    /// way.
    ///
    /// A cashier opens the drawer to give change for something bought next
    /// door, or to put the float in at the start of a shift, and neither rings
    /// a sale. Before this the only ways were to finish a sale or to reach
    /// under the counter and pull it, and a drawer that is easier to open by
    /// hand is a drawer that is left unlocked.
    #[test]
    fn the_drawer_opens_for_somebody_permitted_and_the_trail_says_who() {
        let mut till = stocked_till(MemoryBackend::new());

        // Somebody who may not. The refusal is the record a shop wants most:
        // a drawer coming open with nothing on the paper to say why is the
        // shape of every till theft there is.
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        cashier.permissions.may_open_drawer = false;
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        let refused = till.open_the_drawer(1_000).unwrap_err();
        assert!(
            matches!(
                refused,
                TillError::Auth(crate::auth::AuthError::NotPermitted {
                    action: Action::OpenDrawer
                })
            ),
            "refused with {refused:?}"
        );
        assert!(
            till.unsent_allowed()
                .iter()
                .any(|one| one.action == 13 && one.operator_name == "Karim"),
            "somebody who tried to open the drawer and could not is written down, under its own \
             number: eleven means a line taken off a paid basket, and saying that about this \
             would be accusing them of something else"
        );

        // And somebody who may. The answer is a job for the printer, because
        // almost every drawer in a shop here is on the end of a cable in the
        // printer's socket.
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 2_000).unwrap();
        let job = till.open_the_drawer(3_000).expect("a supervisor may");
        assert_eq!(
            job.bytes,
            alloc::vec![0x1B, 0x70, 0x00, 0x19, 0x32],
            "the pulse, and nothing printed"
        );
        assert!(
            till.unsent_allowed().iter().any(|one| one.action == 5),
            "and the drawer opening is in the trail under the number that already means it"
        );
    }

    #[test]
    fn a_cashiers_ceiling_follows_the_person_not_the_screen() {
        let mut till = stocked_till(MemoryBackend::new());
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        // Before this, the limits were whatever the caller passed to open().
        assert!(
            till.set_line_discount(0, Discount::Rate(crate::money::Bp::new(500).unwrap()))
                .is_err()
        );
        assert!(
            !till.signed_in().unwrap().permissions.may_override_price,
            "and the price override went with the ceiling"
        );
    }

    #[test]
    fn the_drawer_adds_up_and_survives_a_reboot() {
        let mut backend = MemoryBackend::new();
        let expected;
        {
            let mut till = stocked_till(backend.clone());
            till.open_shift(Ulid::from_u128(80), Minor::new(200_000), 0)
                .unwrap();

            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 49_450);
            till.checkout(Ulid::from_u128(900), 0).unwrap();

            till.cash_out(Minor::new(50_000), "drop to the safe", 1_000)
                .unwrap();

            expected = till.shift().unwrap().expected_cash().unwrap();
            // 2,000 float plus 494.50 taken less 500 dropped.
            assert_eq!(expected, Minor::new(199_450));
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.shift().and_then(|shift| shift.expected_cash().ok()),
            Some(expected),
            "the drawer figure is replayed from the same frames as the sales"
        );

        till.sign_in(Ulid::from_u128(70), "9999", 2_000).unwrap();
        let report = till.close_shift(Minor::new(199_000), 3_000).unwrap();
        assert_eq!(
            report.variance,
            Minor::new(-450),
            "a short drawer is a fact to report, not an error to refuse"
        );
    }

    #[test]
    fn a_counted_drawer_names_whoever_counted_it() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.open_shift(Ulid::from_u128(80), Minor::new(50_000), 0)
                .unwrap();
            till.sign_in(Ulid::from_u128(70), "9999", 1_000).unwrap();
            till.close_shift(Minor::new(45_000), 2_000).unwrap();
            backend = till.journal().backend().clone();
        }

        // A variance attached to a till and a time is half of what an owner
        // wants to know. The other half is standing at the counter.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let waiting = till.unsent_shifts();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].variance_minor, -5_000);
        assert_eq!(waiting[0].closed_by, Ulid::from_u128(70).to_u128());
        assert_eq!(
            waiting[0].closed_by_name, "Owner",
            "the name is written down at the time, not looked up later"
        );
    }

    #[test]
    fn a_cash_movement_the_shift_refuses_is_not_written_down() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::ZERO, 0)
            .unwrap();

        // Negative amounts are a caller mistake: direction is the operation.
        assert!(till.cash_in(Minor::new(-100), "typo", 0).is_err());

        let backend = till.journal().backend().clone();
        let (recovered, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            recovered.shift().map(|shift| shift.movements().len()),
            Some(0),
            "a refused movement in the log would be replayed as a real one"
        );
    }

    /// The drawer the cashier is standing at, after the shop has taken every
    /// sale in it.
    ///
    /// This emptied the log, and the open shift lives only in the log, so the
    /// float, the movements and the day's takings went with it. The cashier
    /// found out at the evening count, against a drawer that began at nothing.
    #[test]
    fn an_open_drawer_survives_the_shop_taking_every_sale() {
        let backend;
        {
            let mut till = stocked_till(MemoryBackend::new());
            till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
                .unwrap();
            till.cash_in(Minor::new(5_000), "change from the safe", 500)
                .unwrap();
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 49_450);
            let sold = till.checkout(Ulid::from_u128(900), 1_000).unwrap();

            till.acknowledge(&[sold.ticket.id]).unwrap();
            assert_eq!(till.status().unwrap().unsynced_sales, 0);
            backend = till.journal().backend().clone();
        }

        // The tablet's battery goes, mid-afternoon.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(
            till.shift().is_some_and(Shift::is_open),
            "the cashier would be asked to open a drawer that is already open"
        );
        let report = till.x_report().unwrap();
        assert_eq!(report.opening_float, Minor::new(30_000), "the float");
        assert_eq!(report.cash_in, Minor::new(5_000), "the movement");
        assert_eq!(report.cash_sales, Minor::new(49_450), "the takings");
        assert_eq!(
            report.expected_cash,
            Minor::new(84_450),
            "300 float, 50 in, 494.50 sold"
        );
    }

    /// A drawer that has been counted is not one that is open.
    ///
    /// The till keeps the drawer it counted until somebody opens the next one,
    /// because the Z report is read off it and the count has to survive until
    /// the shop has taken it. Saying "a drawer is open here" about that one put
    /// a counted drawer straight back onto the shop's list of drawers standing
    /// open: the shop deletes that row when the count arrives, and the next
    /// round of sync put it back. An owner at closing time reads that list to
    /// see which tills nobody has counted, so a counted one sitting in it is
    /// the one thing it must never say.
    #[test]
    fn a_counted_drawer_is_not_reported_as_one_standing_open() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
            .unwrap();
        assert!(
            till.situation(true, false).unwrap().drawer_open,
            "it is open until somebody counts it"
        );

        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        till.close_shift(Minor::new(30_000), 2_000).unwrap();
        assert!(
            till.shift().is_some(),
            "the counted drawer is still held, because the shop has not taken it"
        );
        assert!(
            !till.situation(true, false).unwrap().drawer_open,
            "and it is not a drawer standing open"
        );
    }

    /// And the log is emptied whether or not the drawer is counted, or a shop
    /// that never counts one keeps every sale the terminal ever made.
    #[test]
    fn a_counted_drawer_lets_the_log_go() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::ZERO, 0)
            .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 49_450);
        let sold = till.checkout(Ulid::from_u128(900), 1_000).unwrap();
        till.acknowledge(&[sold.ticket.id]).unwrap();

        assert_eq!(
            till.journal().read(Store::Critical).unwrap().len(),
            0,
            "the drawer is written down, so the frames under it may go"
        );
        assert!(
            till.shift().is_some_and(Shift::is_open),
            "and the drawer is still open, because nobody counted it"
        );

        till.close_shift(Minor::new(49_450), 2_000).unwrap();
        // Nothing new to acknowledge, so the drain is the same call with the
        // sale the shop already took.
        till.acknowledge(&[sold.ticket.id]).unwrap();
        assert_eq!(
            till.journal().read(Store::Critical).unwrap().len(),
            0,
            "a counted drawer is in the standing state, so the log may go"
        );
    }

    /// A shop that never counts its drawer used to keep every byte.
    ///
    /// The drawer lived in the critical log and nowhere else, so the log could
    /// not be dropped under an open one: about 145 KB of every thousand sales,
    /// on a tablet, until somebody pressed a button some shops never press. The
    /// drawer is written down at the moment the log goes, and the boot after it
    /// starts from what was written and replays what came after.
    #[test]
    fn a_shop_that_never_counts_its_drawer_still_lets_the_log_go() {
        let backend;
        {
            let mut till = stocked_till(MemoryBackend::new());
            till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
                .unwrap();
            till.cash_in(Minor::new(5_000), "change from the safe", 500)
                .unwrap();

            // A morning of selling, every sale acknowledged as it goes, and
            // nobody ever counts the drawer.
            for at in 0..5_u128 {
                till.scan("8690000000001", Milli::ONE).unwrap();
                pay_cash(&mut till, 49_450);
                let sold = till
                    .checkout(Ulid::from_u128(900 + at), 1_000 + at as u64)
                    .unwrap();
                till.acknowledge(&[sold.ticket.id]).unwrap();
            }

            assert_eq!(
                till.journal().read(Store::Critical).unwrap().len(),
                0,
                "nothing is waiting to be sent, so nothing holds the log down"
            );
            backend = till.journal().backend().clone();
        }

        // The tablet's battery goes with the drawer still open.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(
            till.shift().is_some_and(Shift::is_open),
            "the cashier would be asked to open a drawer that is already open"
        );
        let report = till.x_report().unwrap();
        assert_eq!(report.opening_float, Minor::new(30_000), "the float");
        assert_eq!(report.cash_in, Minor::new(5_000), "the movement it came with");
        assert_eq!(report.sales, 5, "five sales, none of them counted twice");
        assert_eq!(report.cash_sales, Minor::new(5 * 49_450), "the takings");
        assert_eq!(
            report.expected_cash,
            Minor::new(30_000 + 5_000 + 5 * 49_450),
            "300 float, 50 in, five at 494.50"
        );
    }

    /// The fold is written before the log is dropped, so a crash in between
    /// leaves both. The sequence on the fold is what stops the frames being
    /// counted a second time.
    #[test]
    fn a_drawer_folded_and_a_log_that_survived_is_not_counted_twice() {
        let backend;
        {
            let mut till = stocked_till(MemoryBackend::new());
            till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
                .unwrap();
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 49_450);
            let sold = till.checkout(Ulid::from_u128(900), 1_000).unwrap();
            // The outbox is drained and the drawer written down, and then the
            // power goes before the frames are dropped.
            Outbox::acknowledge(till.journal_mut(), &[sold.ticket.id]).unwrap();
            till.fold_the_open_drawer().unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let report = till.x_report().unwrap();
        assert_eq!(report.sales, 1, "one sale, folded and still in the log");
        assert_eq!(
            report.cash_sales,
            Minor::new(49_450),
            "counted once, not once for the fold and once for the frame"
        );
    }

    /// The tablet dies between the drawer being counted and the count being
    /// written down where it is sent from.
    ///
    /// The count is a frame in the log; the queue it is sent from is the
    /// standing state, written a moment later. In between there is a drawer that
    /// replays as counted and a shop that will never hear the figure, and no way
    /// back: the till refuses to count a drawer that is already closed. The
    /// cashier counted, the till agreed, and neither of them can prove it, which
    /// is the exact situation the record exists to end.
    #[test]
    fn a_drawer_counted_in_the_last_moment_before_a_crash_is_still_sent() {
        let backend;
        {
            let mut till = stocked_till(MemoryBackend::new());
            till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
                .unwrap();
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 49_450);
            till.checkout(Ulid::from_u128(900), 1_000).unwrap();

            // Exactly what close_shift makes durable first, and then the power
            // goes before anything else is written.
            till.commit_shift_event(&ShiftEventV1::Closed {
                counted_cash_minor: 79_000,
                at_ms: 2_000,
                counted_by: Ulid::from_u128(70).to_u128(),
                counted_by_name: "Karim".into(),
            })
            .unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(
            !till.shift().is_some_and(Shift::is_open),
            "the drawer was counted"
        );
        let held = till.unsent_shifts();
        assert_eq!(held.len(), 1, "the count is still owed to the shop");
        assert_eq!(held[0].counted_cash_minor, 79_000, "what was in the drawer");
        assert_eq!(
            held[0].expected_cash_minor, 79_450,
            "300 float and 494.50 sold"
        );
        assert_eq!(held[0].variance_minor, -450, "four fifty short");
        assert_eq!(held[0].closed_by_name, "Karim", "who counted it");
        assert_eq!(held[0].sales, 1);
    }

    /// An allowance does not outlive the person it was given to.
    ///
    /// A supervisor can be asked before anything is on the screen: the first
    /// scan is refused, they allow it, and the basket is still empty. Signing
    /// out at that moment left the next cashier holding the allowance.
    #[test]
    fn an_allowance_does_not_survive_the_shift_change_after_it() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.scan("8690000000001", Milli::new(9_000)).unwrap_err();
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::SellBeyondStock,
            1_000,
            60_000,
        )
        .unwrap();

        till.sign_out();
        till.sign_in(Ulid::from_u128(70), "9999", 2_000).unwrap();
        assert!(
            till.scan("8690000000001", Milli::new(9_000)).is_err(),
            "whoever is at the till now was allowed nothing"
        );
    }

    #[test]
    fn a_note_and_its_change_leave_the_drawer_holding_the_basket() {
        let mut till = stocked_till(MemoryBackend::new());
        till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 0)
            .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        // A five hundred note for a basket of 494.50.
        pay_cash(&mut till, 50_000);
        till.checkout(Ulid::from_u128(900), 1_000).unwrap();

        let standing = till.x_report().unwrap();
        assert_eq!(standing.cash_sales, Minor::new(49_450));
        assert_eq!(
            standing.expected_cash,
            Minor::new(30_000 + 49_450),
            "the float and the basket, not the float and the note"
        );

        // And the same after a reboot, because recovery replays the frames and
        // has to reach the figure the cashier watched all day.
        let backend = till.journal().backend().clone();
        let (recovered, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            recovered.x_report().unwrap().expected_cash,
            Minor::new(30_000 + 49_450)
        );
    }

    #[test]
    fn selling_is_never_refused_for_want_of_an_open_drawer() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);

        // A till that will not sell because nobody pressed the right button in
        // the morning is a till the shop works around.
        assert!(till.checkout(Ulid::from_u128(900), 0).is_ok());
        assert!(till.shift().is_none());
    }

    #[test]
    fn sales_still_waiting_for_a_number_are_still_counted_after_a_reboot() {
        let mut backend = MemoryBackend::new();
        {
            // No lease block, so the sale closes without a receipt number. It is
            // still a valid sale and the back office has to number it.
            let (mut till, _) = Till::open(
                backend.clone(),
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
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
            assert_eq!(sale.receipt_no, None);
            assert_eq!(till.status().unwrap().unnumbered_sales, 1);
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.status().unwrap().unnumbered_sales,
            1,
            "the count reset on every reboot, and nothing on the till said so"
        );
    }
}
