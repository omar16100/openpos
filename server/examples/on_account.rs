//! A sale on account, and settling it, against a running server.
//!
//! Two devices again, because that is what the act is: a till sells a bag of
//! rice to somebody who will pay on Friday, and an owner at the back office
//! takes the money and marks it off. Neither half means anything alone, and the
//! screens are the only other place these two endpoints meet.
//!
//! ```text
//! cargo run -p openpos-server --example on_account -- http://127.0.0.1:8098 TILL_CODE OWNER_CODE
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
use openpos_core::money::{Milli, Minor};
use openpos_core::protocol::{
    AccountRequest, AccountResponse, CustomerWire, CustomersRequest, CustomersResponse, DayRequest,
    DayResponse, EnrolRequest, EnrolResponse, OwedRequest, OwedResponse, PROTOCOL_VERSION,
    PullRequest, PullResponse, PushRequest, PushResponse, PutCustomerRequest, TakePaymentRequest,
    TakePaymentResponse,
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

    // The shop writes Karim down. Two Karims share an account otherwise, and
    // which one owes what is decided by whatever the cashier typed that day.
    let written: CustomersResponse = post(
        &host,
        "/v1/back-office/customers",
        Some(&owner_side.token),
        &PutCustomerRequest {
            protocol: PROTOCOL_VERSION,
            customer: CustomerWire {
                id: 21,
                name: "Karim, flat 3".to_owned(),
                phone: Some("01711000000".to_owned()),
                active: true,
            },
        },
    )?;
    println!("the shop lets {} buy on account", written.customers.len());

    // And the till is told, which is what lets a cashier write a sale to that
    // account with the line down.
    let known: CustomersResponse = post(
        &host,
        "/v1/customers",
        Some(&till_side.token),
        &CustomersRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
        },
    )?;
    till.set_customers(
        known
            .customers
            .iter()
            .map(|one| openpos_core::storage::wire::CustomerV1 {
                id: one.id,
                name: one.name.clone(),
                phone: one.phone.clone(),
                active: one.active,
            })
            .collect(),
    )?;

    // Karim takes a bag of rice and pays a hundred taka of it now. The rest
    // goes in the book, which until now was a book.
    let first = till
        .catalogue()
        .items()
        .first()
        .ok_or("the demo shop has nothing to sell")?
        .id;
    till.add(first, Milli::ONE)?;
    // This basket is his, by the id the shop issued rather than by a spelling.
    till.set_customer(Some(Ulid::from_u128(21)))?;
    let total = till.totals()?.total.get();
    till.add_tender(Tender {
        kind: TenderKind::Cash,
        amount: Minor::new(10_000),
        reference: None,
    });
    till.add_tender(Tender {
        kind: TenderKind::Credit,
        amount: Minor::new(total - 10_000),
        // Spelled carelessly on purpose: what he owes is added against the
        // person, and this is only what the receipt in his hand says.
        reference: Some("karim".into()),
    });
    till.checkout(Ulid::from_u128(900), 1_788_600_000_000)?;

    let pending = till.pending_sales(10)?;
    let pushed: PushResponse = post(
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
        "the till sold {} and sent {} sale(s)",
        Minor::new(total).get(),
        pushed.accepted.len()
    );

    let book: OwedResponse = post(
        &host,
        "/v1/back-office/owed",
        Some(&owner_side.token),
        &OwedRequest {
            protocol: PROTOCOL_VERSION,
            limit: 50,
        },
    )?;
    for person in &book.owing {
        println!(
            "in the back office: {} owes {}",
            person.person_name, person.owed_minor
        );
    }
    let person = book.owing.first().ok_or("nobody owes anything")?;

    // Friday. He hands over a hundred, and the reply is dropped, so the owner
    // presses it again.
    let mut taken: TakePaymentResponse = post(
        &host,
        "/v1/back-office/owed/payment",
        Some(&owner_side.token),
        &TakePaymentRequest {
            protocol: PROTOCOL_VERSION,
            id: 5_000,
            person_key: person.person_key.clone(),
            person_name: person.person_name.clone(),
            amount_minor: 10_000,
            at_ms: 1_788_900_000_000,
            note: Some("in cash".to_owned()),
            written_off: false,
        },
    )?;
    println!(
        "took a payment: recorded {}, now owes {}",
        taken.taken, taken.owed_minor
    );
    taken = post(
        &host,
        "/v1/back-office/owed/payment",
        Some(&owner_side.token),
        &TakePaymentRequest {
            protocol: PROTOCOL_VERSION,
            id: 5_000,
            person_key: person.person_key.clone(),
            person_name: person.person_name.clone(),
            amount_minor: 10_000,
            at_ms: 1_788_900_000_000,
            note: Some("in cash".to_owned()),
            written_off: false,
        },
    )?;
    println!(
        "sent again: recorded {}, still owes {}",
        taken.taken, taken.owed_minor
    );

    let account: AccountResponse = post(
        &host,
        "/v1/back-office/owed/account",
        Some(&owner_side.token),
        &AccountRequest {
            protocol: PROTOCOL_VERSION,
            person_key: person.person_key.clone(),
            limit: 50,
        },
    )?;
    for line in &account.entries {
        println!(
            "  {} {} {}",
            line.at_ms,
            if line.is_sale { "took goods" } else { "paid" },
            line.amount_minor.abs()
        );
    }

    // And the one question an owner asks at closing, in one call.
    let seen: DayResponse = post(
        &host,
        "/v1/back-office/day",
        Some(&owner_side.token),
        &DayRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 1_788_000_000_000,
            to_ms: 1_789_999_999_999,
        },
    )?;
    println!(
        "the day: {} sale(s) for {}, {} on account, {} paid off, {} struck off, {} drawer(s) counted",
        seen.sales,
        seen.total_minor,
        seen.charged_minor,
        seen.paid_minor,
        seen.written_off_minor,
        seen.drawers_counted
    );
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
