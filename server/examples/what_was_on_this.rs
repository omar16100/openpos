//! Somebody comes back to the counter with a piece of paper.
//!
//! A till rings a basket with a discount on it, the shop takes the sale, and
//! the back office is asked what was on that receipt. What comes back is what
//! the till wrote down at the time, read out of its own bytes: the goods, what
//! came off, the money, and anything given back against it since.
//!
//! ```text
//! cargo run -p openpos-server --example what_was_on_this -- http://127.0.0.1:8099 TILL_CODE OWNER_CODE
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(clippy::expect_used, clippy::print_stdout)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::domain::Discount;
use openpos_core::ids::Ulid;
use openpos_core::money::{Bp, Milli};
use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse, MadeRequest, MadeResponse,
    PROTOCOL_VERSION, PullRequest, PullResponse, PushRequest, PushResponse, ReceiptRequest,
    ReceiptResponse,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::{deltas_from_pull, envelope_for};
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

    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        till_side.tenant,
        Ulid::from_u128(till_side.terminal),
        1,
        CartLimits::unrestricted(),
    )?;

    // A block of receipt numbers, because the whole point is looking one up.
    let block: LeaseResponse = post(
        &host,
        "/v1/lease",
        Some(&till_side.token),
        &LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            count: 50,
        },
    )?;
    till.grant_lease(&openpos_core::lease::Lease {
        terminal: Ulid::from_u128(till_side.terminal),
        epoch: block.epoch,
        prefix: block.prefix.clone().into_boxed_str(),
        next: block.first,
        last: block.last,
    })?;

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
    let item = till
        .catalogue()
        .items()
        .first()
        .ok_or("the demo shop has something to sell")?
        .id;

    // Two of something, with ten percent off the line: a discount expressed as
    // a rate is the case worth walking, because the person at the counter is
    // arguing about the taka that came off rather than the rate.
    till.add(item, Milli::new(2_000))?;
    till.set_line_discount(0, Discount::Rate(Bp::new(1_000)?))?;
    let total = till.totals()?.total;
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: total,
        reference: None,
    })?;
    let rung = till.checkout(Ulid::from_u128(9_400), now_ms())?;
    let printed = rung
        .receipt_no
        .ok_or("the till numbered it from its block")?;
    println!("the till printed {printed} for {}", total.get());

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
        "the shop took {} and held {}",
        taken.accepted.len(),
        taken.quarantined.len()
    );

    // And what the day made, which is the other half of the same sale: the
    // cost travelled with the line from the catalogue, so the shop can say.
    let earned: MadeResponse = post(
        &host,
        "/v1/back-office/made",
        Some(&owner_side.token),
        &MadeRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: u64::MAX,
        },
    )?;
    println!(
        "made {} on {} of selling before tax, against {} the goods cost, over {} sale(s)",
        earned.made_minor, earned.net_minor, earned.cost_minor, earned.sales
    );
    if earned.sales_without_cost > 0 {
        println!(
            "  and {} sale(s) of {} are not in that, having no cost written down",
            earned.sales_without_cost, earned.net_without_cost_minor
        );
    }

    // And somebody brings the paper back.
    let found: ReceiptResponse = post(
        &host,
        "/v1/back-office/receipt",
        Some(&owner_side.token),
        &ReceiptRequest {
            protocol: PROTOCOL_VERSION,
            receipt_no: printed.clone(),
        },
    )?;
    for sale in &found.found {
        println!(
            "under {}: rung at {}, total {}",
            sale.receipt_no, sale.rung_at_ms, sale.total_minor
        );
        for line in &sale.lines {
            println!(
                "  {} {} x {} less {} = {}",
                line.qty_milli, line.unit, line.unit_price_minor, line.discount_minor,
                line.line_total_minor
            );
        }
        for tender in &sale.tenders {
            println!("  paid {} by {}", tender.amount_minor, tender.kind);
        }
        if !sale.held_for.is_empty() {
            println!("  held: {}", sale.held_for);
        }
        println!("  given back against it so far: {}", sale.refunded_minor);
    }
    if found.found.is_empty() {
        println!("nothing carries that number, which is the answer as well");
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
