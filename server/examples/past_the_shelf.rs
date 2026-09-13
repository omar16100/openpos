//! What a shop's stock rule does at the counter, against a running server.
//!
//! Two devices, because that is the shape of the thing: an owner in the back
//! office decides what a till should do when a basket asks for more than the
//! shelf holds, and a till three miles away does it with the line down. Neither
//! half means anything alone, and the setting is no use until somebody watches a
//! cashier be stopped by it.
//!
//! ```text
//! cargo run -p openpos-server --example past_the_shelf -- http://127.0.0.1:8099 TILL_CODE OWNER_CODE
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::auth::{Action, Operator, Permissions, PinHash, SALT_LEN};
use openpos_core::cart::CartLimits;
use openpos_core::domain::StockRule;
use openpos_core::ids::Ulid;
use openpos_core::money::Milli;
use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, PROTOCOL_VERSION, PullRequest, PullResponse, PutShopRequest,
    ShopRequest, ShopResponse,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::deltas_from_pull;
use openpos_core::till::Till;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:8099".to_owned());
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

    // The back office. Somebody decides the figures are worth trusting.
    let was: ShopResponse = post(
        &host,
        "/v1/shop",
        Some(&till_side.token),
        &ShopRequest {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    println!("the shop was set to {}", in_words(was.stock_rule));
    let now: ShopResponse = post(
        &host,
        "/v1/back-office/shop",
        Some(&owner_side.token),
        &PutShopRequest {
            protocol: PROTOCOL_VERSION,
            name: was.name.clone(),
            bin: was.bin.clone(),
            address: was.address.clone(),
            phone: was.phone.clone(),
            wallets: was.wallets.clone(),
            stock_rule: 2,
            languages: Vec::new(),
            tax_status: 0,
        },
    )?;
    println!("the owner set it to {}", in_words(now.stock_rule));

    // The till. It learns the rule the way it learns the shop's name.
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        till_side.tenant,
        Ulid::from_u128(till_side.terminal),
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
            limit: 100,
        },
    )?;
    till.apply_pull(&deltas_from_pull(&page))?;
    let details: ShopResponse = post(
        &host,
        "/v1/shop",
        Some(&till_side.token),
        &ShopRequest {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    till.set_shop(
        openpos_core::receipt::Shop {
            name: details.name.clone(),
            bin: details.bin.clone(),
            address: details.address.clone(),
            phone: details.phone.clone(),
        },
        details
            .wallets
            .iter()
            .map(|one| one.as_str().into())
            .collect(),
        StockRule::from_u8(details.stock_rule),
        details
            .languages
            .iter()
            .map(|one| one.as_str().into())
            .collect(),
        openpos_core::domain::TaxStatus::from_u8(details.tax_status),
    )?;
    println!(
        "the till fetched the shop and reads the rule as {}",
        in_words(details.stock_rule)
    );

    // Somebody at the counter. A supervisor, so this example can also show one
    // allowing it: a shop this size is often one person doing both.
    let supervisor = Ulid::from_u128(70);
    till.put_operator(Operator {
        id: supervisor,
        name: "Karim".into(),
        pin: PinHash::derive("9999", [5; SALT_LEN], 1_000),
        permissions: Permissions::supervisor(),
        active: true,
    })?;
    till.sign_in(supervisor, "9999", now_ms())?;

    // What the shop believes is on the shelves. Not the catalogue's copy of it:
    // that number is whatever somebody last typed on an item record and never
    // moves, and deciding a refusal on it would refuse a shop's whole day.
    let shelves: openpos_core::protocol::OnHandResponse = post(
        &host,
        "/v1/stock",
        Some(&till_side.token),
        &openpos_core::protocol::OnHandRequest {
            protocol: PROTOCOL_VERSION,
            item_ids: till
                .item_window(0, 200)
                .into_iter()
                .map(|id| id.to_u128())
                .collect(),
        },
    )?;
    let taken = till.apply_on_hand(
        &shelves
            .on_hand
            .iter()
            .map(|entry| (Ulid::from_u128(entry.item_id), Milli::new(entry.qty_milli)))
            .collect::<Vec<_>>(),
    );
    println!("the till asked what the shelves hold and took {taken} figures");
    // One window of two hundred over a shop of eighteen items is the whole
    // shelf, so that was a lap of it. A till says nothing about the shelf until
    // it has been round once, because until then it holds a figure for the
    // items whose turn has come and nothing for the rest, and nothing reads as
    // none: a till enrolled this morning would otherwise refuse everything
    // scanned at it. The driver says this in the app; here this walk is the
    // driver.
    till.shelf_swept();

    // Whatever the shop actually stocks, taken from the catalogue it just
    // pulled rather than assumed: this runs against a demo shop today and
    // should run against a real one tomorrow.
    let item = till
        .item_window(0, 200)
        .into_iter()
        .filter_map(|id| till.catalogue().by_id(id))
        .find(|found| found.on_hand.get() > 0 && !found.barcodes.is_empty())
        .ok_or("this shop holds none of anything, so there is nothing to be stopped from selling")?
        .clone();
    let barcode = item
        .barcodes
        .first()
        .ok_or("that item has no barcode")?
        .to_string();
    let on_hand = item.on_hand.get();
    println!();
    println!(
        "the shelf holds {} {}",
        quantity(on_hand),
        item.name_en.as_ref()
    );

    // Everything it has: fine.
    till.scan(&barcode, Milli::new(on_hand))?;
    println!("  rang {}: taken", quantity(on_hand));

    // One more: not.
    match till.scan(&barcode, Milli::new(1_000)) {
        Ok(_) => println!("  rang one more: TAKEN, which is the rule not working"),
        Err(refusal) => println!("  rang one more: refused, \"{refusal}\""),
    }

    // The supervisor allows it, for this basket.
    till.authorise(
        supervisor,
        "9999",
        Action::SellBeyondStock,
        now_ms(),
        60_000,
    )?;
    till.scan(&barcode, Milli::new(1_000))?;
    println!("  the supervisor allowed it: taken");
    for short in till.beyond_the_shelf() {
        println!(
            "  and the screen still says so: {} has {}, this basket wants {}",
            short.name,
            quantity(short.on_hand_milli),
            quantity(short.wanted_milli)
        );
    }

    // Put the shop back the way it was, so running this twice is the same as
    // running it once.
    let _: ShopResponse = post(
        &host,
        "/v1/back-office/shop",
        Some(&owner_side.token),
        &PutShopRequest {
            protocol: PROTOCOL_VERSION,
            name: was.name,
            bin: was.bin,
            address: was.address,
            phone: was.phone,
            wallets: was.wallets,
            stock_rule: was.stock_rule,
            languages: Vec::new(),
            tax_status: 0,
        },
    )?;
    println!();
    println!(
        "the shop is back where it was: {}",
        in_words(was.stock_rule)
    );
    Ok(())
}

fn in_words(rule: u8) -> &'static str {
    match rule {
        1 => "sell it and warn the cashier",
        2 => "refuse it until a supervisor allows it",
        _ => "sell it and say nothing",
    }
}

fn quantity(milli: i64) -> String {
    if milli % 1_000 == 0 {
        return (milli / 1_000).to_string();
    }
    format!("{}.{:03}", milli / 1_000, (milli % 1_000).abs())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
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
