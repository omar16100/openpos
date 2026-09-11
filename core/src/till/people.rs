//! Who is at the till, and who the shop lets buy on account.
//!
//! Both are held on the device for the same reason: a cashier signs in and a
//! sale goes on somebody's account with the internet down, and a name typed
//! from memory is how one Karim ends up paying for another Karim's rice.

use super::*;

impl<B: Backend> Till<B> {
    // -- who is at the till -------------------------------------------------

    /// Replace everyone this till knows about, and write them down.
    ///
    /// The whole set rather than one at a time, because that is what the server
    /// sends: somebody removed from the shop has to disappear from the till,
    /// and a list that only ever grows would leave a departed cashier able to
    /// sign in forever.
    pub fn set_operators(&mut self, operators: Vec<Operator>) -> Result<()> {
        let previous = core::mem::replace(&mut self.auth, AuthBook::new());
        for operator in operators {
            self.auth.put(operator);
        }
        if let Err(error) = self.persist_terminal_state() {
            self.auth = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Add or replace an operator, and write them down.
    ///
    /// Persisted immediately rather than at the next sync, because the whole
    /// point of holding credentials on the device is that a cashier can sign in
    /// tomorrow morning with the internet still down.
    pub fn put_operator(&mut self, operator: Operator) -> Result<()> {
        // The nil id is what a record carries when it means nobody: a drawer
        // counted before the till wrote down who counted it says zero. A person
        // holding that id would read back as nobody having counted.
        if operator.id.to_u128() == 0 {
            return Err(TillError::NamelessOperator);
        }
        self.auth.put(operator);
        self.persist_terminal_state()
    }

    /// Sign in with a PIN. The cart's ceilings follow from who signed in, so a
    /// cashier cannot be given permissions by a UI that forgot to ask.
    pub fn sign_in(&mut self, id: crate::auth::OperatorId, pin: &str, now_ms: u64) -> Result<()> {
        let outcome = self.auth.sign_in(id, pin, now_ms);
        // Kept whether the PIN was right or wrong, and before the refusal is
        // handed back: a wrong PIN is the thing worth writing down here, and a
        // caller that returns early would drop it.
        self.keep_what_was_allowed()?;
        outcome?;
        if let Some(operator) = self.auth.signed_in() {
            self.limits = CartLimits {
                max_discount: crate::money::Bp::new(operator.permissions.max_discount_bp)
                    .unwrap_or(crate::money::Bp::ZERO),
                allow_price_override: operator.permissions.may_override_price,
            };
            // An empty cart takes the new limits at once. A cart with something
            // in it keeps its lines, because repricing a basket because
            // somebody changed shift is worse than either, and takes the new
            // ceilings, because the ceiling belongs to whoever is standing
            // there. It used to keep the ceilings it was rung under: a basket
            // a supervisor had allowed a discount on carried that permission
            // across the shift change to whoever signed in next.
            if self.cart.is_empty() {
                self.cart = Cart::new(self.limits);
            } else {
                self.cart.stand_under(self.limits);
            }
        }
        Ok(())
    }

    pub fn sign_out(&mut self) {
        self.auth.sign_out();
        self.limits = CartLimits::default();
        // With the ceilings, on the basket as well as on the till. A supervisor
        // allows a person one thing, and the person who takes over the till is
        // not that person: without this, an allowance granted against an empty
        // basket outlived the shift change that followed it, and one granted
        // against a basket with something in it outlived it twice over, because
        // the basket carried the raised ceiling to whoever signed in next.
        self.cart.stand_under(CartLimits::default());
        self.beyond_stock_allowed = false;
    }

    #[must_use]
    pub fn signed_in(&self) -> Option<&Operator> {
        self.auth.signed_in()
    }

    /// A supervisor puts their PIN in to allow the cashier one action.
    ///
    /// Two of these are not checked where the others are. A refund, a void, the
    /// drawer and the close all go through the auth book, which knows about
    /// authorisations. A discount and a price override are stopped by the
    /// cart's own ceilings, which were set when the cashier signed in and knew
    /// nothing about any of this: the supervisor typed their PIN, the till said
    /// yes, and the discount was refused again. So the ceiling is raised here
    /// too, for the basket on the screen.
    ///
    /// For that basket and no longer. The supervisor is standing at the counter
    /// now and will not be there for the next customer, so finishing or
    /// abandoning the sale puts the ceilings back where the cashier's own
    /// permissions leave them.
    pub fn authorise(
        &mut self,
        supervisor: crate::auth::OperatorId,
        pin: &str,
        action: Action,
        now_ms: u64,
        valid_for_ms: u64,
    ) -> Result<()> {
        let outcome = self
            .auth
            .authorise(supervisor, pin, action, now_ms, valid_for_ms);
        // As with signing in: a supervisor's wrong PIN is written down before
        // the refusal goes back to the caller.
        self.keep_what_was_allowed()?;
        outcome?;
        // The cart has carried this since it was written: it lifts its ceilings
        // for the rest of the ticket and writes the reason onto the ticket, so
        // the waiver is on the customer's paper and in the shop's copy. Nothing
        // called it, which is why a supervisor could type their PIN, be told
        // yes, and watch the discount refused again.
        // The four that change what this ticket is: a discount, a price typed
        // over the catalogue's, a basket past the shelf, and a sale to somebody
        // already past what they may owe. Each goes onto the ticket as words,
        // because what was waived belongs on the customer's paper and in the
        // shop's copy. The rest go through the auth book and are about the till
        // rather than about this basket.
        if matches!(
            action,
            Action::Discount { .. }
                | Action::OverridePrice
                | Action::SellBeyondStock
                | Action::BeyondTheirLimit
        ) {
            let who = self
                .auth
                .operators()
                .iter()
                .find(|operator| operator.id == supervisor)
                .map(|operator| operator.name.to_string())
                .unwrap_or_default();
            let reason = match action {
                Action::Discount { bp } => {
                    // As a rate, not as basis points. This line is printed on
                    // the customer's copy and read back off the shop's, and
                    // nobody standing at a counter reads basis points: the
                    // paper said "allowed a discount of 1500 basis points"
                    // where the shop asked for fifteen percent.
                    alloc::format!(
                        "{who} allowed a discount of {}",
                        crate::receipt::rate_of(bp)
                    )
                }
                Action::BeyondTheirLimit => {
                    // On the paper as well as in the trail. The customer takes
                    // this copy home and it is the record of a debt the shop
                    // let them past their own cap for: the trail says who
                    // allowed it and the paper is what either of them has in
                    // hand afterwards.
                    alloc::format!("{who} allowed a sale past what this customer may owe")
                }
                Action::SellBeyondStock => {
                    alloc::format!("{who} allowed more to be sold than the shop has")
                }
                _ => alloc::format!("{who} allowed a price to be typed over the catalogue's"),
            };
            // The shelf is the till's rule, not the cart's, so it is lifted
            // here. The words still go on the ticket by the same route as the
            // rest: what was waived belongs on the customer's paper and in the
            // shop's copy either way.
            if action == Action::SellBeyondStock {
                self.beyond_stock_allowed = true;
            }
            self.cart
                .authorise_override(&reason, crate::cart::CartLimits::allowing(action));
            // Written down here because the auth book never sees these used:
            // the cart's own ceilings stop a discount and a typed price, and
            // the shelf is the till's own rule, so the moment worth recording
            // is the supervisor allowing it. Without this, the only record of
            // who allowed a discount is prose on the ticket, and a ticket the
            // customer walked out with is not an accountability record.
            //
            // A sale past what somebody may owe is the exception: that one is
            // checked through the auth book when the money is put on the
            // ticket, so it writes its own entry then. Writing one here as well
            // would put the same allowance in the trail twice, and a shop
            // counting how often somebody's cap was waived would count double.

            if action == Action::BeyondTheirLimit {
                return self.persist_terminal_state();
            }
            let bp = match action {
                Action::Discount { bp } => bp,
                _ => 0,
            };
            let code = match action {
                Action::Discount { .. } => 1,
                Action::SellBeyondStock => 10,
                _ => 2,
            };
            let cashier = self.auth.signed_in().map_or(supervisor, |who| who.id);
            self.write_down_allowed(now_ms, code, bp, cashier, Some(supervisor), None);
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    /// Keep whatever the auth book has just written down, for the shop.
    ///
    /// The book records a privileged action as it allows it, which is the right
    /// place: a caller that forgets is an action with nobody's name on it. What
    /// it could not do is outlive the process, so this copies each entry into
    /// the standing state, where it waits with the counted drawers until the
    /// shop has it.
    pub(super) fn keep_what_was_allowed(&mut self) -> Result<()> {
        let fresh: Vec<crate::auth::AuditEntry> = self
            .auth
            .audit()
            .iter()
            .skip(self.taken_audit)
            .cloned()
            .collect();
        let refused: Vec<crate::auth::Refusal> = self
            .auth
            .refusals()
            .iter()
            .skip(self.taken_refusals)
            .copied()
            .collect();
        if fresh.is_empty() && refused.is_empty() {
            return Ok(());
        }
        self.taken_refusals = self.auth.refusals().len();
        for one in refused {
            // 7, 8 and 9 rather than variants of the permission enum: typing a
            // PIN, rightly or wrongly, is not an action anybody may be
            // permitted to take, and putting it in that enum would mean
            // writing a permission for it.
            let code = match (one.signed_in, one.locked_out) {
                (true, _) => 9,
                (false, true) => 8,
                (false, false) => 7,
            };
            self.write_down_allowed(one.at_ms, code, 0, one.operator, None, None);
        }
        self.taken_audit = self.auth.audit().len();
        for entry in fresh {
            let (code, bp) = match entry.action {
                Action::Discount { bp } => (1_u8, bp),
                Action::OverridePrice => (2, 0),
                Action::Refund => (3, 0),
                Action::VoidLine => (4, 0),
                Action::OpenDrawer => (5, 0),
                Action::CloseShift => (6, 0),
                // Ten, because seven, eight and nine are the refusals: a wrong
                // PIN, a locked-out person, and somebody signing in. Eleven is
                // a line taken off a paid basket by somebody who may not.
                Action::SellBeyondStock => (10, 0),
                Action::BeyondTheirLimit => (12, 0),
            };
            self.write_down_allowed(
                entry.at_ms,
                code,
                bp,
                entry.operator,
                entry.authorised_by,
                None,
            );
        }
        self.persist_terminal_state()
    }

    /// One line of the trail, with the names as they stand now.
    ///
    /// The names are copied rather than looked up later, for the reason the
    /// counted drawer's is: somebody since renamed, or gone from the shop, is
    /// still the person this belongs to.
    pub(super) fn write_down_allowed(
        &mut self,
        at_ms: u64,
        action: u8,
        bp: u32,
        operator: crate::auth::OperatorId,
        authorised_by: Option<crate::auth::OperatorId>,
        receipt_no: Option<alloc::string::String>,
    ) {
        let name_of = |id: crate::auth::OperatorId| {
            self.auth
                .operators()
                .iter()
                .find(|one| one.id == id)
                .map(|one| one.name.to_string())
                .unwrap_or_default()
        };
        let operator_name = name_of(operator);
        let authorised_by_name = authorised_by.map(name_of).unwrap_or_default();
        self.allowed_seq = self.allowed_seq.saturating_add(1);
        self.unsent_allowed.push(wire::AllowedV1 {
            seq: self.allowed_seq,
            at_ms,
            action,
            bp,
            operator: operator.to_u128(),
            operator_name,
            // Zero when nobody had to allow it: the cashier's own ceiling
            // covered it, which is a different fact from a supervisor standing
            // at the counter.
            authorised_by: authorised_by.map_or(0, crate::ids::Ulid::to_u128),
            authorised_by_name,
            // Only a reprint carries one. The question a shop asks about a
            // second piece of paper is which receipt it was of.
            receipt_no,
        });
    }

    /// Write down something the shop has never heard of, and sell it.
    ///
    /// A delivery arrives during an outage and its barcode is in nobody's
    /// catalogue. A till that could only say "no such item" would lose the sale,
    /// and the shop would sell it off the paper and reconcile nothing. So the
    /// cashier says what it is and what it costs, the till holds it like any
    /// other item, and it goes to the shop with the sales.
    ///
    /// The id is minted by the caller, like a ticket's: this crate has no
    /// entropy. What comes back from the shop later replaces this, which is why
    /// the id matters more than the name.
    ///
    /// # Errors
    /// When the item has no name to print on a receipt, or no barcode to find
    /// it by again, or the standing state cannot be written.
    pub fn quick_add(&mut self, item: Item) -> Result<()> {
        if item.name_en.trim().is_empty() {
            return Err(TillError::NamelessItem);
        }
        if item.barcodes.iter().all(|code| code.trim().is_empty()) {
            return Err(TillError::NoBarcodeToFindItBy);
        }
        let written = wire::ItemV1::from_domain(&item);
        // The obligation first. If the catalogue write fails after this, the
        // shop still gets the item and the till sells it after the next pull;
        // the other order loses the obligation and leaves the shop holding
        // sales that name something nobody can look up.
        let held = self.unsent_items.clone();
        self.unsent_items.retain(|one| one.id != written.id);
        self.unsent_items.push(written.clone());
        if let Err(error) = self.persist_terminal_state() {
            self.unsent_items = held;
            return Err(error);
        }

        // Then into the catalogue, by the same path a pull takes, so it is on
        // disk rather than only in memory: a tablet restarted before the shop
        // has it would otherwise sell it once and never again. The cursor is
        // zero because this came from nowhere: the till has learned nothing
        // about what the server holds.
        self.apply_pull(&ItemDeltasV1 {
            cursor: 0,
            upserts: alloc::vec![written],
            tombstones: Vec::new(),
        })?;
        Ok(())
    }

    /// Items this till wrote down and the shop has not got.
    #[must_use]
    pub fn unsent_items(&self) -> &[wire::ItemV1] {
        &self.unsent_items
    }

    /// Forget the items the shop now holds.
    ///
    /// Called with what the server said it stored, never with what was sent: a
    /// reply that did not arrive must leave them here to be sent again. The
    /// catalogue keeps them either way; what is dropped is the obligation to
    /// send them.
    pub fn items_accepted(&mut self, stored: &[u128]) -> Result<()> {
        let before = self.unsent_items.len();
        self.unsent_items.retain(|one| !stored.contains(&one.id));
        if self.unsent_items.len() != before {
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    /// What this device allowed and the shop has not been told about.
    #[must_use]
    pub fn unsent_allowed(&self) -> &[wire::AllowedV1] {
        &self.unsent_allowed
    }

    /// Forget the entries the shop now holds.
    ///
    /// Called with what the server said it stored, never with what was sent: a
    /// reply that did not arrive must leave the trail here to be sent again.
    pub fn allowed_accepted(&mut self, stored: &[u64]) -> Result<()> {
        let before = self.unsent_allowed.len();
        self.unsent_allowed.retain(|one| !stored.contains(&one.seq));
        if self.unsent_allowed.len() != before {
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    /// Put the ceilings back to what the cashier's own permissions allow.
    ///
    /// Called wherever a basket ends. An authorisation is for the sale in front
    /// of the supervisor, not for the rest of the shift.
    pub(super) fn lower_limits_to_the_cashier(&mut self) {
        self.limits = self
            .auth
            .signed_in()
            .map(|who| CartLimits {
                max_discount: crate::money::Bp::new(who.permissions.max_discount_bp)
                    .unwrap_or(crate::money::Bp::ZERO),
                allow_price_override: who.permissions.may_override_price,
            })
            .unwrap_or_default();
    }

    /// Everyone this till knows about.
    ///
    /// An empty list is a different problem from a wrong PIN, and a screen that
    /// cannot tell them apart sends a shopkeeper looking for a forgotten
    /// password when the truth is that nobody has been added yet.
    #[must_use]
    pub fn people(&self) -> &[Operator] {
        self.auth.operators()
    }

    /// Privileged actions taken on this terminal, and on whose authority.
    #[must_use]
    /// What this device has allowed since it started, as the auth book wrote it.
    ///
    /// The transient copy. What a shop reads is the durable one: each of these
    /// is written into the standing state as it happens and sent, so a device
    /// restarted overnight has still told the shop. This is here for a platform
    /// that wants to show the last few actions on the device itself, offline,
    /// without asking anybody.
    pub fn audit(&self) -> &[crate::auth::AuditEntry] {
        self.auth.audit()
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
    use crate::cart::TenderKind;
    
    use crate::money::Bp;
    use crate::storage::backend::MemoryBackend;
    

    use super::super::proof::*;

    #[test]
    fn a_refund_puts_stock_back_and_pays_the_customer() {
        let mut till = stocked_till(MemoryBackend::new());

        // Sell one first, so the shelf and the ledger have somewhere to return to.
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(39_000))
        );

        // The customer brings it back with the receipt.
        till.start_refund(sale.receipt_no.as_deref(), 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        assert_eq!(till.totals().unwrap().total, Minor::new(-49_450));

        till.add_tender(
            Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(-49_450),
                reference: None,
            },
            0,
        )
        .unwrap();
        let refund = till.checkout(Ulid::from_u128(901), 0).unwrap();

        assert_eq!(refund.ticket.totals.total, Minor::new(-49_450));
        assert_eq!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|i| i.on_hand),
            Some(Milli::new(40_000)),
            "the goods are back on the shelf"
        );
        assert_eq!(till.status().unwrap().unsynced_sales, 2);
    }

    /// A shop can say how much anybody may owe it, and a supervisor can allow
    /// one sale past it.
    #[test]
    fn a_sale_on_account_stops_at_what_the_shop_lets_them_owe() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();
        // A cashier, because a supervisor may go past a cap unaided: whoever
        // may allow things may do this one, like selling past the shelf.
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        // Somebody the shop lets owe five hundred, who already owes four.
        till.set_customers(alloc::vec![crate::storage::wire::CustomerV1 {
            id: 21,
            name: alloc::string::String::from("Karim, flat 3"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 50_000,
        }])
        .unwrap();
        till.set_balances(alloc::vec![(21, 40_000)], 1_000);
        till.set_customer(Some(Ulid::from_u128(21))).unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        let total = till.totals().unwrap().total;

        // Four hundred owed plus this basket is past five hundred.
        let refused = till
            .add_tender(
                Tender {
                    kind: TenderKind::Credit,
                    amount: total,
                    reference: None,
                },
                2_000,
            )
            .unwrap_err();
        assert!(
            matches!(
                refused,
                TillError::BeyondTheirLimit {
                    owed_minor: 40_000,
                    limit_minor: 50_000,
                    ..
                }
            ),
            "refused with {refused:?}"
        );

        // A supervisor standing there allows this one.
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::BeyondTheirLimit,
            2_000,
            60_000,
        )
        .unwrap();
        till.add_tender(
            Tender {
                kind: TenderKind::Credit,
                amount: total,
                reference: None,
            },
            3_000,
        )
        .expect("the supervisor said so");
        assert_eq!(till.cart().tenders().len(), 1);
    }

    /// A receipt printed again is written down, and needs nobody's permission.
    #[test]
    fn a_reprint_is_written_down_and_needs_nobodys_permission() {
        let mut till = stocked_till(MemoryBackend::new());

        // A plain cashier: sell, take cash, open the drawer to give change, and
        // nothing else. A customer who lost their copy is the ordinary reason
        // for a reprint, and a till that needed a supervisor for it is a till a
        // shop works around.
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        till.reprinted(1_000, Some("T1-000104".into()))
            .expect("anybody at the till may reprint");
        let written = till
            .unsent_allowed()
            .iter()
            .find(|one| one.action == 14)
            .expect(
                "a second copy of a receipt is a second piece of paper somebody can hand over, so \
                 the shop is told who printed it and when",
            )
            .clone();
        assert_eq!(written.operator_name, "Karim");
        assert_eq!(
            written.receipt_no.as_deref(),
            Some("T1-000104"),
            "and which receipt, or the shop is left lining times up against its own sales by hand"
        );

        // Six on a Thursday evening is the thing a shop looks at, so each one
        // is its own entry rather than a flag that is already set.
        till.reprinted(2_000, Some("T1-000104".into())).unwrap();
        till.reprinted(3_000, None).unwrap();
        assert_eq!(
            till.unsent_allowed()
                .iter()
                .filter(|one| one.action == 14)
                .count(),
            3
        );
        // The same receipt twice over, which is the pattern worth seeing: the
        // trail holds the number rather than a count of reprints, so a shop can
        // tell three customers who lost their paper from one receipt printed
        // three times.
        assert_eq!(
            till.unsent_allowed()
                .iter()
                .filter(|one| one.receipt_no.as_deref() == Some("T1-000104"))
                .count(),
            2
        );
        // And nothing invented where the screen had nothing to name.
        assert!(
            till.unsent_allowed()
                .iter()
                .any(|one| one.action == 14 && one.receipt_no.is_none())
        );

        // And with nobody at the till there is nobody to write down. A till in
        // that state has no receipt on its screen either.
        till.sign_out();
        assert!(till.reprinted(4_000, None).is_err());
    }

    /// A shop that has said nothing has said nothing.
    #[test]
    fn a_customer_with_no_cap_is_not_capped() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();
        till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
        till.set_customers(alloc::vec![crate::storage::wire::CustomerV1 {
            id: 21,
            name: alloc::string::String::from("Karim, flat 3"),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        }])
        .unwrap();
        till.set_balances(alloc::vec![(21, 4_000_000)], 1_000);
        till.set_customer(Some(Ulid::from_u128(21))).unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        let total = till.totals().unwrap().total;
        till.add_tender(
            Tender {
                kind: TenderKind::Credit,
                amount: total,
                reference: None,
            },
            2_000,
        )
        .expect("no cap is no cap, however much they owe");
    }

    #[test]
    fn what_a_supervisor_allows_is_on_the_paper_and_ends_with_the_sale() {
        let mut till = stocked_till(MemoryBackend::new());
        let cashier = Ulid::from_u128(11);
        till.set_operators(alloc::vec![
            Operator {
                id: cashier,
                name: "Rahima".into(),
                pin: crate::auth::PinHash::derive("4321", [3; 16], 1_000),
                permissions: crate::auth::Permissions {
                    max_discount_bp: 0,
                    may_override_price: false,
                    may_refund: false,
                    may_void_line: true,
                    may_authorise: false,
                    may_open_drawer: true,
                    may_close_shift: false,
                },
                active: true,
            },
            supervisor_operator(),
        ])
        .unwrap();
        till.sign_in(cashier, "4321", 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // Nothing, on this cashier's word.
        assert!(
            till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
                .is_err()
        );

        // The supervisor allows it, and it goes through.
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            crate::auth::Action::Discount { bp: 1_000 },
            1_000,
            60_000,
        )
        .unwrap();
        assert!(
            till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
                .is_ok()
        );

        // And it is on the paper. A waiver nobody can see afterwards is a
        // waiver nobody can ask about.
        pay_cash(&mut till, 100_000);
        let sold = till.checkout(Ulid::from_u128(900), 2_000).unwrap();
        assert_eq!(sold.ticket.overrides.len(), 1);
        assert!(sold.ticket.overrides[0].contains("Owner"));
        // As a rate. Basis points are how the till stores it and not how
        // anybody reads a receipt, and this line is on the customer's copy.
        assert!(
            sold.ticket.overrides[0].contains("10%"),
            "{}",
            sold.ticket.overrides[0]
        );

        // The supervisor has walked away. The next customer gets the cashier's
        // own ceiling back, which is nothing.
        till.scan("8690000000001", Milli::ONE).unwrap();
        assert!(
            till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
                .is_err(),
            "an authorisation is for the sale in front of them, not the shift"
        );
    }

    #[test]
    fn a_credit_tender_naming_a_written_down_customer_is_refused() {
        let mut till = stocked_till(MemoryBackend::new());
        till.set_customers(alloc::vec![wire::CustomerV1 {
            id: 21,
            name: "Karim, flat 3".into(),
            phone: None,
            active: true,
            bin: None,
            limit_minor: 0,
        }])
        .unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        // The cashier types the name instead of choosing the person. It reads
        // as harmless: it is how one person ends up with two accounts, one
        // holding what they took and one holding what they brought back.
        let refused = till.add_tender(
            Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(49_450),
                reference: Some("karim, FLAT 3".into()),
            },
            0,
        );
        assert!(matches!(
            refused,
            Err(TillError::WriteItAgainstThem { ref name }) if name == "Karim, flat 3"
        ));

        // Choosing them is the answer, and then the same tender is fine: what
        // is owed goes against the record rather than against a spelling.
        till.set_customer(Some(Ulid::from_u128(21))).unwrap();
        assert!(
            till.add_tender(
                Tender {
                    kind: TenderKind::Credit,
                    amount: Minor::new(49_450),
                    reference: Some("karim, FLAT 3".into()),
                },
                0
            )
            .is_ok()
        );
    }

    #[test]
    fn more_on_the_account_than_the_basket_is_refused_as_it_is_typed() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();

        // Six hundred on an account for a basket of 494.50. Refused here, with
        // the cashier still looking at what they typed, rather than at the
        // close with a customer waiting and the whole tender to enter again.
        let refused = till.add_tender(
            Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(60_000),
                reference: Some("the man from the tailor's".into()),
            },
            0,
        );
        assert!(matches!(
            refused,
            Err(TillError::Cart(CartError::ChangeFromAPromise { .. }))
        ));

        // A hundred taka note and the rest on the account is the ordinary case
        // and is untouched: the change comes out of the note.
        till.add_tender(
            Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(10_000),
                reference: None,
            },
            0,
        )
        .unwrap();
        till.add_tender(
            Tender {
                kind: TenderKind::Credit,
                amount: Minor::new(40_000),
                reference: Some("the man from the tailor's".into()),
            },
            0,
        )
        .unwrap();
        assert_eq!(till.change_due().unwrap(), Minor::new(550));
    }

    #[test]
    fn what_a_device_allowed_survives_the_process_that_allowed_it() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.open_shift(Ulid::from_u128(80), Minor::new(50_000), 1_000)
                .unwrap();
            // Cash out of the drawer outside a sale, which is the movement an
            // owner asks about when the count comes up short.
            till.cash_out(Minor::new(20_000), "paid the milk man", 2_000)
                .unwrap();
            backend = till.journal().backend().clone();
        }

        // Until now this lived in memory and died with the tab. The question
        // asked a week later is "who opened it", and the answer was gone.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        let kept = till.unsent_allowed();
        assert!(!kept.is_empty(), "the trail outlives the process");
        let opened = kept
            .iter()
            .find(|one| one.action == 5)
            .expect("the drawer opening");
        assert_eq!(opened.operator, Ulid::from_u128(70).to_u128());
        assert_eq!(opened.operator_name, "Owner");
        assert_eq!(opened.at_ms, 2_000);
        assert_eq!(
            opened.authorised_by, 0,
            "nobody had to allow it: their own permission covered it"
        );
    }

    #[test]
    fn a_discount_a_supervisor_allowed_is_written_down_with_both_names() {
        let mut till = stocked_till(MemoryBackend::new());
        // A cashier who may not discount at all, which is the ordinary case.
        let cashier = Operator {
            id: Ulid::from_u128(71),
            name: "Rahima".into(),
            pin: crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS),
            permissions: crate::auth::Permissions::default(),
            active: true,
        };
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 1_000).unwrap();

        till.authorise(
            Ulid::from_u128(70),
            "9999",
            crate::auth::Action::Discount { bp: 1_000 },
            2_000,
            90_000,
        )
        .unwrap();

        // The auth book never sees a discount used: the cart's own ceiling
        // stops it, so the moment worth recording is the supervisor allowing
        // it. Without this the only record is prose on a ticket the customer
        // walked out with.
        let waived: Vec<&wire::AllowedV1> = till
            .unsent_allowed()
            .iter()
            .filter(|one| one.action == 1)
            .collect();
        assert_eq!(waived.len(), 1);
        assert_eq!(waived[0].bp, 1_000);
        assert_eq!(waived[0].operator_name, "Rahima", "who did it");
        assert_eq!(waived[0].authorised_by_name, "Owner", "and who allowed it");
        // And both sign-ins are there, which is who was standing at the till.
        assert_eq!(
            till.unsent_allowed()
                .iter()
                .filter(|one| one.action == 9)
                .count(),
            2
        );
    }

    #[test]
    fn a_pin_typed_wrongly_is_written_down_and_so_is_the_lockout() {
        let mut till = stocked_till(MemoryBackend::new());

        // Somebody at the till after closing, trying the owner's PIN until it
        // locks. Five attempts is the shipped policy.
        for at_ms in [1_000_u64, 2_000, 3_000, 4_000, 5_000] {
            assert!(till.sign_in(Ulid::from_u128(70), "0000", at_ms).is_err());
        }

        // The sign-in that opened the till is in the list too, so the wrong
        // ones are counted apart from it.
        let wrong: Vec<&wire::AllowedV1> = till
            .unsent_allowed()
            .iter()
            .filter(|one| matches!(one.action, 7 | 8))
            .collect();
        assert_eq!(wrong.len(), 5, "every attempt, not only the last");
        assert!(
            wrong.iter().take(4).all(|one| one.action == 7),
            "a wrong PIN"
        );
        assert_eq!(
            wrong[4].action, 8,
            "and the one that used the last attempt says so"
        );
        assert_eq!(wrong[0].operator_name, "Owner", "whose button was pressed");
        assert_eq!(wrong[0].authorised_by, 0);

        // And the sixth is refused for being locked out rather than for the
        // PIN, which is the state the count is there to reach.
        assert!(matches!(
            till.sign_in(Ulid::from_u128(70), "9999", 6_000),
            Err(TillError::Auth(crate::auth::AuthError::LockedOut { .. }))
        ));
    }

    #[test]
    fn who_took_the_till_is_written_down_with_the_rest() {
        let mut till = stocked_till(MemoryBackend::new());
        let cashier = Operator {
            id: Ulid::from_u128(71),
            name: "Rahima".into(),
            pin: crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS),
            permissions: crate::auth::Permissions::cashier(),
            active: true,
        };
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 2_000).unwrap();

        // Who was standing at a till when something happened at it is half of
        // every question an owner asks about that evening, and it used to be
        // inferable only from what they sold.
        let took: Vec<&wire::AllowedV1> = till
            .unsent_allowed()
            .iter()
            .filter(|one| one.action == 9)
            .collect();
        assert_eq!(took.len(), 2, "the owner at open, then the cashier");
        assert_eq!(took[1].operator_name, "Rahima");
        assert_eq!(took[1].at_ms, 2_000);
        assert_eq!(
            took[1].authorised_by, 0,
            "nobody allows somebody to sign in: they type their own PIN"
        );
    }

    /// A shop that wants the till to stop is stopped, in words with the figures
    /// in them.
    #[test]
    fn a_shop_that_wants_stopping_is_stopped_and_told_the_figures() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.scan("8690000000001", Milli::new(3_000))
            .expect("three is what the shelf holds");

        let refusal = till.scan("8690000000001", Milli::ONE).unwrap_err();
        match refusal {
            TillError::MoreThanTheShelfHolds {
                ref name,
                on_hand_milli,
                wanted_milli,
            } => {
                assert_eq!(name, "Rice Miniket 5kg");
                assert_eq!(on_hand_milli, 3_000);
                assert_eq!(wanted_milli, 4_000);
            }
            other => panic!("refused with {other:?}"),
        }
        assert_eq!(
            alloc::format!("{refusal}"),
            "the shop has 3 Rice Miniket 5kg and this basket wants 4"
        );
        assert_eq!(till.cart().lines().len(), 1, "and the basket is as it was");
    }

    /// A supervisor allows it, for this basket and no longer.
    #[test]
    fn a_supervisor_allows_one_basket_past_the_shelf() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Block);
        till.scan("8690000000001", Milli::new(4_000)).unwrap_err();

        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::SellBeyondStock,
            1_000,
            60_000,
        )
        .unwrap();
        till.scan("8690000000001", Milli::new(4_000))
            .expect("the supervisor said so");
        // Still said out loud: allowing it does not make the shelf agree.
        assert_eq!(till.beyond_the_shelf().len(), 1);

        let allowed = till.unsent_allowed().last().expect("written down");
        assert_eq!(allowed.action, 10, "sold past the shelf");
        assert_eq!(allowed.authorised_by_name, "Owner", "on whose authority",);

        pay_cash(&mut till, 200_000);
        let sale = till.checkout(Ulid::from_u128(900), 2_000).unwrap();
        assert!(
            sale.ticket
                .overrides
                .iter()
                .any(|reason| reason.contains("allowed more to be sold than the shop has")),
            "and it is on the customer's paper: {:?}",
            sale.ticket.overrides
        );

        // The next customer starts again.
        assert!(
            till.scan("8690000000001", Milli::new(4_000)).is_err(),
            "allowing one basket is not allowing the day"
        );
    }

    /// What a supervisor allowed is what the basket may take, and no more.
    ///
    /// The allowance lifted the basket's ceiling to everything. So a supervisor
    /// approving fifteen percent left a cashier able to give ninety on the same
    /// ticket without asking anybody, while the trail said "allowed a discount
    /// of 1500 basis points" and the customer walked out with the rest. The one
    /// record a shop has of what was waived described something that did not
    /// happen, which is worse than having no record: it is a record that clears
    /// somebody.
    ///
    /// Found by review rather than by a test, because every test asked for one
    /// discount and stopped.
    #[test]
    fn an_allowance_is_for_what_was_allowed_and_not_for_everything() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();

        // A cashier who may give nothing away unaided, which is the preset
        // every shop uses.
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();

        let fifteen = Discount::Rate(Bp::new(1_500).unwrap());
        till.set_line_discount(0, fifteen)
            .expect_err("a cashier gives nothing away unaided");

        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::Discount { bp: 1_500 },
            1_000,
            60_000,
        )
        .expect("the supervisor is standing there");
        till.set_line_discount(0, fifteen)
            .expect("which is what they allowed");

        // And the rest of the basket is still the shop's. Ninety percent needs
        // asking again, which is the whole point of a ceiling: what got past it
        // can be looked at afterwards, and nothing gets past one set to
        // everything.
        till.set_line_discount(0, Discount::Rate(Bp::new(9_000).unwrap()))
            .expect_err("an allowance of fifteen percent is not an allowance of ninety");

        // The trail says what was allowed, and now the basket agrees with it.
        let written = till
            .unsent_allowed()
            .iter()
            .find(|one| one.action == 1)
            .expect("a discount is written down on whose authority")
            .clone();
        assert_eq!(written.bp, 1_500);
        assert_eq!(written.authorised_by_name, "Owner");
    }

    /// A basket a supervisor approved comes back approved, and comes back at
    /// all.
    ///
    /// Resuming took the ticket off the parked list, persisted that, and only
    /// then applied the discount through the checked setter, which refuses
    /// anything above the ceiling of whoever is at the till now. A cashier
    /// resuming a basket a supervisor had approved at fifteen percent got a
    /// refusal and an empty screen, and the customer's basket was in nobody's
    /// hands: off the list, not on the till. Found by review.
    ///
    /// The waiver travels with it too. It did not, so the basket came back at
    /// the approved price with nothing on the paper saying who approved it,
    /// which is the one line a shop reads when it asks why this price differs
    /// from the shelf.
    #[test]
    fn a_basket_a_supervisor_approved_comes_back_approved() {
        let mut till = stocked_till(MemoryBackend::new());
        till.put_operator(supervisor_operator()).unwrap();

        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Karim".into();
        cashier.pin = crate::auth::PinHash::derive("1234", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        cashier.permissions = crate::auth::Permissions::cashier();
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1234", 0).unwrap();

        till.scan("8690000000001", Milli::ONE).unwrap();
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::Discount { bp: 1_500 },
            1_000,
            60_000,
        )
        .unwrap();
        till.set_ticket_discount(Discount::Rate(Bp::new(1_500).unwrap()))
            .unwrap();
        let approved = till.totals().unwrap().total;

        till.hold(Ulid::from_u128(500), 2_000, "the man with the crate")
            .unwrap();
        assert_eq!(till.held_tickets().unwrap().len(), 1);

        // The supervisor has walked away, and the cashier's own ceiling is
        // nothing. The basket is still the customer's.
        till.resume(Ulid::from_u128(500))
            .expect("a basket already approved comes back");
        assert!(
            till.held_tickets().unwrap().is_empty(),
            "and it is off the parked list, so it cannot be rung twice"
        );
        assert_eq!(
            till.totals().unwrap().total,
            approved,
            "at the price it was parked at"
        );

        pay_cash(&mut till, 200_000);
        let sold = till.checkout(Ulid::from_u128(901), 3_000).unwrap();
        assert!(
            sold.ticket
                .overrides
                .iter()
                .any(|note| note.contains("Owner") && note.contains("15%")),
            "the waiver is on the paper of the sale it belongs to: {:?}",
            sold.ticket.overrides
        );
    }

    /// Somebody buys on account who is in nobody's list.
    ///
    /// The sale used to be written against whatever name was typed and added up
    /// under that spelling, which is how the second Karim pays for the first
    /// one's rice. Writing them down gives the debt a person to go against.
    #[test]
    fn somebody_who_buys_on_account_can_be_written_down_at_the_till() {
        let mut till = stocked_till(MemoryBackend::new());
        let who = Ulid::from_u128(21);
        till.write_customer(wire::CustomerV1 {
            id: who.to_u128(),
            name: alloc::string::String::from("Karim, flat 3"),
            phone: Some(alloc::string::String::from("01711000000")),
            active: true,
            bin: Some(alloc::string::String::from("001234567-0101")),
            limit_minor: 0,
        })
        .unwrap();

        // On the screen at once, and the basket can be pointed at them.
        assert_eq!(till.customers().len(), 1);
        till.set_customer(Some(who)).expect("the basket is theirs");
        assert_eq!(
            till.unsent_customers().len(),
            1,
            "and the shop is owed them"
        );

        // Across a restart, both the list and the obligation.
        let backend = till.journal().backend().clone();
        let (again, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(again.customers().len(), 1);
        assert_eq!(again.customers()[0].name, "Karim, flat 3");
        assert_eq!(again.customers()[0].bin.as_deref(), Some("001234567-0101"));
        assert_eq!(again.unsent_customers().len(), 1);
    }

    /// A supervisor's allowance does not outlive the person it was given to.
    ///
    /// The ceiling used to belong to the basket rather than to whoever was
    /// standing at the till. So a cashier with a basket on the counter got a
    /// supervisor to allow a discount, signed out, and the next cashier signed
    /// in to a basket that still carried the permission: they could give the
    /// discount the supervisor had allowed somebody else, and the trail would
    /// say the supervisor authorised it.
    #[test]
    fn an_allowance_does_not_cross_a_shift_change_on_an_open_basket() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Off);
        // Somebody who may give nothing away without asking, which is every
        // cashier in every shop.
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Rina".into();
        cashier.permissions = crate::auth::Permissions::cashier();
        cashier.pin = crate::auth::PinHash::derive("1111", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        till.put_operator(cashier).unwrap();
        till.sign_in(Ulid::from_u128(71), "1111", 0).unwrap();

        till.scan("8690000000001", Milli::new(1_000)).unwrap();
        // Refused on their own authority, allowed on the supervisor's.
        till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
            .unwrap_err();
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::Discount { bp: 1_000 },
            1_000,
            60_000,
        )
        .unwrap();
        till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
            .expect("the supervisor said so");

        // The shift changes with the basket still on the counter.
        till.sign_out();
        assert_eq!(
            till.cart().limits(),
            crate::cart::CartLimits::default(),
            "the allowance goes when the person it was given to does, rather \
             than sitting on the counter waiting for whoever is next"
        );
        till.sign_in(Ulid::from_u128(71), "1111", 2_000).unwrap();

        assert!(
            !till.cart().is_empty(),
            "the basket is still there: repricing it because somebody changed \
             shift would be worse than either"
        );
        till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
            .expect_err("what a supervisor allowed one person is not the next person's to take");
    }

    /// The same, for somebody who signs in over the top without signing out.
    ///
    /// `sign_in` is the way in whether or not anybody pressed sign out, so the
    /// ceiling has to be put right there as well as on the way out. A shop
    /// where two people share a counter does this all day.
    #[test]
    fn an_allowance_does_not_cross_somebody_signing_in_over_the_top() {
        let mut till = a_till_with_three_on_the_shelf(StockRule::Off);
        let mut cashier = supervisor_operator();
        cashier.id = Ulid::from_u128(71);
        cashier.name = "Rina".into();
        cashier.permissions = crate::auth::Permissions::cashier();
        cashier.pin = crate::auth::PinHash::derive("1111", [4; crate::auth::SALT_LEN], TEST_ROUNDS);
        till.put_operator(cashier.clone()).unwrap();

        let mut other = cashier;
        other.id = Ulid::from_u128(72);
        other.name = "Karim".into();
        other.pin = crate::auth::PinHash::derive("2222", [5; crate::auth::SALT_LEN], TEST_ROUNDS);
        till.put_operator(other).unwrap();

        till.sign_in(Ulid::from_u128(71), "1111", 0).unwrap();
        till.scan("8690000000001", Milli::new(1_000)).unwrap();
        till.authorise(
            Ulid::from_u128(70),
            "9999",
            Action::Discount { bp: 1_000 },
            1_000,
            60_000,
        )
        .unwrap();
        till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
            .expect("the supervisor said so");

        // Somebody else takes the counter, without anybody pressing sign out.
        till.sign_in(Ulid::from_u128(72), "2222", 2_000).unwrap();
        assert_eq!(
            till.cart().limits(),
            crate::cart::CartLimits::default(),
            "a cashier's own ceiling is nothing, and this basket is theirs now"
        );
        till.set_ticket_discount(Discount::Rate(Bp::new(1_000).unwrap()))
            .expect_err("what a supervisor allowed one person is not the next person's to take");
    }
}
