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

use openpos_core::domain::{ticket_totals, TicketInput};
use openpos_core::protocol::{
    negotiate, ProtocolError, PushRequest, PushResponse, QuarantineReason, Quarantined,
    SaleEnvelope,
};
use openpos_core::storage::wire::{self, SaleCommitV1};

use crate::repo::{Repository, StoredSale};

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
pub fn push(repo: &mut impl Repository, request: &PushRequest) -> Result<PushResponse> {
    let protocol = negotiate(request.protocol)?;

    if !repo
        .terminal_enrolled(request.tenant, request.terminal)
        .map_err(|_| IngestError::Storage)?
    {
        return Err(IngestError::Protocol(ProtocolError::UnknownTerminal));
    }

    let mut accepted = Vec::with_capacity(request.sales.len());
    let mut quarantined = Vec::new();

    for envelope in &request.sales {
        // A replay of something already stored. Acknowledge and move on: doing
        // the work twice would double the stock movement.
        if repo
            .has_sale(request.tenant, envelope.id)
            .map_err(|_| IngestError::Storage)?
        {
            accepted.push(envelope.id);
            continue;
        }

        match assess(repo, request, envelope)? {
            Assessment::Clean(sale) => {
                repo.store_sale(sale).map_err(|_| IngestError::Storage)?;
                accepted.push(envelope.id);
            }
            Assessment::Suspect(sale, reason) => {
                repo.store_sale(sale).map_err(|_| IngestError::Storage)?;
                quarantined.push(Quarantined {
                    id: envelope.id,
                    reason,
                });
            }
        }
    }

    Ok(PushResponse {
        protocol,
        accepted,
        quarantined,
    })
}

enum Assessment {
    Clean(StoredSale),
    Suspect(StoredSale, QuarantineReason),
}

fn assess(
    repo: &impl Repository,
    request: &PushRequest,
    envelope: &SaleEnvelope,
) -> Result<Assessment> {
    let Ok(sale) = wire::decode_sale(envelope.schema, &envelope.payload) else {
        // Undecodable, so nothing can be said about it. Store the bytes anyway:
        // they are evidence, and a later build may know how to read them.
        return Ok(Assessment::Suspect(
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
            },
            QuarantineReason::Undecodable,
        ));
    };

    let stored = build(request, envelope, &sale, None);

    if let Some(reason) = totals_disagree(&sale) {
        return Ok(Assessment::Suspect(
            build(request, envelope, &sale, Some(reason.clone())),
            reason,
        ));
    }

    if let (Some(receipt), Some(epoch)) = (&sale.ticket.receipt_no, sale.ticket.receipt_epoch)
        && repo
            .receipt_taken(request.tenant, receipt, epoch)
            .map_err(|_| IngestError::Storage)?
    {
        let reason = QuarantineReason::DuplicateReceiptNumber {
            receipt_no: receipt.clone(),
        };
        return Ok(Assessment::Suspect(
            build(request, envelope, &sale, Some(reason.clone())),
            reason,
        ));
    }

    Ok(Assessment::Clean(stored))
}

/// Recompute the totals and compare them with what the till stored.
///
/// Both sides run `openpos_core::domain`, so this can only differ if the bytes
/// were altered after the till wrote them. That makes it a strong signal rather
/// than a tolerance check, and it is why no epsilon appears here.
fn totals_disagree(sale: &SaleCommitV1) -> Option<QuarantineReason> {
    let ticket = sale.ticket.clone();
    let stored_minor = ticket.total_minor;
    let discount = ticket.ticket_discount.clone().into_domain().ok()?;
    let (lines, _tenders) = ticket.lines_and_tenders().ok()?;

    let recomputed = ticket_totals(&TicketInput {
        lines: lines.iter().map(openpos_core::cart::CartLine::as_input).collect(),
        ticket_discount: discount,
    })
    .ok()?;

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
        stock: sale.stock.clone(),
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

    use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
    use openpos_core::domain::PriceMode;
    use openpos_core::ids::Ulid;
    use openpos_core::money::{Bp, Milli, Minor};
    use openpos_core::protocol::PROTOCOL_VERSION;
    use openpos_core::replica::Item;
    use openpos_core::storage::wire::{encode_sale, sale_commit, SALE_SCHEMA};

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
            .close(Ulid::from_u128(id), Ulid::from_u128(TERMINAL), 1_788_600_000_000)
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
        let mut repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo
    }

    #[test]
    fn accepts_a_clean_batch() {
        let mut repo = repo();
        let response = push(&mut repo, &request(vec![
            envelope(900, Some("T1-000100")),
            envelope(901, Some("T1-000101")),
        ]))
        .unwrap();

        assert_eq!(response.accepted, vec![900, 901]);
        assert!(response.quarantined.is_empty());
        assert_eq!(repo.sale_count(TENANT), 2);
        assert_eq!(repo.sale(TENANT, 900).unwrap().total_minor, 49_450);
    }

    #[test]
    fn a_replay_after_a_dropped_connection_costs_nothing() {
        let mut repo = repo();
        let batch = request(vec![envelope(900, Some("T1-000100"))]);

        let first = push(&mut repo, &batch).unwrap();
        let second = push(&mut repo, &batch).unwrap();

        assert_eq!(first.accepted, vec![900]);
        assert_eq!(second.accepted, vec![900], "a replay is acknowledged");
        assert_eq!(repo.sale_count(TENANT), 1, "and stored exactly once");
    }

    #[test]
    fn stores_and_flags_a_sale_whose_totals_do_not_add_up() {
        let mut repo = repo();
        let mut tampered = envelope(900, Some("T1-000100"));
        // Rewrite the stored total, as corruption or tampering would.
        let mut sale = wire::decode_sale(SALE_SCHEMA, &tampered.payload).unwrap();
        sale.ticket.total_minor = 1;
        tampered.payload = encode_sale(&sale).unwrap();

        let response = push(&mut repo, &request(vec![tampered])).unwrap();

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

    #[test]
    fn catches_a_receipt_number_used_twice() {
        let mut repo = repo();
        push(&mut repo, &request(vec![envelope(900, Some("T1-000100"))])).unwrap();

        // A terminal restored from a backup re-issues the same number.
        let response = push(&mut repo, &request(vec![envelope(901, Some("T1-000100"))])).unwrap();

        assert_eq!(
            response.quarantined[0].reason,
            QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000100".to_owned(),
            }
        );
        assert_eq!(repo.sale_count(TENANT), 2, "both sales are kept for the repair queue");
    }

    #[test]
    fn keeps_bytes_it_cannot_read() {
        let mut repo = repo();
        let broken = SaleEnvelope {
            id: 900,
            schema: 99,
            payload: vec![1, 2, 3],
        };
        let response = push(&mut repo, &request(vec![broken])).unwrap();

        assert_eq!(response.quarantined[0].reason, QuarantineReason::Undecodable);
        assert_eq!(
            repo.sale(TENANT, 900).unwrap().payload,
            vec![1, 2, 3],
            "the bytes are evidence even when unreadable"
        );
    }

    #[test]
    fn an_unnumbered_sale_is_accepted_as_it_is() {
        // A till that ran out of leased numbers still sells.
        let mut repo = repo();
        let response = push(&mut repo, &request(vec![envelope(900, None)])).unwrap();

        assert_eq!(response.accepted, vec![900]);
        assert!(repo.sale(TENANT, 900).unwrap().receipt_no.is_none());
    }

    #[test]
    fn refuses_a_terminal_it_has_never_enrolled() {
        let mut repo = MemoryRepo::new();
        let result = push(&mut repo, &request(vec![envelope(900, None)]));
        assert_eq!(
            result,
            Err(IngestError::Protocol(ProtocolError::UnknownTerminal))
        );
    }

    #[test]
    fn refuses_a_protocol_it_does_not_speak() {
        let mut repo = repo();
        let mut batch = request(vec![envelope(900, None)]);
        batch.protocol = 99;

        assert!(matches!(
            push(&mut repo, &batch),
            Err(IngestError::Protocol(ProtocolError::UnsupportedVersion { .. }))
        ));
    }

    #[test]
    fn a_till_may_drop_everything_the_server_settled() {
        let mut repo = repo();
        let mut tampered = envelope(901, Some("T1-000101"));
        let mut sale = wire::decode_sale(SALE_SCHEMA, &tampered.payload).unwrap();
        sale.ticket.total_minor = 7;
        tampered.payload = encode_sale(&sale).unwrap();

        let response = push(
            &mut repo,
            &request(vec![envelope(900, Some("T1-000100")), tampered]),
        )
        .unwrap();

        // Both are on the server now, one of them flagged, so the till is free
        // of both. Holding the flagged one would leave its only copy on a tablet.
        assert_eq!(response.settled(), vec![900, 901]);
    }
}
