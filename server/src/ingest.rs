//! Accepting sales from a till.
//!
//! Three rules shape this, and all three follow from the same fact: by the time
//! a sale reaches here, it has already happened. Goods left the shop and money
//! changed hands.
//!
//! 1. **Idempotent.** A dropped connection means the till sends again. The ULID
//!    was minted on the device, so a replay is recognised and costs nothing.
//! 2. **Never reject.** A sale the server refuses stays only on a tablet, which
//!    may not survive the week. Anything suspicious is stored *and* flagged for
//!    a human, rather than bounced back.
//! 3. **Revalidate with the same arithmetic.** The totals are recomputed with
//!    the very crate the till used. Because it is the same code, a disagreement
//!    cannot be a rounding difference: it means corruption or tampering, and is
//!    worth waking someone for.

use openpos_core::domain::{TicketInput, ticket_totals};
use openpos_core::protocol::{
    AdoptSalesRequest, AdoptSalesResponse, ProtocolError, PushRequest, PushResponse,
    QuarantineReason, Quarantined, SaleEnvelope, negotiate,
};
use openpos_core::storage::wire::{self, SaleCommitV1};

use openpos_core::accounts::charges;

use crate::repo::{AccountCharge, Admission, Repository, StoredSale};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestError {
    Protocol(ProtocolError),
    /// The store failed. The whole batch is refused so the till keeps its copy
    /// and tries again; telling it otherwise would let it drop the only record.
    Storage,
}

impl From<ProtocolError> for IngestError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

pub type Result<T> = std::result::Result<T, IngestError>;

/// Take a batch of sales from a till.
///
/// Accepts an unsized repository so the HTTP layer can hold one behind a trait
/// object without the logic caring which implementation it is.
pub async fn push<R: Repository + ?Sized>(repo: &R, request: &PushRequest) -> Result<PushResponse> {
    let protocol = negotiate(request.protocol)?;

    if !repo
        .terminal_enrolled(request.tenant, request.terminal)
        .await
        .map_err(|_| IngestError::Storage)?
    {
        return Err(IngestError::Protocol(ProtocolError::UnknownTerminal));
    }

    // How far back a sale in this shop can go. The shop's own beginning, not
    // the device's: a tablet wiped and enrolled again is a new terminal row
    // holding sales it rang yesterday, and those are perfectly good.
    let shop_began_ms = repo
        .tenant_created_at(request.tenant)
        .await
        .map_err(|_| IngestError::Storage)?
        .unwrap_or_default();

    // The shop's own clock, read once for the batch. What it is for is saying
    // whether a till's clock can be believed: the timestamp on a sale decides
    // which day's takings it lands in and which month's return it is declared
    // on, and a cheap tablet that has been switched off for a week comes back
    // believing it is 2010.
    let arrived_at_ms = repo.now_ms().await.map_err(|_| IngestError::Storage)?;

    let mut accepted = Vec::with_capacity(request.sales.len());
    let mut quarantined = Vec::new();

    for envelope in &request.sales {
        // Assessment, storage, the idempotency check and the receipt claim are
        // one round trip and one transaction. It used to be three, and the
        // window between the second and the third is where two pushes carrying
        // the same receipt number both read "free" and both stored clean.
        let (sale, suspicion) = match assess(request, envelope) {
            Assessment::Clean(sale) => {
                // A refund names the receipt it reverses, and until now nothing
                // read it. Asked before the clock, because a refund against a
                // sale nobody has is the more useful thing to say about it.
                let against = match refund_is_answerable(repo, &sale).await {
                    Ok(reason) => reason,
                    Err(error) => return Err(error),
                };
                match against.or_else(|| impossible_clock(&sale, shop_began_ms, arrived_at_ms)) {
                    // Held for a person rather than refused. The goods left the
                    // shop and the money is real; what nobody can settle without
                    // somebody who was there is which day it belongs to.
                    Some(reason) => {
                        let mut held = sale;
                        held.quarantine = Some(reason.clone());
                        (held, Some(reason))
                    }
                    None => (sale, None),
                }
            }
            Assessment::Suspect(sale, reason) => (sale, Some(reason)),
        };

        let admission = repo
            .admit_sale(sale)
            .await
            .map_err(|_| IngestError::Storage)?;

        match (admission, suspicion) {
            // A replay after a dropped reply. Doing the work twice would double
            // the stock movement, and refusing it would strand a sale on a
            // tablet.
            (Admission::AlreadyStored, _) => accepted.push(envelope.id),
            // The database, not this code, decided the number was taken.
            (Admission::DuplicateReceipt { .. }, _) => quarantined.push(Quarantined {
                id: envelope.id,
                reason: QuarantineReason::DuplicateReceiptNumber {
                    receipt_no: receipt_of(envelope),
                },
            }),
            (Admission::Stored, Some(reason)) => {
                quarantined.push(Quarantined {
                    id: envelope.id,
                    reason,
                });
            }
            (Admission::Stored, None) => accepted.push(envelope.id),
        }
    }

    Ok(PushResponse {
        protocol,
        accepted,
        quarantined,
    })
}

/// Take sales somebody carried in from a device that could not send them.
///
/// The ordinary path proves where a sale came from with the terminal's own
/// credential. This one cannot: the device may have lost it, or the shop may
/// have deleted the terminal, which is how it got here. So every carried sale
/// is stored and every one of them is put in front of a person, and the caller
/// is an owner rather than a till.
pub async fn adopt<R: Repository + ?Sized>(
    repo: &R,
    tenant: u128,
    request: &AdoptSalesRequest,
) -> Result<AdoptSalesResponse> {
    let protocol = negotiate(request.protocol)?;

    let mut adopted = Vec::with_capacity(request.sales.len());
    let mut needing_attention = Vec::new();
    for envelope in &request.sales {
        let carried = PushRequest {
            protocol,
            tenant,
            terminal: request.terminal,
            sales: Vec::new(),
        };
        let assessment = assess(&carried, envelope);
        let (mut stored, worse) = match assessment {
            Assessment::Clean(stored) => (stored, None),
            Assessment::Suspect(stored, reason) => (stored, Some(reason)),
        };
        // Carried in is itself a reason to look, and anything worse than that
        // replaces it rather than being lost behind it.
        let reason = worse.unwrap_or(QuarantineReason::CarriedIn);
        stored.quarantine = Some(reason.clone());

        match repo.admit_sale(stored).await {
            Ok(Admission::Stored) => {
                adopted.push(envelope.id);
                needing_attention.push(Quarantined {
                    id: envelope.id,
                    reason,
                });
            }
            // The shop already had it, by the ordinary route or by an earlier
            // attempt at this one. Saying it is waiting on a person would send
            // somebody looking for a queue entry that is not there.
            Ok(Admission::AlreadyStored) => adopted.push(envelope.id),
            Ok(Admission::DuplicateReceipt { .. }) => {
                adopted.push(envelope.id);
                needing_attention.push(Quarantined {
                    id: envelope.id,
                    reason: QuarantineReason::DuplicateReceiptNumber {
                        receipt_no: receipt_of(envelope),
                    },
                });
            }
            Err(_) => return Err(IngestError::Storage),
        }
    }

    Ok(AdoptSalesResponse {
        protocol,
        adopted,
        needing_attention,
    })
}

enum Assessment {
    Clean(StoredSale),
    Suspect(StoredSale, QuarantineReason),
}

/// The receipt number a payload claims, for naming a duplicate in the reply.
fn receipt_of(envelope: &SaleEnvelope) -> String {
    wire::decode_sale(envelope.schema, &envelope.payload)
        .ok()
        .and_then(|sale| sale.ticket.receipt_no)
        .unwrap_or_default()
}

/// Everything that can be decided from the payload alone.
///
/// Takes no repository, because it now asks the database nothing: the duplicate
/// check moved into the same transaction as the write, where it cannot be
/// raced.
/// How far a till's clock may be out before the sale needs a person.
///
/// An hour each way. Devices and servers drift by seconds and a batch can sit in
/// a queue for minutes, so a tighter window would hold ordinary sales; an hour
/// is far more than either and far less than the days a wrong clock is out by.
const CLOCK_TOLERANCE_MS: u64 = 60 * 60 * 1_000;

/// Whether the till's clock says something that cannot be true.
///
/// Two impossibilities, both unambiguous. A sale cannot be rung after the shop
/// received it, and it cannot be rung before the shop existed. Everything
/// between them is left alone: a device offline for six months has six-month-old
/// sales and they are perfectly good, and a tablet wiped and enrolled again is a
/// new terminal row holding yesterday's.
fn impossible_clock(
    sale: &StoredSale,
    shop_began_ms: u64,
    arrived_at_ms: u64,
) -> Option<QuarantineReason> {
    let ahead = sale.rung_at_ms > arrived_at_ms.saturating_add(CLOCK_TOLERANCE_MS);
    let before_the_shop = sale.rung_at_ms < shop_began_ms.saturating_sub(CLOCK_TOLERANCE_MS);
    (ahead || before_the_shop).then_some(QuarantineReason::ClockOutOfRange {
        rung_at_ms: sale.rung_at_ms,
        received_at_ms: arrived_at_ms,
    })
}

fn assess(request: &PushRequest, envelope: &SaleEnvelope) -> Assessment {
    let Ok(sale) = wire::decode_sale(envelope.schema, &envelope.payload) else {
        // Undecodable, so nothing can be said about it. Store the bytes anyway:
        // they are evidence, and a later build may know how to read them.
        return Assessment::Suspect(
            StoredSale {
                tenant: request.tenant,
                terminal: request.terminal,
                id: envelope.id,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: 0,
                total_minor: 0,
                payload: envelope.payload.clone(),
                quarantine: Some(QuarantineReason::Undecodable),
                // Nothing can be read out of bytes nobody can decode.
                refund_of: None,
                stock: Vec::new(),
                // Nothing can be read out of bytes nobody can decode, including
                // who owes for them. It is in the repair queue for a person to
                // look at, which is the only thing left to do with it.
                vat: Vec::new(),
                overrides: Vec::new(),
                on_account: Vec::new(),
            },
            QuarantineReason::Undecodable,
        );
    };

    let stored = build(request, envelope, &sale, None);

    if let Some(reason) = totals_disagree(&sale) {
        return Assessment::Suspect(
            build(request, envelope, &sale, Some(reason.clone())),
            reason,
        );
    }

    // The duplicate receipt check used to live here, as a read. It is now part
    // of the write, where a primary key decides it and no two connections can
    // both be told the number is free.
    Assessment::Clean(stored)
}

/// Recompute the totals and compare them with what the till stored.
///
/// Both sides run `openpos_core::domain`, so this can only differ if the bytes
/// were altered after the till wrote them. That makes it a strong signal rather
/// than a tolerance check, and it is why no epsilon appears here.
///
/// Every failure to recompute quarantines rather than passing. Reading a
/// decode error or an overflow as agreement would mean a payload that cannot be
/// checked is treated exactly like one that checked out, and anybody wanting to
/// bypass the check would only have to make the arithmetic fail instead of
/// disagree: a VAT rate above 100 percent does it.
fn totals_disagree(sale: &SaleCommitV1) -> Option<QuarantineReason> {
    let ticket = sale.ticket.clone();
    let stored_minor = ticket.total_minor;
    let Ok(discount) = ticket.ticket_discount.clone().into_domain() else {
        return Some(QuarantineReason::Undecodable);
    };
    let Ok((lines, _tenders)) = ticket.lines_and_tenders() else {
        return Some(QuarantineReason::Undecodable);
    };

    let Ok(recomputed) = ticket_totals(&TicketInput {
        lines: lines
            .iter()
            .map(openpos_core::cart::CartLine::as_input)
            .collect(),
        ticket_discount: discount,
    }) else {
        return Some(QuarantineReason::Undecodable);
    };

    if recomputed.total.get() == stored_minor {
        return None;
    }
    Some(QuarantineReason::TotalsMismatch {
        stored_minor,
        recomputed_minor: recomputed.total.get(),
    })
}

/// What the shop can say about the receipt a refund reverses.
///
/// Two things are worth holding a refund for. The receipt it names is one this
/// shop does not have, which is either a till whose sales have not arrived or a
/// refund against nothing at all. Or more has now been refunded against that
/// receipt than it was ever rung for, which is the oldest trick at a counter and
/// is also what a customer bringing half a basket back twice looks like.
///
/// Neither is refused. The goods came back and the money went out, and the only
/// copy of that is the one arriving.
async fn refund_is_answerable<R: Repository + ?Sized>(
    repo: &R,
    sale: &StoredSale,
) -> Result<Option<QuarantineReason>> {
    let Some(receipt_no) = sale.refund_of.as_deref() else {
        return Ok(None);
    };
    let found = repo
        .refunded_against(sale.tenant, receipt_no)
        .await
        .map_err(|_| IngestError::Storage)?;

    let Some((sale_minor, refunded_minor)) = found else {
        return Ok(Some(QuarantineReason::RefundAgainstNothing {
            receipt_no: receipt_no.to_owned(),
        }));
    };

    // A refund is negative and the sale it reverses is positive, so what has
    // been given back is the negation of the sum. This one is not stored yet,
    // which is the point of checking now.
    let given_back = refunded_minor
        .saturating_add(sale.total_minor)
        .saturating_neg();
    if given_back > sale_minor {
        return Ok(Some(QuarantineReason::RefundBeyondTheSale {
            receipt_no: receipt_no.to_owned(),
            sale_minor,
            refunded_minor: given_back,
        }));
    }

    // And the goods, which the money does not answer for. A refund of the same
    // taka made of something else, or of more of one thing than was ever
    // bought, puts stock on the shelf that never left it: that is how a count
    // is made to agree with a shelf somebody emptied.
    let mut net = repo
        .goods_against(sale.tenant, receipt_no)
        .await
        .map_err(|_| IngestError::Storage)?;
    for (item, qty) in &sale.stock {
        match net.iter_mut().find(|(known, _)| known == item) {
            Some((_, total)) => *total = total.saturating_add(*qty),
            None => net.push((*item, *qty)),
        }
    }
    // A sale's movement is negative and a refund's is positive, so anything
    // above zero came back more than it went out.
    if let Some((item, over_by)) = net.into_iter().find(|(_, moved)| *moved > 0) {
        return Ok(Some(QuarantineReason::MoreCameBackThanWentOut {
            receipt_no: receipt_no.to_owned(),
            item_id: item,
            over_by_milli: over_by,
        }));
    }
    Ok(None)
}

fn build(
    request: &PushRequest,
    envelope: &SaleEnvelope,
    sale: &SaleCommitV1,
    quarantine: Option<QuarantineReason>,
) -> StoredSale {
    StoredSale {
        tenant: request.tenant,
        terminal: request.terminal,
        id: envelope.id,
        receipt_no: sale.ticket.receipt_no.clone(),
        receipt_epoch: sale.ticket.receipt_epoch,
        rung_at_ms: sale.ticket.rung_at_ms,
        total_minor: sale.ticket.total_minor,
        payload: envelope.payload.clone(),
        quarantine,
        // Beside the sale as well as inside its bytes, so the shop can ask what
        // has been refunded against a receipt without reading its whole ledger.
        refund_of: sale.refund_of.clone(),
        stock: stock_from_lines(sale),
        // Recomputed with the same crate the till used, like the totals check
        // above: what a shop declares to the revenue must not be something a
        // payload could assert. A ticket that cannot be recomputed declares
        // nothing and is in the queue for a person instead.
        vat: vat_from_lines(sale),
        // What a supervisor waived, from the same bytes the customer's receipt
        // was printed from. Carried rather than recomputed, because unlike the
        // totals and the tax there is nothing to recompute it against: it is
        // what somebody at the till decided, and the ticket is the record.
        overrides: sale.ticket.overrides.clone(),
        // Read from the tenders here rather than believed from a separate
        // field, for the same reason the stock movements are: a payload that
        // says what it likes about who owes what would be a way to write off a
        // debt by editing a sale.
        on_account: charges(&sale.ticket)
            .into_iter()
            .map(|charge| AccountCharge {
                person_key: charge.key,
                person_name: charge.name,
                amount_minor: charge.amount_minor,
            })
            .collect(),
    }
}

/// Work out what left the shelf from the ticket, rather than believing the
/// movements the payload carries.
///
/// The two are computed from the same lines by the same crate, so on an honest
/// sale they agree. On a tampered one they do not, and trusting the sent
/// movements let a terminal decrement any item it liked, or none at all, while
/// carrying a ticket whose totals recompute perfectly and sail through the
/// tamper check.
///
/// Summed per item, matching what the till writes: one item legitimately
/// appears on two lines when the first carries a discount, and the ledger keys
/// a movement on the sale and the item.
/// What a ticket owed the revenue, by rate, recomputed here.
pub fn vat_from_lines(sale: &SaleCommitV1) -> Vec<(u32, i64, i64)> {
    let ticket = sale.ticket.clone();
    let Ok(discount) = ticket.ticket_discount.clone().into_domain() else {
        return Vec::new();
    };
    let Ok((lines, _)) = ticket.lines_and_tenders() else {
        return Vec::new();
    };
    let Ok(totals) = ticket_totals(&TicketInput {
        lines: lines
            .iter()
            .map(openpos_core::cart::CartLine::as_input)
            .collect(),
        ticket_discount: discount,
    }) else {
        return Vec::new();
    };
    openpos_core::domain::vat_by_rate(&totals)
        .into_iter()
        .map(|(bp, net, vat)| (bp, net.get(), vat.get()))
        .collect()
}

fn stock_from_lines(sale: &SaleCommitV1) -> Vec<(u128, i64)> {
    let mut movements: Vec<(u128, i64)> = Vec::with_capacity(sale.ticket.lines.len());
    for line in &sale.ticket.lines {
        let moved = line.qty_milli.saturating_neg();
        match movements.iter_mut().find(|(item, _)| *item == line.item_id) {
            Some((_, total)) => *total = total.saturating_add(moved),
            None => movements.push((line.item_id, moved)),
        }
    }
    movements
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

    use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
    use openpos_core::domain::PriceMode;
    use openpos_core::ids::Ulid;
    use openpos_core::money::{Bp, Milli, Minor};
    use openpos_core::protocol::PROTOCOL_VERSION;
    use openpos_core::replica::Item;
    use openpos_core::storage::wire::{SALE_SCHEMA, encode_sale, sale_commit};

    use super::*;
    use crate::repo::MemoryRepo;

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    fn item() -> Item {
        Item {
            id: Ulid::from_u128(1),
            code: "SKU001".into(),
            name_en: "Rice Miniket 5kg".into(),
            name_bn: "মিনিকেট চাল ৫ কেজি".into(),
            unit: "Nos".into(),
            price: Minor::new(43_000),
            cost: Minor::new(38_000),
            vat_rate: Bp::new(1_500).unwrap(),
            price_mode: PriceMode::Exclusive,
            vat_base: openpos_core::domain::VatBase::Discounted,
            barcodes: vec!["8690000000012".into()],
            on_hand: Milli::new(40_000),
            active: true,
        }
    }

    /// A sale exactly as a till would have committed it, at a clock the caller
    /// chooses: the clock is the whole of what some of these tests are about.
    fn envelope_at(id: u128, receipt: Option<&str>, rung_at_ms: u64) -> SaleEnvelope {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(), Milli::ONE).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(50_000),
            reference: None,
        });
        let mut ticket = cart
            .close(Ulid::from_u128(id), Ulid::from_u128(TERMINAL), rung_at_ms)
            .unwrap();
        ticket.receipt_no = receipt.map(Into::into);

        let payload = encode_sale(&sale_commit(&ticket, receipt.map(|_| 1), Some(101))).unwrap();
        SaleEnvelope {
            id,
            schema: SALE_SCHEMA,
            payload,
        }
    }

    /// A sale exactly as a till would have committed it.
    fn envelope(id: u128, receipt: Option<&str>) -> SaleEnvelope {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(), Milli::ONE).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(50_000),
            reference: None,
        });
        let mut ticket = cart
            .close(
                Ulid::from_u128(id),
                Ulid::from_u128(TERMINAL),
                1_788_600_000_000,
            )
            .unwrap();
        ticket.receipt_no = receipt.map(Into::into);

        let payload = encode_sale(&sale_commit(&ticket, receipt.map(|_| 1), Some(101))).unwrap();
        SaleEnvelope {
            id,
            schema: SALE_SCHEMA,
            payload,
        }
    }

    fn request(sales: Vec<SaleEnvelope>) -> PushRequest {
        PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sales,
        }
    }

    fn repo() -> MemoryRepo {
        let repo = MemoryRepo::new();
        // Enrolled before it sold, which is the order a shop does it in.
        repo.enrol_at(TENANT, TERMINAL, 1_788_000_000_000);
        repo
    }

    #[tokio::test]
    async fn accepts_a_clean_batch() {
        let repo = repo();
        let response = push(
            &repo,
            &request(vec![
                envelope(900, Some("T1-000100")),
                envelope(901, Some("T1-000101")),
            ]),
        )
        .await
        .unwrap();

        assert_eq!(response.accepted, vec![900, 901]);
        assert!(response.quarantined.is_empty());
        assert_eq!(repo.sale_count(TENANT), 2);
        assert_eq!(repo.sale(TENANT, 900).unwrap().total_minor, 49_450);
    }

    #[tokio::test]
    async fn a_sale_rung_after_it_arrived_is_held_for_a_person() {
        let repo = MemoryRepo::new();
        repo.enrol_at(TENANT, TERMINAL, 1_700_000_000_000);

        // A tablet whose clock is a year ahead. The sale is real and the money
        // is real; what nobody can settle without somebody who was there is
        // which day's takings and which month's return it belongs to.
        let a_year = 365 * 24 * 60 * 60 * 1_000;
        let ahead = repo.now_ms().await.expect("the shop's own clock") + a_year;
        let sale = envelope_at(900, Some("T1-000100"), ahead);
        let response = push(&repo, &request(vec![sale]))
            .await
            .expect("it is stored");

        assert!(response.accepted.is_empty(), "not taken as it stands");
        assert_eq!(response.quarantined.len(), 1);
        assert!(
            repo.quarantined(TENANT).iter().any(|held| matches!(
                held.quarantine,
                Some(QuarantineReason::ClockOutOfRange { .. })
            )),
            "and the queue says which device to look at"
        );
    }

    #[tokio::test]
    async fn a_sale_rung_before_the_device_existed_is_held_too() {
        let repo = MemoryRepo::new();
        repo.enrol_at(TENANT, TERMINAL, 1_700_000_000_000);

        // The other direction: a cheap tablet switched off long enough to
        // forget the year comes back believing it is 2010, which is before the
        // shop existed.
        let sale = envelope_at(901, Some("T1-000101"), 1_262_304_000_000);
        let response = push(&repo, &request(vec![sale]))
            .await
            .expect("it is stored");

        assert!(response.accepted.is_empty());
        assert_eq!(response.quarantined.len(), 1);
    }

    #[tokio::test]
    async fn a_device_offline_for_a_month_is_left_alone() {
        let repo = MemoryRepo::new();
        repo.enrol_at(TENANT, TERMINAL, 1_700_000_000_000);

        // Between the two impossibilities everything is ordinary. A month of
        // sales carried on a device with no line is exactly what this product
        // is for, and holding those would hold most of what it exists to take.
        let sale = envelope_at(902, Some("T1-000102"), 1_788_600_000_000);
        let response = push(&repo, &request(vec![sale]))
            .await
            .expect("it is stored");

        assert_eq!(response.accepted, vec![902]);
        assert!(response.quarantined.is_empty());
    }

    #[tokio::test]
    async fn a_device_enrolled_today_may_still_carry_yesterdays_sales() {
        let repo = MemoryRepo::new();
        // The shop has been trading for a year; this tablet was wiped and
        // enrolled again an hour ago, and is holding what it rang yesterday.
        repo.enrol_at(TENANT, 999, 1_700_000_000_000);
        repo.enrol_at(TENANT, TERMINAL, 1_788_700_000_000);

        let sale = envelope_at(903, Some("T1-000103"), 1_788_600_000_000);
        let response = push(&repo, &request(vec![sale]))
            .await
            .expect("it is stored");

        // The bound is the shop, not the device. Holding these would hold the
        // whole backlog of every tablet a shop ever replaces.
        assert_eq!(response.accepted, vec![903]);
        assert!(response.quarantined.is_empty());
    }

    #[tokio::test]
    async fn a_replay_after_a_dropped_connection_costs_nothing() {
        let repo = repo();
        let batch = request(vec![envelope(900, Some("T1-000100"))]);

        let first = push(&repo, &batch).await.unwrap();
        let second = push(&repo, &batch).await.unwrap();

        assert_eq!(first.accepted, vec![900]);
        assert_eq!(second.accepted, vec![900], "a replay is acknowledged");
        assert_eq!(repo.sale_count(TENANT), 1, "and stored exactly once");
    }

    #[tokio::test]
    async fn stores_and_flags_a_sale_whose_totals_do_not_add_up() {
        let repo = repo();
        let mut tampered = envelope(900, Some("T1-000100"));
        // Rewrite the stored total, as corruption or tampering would.
        let mut sale = wire::decode_sale(SALE_SCHEMA, &tampered.payload).unwrap();
        sale.ticket.total_minor = 1;
        tampered.payload = encode_sale(&sale).unwrap();

        let response = push(&repo, &request(vec![tampered])).await.unwrap();

        assert!(response.accepted.is_empty());
        assert_eq!(
            response.quarantined[0].reason,
            QuarantineReason::TotalsMismatch {
                stored_minor: 1,
                recomputed_minor: 49_450,
            }
        );
        assert_eq!(repo.sale_count(TENANT), 1, "a suspect sale is still kept");
        assert_eq!(repo.quarantined(TENANT).len(), 1);
    }

    #[tokio::test]
    async fn a_payload_that_cannot_be_rechecked_is_quarantined_rather_than_trusted() {
        let repo = repo();
        let mut tampered = envelope(900, Some("T1-000100"));
        let mut sale = wire::decode_sale(SALE_SCHEMA, &tampered.payload).unwrap();
        // A VAT rate above 100 percent: the recompute errors instead of
        // disagreeing. Reading that as agreement would let anyone bypass the
        // tamper check by breaking the arithmetic rather than the total.
        if let Some(line) = sale.ticket.lines.first_mut() {
            line.vat_bp = 99_999;
        }
        sale.ticket.total_minor = 1;
        tampered.payload = encode_sale(&sale).unwrap();

        let response = push(&repo, &request(vec![tampered])).await.unwrap();

        assert!(
            response.accepted.is_empty(),
            "an uncheckable sale is not clean"
        );
        assert_eq!(
            response.quarantined[0].reason,
            QuarantineReason::Undecodable
        );
        assert_eq!(repo.sale_count(TENANT), 1, "and it is still stored");
    }

    #[tokio::test]
    async fn two_pushes_racing_for_one_receipt_number_cannot_both_win() {
        let repo = repo();
        // A restored tablet pushing its backlog beside the device it was copied
        // from: the exact case the duplicate check exists for, and the one time
        // both pushes arrive at once.
        let first = envelope(900, Some("T1-000100"));
        let second = envelope(901, Some("T1-000100"));

        let one = request(vec![first]);
        let two = request(vec![second]);
        let (left, right) = tokio::join!(push(&repo, &one), push(&repo, &two));
        let (left, right) = (left.unwrap(), right.unwrap());

        let accepted = left.accepted.len() + right.accepted.len();
        let quarantined = left.quarantined.len() + right.quarantined.len();
        assert_eq!(accepted, 1, "exactly one sale may hold the number");
        assert_eq!(
            quarantined, 1,
            "and the other is a repair item, not a refusal"
        );
        assert_eq!(repo.sale_count(TENANT), 2, "both sales are still stored");
    }

    #[tokio::test]
    async fn the_ledger_follows_the_ticket_not_the_movements_it_was_sent() {
        let repo = repo();
        let mut tampered = envelope(900, Some("T1-000100"));
        let mut sale = wire::decode_sale(SALE_SCHEMA, &tampered.payload).unwrap();
        // A self-consistent ticket whose stock movements point somewhere else.
        // The totals recompute perfectly, so the tamper check has nothing to
        // say, and the ledger would have decremented an item never sold.
        sale.stock = vec![(999_u128, -50_000_i64)];
        tampered.payload = encode_sale(&sale).unwrap();

        let response = push(&repo, &request(vec![tampered])).await.unwrap();
        assert_eq!(response.accepted.len(), 1, "the sale itself is honest");

        let stored = repo.sales(TENANT);
        assert_eq!(
            stored[0].stock,
            vec![(1_u128, -1_000_i64)],
            "the movements are recomputed from the lines, not believed"
        );
    }

    #[tokio::test]
    async fn catches_a_receipt_number_used_twice() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        // A terminal restored from a backup re-issues the same number.
        let response = push(&repo, &request(vec![envelope(901, Some("T1-000100"))]))
            .await
            .unwrap();

        assert_eq!(
            response.quarantined[0].reason,
            QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000100".to_owned(),
            }
        );
        assert_eq!(
            repo.sale_count(TENANT),
            2,
            "both sales are kept for the repair queue"
        );
    }

    #[tokio::test]
    async fn keeps_bytes_it_cannot_read() {
        let repo = repo();
        let broken = SaleEnvelope {
            id: 900,
            schema: 99,
            payload: vec![1, 2, 3],
        };
        let response = push(&repo, &request(vec![broken])).await.unwrap();

        assert_eq!(
            response.quarantined[0].reason,
            QuarantineReason::Undecodable
        );
        assert_eq!(
            repo.sale(TENANT, 900).unwrap().payload,
            vec![1, 2, 3],
            "the bytes are evidence even when unreadable"
        );
    }

    #[tokio::test]
    async fn an_unnumbered_sale_is_accepted_as_it_is() {
        // A till that ran out of leased numbers still sells.
        let repo = repo();
        let response = push(&repo, &request(vec![envelope(900, None)]))
            .await
            .unwrap();

        assert_eq!(response.accepted, vec![900]);
        assert!(repo.sale(TENANT, 900).unwrap().receipt_no.is_none());
    }

    #[tokio::test]
    async fn refuses_a_terminal_it_has_never_enrolled() {
        let repo = MemoryRepo::new();
        let result = push(&repo, &request(vec![envelope(900, None)])).await;
        assert_eq!(
            result,
            Err(IngestError::Protocol(ProtocolError::UnknownTerminal))
        );
    }

    #[tokio::test]
    async fn refuses_a_protocol_it_does_not_speak() {
        let repo = repo();
        let mut batch = request(vec![envelope(900, None)]);
        batch.protocol = 99;

        assert!(matches!(
            push(&repo, &batch).await,
            Err(IngestError::Protocol(
                ProtocolError::UnsupportedVersion { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn a_till_may_drop_everything_the_server_settled() {
        let repo = repo();
        let mut tampered = envelope(901, Some("T1-000101"));
        let mut sale = wire::decode_sale(SALE_SCHEMA, &tampered.payload).unwrap();
        sale.ticket.total_minor = 7;
        tampered.payload = encode_sale(&sale).unwrap();

        let response = push(
            &repo,
            &request(vec![envelope(900, Some("T1-000100")), tampered]),
        )
        .await
        .unwrap();

        // Both are on the server now, one of them flagged, so the till is free
        // of both. Holding the flagged one would leave its only copy on a tablet.
        assert_eq!(response.settled(), vec![900, 901]);
    }

    /// A refund, exactly as a till would have committed one, reversing a
    /// receipt the caller names and for the amount the caller chooses.
    fn refund_envelope(id: u128, of_receipt: &str, minor: i64) -> SaleEnvelope {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(Some(of_receipt)).unwrap();
        // A refund's lines are the negative of the same goods, so the quantity
        // is chosen by what it comes to: one of this item is 494.50.
        let of_them = Milli::new(minor * 1_000 / 49_450);
        cart.add_item(&item(), of_them).unwrap();
        let due = cart.totals().unwrap().total;
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: due,
            reference: None,
        });
        let ticket = cart
            .close(
                Ulid::from_u128(id),
                Ulid::from_u128(TERMINAL),
                1_788_600_000_000,
            )
            .unwrap();
        let payload = encode_sale(&sale_commit(&ticket, Some(1), Some(200))).unwrap();
        SaleEnvelope {
            id,
            schema: SALE_SCHEMA,
            payload,
        }
    }

    /// A refund reversing a receipt this shop does not have.
    ///
    /// Ordinary when a till has not synced yet, and indistinguishable from a
    /// refund invented against no sale at all. Held for a person, not refused:
    /// the money went out of the drawer either way.
    #[tokio::test]
    async fn a_refund_against_a_receipt_nobody_has_is_held_for_somebody() {
        let repo = repo();
        let answer = push(
            &repo,
            &request(vec![refund_envelope(901, "T1-000100", 49_450)]),
        )
        .await
        .unwrap();

        assert_eq!(answer.accepted.len(), 0);
        assert_eq!(answer.quarantined.len(), 1);
        assert_eq!(
            answer.quarantined[0].reason,
            QuarantineReason::RefundAgainstNothing {
                receipt_no: "T1-000100".to_owned()
            }
        );
        let held = repo.sales(TENANT);
        assert_eq!(held.len(), 1, "and it is stored: the money left the drawer");
        assert_eq!(held[0].total_minor, -49_450);
        assert_eq!(held[0].refund_of.as_deref(), Some("T1-000100"));
    }

    /// The same receipt refunded twice.
    ///
    /// The oldest trick at a counter, and also what a customer bringing half a
    /// basket back twice looks like when the first refund was rung for all of
    /// it. Only somebody who was there can tell those apart.
    #[tokio::test]
    async fn a_receipt_refunded_past_what_it_was_rung_for_is_held() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        // The whole of it, which is exactly what it was rung for.
        let first = push(
            &repo,
            &request(vec![refund_envelope(901, "T1-000100", 49_450)]),
        )
        .await
        .unwrap();
        assert_eq!(first.accepted.len(), 1, "a refund of the whole sale stands");
        assert!(first.quarantined.is_empty());

        // And again.
        let again = push(
            &repo,
            &request(vec![refund_envelope(902, "T1-000100", 49_450)]),
        )
        .await
        .unwrap();
        assert_eq!(again.quarantined.len(), 1);
        assert_eq!(
            again.quarantined[0].reason,
            QuarantineReason::RefundBeyondTheSale {
                receipt_no: "T1-000100".to_owned(),
                sale_minor: 49_450,
                refunded_minor: 98_900,
            },
            "the shop is told what it was rung for and what has been given back"
        );
    }

    /// Part of a basket, twice, which is an ordinary week.
    #[tokio::test]
    async fn two_partial_refunds_inside_the_sale_are_both_taken() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        for (id, minor) in [(901_u128, 24_725_i64), (902, 24_725)] {
            let answer = push(
                &repo,
                &request(vec![refund_envelope(id, "T1-000100", minor)]),
            )
            .await
            .unwrap();
            assert_eq!(answer.accepted.len(), 1, "{id} is inside the sale");
            assert!(answer.quarantined.is_empty(), "{id}: {answer:?}");
        }
    }

    /// A refund of something that receipt never sold.
    ///
    /// The money can be right and the goods wrong: the same taka back, made of
    /// a different thing. What that does is put stock on the shelf that never
    /// left it, which is how a count is made to agree with a shelf somebody
    /// emptied.
    #[tokio::test]
    async fn goods_that_never_went_out_cannot_come_back_unremarked() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        // A refund for the same money, of a different item.
        let mut other = item();
        other.id = Ulid::from_u128(2);
        other.barcodes = vec!["8690000000029".into()];
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(Some("T1-000100")).unwrap();
        cart.add_item(&other, Milli::ONE).unwrap();
        let due = cart.totals().unwrap().total;
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: due,
            reference: None,
        });
        let ticket = cart
            .close(
                Ulid::from_u128(901),
                Ulid::from_u128(TERMINAL),
                1_788_600_000_000,
            )
            .unwrap();
        let payload = encode_sale(&sale_commit(&ticket, Some(1), Some(200))).unwrap();

        let answer = push(
            &repo,
            &request(vec![SaleEnvelope {
                id: 901,
                schema: SALE_SCHEMA,
                payload,
            }]),
        )
        .await
        .unwrap();

        assert_eq!(answer.quarantined.len(), 1, "{answer:?}");
        assert_eq!(
            answer.quarantined[0].reason,
            QuarantineReason::MoreCameBackThanWentOut {
                receipt_no: "T1-000100".to_owned(),
                item_id: 2,
                over_by_milli: 1_000,
            },
            "the money was right to the poisha and the goods were not"
        );
    }

    /// Twice the goods for the same money.
    ///
    /// The sharper version of the same trick: the receipt is refunded for
    /// exactly what it was rung for, and two of the item come back where one
    /// went out, at half the price each. Nothing about the money is wrong.
    #[tokio::test]
    async fn twice_the_goods_for_the_same_money_is_held() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.start_refund(Some("T1-000100")).unwrap();
        cart.add_item(&item(), Milli::new(2_000)).unwrap();
        // Half the price each, so the taka come back to exactly the sale.
        cart.set_unit_price(0, Minor::new(21_500)).unwrap();
        let due = cart.totals().unwrap().total;
        assert_eq!(due, Minor::new(-49_450), "the same money, to the poisha");
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: due,
            reference: None,
        });
        let ticket = cart
            .close(
                Ulid::from_u128(901),
                Ulid::from_u128(TERMINAL),
                1_788_600_000_000,
            )
            .unwrap();
        let payload = encode_sale(&sale_commit(&ticket, Some(1), Some(200))).unwrap();

        let answer = push(
            &repo,
            &request(vec![SaleEnvelope {
                id: 901,
                schema: SALE_SCHEMA,
                payload,
            }]),
        )
        .await
        .unwrap();

        assert_eq!(answer.quarantined.len(), 1, "{answer:?}");
        assert_eq!(
            answer.quarantined[0].reason,
            QuarantineReason::MoreCameBackThanWentOut {
                receipt_no: "T1-000100".to_owned(),
                item_id: 1,
                over_by_milli: 1_000,
            },
            "one went out and two came back"
        );
    }

    /// And the ordinary case still goes straight through.
    #[tokio::test]
    async fn a_refund_of_what_was_bought_is_taken_without_a_word() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();
        let answer = push(
            &repo,
            &request(vec![refund_envelope(901, "T1-000100", 49_450)]),
        )
        .await
        .unwrap();
        assert_eq!(answer.accepted.len(), 1);
        assert!(answer.quarantined.is_empty(), "{answer:?}");
    }

    #[tokio::test]
    async fn a_pushed_sale_declares_what_the_server_recomputed() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        // What a shop declares to the revenue comes from the same crate that
        // priced the sale, not from anything the payload asserted about tax.
        let declared = repo.vat_summary(TENANT, 0, u64::MAX).await.unwrap();
        assert_eq!(declared.rows.len(), 1);
        assert_eq!(declared.rows[0].vat_bp, 1_500);
        assert_eq!(declared.rows[0].net_minor, 43_000);
        assert_eq!(declared.rows[0].vat_minor, 6_450);
        assert_eq!(declared.rows[0].sales, 1);
        assert_eq!(declared.waiting_sales, 0, "nothing is waiting on anybody");
    }
}
