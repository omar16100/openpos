//! The same basket rung twice, and the shop saying so.
//!
//! A tablet restored from Thursday's backup rings Friday's groceries again. The
//! shop ends up holding two sales for one basket: twice the takings, twice the
//! tax, twice off the shelf, and twice on the customer's account. Until now the
//! repair queue could only take a note about that, and every one of those
//! figures stayed doubled for ever.
//!
//! Here both sales are real to the server, the owner works the queue, and says
//! the second one never happened. Everything that counted it stops. Nothing is
//! deleted: the sale and its bytes stay exactly where they were.
//!
//! ```text
//! cargo run -p openpos-server --example rung_twice -- http://127.0.0.1:8098 TILL_CODE OWNER_CODE
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
    AdoptSalesRequest, AdoptSalesResponse, DayRequest, DayResponse, DecideAgainRequest,
    DecideAgainResponse, DecidedRequest, DecidedResponse, EnrolRequest, EnrolResponse, OwedRequest,
    OwedResponse, PROTOCOL_VERSION, PullRequest, PullResponse, PushRequest, PushResponse,
    RepairQueueRequest, RepairQueueResponse, ResolveRepairRequest, ResolveRepairResponse,
    SaleEnvelope, SoldRequest, SoldResponse, VatRequest, VatResponse,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_core::till::Till;

// The clock a real till rings at, because the shop was created a moment ago
// and a sale it could not have rung is one the server holds for a person.
fn rung_at() -> u64 {
    now_ms()
}

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

    // Karim's groceries, on account, rung and sent.
    ring(&mut till, item, 900)?;
    let sent: PushResponse = post(
        &host,
        "/v1/sync/push",
        Some(&till_side.token),
        &PushRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            sales: till.pending_sales(50)?.iter().map(envelope_for).collect(),
        },
    )?;
    println!("the till sent {} sale(s)", sent.accepted.len());

    // Friday morning: the tablet is restored from Thursday's backup and the
    // same basket goes through again. Carried in by hand here, because that is
    // the shortest honest way to get a second sale in front of a person: it
    // arrives, it is stored, and it waits for somebody to look at it.
    let (mut restored, _) = Till::open(
        MemoryBackend::new(),
        till_side.tenant,
        Ulid::from_u128(till_side.terminal),
        1,
        CartLimits::unrestricted(),
    )?;
    restored.apply_pull(&deltas_from_pull(&page))?;
    ring(&mut restored, item, 901)?;
    let again = restored.carried_out(10)?;
    let taken: AdoptSalesResponse = post(
        &host,
        "/v1/back-office/sales/adopt",
        Some(&owner_side.token),
        &AdoptSalesRequest {
            protocol: PROTOCOL_VERSION,
            terminal: till_side.terminal,
            sales: again
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
        "the same basket came in a second time: {} stored, {} waiting on a person",
        taken.adopted.len(),
        taken.needing_attention.len()
    );

    println!("\nbefore anybody looks at it:");
    report(&host, &owner_side.token)?;

    // The owner works the queue.
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
    let waiting = queue.entries.first().ok_or("nothing is waiting")?;
    println!(
        "\nwaiting: {} because {}",
        waiting.total_minor, waiting.reason
    );

    let struck: ResolveRepairResponse = post(
        &host,
        "/v1/back-office/repairs/resolve",
        Some(&owner_side.token),
        &ResolveRepairRequest {
            protocol: PROTOCOL_VERSION,
            tenant: owner_side.tenant,
            terminal: owner_side.terminal,
            sale: waiting.id,
            note: "the tablet was restored from Thursday and rang it again".to_owned(),
            kept: false,
        },
    )?;
    println!("struck out: {}", struck.resolved);

    println!("\nafter the shop said it never happened:");
    report(&host, &owner_side.token)?;

    // And the morning after: it was the other one that was the duplicate. The
    // entry has left the queue, so the list of what was decided is the way back
    // to it.
    let answered: DecidedResponse = post(
        &host,
        "/v1/back-office/repairs/decided",
        Some(&owner_side.token),
        &DecidedRequest {
            protocol: PROTOCOL_VERSION,
            limit: 50,
        },
    )?;
    for one in &answered.decided {
        println!(
            "\ndecided: {} is {} because \"{}\", answered {} time(s)",
            one.total_minor,
            if one.kept { "counted" } else { "struck out" },
            one.note,
            one.decisions
        );
    }
    let wrong = answered.decided.first().ok_or("nothing was decided")?;

    let put_back: DecideAgainResponse = post(
        &host,
        "/v1/back-office/repairs/decide-again",
        Some(&owner_side.token),
        &DecideAgainRequest {
            protocol: PROTOCOL_VERSION,
            tenant: owner_side.tenant,
            terminal: owner_side.terminal,
            sale: wrong.id,
            note: "wrong one: the other was the duplicate".to_owned(),
            kept: true,
            // What the list showed a moment ago. Another owner answering in
            // between is refused rather than overwritten.
            expected_decisions: wrong.decisions,
        },
    )?;
    println!("put back: {}", put_back.changed);

    println!("\nafter the shop changed its mind:");
    report(&host, &owner_side.token)?;
    Ok(())
}

/// One basket, on Karim's account.
fn ring(
    till: &mut Till<MemoryBackend>,
    item: Ulid,
    id: u128,
) -> Result<(), Box<dyn std::error::Error>> {
    till.add(item, Milli::new(2_000))?;
    let total = till.totals()?.total;
    till.add_tender(Tender {
        kind: TenderKind::Credit,
        amount: total,
        reference: Some("karim".into()),
    }, 0)?;
    till.checkout(Ulid::from_u128(id), rung_at())?;
    Ok(())
}

/// Every figure the duplicate touches, read from the shop's own reports.
fn report(host: &str, token: &str) -> Result<(), Box<dyn std::error::Error>> {
    let day: DayResponse = post(
        host,
        "/v1/back-office/day",
        Some(token),
        &DayRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
        },
    )?;
    println!(
        "  the day: {} sale(s) for {}, {} charged to accounts",
        day.sales, day.total_minor, day.charged_minor
    );

    let book: OwedResponse = post(
        host,
        "/v1/back-office/owed",
        Some(token),
        &OwedRequest {
            protocol: PROTOCOL_VERSION,
            limit: 50,
            // From the top of the list. A screen carries on from where the last
            // page ended instead.
            after_owed_minor: 0,
            after_person_key: String::new(),
        },
    )?;
    for person in &book.owing {
        println!("  {} owes {}", person.person_name, person.owed_minor);
    }

    let tax: VatResponse = post(
        host,
        "/v1/back-office/vat",
        Some(token),
        &VatRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
        },
    )?;
    for row in &tax.rows {
        println!(
            "  tax at {} bp: {} sold, {} tax, over {} sale(s)",
            row.vat_bp, row.net_minor, row.vat_minor, row.sales
        );
    }
    println!(
        "  of which {} is {} sale(s) nobody has looked at yet",
        tax.waiting_vat_minor, tax.waiting_sales
    );

    let moved: SoldResponse = post(
        host,
        "/v1/back-office/sold",
        Some(token),
        &SoldRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
            limit: 20,
        },
    )?;
    for row in &moved.rows {
        println!(
            "  off the shelf: {} milli over {} sale(s)",
            row.qty_milli, row.sales
        );
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
