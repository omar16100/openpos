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

    let mut accepted = Vec::with_capacity(request.sales.len());
    let mut quarantined = Vec::new();

    for envelope in &request.sales {
        // Assessment, storage, the idempotency check and the receipt claim are
        // one round trip and one transaction. It used to be three, and the
        // window between the second and the third is where two pushes carrying
        // the same receipt number both read "free" and both stored clean.
        let (sale, suspicion) = match assess(request, envelope) {
            Assessment::Clean(sale) => (sale, None),
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
                stock: Vec::new(),
                // Nothing can be read out of bytes nobody can decode, including
                // who owes for them. It is in the repair queue for a person to
                // look at, which is the only thing left to do with it.
                vat: Vec::new(),
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
        stock: stock_from_lines(sale),
        // Recomputed with the same crate the till used, like the totals check
        // above: what a shop declares to the revenue must not be something a
        // payload could assert. A ticket that cannot be recomputed declares
        // nothing and is in the queue for a person instead.
        vat: vat_from_lines(sale),
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
        repo.enrol(TENANT, TERMINAL);
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

    #[tokio::test]
    async fn a_pushed_sale_declares_what_the_server_recomputed() {
        let repo = repo();
        push(&repo, &request(vec![envelope(900, Some("T1-000100"))]))
            .await
            .unwrap();

        // What a shop declares to the revenue comes from the same crate that
        // priced the sale, not from anything the payload asserted about tax.
        let declared = repo.vat_summary(TENANT, 0, u64::MAX).await.unwrap();
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].vat_bp, 1_500);
        assert_eq!(declared[0].net_minor, 43_000);
        assert_eq!(declared[0].vat_minor, 6_450);
        assert_eq!(declared[0].sales, 1);
    }
}
