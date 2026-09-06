//! Syncing, arranged so the platform never learns the protocol.
//!
//! Two commands. The till is asked what to do, and it answers with a path and a
//! body already encoded; the platform posts those bytes and hands back whatever
//! came out. Nothing on either side of the FFI parses postcard, holds a cursor,
//! decides a batch size, or knows what a lease is.
//!
//! That division is not tidiness. The alternative is a sync client written in
//! Dart and again in JavaScript, and two sync clients are two sets of retry
//! rules, two cursor bugs and two ways to acknowledge a sale the server never
//! stored. This way there is one, in the same crate as the arithmetic it is
//! delivering, tested against the real server.
//!
//! Bytes cross as hex. It doubles them, which for a batch of twenty five sales
//! is a few kilobytes and is worth the fact that a person can read a request in
//! a debugger and paste it into a bug report.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use openpos_core::ids::Ulid;
use openpos_core::lease::Lease;
use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse, PullRequest, PullResponse,
    PushRequest, PushResponse, ShopRequest, ShopResponse, PROTOCOL_VERSION,
};
use openpos_core::receipt;
use openpos_core::storage::backend::Backend;
use openpos_core::sync::driver::{Driver, Next, Situation};
use openpos_core::sync::{deltas_from_pull, envelope_for};
use openpos_core::till::Till;
use serde::{Deserialize, Serialize};

/// What the platform should do next, with the request already built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Step {
    /// Post `body` to `path`, then hand the reply back as `apply`.
    Post {
        /// Which kind of exchange this is, returned unchanged to `apply` so the
        /// platform never has to remember what it asked for.
        kind: Exchange,
        path: String,
        body: String,
        /// The credential to present, when this terminal has one. Handed over
        /// with every step rather than kept by the platform, so a platform
        /// cannot send a stale one or forget to send any.
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Nothing to do. Come back after this long.
    Wait { for_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exchange {
    Push,
    Pull,
    Lease,
    /// Trading a code for a credential. The one exchange that carries none.
    Enrol,
    /// Asking what shop this is, for the top of a receipt.
    Shop,
}

/// Build the one request that carries no credential.
///
/// Encoded here rather than by hand on each platform. Two small fields is
/// exactly the shape somebody writes out in JavaScript because it looks easy,
/// and exactly the shape that breaks in silence when a field is added.
pub fn enrol_step(code: &str) -> Result<Step, String> {
    let request = EnrolRequest {
        protocol: PROTOCOL_VERSION,
        code: String::from(code),
    };
    Ok(Step::Post {
        kind: Exchange::Enrol,
        path: String::from("/v1/enrol"),
        body: encode(&request)?,
        token: None,
    })
}

/// What applying a reply changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// True when the server said more catalogue changes are waiting.
    pub more_to_pull: bool,
    /// Sales the server confirmed, so a caller can log a number that means
    /// something rather than "sync ran".
    pub settled: usize,
    /// Set by enrolment: which shop and terminal this device turned out to be.
    /// The code decides, not the device, so this is the first moment a till
    /// learns its own identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enrolled: Option<Enrolled>,
}

/// What a device learns when it enrols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrolled {
    pub tenant: String,
    pub terminal: String,
}

/// Ask the driver what to do, and build the request it asked for.
pub fn step<B: Backend>(
    till: &Till<B>,
    driver: &Driver,
    tenant: u128,
    online: bool,
    more_to_pull: bool,
    now_ms: u64,
) -> Result<Step, String> {
    let situation: Situation = till
        .situation(online, more_to_pull)
        .map_err(|error| format!("{error}"))?;

    match driver.next(&situation, now_ms) {
        Next::Wait { for_ms } => Ok(Step::Wait { for_ms }),
        Next::Push { limit } => {
            let pending = till
                .pending_sales(limit)
                .map_err(|error| format!("{error}"))?;
            let request = PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                sales: pending.iter().map(envelope_for).collect(),
            };
            Ok(Step::Post {
                kind: Exchange::Push,
                path: String::from("/v1/sync/push"),
                body: encode(&request)?,
                token: till.token().map(String::from),
            })
        }
        Next::Pull { cursor, limit } => {
            let request = PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                cursor,
                limit,
            };
            Ok(Step::Post {
                kind: Exchange::Pull,
                path: String::from("/v1/sync/pull"),
                body: encode(&request)?,
                token: till.token().map(String::from),
            })
        }
        Next::FetchShop => {
            let request = ShopRequest {
                protocol: PROTOCOL_VERSION,
            };
            Ok(Step::Post {
                kind: Exchange::Shop,
                path: String::from("/v1/shop"),
                body: encode(&request)?,
                token: till.token().map(String::from),
            })
        }
        Next::RenewLease { count } => {
            let request = LeaseRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                count,
            };
            Ok(Step::Post {
                kind: Exchange::Lease,
                path: String::from("/v1/lease"),
                body: encode(&request)?,
                token: till.token().map(String::from),
            })
        }
    }
}

/// Apply a reply the platform fetched.
///
/// The driver is told it succeeded here rather than by the platform, so a reply
/// that arrives but does not decode counts as the failure it is: a platform
/// that reported success on receiving any bytes at all would clear the backoff
/// against a server answering with an error page.
pub fn apply<B: Backend>(
    till: &mut Till<B>,
    driver: &mut Driver,
    kind: Exchange,
    body: &str,
    now_ms: u64,
) -> Result<Applied, String> {
    let bytes = from_hex(body).ok_or_else(|| String::from("the reply was not hex"))?;

    let applied = match kind {
        Exchange::Push => {
            let response: PushResponse =
                postcard::from_bytes(&bytes).map_err(|_| String::from("the push reply did not decode"))?;
            // Quarantined sales count as settled: the server has them, and
            // holding them on the till would leave the only copy on a tablet.
            let settled: Vec<Ulid> = response.settled().into_iter().map(Ulid::from_u128).collect();
            let count = till
                .acknowledge(&settled)
                .map_err(|error| format!("{error}"))?;
            Applied {
                more_to_pull: false,
                settled: count,
                enrolled: None,
            }
        }
        Exchange::Pull => {
            let response: PullResponse =
                postcard::from_bytes(&bytes).map_err(|_| String::from("the pull reply did not decode"))?;
            let more = response.more;
            till.apply_pull(&deltas_from_pull(&response))
                .map_err(|error| format!("{error}"))?;
            // Recorded whatever came back, including nothing: a driver that
            // only counted pulls which changed something would ask again
            // immediately, forever, in a shop whose prices are settled.
            driver.pulled(now_ms);
            Applied {
                more_to_pull: more,
                settled: 0,
                enrolled: None,
            }
        }
        Exchange::Lease => {
            let response: LeaseResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the lease reply did not decode"))?;
            till.grant_lease(&Lease::new(
                till.terminal(),
                response.epoch,
                &response.prefix,
                response.first,
                response.last,
            ))
            .map_err(|error| format!("{error}"))?;
            Applied {
                more_to_pull: false,
                settled: 0,
                enrolled: None,
            }
        }
        Exchange::Shop => {
            let response: ShopResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the shop reply did not decode"))?;
            till.set_shop(receipt::Shop {
                name: response.name,
                bin: response.bin,
                address: response.address,
                phone: response.phone,
            })
            .map_err(|error| format!("{error}"))?;
            Applied {
                more_to_pull: false,
                settled: 0,
                enrolled: None,
            }
        }
        Exchange::Enrol => {
            let response: EnrolResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the enrolment reply did not decode"))?;
            // Stored before the caller is told, so a device that is told it
            // enrolled has the credential on disk. The other order needs the
            // owner to issue another code and gives no clue why.
            till.set_token(&response.token)
                .map_err(|error| format!("{error}"))?;
            Applied {
                more_to_pull: false,
                settled: 0,
                enrolled: Some(Enrolled {
                    tenant: Ulid::from_u128(response.tenant).encode(),
                    terminal: Ulid::from_u128(response.terminal).encode(),
                }),
            }
        }
    };

    driver.succeeded(now_ms);
    Ok(applied)
}

/// Tell the driver the attempt did not work.
pub fn failed(driver: &mut Driver, now_ms: u64) {
    driver.failed(now_ms);
}

fn encode<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes =
        postcard::to_allocvec(value).map_err(|_| String::from("the request could not be built"))?;
    Ok(to_hex(&bytes))
}

/// Hex, for anything else in this crate that has bytes to hand a platform.
#[must_use]
pub fn to_hex_public(bytes: &[u8]) -> String {
    to_hex(bytes)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Two digits written by hand rather than through a formatter: this runs
        // over every byte of every sale in a batch.
        text.push(digit(byte >> 4));
        text.push(digit(*byte));
    }
    text
}

/// A nibble as a hex digit.
///
/// A lookup rather than arithmetic on a byte: the table cannot overflow, cannot
/// be reasoned about wrongly, and is faster than the addition it replaces.
const HEX: [u8; 16] = *b"0123456789abcdef";

fn digit(nibble: u8) -> char {
    // The mask is what makes the index safe rather than a comment claiming it.
    char::from(HEX[usize::from(nibble & 0x0F)])
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let high = value(*bytes.get(index)?)?;
        let low = value(*bytes.get(index.checked_add(1)?)?)?;
        out.push(high.checked_shl(4)?.checked_add(low)?);
        index = index.checked_add(2)?;
    }
    Some(out)
}

const fn value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => match byte.checked_sub(b'a') {
            Some(offset) => offset.checked_add(10),
            None => None,
        },
        b'A'..=b'F' => match byte.checked_sub(b'A') {
            Some(offset) => offset.checked_add(10),
            None => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn hex_survives_a_round_trip_including_the_awkward_bytes() {
        let bytes: Vec<u8> = (0..=255_u8).collect();
        let text = to_hex(&bytes);
        assert_eq!(text.len(), 512);
        assert_eq!(from_hex(&text).unwrap(), bytes);
    }

    #[test]
    fn hex_that_is_not_hex_is_refused_rather_than_guessed_at() {
        // A truncated body and a corrupted one both have to fail, or a sale
        // batch decodes to something shorter than what was sent.
        assert!(from_hex("abc").is_none(), "an odd length is not bytes");
        assert!(from_hex("zz").is_none());
        assert!(from_hex("00ff").is_some());
    }

    #[test]
    fn upper_and_lower_case_hex_both_read() {
        assert_eq!(from_hex("DEADbeef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }
}
