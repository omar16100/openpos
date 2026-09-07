//! What a day already in the log costs to boot.
//!
//! The critical log is emptied when the shop has taken every sale in it, but not
//! while a drawer is open: the open drawer is rebuilt by replaying that same log
//! and lives nowhere else. So a till that syncs all afternoon still holds the
//! day's frames until the drawer is counted, and every restart replays them.
//!
//! That is the same load a day with no internet puts on it, which is the case
//! the design has to survive anyway. This measures it rather than assuming it.
//!
//! ```text
//! cargo run --release -p openpos-bindings --example boot_cost -- 1000
//! ```

// A tool run by hand. It panics on anything unexpected on purpose: there is
// nobody to hand an error to, and a stack trace is more use here than a message.
#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss
)]

use std::time::Instant;

use openpos_bindings::files::FileBackend;
use openpos_core::auth::{Operator, Permissions, PinHash, SALT_LEN};
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::domain::{PriceMode, VatBase};
use openpos_core::ids::Ulid;
use openpos_core::lease::Lease;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::replica::Item;
use openpos_core::till::Till;

extern crate alloc;

const TENANT: u128 = 42;

fn main() {
    let wanted: usize = std::env::args()
        .nth(1)
        .and_then(|given| given.parse().ok())
        .unwrap_or(1_000);

    let home = std::env::temp_dir().join(format!(
        "openpos-boot-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));

    let terminal = Ulid::from_u128(7);
    let sold: Vec<Ulid>;
    {
        let mut till = Till::open(
            FileBackend::open(&home).expect("a store opens"),
            TENANT,
            terminal,
            1,
            CartLimits::unrestricted(),
        )
        .expect("a till opens")
        .0;
        till.apply_pull(&openpos_core::storage::wire::ItemDeltasV1 {
            cursor: 1,
            upserts: alloc::vec![openpos_core::storage::wire::ItemV1::from_domain(&Item {
                id: Ulid::from_u128(1),
                code: "RICE5".into(),
                name_en: "Rice Miniket 5kg".into(),
                name_bn: "মিনিকেট চাল ৫ কেজি".into(),
                unit: "Nos".into(),
                price: Minor::new(43_000),
                cost: Minor::new(38_000),
                vat_rate: Bp::new(1_500).expect("a rate"),
                price_mode: PriceMode::Exclusive,
                vat_base: VatBase::Discounted,
                barcodes: alloc::vec!["8690000000001".into()],
                on_hand: Milli::new(10_000_000),
                active: true,
                supply: openpos_core::domain::Supply::Standard,
            })],
            tombstones: alloc::vec::Vec::new(),
        })
        .expect("a catalogue");
        till.grant_lease(&Lease::new(terminal, 1, "T7", 1, 100_000))
            .expect("numbers");
        till.open_shift(Ulid::from_u128(80), Minor::new(200_000), 1)
            .expect("a drawer");

        let mut ids = alloc::vec::Vec::with_capacity(wanted);
        for index in 0..wanted {
            till.scan("8690000000001", Milli::ONE).expect("a scan");
            let total = till.totals().expect("totals").total;
            till.add_tender(Tender {
                kind: TenderKind::Cash,
                amount: total,
                reference: None,
            })
            .expect("cash");
            ids.push(
                till.checkout(Ulid::from_u128(900_000 + index as u128), 2)
                    .expect("a sale")
                    .ticket
                    .id,
            );
        }
        // The shop takes the lot, mid-afternoon, drawer still open.
        till.acknowledge(&ids).expect("an acknowledgement");
        sold = ids;
    }

    let log = std::fs::metadata(home.join("critical.log"))
        .map(|found| found.len())
        .unwrap_or_default();

    let began = Instant::now();
    let (mut till, report) = Till::open(
        FileBackend::open(&home).expect("a store opens"),
        TENANT,
        terminal,
        1,
        CartLimits::unrestricted(),
    )
    .expect("a till opens");
    let boot = began.elapsed().as_secs_f64() * 1_000.0;

    let drawer = till.x_report().expect("the drawer is still open");
    println!(
        "{} sales, all acknowledged, drawer open: log {} KB, boot {:.1} ms",
        sold.len(),
        log / 1_024,
        boot
    );
    println!(
        "the drawer came back: float {}, sales {}, expected {}",
        drawer.opening_float.get(),
        drawer.sales,
        drawer.expected_cash.get()
    );
    println!(
        "nothing outstanding to send: {} unsynced, {} items in the catalogue",
        report.unsynced_sales, report.items
    );

    // Evening. Somebody counts it, which is what lets the log go.
    till.set_operators(alloc::vec![Operator {
        id: Ulid::from_u128(70),
        name: "Karim".into(),
        pin: PinHash::derive("9999", [3; SALT_LEN], 1_000),
        permissions: Permissions::supervisor(),
        active: true,
    }])
    .expect("a person");
    till.sign_in(Ulid::from_u128(70), "9999", 3)
        .expect("signed in");
    let counted = till
        .close_shift(drawer.expected_cash, 4)
        .expect("a counted drawer");
    drop(till);

    let after = std::fs::metadata(home.join("critical.log"))
        .map(|found| found.len())
        .unwrap_or_default();
    let (till, _) = Till::open(
        FileBackend::open(&home).expect("a store opens"),
        TENANT,
        terminal,
        1,
        CartLimits::unrestricted(),
    )
    .expect("a till opens");
    println!();
    println!(
        "counted at {}, variance {}: log {} bytes after the count",
        counted.counted_cash.get(),
        counted.variance.get(),
        after
    );
    println!(
        "and after a restart: {} drawer open, {} count waiting to be sent by {}",
        till.shift().map_or("no", |_| "a"),
        till.unsent_shifts().len(),
        till.unsent_shifts()
            .first()
            .map_or("nobody", |held| held.closed_by_name.as_str())
    );

    std::fs::remove_dir_all(&home).ok();
}
