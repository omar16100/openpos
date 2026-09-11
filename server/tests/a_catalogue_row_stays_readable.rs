//! A catalogue row written yesterday is still readable today.
//!
//! The bytes below are real. They came out of a shop's own database with
//! `select encode(payload, 'hex')`, and the seven like them written on that
//! shop's first day had stopped decoding: the back office listed them as
//! written by a version of the software it cannot read, every till went on
//! selling those items at the price it already held, and the advice on the
//! screen was to type the prices in again.
//!
//! Nothing about the rows had changed. `ItemWire` had grown three fields, each
//! appended correctly, while the stored schema number stayed at 2. postcard is
//! positional, so the older, shorter rows could no longer be read as the newer,
//! longer shape, and there was nothing else to try them as.
//!
//! The constant that number lives on says, in its own comment, that it must be
//! bumped whenever `ItemWire` changes. It was read and ignored three times. So
//! this file counts the fields instead: a comment that has to be remembered is
//! not a rule, and the only thing that has ever caught this is a test.

use openpos_server::repo::{CATALOGUE_SCHEMA, ITEM_WIRE_FIELDS};

/// An item as this shop's seed wrote it on 7 September 2026: sugar, a kilo,
/// with a Bangla name, stamped schema 2, and one field short of what schema 2
/// came to mean by the ninth.
const SUGAR_AS_WRITTEN_ON_THE_SEVENTH: &str = "04045355473109537567617220316b671de0a69ae0a6bfe0a6a8e0a6bf20e0a7a720e0a695e0a787e0a69ce0a6bf034e6f73a8c301a09c01dc0b0000010d38363930303030303030303034000100";

/// An item written on the ninth, stamped schema 2 as well, and three fields
/// longer.
const A_BARCODE_WALK_ON_THE_NINTH: &str = "ef90c4c798c7d2c8a191a999d8e3b8eda098020757414c4b4243410e57616c6b20426172636f646520410e57616c6b20426172636f64652041034e6f73904ef02edc0b0000010d393939393030303031313131320001000000";

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect()
}

#[test]
fn both_vintages_of_schema_two_are_read_and_neither_is_guessed_at() {
    let old = openpos_server::read_catalogue_payload(2, &bytes(SUGAR_AS_WRITTEN_ON_THE_SEVENTH))
        .expect("the row this shop already holds");
    assert_eq!(old.name_en, "Sugar 1kg");
    assert_eq!(old.price_minor, 12_500, "a hundred and twenty-five taka, as the shop wrote it");
    assert_eq!(old.barcodes, vec![String::from("8690000000004")]);
    // That build could not say what kind of supply this is or what the shop
    // sorts it under, so it says neither. Standard rated and unsorted is what
    // it was sold as, and is the only honest answer.
    assert_eq!(old.supply, 0);
    assert_eq!(old.category, "");

    let new = openpos_server::read_catalogue_payload(2, &bytes(A_BARCODE_WALK_ON_THE_NINTH))
        .expect("and the longer row beside it");
    assert_eq!(new.name_en, "Walk Barcode A");
    assert_eq!(new.price_minor, 5_000);

    // The longer row must not be read as the shorter shape. postcard leaves the
    // extra bytes without complaining, so a decoder that took the first shape
    // that worked would drop this item's tax class and category and nobody
    // would be told.
    let shorter =
        postcard::from_bytes::<openpos_core::protocol::ItemWireV2>(&bytes(A_BARCODE_WALK_ON_THE_NINTH));
    assert!(
        shorter.is_ok(),
        "the point of this assertion is that it does decode, wrongly, which is why the decoder \
         requires the whole payload to be consumed"
    );
}

#[test]
fn a_row_this_build_cannot_read_is_skipped_rather_than_guessed() {
    // From a newer build, on a shared database during a rolling upgrade.
    assert!(openpos_server::read_catalogue_payload(99, &bytes(A_BARCODE_WALK_ON_THE_NINTH)).is_none());
    // And nonsense is nonsense.
    assert!(openpos_server::read_catalogue_payload(2, &[0xff, 0xff, 0xff]).is_none());
}

#[test]
fn the_schema_moves_when_the_shape_does() {
    // The field count is read from the source rather than from the type,
    // because a struct cannot be asked how many fields it has and this is the
    // check that matters: somebody appending a field is exactly who needs to be
    // stopped and told what else to do.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../core/src/protocol/mod.rs"
    ))
    .expect("the protocol's own source");
    let at = source
        .find("pub struct ItemWire {")
        .expect("ItemWire is where it was");
    let body = &source[at..source[at..].find("\n}").map_or(source.len(), |end| at + end)];
    let fields = body.matches("\n    pub ").count();

    assert_eq!(
        fields, ITEM_WIRE_FIELDS,
        "ItemWire now has {fields} fields and the last catalogue schema was minted for \
         {ITEM_WIRE_FIELDS}. A field appended to it cannot be read out of the rows a shop already \
         holds: freeze the shape as it stands in core/src/protocol/mod.rs, teach \
         decode_catalogue_payload to try it, raise CATALOGUE_SCHEMA and ITEM_WIRE_FIELDS together. \
         Skipping that leaves every row written before today unreadable, and the shop is told to \
         type its prices in again."
    );
    assert_eq!(
        CATALOGUE_SCHEMA, 3,
        "the schema this build writes, which the message above is about"
    );
}
