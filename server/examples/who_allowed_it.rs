//! Who allowed what, from the till that allowed it to the owner who asks.
//!
//! Every ceiling in this product exists so that giving money away is somebody's
//! decision rather than everybody's habit. That only means something if the
//! decisions can be looked at afterwards, and until now they could not be: the
//! till wrote each one down in memory and the record died with the process. A
//! tablet restarted overnight answered nobody.
//!
//! Here a cashier who may not discount is allowed one by a supervisor, takes
//! cash out of the drawer on their own permission, and the owner reads both
//! back with the names attached.
//!
//! ```text
//! cargo run -p openpos-server --example who_allowed_it -- http://127.0.0.1:8098 TILL_CODE OWNER_CODE
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

use openpos_core::auth::{Action, Operator, Permissions, PinHash, SALT_LEN};
use openpos_core::cart::CartLimits;
use openpos_core::ids::Ulid;
use openpos_core::money::Minor;
use openpos_core::protocol::{
    AllowedRequest, AllowedResponse, AllowedWire, EnrolRequest, EnrolResponse, PROTOCOL_VERSION,
    PushAllowedRequest, PushAllowedResponse,
};
use openpos_core::storage::backend::MemoryBackend;
use openpos_core::till::Till;

/// Weak on purpose: this is a demonstration, not a shop.
const ROUNDS: u32 = 1_000;

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

    // Two people at one counter: a cashier who may not discount at all, and the
    // owner, who may allow one.
    let cashier = Ulid::from_u128(71);
    let supervisor = Ulid::from_u128(70);
    till.put_operator(Operator {
        id: supervisor,
        name: "Karim".into(),
        pin: PinHash::derive("9999", [3; SALT_LEN], ROUNDS),
        permissions: Permissions::supervisor(),
        active: true,
    })?;
    till.put_operator(Operator {
        id: cashier,
        name: "Rahima".into(),
        pin: PinHash::derive("1234", [4; SALT_LEN], ROUNDS),
        permissions: Permissions {
            may_open_drawer: true,
            ..Permissions::default()
        },
        active: true,
    })?;
    till.sign_in(cashier, "1234", 1_788_600_000_000)?;

    // "Apa, ten percent." She cannot, and the owner comes over and types a PIN.
    till.authorise(
        supervisor,
        "9999",
        Action::Discount { bp: 1_000 },
        1_788_600_060_000,
        90_000,
    )?;

    // And money out of the drawer for the milk man, which her own permission
    // covers: a different fact from a supervisor standing at the counter.
    till.open_shift(Ulid::from_u128(80), Minor::new(500_000), 1_788_600_000_000)?;
    till.cash_out(Minor::new(20_000), "paid the milk man", 1_788_600_120_000)?;

    // And somebody at the counter after closing, trying her PIN twice. Not an
    // action anybody was allowed to take: somebody failing to be allowed, which
    // is exactly what an owner wants to see beside the rest.
    for at_ms in [1_788_601_000_000, 1_788_601_010_000] {
        let refused = till.sign_in(cashier, "0000", at_ms);
        println!(
            "a wrong PIN: {}",
            match refused {
                Ok(()) => "let in, which it should not have been".to_owned(),
                Err(error) => format!("{error:?}"),
            }
        );
    }

    let waiting = till.unsent_allowed().to_vec();
    println!(
        "the till is holding {} record(s) of what it allowed",
        waiting.len()
    );

    let sent: PushAllowedResponse = post(
        &host,
        "/v1/sync/allowed",
        Some(&till_side.token),
        &PushAllowedRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            allowed: waiting
                .iter()
                .map(|one| AllowedWire {
                    seq: one.seq,
                    at_ms: one.at_ms,
                    action: one.action,
                    bp: one.bp,
                    operator: one.operator,
                    operator_name: one.operator_name.clone(),
                    authorised_by: one.authorised_by,
                    authorised_by_name: one.authorised_by_name.clone(),
                })
                .collect(),
        },
    )?;
    till.allowed_accepted(&sent.stored)?;
    println!(
        "the shop took {} of them; the till is now holding {}",
        sent.stored.len(),
        till.unsent_allowed().len()
    );

    // Sent again, as a till does when a reply goes missing. The shop must not
    // end up with the same thing twice.
    let _: PushAllowedResponse = post(
        &host,
        "/v1/sync/allowed",
        Some(&till_side.token),
        &PushAllowedRequest {
            protocol: PROTOCOL_VERSION,
            tenant: till_side.tenant,
            terminal: till_side.terminal,
            allowed: waiting
                .iter()
                .map(|one| AllowedWire {
                    seq: one.seq,
                    at_ms: one.at_ms,
                    action: one.action,
                    bp: one.bp,
                    operator: one.operator,
                    operator_name: one.operator_name.clone(),
                    authorised_by: one.authorised_by,
                    authorised_by_name: one.authorised_by_name.clone(),
                })
                .collect(),
        },
    )?;

    // What the owner reads, which is the whole point.
    let trail: AllowedResponse = post(
        &host,
        "/v1/back-office/allowed",
        Some(&owner_side.token),
        &AllowedRequest {
            protocol: PROTOCOL_VERSION,
            from_ms: 0,
            to_ms: 1_799_999_999_999,
            limit: 50,
        },
    )?;
    println!(
        "the shop holds {} record(s), newest first:",
        trail.allowed.len()
    );
    for one in &trail.allowed {
        let what = match one.action {
            1 => "a discount",
            2 => "a price typed over the catalogue's",
            3 => "a refund",
            4 => "a line taken off",
            5 => "the drawer opened",
            6 => "the drawer counted and closed",
            7 => "a PIN typed wrongly",
            8 => "a PIN typed wrongly, and that person locked out",
            9 => "took the till",
            _ => "something this build does not know about",
        };
        // A wrong PIN has a name on it because a button was pressed, not
        // because anybody did anything they were permitted to do.
        let who = if one.action == 9 {
            format!("({})", one.operator_name)
        } else if matches!(one.action, 7 | 8) {
            format!("on {}'s button", one.operator_name)
        } else if one.authorised_by_name.is_empty() {
            format!("by {}, on their own permission", one.operator_name)
        } else {
            format!(
                "by {}, allowed by {}",
                one.operator_name, one.authorised_by_name
            )
        };
        println!("  {} {} {}", one.at_ms, what, who);
    }
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
