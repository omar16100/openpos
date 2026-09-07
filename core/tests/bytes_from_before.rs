//! The bytes older builds actually wrote, frozen, and read by this one.
//!
//! Every legacy shape in `wire.rs` exists to read what a device already holds,
//! and every one of them is written in terms of the *current* nested types: the
//! leases, the parked baskets, the people, the counted drawers, the customers,
//! the credential, what was allowed. The day a field is added to any of those,
//! all seven legacy standing states quietly change shape with it and stop
//! reading the bytes they were kept for. The tests that build a legacy struct
//! and decode it do not catch that, because they build it out of the same
//! changed type: they encode and decode one shape and agree with themselves.
//!
//! So the bytes are here instead, as hex, taken from the shapes on the day each
//! was current. Nothing generates them; they are a record. A field added
//! anywhere below the surface fails these immediately, on a laptop, rather than
//! on a shop's tablet on the morning after an upgrade, where the symptom is a
//! till that will not open its own ledger.
//!
//! What each fixture describes is the same shop throughout: five hundred receipt
//! numbers with four spent, Rahima who may allow things, a credential, the shop
//! that heads the receipts, and from the version each arrived in, the drawer she
//! counted, the man in flat 3 who buys on account, and when the credential was
//! taken.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::storage::wire::{
    self, SALE_SCHEMA_V1, SALE_SCHEMA_V2, SHIFT_SCHEMA_V1, ShiftEventV1, TERMINAL_SCHEMA_V1,
    TERMINAL_SCHEMA_V2, TERMINAL_SCHEMA_V3, TERMINAL_SCHEMA_V4, TERMINAL_SCHEMA_V5,
    TERMINAL_SCHEMA_V6, TERMINAL_SCHEMA_V7, TERMINAL_SCHEMA_V8, TERMINAL_SCHEMA_V9,
    TERMINAL_SCHEMA_V10,
};

/// The standing state, one line per version, as that version wrote it.
const TERMINAL: [(u16, &str); 10] = [
    (
        TERMINAL_SCHEMA_V1,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b6100",
    ),
    (
        TERMINAL_SCHEMA_V2,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b617368",
    ),
    (
        TERMINAL_SCHEMA_V3,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b61736800",
    ),
    (
        TERMINAL_SCHEMA_V4,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b61736801504606526168696d6180bcf886873480f0819a873480b5180ca8890cd48406904ed00fe8fc24e4f5248307",
    ),
    (
        TERMINAL_SCHEMA_V5,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b61736801504606526168696d6180bcf886873480f0819a873480b5180ca8890cd48406904ed00fe8fc24e4f524830701150d4b6172696d2c20666c61742033010b303137313130303030303001",
    ),
    (
        TERMINAL_SCHEMA_V6,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b61736801504606526168696d6180bcf886873480f0819a873480b5180ca8890cd48406904ed00fe8fc24e4f524830701150d4b6172696d2c20666c61742033010b3031373131303030303030010180bcf886873480d8c4bd75",
    ),
    (
        TERMINAL_SCHEMA_V7,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b61736801504606526168696d6180bcf886873480f0819a873480b5180ca8890cd48406904ed00fe8fc24e4f524830701150d4b6172696d2c20666c61742033010b3031373131303030303030010180bcf886873480d8c4bd750004",
    ),
    (
        TERMINAL_SCHEMA_V8,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b6173680201504606526168696d6180bcf886873480f0819a873480b5180ca8890cd48406904ed00fe8fc24e4f524830701150d4b6172696d2c20666c61742033010b3031373131303030303030010180bcf886873480d8c4bd750004",
    ),
    (
        TERMINAL_SCHEMA_V9,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b6173680201504606526168696d6180bcf886873480f0819a873480b5180ca8890cd48406904ed00fe8fc24e4f524830701150d4b6172696d2c20666c61742033010b3031373131303030303030010180bcf886873480d8c4bd75000401090d383639303030303030393939391642697363756974732c20746865206e6577206f6e65731642697363756974732c20746865206e6577206f6e6573034e6f73c0bb0100dc0b0000010d383639303030303030393939390001",
    ),
    (
        TERMINAL_SCHEMA_V10,
        "01070102543164d7040002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b6173680201504606526168696d6180bcf886873480f0819a873480b5182788d51280f10400a08d06e8fc24e4f524830701150d4b6172696d2c20666c61742033010b303137313130303030303001010e3030323334353637382d303230320180bcf886873480d8c4bd75000401090d383639303030303030393939391642697363756974732c20746865206e6577206f6e65731642697363756974732c20746865206e6577206f6e6573034e6f73c0bb0100dc0b0000010d38363930303030303039393939000101161353686566616c692c20746865207461696c6f72000100",
    ),
];

/// Saturday's last sale, rung by a build that did not say what it sold a thing
/// by and never sent.
const SALE_BEFORE_SUPPLY: &str = "85070780bcf8868734010954312d30303031303501010001010552494345351052696365204d696e696b657420356b67f09f05d00f00dc0b0000034e6f73000100a08d0600f09f05e46400d48406cc0800016a01010101cf0f00";

const SALE: &str = "84070780bcf8868734010954312d30303031303401010001010552494345351052696365204d696e696b657420356b67f09f05d00f00dc0b00000000f09f05e46400d484060000016901010101cf0f00";

/// A drawer opened, topped up from the safe, and counted, by the build before
/// the count learned to name who counted it.
const SHIFT_OPENED: &str = "00500780b51880bcf8868734";
const SHIFT_MOVED: &str = "0101a08d06146368616e67652066726f6d207468652073616665c0c0b5878734";
const SHIFT_CLOSED: &str = "02b0a51880f0819a8734";

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

/// Every standing state a supported build ever wrote still reads.
///
/// The things asserted are the ones a shop loses if this breaks: the numbers it
/// owns, the credential it syncs with, the shop that heads its receipts, and
/// from the version each arrived in, the drawer somebody counted and the person
/// who buys on account.
#[test]
fn every_standing_state_an_older_build_wrote_still_reads() {
    for (schema, hex) in TERMINAL {
        let read = wire::decode_terminal_state(schema, &bytes(hex))
            .unwrap_or_else(|error| panic!("version {schema} no longer reads: {error}"));

        assert_eq!(read.leases.len(), 1, "version {schema}: the block it owns");
        assert_eq!(read.leases[0].first, 100, "version {schema}");
        assert_eq!(read.leases[0].last, 599, "version {schema}");
        assert_eq!(read.unnumbered, 2, "version {schema}: sales owed a number");
        assert_eq!(
            read.token.as_deref(),
            Some("a-credential"),
            "version {schema}: what it syncs with"
        );
        assert_eq!(
            read.operators.len(),
            1,
            "version {schema}: who may stand at it"
        );
        assert_eq!(read.operators[0].name, "Rahima", "version {schema}");
        assert!(read.operators[0].may_authorise, "version {schema}");
        assert_eq!(read.operators[0].rounds, 1_000, "version {schema}");
        assert_eq!(read.operators[0].key.len(), 32, "version {schema}");

        let shop = read
            .shop
            .unwrap_or_else(|| panic!("version {schema}: the shop that heads its receipts"));
        assert_eq!(shop.name, "Karim General Store", "version {schema}");
        assert_eq!(
            shop.bin.as_deref(),
            Some("001234567-0101"),
            "version {schema}"
        );
        // Version 1 predates the wallets, and a shop that was never asked which
        // it takes takes none.
        if schema == TERMINAL_SCHEMA_V1 {
            assert!(shop.wallets.is_empty(), "version {schema}");
        } else {
            assert_eq!(shop.wallets, ["bKash"], "version {schema}");
        }
        // Nothing before version 8 was asked what to do about the shelf, and
        // the answer for a shop that was never asked is to do nothing.
        if schema >= TERMINAL_SCHEMA_V8 {
            assert_eq!(shop.stock_rule, 2, "version {schema}: refuse it");
        } else {
            assert_eq!(shop.stock_rule, 0, "version {schema}");
        }

        // Items a till wrote down itself, from version 9.
        if schema >= TERMINAL_SCHEMA_V9 {
            assert_eq!(read.unsent_items.len(), 1, "version {schema}");
            assert_eq!(
                read.unsent_items[0].name_en, "Biscuits, the new ones",
                "version {schema}"
            );
            assert_eq!(read.unsent_items[0].price_minor, 12_000, "version {schema}");
        } else {
            assert!(read.unsent_items.is_empty(), "version {schema}");
        }

        // Nobody before version 10 could hold a buyer's BIN, and nobody was
        // asked for one. From version 10 the shop's own BIN row on a receipt
        // needs it, and it comes back as it was written.
        if schema >= TERMINAL_SCHEMA_V10 {
            assert_eq!(
                read.customers[0].bin.as_deref(),
                Some("002345678-0202"),
                "version {schema}"
            );
            assert_eq!(read.unsent_customers.len(), 1, "version {schema}");
            assert_eq!(
                read.unsent_customers[0].name, "Shefali, the tailor",
                "version {schema}"
            );
        } else {
            for known in &read.customers {
                assert!(known.bin.is_none(), "version {schema}");
            }
            // And nobody wrote people down at a till before then either.
            assert!(read.unsent_customers.is_empty(), "version {schema}");
        }
        // Nothing written before version 11 says what kind of supply it is, and
        // everything those builds sold was taxed at whatever rate it carried.
        for held in &read.unsent_items {
            assert_eq!(held.supply, 0, "version {schema}");
        }

        // A counted drawer from version 4, when a device started keeping them.
        if schema >= TERMINAL_SCHEMA_V4 {
            assert_eq!(read.unsent_shifts.len(), 1, "version {schema}");
            assert_eq!(
                read.unsent_shifts[0].closed_by_name, "Rahima",
                "version {schema}"
            );
            assert_eq!(
                read.unsent_shifts[0].variance_minor, -450,
                "version {schema}"
            );
            assert_eq!(
                read.unsent_shifts[0].expected_cash_minor, 302_900,
                "version {schema}"
            );
        } else {
            assert!(read.unsent_shifts.is_empty(), "version {schema}");
        }

        // The people who buy on account, from version 5.
        if schema >= TERMINAL_SCHEMA_V5 {
            assert_eq!(read.customers.len(), 1, "version {schema}");
            assert_eq!(read.customers[0].name, "Karim, flat 3", "version {schema}");
        } else {
            assert!(read.customers.is_empty(), "version {schema}");
        }

        // When the credential was taken, from version 6. Without it a device
        // renews at the first opportunity, which is right and is not this.
        if schema >= TERMINAL_SCHEMA_V6 {
            let note = read
                .credential
                .unwrap_or_else(|| panic!("version {schema}: when it took its credential"));
            assert_eq!(note.taken_at_ms, 1_788_600_000_000, "version {schema}");
            assert_eq!(note.lifetime_ms, 31_536_000_000, "version {schema}");
        } else {
            assert!(read.credential.is_none(), "version {schema}");
        }

        // What it allowed and has not sent, from version 7.
        if schema >= TERMINAL_SCHEMA_V7 {
            assert_eq!(read.allowed_seq, 4, "version {schema}");
        } else {
            assert_eq!(read.allowed_seq, 0, "version {schema}");
        }
    }
}

/// The last sale the build before the supply distinction wrote.
///
/// A sale sitting in an outbox across an upgrade, which is the ordinary case on
/// the morning a shop updates: the till was closed with sales in it, and every
/// one of them has to still read, still total, and still declare what it did.
#[test]
fn a_sale_from_before_the_supply_distinction_still_reads() {
    let read = wire::decode_sale(SALE_SCHEMA_V2, &bytes(SALE_BEFORE_SUPPLY))
        .expect("a sale held across the upgrade");

    assert_eq!(read.ticket.receipt_no.as_deref(), Some("T1-000105"));
    assert_eq!(read.ticket.total_minor, 49_450, "what the customer paid");
    assert_eq!(read.ticket.change_minor, 550);
    assert_eq!(read.ticket.lines.len(), 1);
    assert_eq!(read.ticket.lines[0].name, "Rice Miniket 5kg");
    assert_eq!(read.ticket.lines[0].unit, "Nos");
    // Sold before a shop could say a thing was exempt, so it is what that build
    // charged: the ordinary treatment at the rate on the line.
    assert_eq!(read.ticket.lines[0].supply, 0);
    assert_eq!(read.ticket.lines[0].vat_bp, 1_500);
    assert_eq!(read.lease_next, Some(106));
    assert_eq!(read.stock, [(1, -1_000)]);
}

/// The sale a device may still be holding, unsent, from a build ago.
///
/// The one thing in the ledger that exists nowhere else: goods left the shop and
/// money changed hands, and nobody but this device knows.
#[test]
fn a_sale_an_older_build_wrote_still_reads_and_still_totals() {
    let read = wire::decode_sale(SALE_SCHEMA_V1, &bytes(SALE)).expect("Saturday's last sale");

    assert_eq!(read.ticket.id, 900);
    assert_eq!(read.ticket.receipt_no.as_deref(), Some("T1-000104"));
    assert_eq!(read.ticket.rung_at_ms, 1_788_600_000_000);
    assert_eq!(read.ticket.total_minor, 49_450, "what the customer paid");
    assert_eq!(read.ticket.net_minor, 43_000);
    assert_eq!(read.ticket.vat_minor, 6_450);
    assert_eq!(read.ticket.lines.len(), 1);
    assert_eq!(read.ticket.lines[0].name, "Rice Miniket 5kg");
    assert_eq!(read.ticket.lines[0].qty_milli, 1_000);
    // Rung before a shop could say what it sold a thing by. Every one of them
    // meant pieces.
    assert_eq!(read.ticket.lines[0].unit, "Nos");
    assert_eq!(read.lease_next, Some(105), "the number it had reached");
    assert_eq!(read.stock, [(1, -1_000)]);
}

/// The drawer events a device may still be replaying from a build ago.
///
/// The open drawer is not stored anywhere: it is these, replayed. A build that
/// cannot read them is a shop that opens on Sunday morning, is told no drawer is
/// open, and counts the evening against a float of nothing.
#[test]
fn the_drawer_events_an_older_build_wrote_still_replay() {
    match wire::decode_shift_event(SHIFT_SCHEMA_V1, &bytes(SHIFT_OPENED)).expect("the morning") {
        ShiftEventV1::Opened {
            id,
            terminal,
            opening_float_minor,
            at_ms,
        } => {
            assert_eq!(id, 80);
            assert_eq!(terminal, 7);
            assert_eq!(opening_float_minor, 200_000, "two thousand taka counted in");
            assert_eq!(at_ms, 1_788_600_000_000);
        }
        other => panic!("the morning read back as {other:?}"),
    }

    match wire::decode_shift_event(SHIFT_SCHEMA_V1, &bytes(SHIFT_MOVED)).expect("the safe") {
        ShiftEventV1::CashMoved {
            inward,
            amount_minor,
            ref reason,
            at_ms,
        } => {
            assert!(inward, "money in, not out: the direction is the operation");
            assert_eq!(amount_minor, 50_000);
            assert_eq!(reason, "change from the safe");
            assert_eq!(at_ms, 1_788_601_000_000);
        }
        other => panic!("the safe read back as {other:?}"),
    }

    match wire::decode_shift_event(SHIFT_SCHEMA_V1, &bytes(SHIFT_CLOSED)).expect("the count") {
        ShiftEventV1::Closed {
            counted_cash_minor,
            at_ms,
            counted_by,
            ref counted_by_name,
        } => {
            assert_eq!(counted_cash_minor, 199_000);
            assert_eq!(at_ms, 1_788_640_000_000);
            // Nobody, because the build that wrote it did not ask. Better than
            // a name invented here.
            assert_eq!(counted_by, 0);
            assert!(counted_by_name.is_empty());
        }
        other => panic!("the count read back as {other:?}"),
    }
}
