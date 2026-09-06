//! Who owes the shop, and what a sale on account adds to that.
//!
//! A shop here sells on account all day: a regular takes rice now and settles on
//! Friday, and the record of it is a name in a notebook. The till has been able
//! to take "on account" since tenders existed and to name who took it since the
//! receipt learned to print the name, and neither of those adds up. A shop with
//! more than a handful of these keeps the book on paper, which is the thing this
//! product was meant to replace.
//!
//! What identifies a person here is the name the cashier typed, folded so that
//! "Karim", "karim" and "  Karim " are one person rather than three. That is
//! weaker than a customer record with a phone number, and it is what a shop
//! already does: the notebook says "Karim, flat 3" and everybody knows who that
//! is. A stronger identity can be added later without changing what is stored,
//! because the fold is applied where the name is read rather than baked into it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::storage::wire::{TenderKindV1, TicketV1};

/// What one person owes from one ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charge {
    /// The folded name, which is what balances are summed on.
    pub key: String,
    /// The name as the cashier wrote it, kept for showing back.
    pub name: String,
    /// Positive when the shop is owed. A refund on account is negative, which
    /// is the same arithmetic from the other side.
    pub amount_minor: i64,
}

/// What a person is called for the purpose of adding up what they owe.
///
/// Case folded and with runs of spaces collapsed, so a cashier's spacing does
/// not split one person's account in two. Nothing else is stripped: a shop that
/// writes "Karim, flat 3" means the flat number, and dropping it would merge
/// that Karim with the other one.
#[must_use]
pub fn account_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len());
    for word in name.split_whitespace() {
        if !key.is_empty() {
            key.push(' ');
        }
        key.push_str(&word.to_lowercase());
    }
    key
}

/// What a name means when the till did not write one down.
///
/// An older build could take money on account without naming anybody. That debt
/// is real and the shop still has to chase it, so it is recorded under a name
/// that says exactly what happened rather than dropped into a total nobody can
/// take apart.
pub const UNNAMED: &str = "not written down";

/// What a ticket adds to the book, one entry per person named on it.
///
/// Read from the tenders rather than the total: a ticket can be part cash and
/// part on account, and it is only the part on account that anybody owes.
#[must_use]
pub fn charges(ticket: &TicketV1) -> Vec<Charge> {
    let mut found: Vec<Charge> = Vec::new();
    for tender in &ticket.tenders {
        if tender.kind != TenderKindV1::Credit {
            continue;
        }
        let name = tender
            .reference
            .as_deref()
            .map(str::trim)
            .filter(|written| !written.is_empty())
            .unwrap_or(UNNAMED);
        let key = account_key(name);

        // One ticket can carry two tenders naming the same person, which is a
        // cashier correcting themselves rather than two debts.
        match found.iter_mut().find(|charge| charge.key == key) {
            Some(charge) => {
                charge.amount_minor = charge.amount_minor.saturating_add(tender.amount_minor)
            }
            None => found.push(Charge {
                key,
                name: name.to_string(),
                amount_minor: tender.amount_minor,
            }),
        }
    }
    found
}

#[cfg(test)]
// Tests assert with plain arithmetic and panic on failure, which is the point
// of them. The workspace bans both in production code.
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::storage::wire::TenderV1;
    use alloc::vec;

    fn ticket(tenders: Vec<TenderV1>) -> TicketV1 {
        TicketV1 {
            id: 900,
            terminal: 7,
            rung_at_ms: 1_788_600_000_000,
            receipt_no: None,
            receipt_epoch: None,
            customer: None,
            lines: vec![],
            ticket_discount: crate::storage::wire::DiscountV1::None,
            tenders,
            net_minor: 0,
            vat_minor: 0,
            discount_minor: 0,
            total_minor: 0,
            change_minor: 0,
            overrides: vec![],
        }
    }

    fn on_account(name: Option<&str>, amount_minor: i64) -> TenderV1 {
        TenderV1 {
            kind: TenderKindV1::Credit,
            amount_minor,
            reference: name.map(ToString::to_string),
        }
    }

    #[test]
    fn one_person_written_three_ways_is_one_person() {
        assert_eq!(account_key("Karim"), "karim");
        assert_eq!(account_key("  KARIM  "), "karim");
        assert_eq!(account_key("Karim   Uddin"), "karim uddin");
    }

    #[test]
    fn what_makes_two_people_two_is_kept() {
        // A shop writes the flat number because there are two Karims. Folding
        // that away would hand one of them the other's debt.
        assert_ne!(account_key("Karim, flat 3"), account_key("Karim, flat 9"));
    }

    #[test]
    fn only_the_part_on_account_is_owed() {
        let charges = charges(&ticket(vec![
            TenderV1 {
                kind: TenderKindV1::Cash,
                amount_minor: 20_000,
                reference: None,
            },
            on_account(Some("Karim"), 29_450),
        ]));

        assert_eq!(charges.len(), 1);
        assert_eq!(charges[0].amount_minor, 29_450, "not the whole ticket");
        assert_eq!(charges[0].name, "Karim");
    }

    #[test]
    fn a_cashier_correcting_themselves_is_one_debt() {
        let charges = charges(&ticket(vec![
            on_account(Some("Karim"), 10_000),
            on_account(Some("karim"), 5_000),
        ]));

        assert_eq!(charges.len(), 1);
        assert_eq!(charges[0].amount_minor, 15_000);
    }

    #[test]
    fn money_given_away_with_no_name_is_still_written_down() {
        // An older till could do this. The debt is real and the shop still has
        // to chase it, so it appears as a line rather than vanishing.
        let charges = charges(&ticket(vec![on_account(None, 49_450)]));

        assert_eq!(charges.len(), 1);
        assert_eq!(charges[0].name, UNNAMED);
        assert_eq!(charges[0].amount_minor, 49_450);
    }

    #[test]
    fn a_sale_paid_for_owes_nothing() {
        assert!(charges(&ticket(vec![TenderV1 {
            kind: TenderKindV1::Cash,
            amount_minor: 49_450,
            reference: None,
        }]))
        .is_empty());
    }
}
