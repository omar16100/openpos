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

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use openpos_core::auth::{Operator, Permissions, PinHash, SALT_LEN};
use openpos_core::ids::Ulid;
use openpos_core::lease::Lease;
use openpos_core::protocol::{
    EnrolRequest, EnrolResponse, IssueCodeRequest, LeaseRequest, LeaseResponse, OperatorWire,
    OperatorsRequest, OperatorsResponse, PROTOCOL_VERSION, PullRequest, PullResponse, PushRequest,
    PushResponse, PutOperatorRequest, PutShopRequest, ShopRequest, ShopResponse, UpsertItemRequest,
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
    AdminDeleteItem,
    AdminCode,
    AdminTerminals,
    AdminAmendOperator,
    AdminOperatorPin,
    Shifts,
    /// What a till allowed, on its way to the shop.
    Allowed,
    /// Items a till wrote down at the counter, on their way to the shop.
    Items,
    /// People a till wrote down at the counter, on their way to the shop.
    People,
    AdminReceive,
    AdminCount,
    AdminOnHand,
    AdminSuppliers,
    AdminPutSupplier,
    AdminDeliveries,
    Customers,
    Balances,
    /// What the shop believes is on the shelves, for a shop that has asked its
    /// tills to warn or refuse.
    Stock,
    Settings,
    Renew,
    ReportDrawer,
    AdminShifts,
    AdminAdoptSales,
    AdminOpenDrawers,
    AdminCustomers,
    /// What the shop's own details and settings stand at, for a screen that is
    /// about to change one of them.
    AdminShopNow,
    AdminItemNow,
    AdminDay,
    AdminVat,
    AdminSold,
    AdminWaived,
    AdminResendCatalogue,
    AdminUnreadable,
    /// Items a till wrote down at a counter, for somebody to look at.
    AdminItemsFromTills,
    AdminRevokeTerminal,
    AdminSupplierOwing,
    AdminSupplierStatement,
    AdminPaySupplier,
    AdminOwed,
    AdminTakePayment,
    AdminAccount,
    AdminRepairs,
    AdminReceipt,
    AdminMade,
    AdminCorrectStock,
    AdminResolveRepair,
    AdminDecided,
    AdminDecideAgain,
    AdminAllowed,
    AdminReceiptGaps,
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
            wallets,
            stock_rule,
        } => (
            Exchange::AdminShop,
            "/v1/back-office/shop",
            encode(&PutShopRequest {
                protocol: PROTOCOL_VERSION,
                name: name.clone(),
                bin: blank_to_none(bin),
                address: blank_to_none(address),
                phone: blank_to_none(phone),
                wallets: wallets.clone(),
                stock_rule: *stock_rule,
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
            expected_seq,
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
                // Where the item stood when it was read for editing. Zero from
                // a screen that has not read it, which is a new item or an
                // older screen, and is accepted.
                expected_seq: *expected_seq,
                item: openpos_core::protocol::ItemWire {
                    id: Ulid::decode(&item.id)
                        .map_err(|_| String::from("that item id is not a valid id"))?
                        .to_u128(),
                    code: item.code.clone(),
                    name_en: item.name.clone(),
                    // The English name when the shop has not typed a Bangla one,
                    // so a search in either script still finds it. Copying it
                    // unconditionally, which is what this did, meant the Bangla
                    // name could never be anything else.
                    name_bn: if item.name_bn.trim().is_empty() {
                        item.name.clone()
                    } else {
                        item.name_bn.clone()
                    },
                    unit: if item.unit.trim().is_empty() {
                        String::from("Nos")
                    } else {
                        item.unit.clone()
                    },
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
                    // Saved from the back office, which is somebody looking at
                    // it: that is exactly what stops being provisional.
                    from_a_till: false,
                    // What the owner said this is: standard rated, zero rated
                    // or exempt. Carried on the item itself rather than beside
                    // it, so a screen that says nothing means the ordinary
                    // case rather than silently reclassifying the shop.
                    supply: item.supply,
                    // And what they sort it under, in the shop's own words.
                    category: item.category.clone(),
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
            let delivery = Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
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
        AdminRequest::DeleteItem { item_id } => {
            let item =
                Ulid::decode(item_id).map_err(|_| String::from("that is not a valid item id"))?;
            (
                Exchange::AdminDeleteItem,
                "/v1/back-office/catalogue/delete",
                encode(&openpos_core::protocol::DeleteItemRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant,
                    terminal: till.terminal().to_u128(),
                    item: item.to_u128(),
                })?,
            )
        }
        AdminRequest::CorrectStock {
            id,
            item_id,
            qty_milli,
            reason,
            occurred_at_ms,
        } => {
            let correction =
                Ulid::decode(id).map_err(|_| String::from("that is not a valid id"))?;
            let item =
                Ulid::decode(item_id).map_err(|_| String::from("that is not a valid item id"))?;
            (
                Exchange::AdminCorrectStock,
                "/v1/back-office/stock/correct",
                encode(&openpos_core::protocol::CorrectStockRequest {
                    protocol: PROTOCOL_VERSION,
                    id: correction.to_u128(),
                    item_id: item.to_u128(),
                    qty_milli: *qty_milli,
                    reason: reason.clone(),
                    occurred_at_ms: *occurred_at_ms,
                })?,
            )
        }
        AdminRequest::Count {
            counted_at_ms,
            lines,
        } => {
            let mut wire = Vec::with_capacity(lines.len());
            for line in lines {
                let id =
                    Ulid::decode(&line.id).map_err(|_| String::from("that is not a valid id"))?;
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
        AdminRequest::Made { from_ms, to_ms } => (
            Exchange::AdminMade,
            "/v1/back-office/made",
            encode(&openpos_core::protocol::MadeRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
            })?,
        ),
        AdminRequest::Receipt { receipt_no } => (
            Exchange::AdminReceipt,
            // Not the back office's own route, though it answers the same
            // question. A customer with a piece of paper is standing at a
            // counter, and the person holding it is a cashier: this one takes
            // the shop from the credential and asks nothing about the role.
            "/v1/receipt",
            encode(&openpos_core::protocol::ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: receipt_no.clone(),
            })?,
        ),
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
        AdminRequest::ReceiptGaps { limit } => (
            Exchange::AdminReceiptGaps,
            "/v1/back-office/receipt-gaps",
            encode(&openpos_core::protocol::ReceiptGapsRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
            })?,
        ),
        AdminRequest::Allowed {
            from_ms,
            to_ms,
            limit,
        } => (
            Exchange::AdminAllowed,
            "/v1/back-office/allowed",
            encode(&openpos_core::protocol::AllowedRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
                limit: *limit,
            })?,
        ),
        AdminRequest::Decided { limit } => (
            Exchange::AdminDecided,
            "/v1/back-office/repairs/decided",
            encode(&openpos_core::protocol::DecidedRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
            })?,
        ),
        AdminRequest::DecideAgain {
            sale,
            note,
            kept,
            expected_decisions,
        } => {
            let which =
                Ulid::decode(sale).map_err(|_| String::from("that is not a valid sale id"))?;
            (
                Exchange::AdminDecideAgain,
                "/v1/back-office/repairs/decide-again",
                encode(&openpos_core::protocol::DecideAgainRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant,
                    terminal: till.terminal().to_u128(),
                    sale: which.to_u128(),
                    note: note.clone(),
                    kept: *kept,
                    expected_decisions: *expected_decisions,
                })?,
            )
        }
        AdminRequest::ResolveRepair { sale, note, kept } => {
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
                    kept: *kept,
                })?,
            )
        }
        AdminRequest::Shifts { limit } => (
            Exchange::AdminShifts,
            "/v1/back-office/shifts",
            encode(&openpos_core::protocol::ShiftsRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
            })?,
        ),
        AdminRequest::AdoptSales { bundle } => {
            // Whitespace taken out first. A bundle travels through whatever the
            // shop has, which is usually a message on a phone, and those wrap
            // long text and add line breaks. Refusing a paste for a newline
            // somebody did not put there would break the one path a stranded
            // device has.
            let cleaned: String = bundle.chars().filter(|one| !one.is_whitespace()).collect();
            // Decoded here only to refuse a paste that is not a bundle, so
            // somebody who pasted the wrong thing is told at the keyboard
            // rather than by a 400.
            let bytes = from_hex(&cleaned).ok_or_else(|| String::from("that is not a bundle"))?;
            postcard::from_bytes::<openpos_core::protocol::AdoptSalesRequest>(&bytes)
                .map_err(|_| String::from("that is not a bundle of sales"))?;
            (
                Exchange::AdminAdoptSales,
                "/v1/back-office/sales/adopt",
                to_hex(&bytes),
            )
        }
        AdminRequest::Day { from_ms, to_ms } => (
            Exchange::AdminDay,
            "/v1/back-office/day",
            encode(&openpos_core::protocol::DayRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
            })?,
        ),
        AdminRequest::Vat { from_ms, to_ms } => (
            Exchange::AdminVat,
            "/v1/back-office/vat",
            encode(&openpos_core::protocol::VatRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
            })?,
        ),
        AdminRequest::RevokeTerminal { terminal } => (
            Exchange::AdminRevokeTerminal,
            "/v1/back-office/terminals/revoke",
            encode(&openpos_core::protocol::RevokeTerminalRequest {
                protocol: PROTOCOL_VERSION,
                terminal: Ulid::decode(terminal)
                    .map_err(|_| String::from("that is not a till"))?
                    .to_u128(),
            })?,
        ),
        AdminRequest::ItemsFromTills { limit } => (
            Exchange::AdminItemsFromTills,
            "/v1/back-office/catalogue/from-tills",
            encode(&openpos_core::protocol::TillItemsRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
            })?,
        ),
        AdminRequest::ResendCatalogue => (
            Exchange::AdminResendCatalogue,
            "/v1/back-office/catalogue/resend",
            encode(&openpos_core::protocol::ResendCatalogueRequest {
                protocol: PROTOCOL_VERSION,
            })?,
        ),
        AdminRequest::UnreadableChanges { limit } => (
            Exchange::AdminUnreadable,
            "/v1/back-office/catalogue/unreadable",
            encode(&openpos_core::protocol::UnreadableChangesRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
            })?,
        ),
        AdminRequest::Waived {
            from_ms,
            to_ms,
            limit,
        } => (
            Exchange::AdminWaived,
            "/v1/back-office/waived",
            encode(&openpos_core::protocol::WaivedRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
                limit: *limit,
            })?,
        ),
        AdminRequest::Sold {
            from_ms,
            to_ms,
            limit,
        } => (
            Exchange::AdminSold,
            "/v1/back-office/sold",
            encode(&openpos_core::protocol::SoldRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: *from_ms,
                to_ms: *to_ms,
                limit: *limit,
            })?,
        ),
        AdminRequest::SupplierStatement {
            supplier,
            from_ms,
            to_ms,
        } => (
            Exchange::AdminSupplierStatement,
            "/v1/back-office/suppliers/statement",
            encode(&openpos_core::protocol::SupplierStatementRequest {
                protocol: PROTOCOL_VERSION,
                supplier_id: Ulid::decode(supplier)
                    .map_err(|_| String::from("that is not a supplier"))?
                    .to_u128(),
                from_ms: *from_ms,
                to_ms: *to_ms,
            })?,
        ),
        AdminRequest::SupplierOwing => (
            Exchange::AdminSupplierOwing,
            "/v1/back-office/suppliers/owed",
            encode(&openpos_core::protocol::SupplierOwingRequest {
                protocol: PROTOCOL_VERSION,
            })?,
        ),
        AdminRequest::PaySupplier {
            id,
            supplier,
            amount_minor,
            paid_at_ms,
            note,
        } => (
            Exchange::AdminPaySupplier,
            "/v1/back-office/suppliers/payment",
            encode(&openpos_core::protocol::PaySupplierRequest {
                protocol: PROTOCOL_VERSION,
                id: Ulid::decode(id)
                    .map_err(|_| String::from("that is not a payment id"))?
                    .to_u128(),
                supplier_id: Ulid::decode(supplier)
                    .map_err(|_| String::from("that is not a supplier"))?
                    .to_u128(),
                amount_minor: *amount_minor,
                paid_at_ms: *paid_at_ms,
                note: note.clone(),
            })?,
        ),
        AdminRequest::ItemNow { item } => (
            Exchange::AdminItemNow,
            "/v1/back-office/catalogue/item",
            encode(&openpos_core::protocol::ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: Ulid::decode(item)
                    .map_err(|_| String::from("that is not an item"))?
                    .to_u128(),
            })?,
        ),
        AdminRequest::ShopNow => (
            Exchange::AdminShopNow,
            // The till's own route again: the shop is the same shop, and a
            // second one reading the same row is a second thing to keep in step.
            "/v1/shop",
            encode(&openpos_core::protocol::ShopRequest {
                protocol: PROTOCOL_VERSION,
            })?,
        ),
        AdminRequest::Customers => (
            Exchange::AdminCustomers,
            // The till's own route: the list is the same list, and a second one
            // reading the same table is a second thing to keep in step.
            "/v1/customers",
            encode(&openpos_core::protocol::CustomersRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
            })?,
        ),
        AdminRequest::Customer {
            id,
            name,
            phone,
            active,
            bin,
            limit_minor,
        } => (
            Exchange::AdminCustomers,
            "/v1/back-office/customers",
            encode(&openpos_core::protocol::PutCustomerRequest {
                protocol: PROTOCOL_VERSION,
                customer: openpos_core::protocol::CustomerWire {
                    id: Ulid::decode(id)
                        .map_err(|_| String::from("that is not a customer id"))?
                        .to_u128(),
                    name: name.clone(),
                    phone: phone.clone(),
                    active: *active,
                    bin: bin.clone(),
                    // What the shop will let them owe. Zero is no cap, which
                    // is what a screen that says nothing means.
                    limit_minor: *limit_minor,
                },
            })?,
        ),
        AdminRequest::OpenDrawers => (
            Exchange::AdminOpenDrawers,
            "/v1/back-office/drawers",
            encode(&openpos_core::protocol::OpenDrawersRequest {
                protocol: PROTOCOL_VERSION,
            })?,
        ),
        AdminRequest::Owed {
            limit,
            after_owed_minor,
            after_person_key,
        } => (
            Exchange::AdminOwed,
            "/v1/back-office/owed",
            encode(&openpos_core::protocol::OwedRequest {
                protocol: PROTOCOL_VERSION,
                limit: *limit,
                after_owed_minor: *after_owed_minor,
                after_person_key: after_person_key.clone(),
            })?,
        ),
        AdminRequest::TakePayment {
            id,
            person_key,
            person_name,
            amount_minor,
            at_ms,
            note,
            written_off,
        } => (
            Exchange::AdminTakePayment,
            "/v1/back-office/owed/payment",
            encode(&openpos_core::protocol::TakePaymentRequest {
                protocol: PROTOCOL_VERSION,
                id: Ulid::decode(id)
                    .map_err(|_| String::from("that is not a payment id"))?
                    .to_u128(),
                person_key: person_key.clone(),
                person_name: person_name.clone(),
                amount_minor: *amount_minor,
                at_ms: *at_ms,
                note: note.clone(),
                written_off: *written_off,
            })?,
        ),
        AdminRequest::Account {
            person_key,
            limit,
            after_at_ms,
            after_source_id,
        } => (
            Exchange::AdminAccount,
            "/v1/back-office/owed/account",
            encode(&openpos_core::protocol::AccountRequest {
                protocol: PROTOCOL_VERSION,
                person_key: person_key.clone(),
                limit: *limit,
                after_at_ms: *after_at_ms,
                // An empty cursor is the newest entry. A malformed one is a
                // screen bug rather than a shop's, and starting over is a
                // better answer than an error nobody can act on.
                after_source_id: Ulid::decode(after_source_id)
                    .map(|id| id.to_u128())
                    .unwrap_or_default(),
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
                let item =
                    Ulid::decode(id).map_err(|_| String::from("that is not a valid item id"))?;
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

/// A screen that says nothing about whether a sale stands means it stands.
/// Striking one out is the deliberate act; leaving it alone is not.
const fn yes() -> bool {
    true
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
        /// The wallets this shop takes. Set here rather than typed at a till,
        /// where one typo becomes a third wallet with its own line in every
        /// report and nothing to reconcile against.
        #[serde(default)]
        wallets: Vec<String>,
        /// What a till does when a basket asks for more than the shelf holds:
        /// 0 nothing, 1 say so, 2 refuse it and let a supervisor allow it.
        #[serde(default)]
        stock_rule: u8,
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
        /// Where the item stood when whoever is editing it read it, so the
        /// server can refuse a save built on a copy somebody else has since
        /// changed. Zero for something new, or for a screen that did not read
        /// it first.
        #[serde(default)]
        expected_seq: u64,
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
    /// Take one item off the shop's books entirely.
    ///
    /// For a line typed by mistake and never traded, which the shop refuses to
    /// do for anything else: a deletion is a tombstone every till obeys, and
    /// what it removes is the name behind figures still in the books. Anything
    /// the shop has sold is withdrawn instead, which the item save already does.
    DeleteItem {
        item_id: String,
    },
    /// Goods gone, with why: a bottle dropped, a bag spoiled, something taken.
    ///
    /// The other way a stock figure moves without a sale. A shop that could
    /// only sell or count carried a wrong shelf until its next count and had
    /// nowhere to say what happened to the difference.
    CorrectStock {
        /// Minted by the screen and kept, so a retry after a dropped reply is
        /// the same correction rather than a second one.
        id: String,
        item_id: String,
        /// Signed: negative for goods gone, positive for a count that was
        /// under.
        qty_milli: i64,
        reason: String,
        occurred_at_ms: u64,
    },
    /// Sales the server could not accept as they stood, waiting on a decision.
    Repairs {
        limit: u32,
    },
    /// What was on a receipt somebody brought back to the counter.
    Receipt {
        receipt_no: String,
    },
    /// What the shop made over a period: turnover before tax, less what the
    /// goods cost.
    Made {
        from_ms: u64,
        to_ms: u64,
    },
    /// Mark one of them as dealt with, and say what was decided.
    /// Where the shop's numbering jumps.
    ReceiptGaps {
        limit: u32,
    },
    /// Who allowed what, in a window.
    Allowed {
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    },
    /// What has been answered lately, so a wrong answer can be found again.
    Decided {
        limit: u32,
    },
    /// Change an answer already given. Its own request, so it cannot happen by
    /// pressing the same button twice.
    DecideAgain {
        sale: String,
        note: String,
        kept: bool,
        /// How many answers the screen saw on this sale. Absent means it did
        /// not look, which is accepted: an older screen has no way to know.
        #[serde(default)]
        expected_decisions: u32,
    },
    ResolveRepair {
        sale: String,
        note: String,
        /// Whether the sale stands. Absent means it does, which is the answer
        /// for every entry except the duplicate the queue was built for.
        #[serde(default = "yes")]
        kept: bool,
    },
    /// Drawers this shop has counted and closed, newest first. What the
    /// counting is for: somebody who was not at the till reconciling it.
    Shifts {
        limit: u32,
    },
    /// Take in sales somebody carried from a device that could not send them.
    /// The bundle is what that device wrote out, verbatim.
    AdoptSales {
        bundle: String,
    },
    /// Which tills have a drawer open right now.
    OpenDrawers,
    /// What a day looked like: sold, refunded, counted, and put on account.
    Day {
        from_ms: u64,
        to_ms: u64,
    },
    /// What was sold at each tax rate over a period, for a return.
    Vat {
        from_ms: u64,
        to_ms: u64,
    },
    /// Catalogue changes that never reached the tills.
    /// Items a till wrote down at a counter that nobody has agreed to.
    ItemsFromTills {
        limit: u32,
    },
    UnreadableChanges {
        limit: u32,
    },
    /// Say every item to the tills again, for a shop whose tills are behind.
    ResendCatalogue,
    /// What supervisors waived over a period, newest first.
    Waived {
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    },
    /// What sold over a period, most sold first.
    Sold {
        from_ms: u64,
        to_ms: u64,
        limit: u32,
    },
    /// Cut a device off, because it is lost or stolen. Every credential that
    /// terminal holds stops working; the terminal itself stays, because its
    /// sales are still its sales.
    RevokeTerminal {
        terminal: String,
    },
    /// What the shop owes its suppliers.
    SupplierOwing,
    /// What passed between the shop and one supplier over a period.
    SupplierStatement {
        supplier: String,
        from_ms: u64,
        to_ms: u64,
    },
    /// Record money paid to a supplier. The id is minted here so a dropped
    /// reply can be resent without paying twice.
    PaySupplier {
        id: String,
        supplier: String,
        amount_minor: i64,
        paid_at_ms: u64,
        note: Option<String>,
    },
    /// One item as the shop holds it now, with the sequence it stands at.
    ItemNow {
        item: String,
    },
    /// Everybody who buys on account, stopped accounts included.
    Customers,
    /// The shop's own details and settings as they stand. Read before showing
    /// the form that overwrites them: a form that opens empty is a form that
    /// saves an empty shop, and a rule nobody can see is a rule nobody can tell
    /// is on.
    ShopNow,
    /// Add or correct somebody who buys on account.
    Customer {
        id: String,
        name: String,
        phone: Option<String>,
        active: bool,
        /// Their Business Identification Number, when the buyer is a business.
        /// Absent from a screen that does not ask, which keeps what the shop
        /// already holds rather than wiping it.
        #[serde(default)]
        bin: Option<String>,
        /// The most the shop will let them owe at once, in poisha. Zero is no
        /// cap, which is what everybody has until an owner says otherwise.
        #[serde(default)]
        limit_minor: i64,
    },
    /// Who owes the shop money.
    Owed {
        limit: u32,
        /// Where the last page ended, so the next carries on from it. Absent
        /// starts at the top, which is what a screen opening the list sends.
        #[serde(default)]
        after_owed_minor: i64,
        #[serde(default)]
        after_person_key: String,
    },
    /// Take money off what somebody owes. The id is minted here so a dropped
    /// reply can be resent without counting the payment twice.
    TakePayment {
        id: String,
        person_key: String,
        person_name: String,
        amount_minor: i64,
        at_ms: u64,
        note: Option<String>,
        /// True when nothing was handed over and the debt is being struck off.
        /// Needs a note, and is never added in with money the shop was given.
        #[serde(default)]
        written_off: bool,
    },
    /// What one person's balance is made of.
    Account {
        person_key: String,
        limit: u32,
        /// Where the last page ended: when that entry was and what made it.
        /// Absent starts at the newest.
        #[serde(default)]
        after_at_ms: u64,
        #[serde(default)]
        after_source_id: String,
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

/// The shop's own details and settings, for a screen about to change one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopNow {
    pub name: String,
    pub bin: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub wallets: Vec<String>,
    pub stock_rule: u8,
}

/// An item as the till holds it, as the protocol carries it.
///
/// The two shapes are deliberately not one type: one is what a device writes to
/// its own disk and the other is what crosses a network, and they change on
/// different days.
fn into_wire_item(held: openpos_core::storage::wire::ItemV1) -> openpos_core::protocol::ItemWire {
    openpos_core::protocol::ItemWire {
        id: held.id,
        code: held.code,
        name_en: held.name_en,
        name_bn: held.name_bn,
        unit: held.unit,
        price_minor: held.price_minor,
        cost_minor: held.cost_minor,
        vat_bp: held.vat_bp,
        price_inclusive: held.price_inclusive,
        vat_on_undiscounted: held.vat_on_undiscounted,
        barcodes: held.barcodes,
        on_hand_milli: held.on_hand_milli,
        active: held.active,
        // Said here and forced by the server anyway: a till cannot write down
        // an item the shop has already agreed to.
        from_a_till: true,
        // What a till writes down at the counter is sold at the rate the
        // cashier typed, which is the standard treatment. An owner says
        // otherwise in the back office when they look at it.
        supply: held.supply,
        category: held.category,
    }
}

/// Skipped when nothing was taken, so a reply that changed no stock reads the
/// same as it always did.
fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// What applying a reply changed.
///
/// Fields default on the way in, because most of them are skipped on the way
/// out when they are empty: without this the shape cannot read back its own
/// output, and anything that round-trips a view fails on whichever list
/// happened to be empty that time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
    /// Seconds the code above is good for, as the shop said.
    #[serde(default)]
    pub code_lasts_seconds: u64,
    /// The shop's tills, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<Terminal>,
    /// True when the server had already booked this delivery. Not a failure.
    #[serde(default)]
    pub already_booked: bool,
    /// What the counted shelves hold, after a count.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub on_hand: Vec<OnHand>,
    /// Whether those figures are every item the shop sells. False on a page of
    /// them, so a screen adding them up can say which it is looking at.
    #[serde(default)]
    pub on_hand_whole: bool,
    /// The shop as it stands, when it was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shop: Option<ShopNow>,
    /// Items a till wrote down at a counter that nobody has agreed to, when
    /// they were asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_tills: Vec<crate::WireItem>,
    /// How many people this till wrote down the shop has now taken.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub people_taken: usize,
    /// How many items this till wrote down the shop has now taken.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub items_taken: usize,
    /// How many shelf figures this till took from the shop, when it asked. A
    /// screen showing a stock warning should be able to say when the figure
    /// behind it last moved.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stock_taken: usize,
    /// Who the shop buys from, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suppliers: Vec<Supplier>,
    /// What came in lately, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deliveries: Vec<Delivery>,
    /// Drawers counted and closed, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shifts: Vec<ClosedDrawer>,
    /// Sales taken in from a device that could not send them. Counted rather
    /// than listed: what the person carrying them needs to know is whether the
    /// shop has them now, which is when the device may be wiped.
    #[serde(default)]
    pub adopted: usize,
    /// Of those, how many are now waiting for somebody to look at them.
    ///
    /// Not the same number, and the difference is the point. A sale the shop
    /// already had is taken in and joins no queue, because the server refuses
    /// to send anybody hunting for an entry that is not there. Without this the
    /// screen said "they are in the list below" about sales that were not, and
    /// undid that care one layer up.
    #[serde(default)]
    pub adopted_needing_attention: usize,
    /// What a day looked like, when it was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<Day>,
    /// How many credentials were withdrawn when a device was cut off. Zero is
    /// an ordinary answer: a device enrolled and never used, or one already cut
    /// off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<u32>,
    /// Catalogue changes no till could read, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<UnreadableChange>,
    /// How many items were said to the tills again, when that is what was
    /// asked for. Zero otherwise, which is also the honest answer for a shop
    /// with nothing in its catalogue.
    #[serde(default)]
    pub resent: u64,
    /// One item as the shop holds it now, when it was asked for, and where it
    /// stands. A screen edits from this rather than from its own copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_now: Option<crate::WireItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_seq: Option<u64>,
    /// What supervisors waived, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waived: Vec<Waived>,
    /// What sold over a period, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sold: Vec<SoldLine>,
    /// What passed between the shop and one supplier, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub statement: Vec<SupplierLine>,
    /// What the shop owes its suppliers, when it was asked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supplier_owing: Vec<SupplierOwing>,
    /// What was sold at each tax rate, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vat: Vec<VatLine>,
    /// How much of that figure is sales still waiting on somebody to look at
    /// them. In the figure and counted apart from it, because a return is a
    /// number a shop signs its name to.
    #[serde(default)]
    pub vat_waiting_sales: u64,
    #[serde(default)]
    pub vat_waiting_minor: i64,
    /// What the rows come to, as the shop added them up. The screen was summing
    /// them itself, and that figure is the one an owner writes on a return.
    #[serde(default)]
    pub vat_minor: i64,
    /// Everybody who buys on account, stopped accounts included. The till's own
    /// view lists only the active ones, which is right for a cashier and leaves
    /// the back office nowhere to let anybody back in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub every_customer: Vec<crate::Customer>,
    /// Drawers standing open right now, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_drawers: Vec<OpenDrawer>,
    /// Who owes the shop, when it was asked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub owed: Vec<Owing>,
    /// One person's account, when it was asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub account: Vec<AccountLine>,
    /// True when the server had already recorded this payment. Not a failure.
    #[serde(default)]
    pub already_paid: bool,
    /// What that person owes now, straight from the book rather than worked out
    /// on the screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owed_now: Option<i64>,
    /// Sales waiting on a decision, when they were asked for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<Repair>,
    /// What the shop holds under one receipt number: both of them when two
    /// carry it, which is the case somebody comes in about.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub on_paper: Vec<SaleOnPaper>,
    /// What a period made, when that is what was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub made: Option<Made>,
    /// True when the sale was already dealt with, or was never in the queue.
    #[serde(default)]
    pub already_resolved: bool,
    /// Sales somebody has already answered about, newest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decided: Vec<Decided>,
    /// Who allowed what, newest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<Allowed>,
    /// Runs of receipt numbers with no sale against them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Gap>,
    /// True when the answer was changed. False means nobody had answered about
    /// that sale, so it is still in the queue where a first answer is given.
    #[serde(default)]
    pub decision_changed: bool,
    /// True when somebody else answered while this screen was open. Nothing
    /// moved: read the list again and decide against what is there.
    #[serde(default)]
    pub decision_stale: bool,
}

/// A run of receipt numbers the shop has no sale for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub terminal: String,
    /// The numbers either side of it, as they are printed.
    pub after: String,
    pub before: String,
    pub missing: u64,
}

/// One privileged action, as a person reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowed {
    pub terminal: String,
    /// The device's own count. Carried so a screen has something to key a list
    /// on that cannot collide.
    pub seq: u64,
    pub at_ms: u64,
    /// What was done, in words. English, and the fallback for a screen that has
    /// never heard of this kind: see `kind`.
    pub what: String,
    /// The same thing as a number, as the trail on the device stores it, so a
    /// screen in another language can say it in its own words rather than
    /// matching on the sentence above.
    #[serde(default)]
    pub kind: u8,
    /// Basis points, for a discount. Zero otherwise.
    pub bp: u32,
    pub operator_name: String,
    /// Empty when nobody had to allow it: the operator's own permission covered
    /// it, which is a different fact from a supervisor standing at the counter.
    pub authorised_by_name: String,
    /// True when this is somebody failing to be allowed rather than somebody
    /// being allowed: a PIN typed wrongly. The name on it is the button that
    /// was pressed, not a person who did anything, and a screen that says
    /// "on their own permission" about it is telling the shop a lie.
    pub refused: bool,
    /// True when this is somebody signing in. Also not an action anybody was
    /// permitted to take, and the name on it is the person who typed a PIN that
    /// was right.
    pub took_the_till: bool,
    /// True when somebody tried to do something and was not permitted to.
    ///
    /// The name on it is the person, unlike a wrong PIN where it is the button
    /// that was pressed, so this is its own thing rather than one of the two
    /// above. Without it these read "their own permission covered it", which is
    /// the exact opposite of what happened and is the lie the comment on
    /// `refused` is about. Taking a line off a basket somebody had paid
    /// towards, and opening the drawer.
    pub was_not_permitted: bool,
    /// True when permission was never the question.
    ///
    /// A receipt printed again needs nobody's leave: what it needs is a record,
    /// because a second copy is a second piece of paper somebody can hand over.
    /// Saying a permission covered it invites a shop to go looking for a
    /// permission to take away, and there is not one.
    pub needed_no_permission: bool,
    /// Which receipt was printed again, when the till that wrote it knew.
    ///
    /// Absent on every other kind of entry, and on a reprint written by a till
    /// from before this was recorded. Carried through to the screen rather than
    /// dropped here: the shop's store holds it, the wire carries it, and a
    /// trail that says only that somebody printed something leaves the shop
    /// lining times up against its own sales by hand. Found by walking, with
    /// the number in Postgres and the screen not showing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_no: Option<String>,
}

/// One sale somebody has already answered about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decided {
    pub id: String,
    pub receipt_no: Option<String>,
    pub total_minor: i64,
    pub reason: String,
    pub note: String,
    /// False means every figure is ignoring this sale.
    pub kept: bool,
    pub decided_at_ms: u64,
    /// Two or more is a shop that changed its mind, shown rather than hidden.
    pub decisions: u32,
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
    /// 2 for a device that is the back office as well, 1 for a till, 0 for one
    /// the shop has withdrawn every credential from. A screen offering a lost
    /// device a new code has to know which of those it is, or it can only ever
    /// offer a till's.
    pub role: u8,
    /// When the shop took this device on.
    #[serde(default)]
    pub enrolled_at_ms: u64,
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

/// What a period made, and how much of it the shop can answer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Made {
    pub net_minor: i64,
    pub cost_minor: i64,
    pub made_minor: i64,
    pub sales: u64,
    /// Sales with something on them the shop has never said the cost of. Their
    /// turnover is not in the figure either: half a margin read as a whole one
    /// is worse than none.
    pub sales_without_cost: u64,
    pub net_without_cost_minor: i64,
}

/// One line of a sale, as the customer's paper shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperLine {
    /// Which item this was. What a refund needs beside the money: the same
    /// goods have to go back on the same shelf, and a line rung again from
    /// today's catalogue is priced at today's price rather than at what this
    /// customer paid.
    pub item_id: String,
    pub name: String,
    pub qty_milli: i64,
    pub unit: String,
    pub unit_price_minor: i64,
    pub discount_minor: i64,
    pub vat_bp: u32,
    pub line_total_minor: i64,
}

/// One payment, as the paper shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperTender {
    pub kind: String,
    /// Which of the three every shop has, so a screen says it in the shop's
    /// language and leaves a wallet's own name alone.
    #[serde(default)]
    pub kind_code: String,
    pub amount_minor: i64,
    pub reference: Option<String>,
}

/// A sale the shop holds, read out of the bytes the till committed.
///
/// For the person at the counter with a piece of paper in their hand: the
/// goods, the money, what was waived, and anything given back since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleOnPaper {
    pub id: String,
    pub terminal: String,
    pub receipt_no: String,
    pub rung_at_ms: u64,
    pub lines: Vec<PaperLine>,
    pub tenders: Vec<PaperTender>,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub overrides: Vec<String>,
    /// Empty when the shop took it without question. English, and the fallback
    /// for a reason the screen cannot name.
    pub held_for: String,
    /// A stable name for why it is held, and the figures inside it, for a
    /// screen saying it in the shop's language. Keyed the same way the repair
    /// queue's are, because it is the same fact shown in a second place.
    #[serde(default)]
    pub held_for_kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub held_for_parts: BTreeMap<String, String>,
    /// What somebody decided about it, when anybody has.
    pub decided: Option<String>,
    pub still_counts: bool,
    pub refunded_minor: i64,
    pub refund_of: Option<String>,
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
    /// The sentence the shop decided on when it held the sale. English, and
    /// what a screen falls back to for anything it cannot name.
    pub reason: String,
    /// A stable name for why it is held, for a screen wording it in the shop's
    /// language. Empty for a sale held before the shop stored the reason
    /// itself.
    #[serde(default)]
    pub kind: String,
    /// The figures inside that reason, named and already formatted, for the
    /// same reason a refusal's are: a sentence cannot be translated with the
    /// shop's own numbers baked into it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, String>,
}

/// A held sale's reason, as a name and its figures.
///
/// The words are the screen's. Formatted here with the same two helpers the
/// receipt uses, so an amount reads the same on paper, on the screen and in the
/// queue.
fn held_for(
    reason: &openpos_core::protocol::QuarantineReason,
) -> (String, BTreeMap<String, String>) {
    use openpos_core::protocol::QuarantineReason as Why;
    use openpos_core::receipt::{money_of, quantity_of};

    let mut parts = BTreeMap::new();
    let mut say = |key: &str, value: String| {
        parts.insert(String::from(key), value);
    };
    let code = match reason {
        Why::TotalsMismatch {
            stored_minor,
            recomputed_minor,
        } => {
            say("stored", money_of(*stored_minor));
            say("recomputed", money_of(*recomputed_minor));
            "totals-mismatch"
        }
        Why::DuplicateReceiptNumber { receipt_no } => {
            say("receipt_no", receipt_no.clone());
            "duplicate-receipt"
        }
        Why::Undecodable => "undecodable",
        Why::CarriedIn => "carried-in",
        Why::ClockOutOfRange {
            rung_at_ms,
            received_at_ms,
        } => {
            // How far out, in the largest unit that says something, and which
            // way. The moments themselves are milliseconds since 1970 and mean
            // nothing to anybody at a counter.
            let apart = rung_at_ms.abs_diff(*received_at_ms);
            let minutes = apart / 60_000;
            let hours = minutes / 60;
            let days = hours / 24;
            // Under a minute is said as under a minute rather than rounded up
            // to one: the server's own sentence says that, and two places
            // wording the same gap differently is a shop reading two answers.
            let (count, unit) = if days > 0 {
                (days, "days")
            } else if hours > 0 {
                (hours, "hours")
            } else if minutes > 0 {
                (minutes, "minutes")
            } else {
                (0, "moment")
            };
            say("how_far", alloc::format!("{count}"));
            say("unit", String::from(unit));
            // The direction picks the sentence rather than being poured into
            // one: "after" is a word, and a word interpolated into another
            // language's sentence reads as English in the middle of it.
            if rung_at_ms > received_at_ms {
                "clock-after"
            } else {
                "clock-before"
            }
        }
        Why::RefundAgainstNothing { receipt_no } => {
            say("receipt_no", receipt_no.clone());
            "refund-against-nothing"
        }
        Why::RefundBeyondTheSale {
            receipt_no,
            sale_minor,
            refunded_minor,
        } => {
            say("receipt_no", receipt_no.clone());
            say("sale", money_of(*sale_minor));
            say("refunded", money_of(*refunded_minor));
            "refund-beyond-the-sale"
        }
        Why::TendersDoNotAddUp {
            total_minor,
            tendered_minor,
            change_minor,
        } => {
            say("total", money_of(*total_minor));
            say("tendered", money_of(*tendered_minor));
            say("change", money_of(*change_minor));
            "tenders-do-not-add-up"
        }
        Why::MoreCameBackThanWentOut {
            receipt_no,
            item_id,
            over_by_milli,
        } => {
            say("receipt_no", receipt_no.clone());
            say("over_by", quantity_of(*over_by_milli));
            // Which item, so a screen can name it from its own catalogue. The
            // sentence beside this says "item 0193..." because the server has
            // the id and not the name, and a receipt of nine lines with one of
            // them wrong is a shop being told a quantity and not a thing.
            say("item", Ulid::from_u128(*item_id).encode());
            "more-came-back"
        }
    };
    (String::from(code), parts)
}

/// What a day looked like: what was sold, what came back, what the drawers held
/// against what they should have, and what went on account rather than into the
/// till.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Day {
    pub sales: u64,
    pub total_minor: i64,
    pub refunds: u64,
    pub refunded_minor: i64,
    pub drawers_counted: u32,
    pub expected_cash_minor: i64,
    pub counted_cash_minor: i64,
    pub variance_minor: i64,
    pub charged_minor: i64,
    /// Goods brought back by somebody who took them on account. Apart from what
    /// was charged, not netted into it.
    pub returned_minor: i64,
    pub paid_minor: i64,
    pub written_off_minor: i64,
    pub tills: Vec<TillDay>,
}

/// A catalogue change every till has passed over, because this build cannot
/// read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableChange {
    pub seq: u64,
    pub item: String,
    /// The schema its payload was written under: one number naming the build
    /// that wrote it.
    pub schema: u8,
}

/// One thing a supervisor allowed, and the sale it was allowed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waived {
    pub sale: String,
    pub terminal: String,
    pub rung_at_ms: u64,
    pub total_minor: i64,
    pub reason: String,
}

/// How much of one item left the shelf over a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoldLine {
    /// The item's id. Named by the screen from the catalogue it already holds,
    /// rather than by sending the same strings on every report for ever.
    pub item: String,
    pub qty_milli: i64,
    pub sales: u64,
}

/// One line of what passed between the shop and a supplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierLine {
    pub at_ms: u64,
    /// True when goods came in, false when money went out.
    pub delivered: bool,
    pub amount_minor: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// What the shop owes one supplier: the deliveries less what has been paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplierOwing {
    pub supplier: String,
    pub name: String,
    pub owed_minor: i64,
    pub deliveries: u32,
    pub since_ms: u64,
}

/// What was sold at one tax rate, and the tax on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatLine {
    pub vat_bp: u32,
    pub net_minor: i64,
    pub vat_minor: i64,
    pub sales: u64,
    /// Standard rated, zero rated or exempt, as the shop said. Carried because
    /// a rate of zero cannot say which of the last two a shop meant and a
    /// return declares them in different places: the shop's own figures group
    /// by it and the wire has always sent it, and this was the one hop that
    /// dropped it, so every line on that screen read as a percentage.
    #[serde(default)]
    pub supply: u8,
}

/// One till's part of a day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TillDay {
    pub terminal: String,
    pub sales: u64,
    pub total_minor: i64,
    pub needing_attention: u64,
}

/// A drawer a till has open, as that till last said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenDrawer {
    pub terminal: String,
    pub opened_at_ms: u64,
    /// When the till last said this, which is how stale the figure is.
    pub reported_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
}

/// What one person owes the shop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owing {
    /// What a payment is recorded against. Sent back rather than folded again
    /// on the screen, so both sides mean the same person.
    pub person_key: String,
    pub person_name: String,
    /// Positive is owed to the shop.
    pub owed_minor: i64,
    pub since_ms: u64,
    pub last_at_ms: u64,
    pub entries: u32,
}

/// One line of somebody's account: a sale that put money on it, or a payment
/// that took some off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountLine {
    pub source: String,
    pub is_sale: bool,
    /// True when it came off the account without money changing hands.
    pub written_off: bool,
    pub amount_minor: i64,
    pub at_ms: u64,
    pub note: String,
}

/// A drawer that was counted and closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedDrawer {
    pub id: String,
    pub terminal: String,
    /// What whoever counted it was called at the time. Empty for a drawer
    /// counted by a build that did not write it down.
    pub closed_by_name: String,
    pub opened_at_ms: u64,
    pub closed_at_ms: u64,
    pub opening_float_minor: i64,
    pub sales: u32,
    pub cash_sales_minor: i64,
    pub non_cash_sales_minor: i64,
    pub cash_in_minor: i64,
    pub cash_out_minor: i64,
    pub expected_cash_minor: i64,
    /// What the shop's own sales say the same drawer should have held, when it
    /// can say. Absent where it cannot: a drawer holding sales from before the
    /// shop worked this out has no figure of its own, and a zero there would
    /// read as a disagreement on every drawer in the shop's history.
    pub expected_from_sales_minor: Option<i64>,
    /// How much of that window's cash belongs to sales the shop has since
    /// struck out, when there is any. Absent where there is none, and where the
    /// shop cannot say.
    ///
    /// The two figures above disagree for good once a sale is struck out: the
    /// drawer keeps what the evening recorded, deliberately, because a
    /// duplicate that inflated the expectation is exactly what the shortfall
    /// that evening was. This is what lets a screen say so, rather than leaving
    /// an owner to read a gap and go and ask the person who counted.
    pub struck_out_cash_minor: Option<i64>,
    pub counted_cash_minor: i64,
    /// Counted less expected. Negative is short.
    pub variance_minor: i64,
}

/// A delivery that has already happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub supplier_id: Option<String>,
    pub reference: Option<String>,
    pub received_at_ms: u64,
    pub lines: Vec<ReceivedLine>,
    /// What the whole delivery cost, as the shop added it up. The screen used
    /// to multiply the lines out itself. Absent when the shop could not add it
    /// up, which takes figures no shop has.
    pub cost_minor: Option<i64>,
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
    /// When somebody last stood in front of that shelf and counted it. Absent
    /// where nobody ever has, which is the thing worth saying: the figure
    /// beside it is then deliveries and sales added up and nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted_at_ms: Option<u64>,
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
        Next::PushShifts => {
            let shifts = till
                .unsent_shifts()
                .iter()
                .map(|shift| openpos_core::protocol::ClosedShiftWire {
                    id: shift.id,
                    terminal: till.terminal().to_u128(),
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
                    // The shop works these out from its own sales. A till
                    // asserting them would be the same word twice.
                    expected_from_sales_minor: None,
                    struck_out_cash_minor: None,
                    counted_cash_minor: shift.counted_cash_minor,
                    variance_minor: shift.variance_minor,
                })
                .collect();
            Ok(Step::Post {
                kind: Exchange::Shifts,
                path: String::from("/v1/sync/shifts"),
                body: encode(&openpos_core::protocol::PushShiftsRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant,
                    terminal: till.terminal().to_u128(),
                    shifts,
                })?,
                token: till.token().map(String::from),
            })
        }
        Next::PushAllowed => {
            let allowed = till
                .unsent_allowed()
                .iter()
                .map(|one| openpos_core::protocol::AllowedWire {
                    seq: one.seq,
                    at_ms: one.at_ms,
                    action: one.action,
                    bp: one.bp,
                    operator: one.operator,
                    operator_name: one.operator_name.clone(),
                    authorised_by: one.authorised_by,
                    authorised_by_name: one.authorised_by_name.clone(),
                    receipt_no: one.receipt_no.clone(),
                })
                .collect();
            Ok(Step::Post {
                kind: Exchange::Allowed,
                path: String::from("/v1/sync/allowed"),
                body: encode(&openpos_core::protocol::PushAllowedRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant,
                    terminal: till.terminal().to_u128(),
                    allowed,
                })?,
                token: till.token().map(String::from),
            })
        }
        Next::RenewCredential => Ok(Step::Post {
            kind: Exchange::Renew,
            path: String::from("/v1/renew"),
            body: encode(&openpos_core::protocol::RenewRequest {
                protocol: PROTOCOL_VERSION,
            })?,
            token: till.token().map(String::from),
        }),
        Next::CheckSettings => Ok(Step::Post {
            kind: Exchange::Settings,
            path: String::from("/v1/settings"),
            body: encode(&openpos_core::protocol::SettingsRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
            })?,
            token: till.token().map(String::from),
        }),
        Next::FetchBalances => Ok(Step::Post {
            kind: Exchange::Balances,
            path: String::from("/v1/customers/owed"),
            body: encode(&openpos_core::protocol::BalancesRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
            })?,
            token: till.token().map(String::from),
        }),
        Next::PushItems => Ok(Step::Post {
            kind: Exchange::Items,
            path: String::from("/v1/sync/items"),
            body: encode(&openpos_core::protocol::PushItemsRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                items: till
                    .unsent_items()
                    .iter()
                    .cloned()
                    .map(into_wire_item)
                    .collect(),
            })?,
            token: till.token().map(String::from),
        }),
        Next::PushCustomers => Ok(Step::Post {
            kind: Exchange::People,
            path: String::from("/v1/sync/customers"),
            body: encode(&openpos_core::protocol::PushCustomersRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
                customers: till
                    .unsent_customers()
                    .iter()
                    .map(|written| openpos_core::protocol::CustomerWire {
                        id: written.id,
                        name: written.name.clone(),
                        phone: written.phone.clone(),
                        active: written.active,
                        bin: written.bin.clone(),
                        // A till writing somebody down at the counter sets no
                        // cap on them: that is the owner's to decide, in the
                        // back office, looking at what the shop can carry.
                        limit_minor: 0,
                    })
                    .collect(),
            })?,
            token: till.token().map(String::from),
        }),
        Next::FetchStock { from, limit } => Ok(Step::Post {
            kind: Exchange::Stock,
            path: String::from("/v1/stock"),
            body: encode(&openpos_core::protocol::OnHandRequest {
                protocol: PROTOCOL_VERSION,
                // The window this ask covers, named by the till: the server has
                // no idea which items this device holds or where it got to.
                item_ids: till
                    .item_window(from, limit)
                    .into_iter()
                    .map(|id| id.to_u128())
                    .collect(),
            })?,
            token: till.token().map(String::from),
        }),
        Next::FetchCustomers => Ok(Step::Post {
            kind: Exchange::Customers,
            path: String::from("/v1/customers"),
            body: encode(&openpos_core::protocol::CustomersRequest {
                protocol: PROTOCOL_VERSION,
                tenant,
                terminal: till.terminal().to_u128(),
            })?,
            token: till.token().map(String::from),
        }),
        Next::ReportDrawer => {
            // Built from the same X report a cashier reads on the screen, so
            // what the shop is told and what the till shows are one figure
            // taken once rather than two computed twice.
            let report = till
                .shift()
                .ok_or_else(|| String::from("no drawer is open on this terminal"))?
                .x_report()
                .map_err(|error| format!("{error}"))?;
            Ok(Step::Post {
                kind: Exchange::ReportDrawer,
                path: String::from("/v1/sync/drawer"),
                body: encode(&openpos_core::protocol::ReportDrawerRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant,
                    terminal: till.terminal().to_u128(),
                    shift: report.shift.to_u128(),
                    opened_at_ms: report.opened_at_ms,
                    at_ms: now_ms,
                    opening_float_minor: report.opening_float.get(),
                    sales: u32::try_from(report.sales).unwrap_or(u32::MAX),
                    cash_sales_minor: report.cash_sales.get(),
                    non_cash_sales_minor: report.non_cash_sales.get(),
                    cash_in_minor: report.cash_in.get(),
                    cash_out_minor: report.cash_out.get(),
                    expected_cash_minor: report.expected_cash.get(),
                })?,
                token: till.token().map(String::from),
            })
        }
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
            let response: PushResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the push reply did not decode"))?;
            // Quarantined sales count as settled: the server has them, and
            // holding them on the till would leave the only copy on a tablet.
            let settled: Vec<Ulid> = response
                .settled()
                .into_iter()
                .map(Ulid::from_u128)
                .collect();
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
            let response: PullResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the pull reply did not decode"))?;
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
        Exchange::AdminItem | Exchange::AdminDeleteItem => {
            postcard::from_bytes::<openpos_core::protocol::CatalogueEditResponse>(&bytes)
                .map_err(|_| String::from("the catalogue reply did not decode"))?;
            Applied::default()
        }
        Exchange::AdminCode => {
            let response: openpos_core::protocol::IssueCodeResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the enrolment code reply did not decode"))?;
            Applied {
                issued_code: Some(response.code),
                // How long it lasts, as the shop says. The screen used to state
                // an hour as a sentence of its own: true today, and a sentence
                // that becomes false the day a shop's policy changes, on the one
                // screen where somebody is standing at a device with a code in
                // their hand and no way to check.
                code_lasts_seconds: response.expires_in_seconds,
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
        Exchange::AdminCorrectStock => {
            let response: openpos_core::protocol::CorrectStockResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the correction reply did not decode"))?;
            Applied {
                // False when the shop already had this correction, which a
                // retry after a dropped reply is, and which is not an error.
                already_booked: !response.recorded,
                on_hand: response
                    .on_hand
                    .into_iter()
                    .map(|entry| OnHand {
                        item_id: Ulid::from_u128(entry.item_id).encode(),
                        qty_milli: entry.qty_milli,
                        unreconciled_milli: entry.unreconciled_milli,
                        unreconciled_sales: entry.unreconciled_sales,
                        // When it was last counted, and nothing when it never
                        // was. The wire's own words: a figure resting on no
                        // count is a running total, and a shop should be told
                        // rather than left to read it as a number somebody
                        // stood in front of the shelf for.
                        counted_at_ms: entry.counted_at_ms,
                    })
                    .collect(),
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
                        // When it was last counted, and nothing when it never
                        // was. The wire's own words: a figure resting on no
                        // count is a running total, and a shop should be told
                        // rather than left to read it as a number somebody
                        // stood in front of the shelf for.
                        counted_at_ms: entry.counted_at_ms,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminMade => {
            let response: openpos_core::protocol::MadeResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the reply about what was made did not decode"))?;
            Applied {
                made: Some(Made {
                    net_minor: response.net_minor,
                    cost_minor: response.cost_minor,
                    made_minor: response.made_minor,
                    sales: response.sales,
                    sales_without_cost: response.sales_without_cost,
                    net_without_cost_minor: response.net_without_cost_minor,
                }),
                ..Applied::default()
            }
        }
        Exchange::AdminReceipt => {
            let response: openpos_core::protocol::ReceiptResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the reply about the receipt did not decode"))?;
            Applied {
                on_paper: response
                    .found
                    .into_iter()
                    .map(|one| SaleOnPaper {
                        // The same reason the repair queue names, because it is
                        // the same fact shown in a second place.
                        held_for_kind: one
                            .held_for_kind
                            .as_ref()
                            .map_or_else(String::new, |why| held_for(why).0),
                        held_for_parts: one
                            .held_for_kind
                            .as_ref()
                            .map_or_else(BTreeMap::new, |why| held_for(why).1),
                        id: Ulid::from_u128(one.id).encode(),
                        terminal: Ulid::from_u128(one.terminal).encode(),
                        receipt_no: one.receipt_no,
                        rung_at_ms: one.rung_at_ms,
                        lines: one
                            .lines
                            .into_iter()
                            .map(|line| PaperLine {
                                item_id: Ulid::from_u128(line.item_id).encode(),
                                name: line.name,
                                qty_milli: line.qty_milli,
                                unit: line.unit,
                                unit_price_minor: line.unit_price_minor,
                                discount_minor: line.discount_minor,
                                vat_bp: line.vat_bp,
                                line_total_minor: line.line_total_minor,
                            })
                            .collect(),
                        tenders: one
                            .tenders
                            .into_iter()
                            .map(|tender| PaperTender {
                                kind: tender.kind,
                                kind_code: tender.kind_code,
                                amount_minor: tender.amount_minor,
                                reference: tender.reference,
                            })
                            .collect(),
                        net_minor: one.net_minor,
                        vat_minor: one.vat_minor,
                        discount_minor: one.discount_minor,
                        total_minor: one.total_minor,
                        change_minor: one.change_minor,
                        overrides: one.overrides,
                        held_for: one.held_for,
                        decided: one.decided,
                        still_counts: one.still_counts,
                        refunded_minor: one.refunded_minor,
                        refund_of: one.refund_of,
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
                    .map(|entry| {
                        let (kind, parts) = entry
                            .held_for
                            .as_ref()
                            .map_or_else(|| (String::new(), BTreeMap::new()), held_for);
                        Repair {
                            id: Ulid::from_u128(entry.id).encode(),
                            receipt_no: entry.receipt_no,
                            total_minor: entry.total_minor,
                            received_at_ms: entry.received_at_ms,
                            reason: entry.reason,
                            kind,
                            parts,
                        }
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminReceiptGaps => {
            let response: openpos_core::protocol::ReceiptGapsResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the reply about the numbering did not decode"))?;
            Applied {
                gaps: response
                    .gaps
                    .into_iter()
                    .map(|gap| Gap {
                        terminal: Ulid::from_u128(gap.terminal).encode(),
                        after: gap.after,
                        before: gap.before,
                        missing: gap.missing,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminAllowed => {
            let response: openpos_core::protocol::AllowedResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the reply about what was allowed did not decode"))?;
            Applied {
                allowed: response
                    .allowed
                    .into_iter()
                    .map(|one| Allowed {
                        terminal: Ulid::from_u128(one.terminal).encode(),
                        seq: one.seq,
                        at_ms: one.at_ms,
                        // Words rather than a number, because the number is for
                        // the bytes and the screen is for a person.
                        what: String::from(match one.action {
                            1 => "a discount",
                            2 => "a price typed over the catalogue's",
                            3 => "a refund",
                            4 => "a line taken off",
                            5 => "the drawer opened",
                            6 => "the drawer counted and closed",
                            7 => "a PIN typed wrongly",
                            8 => "a PIN typed wrongly, and that person locked out",
                            9 => "took the till",
                            10 => "more sold than the shop has",
                            11 => {
                                "tried to take a line off a basket that had \
                                    been paid towards"
                            }
                            12 => "sold to somebody already past what they may owe",
                            13 => "tried to open the drawer",
                            14 => "printed a receipt again",
                            _ => "something this build does not know about",
                        }),
                        kind: one.action,
                        bp: one.bp,
                        operator_name: one.operator_name,
                        authorised_by_name: one.authorised_by_name,
                        refused: matches!(one.action, 7 | 8),
                        took_the_till: one.action == 9,
                        // Eleven is a line taken off a paid basket, thirteen is
                        // the drawer: both are somebody who tried and could not.
                        was_not_permitted: matches!(one.action, 11 | 13),
                        needed_no_permission: one.action == 14,
                        receipt_no: one.receipt_no,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminDecided => {
            let response: openpos_core::protocol::DecidedResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the decided reply did not decode"))?;
            Applied {
                decided: response
                    .decided
                    .into_iter()
                    .map(|one| Decided {
                        id: Ulid::from_u128(one.id).encode(),
                        receipt_no: one.receipt_no,
                        total_minor: one.total_minor,
                        reason: one.reason,
                        note: one.note,
                        kept: one.kept,
                        decided_at_ms: one.decided_at_ms,
                        decisions: one.decisions,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminDecideAgain => {
            let response: openpos_core::protocol::DecideAgainResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the decide-again reply did not decode"))?;
            Applied {
                decision_changed: response.changed,
                // Somebody else answered while the list was open. The screen
                // reads again rather than putting a stale view back.
                decision_stale: response.stale,
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
        Exchange::Customers => {
            let response: openpos_core::protocol::CustomersResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the customers reply did not decode"))?;
            let customers = response
                .customers
                .into_iter()
                .map(|one| openpos_core::storage::wire::CustomerV1 {
                    id: one.id,
                    name: one.name,
                    phone: one.phone,
                    active: one.active,
                    bin: one.bin,
                    // What the shop says they may owe. Dropped here at first,
                    // which made a cap the back office could set and no till
                    // could enforce: the whole rule arrived and was thrown away
                    // one line before it was used.
                    limit_minor: one.limit_minor,
                })
                .collect();
            till.set_customers(customers)
                .map_err(|error| format!("{error}"))?;
            driver.fetched_customers(now_ms);
            Applied::default()
        }
        Exchange::Renew => {
            let response: openpos_core::protocol::RenewResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the renewal reply did not decode"))?;
            // Written down before anything else happens. A device that acts on
            // this reply without storing the credential has thrown away the one
            // it was given and kept one the shop is about to stop accepting.
            till.take_credential(
                &response.token,
                now_ms,
                response.expires_in_seconds.saturating_mul(1_000),
            )
            .map_err(|error| format!("{error}"))?;
            Applied::default()
        }
        Exchange::Settings => {
            let response: openpos_core::protocol::SettingsResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the settings reply did not decode"))?;
            // A number that has moved makes the three lists due again. That is
            // the whole point of asking for one number often.
            driver.settings_seq(response.seq, now_ms);
            Applied::default()
        }
        Exchange::Balances => {
            let response: openpos_core::protocol::BalancesResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the balances reply did not decode"))?;
            till.set_balances(
                response
                    .balances
                    .into_iter()
                    .map(|one| (one.customer, one.owed_minor))
                    .collect(),
                now_ms,
            );
            driver.fetched_balances(now_ms);
            Applied::default()
        }
        Exchange::Items => {
            let response: openpos_core::protocol::PushItemsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the items reply did not decode"))?;
            let stored = response.stored.len();
            till.items_accepted(&response.stored)
                .map_err(|error| format!("{error}"))?;
            Applied {
                items_taken: stored,
                ..Applied::default()
            }
        }
        Exchange::People => {
            let response: openpos_core::protocol::PushCustomersResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the customers reply did not decode"))?;
            let stored = response.stored.len();
            till.customers_accepted(&response.stored)
                .map_err(|error| format!("{error}"))?;
            Applied {
                people_taken: stored,
                ..Applied::default()
            }
        }
        Exchange::Stock => {
            let response: openpos_core::protocol::OnHandResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the stock reply did not decode"))?;
            let figures: alloc::vec::Vec<_> = response
                .on_hand
                .iter()
                .map(|entry| {
                    (
                        openpos_core::ids::Ulid::from_u128(entry.item_id),
                        openpos_core::money::Milli::new(entry.qty_milli),
                    )
                })
                .collect();
            let taken = till.apply_on_hand(&figures);
            // Recorded as asked whatever came back, and the window moves on
            // either way: a shop that answered about items this till no longer
            // holds should not make it ask about them for ever.
            let round = driver.fetched_stock(
                now_ms,
                till.catalogue().len(),
                till.catalogue().shape_moved(),
            );
            // The lap closed, so this device has now been told what the shop
            // believes it holds of everything it sells, and its shelf rule can
            // start meaning something. Before that it holds a figure for the
            // items whose turn has come and nothing for the rest, and nothing
            // reads as none: a till enrolled this morning refused an item the
            // shop had sixty-one of.
            if round {
                till.shelf_swept();
            }
            Applied {
                stock_taken: taken,
                ..Applied::default()
            }
        }
        Exchange::ReportDrawer => {
            let _: openpos_core::protocol::ReportDrawerResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the drawer report reply did not decode"))?;
            driver.reported_drawer(now_ms);
            Applied::default()
        }
        Exchange::AdminShifts => {
            let response: openpos_core::protocol::ShiftsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the shifts reply did not decode"))?;
            Applied {
                shifts: response
                    .shifts
                    .into_iter()
                    .map(|one| ClosedDrawer {
                        id: Ulid::from_u128(one.id).encode(),
                        terminal: Ulid::from_u128(one.terminal).encode(),
                        closed_by_name: one.closed_by_name,
                        opened_at_ms: one.opened_at_ms,
                        closed_at_ms: one.closed_at_ms,
                        opening_float_minor: one.opening_float_minor,
                        sales: one.sales,
                        cash_sales_minor: one.cash_sales_minor,
                        non_cash_sales_minor: one.non_cash_sales_minor,
                        cash_in_minor: one.cash_in_minor,
                        cash_out_minor: one.cash_out_minor,
                        expected_cash_minor: one.expected_cash_minor,
                        expected_from_sales_minor: one.expected_from_sales_minor,
                        struck_out_cash_minor: one.struck_out_cash_minor,
                        counted_cash_minor: one.counted_cash_minor,
                        variance_minor: one.variance_minor,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminAdoptSales => {
            let response: openpos_core::protocol::AdoptSalesResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the reply to those carried sales did not decode"))?;
            Applied {
                adopted: response.adopted.len(),
                adopted_needing_attention: response.needing_attention.len(),
                ..Applied::default()
            }
        }
        Exchange::AdminDay => {
            let response: openpos_core::protocol::DayResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the day reply did not decode"))?;
            Applied {
                day: Some(Day {
                    sales: response.sales,
                    total_minor: response.total_minor,
                    refunds: response.refunds,
                    refunded_minor: response.refunded_minor,
                    drawers_counted: response.drawers_counted,
                    expected_cash_minor: response.expected_cash_minor,
                    counted_cash_minor: response.counted_cash_minor,
                    variance_minor: response.variance_minor,
                    charged_minor: response.charged_minor,
                    returned_minor: response.returned_minor,
                    paid_minor: response.paid_minor,
                    written_off_minor: response.written_off_minor,
                    tills: response
                        .tills
                        .into_iter()
                        .map(|till| TillDay {
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
        Exchange::AdminVat => {
            let response: openpos_core::protocol::VatResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the tax reply did not decode"))?;
            Applied {
                vat: response
                    .rows
                    .into_iter()
                    .map(|row| VatLine {
                        vat_bp: row.vat_bp,
                        net_minor: row.net_minor,
                        vat_minor: row.vat_minor,
                        sales: row.sales,
                        supply: row.supply,
                    })
                    .collect(),
                vat_waiting_sales: response.waiting_sales,
                vat_waiting_minor: response.waiting_vat_minor,
                vat_minor: response.vat_minor,
                ..Applied::default()
            }
        }
        Exchange::AdminRevokeTerminal => {
            let response: openpos_core::protocol::RevokeTerminalResponse =
                postcard::from_bytes(&bytes).map_err(|_| {
                    String::from("the reply to cutting that device off did not decode")
                })?;
            Applied {
                withdrawn: Some(response.withdrawn),
                ..Applied::default()
            }
        }
        Exchange::AdminItemsFromTills => {
            let response: openpos_core::protocol::TillItemsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the reply about items from tills did not decode"))?;
            Applied {
                from_tills: response
                    .items
                    .iter()
                    .map(crate::WireItem::from_wire)
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminResendCatalogue => {
            let response: openpos_core::protocol::ResendCatalogueResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("that reply did not decode"))?;
            Applied {
                resent: response.sent,
                ..Applied::default()
            }
        }
        Exchange::AdminUnreadable => {
            let response: openpos_core::protocol::UnreadableChangesResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("that reply did not decode"))?;
            Applied {
                unreadable: response
                    .changes
                    .into_iter()
                    .map(|change| UnreadableChange {
                        seq: change.seq,
                        item: Ulid::from_u128(change.item_id).encode(),
                        schema: change.schema,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminWaived => {
            let response: openpos_core::protocol::WaivedResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("that reply did not decode"))?;
            Applied {
                waived: response
                    .waived
                    .into_iter()
                    .map(|one| Waived {
                        sale: Ulid::from_u128(one.sale_id).encode(),
                        terminal: Ulid::from_u128(one.terminal).encode(),
                        rung_at_ms: one.rung_at_ms,
                        total_minor: one.total_minor,
                        reason: one.reason,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminSold => {
            let response: openpos_core::protocol::SoldResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the sold reply did not decode"))?;
            Applied {
                sold: response
                    .rows
                    .into_iter()
                    .map(|row| SoldLine {
                        item: Ulid::from_u128(row.item_id).encode(),
                        qty_milli: row.qty_milli,
                        sales: row.sales,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminSupplierStatement => {
            let response: openpos_core::protocol::SupplierStatementResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the statement did not decode"))?;
            Applied {
                statement: response
                    .entries
                    .into_iter()
                    .map(|entry| SupplierLine {
                        at_ms: entry.at_ms,
                        delivered: entry.delivered,
                        amount_minor: entry.amount_minor,
                        reference: entry.reference,
                    })
                    .collect(),
                owed_now: Some(response.owed_minor),
                ..Applied::default()
            }
        }
        Exchange::AdminSupplierOwing => {
            let response: openpos_core::protocol::SupplierOwingResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the supplier reply did not decode"))?;
            Applied {
                supplier_owing: response
                    .owing
                    .into_iter()
                    .map(|one| SupplierOwing {
                        supplier: Ulid::from_u128(one.supplier_id).encode(),
                        name: one.name,
                        owed_minor: one.owed_minor,
                        deliveries: one.deliveries,
                        since_ms: one.since_ms,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminPaySupplier => {
            let response: openpos_core::protocol::PaySupplierResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the payment reply did not decode"))?;
            Applied {
                // False means it was already recorded, which is ordinary rather
                // than a failure: a dropped reply is why one is sent twice.
                already_paid: !response.paid,
                owed_now: Some(response.owed_minor),
                ..Applied::default()
            }
        }
        Exchange::AdminItemNow => {
            let response: openpos_core::protocol::ItemNowResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("that item did not decode"))?;
            Applied {
                item_now: response.item.map(|item| crate::WireItem::from_wire(&item)),
                item_seq: Some(response.seq),
                ..Applied::default()
            }
        }
        Exchange::AdminShopNow => {
            let response: openpos_core::protocol::ShopResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the shop reply did not decode"))?;
            Applied {
                shop: Some(ShopNow {
                    name: response.name,
                    bin: response.bin,
                    address: response.address,
                    phone: response.phone,
                    wallets: response.wallets,
                    stock_rule: response.stock_rule,
                }),
                ..Applied::default()
            }
        }
        Exchange::AdminCustomers => {
            let response: openpos_core::protocol::CustomersResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the customers reply did not decode"))?;
            // Held on this device too, so the back office shows the list from
            // the same place a till reads it rather than from its own memory.
            let customers: Vec<openpos_core::storage::wire::CustomerV1> = response
                .customers
                .into_iter()
                .map(|one| openpos_core::storage::wire::CustomerV1 {
                    id: one.id,
                    name: one.name,
                    phone: one.phone,
                    active: one.active,
                    bin: one.bin,
                    limit_minor: 0,
                })
                .collect();
            let everyone = customers
                .iter()
                .map(|one| crate::Customer {
                    id: Ulid::from_u128(one.id).encode(),
                    name: one.name.clone(),
                    phone: one.phone.clone(),
                    active: one.active,
                    // The back office reads what anybody owes from the book
                    // itself, which is the shop's own figure rather than a
                    // till's copy of it.
                    owed_minor: None,
                    owed_as_of_ms: None,
                    bin: one.bin.clone(),
                    limit_minor: one.limit_minor,
                })
                .collect();
            till.set_customers(customers)
                .map_err(|error| format!("{error}"))?;
            Applied {
                every_customer: everyone,
                ..Applied::default()
            }
        }
        Exchange::AdminOpenDrawers => {
            let response: openpos_core::protocol::OpenDrawersResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the open drawers reply did not decode"))?;
            Applied {
                open_drawers: response
                    .drawers
                    .into_iter()
                    .map(|one| OpenDrawer {
                        terminal: Ulid::from_u128(one.terminal).encode(),
                        opened_at_ms: one.opened_at_ms,
                        reported_at_ms: one.reported_at_ms,
                        opening_float_minor: one.opening_float_minor,
                        sales: one.sales,
                        cash_sales_minor: one.cash_sales_minor,
                        non_cash_sales_minor: one.non_cash_sales_minor,
                        cash_in_minor: one.cash_in_minor,
                        cash_out_minor: one.cash_out_minor,
                        expected_cash_minor: one.expected_cash_minor,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminOwed => {
            let response: openpos_core::protocol::OwedResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the account book reply did not decode"))?;
            Applied {
                owed: response
                    .owing
                    .into_iter()
                    .map(|one| Owing {
                        person_key: one.person_key,
                        person_name: one.person_name,
                        owed_minor: one.owed_minor,
                        since_ms: one.since_ms,
                        last_at_ms: one.last_at_ms,
                        entries: one.entries,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminTakePayment => {
            let response: openpos_core::protocol::TakePaymentResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the payment reply did not decode"))?;
            Applied {
                // False means it was already recorded, which is ordinary rather
                // than a failure: a dropped reply is why one is sent twice.
                already_paid: !response.taken,
                owed_now: Some(response.owed_minor),
                ..Applied::default()
            }
        }
        Exchange::AdminAccount => {
            let response: openpos_core::protocol::AccountResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the account reply did not decode"))?;
            Applied {
                account: response
                    .entries
                    .into_iter()
                    .map(|one| AccountLine {
                        source: Ulid::from_u128(one.source_id).encode(),
                        is_sale: one.is_sale,
                        written_off: one.written_off,
                        amount_minor: one.amount_minor,
                        at_ms: one.at_ms,
                        note: one.note,
                    })
                    .collect(),
                ..Applied::default()
            }
        }
        Exchange::AdminDeliveries => {
            let response: openpos_core::protocol::DeliveriesResponse = postcard::from_bytes(&bytes)
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
                        cost_minor: one.cost_minor,
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
            let response: openpos_core::protocol::SuppliersResponse = postcard::from_bytes(&bytes)
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
            let response: openpos_core::protocol::OnHandResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the stock reply did not decode"))?;
            Applied {
                on_hand_whole: response.whole,
                on_hand: response
                    .on_hand
                    .into_iter()
                    .map(|entry| OnHand {
                        item_id: Ulid::from_u128(entry.item_id).encode(),
                        qty_milli: entry.qty_milli,
                        unreconciled_milli: entry.unreconciled_milli,
                        unreconciled_sales: entry.unreconciled_sales,
                        // When it was last counted, and nothing when it never
                        // was. The wire's own words: a figure resting on no
                        // count is a running total, and a shop should be told
                        // rather than left to read it as a number somebody
                        // stood in front of the shelf for.
                        counted_at_ms: entry.counted_at_ms,
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
                        role: entry.role,
                        // When the shop took this device on. A list of devices
                        // is read when one of them is to be cut off, and the
                        // question then is which of two tills with similar
                        // names is the one somebody enrolled last week.
                        enrolled_at_ms: entry.enrolled_at_ms,
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
            till.set_shop(
                receipt::Shop {
                    name: response.name,
                    bin: response.bin,
                    address: response.address,
                    phone: response.phone,
                },
                response.wallets.into_iter().map(Into::into).collect(),
                openpos_core::domain::StockRule::from_u8(response.stock_rule),
            )
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
        Exchange::Shifts => {
            let response: openpos_core::protocol::PushShiftsResponse = postcard::from_bytes(&bytes)
                .map_err(|_| String::from("the shifts reply did not decode"))?;
            // What the server said it holds, never what was sent: a reply that
            // did not arrive must leave the count on the device to send again.
            till.shifts_accepted(&response.accepted)
                .map_err(|error| format!("{error}"))?;
            Applied::default()
        }
        Exchange::Allowed => {
            let response: openpos_core::protocol::PushAllowedResponse =
                postcard::from_bytes(&bytes)
                    .map_err(|_| String::from("the reply about what was allowed did not decode"))?;
            // What the server said it holds, never what was sent: a reply that
            // did not arrive must leave the trail on the device to send again.
            till.allowed_accepted(&response.stored)
                .map_err(|error| format!("{error}"))?;
            Applied::default()
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

pub fn from_hex_public(text: &str) -> Option<Vec<u8>> {
    from_hex(text)
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
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

pub(crate) fn from_hex(text: &str) -> Option<Vec<u8>> {
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

    /// A return tells zero rated from exempt, because a shop declares them in
    /// different places.
    ///
    /// The shop's own figures are grouped by the kind of supply for exactly
    /// that reason, and the wire has always carried it. This crossing dropped
    /// it, so every line on the return read as a percentage: two rows both
    /// saying "0%", which is the one thing that screen exists to tell apart.
    #[test]
    fn a_return_says_which_nothing_it_is() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{VatResponse, VatRowWire};
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

        let response = VatResponse {
            protocol: PROTOCOL_VERSION,
            rows: alloc::vec![
                VatRowWire {
                    vat_bp: 1_500,
                    net_minor: 72_901_25,
                    vat_minor: 10_935_18,
                    sales: 32,
                    supply: 0,
                },
                VatRowWire {
                    vat_bp: 0,
                    net_minor: 5_000_00,
                    vat_minor: 0,
                    sales: 4,
                    supply: 1,
                },
                VatRowWire {
                    vat_bp: 0,
                    net_minor: 1_200_00,
                    vat_minor: 0,
                    sales: 2,
                    supply: 2,
                },
            ],
            waiting_sales: 0,
            waiting_vat_minor: 0,
            vat_minor: 10_935_18,
        };
        let body = to_hex(&postcard::to_allocvec(&response).expect("it encodes"));

        let applied = apply(&mut till, &mut driver, Exchange::AdminVat, &body, 1)
            .expect("the reply decodes");

        assert_eq!(applied.vat.len(), 3);
        assert_eq!(applied.vat[0].supply, 0, "standard rated");
        assert_eq!(applied.vat[1].supply, 1, "zero rated");
        assert_eq!(applied.vat[2].supply, 2, "exempt");
        // Both of the last two are nothing, and a screen with only the rate to
        // go on would print "0%" twice and leave the shop to guess which line
        // belongs in which box on the return.
        assert_eq!(applied.vat[1].vat_bp, applied.vat[2].vat_bp);
    }

    /// The trail hands the screen which receipt was printed again.
    ///
    /// Written because it was not. The till recorded it, the wire carried it
    /// and the shop's store held it, and this crossing dropped it on the floor:
    /// the back office showed "printed a receipt again" with no number, which
    /// is the shape of defect this codebase keeps finding, a lower layer being
    /// careful and the last hop throwing the care away. Found by walking a
    /// reprint and reading the row in Postgres afterwards.
    #[test]
    fn the_trail_tells_a_screen_which_receipt_was_printed_again() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{AllowedEntry, AllowedResponse};
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

        let entry = |seq: u64, action: u8, receipt_no: Option<&str>| AllowedEntry {
            terminal: 7,
            seq,
            at_ms: 1_788_600_000_000 + seq,
            action,
            bp: 0,
            operator: 71,
            operator_name: String::from("Rahima"),
            authorised_by: 0,
            authorised_by_name: String::new(),
            receipt_no: receipt_no.map(String::from),
        };
        let response = AllowedResponse {
            protocol: PROTOCOL_VERSION,
            allowed: alloc::vec![
                entry(1, 14, Some("T1-000104")),
                // A reprint from a till that did not record which. Absent
                // rather than empty, and nothing invented for it here.
                entry(2, 14, None),
                // And a drawer opening, which is of no receipt at all.
                entry(3, 5, None),
            ],
        };
        let body = to_hex(&postcard::to_allocvec(&response).expect("it encodes"));

        let applied = apply(&mut till, &mut driver, Exchange::AdminAllowed, &body, 1)
            .expect("the reply decodes");

        assert_eq!(applied.allowed.len(), 3);
        assert_eq!(
            applied.allowed[0].receipt_no.as_deref(),
            Some("T1-000104"),
            "a shop asking about a second piece of paper is asking which one"
        );
        assert!(applied.allowed[0].needed_no_permission);
        assert_eq!(applied.allowed[1].receipt_no, None);
        assert_eq!(applied.allowed[2].receipt_no, None);
        // The English underneath the dictionary knows this kind, so a screen
        // with no word for it says what happened rather than that this build
        // does not know.
        assert_eq!(applied.allowed[0].what, "printed a receipt again");
    }

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
                    held_for: Some(
                        openpos_core::protocol::QuarantineReason::DuplicateReceiptNumber {
                            receipt_no: String::from("T1-000100"),
                        },
                    ),
                },
                RepairEntry {
                    id: 901,
                    // A till that sold without a leased block. Absent, not
                    // empty: a screen showing "" reads as a number nobody typed.
                    receipt_no: None,
                    total_minor: 1_200,
                    received_at_ms: 1_788_600_100_000,
                    reason: String::from("the payload could not be decoded"),
                    // A sale held before the shop kept the reason itself: the
                    // sentence is all any screen will ever have for it.
                    held_for: None,
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

        // And beside the prose, a name for what happened and the figures in it,
        // so a screen in Bangla can say it rather than showing one English
        // paragraph at the moment the shop is asked to make a judgement.
        assert_eq!(applied.repairs[0].kind, "duplicate-receipt");
        // A clock that is wrong names which way it is wrong, because "after"
        // and "before" are words and a word poured into another language's
        // sentence reads as English in the middle of it.
        let (kind, parts) = held_for(&openpos_core::protocol::QuarantineReason::ClockOutOfRange {
            rung_at_ms: 1_788_600_000_000 + 3 * 60 * 60 * 1_000,
            received_at_ms: 1_788_600_000_000,
        });
        assert_eq!(kind, "clock-after");
        assert_eq!(parts.get("how_far").map(String::as_str), Some("3"));
        assert_eq!(parts.get("unit").map(String::as_str), Some("hours"));

        // Under a minute is said as under a minute rather than rounded up to
        // one. The server's own sentence says that, and two places wording the
        // same gap differently is a shop reading two answers about one sale.
        let (_, brief) = held_for(&openpos_core::protocol::QuarantineReason::ClockOutOfRange {
            rung_at_ms: 1_788_600_000_000 + 30_000,
            received_at_ms: 1_788_600_000_000,
        });
        assert_eq!(brief.get("unit").map(String::as_str), Some("moment"));

        // And a reason about goods coming back names which goods: the shop has
        // the id, this device has the names, and a queue that says a quantity
        // and not a thing is a queue nobody can act on.
        let (kind, goods) = held_for(
            &openpos_core::protocol::QuarantineReason::MoreCameBackThanWentOut {
                receipt_no: String::from("T1-000100"),
                item_id: 1,
                over_by_milli: 2_500,
            },
        );
        assert_eq!(kind, "more-came-back");
        assert!(goods.contains_key("item"), "{goods:?}");
        assert!(
            !parts.contains_key("direction"),
            "the direction picks the sentence rather than being said in it"
        );
        assert_eq!(
            applied.repairs[0]
                .parts
                .get("receipt_no")
                .map(String::as_str),
            Some("T1-000100")
        );
        // A sale held before the shop kept the reason itself has no name for it,
        // and the sentence is all any screen will ever have.
        assert_eq!(applied.repairs[1].kind, "");
        assert!(applied.repairs[1].parts.is_empty());
        assert!(applied.repairs[1].reason.contains("could not be decoded"));
    }

    /// The shop's own figure for a counted drawer reaches the screen.
    ///
    /// It did not. The server worked it out, the protocol carried it, and this
    /// layer's own shape for a drawer had no field to put it in, so the back
    /// office silently showed nothing: the whole point of the figure is that
    /// somebody reads it beside the till's, and nobody could.
    #[test]
    fn what_the_shops_own_sales_say_a_drawer_held_reaches_the_screen() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{ClosedShiftWire, ShiftsResponse};
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

        let response = ShiftsResponse {
            protocol: PROTOCOL_VERSION,
            shifts: alloc::vec![ClosedShiftWire {
                id: 700,
                terminal: 7,
                closed_by: 91,
                closed_by_name: String::from("Rahima"),
                opened_at_ms: 1_788_600_000_000,
                closed_at_ms: 1_788_640_000_000,
                opening_float_minor: 30_000,
                sales: 1,
                cash_sales_minor: 49_450,
                non_cash_sales_minor: 0,
                cash_in_minor: 0,
                cash_out_minor: 0,
                expected_cash_minor: 79_450,
                // The till has not sent that sale yet, so the shop's own
                // figure is its float and nothing else.
                expected_from_sales_minor: Some(30_000),
                // And nothing has been struck out of it.
                struck_out_cash_minor: None,
                counted_cash_minor: 79_450,
                variance_minor: 0,
            }],
        };
        let hex = to_hex_public(&postcard::to_allocvec(&response).expect("encodes"));
        let applied = apply(
            &mut till,
            &mut driver,
            Exchange::AdminShifts,
            &hex,
            1_788_700_000_000,
        )
        .expect("the reply applies");

        let seen = applied.shifts;
        assert_eq!(seen.len(), 1, "a list of drawers");
        assert_eq!(seen[0].expected_cash_minor, 79_450, "what the till said");
        assert_eq!(
            seen[0].struck_out_cash_minor, None,
            "nothing struck out of this one, and a screen shows nothing about it"
        );
        assert_eq!(
            seen[0].expected_from_sales_minor,
            Some(30_000),
            "and what the shop's own sales come to"
        );
    }

    /// Everything a till writes down about a counted drawer reaches the shop.
    ///
    /// The other direction of the crossing that lost three fields in a day, and
    /// the one where losing something cannot be put right by asking again: a
    /// drawer is counted once, by a person, at the end of an evening. If a
    /// field is dropped on the way out, the shop never had it.
    #[test]
    fn everything_a_till_counted_reaches_the_shop() {
        use openpos_core::auth::{Operator, Permissions, PinHash};
        use openpos_core::cart::CartLimits;
        use openpos_core::money::Minor;
        use openpos_core::protocol::PushShiftsRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (mut till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::unrestricted(),
        )
        .expect("a till opens");
        till.put_operator(Operator {
            id: Ulid::from_u128(91),
            name: "Rahima".into(),
            pin: PinHash::derive("4321", [7; openpos_core::auth::SALT_LEN], 1_000),
            permissions: Permissions::supervisor(),
            active: true,
        })
        .expect("the shop's own person");
        till.sign_in(Ulid::from_u128(91), "4321", 0)
            .expect("she signs in");
        till.open_shift(Ulid::from_u128(80), Minor::new(30_000), 1_000)
            .expect("the drawer opens");
        till.cash_out(Minor::new(5_000), "paid the milk man", 1_500)
            .expect("money out of the drawer");
        let report = till
            .close_shift(Minor::new(24_550), 3_000)
            .expect("she counts it");

        // A block of numbers first, because a till with none asks for those
        // before anything else and this test is about what it sends next.
        till.grant_lease(&openpos_core::lease::Lease {
            terminal: Ulid::from_u128(7),
            epoch: 1,
            prefix: "T1".into(),
            next: 100,
            last: 599,
        })
        .expect("the shop grants numbers");

        let mut driver = Driver::default();
        let Step::Post { body, path, .. } =
            step(&till, &driver, 42, true, false, 4_000).expect("a step")
        else {
            panic!("a counted drawer is a post");
        };
        assert_eq!(path, "/v1/sync/shifts");
        let sent: PushShiftsRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("decodes");
        let one = sent.shifts.first().expect("the drawer she counted");

        // Every figure on the slip, against the report the core made.
        assert_eq!(one.terminal, Ulid::from_u128(7).to_u128());
        assert_eq!(one.closed_by, 91, "who counted it");
        assert_eq!(one.closed_by_name, "Rahima", "and what she is called");
        assert_eq!(one.opened_at_ms, 1_000);
        assert_eq!(one.closed_at_ms, 3_000);
        assert_eq!(one.opening_float_minor, 30_000);
        assert_eq!(one.sales, 0);
        assert_eq!(one.cash_in_minor, 0);
        assert_eq!(one.cash_out_minor, 5_000, "the milk man");
        assert_eq!(one.expected_cash_minor, report.totals.expected_cash.get());
        assert_eq!(one.counted_cash_minor, 24_550);
        assert_eq!(
            one.variance_minor,
            report.variance.get(),
            "and the difference"
        );
        // The shop works this one out from its own sales; a till asserting it
        // would be the same word twice.
        assert_eq!(one.expected_from_sales_minor, None);
        let _ = &mut driver;
    }

    /// And everything a till writes down about an item it invented at the
    /// counter, which is the only record of a price somebody sold at.
    #[test]
    fn everything_a_till_wrote_down_about_an_item_reaches_the_shop() {
        use openpos_core::cart::CartLimits;
        use openpos_core::money::{Bp, Milli, Minor};
        use openpos_core::protocol::PushItemsRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (mut till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::unrestricted(),
        )
        .expect("a till opens");
        till.quick_add(openpos_core::replica::Item {
            id: Ulid::from_u128(9),
            code: "BISCUIT".into(),
            name_en: "Biscuits, the new ones".into(),
            name_bn: "বিস্কুট".into(),
            unit: "packet".into(),
            price: Minor::new(12_000),
            cost: Minor::ZERO,
            vat_rate: Bp::new(1_500).expect("a real rate"),
            price_mode: openpos_core::domain::PriceMode::Inclusive,
            vat_base: openpos_core::domain::VatBase::Discounted,
            barcodes: alloc::vec!["8690000009999".into()],
            on_hand: Milli::ZERO,
            active: true,
            supply: openpos_core::domain::Supply::ZeroRated,
            category: "Biscuits".into(),
        })
        .expect("a delivery nobody booked, sold to keep the queue moving");

        // A block of numbers first, because a till with none asks for those
        // before anything else and this test is about what it sends next.
        till.grant_lease(&openpos_core::lease::Lease {
            terminal: Ulid::from_u128(7),
            epoch: 1,
            prefix: "T1".into(),
            next: 100,
            last: 599,
        })
        .expect("the shop grants numbers");

        let driver = Driver::default();
        let Step::Post { body, path, .. } =
            step(&till, &driver, 42, true, false, 4_000).expect("a step")
        else {
            panic!("an item written down is a post");
        };
        assert_eq!(path, "/v1/sync/items");
        let sent: PushItemsRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("decodes");
        let one = sent.items.first().expect("what the cashier wrote down");

        assert_eq!(one.code, "BISCUIT");
        assert_eq!(one.name_en, "Biscuits, the new ones");
        assert_eq!(one.name_bn, "বিস্কুট", "Bangla survives the crossing");
        assert_eq!(one.unit, "packet");
        assert_eq!(one.price_minor, 12_000);
        assert_eq!(one.vat_bp, 1_500);
        assert!(
            one.price_inclusive,
            "the price the cashier typed is what it costs"
        );
        assert_eq!(one.barcodes.len(), 1);
        assert_eq!(one.supply, 1, "zero rated, as the cashier was told");
        assert_eq!(one.category, "Biscuits");
        assert!(
            one.from_a_till,
            "a price typed to keep a queue moving is not a price the shop agreed"
        );
    }

    /// Everything the shop says about an item reaches the till.
    ///
    /// Three times in one day a field was added to the wire, filled in on both
    /// ends, and dropped in the middle: the classification for tax, the shop's
    /// own sorting, and the cap on what somebody may owe. Each time the tests
    /// on either side passed, because each side was right. This one walks the
    /// whole item across and compares field by field, so the next one fails
    /// here rather than in a shop.
    #[test]
    fn everything_the_shop_says_about_an_item_reaches_the_till() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{ItemWire, PullResponse};
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

        // Every field set to something a default would not produce, so a field
        // dropped on the way reads as a difference rather than as a match.
        let sent = ItemWire {
            id: 1,
            code: String::from("RICE5"),
            name_en: String::from("Rice Miniket 5kg"),
            name_bn: String::from("মিনিকেট চাল ৫ কেজি"),
            unit: String::from("kg"),
            price_minor: 43_000,
            cost_minor: 38_000,
            vat_bp: 750,
            price_inclusive: true,
            vat_on_undiscounted: true,
            barcodes: alloc::vec![String::from("8690000000001")],
            on_hand_milli: 40_000,
            active: true,
            from_a_till: false,
            supply: 2,
            category: String::from("Rice"),
        };
        let hex = to_hex_public(
            &postcard::to_allocvec(&PullResponse {
                protocol: PROTOCOL_VERSION,
                cursor: 9,
                upserts: alloc::vec![sent.clone()],
                tombstones: alloc::vec![],
                more: false,
            })
            .expect("encodes"),
        );
        apply(&mut till, &mut driver, Exchange::Pull, &hex, 1).expect("the page applies");

        let held = till
            .catalogue()
            .items()
            .first()
            .cloned()
            .expect("the till holds it");
        assert_eq!(&*held.code, sent.code);
        assert_eq!(&*held.name_en, sent.name_en);
        assert_eq!(&*held.name_bn, sent.name_bn, "Bangla survives the crossing");
        assert_eq!(&*held.unit, sent.unit);
        assert_eq!(held.price.get(), sent.price_minor);
        assert_eq!(held.cost.get(), sent.cost_minor, "what the shop pays");
        assert_eq!(held.vat_rate.get(), sent.vat_bp);
        assert_eq!(
            held.price_mode,
            openpos_core::domain::PriceMode::Inclusive,
            "a shelf price with the tax in it"
        );
        assert_eq!(
            held.vat_base,
            openpos_core::domain::VatBase::Undiscounted,
            "tax fixed to the listed price"
        );
        assert_eq!(
            held.supply,
            openpos_core::domain::Supply::Exempt,
            "what it is for tax"
        );
        assert_eq!(
            &*held.category, sent.category,
            "what the shop sorts it under"
        );
        assert_eq!(held.on_hand.get(), sent.on_hand_milli);
        assert_eq!(held.active, sent.active);
        assert_eq!(held.barcodes.len(), 1);
    }

    /// Everything the shop says about a person reaches the till, for the same
    /// reason: this is the crossing that lost a credit limit.
    #[test]
    fn everything_the_shop_says_about_a_person_reaches_the_till() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{CustomerWire, CustomersResponse};
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

        let sent = CustomerWire {
            id: 21,
            name: String::from("Karim, flat 3"),
            phone: Some(String::from("01711000000")),
            active: true,
            bin: Some(String::from("002345678-0202")),
            limit_minor: 30_000,
        };
        let hex = to_hex_public(
            &postcard::to_allocvec(&CustomersResponse {
                protocol: PROTOCOL_VERSION,
                customers: alloc::vec![sent.clone()],
            })
            .expect("encodes"),
        );
        apply(&mut till, &mut driver, Exchange::Customers, &hex, 1).expect("the list applies");

        let held = till
            .customers()
            .first()
            .cloned()
            .expect("the till holds them");
        assert_eq!(held.id, sent.id);
        assert_eq!(held.name, sent.name);
        assert_eq!(held.phone, sent.phone);
        assert_eq!(held.active, sent.active);
        assert_eq!(held.bin, sent.bin, "what a tax invoice names");
        assert_eq!(held.limit_minor, sent.limit_minor, "what they may owe");
    }

    /// And everything the shop says about itself, which is what heads every
    /// receipt and decides what a till does about the shelf.
    #[test]
    fn everything_the_shop_says_about_itself_reaches_the_till() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::ShopResponse;
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

        let hex = to_hex_public(
            &postcard::to_allocvec(&ShopResponse {
                protocol: PROTOCOL_VERSION,
                name: String::from("Karim General Store"),
                bin: Some(String::from("001234567-0101")),
                address: Some(String::from("12 Mirpur Road, Dhaka")),
                phone: Some(String::from("01711000000")),
                wallets: alloc::vec![String::from("bKash"), String::from("Nagad")],
                stock_rule: 2,
            })
            .expect("encodes"),
        );
        apply(&mut till, &mut driver, Exchange::Shop, &hex, 1).expect("the shop applies");

        let shop = till.shop().cloned().expect("the till holds it");
        assert_eq!(shop.name, "Karim General Store");
        assert_eq!(shop.bin.as_deref(), Some("001234567-0101"));
        assert_eq!(shop.address.as_deref(), Some("12 Mirpur Road, Dhaka"));
        assert_eq!(shop.phone.as_deref(), Some("01711000000"));
        assert_eq!(till.wallets().len(), 2, "both of them, in order");
        assert_eq!(
            till.stock_rule(),
            openpos_core::domain::StockRule::Block,
            "what this shop does about the shelf"
        );
    }

    /// A cap the shop sets reaches the till that has to enforce it.
    ///
    /// It did not. The list arrived with the cap on it and this layer dropped
    /// it one line before the till was told, so the back office could set a
    /// limit, the screen could show it, and no till anywhere would stop a sale.
    #[test]
    fn what_the_shop_lets_somebody_owe_reaches_the_till() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{CustomerWire, CustomersResponse};
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

        let response = CustomersResponse {
            protocol: PROTOCOL_VERSION,
            customers: alloc::vec![CustomerWire {
                id: 21,
                name: String::from("Karim, flat 3"),
                phone: None,
                active: true,
                bin: None,
                limit_minor: 30_000,
            }],
        };
        let hex = to_hex_public(&postcard::to_allocvec(&response).expect("encodes"));
        apply(
            &mut till,
            &mut driver,
            Exchange::Customers,
            &hex,
            1_788_700_000_000,
        )
        .expect("the list applies");

        assert_eq!(
            till.customers().first().map(|known| known.limit_minor),
            Some(30_000),
            "the till holds what the shop said they may owe"
        );
    }

    /// What the shop sorts a thing under leaves the screen with the item.
    ///
    /// The same seam that lost the drawer figure: a field added to the wire and
    /// to the form, and this layer's own shape between them with nowhere to put
    /// it. Sorted under nothing is what the shop would then have been told,
    /// silently, on every item anybody edited.
    #[test]
    fn what_a_shop_sorts_a_thing_under_leaves_the_screen_with_it() {
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
            expected_seq: 0,
            item: crate::WireItem {
                id: Ulid::from_u128(9).encode(),
                code: String::from("RICE9"),
                name: String::from("Rice Miniket 5kg"),
                name_bn: String::new(),
                unit: String::from("Nos"),
                price_minor: 0,
                cost_minor: 0,
                vat_bp: 0,
                price_inclusive: false,
                vat_on_undiscounted: false,
                barcodes: alloc::vec![String::from("8690000000001")],
                on_hand_milli: 0,
                active: true,
                supply: 2,
                category: String::from("Rice"),
            },
            price_minor: 43_000,
            cost_minor: 38_000,
            vat_bp: 1_500,
            price_inclusive: false,
            vat_on_undiscounted: false,
        };
        let Step::Post { body, path, .. } = admin_step(&till, 42, &request).expect("a step") else {
            panic!("saving an item is a post");
        };
        assert_eq!(path, "/v1/back-office/catalogue/upsert");
        let sent: UpsertItemRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("decodes");
        assert_eq!(sent.item.category, "Rice", "the shop's own word for it");
        assert_eq!(sent.item.supply, 2, "and what it said about the tax");
        assert!(
            !sent.item.from_a_till,
            "saved from the back office, which is somebody looking at it"
        );
    }

    #[test]
    fn a_payment_taken_in_the_back_office_carries_an_id_the_screen_minted() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::TakePaymentRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (mut till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        // The screen sends the folded name it was given rather than folding a
        // display name again, so both sides mean the same person.
        let request = AdminRequest::TakePayment {
            id: Ulid::from_u128(5_000).encode(),
            person_key: String::from("karim, flat 3"),
            person_name: String::from("Karim, flat 3"),
            amount_minor: 20_000,
            at_ms: 1_788_900_000_000,
            note: Some(String::from("in cash")),
            written_off: false,
        };
        let Step::Post { body, path, .. } = admin_step(&till, 42, &request).expect("a step") else {
            panic!("taking a payment is a post");
        };
        assert_eq!(path, "/v1/back-office/owed/payment");
        let sent: TakePaymentRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("it decodes");
        assert_eq!(sent.id, 5_000, "so a resend is not counted twice");
        assert_eq!(sent.person_key, "karim, flat 3");
        assert_eq!(sent.amount_minor, 20_000);

        // And the reply says what the book now holds, rather than leaving the
        // screen to work it out.
        let response = openpos_core::protocol::TakePaymentResponse {
            protocol: PROTOCOL_VERSION,
            taken: false,
            owed_minor: 9_450,
        };
        let mut driver = Driver::default();
        let applied = apply(
            &mut till,
            &mut driver,
            Exchange::AdminTakePayment,
            &to_hex(&postcard::to_allocvec(&response).expect("it encodes")),
            1,
        )
        .expect("the reply decodes");
        assert!(applied.already_paid, "already recorded, which is ordinary");
        assert_eq!(applied.owed_now, Some(9_450));
    }

    #[test]
    fn a_repair_is_resolved_by_naming_the_sale_and_what_was_decided() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::ResolveRepairRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let terminal = Ulid::from_u128(7);
        let (till, _boot) =
            Till::open(MemoryBackend::new(), 42, terminal, 1, CartLimits::default())
                .expect("a till opens");

        let request = AdminRequest::ResolveRepair {
            sale: Ulid::from_u128(900).encode(),
            note: String::from("counted twice on the paper roll, left as it stands"),
            kept: true,
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
    fn a_bundle_that_travelled_through_a_messaging_app_is_still_taken_in() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::{AdoptSalesRequest, PROTOCOL_VERSION, SaleEnvelope};
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let bundle = AdoptSalesRequest {
            protocol: PROTOCOL_VERSION,
            terminal: 7,
            sales: alloc::vec![SaleEnvelope {
                id: 900,
                schema: 1,
                payload: alloc::vec![1, 2, 3, 4],
            }],
        };
        let text = to_hex(&postcard::to_allocvec(&bundle).expect("it encodes"));

        // As it arrives after a phone wrapped it and somebody pasted it with a
        // trailing newline. Those line breaks are not the shop's doing, and this
        // is the only route a stranded device's money has home.
        let carried = alloc::format!("{}\n{}\n", &text[..text.len() / 2], &text[text.len() / 2..]);
        let request = AdminRequest::AdoptSales {
            bundle: carried.clone(),
        };
        let Step::Post { body, path, .. } = admin_step(&till, 42, &request).expect("a step") else {
            panic!("a back-office request is a post");
        };
        assert_eq!(path, "/v1/back-office/sales/adopt");
        let sent: AdoptSalesRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("it decodes");
        assert_eq!(sent.sales.len(), 1);
        assert_eq!(sent.sales[0].id, 900);

        // And a paste that lost its second half is refused at the keyboard,
        // rather than quietly taking in fewer sales than the device is holding.
        let cut = AdminRequest::AdoptSales {
            bundle: text[..text.len() / 2].to_owned(),
        };
        assert!(admin_step(&till, 42, &cut).is_err());
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

    /// Goods gone, with why, on the way to the shop.
    ///
    /// The route existed since the week it was written and nothing could reach
    /// it: neither this layer nor any screen had a way to say a bottle broke.
    #[test]
    fn goods_gone_travel_with_the_reason_they_went() {
        use openpos_core::cart::CartLimits;
        use openpos_core::protocol::CorrectStockRequest;
        use openpos_core::storage::backend::MemoryBackend;

        let (till, _boot) = Till::open(
            MemoryBackend::new(),
            42,
            Ulid::from_u128(7),
            1,
            CartLimits::default(),
        )
        .expect("a till opens");

        let request = AdminRequest::CorrectStock {
            id: Ulid::from_u128(6_000).encode(),
            item_id: Ulid::from_u128(1).encode(),
            qty_milli: -2_000,
            reason: String::from("two bottles broken carrying them in"),
            occurred_at_ms: 1_788_900_000_000,
        };
        let Step::Post { body, path, .. } = admin_step(&till, 42, &request).expect("a step") else {
            panic!("a correction is a post");
        };
        assert_eq!(path, "/v1/back-office/stock/correct");
        let sent: CorrectStockRequest =
            postcard::from_bytes(&from_hex(&body).expect("hex")).expect("decodes");
        assert_eq!(sent.qty_milli, -2_000, "goods gone, not goods arriving");
        assert_eq!(sent.reason, "two bottles broken carrying them in");
        assert_eq!(
            sent.id, 6_000,
            "the screen's own id, so a retry is one correction"
        );
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
            // A new item: there is nothing it could be stale against.
            expected_seq: 0,
            item: crate::WireItem {
                id: Ulid::from_u128(5).encode(),
                code: String::from("TEA400"),
                name: String::from("Tea 400g"),
                name_bn: String::new(),
                unit: String::from("Nos"),
                price_minor: 0,
                cost_minor: 0,
                vat_bp: 0,
                price_inclusive: false,
                vat_on_undiscounted: false,
                barcodes: alloc::vec![String::from("8690000000005")],
                on_hand_milli: 0,
                active: false,
                supply: 0,
                category: String::new(),
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
                role: 1,
            }],
        };
        let body = to_hex(&postcard::to_allocvec(&response).expect("it encodes"));

        let applied = apply(
            &mut till,
            &mut driver,
            Exchange::AdminTerminals,
            &body,
            2_000,
        )
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
