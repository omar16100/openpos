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
    IssueCodeRequest, OperatorWire, OperatorsRequest, OperatorsResponse, PushRequest, PushResponse,
    PutOperatorRequest, PutShopRequest, ShopRequest, ShopResponse, UpsertItemRequest,
    PROTOCOL_VERSION,
};
use openpos_core::auth::{Operator, Permissions, PinHash, SALT_LEN};
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
    Wait {
        for_ms: u64,
        /// Failures since the last thing that worked. Zero is a till with
        /// nothing to do; anything else is a till that is not reaching the shop
        /// and will look identical unless it says so. A till that quietly stops
        /// syncing is the failure this whole design is arranged against, and it
        /// stopped quietly because a wait carried no reason.
        after_failures: u32,
    },
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
    /// Asking who may stand at this till.
    Operators,
    /// Back office: changes an owner makes, rather than things a till fetches.
    /// One variant each, so the reply can be read as what it is.
    AdminShop,
    AdminOperator,
    AdminItem,
    AdminCode,
    AdminTerminals,
    AdminAmendOperator,
    AdminOperatorPin,
    AdminReceive,
    AdminCount,
    AdminOnHand,
    AdminSuppliers,
    AdminPutSupplier,
    AdminDeliveries,
    AdminTakings,
    AdminRepairs,
    AdminResolveRepair,
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

/// Build a back-office request.
///
/// The same arrangement as the till's own exchanges: the core builds the bytes
/// and reads the reply, and the admin screen posts them. An admin that encoded
/// its own would be a second implementation of the protocol, and the one that
/// drifts is always the one used least.
pub fn admin_step<B: Backend>(
    till: &Till<B>,
    tenant: u128,
    request: &AdminRequest,
) -> Result<Step, String> {
    let (kind, path, body) = match request {
        AdminRequest::Shop {
            name,
            bin,
            address,
            phone,
        } => (
            Exchange::AdminShop,
            "/v1/back-office/shop",
            encode(&PutShopRequest {
                protocol: PROTOCOL_VERSION,
                name: name.clone(),
                bin: blank_to_none(bin),
                address: blank_to_none(address),
                phone: blank_to_none(phone),
            })?,
        ),
        AdminRequest::Operator {
            id,
            name,
            pin,
            salt,
            permissions,
            active,
        } => {
            let id = Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
            let salt: [u8; SALT_LEN] = salt
                .clone()
                .try_into()
                .map_err(|_| alloc::format!("a salt must be {SALT_LEN} bytes"))?;
            // Derived here, so the PIN never leaves this device and the key is
            // made by the same code the till will check it with.
            let hash = PinHash::derive(pin, salt, openpos_core::auth::DEFAULT_ROUNDS);
            (
                Exchange::AdminOperator,
                "/v1/back-office/operators",
                encode(&PutOperatorRequest {
                    protocol: PROTOCOL_VERSION,
                    operator: OperatorWire {
                        id: id.to_u128(),
                        name: name.clone(),
                        pin_salt: salt.to_vec(),
                        pin_rounds: openpos_core::auth::DEFAULT_ROUNDS,
                        pin_key: hash.key().to_vec(),
                        max_discount_bp: permissions.max_discount_bp,
                        may_override_price: permissions.may_override_price,
                        may_refund: permissions.may_refund,
                        may_void_line: permissions.may_void_line,
                        may_authorise: permissions.may_authorise,
                        may_open_drawer: permissions.may_open_drawer,
                        may_close_shift: permissions.may_close_shift,
                        active: *active,
                    },
                })?,
            )
        }
        AdminRequest::Item {
            item,
            price_minor,
            cost_minor,
            vat_bp,
            price_inclusive,
            vat_on_undiscounted,
        } => (
            Exchange::AdminItem,
            "/v1/back-office/catalogue/upsert",
            // The shop and the terminal come from the till, never from the
            // caller. A screen that supplied them is a screen that can supply
            // the wrong ones, and the server would answer with a refusal that
            // reads as a permission problem.
            encode(&UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                item: openpos_core::protocol::ItemWire {
                    id: Ulid::decode(&item.id)
                        .map_err(|_| String::from("that item id is not a valid id"))?
                        .to_u128(),
                    code: item.code.clone(),
                    name_en: item.name.clone(),
                    name_bn: item.name.clone(),
                    unit: String::from("Nos"),
                    price_minor: *price_minor,
                    cost_minor: *cost_minor,
                    vat_bp: *vat_bp,
                    price_inclusive: *price_inclusive,
                    vat_on_undiscounted: *vat_on_undiscounted,
                    barcodes: item.barcodes.clone(),
                    on_hand_milli: item.on_hand_milli,
                    // Whether the shop still sells it. Hardcoded true until now,
                    // so nothing could ever stop selling anything.
                    active: item.active,
                },
            })?,
        ),
        AdminRequest::OperatorPin { id, pin, salt } => {
            let who = Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
            let salt: [u8; SALT_LEN] = salt
                .clone()
                .try_into()
                .map_err(|_| alloc::format!("a salt must be {SALT_LEN} bytes"))?;
            // Derived here, as it is when somebody is added: the PIN never
            // leaves this device, and the key is made by the same code the till
            // will check it with.
            let hash = PinHash::derive(pin, salt, openpos_core::auth::DEFAULT_ROUNDS);
            (
                Exchange::AdminOperatorPin,
                "/v1/back-office/operators/pin",
                encode(&openpos_core::protocol::SetOperatorPinRequest {
                    protocol: PROTOCOL_VERSION,
                    operator_id: who.to_u128(),
                    pin_salt: salt.to_vec(),
                    pin_rounds: openpos_core::auth::DEFAULT_ROUNDS,
                    pin_key: hash.key().to_vec(),
                })?,
            )
        }
        AdminRequest::AmendOperator {
            id,
            name,
            permissions,
            active,
        } => {
            let who = Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
            (
                Exchange::AdminAmendOperator,
                "/v1/back-office/operators/amend",
                encode(&openpos_core::protocol::AmendOperatorRequest {
                    protocol: PROTOCOL_VERSION,
                    operator_id: who.to_u128(),
                    name: name.clone(),
                    max_discount_bp: permissions.max_discount_bp,
                    may_override_price: permissions.may_override_price,
                    may_refund: permissions.may_refund,
                    may_void_line: permissions.may_void_line,
                    may_authorise: permissions.may_authorise,
                    may_open_drawer: permissions.may_open_drawer,
                    may_close_shift: permissions.may_close_shift,
                    active: *active,
                })?,
            )
        }
        AdminRequest::Receive {
            id,
            supplier_id,
            reference,
            received_at_ms,
            lines,
        } => {
            let delivery =
                Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
            let mut wire = Vec::with_capacity(lines.len());
            for line in lines {
                let item = Ulid::decode(&line.item_id)
                    .map_err(|_| String::from("that is not a valid item id"))?;
                wire.push(openpos_core::protocol::ReceiptLineWire {
                    item_id: item.to_u128(),
                    qty_milli: line.qty_milli,
                    unit_cost_minor: line.unit_cost_minor,
                });
            }
            // A shop that has not written its suppliers down should still be
            // able to book goods in, so this is optional and an empty box is
            // nobody rather than an id that will not decode.
            let from = match supplier_id.as_deref().filter(|id| !id.is_empty()) {
                Some(id) => Some(
                    Ulid::decode(id)
                        .map_err(|_| String::from("that is not a valid supplier id"))?
                        .to_u128(),
                ),
                None => None,
            };
            (
                Exchange::AdminReceive,
                "/v1/back-office/stock/receive",
                encode(&openpos_core::protocol::ReceiveGoodsRequest {
                    protocol: PROTOCOL_VERSION,
                    id: delivery.to_u128(),
                    supplier_id: from,
                    reference: blank_to_none(reference),
                    received_at_ms: *received_at_ms,
                    note: None,
                    lines: wire,
                })?,
            )
        }
        AdminRequest::Count {
            counted_at_ms,
            lines,
        } => {
            let mut wire = Vec::with_capacity(lines.len());
            for line in lines {
                let id = Ulid::decode(&line.id)
                    .map_err(|_| String::from("that is not a valid id"))?;
                let item = Ulid::decode(&line.item_id)
                    .map_err(|_| String::from("that is not a valid item id"))?;
                wire.push(openpos_core::protocol::CountedItem {
                    id: id.to_u128(),
                    item_id: item.to_u128(),
                    counted_milli: line.qty_milli,
                });
            }
            (
                Exchange::AdminCount,
                "/v1/back-office/stock/count",
                encode(&openpos_core::protocol::RecordCountRequest {
                    protocol: PROTOCOL_VERSION,
                    counted_at_ms: *counted_at_ms,
                    note: None,
                    lines: wire,
                })?,
            )
        }
        AdminRequest::Repairs { limit } => (
            Exchange::AdminRepairs,
            "/v1/back-office/repairs",
            encode(&openpos_core::protocol::RepairQueueRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                limit: *limit,
            })?,
        ),
        AdminRequest::ResolveRepair { sale, note } => {
            let which =
                Ulid::decode(sale).map_err(|_| String::from("that is not a valid sale id"))?;
            (
                Exchange::AdminResolveRepair,
                "/v1/back-office/repairs/resolve",
                encode(&openpos_core::protocol::ResolveRepairRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant,
                    terminal: till.terminal().to_u128(),
                    sale: which.to_u128(),
                    note: note.clone(),
                })?,
            )
        }
        AdminRequest::Takings { from_ms, to_ms } => (
            Exchange::AdminTakings,
            "/v1/back-office/takings",
            encode(&openpos_core::protocol::TakingsRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
            })?,
        ),
        AdminRequest::Deliveries { limit } => (
            Exchange::AdminDeliveries,
            "/v1/back-office/deliveries",
            encode(&openpos_core::protocol::DeliveriesRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
            })?,
        ),
        AdminRequest::Suppliers => (
            Exchange::AdminSuppliers,
            "/v1/back-office/suppliers",
            encode(&openpos_core::protocol::SuppliersRequest {
                protocol: PROTOCOL_VERSION,
            })?,
        ),
        AdminRequest::Supplier {
            id,
            name,
            phone,
            bin,
            active,
        } => {
            let who = Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
            (
                Exchange::AdminPutSupplier,
                "/v1/back-office/suppliers/put",
                encode(&openpos_core::protocol::PutSupplierRequest {
                    protocol: PROTOCOL_VERSION,
                    supplier: openpos_core::protocol::SupplierWire {
                        id: who.to_u128(),
                        name: name.clone(),
                        phone: blank_to_none(phone),
                        bin: blank_to_none(bin),
                        active: *active,
                    },
                })?,
            )
        }
        AdminRequest::OnHand { item_ids } => {
            let mut ids = Vec::with_capacity(item_ids.len());
            for id in item_ids {
                let item = Ulid::decode(id)
                    .map_err(|_| String::from("that is not a valid item id"))?;
                ids.push(item.to_u128());
            }
            (
                Exchange::AdminOnHand,
                "/v1/back-office/stock/on-hand",
                encode(&openpos_core::protocol::OnHandRequest {
                    protocol: PROTOCOL_VERSION,
                    item_ids: ids,
                })?,
            )
        }
        AdminRequest::Terminals => (
            Exchange::AdminTerminals,
            "/v1/back-office/terminals",
            encode(&openpos_core::protocol::TerminalHealthRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
            })?,
        ),
        AdminRequest::Code {
            terminal_id,
            label,
            role,
            valid_for_seconds,
        } => {
            let terminal =
                Ulid::decode(terminal_id).map_err(|_| String::from("that is not a valid id"))?;
            (
                Exchange::AdminCode,
                "/v1/back-office/enrolment-codes",
                encode(&IssueCodeRequest {
                    protocol: PROTOCOL_VERSION,
                    terminal_id: terminal.to_u128(),
                    label: label.clone(),
                    role: *role,
                    valid_for_seconds: *valid_for_seconds,
                })?,
            )
        }
    };

    Ok(Step::Post {
        kind,
        path: String::from(path),
        body,
        token: till.token().map(String::from),
    })
}

/// An empty box on a form is nothing filled in, not an empty string. A receipt
/// prints a label with nothing after it either way, and one of those is a lie.
fn blank_to_none(text: &Option<String>) -> Option<String> {
    text.as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(String::from)
}

/// What the back office is being asked to change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum AdminRequest {
    Shop {
        name: String,
        bin: Option<String>,
        address: Option<String>,
        phone: Option<String>,
    },
    Operator {
        id: String,
        name: String,
        /// Never stored and never sent: derived into a key here and dropped.
        pin: String,
        /// Random, from the caller, because this crate has no entropy source.
        salt: Vec<u8>,
        permissions: openpos_core::auth::Permissions,
        active: bool,
    },
    Item {
        /// The item, with its id as text like every other id that crosses this
        /// boundary. Not because text is nicer: serde cannot carry a 128-bit
        /// number inside an internally tagged enum at all, and finding that out
        /// from a decoder error is worse than never writing it.
        item: crate::WireItem,
        price_minor: i64,
        cost_minor: i64,
        vat_bp: u32,
        price_inclusive: bool,
        vat_on_undiscounted: bool,
    },
    /// Give somebody a new PIN. Carries the PIN itself no further than this
    /// device: the key is derived here and the plain digits never travel.
    ///
    /// A fresh salt every time, minted where the PIN is typed. A shared one
    /// means a single search cracks every PIN in the shop at once.
    OperatorPin {
        id: String,
        pin: String,
        salt: Vec<u8>,
    },
    /// Change a person: their name, what they may do, whether they may sign in.
    /// Carries no PIN, because the back office does not have one: a PIN is
    /// hashed where it is set and never travels, so the request that sets one
    /// cannot be the request that corrects a name.
    AmendOperator {
        id: String,
        name: String,
        permissions: Permissions,
        active: bool,
    },
    /// A delivery. The id is minted on the device so a dropped reply can be
    /// resent without booking the same goods twice.
    Receive {
        id: String,
        /// Who it came from. Optional, because a shop that has not written its
        /// suppliers down should still be able to book goods in rather than
        /// being stopped at the door by a form.
        supplier_id: Option<String>,
        reference: Option<String>,
        received_at_ms: u64,
        lines: Vec<ReceivedLine>,
    },
    /// What a shelf was found to hold, which replaces the running figure rather
    /// than adjusting it.
    Count {
        counted_at_ms: u64,
        lines: Vec<CountedLine>,
    },
    /// Sales the server could not accept as they stood, waiting on a decision.
    Repairs {
        limit: u32,
    },
    /// Mark one of them as dealt with, and say what was decided.
    ResolveRepair {
        sale: String,
        note: String,
    },
    /// What the shop took between two moments. The caller says where the day
    /// starts and ends, because a shop's day ends when it closes.
    Takings {
        from_ms: u64,
        to_ms: u64,
    },
    /// What came in lately, newest first.
    Deliveries {
        limit: u32,
    },
    /// Who the shop buys from.
    Suppliers,
    /// Add or correct one of them.
    Supplier {
        id: String,
        name: String,
        phone: Option<String>,
        bin: Option<String>,
        active: bool,
    },
    /// What the shop believes it holds. A separate question from the catalogue,
    /// because a sale is not a catalogue change and the figure on an item record
    /// is whatever it was when somebody last edited that item.
    OnHand {
        item_ids: Vec<String>,
    },
    /// The tills this shop has. Needed before a code can be issued for one that
    /// already exists, which is the only way a device whose credential was
    /// revoked gets back its own ledger instead of a fresh one.
    Terminals,
    Code {
        terminal_id: String,
        label: String,
        role: i16,
        valid_for_seconds: u64,
    },
}

/// Turn the people on the wire into the people the core holds.
///
/// One conversion, used by the settings refresh and by suspending somebody.
/// Written twice it would be two, and the one used least would be the one that
/// drifted: that has happened twice already in this codebase.
fn people_from(wire: Vec<openpos_core::protocol::OperatorWire>) -> Result<Vec<Operator>, String> {
    let mut people = Vec::with_capacity(wire.len());
    for one in wire {
        // A credential of the wrong length is a record this build cannot verify
        // against. Padding it out would produce somebody whose PIN never works
        // and who looks like they forgot it.
        let salt: [u8; SALT_LEN] = one
            .pin_salt
            .try_into()
            .map_err(|_| String::from("an operator's credential is the wrong shape"))?;
        let key: [u8; openpos_core::auth::KEY_BYTES] = one
            .pin_key
            .try_into()
            .map_err(|_| String::from("an operator's credential is the wrong shape"))?;

        people.push(Operator {
            id: Ulid::from_u128(one.id),
            name: one.name.into_boxed_str(),
            pin: PinHash::from_parts(salt, one.pin_rounds, key),
            permissions: Permissions {
                max_discount_bp: one.max_discount_bp,
                may_override_price: one.may_override_price,
                may_refund: one.may_refund,
                may_void_line: one.may_void_line,
                may_authorise: one.may_authorise,
                may_open_drawer: one.may_open_drawer,
                may_close_shift: one.may_close_shift,
            },
            active: one.active,
        });
    }
    Ok(people)
}

/// Read an enrolment reply without a till.
///
/// Enrolment is the one exchange that happens before a till exists, because the
/// code decides which terminal this device is and a till has to be opened as
/// somebody. Routing it through a till meant opening one as a guess first, and
/// the guess was wrong the moment a second device enrolled.
pub fn read_enrolment(body: &str) -> Result<Credential, String> {
    let bytes = from_hex(body).ok_or_else(|| String::from("the reply was not hex"))?;
    let response: EnrolResponse = postcard::from_bytes(&bytes)
        .map_err(|_| String::from("the enrolment reply did not decode"))?;
    Ok(Credential {
        tenant: Ulid::from_u128(response.tenant).encode(),
        terminal: Ulid::from_u128(response.terminal).encode(),
        token: response.token,
    })
}

/// What a device is, once a code has told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub tenant: String,
    pub terminal: String,
    /// Stored in the till's standing state the moment it opens, and nowhere
    /// else.
    pub token: String,
}

/// What applying a reply changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// A code an owner just issued, shown once and never retrievable again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_code: Option<String>,
    /// The shop's tills, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<Terminal>,
    /// True when the server had already booked this delivery. Not a failure.
    #[serde(default)]
    pub already_booked: bool,
    /// What the counted shelves hold, after a count.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub on_hand: Vec<OnHand>,
    /// Who the shop buys from, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suppliers: Vec<Supplier>,
    /// What came in lately, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deliveries: Vec<Delivery>,
    /// Sales waiting on a decision, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<Repair>,
    /// True when the sale was already dealt with, or was never in the queue.
    #[serde(default)]
    pub already_resolved: bool,
    /// What the shop took, when it was asked for. An option rather than a
    /// default, because a day with no sales is a real answer and zero is what it
    /// looks like.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takings: Option<Takings>,
}

/// One till, as an owner needs to see it: enough to recognise which device it
/// is and to decide whether it is the one that has stopped reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: String,
    pub label: String,
    /// `None` for a device the server has not heard from since it started
    /// keeping the column, which is not the same as one that never synced.
    pub last_seen_ms: Option<u64>,
    pub sales: u64,
    pub open_repairs: u64,
}

/// One line of a delivery, as a screen hands it over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedLine {
    pub item_id: String,
    pub qty_milli: i64,
    /// What this delivery cost per unit. A margin is measured against what these
    /// goods cost, not against the price the item was last bought at.
    pub unit_cost_minor: i64,
}

/// One line of a count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountedLine {
    /// Minted on the device, so a count survives a dropped reply.
    pub id: String,
    pub item_id: String,
    pub qty_milli: i64,
}

/// A sale the server could not accept as it stood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    pub id: String,
    /// Absent when the till sold without a leased block, or when the payload
    /// could not be decoded far enough to find one.
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    /// When the server received it, not when it was rung. The gap between the
    /// two is how long the till was offline, which is usually the story.
    pub received_at_ms: u64,
    pub reason: String,
}

/// What a shop took over a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Takings {
    pub sales: u64,
    pub total_minor: i64,
    /// Refunds are in the total above, with their own sign. Counted separately,
    /// because a quiet day and a busy day with returns are not the same day.
    pub refunds: u64,
    pub refunded_minor: i64,
    pub tills: Vec<TillTakings>,
}

/// One till's part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillTakings {
    pub terminal: String,
    pub sales: u64,
    pub total_minor: i64,
    pub needing_attention: u64,
}

/// A delivery that has already happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub supplier_id: Option<String>,
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub lines: Vec<ReceivedLine>,
}

/// Somebody the shop buys from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supplier {
    pub id: String,
    pub name: String,
    pub phone: Option<String>,
    /// Business Identification Number. Most neighbourhood suppliers have none,
    /// and a required field would be filled with zeros.
    pub bin: Option<String>,
    pub active: bool,
}

/// What a shelf holds after the server has thought about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnHand {
    pub item_id: String,
    pub qty_milli: i64,
    /// Sales rung before the count but which reached the server after it. Not in
    /// the figure, because nobody can say whether the person counting saw those
    /// goods, and a shop reading a variance a month later needs to know it.
    pub unreconciled_milli: i64,
    pub unreconciled_sales: u32,
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
        Next::Wait {
            for_ms,
            // How many sales are waiting is already on every screen that shows
            // this, from the view. Carrying it here as well gave two numbers
            // from two moments, and they disagreed on the first try.
        } => Ok(Step::Wait {
            for_ms,
            after_failures: driver.failures(),
        }),
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
        Next::FetchOperators => {
            let request = OperatorsRequest {
                protocol: PROTOCOL_VERSION,
            };
            Ok(Step::Post {
                kind: Exchange::Operators,
                path: String::from("/v1/operators"),
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
                ..Applied::default()
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
                ..Applied::default()
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
                ..Applied::default()
            }
        }
        // A back-office reply is checked for shape and otherwise carries
        // nothing the till needs. Decoding it anyway is what catches a server
        // that answered 200 with something else entirely.
        Exchange::AdminShop => {
            postcard::from_bytes::<ShopResponse>(&bytes)
                .map_err(|_| String::from("the shop reply did not decode"))?;
            Applied::default()
        }
        Exchange::AdminOperator => {
            let response: OperatorsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the operators reply did not decode"))?;
            // The whole list back, so the device that added somebody shows them
            // at once rather than after its next settings refresh, which is ten
            // minutes away. Same as amending one.
            till.set_operators(people_from(response.operators)?)
                .map_err(|error| format!("{error}"))?;
            Applied::default()
        }
        Exchange::AdminItem => {
            postcard::from_bytes::<openpos_core::protocol::CatalogueEditResponse>(&bytes)
                .map_err(|_| String::from("the catalogue reply did not decode"))?;
            Applied::default()
        }
        Exchange::AdminCode => {
            let response: openpos_core::protocol::IssueCodeResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the enrolment code reply did not decode"))?;
            Applied {
                issued_code: Some(response.code),
                ..Applied::default()
            }
        }
        Exchange::AdminAmendOperator | Exchange::AdminOperatorPin => {
            let response: OperatorsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the operators reply did not decode"))?;
            // The whole list comes back and replaces what this device held, so
            // somebody suspended stops appearing on the sign-in panel here
            // without waiting for the next settings refresh.
            till.set_operators(people_from(response.operators)?)
                .map_err(|error| format!("{error}"))?;
            Applied::default()
        }
        Exchange::AdminReceive => {
            let response: openpos_core::protocol::ReceiveGoodsResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the delivery reply did not decode"))?;
            Applied {
                // False when the server had already booked this delivery. Not a
                // failure: a retry after a dropped reply is ordinary, and a
                // screen that treats it as one teaches a shop to book twice.
                already_booked: !response.recorded,
                ..Applied::default()
            }
        }
        Exchange::AdminCount => {
            let response: openpos_core::protocol::RecordCountResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the count reply did not decode"))?;
            Applied {
                on_hand: response
                    .on_hand
                    .into_iter()
                    .map(|entry| OnHand {
                        item_id: Ulid::from_u128(entry.item_id).encode(),
                        qty_milli: entry.qty_milli,
                        unreconciled_milli: entry.unreconciled_milli,
                        unreconciled_sales: entry.unreconciled_sales,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminRepairs => {
            let response: openpos_core::protocol::RepairQueueResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the repair queue reply did not decode"))?;
            Applied {
                repairs: response
                    .entries
                    .into_iter()
                    .map(|entry| Repair {
                        id: Ulid::from_u128(entry.id).encode(),
                        receipt_no: entry.receipt_no,
                        total_minor: entry.total_minor,
                        received_at_ms: entry.received_at_ms,
                        reason: entry.reason,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminResolveRepair => {
            let response: openpos_core::protocol::ResolveRepairResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the resolve reply did not decode"))?;
            Applied {
                // False when it was already dealt with, or was never in the
                // queue. One answer for both, because acting on either is the
                // same: load the queue again and look.
                already_resolved: !response.resolved,
                ..Applied::default()
            }
        }
        Exchange::AdminTakings => {
            let response: openpos_core::protocol::TakingsResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the takings reply did not decode"))?;
            Applied {
                takings: Some(Takings {
                    sales: response.sales,
                    total_minor: response.total_minor,
                    refunds: response.refunds,
                    refunded_minor: response.refunded_minor,
                    tills: response
                        .tills
                        .into_iter()
                        .map(|till| TillTakings {
                            terminal: Ulid::from_u128(till.terminal).encode(),
                            sales: till.sales,
                            total_minor: till.total_minor,
                            needing_attention: till.needing_attention,
                        })
                        .collect(),
                }),
                ..Applied::default()
            }
        }
        Exchange::AdminDeliveries => {
            let response: openpos_core::protocol::DeliveriesResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the deliveries reply did not decode"))?;
            Applied {
                deliveries: response
                    .deliveries
                    .into_iter()
                    .map(|one| Delivery {
                        id: Ulid::from_u128(one.id).encode(),
                        supplier_id: one.supplier_id.map(|who| Ulid::from_u128(who).encode()),
                        reference: one.reference,
                        received_at_ms: one.received_at_ms,
                        lines: one
                            .lines
                            .into_iter()
                            .map(|line| ReceivedLine {
                                item_id: Ulid::from_u128(line.item_id).encode(),
                                qty_milli: line.qty_milli,
                                unit_cost_minor: line.unit_cost_minor,
                            })
                            .collect(),
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminSuppliers | Exchange::AdminPutSupplier => {
            let response: openpos_core::protocol::SuppliersResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the suppliers reply did not decode"))?;
            Applied {
                suppliers: response
                    .suppliers
                    .into_iter()
                    .map(|one| Supplier {
                        id: Ulid::from_u128(one.id).encode(),
                        name: one.name,
                        phone: one.phone,
                        bin: one.bin,
                        active: one.active,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminOnHand => {
            let response: openpos_core::protocol::OnHandResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the stock reply did not decode"))?;
            Applied {
                on_hand: response
                    .on_hand
                    .into_iter()
                    .map(|entry| OnHand {
                        item_id: Ulid::from_u128(entry.item_id).encode(),
                        qty_milli: entry.qty_milli,
                        unreconciled_milli: entry.unreconciled_milli,
                        unreconciled_sales: entry.unreconciled_sales,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminTerminals => {
            let response: openpos_core::protocol::TerminalHealthResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the terminals reply did not decode"))?;
            Applied {
                terminals: response
                    .terminals
                    .into_iter()
                    .map(|entry| Terminal {
                        id: Ulid::from_u128(entry.terminal).encode(),
                        label: entry.label,
                        last_seen_ms: entry.last_seen_ms,
                        sales: entry.sales,
                        open_repairs: entry.open_repairs,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::Shop => {
            let response: ShopResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the shop reply did not decode"))?;
            // Recorded as asked whatever came back, for the same reason a pull
            // is: a shop that has filled nothing in still answered.
            driver.fetched_shop(now_ms);
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
                ..Applied::default()
            }
        }
        Exchange::Operators => {
            let response: OperatorsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the operators reply did not decode"))?;

            let people = people_from(response.operators)?;

            driver.fetched_operators(now_ms);
            till.set_operators(people)
                .map_err(|error| format!("{error}"))?;
            Applied {
                more_to_pull: false,
                settled: 0,
                enrolled: None,
                ..Applied::default()
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
                ..Applied::default()
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
    fn a_repair_queue_comes_back_with_the_prose_a_person_has_to_read() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{RepairEntry, RepairQueueResponse};
        use openpos_core::storage::backend::MemoryBackend;

        let (mut till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");
        let mut driver = Driver::default();

        let response = RepairQueueResponse {
            protocol: PROTOCOL_VERSION,
            entries: alloc::vec![
                RepairEntry {
                    id: 900,
                    receipt_no: Some(String::from("T1-000100")),
                    total_minor: 49_450,
                    received_at_ms: 1_788_600_000_000,
                    reason: String::from("another sale already carries this receipt number"),
                },
                RepairEntry {
                    id: 901,
                    // A till that sold without a leased block. Absent, not
                    // empty: a screen showing "" reads as a number nobody typed.
                    receipt_no: None,
                    total_minor: 1_200,
                    received_at_ms: 1_788_600_100_000,
                    reason: String::from("the payload could not be decoded"),
                },
            ],
        };
        let body = to_hex(&postcard::to_allocvec(&response).expect("it encodes"));

        let applied = apply(&mut till, &mut driver, Exchange::AdminRepairs, &body, 1)
            .expect("the reply decodes");

        assert_eq!(applied.repairs.len(), 2);
        assert_eq!(applied.repairs[0].id, Ulid::from_u128(900).encode());
        assert_eq!(applied.repairs[0].receipt_no.as_deref(), Some("T1-000100"));
        assert_eq!(applied.repairs[1].receipt_no, None);
        // The reason is prose written for the person deciding, and it has to
        // survive the crossing intact rather than becoming a code they cannot
        // look up.
        assert!(applied.repairs[0].reason.contains("receipt number"));
    }

    #[test]
    fn a_repair_is_resolved_by_naming_the_sale_and_what_was_decided() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::ResolveRepairRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let terminal = Ulid::from_u128(7);
        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            terminal,
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let request = AdminRequest::ResolveRepair {
            sale: Ulid::from_u128(900).encode(),
            note: String::from("counted twice on the paper roll, left as it stands"),
        };
        let Step::Post { body, path, .. } = admin_step(&till, 42, &request).expect("a step") else {
            panic!("a back-office request is a post");
        };
        assert_eq!(path, "/v1/back-office/repairs/resolve");
        let sent: ResolveRepairRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("it decodes");

        assert_eq!(sent.sale, 900);
        assert_eq!(sent.tenant, 42);
        // The note travels. The queue is worked months before anybody asks why a
        // total was wrong, and an entry that disappears without one leaves that
        // question unanswerable.
        assert!(sent.note.starts_with("counted twice"));
    }

    #[test]
    fn a_wait_says_whether_the_till_is_idle_or_cut_off() {
        use openpos_core::cart::CartLimits;
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        // Nothing to do. The till has no lease and no sales, so it waits.
        let mut driver = Driver::default();
        let idle = step(&till, &driver, 42, false, false, 1_000).expect("a step");
        let Step::Wait { after_failures, .. } = idle else {
            panic!("an offline till waits");
        };
        assert_eq!(after_failures, 0);

        // Now it has tried and failed. Same shape, and until this carried the
        // reason a till that could not reach the shop rendered as "idle" on
        // every screen, which is the failure this design exists to catch.
        failed(&mut driver, 2_000);
        failed(&mut driver, 3_000);
        let cut_off = step(&till, &driver, 42, false, false, 4_000).expect("a step");
        let Step::Wait { after_failures, .. } = cut_off else {
            panic!("a failing till waits");
        };
        assert_eq!(after_failures, 2);
    }

    #[test]
    fn a_delivery_carries_its_own_id_so_a_retry_is_not_a_second_delivery() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::ReceiveGoodsRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let request = AdminRequest::Receive {
            id: Ulid::from_u128(900).encode(),
            supplier_id: None,
            reference: Some(String::from("  ")),
            received_at_ms: 1_700_000_000_000,
            lines: alloc::vec![ReceivedLine {
                item_id: Ulid::from_u128(1).encode(),
                qty_milli: 24_000,
                unit_cost_minor: 34_400,
            }],
        };

        let step = admin_step(&till, 42, &request).expect("a step");
        let Step::Post { body, .. } = step else {
            panic!("a back-office request is a post");
        };
        let sent: ReceiveGoodsRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("it decodes");

        // The id is the shop's protection against a dropped reply: sending the
        // same delivery twice must not book the goods twice.
        assert_eq!(sent.id, 900);
        assert_eq!(sent.lines.len(), 1);
        assert_eq!(sent.lines[0].qty_milli, 24_000);
        assert_eq!(sent.lines[0].unit_cost_minor, 34_400);
        // A box with spaces in it is a box nobody filled in, and a challan
        // number of "  " printed on a report is worse than none.
        assert_eq!(sent.reference, None);
    }

    #[test]
    fn a_delivery_says_who_it_came_from_when_the_shop_knows() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::ReceiveGoodsRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let sent = |supplier: Option<String>| {
            let request = AdminRequest::Receive {
                id: Ulid::from_u128(900).encode(),
                supplier_id: supplier,
                reference: None,
                received_at_ms: 1,
                lines: alloc::vec![ReceivedLine {
                    item_id: Ulid::from_u128(1).encode(),
                    qty_milli: 1_000,
                    unit_cost_minor: 100,
                }],
            };
            let Step::Post { body, .. } = admin_step(&till, 42, &request).expect("a step") else {
                panic!("a back-office request is a post");
            };
            postcard::from_bytes::<ReceiveGoodsRequest>(&from_hex(&body).expect("hex"))
                .expect("it decodes")
        };

        assert_eq!(sent(Some(Ulid::from_u128(5).encode())).supplier_id, Some(5));

        // A shop that has not written its suppliers down must still be able to
        // book goods in. An unchosen dropdown is nobody, not an id that fails
        // to decode and stops the delivery at the door.
        assert_eq!(sent(None).supplier_id, None);
        assert_eq!(sent(Some(String::new())).supplier_id, None);
    }

    #[test]
    fn a_count_says_what_was_found_rather_than_what_changed() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::RecordCountRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let request = AdminRequest::Count {
            counted_at_ms: 1_700_000_000_000,
            lines: alloc::vec![CountedLine {
                id: Ulid::from_u128(901).encode(),
                item_id: Ulid::from_u128(1).encode(),
                qty_milli: 0,
            }],
        };

        let step = admin_step(&till, 42, &request).expect("a step");
        let Step::Post { body, .. } = step else {
            panic!("a back-office request is a post");
        };
        let sent: RecordCountRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("it decodes");

        // Zero is a real count. A shelf found empty is the most useful thing a
        // count can say, and dropping it as "nothing entered" leaves the running
        // figure exactly as wrong as it was.
        assert_eq!(sent.lines.len(), 1);
        assert_eq!(sent.lines[0].counted_milli, 0);
        assert_eq!(sent.counted_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn stopping_an_item_being_sold_travels_as_a_stopped_item() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::UpsertItemRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let request = AdminRequest::Item {
            item: crate::WireItem {
                id: Ulid::from_u128(5).encode(),
                code: String::from("TEA400"),
                name: String::from("Tea 400g"),
                price_minor: 0,
                cost_minor: 0,
                vat_bp: 0,
                price_inclusive: false,
                vat_on_undiscounted: false,
                barcodes: alloc::vec![String::from("8690000000005")],
                on_hand_milli: 0,
                active: false,
            },
            price_minor: 22_000,
            cost_minor: 17_600,
            vat_bp: 1_500,
            price_inclusive: false,
            vat_on_undiscounted: false,
        };

        let step = admin_step(&till, 42, &request).expect("a step");
        let Step::Post { body, .. } = step else {
            panic!("a back-office request is a post");
        };
        let bytes = from_hex(&body).expect("hex");
        let sent: UpsertItemRequest = postcard::from_bytes(&bytes).expect("it decodes");

        // Hardcoded true until now, so nothing could stop selling anything and
        // the flag was set on the screen and dropped on the way out.
        assert!(!sent.item.active);
        assert_eq!(sent.item.price_minor, 22_000, "and the price still travels");
    }

    #[test]
    fn the_shops_tills_come_back_with_ids_a_screen_can_hand_straight_back() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{TerminalHealthEntry, TerminalHealthResponse};
        use openpos_core::storage::backend::MemoryBackend;

        let terminal = Ulid::from_u128(7);
        let (mut till, _boot) =
            Till::open(MemoryBackend::new(), 42, terminal, 1, CartLimits::default())
                .expect("a till opens");
        let mut driver = Driver::default();

        let response = TerminalHealthResponse {
            protocol: PROTOCOL_VERSION,
            terminals: alloc::vec![TerminalHealthEntry {
                terminal: 9,
                label: String::from("Front counter"),
                epoch: 1,
                enrolled_at_ms: 1_000,
                last_seen_ms: None,
                sales: 3,
                open_repairs: 0,
            }],
        };
        let body = to_hex(&postcard::to_allocvec(&response).expect("it encodes"));

        let applied = apply(&mut till, &mut driver, Exchange::AdminTerminals, &body, 2_000)
            .expect("the reply decodes");

        // As text, because that id goes back out as the terminal a new enrolment
        // code is for. A number that has to be re-encoded on the way back is a
        // number that will be re-encoded differently.
        assert_eq!(applied.terminals.len(), 1);
        assert_eq!(applied.terminals[0].id, Ulid::from_u128(9).encode());
        assert_eq!(applied.terminals[0].label, "Front counter");
        // Never heard from is not the same as heard from at zero.
        assert_eq!(applied.terminals[0].last_seen_ms, None);
    }

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
