//! What a full day offline costs to send.
//!
//! A shop with no internet since morning has a device holding hundreds of
//! sales, and the thing nobody had measured is how long they take to reach the
//! shop when the line comes back. The design says one transaction per sale and
//! twenty-five sales per request; this says what that is worth in seconds
//! against a real Postgres, and whether the cost per sale stays flat as the
//! batch grows or bends upwards.
//!
//! Not over mobile data, which is the number that matters in a shop and cannot
//! be measured on a desk. This measures the server, which is the part that can
//! be: what a slow line adds to it is the line's own round trips.
//!
//! ```text
//! cargo run --release -p openpos-server --example long_day -- http://127.0.0.1:8098 TILL_CODE [SALES]
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::Milli;
use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse, PROTOCOL_VERSION, PullRequest,
    PullResponse, PushRequest, PushResponse,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::driver::PUSH_BATCH;
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_core::till::Till;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:8098".to_owned());
    let till_code = args.next().ok_or("give me the till's enrolment code")?;
    // A busy neighbourhood shop rings a few hundred sales in a day.
    let wanted: usize = args
        .next()
        .map_or(Ok(300), |given| given.parse())
        .map_err(|_| "how many sales?")?;
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
    let item = till
        .catalogue()
        .items()
        .first()
        .ok_or("the demo shop has nothing to sell")?
        .id;

    // Numbers enough for the day, taken the way a till takes them.
    let lease: LeaseResponse = post(
        &host,
        "/v1/lease",
        Some(&till_side.token),
        &LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            count: u32::try_from(wanted).unwrap_or(u32::MAX) + 10,
        },
    )?;
    till.grant_lease(&openpos_core::lease::Lease::new(
        Ulid::from_u128(till_side.terminal),
        lease.epoch,
        &lease.prefix,
        lease.first,
        lease.last,
    ))?;

    // The day itself, with nothing sent.
    let rang = Instant::now();
    for index in 0..wanted {
        till.add(item, Milli::ONE)?;
        let total = till.totals()?.total;
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: total,
            reference: None,
        })?;
        till.checkout(
            Ulid::from_u128(900_000 + index as u128),
            1_788_600_000_000 + index as u64,
        )?;
    }
    let ringing = rang.elapsed();
    // On a memory store, so this is the arithmetic and nothing else. A real
    // device flushes each sale to its own storage before the cashier is told it
    // is done, which is the number that decides whether a queue moves, and it
    // is measured where that storage is: in the browser, by the till itself.
    println!(
        "rang {wanted} sales into memory in {:.0} ms, {:.3} ms each (no storage flush)",
        ringing.as_secs_f64() * 1_000.0,
        ringing.as_secs_f64() * 1_000.0 / wanted as f64
    );

    // And the line comes back.
    let drain = Instant::now();
    let mut batches = 0_usize;
    let mut sent = 0_usize;
    let mut slowest = 0.0_f64;
    loop {
        let waiting = till.pending_sales(PUSH_BATCH)?;
        if waiting.is_empty() {
            break;
        }
        let batch = Instant::now();
        let accepted: PushResponse = post(
            &host,
            "/v1/sync/push",
            Some(&till_side.token),
            &PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: till_side.tenant,
                terminal: till_side.terminal,
                sales: waiting.iter().map(envelope_for).collect(),
            },
        )?;
        let took = batch.elapsed().as_secs_f64() * 1_000.0;
        slowest = slowest.max(took);
        batches += 1;
        sent += accepted.accepted.len();
        let ids: Vec<Ulid> = accepted
            .accepted
            .iter()
            .map(|id| Ulid::from_u128(*id))
            .collect();
        till.acknowledge(&ids)?;
    }
    let took = drain.elapsed();
    println!(
        "sent {sent} in {batches} batch(es) of {PUSH_BATCH} in {:.2} s",
        took.as_secs_f64()
    );
    println!(
        "  {:.1} ms per batch, {:.2} ms per sale, slowest batch {slowest:.0} ms",
        took.as_secs_f64() * 1_000.0 / batches as f64,
        took.as_secs_f64() * 1_000.0 / sent as f64
    );
    println!(
        "  {:.0} sales a second on this machine, over a loopback socket",
        sent as f64 / took.as_secs_f64()
    );
    println!(
        "  a real shop adds its line's round trip to each of those {batches} batches, which is what",
    );
    println!("  decides the wait: the server is not the slow part here");
    Ok(())
}

/// One postcard request over a socket, as in the other examples here.
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
