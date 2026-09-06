//! What the server actually sends a device that asks for catalogue changes.
//!
//! Written because two devices sat on stale prices while the server plainly had
//! the changes, and every screen involved could only report what it believed.
//! This asks the question directly: enrol, pull from a cursor, print what comes
//! back.
//!
//! ```text
//! cargo run -p openpos-server --example pull_once -- http://127.0.0.1:8099 CODE 0
//! ```

#![allow(clippy::expect_used, clippy::print_stdout)]

use std::io::{Read, Write};
use std::net::TcpStream;

use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, PullRequest, PullResponse, PROTOCOL_VERSION,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:8099".to_owned());
    let code = args.next().ok_or("give me an enrolment code")?;
    let cursor: u64 = args.next().unwrap_or_else(|| "0".to_owned()).parse()?;
    let host = base.trim_start_matches("http://").to_owned();

    let who: EnrolResponse = post(
        &host,
        "/v1/enrol",
        None,
        &EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code,
        },
    )?;

    let page: PullResponse = post(
        &host,
        "/v1/sync/pull",
        Some(&who.token),
        &PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: who.tenant,
            terminal: who.terminal,
            cursor,
            limit: 500,
        },
    )?;

    println!(
        "from cursor {cursor}: {} upserts, {} tombstones, cursor now {}, more {}",
        page.upserts.len(),
        page.tombstones.len(),
        page.cursor,
        page.more
    );
    for item in &page.upserts {
        println!(
            "  {:<24} {:>9} active={}",
            item.name_en, item.price_minor, item.active
        );
    }
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
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the server sent no headers")?;
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default());
    let status = head.lines().next().unwrap_or_default().to_owned();
    if !status.contains("200") {
        return Err(format!("{path} answered: {status}").into());
    }
    let body = raw
        .get(split.saturating_add(4)..)
        .ok_or("headers and nothing after them")?;
    Ok(postcard::from_bytes(body)?)
}
