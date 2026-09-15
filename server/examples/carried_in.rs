//! A till the shop will not take sales from, and the way its money gets home.
//!
//! This is the state nothing could fix before: a device holding sales it cannot
//! send, because its terminal was deleted or because it has to be enrolled again
//! as another and would abandon what it is holding. Its outbox is the only
//! record of goods that left the shop.
//!
//! Here it is on purpose: the till enrols, rings sales, and then the shop
//! deletes it. The push is refused, the sales are read off the device, and the
//! owner takes them in by hand.
//!
//! ```text
//! cargo run -p openpos-server --example carried_in -- http://127.0.0.1:8098 TILL_CODE OWNER_CODE
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

use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::Milli;
use openpos_core::protocol::{
    AdoptSalesRequest, AdoptSalesResponse, EnrolRequest, EnrolResponse, PROTOCOL_VERSION,
    PullRequest, PullResponse, PushRequest, RepairQueueRequest, RepairQueueResponse, SaleEnvelope,
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
    for id in [900_u128, 901] {
        till.add(item, Milli::ONE)?;
        let total = till.totals()?.total;
        till.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: total,
            reference: None,
        }, 0)?;
        till.checkout(Ulid::from_u128(id), now_ms())?;
    }
    let carried = till.carried_out(50)?;
    println!(
        "the till is holding {} sale(s), {} in all",
        carried.len(),
        carried
            .iter()
            .map(|sale| sale.total_minor)
            .fold(0_i64, i64::saturating_add)
    );

    // The shop cuts the device off, which is what an owner does the moment a
    // tablet is lost. Every credential it holds stops working.
    let cut: openpos_core::protocol::RevokeTerminalResponse = post(
        &host,
        "/v1/back-office/terminals/revoke",
        Some(&owner_side.token),
        &openpos_core::protocol::RevokeTerminalRequest {
            protocol: PROTOCOL_VERSION,
            terminal: till_side.terminal,
        },
    )?;
    println!(
        "the shop cut it off: {} credential(s) withdrawn",
        cut.withdrawn
    );

    // So the ordinary way is shut: the push comes back refused and the till has
    // nowhere else to put what it is holding.
    let refused = post::<_, openpos_core::protocol::PushResponse>(
        &host,
        "/v1/sync/push",
        Some(&till_side.token),
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            sales: till.pending_sales(50)?.iter().map(envelope_for).collect(),
        },
    );
    println!(
        "pushing as a withdrawn device: {}",
        match refused {
            Ok(_) => "accepted, which it should not have been".to_owned(),
            Err(error) => format!("{error}"),
        }
    );

    // So somebody reads them off the device and the owner takes them in.
    let taken: AdoptSalesResponse = post(
        &host,
        "/v1/back-office/sales/adopt",
        Some(&owner_side.token),
        &AdoptSalesRequest {
            protocol: PROTOCOL_VERSION,
            terminal: till_side.terminal,
            sales: carried
                .iter()
                .map(|sale| SaleEnvelope {
                    id: sale.id.to_u128(),
                    schema: sale.schema,
                    payload: sale.payload.clone(),
                })
                .collect(),
        },
    )?;
    println!(
        "the shop took in {} sale(s), {} of them waiting on a person",
        taken.adopted.len(),
        taken.needing_attention.len()
    );

    let queue: RepairQueueResponse = post(
        &host,
        "/v1/back-office/repairs",
        Some(&owner_side.token),
        &RepairQueueRequest {
            protocol: PROTOCOL_VERSION,
            tenant: owner_side.tenant,
            terminal: owner_side.terminal,
            limit: 50,
        },
    )?;
    for entry in &queue.entries {
        println!("  waiting: {} because {}", entry.total_minor, entry.reason);
    }
    Ok(())
}

/// The clock a real till rings at: this machine's own.
///
/// Fixed timestamps read well in an example and are a lie the server now
/// catches: a shop created a minute ago cannot have sales from Tuesday, and one
/// of the two impossibilities the ingest holds a sale for is exactly that.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
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
