//! A basket of taxed, zero-rated and exempt goods, declared as three things.
//!
//! A rate of zero is not one thing. Zero rated is taxable at nothing; exempt is
//! outside the tax; and a return puts them in different places. Until this
//! existed both arrived at the shop as a rate of zero and were added together,
//! so a shop that had to tell them apart on a return could not.
//!
//! The whole path only means anything end to end: the owner classifies two
//! items, a till sells all three, the shop recomputes what the ticket owed from
//! the lines rather than believing the payload, and the return comes back with
//! the two nothings apart.
//!
//! ```text
//! cargo run -p openpos-server --example two_kinds_of_nothing -- http://127.0.0.1:8098 TILL_CODE OWNER_CODE
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(clippy::expect_used, clippy::print_stdout)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::Milli;
use openpos_core::protocol::{
    CatalogueEditResponse, EnrolRequest, EnrolResponse, ItemWire, PROTOCOL_VERSION, PullRequest,
    PullResponse, PushRequest, PushResponse, UpsertItemRequest, VatRequest, VatResponse,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_core::till::Till;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:8098".to_owned());
    let till_code = args.next().ok_or("give me the till's enrolment code")?;
    let owner_code = args.next().ok_or("give me the back office's code")?;
    let host = base.trim_start_matches("http://").to_owned();

    let till_side: EnrolResponse = post(
        &host,
        "/v1/enrol",
        None,
        &EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: till_code,
        },
    )?;
    let owner_side: EnrolResponse = post(
        &host,
        "/v1/enrol",
        None,
        &EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: owner_code,
        },
    )?;

    // The owner says which of the shop's goods are which. Nothing here decides
    // that; it is the revenue's word and the shop's to set.
    for (id, code, name, price_minor, supply, said) in [
        (
            9_001_u128,
            "MILK1",
            "Fresh milk, one litre",
            8_000_i64,
            1_u8,
            "zero rated",
        ),
        (
            9_002,
            "BOOK1",
            "A school exercise book",
            5_000,
            2,
            "exempt",
        ),
    ] {
        let _: CatalogueEditResponse = post(
            &host,
            "/v1/back-office/catalogue/upsert",
            Some(&owner_side.token),
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: owner_side.tenant,
                terminal: owner_side.terminal,
                item: ItemWire {
                    id,
                    code: code.to_owned(),
                    name_en: name.to_owned(),
                    name_bn: name.to_owned(),
                    unit: String::from("Nos"),
                    price_minor,
                    cost_minor: 0,
                    // A rate left on the item, which is the case worth walking:
                    // the classification decides, and the two must not be able
                    // to disagree.
                    vat_bp: 1_500,
                    price_inclusive: false,
                    vat_on_undiscounted: false,
                    barcodes: vec![format!("869000000{id}")],
                    on_hand_milli: 100_000,
                    active: true,
                    from_a_till: false,
                    supply,
                    // Sorted under the shop's own word for it, which comes
                    // back down to every till with the rest of the item.
                    category: String::from(if supply == 1 { "Dairy" } else { "Stationery" }),
                },
                expected_seq: 0,
            },
        )?;
        println!("the owner says {name} is {said}");
    }

    // A till pulls the shop and rings one of each.
    let terminal = Ulid::from_u128(till_side.terminal);
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        till_side.tenant,
        terminal,
        1,
        CartLimits::unrestricted(),
    )?;
    let page: PullResponse = post(
        &host,
        "/v1/sync/pull",
        Some(&till_side.token),
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            cursor: 0,
            limit: 200,
        },
    )?;
    till.apply_pull(&deltas_from_pull(&page))?;

    // What the till received, in the shop's own words: the classification and
    // the sorting travel with the item rather than staying in the back office.
    for item in till.catalogue().items() {
        if !item.category.is_empty() {
            println!(
                "the till holds {} under {}, {}",
                item.name_en,
                item.category,
                match item.supply {
                    openpos_core::domain::Supply::ZeroRated => "zero rated",
                    openpos_core::domain::Supply::Exempt => "exempt",
                    openpos_core::domain::Supply::Standard => "taxed",
                }
            );
        }
    }

    let taxed = till
        .catalogue()
        .items()
        .iter()
        .find(|item| item.supply == openpos_core::domain::Supply::Standard)
        .ok_or("the demo shop has something taxed in it")?
        .id;
    till.add(taxed, Milli::ONE)?;
    till.add(Ulid::from_u128(9_001), Milli::ONE)?;
    till.add(Ulid::from_u128(9_002), Milli::new(2_000))?;

    let totals = till.totals()?;
    println!(
        "the basket: net {}, tax {}, total {}",
        totals.net_total.get(),
        totals.vat_total.get(),
        totals.total.get()
    );

    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: totals.total,
        reference: None,
    }, 0)?;
    till.checkout(Ulid::from_u128(9_100), now_ms())?;

    let pending = till.pending_sales(10)?;
    let taken: PushResponse = post(
        &host,
        "/v1/sync/push",
        Some(&till_side.token),
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            sales: pending.iter().map(envelope_for).collect(),
        },
    )?;
    println!(
        "the shop took {} sale(s) and held {}",
        taken.accepted.len(),
        taken.quarantined.len()
    );

    // And the return, recomputed from the lines by the shop rather than read
    // from anything the till asserted about tax.
    let declared: VatResponse = post(
        &host,
        "/v1/back-office/vat",
        Some(&owner_side.token),
        &VatRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: u64::MAX,
        },
    )?;
    println!("what the shop would declare:");
    for row in &declared.rows {
        let kind = match row.supply {
            1 => "zero rated".to_owned(),
            2 => "exempt".to_owned(),
            _ => format!("taxed at {} basis points", row.vat_bp),
        };
        println!(
            "  {kind}: {} sold, {} tax",
            row.net_minor, row.vat_minor
        );
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn post<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    host: &str,
    path: &str,
    token: Option<&str>,
    body: &T,
) -> Result<R, Box<dyn std::error::Error>> {
    let bytes = postcard::to_allocvec(body)?;
    let mut stream = TcpStream::connect(host)?;
    let auth = token.map_or(String::new(), |value| {
        format!("authorization: Bearer {value}\r\n")
    });
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: {host}\r\ncontent-length: {}\r\n{auth}connection: close\r\n\r\n",
        bytes.len()
    )?;
    stream.write_all(&bytes)?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the server sent no headers")?;
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default());
    let status = head.lines().next().unwrap_or_default().to_owned();
    if !status.contains("200") {
        return Err(format!("{path} answered: {status}").into());
    }
    let body = raw
        .get(split.saturating_add(4)..)
        .ok_or("the server sent headers and nothing after them")?;
    Ok(postcard::from_bytes(body)?)
}
