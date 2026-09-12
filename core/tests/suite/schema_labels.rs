//! Every frame is labelled with the schema its payload was actually written
//! under.
//!
//! Two bugs in two days came from this going unchecked. The standing state was
//! read under the reading build's constant rather than the schema in the bytes,
//! so an upgrade would have lost every till's leases, parked sales and
//! credential. The snapshot was the same, and worse: its frame said schema 1
//! while its payload had been version 2 since the tax base was added to an item.
//!
//! Neither showed up. Passing the constant on the way in and writing the wrong
//! number on the way out cancel out exactly, and go on cancelling until somebody
//! corrects one of them. Nothing in the suite wrote a blob and then read its
//! label.
//!
//! So this drives the real writers, reads back what they wrote, and insists that
//! the number on the outside decodes what is inside. It fails if a writer stamps
//! a constant that does not match, and it fails if a schema is bumped without
//! the writer being told.

// Tests assert with plain arithmetic and panic on failure, which is the point of
// them. The workspace bans both in production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use openpos_core::domain::Supply;
use openpos_core::auth::{Operator, Permissions, PinHash};
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::lease::Lease;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::Item;
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::storage::frame::{PayloadKind, Store};
use openpos_core::storage::wire::{
    self, ACK_SCHEMA, DELTAS_SCHEMA, ItemDeltasV1, ItemV1, SALE_SCHEMA, SHIFT_SCHEMA,
    SNAPSHOT_SCHEMA, TERMINAL_SCHEMA,
};
use openpos_core::till::Till;

const TENANT: u128 = 42;

fn item() -> Item {
    Item {
        id: Ulid::from_u128(1),
        code: "RICE5".into(),
        name_en: "Rice Miniket 5kg".into(),
        name_bn: "মিনিকেট চাল ৫ কেজি".into(),
        unit: "Nos".into(),
        price: Minor::new(43_000),
        cost: Minor::new(38_000),
        vat_rate: Bp::new(1_500).unwrap(),
        price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
        vat_base: openpos_core::domain::pricing::VatBase::Discounted,
        barcodes: vec!["8690000000001".into()],
        on_hand: Milli::new(40_000),
        active: true,
        supply: Supply::Standard,
        category: "".into(),
    }
}

fn supervisor() -> Operator {
    Operator {
        id: Ulid::from_u128(70),
        name: "Supervisor".into(),
        // Few rounds: this is a test about labels, not about PBKDF2, and the
        // cost is stored per person for exactly this reason.
        pin: PinHash::derive("9999", [7_u8; 16], 1_000),
        permissions: Permissions {
            max_discount_bp: 10_000,
            may_override_price: true,
            may_refund: true,
            may_void_line: true,
            may_authorise: true,
            may_open_drawer: true,
            may_close_shift: true,
        },
        active: true,
    }
}

/// A day's work, through the writers a shop actually goes through.
fn a_till_that_has_done_everything() -> Till<MemoryBackend> {
    let terminal = Ulid::from_u128(7);
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        TENANT,
        terminal,
        1,
        CartLimits::unrestricted(),
    )
    .unwrap();

    // The standing state: people, a credential, the shop, a lease.
    till.put_operator(supervisor()).unwrap();
    till.sign_in(Ulid::from_u128(70), "9999", 0).unwrap();
    till.set_token("a-credential").unwrap();
    till.set_shop(
        openpos_core::receipt::Shop {
            name: "Karim General Store".to_owned(),
            bin: None,
            address: None,
            phone: None,
        },
        vec!["bKash".into()],
        openpos_core::domain::StockRule::Off,
        vec![],
    )
    .unwrap();
    till.grant_lease(&Lease::new(terminal, 1, "T1", 100, 599))
        .unwrap();

    // The catalogue, and a snapshot folded from it.
    till.apply_pull(&ItemDeltasV1 {
        cursor: 1,
        upserts: vec![ItemV1::from_domain(&item())],
        tombstones: vec![],
    })
    .unwrap();
    till.checkpoint_now().unwrap();

    // A drawer, and a sale through it.
    till.open_shift(Ulid::from_u128(500), Minor::new(50_000), 1_000)
        .unwrap();
    till.scan("8690000000001", Milli::ONE).unwrap();
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(49_450),
        reference: None,
    }, 0)
    .unwrap();
    till.checkout(Ulid::from_u128(900), 2_000).unwrap();

    // A second sale, and only the first acknowledged. A full acknowledgement
    // empties the critical log by design, so one has to still be waiting for
    // the sale frame and the acknowledgement to be in it together.
    till.scan("8690000000001", Milli::ONE).unwrap();
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(49_450),
        reference: None,
    }, 0)
    .unwrap();
    till.checkout(Ulid::from_u128(901), 3_000).unwrap();
    till.acknowledge(&[Ulid::from_u128(900)]).unwrap();

    // A pull after the checkpoint, so a delta frame is in the log rather than
    // folded away by it.
    till.apply_pull(&ItemDeltasV1 {
        cursor: 2,
        upserts: vec![],
        tombstones: vec![Ulid::from_u128(99).to_u128()],
    })
    .unwrap();

    till
}

#[test]
fn every_frame_is_labelled_with_the_schema_its_payload_uses() {
    let till = a_till_that_has_done_everything();
    let journal = till.journal();

    let mut seen = Vec::new();
    for store in [Store::Critical, Store::ReplicaCache] {
        for record in journal.read(store).unwrap() {
            let schema = record.header.schema;
            let payload = &record.payload;
            match record.header.kind {
                PayloadKind::SaleCommit => {
                    assert_eq!(schema, SALE_SCHEMA, "a sale");
                    wire::decode_sale(schema, payload).expect("a sale decodes under its own label");
                }
                PayloadKind::ItemDeltas => {
                    assert_eq!(schema, DELTAS_SCHEMA, "catalogue changes");
                    wire::decode_deltas(schema, payload).expect("deltas decode under their label");
                }
                PayloadKind::ShiftEvent => {
                    assert_eq!(schema, SHIFT_SCHEMA, "a drawer event");
                    wire::decode_shift_event(schema, payload)
                        .expect("a shift event decodes under its label");
                }
                PayloadKind::SyncAck => {
                    assert_eq!(schema, ACK_SCHEMA, "an acknowledgement");
                    wire::decode_ack(schema, payload).expect("an ack decodes under its label");
                }
                other => panic!("nothing writes {other:?} to a log any more"),
            }
            seen.push(record.header.kind);
        }
    }

    // The blobs, which is where both bugs were: they are read by label and were
    // the only records nothing checked.
    let (schema, bytes) = journal.load_snapshot().unwrap().expect("a snapshot");
    assert_eq!(schema, SNAPSHOT_SCHEMA, "a snapshot");
    wire::decode_snapshot(schema, &bytes).expect("a snapshot decodes under its own label");

    let (schema, bytes) = journal
        .load_terminal_state()
        .unwrap()
        .expect("standing state");
    assert_eq!(schema, TERMINAL_SCHEMA, "the standing state");
    wire::decode_terminal_state(schema, &bytes)
        .expect("standing state decodes under its own label");

    // And the day's work really did write the kinds this claims to cover, so a
    // writer that stops running cannot make this pass by covering nothing.
    assert!(seen.contains(&PayloadKind::SaleCommit));
    assert!(seen.contains(&PayloadKind::ShiftEvent));
    assert!(seen.contains(&PayloadKind::ItemDeltas));
    assert!(seen.contains(&PayloadKind::SyncAck));
}
