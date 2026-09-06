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
    AccountRequest, AccountResponse, BalancesRequest, BalancesResponse, CustomerWire,
    CustomersRequest, CustomersResponse, DayRequest, DayResponse, EnrolRequest, EnrolResponse,
    OwedRequest, OwedResponse, PROTOCOL_VERSION, PullRequest, PullResponse, PushRequest,
    PushResponse, PutCustomerRequest, SettingsRequest, SettingsResponse, ShopRequest, ShopResponse,
    SoldRequest, SoldResponse, TakePaymentRequest, TakePaymentResponse, VatRequest, VatResponse,
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

    // The shop that heads the paper. A till fetches this before it can print,
    // and a receipt with no name on it is not a receipt.
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
    )?;

    // A credential does not last for ever, and until this week nothing ever
    // asked for a fresh one: every device would have stopped a year after it
    // was enrolled. The old one keeps working through an overlap, so a lost
    // reply does not strand the till.
    let renewed: openpos_core::protocol::RenewResponse = post(
        &host,
        "/v1/renew",
        Some(&till_side.token),
        &openpos_core::protocol::RenewRequest {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    println!(
        "the till took a fresh credential: good for {} days, the old one for {} more seconds",
        renewed.expires_in_seconds / 86_400,
        renewed.previous_valid_for_seconds
    );
    till.take_credential(
        &renewed.token,
        1_788_600_000_000,
        renewed.expires_in_seconds.saturating_mul(1_000),
    )?;

    // Where the shop's settings stand before anything is changed. A till asks
    // for this every half minute and asks for the three lists only when it has
    // moved, which is what makes locking somebody out take half a minute.
    let before: SettingsResponse = post(
        &host,
        "/v1/settings",
        Some(&renewed.token),
        &SettingsRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
        },
    )?;

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

    let after: SettingsResponse = post(
        &host,
        "/v1/settings",
        Some(&renewed.token),
        &SettingsRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
        },
    )?;
    println!(
        "the settings counter went from {} to {}, so every till re-reads",
        before.seq, after.seq
    );

    // And the till is told, which is what lets a cashier write a sale to that
    // account with the line down.
    let known: CustomersResponse = post(
        &host,
        "/v1/customers",
        Some(&renewed.token),
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

    // A cashier who may give away nothing, and a supervisor who may. This is
    // the shape of a shop: the ceiling exists so that giving money away is
    // somebody's decision rather than everybody's habit.
    till.set_operators(vec![
        openpos_core::auth::Operator {
            id: Ulid::from_u128(11),
            name: "Rahima".into(),
            pin: openpos_core::auth::PinHash::derive("4321", [3; 16], 1_000),
            permissions: openpos_core::auth::Permissions {
                max_discount_bp: 0,
                may_override_price: false,
                may_refund: false,
                may_void_line: true,
                may_authorise: false,
                may_open_drawer: true,
                may_close_shift: false,
            },
            active: true,
        },
        openpos_core::auth::Operator {
            id: Ulid::from_u128(12),
            name: "Karim".into(),
            pin: openpos_core::auth::PinHash::derive("9999", [4; 16], 1_000),
            permissions: openpos_core::auth::Permissions::supervisor(),
            active: true,
        },
    ])?;
    till.sign_in(Ulid::from_u128(11), "4321", 0)?;

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
    // "Apa, twenty taka off." The cashier cannot, and says so.
    let refused = till.set_ticket_discount(openpos_core::domain::Discount::Rate(
        openpos_core::money::Bp::new(1_000)?,
    ));
    println!(
        "the cashier tried a discount: {}",
        refused
            .err()
            .map_or_else(|| "allowed".to_owned(), |error| format!("{error}"))
    );

    // The supervisor is standing there and allows it, for this sale.
    till.authorise(
        Ulid::from_u128(12),
        "9999",
        openpos_core::auth::Action::Discount { bp: 1_000 },
        1_000,
        60_000,
    )?;
    till.set_ticket_discount(openpos_core::domain::Discount::Rate(
        openpos_core::money::Bp::new(1_000)?,
    ))?;

    let sold = till.checkout(Ulid::from_u128(900), 1_788_600_000_000)?;

    // What the customer is handed. Rendered here rather than described, so the
    // paper can be read rather than trusted.
    if let Some(shop) = till.shop().cloned() {
        println!("\n--- what the customer is handed ---");
        for line in openpos_core::receipt::render(
            &sold.ticket,
            &openpos_core::receipt::Context {
                shop,
                rung_at: "07 Sep 2026 00:30".to_owned(),
                cashier: Some("Rahima".to_owned()),
                customer: Some("Karim, flat 3".to_owned()),
                width: 32,
            },
        ) {
            println!("{}", line.text);
        }
        println!("--- end ---\n");
    }

    let pending = till.pending_sales(10)?;
    let pushed: PushResponse = post(
        &host,
        "/v1/sync/push",
        Some(&renewed.token),
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
            // From the top of the list. A screen carries on from where the last
            // page ended instead.
            after_owed_minor: 0,
            after_person_key: String::new(),
        },
    )?;
    for person in &book.owing {
        println!(
            "in the back office: {} owes {}",
            person.person_name, person.owed_minor
        );
    }
    // And the next page, from where that one ended. A shop with more people on
    // account than a page holds reads the rest rather than being shown the
    // first page as though it were the whole list.
    if let Some(last) = book.owing.last() {
        let next: OwedResponse = post(
            &host,
            "/v1/back-office/owed",
            Some(&owner_side.token),
            &OwedRequest {
                protocol: PROTOCOL_VERSION,
                limit: 50,
                after_owed_minor: last.owed_minor,
                after_person_key: last.person_key.clone(),
            },
        )?;
        println!(
            "after {}: {} more on the list",
            last.person_name,
            next.owing.len()
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
            after_at_ms: 0,
            after_source_id: 0,
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

    // And what the till would tell somebody asking across the counter. Asked as
    // the till, because that is who answers the question.
    let owed: BalancesResponse = post(
        &host,
        "/v1/customers/owed",
        Some(&renewed.token),
        &BalancesRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
        },
    )?;
    for balance in &owed.balances {
        println!("at the till, how much do I owe: {}", balance.owed_minor);
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
    // What to order against: what left the shelves, rather than what was
    // charged for it.
    let moved: SoldResponse = post(
        &host,
        "/v1/back-office/sold",
        Some(&owner_side.token),
        &SoldRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
            limit: 20,
        },
    )?;
    for row in &moved.rows {
        println!(
            "what sold: {} milli over {} sale(s)",
            row.qty_milli, row.sales
        );
    }

    // And what anybody allowed over a cashier's ceiling.
    let allowed: openpos_core::protocol::WaivedResponse = post(
        &host,
        "/v1/back-office/waived",
        Some(&owner_side.token),
        &openpos_core::protocol::WaivedRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
            limit: 50,
        },
    )?;
    println!("waived in that window: {}", allowed.waived.len());
    for one in &allowed.waived {
        println!("  {} on a sale of {}", one.reason, one.total_minor);
    }

    // And what the shop owes the revenue for the month, which is the figure a
    // return is filled in from.
    let owed_in_tax: VatResponse = post(
        &host,
        "/v1/back-office/vat",
        Some(&owner_side.token),
        &VatRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 1_788_000_000_000,
            to_ms: 1_789_999_999_999,
        },
    )?;
    for row in &owed_in_tax.rows {
        println!(
            "tax at {} bp: {} sold, {} tax, over {} sale(s)",
            row.vat_bp, row.net_minor, row.vat_minor, row.sales
        );
    }
    println!(
        "of which {} is {} sale(s) nobody has looked at yet",
        owed_in_tax.waiting_vat_minor, owed_in_tax.waiting_sales
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
