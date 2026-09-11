//! Booting a till from whatever is on the device, and writing down what has to
//! outlive the log.
//!
//! The cold start the whole design exists for: a tablet switched on at eight in
//! the morning with no internet has to be selling in seconds, from its own
//! files. And the standing state, which is everything the critical log may not
//! hold, because the log is emptied the moment the shop has taken every sale in
//! it: the receipt numbers this terminal was given, the parked baskets, the
//! counted drawers nobody has sent, the people who may sign in, and the drawer
//! that is still open.

use super::*;

impl<B: Backend> Till<B> {
    /// Boot from whatever is on the device.
    ///
    /// Rebuilds the catalogue from its snapshot plus any deltas the log still
    /// holds, restores the receipt-number position from the sales that actually
    /// happened, and reports what it found. No network is involved: this is the
    /// path that must work at eight in the morning with the internet down.
    pub fn open(
        backend: B,
        tenant: u128,
        terminal: TerminalId,
        producer: u16,
        limits: CartLimits,
    ) -> Result<(Self, BootReport)> {
        let (journal, recovery) = Journal::open(backend, tenant, terminal.to_u128(), producer)?;
        let mut replica = Replica::new();
        let (sync, sync_status) = SyncEngine::recover(&journal, &mut replica)?;
        let Standing {
            leases,
            held,
            auth,
            token,
            shop,
            wallets,
            stock_rule,
            unsent_shifts,
            folded_drawer,
            unsent_allowed,
            unsent_items,
            unsent_customers,
            allowed_seq,
            customers,
            credential,
        } = Self::recover_terminal_state(&journal)?;
        // Items this till wrote down and the shop has not got yet, put back into
        // the catalogue. They are already in the replica log, so this is belt
        // and braces for the one case that log cannot cover: a device that lost
        // the catalogue write and kept the obligation.
        for held in &unsent_items {
            replica.apply([crate::replica::ItemDelta::Upsert(
                held.clone().into_domain()?,
            )]);
        }
        let RecoveredShift { shift, counted_by } =
            Self::recover_shift(&journal, terminal, folded_drawer.as_ref())?;
        // A drawer counted in the log with no record of it in the standing state
        // is a device that died in the moment between the two. The count is not
        // repeatable: the till refuses to close a drawer that is already closed,
        // so without this the cashier counted, the till agreed, and the shop
        // never hears the figure.
        let (unsent_shifts, rebuilt) =
            Self::keep_a_count_the_crash_took(unsent_shifts, shift.as_ref(), counted_by.as_ref())?;

        let report = BootReport {
            items: replica.len(),
            unsynced_sales: sync_status.unsynced,
            receipt_numbers_left: leases.remaining(),
            cursor: sync_status.cursor,
            repaired: !recovery.is_clean(),
            catalogue_refetched: sync_status.snapshot_unreadable,
            salvaged_bytes: recovery.salvaged_bytes,
        };

        let mut till = Self {
            journal,
            replica,
            sync,
            leases,
            cart: Cart::new(limits),
            beyond_stock_allowed: false,
            // Every boot starts not having been round the shelf. See the field.
            shelf_swept: false,
            drawer_is_behind: false,
            limits,
            terminal,
            held,
            token,
            shop,
            wallets,
            stock_rule,
            unsent_shifts,
            folded_drawer,
            unsent_allowed,
            unsent_items,
            unsent_customers,
            allowed_seq,
            taken_audit: 0,
            taken_refusals: 0,
            customers,
            credential,
            balances: Vec::new(),
            balances_at_ms: None,
            auth,
            shift,
        };
        if rebuilt {
            // Made durable now rather than at the next thing that writes the
            // blob, because the frame it was rebuilt from goes as soon as the
            // log is emptied.
            till.persist_terminal_state()?;
        }
        Ok((till, report))
    }

    /// Put back a counted drawer that is in the log and not in the queue.
    ///
    /// Returns the queue and whether anything was added. Nothing is invented: a
    /// closed shift replays into the same totals the cashier was shown, and the
    /// name comes off the frame. A count written by a build that did not record
    /// the name comes back with nobody's, which is what that build knew.
    pub(super) fn keep_a_count_the_crash_took(
        mut unsent_shifts: Vec<wire::ClosedShiftV1>,
        shift: Option<&Shift>,
        counted_by: Option<&(u128, alloc::string::String)>,
    ) -> Result<(Vec<wire::ClosedShiftV1>, bool)> {
        let Some(closed) = shift.filter(|shift| !shift.is_open()) else {
            return Ok((unsent_shifts, false));
        };
        let id = closed.id().to_u128();
        if unsent_shifts.iter().any(|held| held.id == id) {
            return Ok((unsent_shifts, false));
        }
        let report = closed.z_report()?;
        let (who, name) = counted_by.map_or((0, alloc::string::String::new()), |(who, name)| {
            (*who, name.clone())
        });
        unsent_shifts.push(wire::ClosedShiftV1 {
            id,
            closed_by: who,
            closed_by_name: name,
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
        Ok((unsent_shifts, true))
    }

    /// Rebuild what the terminal owns: its receipt-number blocks and its parked
    /// baskets.
    ///
    /// The blocks and the baskets come from the standing-state blob, which
    /// survives the critical log being emptied on a full acknowledgement. The
    /// position *within* a block then comes from the sales still in the log,
    /// because a sale is the only proof a number was actually handed over, and
    /// a sale committed after the last blob write would otherwise have its
    /// number issued a second time.
    pub(super) fn recover_terminal_state(journal: &Journal<B>) -> Result<Standing> {
        let mut book = LeaseBook::new();
        let mut held = HeldTicketsV1::default();
        let mut auth = AuthBook::new();
        let mut token = None;
        let mut shop = None;
        let mut wallets: Vec<Box<str>> = Vec::new();
        let mut stock_rule = StockRule::default();
        let mut unsent_shifts: Vec<wire::ClosedShiftV1> = Vec::new();
        let mut folded_drawer: Option<wire::OpenDrawerV1> = None;
        let mut unsent_allowed: Vec<wire::AllowedV1> = Vec::new();
        let mut unsent_items: Vec<wire::ItemV1> = Vec::new();
        let mut unsent_customers: Vec<wire::CustomerV1> = Vec::new();
        let mut allowed_seq = 0_u64;
        let mut customers: Vec<wire::CustomerV1> = Vec::new();
        let mut credential: Option<wire::CredentialV1> = None;

        if let Some((schema, bytes)) = journal.load_terminal_state()? {
            // The schema the bytes were written under, not this build's. A
            // device upgrading reads what the build before it wrote.
            let state = wire::decode_terminal_state(schema, &bytes)?;
            for grant in state.leases {
                book.grant(Lease::new(
                    Ulid::from_u128(grant.terminal),
                    grant.epoch,
                    &grant.prefix,
                    grant.first,
                    grant.last,
                ));
            }
            held = state.held;
            token = state.token;
            unsent_shifts = state.unsent_shifts;
            folded_drawer = state.open_drawer;
            unsent_allowed = state.unsent_allowed;
            unsent_items = state.unsent_items;
            unsent_customers = state.unsent_customers;
            allowed_seq = state.allowed_seq;
            customers = state.customers;
            credential = state.credential;
            shop = state.shop.map(|stored| {
                wallets = stored.wallets.into_iter().map(Into::into).collect();
                stock_rule = StockRule::from_u8(stored.stock_rule);
                crate::receipt::Shop {
                    name: stored.name,
                    bin: stored.bin,
                    address: stored.address,
                    phone: stored.phone,
                }
            });
            for operator in state.operators {
                auth.put(operator.into_domain()?);
            }
        }

        // What the server has already taken. Sales at or below it are its to
        // number, and they stay in the log for as long as a drawer is open, so
        // the count below has to skip them by sequence rather than by absence.
        let acknowledged = Outbox::watermark(journal)?;
        let mut highest_used: Option<u64> = None;
        let mut unnumbered = 0_u64;
        for record in journal.read(Store::Critical)? {
            if record.header.kind == PayloadKind::SaleCommit {
                let sale: SaleCommitV1 = wire::decode_sale(record.header.schema, &record.payload)?;
                // Every position any sale reached, acknowledged or not: the
                // point of this one is not to issue a number twice.
                if let Some(next) = sale.lease_next {
                    highest_used = Some(highest_used.map_or(next, |current| current.max(next)));
                }
                // What the ticket got, not what the block was doing. An
                // exhausted block stays active with its position one past its
                // last number, so a sale that closed with nothing to give it
                // still records a position, and reading that as "numbered" is
                // how a till came back from a restart having forgotten every
                // sale the back office still owes a number to.
                //
                // Counted from the log rather than read from the blob, because a
                // sale that closes without a number would otherwise need a blob
                // write, and therefore a second flush, on the one path that must
                // stay as short as possible. Sales the shop has already taken
                // are its to number, and they stay in the log for as long as a
                // drawer is open, so they are skipped by sequence rather than by
                // being absent.
                if sale.ticket.receipt_no.is_none() && record.header.sequence > acknowledged {
                    unnumbered = unnumbered.saturating_add(1);
                }
            }
        }
        if let Some(next) = highest_used {
            book.resume_at(next);
        }
        book.resume_unnumbered(unnumbered);

        Ok(Standing {
            leases: book,
            held,
            auth,
            token,
            shop,
            wallets,
            stock_rule,
            unsent_shifts,
            folded_drawer,
            unsent_allowed,
            unsent_items,
            unsent_customers,
            allowed_seq,
            customers,
            credential,
        })
    }

    /// Who the shop lets buy on account, by the name a screen should offer.
    ///
    /// Held on the device, like the people who may sign in, because a sale on
    /// account is written with the internet down and a name typed from memory
    /// is how one Karim ends up paying for another Karim's rice.
    #[must_use]
    pub fn customers(&self) -> &[wire::CustomerV1] {
        &self.customers
    }

    /// Take what the shop says each of them owes.
    pub fn set_balances(&mut self, balances: Vec<(u128, i64)>, at_ms: u64) {
        self.balances = balances;
        self.balances_at_ms = Some(at_ms);
    }

    /// What somebody owed when the shop last said, and when that was.
    ///
    /// Both, always: a number without its age is a number a cashier reads out
    /// as though it were true, and another till may have sold to this person
    /// since.
    #[must_use]
    pub fn owed_by(&self, customer: Ulid) -> Option<(Minor, u64)> {
        let at_ms = self.balances_at_ms?;
        self.balances
            .iter()
            .find(|(id, _)| *id == customer.to_u128())
            .map(|(_, owed)| (Minor::new(*owed), at_ms))
    }

    /// Take the shop's list of who may buy on account.
    ///
    /// Whoever this till wrote down and the shop has not got yet is kept on top
    /// of it: the shop's list cannot name them, and dropping them here would
    /// take a person off the screen between writing them down and the shop
    /// hearing about it, with the debt already rung against them.
    pub fn set_customers(&mut self, customers: Vec<wire::CustomerV1>) -> Result<()> {
        let mut list = customers;
        for written in &self.unsent_customers {
            if !list.iter().any(|known| known.id == written.id) {
                list.push(written.clone());
            }
        }
        self.customers = list;
        self.persist_terminal_state()
    }

    /// Write somebody down at the till, so a sale on account has a person to go
    /// against rather than a spelling.
    ///
    /// The shop's list arrives from the back office, and a neighbour buying on
    /// credit for the first time is in nobody's list yet. Until this, the sale
    /// was written against the name that was typed and added up under it, which
    /// is how the second Karim ends up paying for the first one's rice.
    ///
    /// The id is minted by the caller, like a ticket's: this crate has no
    /// entropy. What the shop later holds under that id replaces this.
    ///
    /// # Errors
    /// When there is no name to call them by, or the standing state cannot be
    /// written.
    pub fn write_customer(&mut self, written: wire::CustomerV1) -> Result<()> {
        if written.name.trim().is_empty() {
            return Err(TillError::NamelessCustomer);
        }
        let held = (self.customers.clone(), self.unsent_customers.clone());
        for list in [&mut self.customers, &mut self.unsent_customers] {
            list.retain(|one| one.id != written.id);
            list.push(written.clone());
        }
        if let Err(error) = self.persist_terminal_state() {
            (self.customers, self.unsent_customers) = held;
            return Err(error);
        }
        Ok(())
    }

    /// People this till wrote down and the shop has not got.
    #[must_use]
    pub fn unsent_customers(&self) -> &[wire::CustomerV1] {
        &self.unsent_customers
    }

    /// Forget the people the shop now holds.
    ///
    /// Called with what the server said it stored, never with what was sent: a
    /// reply that did not arrive must leave them here to be sent again. The
    /// list a cashier picks from keeps them either way.
    pub fn customers_accepted(&mut self, stored: &[u128]) -> Result<()> {
        let before = self.unsent_customers.len();
        self.unsent_customers
            .retain(|one| !stored.contains(&one.id));
        if self.unsent_customers.len() != before {
            self.persist_terminal_state()?;
        }
        Ok(())
    }

    /// Say which of them this basket is for.
    ///
    /// Named on the ticket rather than only in the tender's reference, so what
    /// somebody owes is added up against a person the shop has a record of
    /// rather than against the spelling a cashier used that day.
    pub fn set_customer(&mut self, customer: Option<Ulid>) -> Result<()> {
        if let Some(id) = customer
            && !self
                .customers
                .iter()
                .any(|known| known.id == id.to_u128() && known.active)
        {
            return Err(TillError::UnknownCustomer);
        }
        self.cart.set_customer(customer);
        Ok(())
    }

    /// Who this basket is for, if anybody.
    #[must_use]
    pub fn customer(&self) -> Option<Ulid> {
        self.cart.customer()
    }

    /// The wallets this shop takes, by the name a report should read.
    #[must_use]
    pub fn wallets(&self) -> &[Box<str>] {
        &self.wallets
    }

    /// The shop, as its receipts describe it.
    #[must_use]
    pub fn shop(&self) -> Option<&crate::receipt::Shop> {
        self.shop.as_ref()
    }

    /// Record the shop's details, durably.
    ///
    /// A shop with no name is refused. It would print a receipt with an empty
    /// line where the shop should be, which looks like a printer fault and is
    /// not something a customer can take back to anybody.
    /// The shop's own details, and what it takes money by.
    ///
    /// The wallets travel with the shop rather than separately because they
    /// arrive together and are wanted together: a till that knows the shop's
    /// name but not that it takes bKash is a till a cashier has to spell it at.
    pub fn set_shop(
        &mut self,
        shop: crate::receipt::Shop,
        wallets: Vec<Box<str>>,
        stock_rule: StockRule,
    ) -> Result<()> {
        if shop.name.trim().is_empty() {
            return Err(TillError::NamelessShop);
        }
        let held = core::mem::replace(&mut self.wallets, wallets);
        let ruled = core::mem::replace(&mut self.stock_rule, stock_rule);
        // A shop that has turned the rule off stops being sent figures, so what
        // this device holds stops being maintained the moment it does. Turning
        // it on again a month later must not start refusing sales on month-old
        // figures: the lap begins again, and the rule waits for it.
        if stock_rule == StockRule::Off {
            self.shelf_swept = false;
        }
        let previous = self.shop.replace(shop);
        if let Err(error) = self.persist_terminal_state() {
            self.shop = previous;
            self.wallets = held;
            self.stock_rule = ruled;
            return Err(error);
        }
        Ok(())
    }

    /// The credential this terminal syncs with, if it has been enrolled.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Record the credential enrolment returned, durably.
    ///
    /// Written to standing state at once rather than at the next convenient
    /// moment: a device that enrolled, was told it had, and then lost the
    /// credential to a power cut would need the owner to issue another code,
    /// and would give no clue why.
    pub fn set_token(&mut self, token: &str) -> Result<()> {
        let previous = self.token.replace(alloc::string::String::from(token));
        if let Err(error) = self.persist_terminal_state() {
            self.token = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Take a credential and write down when it was taken.
    ///
    /// The time matters as much as the credential: one expires, and a device
    /// that does not know how old its own is cannot renew before it stops
    /// working. `lifetime_ms` is what the shop said one lasts, and is zero on
    /// enrolment because nothing has said yet.
    pub fn take_credential(&mut self, token: &str, at_ms: u64, lifetime_ms: u64) -> Result<()> {
        let previous_token = self.token.replace(alloc::string::String::from(token));
        let previous_note = self.credential.replace(wire::CredentialV1 {
            taken_at_ms: at_ms,
            lifetime_ms,
        });
        if let Err(error) = self.persist_terminal_state() {
            self.token = previous_token;
            self.credential = previous_note;
            return Err(error);
        }
        Ok(())
    }

    /// When this device's credential was taken, and how long the shop says one
    /// lasts. Absent on a device enrolled by a build that did not write it down.
    #[must_use]
    pub fn credential_age(&self) -> Option<(u64, u64)> {
        self.credential
            .map(|note| (note.taken_at_ms, note.lifetime_ms))
    }

    /// Rebuild the open drawer by replaying the log in order.
    ///
    /// Events rather than a stored shift total: the sales are already frames in
    /// this log, so replaying them is what makes the drawer figure and the sales
    /// figure agree by construction. A stored total could only ever disagree
    /// with the sales it claims to summarise, and then there would be no way to
    /// tell which was right.
    ///
    /// Reports who counted it as well, when the log holds a count. That name is
    /// on the frame and not otherwise on a shift, and the record built from the
    /// frame needs it.
    pub(super) fn recover_shift(
        journal: &Journal<B>,
        terminal: TerminalId,
        folded: Option<&wire::OpenDrawerV1>,
    ) -> Result<RecoveredShift> {
        // What was written down when the log under this drawer was dropped, if
        // it was. Everything at or below `folded_through` is already in these
        // figures: the frames may still be there, because the fold is written
        // before the log is emptied and a crash in between leaves both.
        let mut shift: Option<Shift> = folded.map(as_it_was).transpose()?;
        let folded_through = folded.map_or(0, |drawer| drawer.folded_through);
        let mut counted_by: Option<(u128, alloc::string::String)> = None;
        for record in journal.read(Store::Critical)? {
            if record.header.sequence <= folded_through {
                continue;
            }
            match record.header.kind {
                PayloadKind::ShiftEvent => {
                    let event = wire::decode_shift_event(record.header.schema, &record.payload)?;
                    match event {
                        ShiftEventV1::Opened {
                            id,
                            terminal: on,
                            opening_float_minor,
                            at_ms,
                        } => {
                            shift = Some(Shift::open(
                                Ulid::from_u128(id),
                                Ulid::from_u128(on),
                                Minor::new(opening_float_minor),
                                at_ms,
                            )?);
                        }
                        ShiftEventV1::CashMoved {
                            inward,
                            amount_minor,
                            reason,
                            at_ms,
                        } => {
                            if let Some(open) = shift.as_mut() {
                                let amount = Minor::new(amount_minor);
                                if inward {
                                    open.cash_in(amount, &reason, at_ms)?;
                                } else {
                                    open.cash_out(amount, &reason, at_ms)?;
                                }
                            }
                        }
                        ShiftEventV1::Closed {
                            counted_cash_minor,
                            at_ms,
                            counted_by: who,
                            counted_by_name,
                        } => {
                            if let Some(open) = shift.as_mut() {
                                open.close(Minor::new(counted_cash_minor), at_ms)?;
                                counted_by = Some((who, counted_by_name));
                            }
                        }
                    }
                }
                PayloadKind::SaleCommit => {
                    if let Some(open) = shift.as_mut().filter(|open| open.is_open()) {
                        let sale: SaleCommitV1 =
                            wire::decode_sale(record.header.schema, &record.payload)?;
                        // Read before the ticket is consumed. With the change,
                        // which left the drawer as surely as the note came into
                        // it: replaying without it would make a rebuilt drawer
                        // disagree with the one the cashier watched all day.
                        let change = Minor::new(sale.ticket.change_minor);
                        let (_lines, tenders) = sale.ticket.lines_and_tenders()?;
                        open.record_sale(&tenders, change)?;
                    }
                }
                _ => {}
            }
        }
        let _ = terminal;
        Ok(RecoveredShift { shift, counted_by })
    }

    /// Write down what this terminal owns.
    ///
    /// Called after anything that changes the blocks in hand or the baskets on
    /// the counter, and always before the critical log is emptied. The blob is
    /// A/B, so a device dying mid-write comes back one step stale rather than
    /// with nothing.
    pub(super) fn persist_terminal_state(&mut self) -> Result<()> {
        let mut leases = Vec::new();
        for lease in self.leases.blocks() {
            leases.push(LeaseGrantV1 {
                terminal: lease.terminal.to_u128(),
                epoch: lease.epoch,
                prefix: alloc::string::String::from(&*lease.prefix),
                first: lease.next,
                last: lease.last,
            });
        }
        let bytes = wire::encode_terminal_state(&TerminalStateV1 {
            unsent_shifts: self.unsent_shifts.clone(),
            unsent_allowed: self.unsent_allowed.clone(),
            unsent_items: self.unsent_items.clone(),
            unsent_customers: self.unsent_customers.clone(),
            allowed_seq: self.allowed_seq,
            customers: self.customers.clone(),
            credential: self.credential,
            leases,
            held: self.held.clone(),
            unnumbered: self.leases.unnumbered(),
            token: self.token.clone(),
            shop: self.shop.as_ref().map(|shop| wire::ShopV1 {
                name: shop.name.clone(),
                bin: shop.bin.clone(),
                address: shop.address.clone(),
                phone: shop.phone.clone(),
                wallets: self.wallets.iter().map(ToString::to_string).collect(),
                stock_rule: self.stock_rule.as_u8(),
            }),
            operators: self
                .auth
                .operators()
                .iter()
                .map(OperatorV1::from_domain)
                .collect(),
            // Written only once the log under the drawer has been dropped. See
            // fold_the_open_drawer: while the log is there the drawer is the
            // frames in it, and two homes for one drawer is two answers.
            open_drawer: self.folded_drawer.clone(),
        })?;
        self.journal.write_terminal_state(&bytes)?;
        Ok(())
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
    
    
    
    use crate::storage::backend::MemoryBackend;
    use crate::storage::wire::ItemV1;

    use super::super::proof::*;

    #[test]
    fn boots_empty_and_reports_it() {
        let (_till, report) = Till::open(
            MemoryBackend::new(),
            TENANT,
            terminal(),
            1,
            CartLimits::unrestricted(),
        )
        .unwrap();
        assert_eq!(report.items, 0);
        assert_eq!(report.unsynced_sales, 0);
        assert_eq!(report.receipt_numbers_left, 0);
        assert!(!report.repaired);
    }

    #[test]
    fn a_cold_start_restores_the_catalogue_stock_and_number_position() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            till.checkout(Ulid::from_u128(900), 0).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, report) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();

        assert_eq!(report.items, 1, "the catalogue comes back from the log");
        assert_eq!(
            report.unsynced_sales, 1,
            "the sale is still owed to the server"
        );
        assert_eq!(
            report.receipt_numbers_left, 499,
            "the used number is not reissued"
        );

        // The next number continues rather than restarting the block.
        let mut till = till;
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let next = till.checkout(Ulid::from_u128(901), 0).unwrap();
        assert_eq!(next.receipt_no.as_deref(), Some("T1-000101"));
    }

    #[test]
    fn a_discarded_basket_does_not_come_back_after_a_restart() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            till.hold(Ulid::from_u128(500), 0, "abandoned").unwrap();
            till.discard_held(Ulid::from_u128(500)).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(
            till.held_tickets().unwrap().is_empty(),
            "the newest written set is the answer, so a cancellation sticks"
        );
    }

    #[test]
    fn who_buys_on_account_survives_a_reboot() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.set_customers(alloc::vec![wire::CustomerV1 {
                id: 21,
                name: "Karim, flat 3".into(),
                phone: None,
                active: true,
                bin: None,
                limit_minor: 0,
            }])
            .unwrap();
            backend = till.journal().backend().clone();
        }

        // The point of holding them at all: a sale on account is written with
        // the internet down, and a name typed from memory is how one Karim ends
        // up paying for another Karim's rice.
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(till.customers().len(), 1);
        assert_eq!(till.customers()[0].name, "Karim, flat 3");
    }

    /// A restart starts the lap again, because the figures do not survive it.
    ///
    /// The shelf answers live in the replica, and the replica reaches the disk
    /// as a snapshot rewritten when the delta log has grown: a sweep changes no
    /// catalogue rows, so nothing it learned is saved by anything it does. This
    /// is that fact, written as a test, because it is the reason the till does
    /// not write down that it has been round: for a day it did, and a reloaded
    /// till came back claiming to know a shelf while holding the catalogue's
    /// own figures, which are zero. In a shop whose rule says refuse, that till
    /// turned away everything scanned at it.
    #[test]
    fn a_restart_has_not_been_round_the_shelf_and_says_so() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.apply_on_hand(&[(Ulid::from_u128(1), Milli::new(61_000))]);
            till.shelf_swept();
            assert!(till.shelf_known());
            backend = till.journal().backend().clone();
        }
        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert!(
            !till.shelf_known(),
            "the figures it went round for are not here, so neither is the claim"
        );
        assert_ne!(
            till.catalogue()
                .by_id(Ulid::from_u128(1))
                .map(|item| item.on_hand),
            Some(Milli::new(61_000)),
            "if this ever survives a restart, this test is the thing to change: write down that \
             the device has been round the shelf again, and say why it is safe"
        );
    }

    /// The sales the back office still owes a number to, after the tablet
    /// restarts.
    ///
    /// They were counted from the log by asking what the block was doing rather
    /// than what the ticket got. A spent block stays active with its position
    /// one past its last number, so a sale that closed with nothing to give it
    /// still recorded a position, and reading that as "numbered" meant the till
    /// came back owing nothing: the receipts went out blank and the shop was
    /// never asked to fill them in.
    #[test]
    fn a_sale_still_waiting_for_a_number_is_still_waiting_after_a_restart() {
        let (till, _) = a_till_whose_numbers_ran_out();
        let backend = till.journal().backend().clone();

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.status().unwrap().unnumbered_sales,
            1,
            "the shop has not taken it, so the number is still owed"
        );
    }

    #[test]
    fn refuses_to_boot_on_another_terminals_log() {
        let mut backend = MemoryBackend::new();
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            till.checkout(Ulid::from_u128(900), 0).unwrap();
            backend = till.journal().backend().clone();
        }

        // The same image on a second tablet. Booting it would issue the first
        // terminal's numbers a second time, under the same epoch, on paper.
        let cloned = Till::open(
            backend,
            TENANT,
            Ulid::from_u128(999),
            1,
            CartLimits::unrestricted(),
        );
        assert!(matches!(
            cloned.err(),
            Some(TillError::Journal(JournalError::ForeignLog { .. }))
        ));
    }

    #[test]
    fn a_log_that_has_gone_bad_is_not_emptied_under_the_sales_behind_it() {
        let mut till = stocked_till(MemoryBackend::new());
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let first = till.checkout(Ulid::from_u128(900), 0).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        till.checkout(Ulid::from_u128(901), 0).unwrap();

        // A byte goes bad inside the first sale, the way a device does it:
        // while the till is running, with the second sale behind it.
        let log = till
            .journal()
            .backend()
            .read_log(Store::Critical)
            .unwrap();
        let at = log.len() / 3;
        let mut rotten = log.clone();
        rotten[at] ^= 0xff;
        {
            let backend = till.journal_mut().backend_mut();
            backend.truncate_log(Store::Critical, 0).unwrap();
            backend.append_log(Store::Critical, &rotten).unwrap();
        }

        // The outbox can no longer see the second sale, so the shop says the
        // first one is settled and the till has nothing left to send. This is
        // the moment the log would be emptied.
        till.acknowledge(&[first.ticket.id]).unwrap();
        assert!(
            Outbox::pending(till.journal()).unwrap().is_empty(),
            "the sale behind the bad frame is what the outbox cannot see"
        );
        till.empty_the_log_if_nothing_needs_it().unwrap();

        let after = till
            .journal()
            .backend()
            .read_log(Store::Critical)
            .unwrap();
        assert_eq!(
            after.len(),
            rotten.len(),
            "the log keeps every byte it has, including the sale nobody can see yet"
        );
    }

    #[test]
    fn journal_sequences_do_not_restart_after_the_log_is_emptied() {
        let mut backend = MemoryBackend::new();
        let before;
        {
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
            before = sale.journal_sequence;
            till.acknowledge(&[sale.ticket.id]).unwrap();
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let after = till.checkout(Ulid::from_u128(901), 0).unwrap();

        assert!(
            after.journal_sequence > before,
            "sequences must not rewind when the log is emptied: {} then {}",
            before,
            after.journal_sequence
        );
    }

    #[test]
    fn a_drained_till_still_holds_its_numbers_after_a_reboot() {
        // The commonest path in the product: the shop drains its outbox at close
        // of business, then opens next morning with the internet down.
        let mut backend = MemoryBackend::new();
        {
            // The helper already holds a block of five hundred numbers.
            let mut till = stocked_till(backend.clone());
            till.scan("8690000000001", Milli::ONE).unwrap();
            pay_cash(&mut till, 50_000);
            let sale = till.checkout(Ulid::from_u128(900), 0).unwrap();
            till.acknowledge(&[sale.ticket.id]).unwrap();
            backend = till.journal().backend().clone();
        }

        let (till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        assert_eq!(
            till.leases().remaining(),
            499,
            "draining the outbox must not spend the numbers the server already issued"
        );
    }

    #[test]
    fn a_reboot_after_crossing_a_block_boundary_does_not_reissue_numbers() {
        let mut backend = MemoryBackend::new();
        let issued;
        {
            // A bare till, so the only blocks in hand are the two granted here:
            // a short active block and the reserve that lets the till keep
            // selling across the boundary while offline.
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
            till.grant_lease(&Lease::new(terminal(), 1, "T1", 100, 101))
                .unwrap();
            till.grant_lease(&Lease::new(terminal(), 1, "T1", 600, 699))
                .unwrap();

            let mut numbers = alloc::vec::Vec::new();
            for index in 0..4_u128 {
                till.scan("8690000000001", Milli::ONE).unwrap();
                pay_cash(&mut till, 50_000);
                let sale = till.checkout(Ulid::from_u128(900 + index), 0).unwrap();
                numbers.push(sale.ticket.receipt_no.clone());
            }
            issued = numbers;
            backend = till.journal().backend().clone();
        }

        let (mut till, _) =
            Till::open(backend, TENANT, terminal(), 1, CartLimits::unrestricted()).unwrap();
        till.scan("8690000000001", Milli::ONE).unwrap();
        pay_cash(&mut till, 50_000);
        let after = till.checkout(Ulid::from_u128(999), 0).unwrap();

        assert!(
            !issued.contains(&after.ticket.receipt_no),
            "a number already on a customer\'s receipt was printed again: {:?} after {:?}",
            after.ticket.receipt_no,
            issued
        );
    }
}
