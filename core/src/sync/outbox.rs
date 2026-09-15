//! What this terminal still owes the server.
//!
//! The outbox is derived, not stored. It is the critical log read back and
//! filtered by what has been acknowledged, so there is no second structure that
//! can fall out of step with the ledger. A separate queue would introduce a
//! window between committing a sale and enqueueing it, and a crash inside that
//! window either loses the sale from sync or duplicates it.
//!
//! Acknowledgement is recorded as a watermark appended to the same log, not by
//! deleting the sales it covers. Deleting from the front of a log means
//! rewriting it, and a crash during that rewrite would take the unacknowledged
//! tail with it: the shop would lose exactly the sales the server had never
//! seen. Appending a watermark risks nothing, and the covered sales are dropped
//! later in one truncation, once nothing is outstanding.
//!
//! Only a contiguous run from the oldest counts. If the server confirms sales one
//! and three but not two, the watermark stops at one: sale three is simply sent
//! again, and the server discards it as a replay because every sale carries a
//! ULID minted on the device. At-least-once delivery with idempotent receipt is
//! the cheap, correct choice here; exactly-once needs a protocol nobody at this
//! scale needs.

use alloc::vec::Vec;

use crate::ids::Ulid;
use crate::storage::backend::Backend;
use crate::storage::frame::{PayloadKind, Store};
use crate::storage::journal::Journal;
use crate::storage::wire::{self, SaleCommitV1, SyncAckV1, ACK_SCHEMA};

use super::{Result, SyncError};

/// One sale waiting to reach the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSale {
    /// Journal sequence, used to compute how much of the log may be dropped.
    pub sequence: u64,
    /// The sale's own identity, and the key the server deduplicates on.
    pub id: Ulid,
    /// Bytes as committed, forwarded verbatim. Re-encoding risks sending
    /// something subtly different from what is on disk and on the receipt.
    pub payload: Vec<u8>,
    /// The schema those bytes were written under, which is not always the one
    /// this build writes.
    ///
    /// A till that was offline when it was upgraded is holding sales in the
    /// shape the older build wrote. Sending them stamped with today's number
    /// tells the shop to read them as something they are not: postcard is
    /// positional, so the shop cannot decode them, holds the bytes as a repair
    /// nobody can read, and the till drops them as sent. The goods, the tax and
    /// anybody's account go with them. This is the number that was on the frame.
    pub schema: u16,
    /// Total charged, so a caller can show a value without decoding everything.
    pub total_minor: i64,
}

/// Reads pending work out of the journal.
pub struct Outbox;

/// What an acknowledgement covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acknowledged {
    /// Sales the server has confirmed, counted from the oldest.
    pub confirmed: usize,
    /// True when nothing in the log is outstanding any more, so it may be
    /// emptied once the terminal's standing state is safely written elsewhere.
    pub drained: bool,
}

impl Outbox {
    /// The sequence the server has confirmed through, read from the log itself.
    ///
    /// Public because the log is no longer emptied the moment everything is
    /// acknowledged: an open drawer holds the front of it down, so anything that
    /// counts sales has to know which of them the server already owns.
    pub fn watermark<B: Backend>(journal: &Journal<B>) -> Result<u64> {
        let mut watermark = 0_u64;
        for record in journal.read(Store::Critical)? {
            if record.header.kind == PayloadKind::SyncAck {
                let ack: SyncAckV1 = wire::decode_ack(record.header.schema, &record.payload)
                    .map_err(SyncError::Wire)?;
                watermark = watermark.max(ack.through_sequence);
            }
        }
        Ok(watermark)
    }

    /// Sales the server has not confirmed, oldest first.
    pub fn pending<B: Backend>(journal: &Journal<B>) -> Result<Vec<PendingSale>> {
        let records = journal.read(Store::Critical)?;

        // The watermark lives in the same log, so it is recovered by the same
        // read and cannot disagree with the sales it describes.
        let mut watermark = 0_u64;
        for record in &records {
            if record.header.kind == PayloadKind::SyncAck {
                let ack: SyncAckV1 = wire::decode_ack(record.header.schema, &record.payload)
                    .map_err(SyncError::Wire)?;
                watermark = watermark.max(ack.through_sequence);
            }
        }

        let mut pending = Vec::new();
        for record in records {
            if record.header.kind != PayloadKind::SaleCommit
                || record.header.sequence <= watermark
            {
                continue;
            }
            let sale: SaleCommitV1 = wire::decode_sale(record.header.schema, &record.payload)
                .map_err(SyncError::Wire)?;
            pending.push(PendingSale {
                sequence: record.header.sequence,
                id: Ulid::from_u128(sale.ticket.id),
                schema: record.header.schema,
                payload: record.payload,
                total_minor: sale.ticket.total_minor,
            });
        }
        Ok(pending)
    }

        /// The next batch to push, oldest first.
    ///
    /// Oldest first on purpose: a shop that has been offline all day wants its
    /// morning takings recorded before its afternoon ones, and a server that
    /// falls behind should still see a coherent prefix of the day.
    pub fn batch<B: Backend>(journal: &Journal<B>, limit: usize) -> Result<Vec<PendingSale>> {
        let mut pending = Self::pending(journal)?;
        pending.truncate(limit);
        Ok(pending)
    }

    /// Record what the server has confirmed.
    ///
    /// Returns how many sales that covered. Only a contiguous run from the oldest
    /// counts: a gap means an older sale is still unconfirmed, and a watermark
    /// past it would hide evidence the server has never seen.
    ///
    /// Reports whether nothing is left outstanding, but does not empty the log
    /// itself. Emptying it is the caller's decision because the caller is the
    /// one holding the standing state that has to be written down first.
    pub fn acknowledge<B: Backend>(
        journal: &mut Journal<B>,
        acknowledged: &[Ulid],
    ) -> Result<Acknowledged> {
        let pending = Self::pending(journal)?;
        if pending.is_empty() {
            return Ok(Acknowledged {
                confirmed: 0,
                drained: false,
            });
        }

        let mut confirmed = 0_usize;
        let mut through = 0_u64;
        for sale in &pending {
            if acknowledged.contains(&sale.id) {
                confirmed = confirmed.saturating_add(1);
                through = sale.sequence;
            } else {
                break;
            }
        }
        if confirmed == 0 {
            return Ok(Acknowledged {
                confirmed: 0,
                drained: false,
            });
        }

        let payload = wire::encode_ack(&SyncAckV1 {
            through_sequence: through,
        })
        .map_err(SyncError::Wire)?;
        journal.commit(Store::Critical, PayloadKind::SyncAck, ACK_SCHEMA, &payload)?;

        Ok(Acknowledged {
            confirmed,
            drained: confirmed == pending.len(),
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
    use crate::cart::{Cart, CartLimits, Tender, TenderKind};
    use crate::domain::PriceMode;
    use crate::money::{Bp, Milli, Minor};
    use crate::replica::Item;
    use crate::storage::backend::MemoryBackend;
    use crate::storage::wire::{encode_sale, sale_commit, SALE_SCHEMA};

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
            vat_base: crate::domain::VatBase::Discounted,
            barcodes: vec!["8690000000012".into()],
            on_hand: Milli::new(40_000),
            active: true,
            supply: crate::domain::Supply::Standard,
            category: "".into(),
        }
    }

    /// Ring and commit one sale, returning its id.
    fn ring(journal: &mut Journal<MemoryBackend>, ticket_id: u128) -> Ulid {
        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(&item(), Milli::ONE).unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(50_000),
            reference: None,
        });
        let ticket = cart
            .close(Ulid::from_u128(ticket_id), Ulid::from_u128(TERMINAL), 1_788_600_000_000)
            .unwrap();
        let payload = encode_sale(&sale_commit(&ticket, Some(1), Some(100))).unwrap();
        journal
            .commit(Store::Critical, PayloadKind::SaleCommit, SALE_SCHEMA, &payload)
            .unwrap();
        ticket.id
    }

    fn open(backend: MemoryBackend) -> Journal<MemoryBackend> {
        Journal::open(backend, TENANT, TERMINAL, 1).unwrap().0
    }

    #[test]
    fn pending_is_derived_from_the_ledger() {
        let mut journal = open(MemoryBackend::new());
        let first = ring(&mut journal, 1);
        let second = ring(&mut journal, 2);

        let pending = Outbox::pending(&journal).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].id, first);
        assert_eq!(pending[1].id, second);
        assert_eq!(pending[0].total_minor, 49_450);
    }

    /// A sale waits in the shape it was written in, and goes in that shape.
    ///
    /// A till offline when it is upgraded is holding sales the older build
    /// wrote. Sending them stamped with today's number tells the shop to read
    /// them as something they are not: postcard is positional, so the shop
    /// cannot decode them, keeps the bytes as a repair nobody can read, and the
    /// till drops them as sent. The goods, the tax and anybody's account go
    /// with them, and the only copy was on the device.
    ///
    /// The shop knows every schema this build's ancestors wrote. All it needs
    /// is to be told which one, which is the number on the frame.
    #[test]
    fn a_sale_written_by_an_older_build_is_sent_as_what_it_is() {
        use crate::storage::wire::SALE_SCHEMA_V3;

        let mut journal = open(MemoryBackend::new());
        // A real sale as version 3 wrote it, which is the fixture
        // `bytes_from_before.rs` keeps: one line of rice, paid in cash, from
        // before what the shop paid travelled with a line.
        const AS_THREE_WROTE_IT: &str = "86070780bcf8868734010954312d30303031303601010001010552494345351052696365204d696e696b657420356b67f09f05d00f00dc0b0000034e6f7300000100d4840600f09f05e46400d484060000016b01010101cf0f00";
        let payload: alloc::vec::Vec<u8> = (0..AS_THREE_WROTE_IT.len())
            .step_by(2)
            .filter_map(|at| u8::from_str_radix(AS_THREE_WROTE_IT.get(at..at + 2)?, 16).ok())
            .collect();
        journal
            .commit(Store::Critical, PayloadKind::SaleCommit, SALE_SCHEMA_V3, &payload)
            .unwrap();

        let pending = Outbox::pending(&journal).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].schema, SALE_SCHEMA_V3,
            "the number on the frame, not the number this build writes"
        );
        assert_eq!(
            crate::sync::envelope_for(&pending[0]).schema,
            SALE_SCHEMA_V3,
            "and that is what the shop is told to read them as"
        );
    }

    #[test]
    fn a_batch_takes_the_oldest_first() {
        let mut journal = open(MemoryBackend::new());
        let first = ring(&mut journal, 1);
        ring(&mut journal, 2);
        ring(&mut journal, 3);

        let batch = Outbox::batch(&journal, 2).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].id, first, "the morning's takings go first");
    }

    #[test]
    fn acknowledging_a_prefix_drops_it() {
        let mut journal = open(MemoryBackend::new());
        let first = ring(&mut journal, 1);
        let second = ring(&mut journal, 2);
        let third = ring(&mut journal, 3);

        let dropped = Outbox::acknowledge(&mut journal, &[first, second]).unwrap();
        assert_eq!(dropped.confirmed, 2);

        let pending = Outbox::pending(&journal).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, third);
    }

    #[test]
    fn a_gap_stops_the_truncation() {
        let mut journal = open(MemoryBackend::new());
        let first = ring(&mut journal, 1);
        let _second = ring(&mut journal, 2);
        let third = ring(&mut journal, 3);

        // The server confirmed the first and third but not the second.
        let dropped = Outbox::acknowledge(&mut journal, &[first, third]).unwrap();
        assert_eq!(dropped.confirmed, 1, "only the leading run may be dropped");
        assert!(!dropped.drained, "an older sale is still unconfirmed");

        let pending = Outbox::pending(&journal).unwrap();
        assert_eq!(pending.len(), 2, "the unconfirmed sale and everything after it stay");
        // The third is simply sent again; the server discards it by ULID.
        assert_eq!(pending[1].id, third);
    }

    #[test]
    fn acknowledging_nothing_changes_nothing() {
        let mut journal = open(MemoryBackend::new());
        ring(&mut journal, 1);
        assert_eq!(Outbox::acknowledge(&mut journal, &[]).unwrap().confirmed, 0);
        assert_eq!(
            Outbox::acknowledge(&mut journal, &[Ulid::from_u128(99)])
                .unwrap()
                .confirmed,
            0
        );
        assert_eq!(Outbox::pending(&journal).unwrap().len(), 1);
    }

    #[test]
    fn the_remaining_log_survives_a_reboot_after_truncation() {
        let mut journal = open(MemoryBackend::new());
        let first = ring(&mut journal, 1);
        let second = ring(&mut journal, 2);
        Outbox::acknowledge(&mut journal, &[first]).unwrap();

        let reopened = open(journal.backend().clone());
        let pending = Outbox::pending(&reopened).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, second);
    }
}
