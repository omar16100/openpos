//! The bytes this build writes, frozen, so a shape cannot change under a
//! schema number that stays put.
//!
//! `bytes_from_before.rs` freezes what older builds wrote and proves this one
//! still reads them. It cannot see the other half of the same mistake: a field
//! appended to a shape that is still writing under the current number. Every
//! file written before that append, by this same build, stops decoding the day
//! it ships, and the device holding one is a shop's own till.
//!
//! That is not hypothetical. The catalogue payload's schema number sat at 2
//! while its shape grew three fields, and one shop's seven oldest rows became
//! unreadable: every till went on selling those items at the price it already
//! held and the back office told the owner to type the prices in again. The
//! number's own comment said it had to be bumped whenever the shape changed.
//! The comment was read and ignored three times.
//!
//! So the check is bytes rather than a comment. Change anything anywhere in
//! these shapes, including inside a line, a tender, a person or a trail entry,
//! and the encoding moves and this file fails with what to do about it.
//!
//! The values are decoded from the frozen bytes of the version before, so the
//! fixture cannot drift on its own: it is the shop `bytes_from_before.rs`
//! describes, carried forward.
//!
//! What it cannot see: two fields of the same type, next to each other, holding
//! equal values in this fixture, swapped. The encoding is identical and so is
//! everything a shop would notice, until one of them holds a different value.
//! Tried, and it is why this note is here rather than a claim that nothing gets
//! past it.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::storage::wire::{
    self, SALE_SCHEMA, SALE_SCHEMA_V3, TERMINAL_SCHEMA, TERMINAL_SCHEMA_V15, encode_sale,
    encode_terminal_state,
};

/// The standing state as version 15 wrote it: a shop with a block of receipt
/// numbers, a basket parked, somebody who may allow things, a counted drawer, a
/// person who buys on account, a credential, an entry in the trail and an item
/// the till wrote down itself.
const STATE_AS_FIFTEEN_WROTE_IT: &str = "01070102543164d70401f403a0fe968787340016746865206d616e207769746820746865206372617465000002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b6173680201504606526168696d6180bcf886873480f0819a873480b5182788d51280f10400a08d06e8fc24e4f524830701150d4b6172696d2c20666c61742033010b303137313130303030303001010e3030323334353637382d30323032c0843d0180bcf886873480d8c4bd75010580c4f1aa91330e004606526168696d610000010954312d3030303130340501090d383639303030303030393939391642697363756974732c20746865206e6577206f6e65731642697363756974732c20746865206e6577206f6e6573034e6f73c0bb0100dc0b0000010d3836393030303030303939393900010208426973637569747301161353686566616c692c20746865207461696c6f7200010000";

/// What this build writes for that same shop. One byte longer than the line
/// above for each field the standing state has gained since, and different
/// anywhere a nested shape has moved.
///
/// It went a byte longer for a day, when a device wrote down whether it had
/// been round the shelf, and came back when that claim turned out to outlive
/// the figures it was about. Schema 17 wrote that longer line and this build
/// still reads it.
///
/// A byte longer again under schema 20, which is the empty list of languages a
/// shop offers: this shop has never said, which means all of them, which is
/// what it had before a shop could say. And one more under schema 21, the empty
/// list of PINs got wrong: nobody on this device has typed one wrongly, which
/// is what every device on the build before believed every time a tab closed.
const STATE_AS_THIS_BUILD_WRITES_IT: &str = "01070102543164d70401f403a0fe968787340016746865206d616e207769746820746865206372617465000000000002014606526168696d611009090909090909090909090909090909e807200303030303030303030303030303030303030303030303030303030303030303d00f01010101010101010c612d63726564656e7469616c01134b6172696d2047656e6572616c2053746f7265010e3030313233343536372d3031303101153132204d697270757220526f61642c204468616b61000105624b61736802000001504606526168696d6180bcf886873480f0819a873480b5182788d51280f10400a08d06e8fc24e4f524830701150d4b6172696d2c20666c61742033010b303137313130303030303001010e3030323334353637382d30323032c0843d000180bcf886873480d8c4bd75010580c4f1aa91330e004606526168696d610000010954312d3030303130340501090d383639303030303030393939391642697363756974732c20746865206e6577206f6e65731642697363756974732c20746865206e6577206f6e6573034e6f73c0bb0100dc0b0000010d3836393030303030303939393900010208426973637569747301161353686566616c692c20746865207461696c6f72000100000001070980f0819a8734e0d40304d09218904e000100d092180101904e146368616e67652066726f6d207468652073616665a0fd879a8734290101c8fb0100";

/// A sale as the build before the shop's own cost travelled with a line wrote
/// it: one line of rice, paid in cash, with a receipt number on it.
const SALE_AS_THREE_WROTE_IT: &str = "86070780bcf8868734010954312d30303031303601010001010552494345351052696365204d696e696b657420356b67f09f05d00f00dc0b0000034e6f7300000100d4840600f09f05e46400d484060000016b01010101cf0f00";

/// And what this build writes for the same sale.
const SALE_AS_THIS_BUILD_WRITES_IT: &str = "86070780bcf8868734010954312d30303031303601010001010552494345351052696365204d696e696b657420356b67f09f05d00f00dc0b0000034e6f730000000100d4840600f09f05e46400d48406000000016b01010101cf0f00";

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// What to do, said once, because both failures below need the same answer.
const WHAT_TO_DO: &str = "a shape written to disk under the current schema number has changed. \
     Every file this build has already written stops decoding the day this ships, and the device \
     holding one is a shop's till with its unsent sales in it. Freeze the shape as it stands, \
     raise the schema number, add the decode arm and a fixture in bytes_from_before.rs, then put \
     the new bytes here. If the change really is invisible on the wire, say why in the commit.";

#[test]
fn the_standing_state_is_written_as_this_build_has_always_written_it() {
    let mut shop =
        wire::decode_terminal_state(TERMINAL_SCHEMA_V15, &bytes(STATE_AS_FIFTEEN_WROTE_IT))
            .expect("the shop version 15 wrote");

    // Everything a standing state can hold is in here, so a field added
    // anywhere below the surface moves these bytes: a parked basket and the
    // line on it, the person at the till, the drawer they counted, the customer
    // who buys on account, the credential, the trail entry, the item the till
    // wrote down itself.
    assert_eq!(shop.leases.len(), 1);
    assert_eq!(shop.held.tickets.len(), 1);
    assert_eq!(shop.operators.len(), 1);
    assert_eq!(shop.unsent_shifts.len(), 1);
    assert_eq!(shop.customers.len(), 1);
    assert_eq!(shop.unsent_allowed.len(), 1);
    assert_eq!(shop.unsent_items.len(), 1);
    assert_eq!(shop.unsent_customers.len(), 1);

    // And the drawer that is open, which schema 19 added: a shop that never
    // counts one used to keep every byte it had ever written, because the log
    // could not be dropped under a drawer that lived only inside it. Put here
    // rather than left absent so that a field added to it moves these bytes.
    shop.open_drawer = Some(wire::OpenDrawerV1 {
        id: 7,
        terminal: 9,
        opened_at_ms: 1_788_640_000_000,
        opening_float_minor: 30_000,
        sales: 4,
        cash_sales_minor: 197_800,
        cash_in_minor: 5_000,
        cash_out_minor: 0,
        tenders: vec![wire::DrawerTenderV1 {
            kind: wire::TenderKindV1::Cash,
            amount_minor: 197_800,
        }],
        movements: vec![wire::DrawerMovementV1 {
            inward: true,
            amount_minor: 5_000,
            reason: String::from("change from the safe"),
            at_ms: 1_788_640_100_000,
        }],
        folded_through: 41,
        refunds: Some(wire::RefundsV1 {
            count: 1,
            cash_minor: 16_100,
        }),
    });

    let written = encode_terminal_state(&shop).expect("it encodes");
    assert_eq!(hex(&written), STATE_AS_THIS_BUILD_WRITES_IT, "{WHAT_TO_DO}");

    // And this build reads its own bytes back, which is the other half of the
    // same promise.
    let read = wire::decode_terminal_state(TERMINAL_SCHEMA, &written).expect("read back");
    assert_eq!(read, shop);
}

#[test]
fn a_sale_is_written_as_this_build_has_always_written_it() {
    let sale = wire::decode_sale(SALE_SCHEMA_V3, &bytes(SALE_AS_THREE_WROTE_IT))
        .expect("the sale version 3 wrote");
    assert_eq!(sale.ticket.lines.len(), 1);
    assert_eq!(sale.ticket.tenders.len(), 1);

    let written = encode_sale(&sale).expect("it encodes");
    assert_eq!(hex(&written), SALE_AS_THIS_BUILD_WRITES_IT, "{WHAT_TO_DO}");

    let read = wire::decode_sale(SALE_SCHEMA, &written).expect("read back");
    assert_eq!(read, sale);
}
