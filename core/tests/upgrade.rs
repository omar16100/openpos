//! The morning after an upgrade, on a device that has been trading for months.
//!
//! Three schemas were bumped in two days: the standing state, the snapshot and
//! the sale. Each has its own test, and each passes. None of them describes what
//! a real device holds when it is upgraded, which is all three at once, written
//! by builds that predate different fields.
//!
//! That combination is where this kind of bug actually lands. A shop does not
//! upgrade one blob; it opens the till on Sunday morning and finds out whether
//! the money it took on Saturday is still there.
//!
//! What this insists on, in the order a shopkeeper would care:
//!
//! 1. **The sale it has not sent yet is still there and still sendable.** It is
//!    the only copy of goods that left the shop.
//! 2. **The receipt numbers it owns are still its own.** Losing the lease means
//!    a device that either stops selling or issues numbers another till has
//!    already printed.
//! 3. **The credential still works.** Losing it means a device that looks
//!    enrolled and syncs nothing until somebody notices.
//! 4. **The till opens at all**, even where a cache cannot be read.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::cart::CartLimits;
use openpos_core::ids::Ulid;
use openpos_core::storage::backend::{Backend, Blob, MemoryBackend};
use openpos_core::storage::frame::{self, FrameHeader, PayloadKind, Store};
use openpos_core::storage::wire::{
    self, ClosedShiftV3Legacy, DiscountV1, HeldTicketsV1, LeaseGrantV1, LineV1Legacy,
    SaleCommitV1Legacy, ShopV1Legacy, TerminalStateV1Legacy, TerminalStateV3Legacy, TicketV1Legacy,
    SALE_SCHEMA_V1, TERMINAL_SCHEMA_V1, TERMINAL_SCHEMA_V3,
};
use openpos_core::till::Till;

const TENANT: u128 = 42;
const TERMINAL: u128 = 7;

fn frame_of(kind: PayloadKind, schema: u16, sequence: u64, payload: &[u8]) -> Vec<u8> {
    let header = FrameHeader {
        store: Store::Critical,
        kind,
        schema,
        producer: 1,
        tenant: TENANT,
        terminal: TERMINAL,
        sequence,
    };
    let mut bytes = Vec::new();
    frame::encode(&header, payload, &mut bytes).unwrap();
    bytes
}

/// Everything a trading device holds, all of it written by older builds.
fn a_device_from_before() -> MemoryBackend {
    let mut backend = MemoryBackend::new();

    // The standing state: five hundred receipt numbers with four spent, the
    // credential it syncs with, and the shop that heads its receipts.
    let standing = TerminalStateV1Legacy {
        leases: vec![LeaseGrantV1 {
            terminal: TERMINAL,
            epoch: 1,
            prefix: "T1".to_owned(),
            first: 100,
            last: 599,
        }],
        held: HeldTicketsV1::default(),
        unnumbered: 0,
        operators: vec![],
        token: Some("a-credential".to_owned()),
        shop: Some(ShopV1Legacy {
            name: "Karim General Store".to_owned(),
            bin: Some("001234567-0101".to_owned()),
            address: None,
            phone: None,
        }),
    };
    backend
        .write_blob(
            Blob::TerminalA,
            &frame_of(
                PayloadKind::TerminalState,
                TERMINAL_SCHEMA_V1,
                1,
                &postcard::to_allocvec(&standing).unwrap(),
            ),
        )
        .unwrap();

    // A catalogue snapshot this build cannot read, because the snapshot format
    // moved on and nothing was kept to read the old one. It is a cache, so the
    // till must open anyway and fetch the catalogue again.
    backend
        .write_blob(
            Blob::SnapshotA,
            &frame_of(PayloadKind::Snapshot, 1, 2, b"whatever version one wrote"),
        )
        .unwrap();

    // Saturday's last sale, rung before the shop could say what it sold a thing
    // by, and never sent: the shop was shut before the network came back.
    let sale = SaleCommitV1Legacy {
        ticket: TicketV1Legacy {
            id: 900,
            terminal: TERMINAL,
            rung_at_ms: 1_788_600_000_000,
            receipt_no: Some("T1-000104".to_owned()),
            receipt_epoch: Some(1),
            customer: None,
            lines: vec![LineV1Legacy {
                item_id: 1,
                code: "RICE5".to_owned(),
                name: "Rice Miniket 5kg".to_owned(),
                unit_price_minor: 43_000,
                qty_milli: 1_000,
                discount: DiscountV1::None,
                vat_bp: 1_500,
                price_inclusive: false,
                vat_on_undiscounted: false,
            }],
            ticket_discount: DiscountV1::None,
            tenders: vec![],
            net_minor: 43_000,
            vat_minor: 6_450,
            discount_minor: 0,
            total_minor: 49_450,
            change_minor: 0,
            overrides: vec![],
        },
        lease_next: Some(105),
        lease_epoch: Some(1),
        stock: vec![(1, -1_000)],
        refund_of: None,
    };
    backend
        .append_log(
            Store::Critical,
            &frame_of(
                PayloadKind::SaleCommit,
                SALE_SCHEMA_V1,
                3,
                &postcard::to_allocvec(&sale).unwrap(),
            ),
        )
        .unwrap();
    backend.flush().unwrap();
    backend
}

/// A drawer counted on Saturday by a build that did not record who counted it.
#[test]
fn a_drawer_counted_before_the_till_named_the_counter_still_reaches_the_shop() {
    let mut backend = MemoryBackend::new();
    let standing = TerminalStateV3Legacy {
        unsent_shifts: vec![ClosedShiftV3Legacy {
            id: 800,
            opened_at_ms: 1_788_500_000_000,
            closed_at_ms: 1_788_600_000_000,
            opening_float_minor: 50_000,
            sales: 1,
            cash_sales_minor: 49_450,
            non_cash_sales_minor: 0,
            cash_in_minor: 0,
            cash_out_minor: 0,
            expected_cash_minor: 99_450,
            counted_cash_minor: 95_450,
            variance_minor: -4_000,
        }],
        ..TerminalStateV3Legacy::default()
    };
    backend
        .write_blob(
            Blob::TerminalA,
            &frame_of(
                PayloadKind::TerminalState,
                TERMINAL_SCHEMA_V3,
                1,
                &postcard::to_allocvec(&standing).unwrap(),
            ),
        )
        .unwrap();
    backend.flush().unwrap();

    let (till, _) = Till::open(
        backend,
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .expect("a till whose last drawer predates this field still has to open");

    // The count itself is the accountability record, and it is not thrown away
    // for want of a name the old build never wrote down.
    let waiting = till.unsent_shifts();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].variance_minor, -4_000);
    // The shop will be told this drawer was four hundred taka short, and cannot
    // be told by whom. That is the truth about it, and better than a guess.
    assert_eq!(waiting[0].closed_by, 0);
    assert!(waiting[0].closed_by_name.is_empty());
}

#[test]
fn a_device_written_by_older_builds_opens_and_keeps_what_matters() {
    let (till, report) = Till::open(
        a_device_from_before(),
        TENANT,
        Ulid::from_u128(TERMINAL),
        1,
        CartLimits::unrestricted(),
    )
    .expect("a shop's till has to open on Monday morning");

    // Saturday's takings. Goods left the shop and money changed hands, and this
    // is the only record of it anywhere.
    assert_eq!(report.unsynced_sales, 1);
    let waiting = till.pending_sales(10).unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].total_minor, 49_450);

    // And it is still sendable: the outbox decodes it under the schema it was
    // written with, so it can go to the server as it stands.
    let payload = wire::decode_sale(SALE_SCHEMA_V1, &waiting[0].payload).unwrap();
    assert_eq!(payload.ticket.receipt_no.as_deref(), Some("T1-000104"));
    // A line rung before units existed. Every one of them meant pieces.
    assert_eq!(payload.ticket.lines[0].unit, "Nos");

    // The numbers this terminal owns, resumed where it actually stopped rather
    // than where it last synced. Losing these is a till that stops selling, or
    // one that prints a number another till has already handed a customer.
    let status = till.status().unwrap();
    assert_eq!(status.receipt_numbers_left, 495);

    // The credential, and the shop that heads its paper.
    assert_eq!(till.token(), Some("a-credential"));
    assert_eq!(
        till.shop().map(|shop| shop.name.as_str()),
        Some("Karim General Store")
    );
    // A shop never asked which wallets it takes takes none, and the till lets a
    // cashier name one.
    assert!(till.wallets().is_empty());

    // And the device says so. A snapshot it cannot read is not an error, but a
    // device that quietly re-downloads its whole catalogue every morning on a
    // shop's mobile data is a bill nobody can explain.
    assert!(report.catalogue_refetched);

    // The catalogue is the one thing that did not survive, and it is the one
    // thing that should not have to: it is a cache, and it comes back from the
    // server. The alternative is a till that will not open.
    assert_eq!(till.replica().len(), 0);
    assert_eq!(
        status.cursor, 0,
        "and it asks for the catalogue from the start"
    );
}
