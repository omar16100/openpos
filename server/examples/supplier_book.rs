//! What the shop owes its suppliers, and paying one.
//!
//! The other half of the account book. A shop here takes stock on credit and
//! settles on a day of the week: the distributor's man comes on Saturday and is
//! paid for what came in since the last one. Deliveries have been recorded since
//! purchasing existed and nothing was ever recorded against them.
//!
//! ```text
//! cargo run -p openpos-server --example supplier_book -- http://127.0.0.1:8098 OWNER_CODE
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

use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, PROTOCOL_VERSION, PaySupplierRequest, PaySupplierResponse,
    PullRequest, PullResponse, PutSupplierRequest, ReceiptLineWire, ReceiveGoodsRequest,
    ReceiveGoodsResponse, SupplierOwingRequest, SupplierOwingResponse, SupplierStatementRequest,
    SupplierStatementResponse, SupplierWire, SuppliersResponse,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:8098".to_owned());
    let owner_code = args.next().ok_or("give me the back office's code")?;
    let host = base.trim_start_matches("http://").to_owned();

    let owner: EnrolResponse = post(
        &host,
        "/v1/enrol",
        None,
        &EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: owner_code,
        },
    )?;

    // Something to book in, taken from the shop's own catalogue.
    let page: PullResponse = post(
        &host,
        "/v1/sync/pull",
        Some(&owner.token),
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: owner.tenant,
            terminal: owner.terminal,
            cursor: 0,
            limit: 10,
        },
    )?;
    let item = page
        .upserts
        .first()
        .ok_or("the demo shop has nothing to sell")?
        .id;

    let distributor = 4_242_u128;
    let _: SuppliersResponse = post(
        &host,
        "/v1/back-office/suppliers/put",
        Some(&owner.token),
        &PutSupplierRequest {
            protocol: PROTOCOL_VERSION,
            supplier: SupplierWire {
                id: distributor,
                name: "Mirpur Distributors".to_owned(),
                phone: Some("01711000000".to_owned()),
                bin: None,
                active: true,
            },
        },
    )?;

    // A delivery on credit: forty bags at 344.00, and a part case whose cost
    // lands on half a poisha, which is where two stores' arithmetic can differ.
    for (qty_milli, unit_cost_minor, id) in
        [(40_000_i64, 34_400_i64, 5_001_u128), (1_500, 4_333, 5_002)]
    {
        let _: ReceiveGoodsResponse = post(
            &host,
            "/v1/back-office/stock/receive",
            Some(&owner.token),
            &ReceiveGoodsRequest {
                protocol: PROTOCOL_VERSION,
                id,
                supplier_id: Some(distributor),
                reference: Some("CH-1".to_owned()),
                received_at_ms: 1_788_600_000_000,
                note: None,
                lines: vec![ReceiptLineWire {
                    item_id: item,
                    qty_milli,
                    unit_cost_minor,
                }],
            },
        )?;
    }

    let owing: SupplierOwingResponse = post(
        &host,
        "/v1/back-office/suppliers/owed",
        Some(&owner.token),
        &SupplierOwingRequest {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    for one in &owing.owing {
        println!(
            "the shop owes {} {} over {} delivery(ies)",
            one.name, one.owed_minor, one.deliveries
        );
    }

    // Saturday. Most of it is handed over, and the reply is dropped, so the
    // same payment is sent again.
    let mut paid: PaySupplierResponse = post(
        &host,
        "/v1/back-office/suppliers/payment",
        Some(&owner.token),
        &PaySupplierRequest {
            protocol: PROTOCOL_VERSION,
            id: 6_000,
            supplier_id: distributor,
            amount_minor: 1_000_000,
            paid_at_ms: 1_788_900_000_000,
            note: Some("in cash, Saturday".to_owned()),
        },
    )?;
    println!(
        "paid: recorded {}, still owes {}",
        paid.paid, paid.owed_minor
    );
    paid = post(
        &host,
        "/v1/back-office/suppliers/payment",
        Some(&owner.token),
        &PaySupplierRequest {
            protocol: PROTOCOL_VERSION,
            id: 6_000,
            supplier_id: distributor,
            amount_minor: 1_000_000,
            paid_at_ms: 1_788_900_000_000,
            note: Some("in cash, Saturday".to_owned()),
        },
    )?;
    println!(
        "sent again: recorded {}, still owes {}",
        paid.paid, paid.owed_minor
    );
    // And what the two of them put side by side when their figures disagree.
    let statement: SupplierStatementResponse = post(
        &host,
        "/v1/back-office/suppliers/statement",
        Some(&owner.token),
        &SupplierStatementRequest {
            protocol: PROTOCOL_VERSION,
            supplier_id: distributor,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
        },
    )?;
    println!("the statement, oldest first:");
    for line in &statement.entries {
        println!(
            "  {} {} {}",
            line.at_ms,
            if line.delivered { "goods in" } else { "paid" },
            line.amount_minor
        );
    }
    println!("and it ends owing {}", statement.owed_minor);
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
