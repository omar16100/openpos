//! A till restored from yesterday's backup, which is how duplicate receipt
//! numbers happen in a real shop.
//!
//! The repair queue, the duplicate-receipt check and the totals check are the
//! three paths that only appear when something has gone wrong, and none of them
//! can be reached from a browser: a screen cannot ring a sale twice under one
//! number, and it cannot tamper with a payload the core just wrote.
//!
//! So this is a client, not a back door. It enrols with an ordinary code, pushes
//! ordinary sales over the ordinary endpoint, and the only unusual thing about
//! it is that it behaves like a device somebody restored from a backup. Nothing
//! here exists in the server, and nothing here can be switched on in a shop.
//!
//! ```text
//! cargo run -p openpos-server --example restored_till -- http://127.0.0.1:8099 CODE
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(clippy::expect_used, clippy::print_stdout)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli, Minor};
use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, PushRequest, PushResponse, SaleEnvelope, PROTOCOL_VERSION,
};
use openpos_core::replica::Item;
use openpos_core::storage::wire::{encode_sale, sale_commit, SALE_SCHEMA};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base = args.next().unwrap_or_else(|| "http://127.0.0.1:8099".to_owned());
    let code = args
        .next()
        .ok_or("give me an enrolment code: the one the demo printed for a till")?;

    let host = base.trim_start_matches("http://").to_owned();

    let reply: EnrolResponse = post(
        &host,
        "/v1/enrol",
        None,
        &EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code,
        },
    )?;
    println!("enrolled as terminal {}", Ulid::from_u128(reply.terminal).encode());

    // Two sales under one receipt number. A till restored from a backup has
    // forgotten which numbers it already used, so it hands out numbers the shop
    // has already printed on paper in somebody's hand.
    let receipt = "T1-000100";
    let sales = vec![
        envelope(reply.terminal, 9_001, Some(receipt), None),
        envelope(reply.terminal, 9_002, Some(receipt), None),
        // And one whose stored total disagrees with its lines, which is what
        // corruption or tampering looks like from the server's side.
        envelope(reply.terminal, 9_003, Some("T1-000101"), Some(1)),
    ];

    let pushed: PushResponse = post(
        &host,
        "/v1/sync/push",
        Some(&reply.token),
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: reply.tenant,
            terminal: reply.terminal,
            sales,
        },
    )?;

    println!("accepted: {}", pushed.accepted.len());
    for entry in &pushed.quarantined {
        println!("needs a look: {:?}", entry.reason);
    }
    println!("\nOpen the back office. The queue should have two entries in it.");
    Ok(())
}

/// One sale, optionally with its stored total rewritten.
fn envelope(
    terminal: u128,
    id: u128,
    receipt: Option<&str>,
    wrong_total: Option<i64>,
) -> SaleEnvelope {
    let mut cart = Cart::new(CartLimits::unrestricted());
    cart.add_item(&item(), Milli::ONE).expect("a cart takes an item");
    cart.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(50_000),
        reference: None,
    });
    let mut ticket = cart
        .close(
            Ulid::from_u128(id),
            Ulid::from_u128(terminal),
            1_788_600_000_000,
        )
        .expect("a cart with a line and a tender closes");
    ticket.receipt_no = receipt.map(Into::into);

    let mut sale = sale_commit(&ticket, receipt.map(|_| 1), Some(101));
    if let Some(total) = wrong_total {
        sale.ticket.total_minor = total;
    }
    SaleEnvelope {
        id,
        schema: SALE_SCHEMA,
        payload: encode_sale(&sale).expect("a sale encodes"),
    }
}

fn item() -> Item {
    Item {
        id: Ulid::from_u128(1),
        code: "RICE5".into(),
        name_en: "Rice Miniket 5kg".into(),
        name_bn: "মিনিকেট চাল ৫ কেজি".into(),
        unit: "Nos".into(),
        price: Minor::new(43_000),
        cost: Minor::new(38_000),
        vat_rate: Bp::new(1_500).expect("fifteen percent"),
        price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
        vat_base: openpos_core::domain::pricing::VatBase::Discounted,
        barcodes: vec!["8690000000001".into()],
        on_hand: Milli::new(40_000),
        active: true,
    }
}

/// One postcard request over a socket.
///
/// Written by hand rather than pulling in an HTTP client, because this is forty
/// lines against a local server and a dependency here is a dependency in the
/// crate that runs the shop.
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
