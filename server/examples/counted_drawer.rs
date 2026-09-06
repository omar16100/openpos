//! A drawer counted at a till, read by the owner in the back office.
//!
//! The point of counting a drawer is that somebody who was not standing at the
//! till reconciles it, so the whole path only means anything end to end: the
//! till stamps whoever counted, the count survives being written down, and the
//! owner reads a name rather than a terminal id and a timestamp.
//!
//! Two enrolment codes, because a till may report its own drawers and only the
//! back office may read the shop's.
//!
//! ```text
//! cargo run -p openpos-server --example counted_drawer -- http://127.0.0.1:8098 TILL_CODE OWNER_CODE
//! ```

// A tool run by hand against a local server. It panics on anything unexpected
// on purpose: there is nobody to hand an error to, and a stack trace is more use
// here than a message. The workspace bans this in the code that runs a shop.
#![allow(clippy::expect_used, clippy::print_stdout)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::auth::{Operator, Permissions, PinHash};
use openpos_core::cart::CartLimits;
use openpos_core::ids::Ulid;
use openpos_core::money::Minor;
use openpos_core::protocol::{
    ClosedShiftWire, EnrolRequest, EnrolResponse, PushShiftsRequest, PushShiftsResponse,
    ShiftsRequest, ShiftsResponse, PROTOCOL_VERSION,
};
use openpos_core::storage::backend::MemoryBackend;
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

    // A cashier signs in, opens the drawer with a float, and counts it forty
    // taka short of what the till expected.
    let terminal = Ulid::from_u128(till_side.terminal);
    let (mut till, _) = Till::open(
        MemoryBackend::new(),
        till_side.tenant,
        terminal,
        1,
        CartLimits::unrestricted(),
    )?;
    till.put_operator(cashier())?;
    till.sign_in(Ulid::from_u128(91), "4321", 0)?;
    till.open_shift(Ulid::from_u128(500), Minor::new(30_000), 1_000)?;
    let report = till.close_shift(Minor::new(26_000), 3_000)?;
    println!(
        "at the till: counted {:?}, out by {:?}",
        report.counted_cash, report.variance
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
                    counted_cash_minor: shift.counted_cash_minor,
                    variance_minor: shift.variance_minor,
                })
                .collect(),
        },
    )?;
    println!("the shop took {} of them", pushed.accepted.len());

    // And the owner reads it back, which is the half that was missing: a name
    // rather than a terminal and a time.
    let seen: ShiftsResponse = post(
        &host,
        "/v1/back-office/shifts",
        Some(&owner_side.token),
        &ShiftsRequest {
            protocol: PROTOCOL_VERSION,
            limit: 10,
        },
    )?;
    for shift in &seen.shifts {
        println!(
            "in the back office: counted by {}, expected {}, counted {}, out by {}",
            if shift.closed_by_name.is_empty() {
                "nobody the till wrote down"
            } else {
                &shift.closed_by_name
            },
            shift.expected_cash_minor,
            shift.counted_cash_minor,
            shift.variance_minor,
        );
    }
    Ok(())
}

fn cashier() -> Operator {
    Operator {
        id: Ulid::from_u128(91),
        name: "Rahima".into(),
        pin: PinHash::derive("4321", [7_u8; 16], 1_000),
        permissions: Permissions {
            max_discount_bp: 0,
            may_override_price: false,
            may_refund: false,
            may_void_line: true,
            may_authorise: false,
            may_open_drawer: true,
            may_close_shift: true,
        },
        active: true,
    }
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
