//! A till counts its drawer and hands it to the shop, and nothing else.
//!
//! For walking the back office by hand: the counted-drawer list is the one
//! screen that needs a drawer already closed before there is anything to look
//! at, and the whole of `counted_drawer` wants an owner's code as well because
//! it reads the answer back itself. This one only pushes.
//!
//! ```text
//! cargo run -p openpos-server --example push_a_drawer -- http://127.0.0.1:8099 TILL_CODE
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(clippy::expect_used, clippy::print_stdout)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::auth::{Operator, Permissions, PinHash};
use openpos_core::cart::{CartLimits, Tender, TenderKind};
use openpos_core::ids::Ulid;
use openpos_core::money::{Milli, Minor};
use openpos_core::protocol::{
    ClosedShiftWire, EnrolRequest, EnrolResponse, PROTOCOL_VERSION, PullRequest, PullResponse,
    PushShiftsRequest, PushShiftsResponse,
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
    // A drawer is stored once against its id, so walking this twice against one
    // shop needs a second id rather than a second run of the first.
    let shift_id: u128 = args
        .next()
        .and_then(|given| given.parse().ok())
        .unwrap_or(510);
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
    till.put_operator(Operator {
        id: Ulid::from_u128(91),
        name: "Rahima".into(),
        pin: PinHash::derive("4321", [7; openpos_core::auth::SALT_LEN], 1_000),
        permissions: Permissions::supervisor(),
        active: true,
    })?;
    till.sign_in(Ulid::from_u128(91), "4321", 0)?;
    till.open_shift(Ulid::from_u128(shift_id), Minor::new(30_000), 1_000)?;

    // One cash sale, rung and kept: this device does not send it. That is the
    // ordinary shape of a disagreement between what a till expected in its
    // drawer and what the shop's own sales come to, and the back office should
    // say so rather than treating it as a wrong.
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
    till.add(item, Milli::ONE)?;
    let total = till.totals()?.total;
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: total,
        reference: None,
    }, 0)?;
    till.checkout(Ulid::from_u128(9_200), 1_500)?;
    println!("rang {} and kept it on the device", total.get());

    // Counted at closing, and the count agrees with the till exactly: the
    // shortfall the shop sees is only the sale it has not been told about.
    let expected = till
        .shift()
        .ok_or("a drawer is open")?
        .expected_cash()?
        .get();
    let report = till.close_shift(Minor::new(expected), 3_000)?;
    println!(
        "counted {}, out by {}",
        report.counted_cash.get(),
        report.variance.get()
    );

    let pushed: PushShiftsResponse = post(
        &host,
        "/v1/sync/shifts",
        Some(&till_side.token),
        &PushShiftsRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            shifts: till
                .unsent_shifts()
                .iter()
                .map(|shift| ClosedShiftWire {
                    id: shift.id,
                    terminal: till_side.terminal,
                    closed_by: shift.closed_by,
                    closed_by_name: shift.closed_by_name.clone(),
                    opened_at_ms: shift.opened_at_ms,
                    closed_at_ms: shift.closed_at_ms,
                    opening_float_minor: shift.opening_float_minor,
                    sales: shift.sales,
                    cash_sales_minor: shift.cash_sales_minor,
                    non_cash_sales_minor: shift.non_cash_sales_minor,
                    cash_in_minor: shift.cash_in_minor,
                    cash_out_minor: shift.cash_out_minor,
                    expected_cash_minor: shift.expected_cash_minor,
                    expected_from_sales_minor: None,
                    counted_cash_minor: shift.counted_cash_minor,
                    variance_minor: shift.variance_minor,
                })
                .collect(),
        },
    )?;
    println!("the shop took {} of them", pushed.accepted.len());
    Ok(())
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
