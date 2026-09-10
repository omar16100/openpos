//! The back office: everything an owner does and a till may not.
//!
//! Split from the routes a till uses because that file had grown past three
//! thousand lines and this half is the half still growing: shop details,
//! people, prices, stock, suppliers, deliveries, takings, the repair queue and
//! the codes that enrol more devices.
//!
//! Every handler here asks for an owner. The check is `owner_from` rather than
//! a check inside each handler, so adding a route is a matter of asking for the
//! right caller rather than remembering to look: a forgotten check is how a till
//! ends up able to reprice the shop.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use openpos_core::protocol::{
    AccountEntryWire, AccountRequest, AccountResponse, AdoptSalesRequest, AllowedEntry,
    AllowedEntryV4, AllowedRequest, AllowedResponse, AllowedResponseV4, AmendOperatorRequest,
    CatalogueEditResponse, ClosedShiftWire, ClosedShiftWireV1, CorrectStockRequest,
    CorrectStockResponse, CustomerWire, CustomersResponse, DayRequest, DayResponse,
    DecideAgainRequest, DecideAgainResponse, DecidedEntry, DecidedRequest, DecidedResponse,
    DeleteItemRequest, DeliveredLineWire, DeliveriesRequest, DeliveriesResponse, DeliveryWire,
    IssueCodeRequest, IssueCodeResponse, ItemNowRequest, ItemNowResponse, ItemWire, MadeRequest,
    MadeResponse, OnHandEntry, OnHandRequest, OnHandResponse, OpenDrawerWire, OpenDrawersRequest,
    OpenDrawersResponse, OperatorWire, OperatorsResponse, OwedRequest, OwedResponse, OwingWire,
    PaperLineWire, PaperTenderWire, PaySupplierRequest, PaySupplierResponse, ProtocolError,
    PutCustomerRequest, PutOperatorRequest, PutShopRequest, PutSupplierRequest, ReceiptGapWire,
    ReceiptGapsRequest, ReceiptGapsResponse, ReceiptRequest, ReceiptResponse, ReceiptResponseV2,
    ReceiveGoodsRequest, ReceiveGoodsResponse, RecordCountRequest, RecordCountResponse,
    RepairEntry, RepairEntryV2, RepairQueueRequest, RepairQueueResponse, RepairQueueResponseV2,
    ResolveRepairRequest, ResolveRepairRequestV1, ResolveRepairResponse, RevokeTerminalRequest,
    RevokeTerminalResponse, SaleOnPaperWire, SaleOnPaperWireV2, SetOperatorPinRequest,
    ShiftsRequest, ShiftsResponse, ShiftsResponseV1, ShopResponse, SoldRequest, SoldResponse,
    SoldWire, SupplierEntryWire, SupplierOwingRequest, SupplierOwingResponse, SupplierOwingWire,
    SupplierStatementRequest, SupplierStatementResponse, SupplierWire, SuppliersRequest,
    SuppliersResponse, TakePaymentRequest, TakePaymentResponse, TerminalHealthEntry,
    TerminalHealthRequest, TerminalHealthResponse, TillItemsRequest, TillItemsResponse,
    ResendCatalogueRequest, ResendCatalogueResponse, TillTakings, UnreadableChangeWire,
    UnreadableChangesRequest, UnreadableChangesResponse,
    UpsertItemRequest, VatRequest, VatResponse, VatRowWire, WaivedRequest, WaivedResponse,
    WaivedWire,
};

use super::{
    AppState, MAX_ACCOUNT_PAGE, MAX_ALLOWED_PAGE, MAX_CODE_LIFETIME, MAX_OWED_PAGE,
    MAX_REPAIR_PAGE, MAX_RESOLUTION_NOTE, authenticate, decode, encoded, owner_from,
    protocol_error, require_owner, unavailable,
};
use crate::auth::{Caller, EnrolmentCode, Role};
use crate::repo::{
    Decided, GoodsReceipt, OperatorRecord, ReceiptLine, RepoError, Repository, ShopDetails,
    StockCorrection, StockCount, Supplier,
};

/// Cut a device off. Owner only.
///
/// The moment a tablet is lost or stolen, every credential it holds stops
/// working. Until now the store could do this and nothing could ask it to,
/// which made "unenrol the device" an answer the shop had no way to carry out.
///
/// The terminal is left in place: its sales are still its sales, and a shop
/// looking into a theft wants to see that a device existed and when it was cut
/// off rather than an absence. If it turns up still holding sales, they are read
/// off it and carried in by hand, which is a route that already exists.
pub(super) async fn revoke_terminal<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<RevokeTerminalRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // Not the device asking. An owner who cuts off the tablet in their own hand
    // has locked themselves out of the shop with one press, and the shop is now
    // a set of tills nobody can issue a code from.
    if request.terminal == caller.terminal {
        return protocol_error(&ProtocolError::NotPermitted);
    }

    match state
        .repo
        .revoke_all_tokens(Caller {
            tenant: caller.tenant,
            terminal: request.terminal,
            role: Role::Till,
        })
        .await
    {
        Ok(withdrawn) => {
            // A till stopping dead is the loudest thing an owner can do from
            // here, and it was the one act that wrote nothing down. The device
            // it stops may be holding sales nobody else has.
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %request.terminal,
                withdrawn,
                "a till's access was withdrawn"
            );
            encoded(&RevokeTerminalResponse {
                protocol,
                withdrawn: u32::try_from(withdrawn).unwrap_or(u32::MAX),
            })
        }
        Err(_) => unavailable(),
    }
}

/// What supervisors waived over a period. Owner only.
///
/// A cashier's ceiling exists so that giving money away is somebody's decision
/// rather than everybody's habit. That only means anything if the decisions can
/// be looked at afterwards: the reason is on the customer's receipt, and this is
/// the shop's side of the same sentence.
pub(super) async fn waived<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<WaivedRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .waived(
            caller.tenant,
            request.from_ms,
            request.to_ms,
            request.limit.clamp(1, 500),
        )
        .await
    {
        Ok(rows) => encoded(&WaivedResponse {
            protocol,
            waived: rows
                .into_iter()
                .map(|row| WaivedWire {
                    sale_id: row.sale_id,
                    terminal: row.terminal,
                    rung_at_ms: row.rung_at_ms,
                    total_minor: row.total_minor,
                    reason: row.reason,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// What sold over a period. Owner only.
///
/// The question a shop asks before it orders. From the movements each sale
/// wrote, so it is what left the shelf rather than what was charged: a line
/// given away at a discount still left the shelf, and the shop still has to
/// replace it.
pub(super) async fn sold<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<SoldRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .sold(
            caller.tenant,
            request.from_ms,
            request.to_ms,
            request.limit.clamp(1, 500),
        )
        .await
    {
        Ok(rows) => encoded(&SoldResponse {
            protocol,
            rows: rows
                .into_iter()
                .map(|row| SoldWire {
                    item_id: row.item_id,
                    qty_milli: row.qty_milli,
                    sales: row.sales,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// What the shop owes its suppliers. Owner only.
///
/// The deliveries less what has been paid for them. Neither side is stored as a
/// balance: a shop argues about the deliveries, not about a number somebody
/// wrote down, and a stored balance that disagrees with them is a question
/// nobody can answer.
pub(super) async fn supplier_owing<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<SupplierOwingRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.supplier_owing(caller.tenant).await {
        Ok(found) => encoded(&SupplierOwingResponse {
            protocol,
            owing: found
                .into_iter()
                .map(|owing| SupplierOwingWire {
                    supplier_id: owing.supplier_id,
                    name: owing.name,
                    owed_minor: owing.owed_minor,
                    deliveries: owing.deliveries,
                    since_ms: owing.since_ms,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Catalogue changes that never reached the tills. Owner only.
///
/// A change this build cannot read is passed over on the way to a till and the
/// cursor still moves, because failing the page would stop every till in the
/// shop syncing for ever over one bad row. That trade is only defensible if
/// somebody can be told, and until now nobody could be: the count was written
/// into a field the pull handler ignored.
///
/// A shop with none of these gets an empty list, which is the ordinary answer.
/// Items a till wrote down at a counter that nobody has agreed to. Owner only.
///
/// A price typed to get a queue moving is not a price the shop set, and a name
/// typed the same way is not the name the shop calls it. They sell, they are in
/// every report, and this is the list somebody works through: correct it, or
/// press the button that says it is right.
///
/// Read out of the catalogue as it stands rather than from a list of its own,
/// because the answer is "which items are marked this way now", and a second
/// list would be a second answer to keep in step.
pub(super) async fn items_from_tills<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<TillItemsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.items_since(caller.tenant, 0, u32::MAX).await {
        Ok(page) => {
            // The last change to an item is what the shop holds, so a later
            // upsert that cleared the flag wins over the one that set it.
            let mut latest: Vec<ItemWire> = Vec::new();
            for item in page.upserts {
                match latest.iter_mut().find(|held| held.id == item.id) {
                    Some(held) => *held = item,
                    None => latest.push(item),
                }
            }
            latest.retain(|item| item.from_a_till && !page.tombstones.contains(&item.id));
            latest.truncate(request.limit.clamp(1, 500) as usize);
            encoded(&TillItemsResponse {
                protocol,
                items: latest,
            })
        }
        Err(_) => unavailable(),
    }
}

pub(super) async fn unreadable_changes<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<UnreadableChangesRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .unreadable_changes(caller.tenant, request.limit.clamp(1, 1_000))
        .await
    {
        Ok(changes) => encoded(&UnreadableChangesResponse {
            protocol,
            changes: changes
                .into_iter()
                .map(|change| UnreadableChangeWire {
                    seq: change.seq,
                    item_id: change.item_id,
                    schema: change.schema,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Say every item to the tills again.
///
/// The other half of the screen that lists changes no till could read: knowing
/// which ones were lost is no use without a way to send them. Every item's
/// current state goes back into the log under a new sequence, so a till that
/// passed over a row when it could not be read receives it on the next pull.
pub(super) async fn resend_catalogue<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ResendCatalogueRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.resend_catalogue(caller.tenant).await {
        Ok(sent) => encoded(&ResendCatalogueResponse { protocol, sent }),
        Err(_) => unavailable(),
    }
}

/// What passed between the shop and one supplier over a period. Owner only.
///
/// The statement two people put side by side when the shop's figure and the
/// distributor's disagree, which is the conversation the whole ledger exists
/// for. Deliveries in and payments out, oldest first.
pub(super) async fn supplier_statement<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<SupplierStatementRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    let entries = match state
        .repo
        .supplier_statement(
            caller.tenant,
            request.supplier_id,
            request.from_ms,
            request.to_ms,
        )
        .await
    {
        Ok(entries) => entries,
        Err(_) => return unavailable(),
    };

    // The balance is the whole account rather than the period, because that is
    // the number the two people are arguing about. A period that opens owing
    // and closes owing says so either way.
    let owed_minor = match state.repo.supplier_owing(caller.tenant).await {
        Ok(owing) => owing
            .into_iter()
            .find(|one| one.supplier_id == request.supplier_id)
            .map_or(0, |one| one.owed_minor),
        Err(_) => return unavailable(),
    };

    encoded(&SupplierStatementResponse {
        protocol,
        entries: entries
            .into_iter()
            .map(|entry| SupplierEntryWire {
                at_ms: entry.at_ms,
                delivered: entry.delivered,
                amount_minor: entry.amount_minor,
                reference: entry.reference,
            })
            .collect(),
        owed_minor,
    })
}

/// Record money paid to a supplier. Owner only.
pub(super) async fn pay_supplier<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PaySupplierRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // A payment of nothing, or to nobody, is a mistake at the keyboard rather
    // than an act. What the shop owes goes up when goods arrive, which is a
    // delivery, and there is a route for that.
    if request.amount_minor <= 0 || request.supplier_id == 0 {
        return protocol_error(&ProtocolError::Malformed);
    }

    let payment = crate::repo::SupplierPayment {
        id: request.id,
        supplier_id: request.supplier_id,
        amount_minor: request.amount_minor,
        paid_at_ms: request.paid_at_ms,
        note: request.note,
    };
    let paid = match state.repo.pay_supplier(caller.tenant, &payment).await {
        Ok(paid) => paid,
        Err(_) => return unavailable(),
    };

    match state.repo.supplier_owing(caller.tenant).await {
        Ok(found) => encoded(&PaySupplierResponse {
            protocol,
            paid,
            owed_minor: found
                .into_iter()
                .find(|owing| owing.supplier_id == request.supplier_id)
                .map_or(0, |owing| owing.owed_minor),
        }),
        Err(_) => unavailable(),
    }
}

/// What was sold at each tax rate over a period. Owner only.
///
/// A shop here files a monthly return, and this is what it has to put on it.
/// Answered from what the server recomputed when each sale arrived rather than
/// by decoding a month of tickets, and by when the goods were sold rather than
/// by when the server heard about them: a till that syncs on Tuesday sold on
/// Monday, and Monday is the day that belongs in the return.
pub(super) async fn vat<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<VatRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .vat_summary(caller.tenant, request.from_ms, request.to_ms)
        .await
    {
        Ok(summary) => encoded(&VatResponse {
            protocol,
            rows: summary
                .rows
                .into_iter()
                .map(|row| VatRowWire {
                    vat_bp: row.vat_bp,
                    net_minor: row.net_minor,
                    vat_minor: row.vat_minor,
                    sales: row.sales,
                    supply: row.supply,
                })
                .collect(),
            waiting_sales: summary.waiting_sales,
            waiting_vat_minor: summary.waiting_vat_minor,
        }),
        Err(_) => unavailable(),
    }
}

/// What a day looked like. Owner only.
///
/// One question an owner asks once at closing, answered in one call: what was
/// sold, what came back, what the drawers held against what they should have,
/// and what went on account rather than into the till. Composed from the sale
/// headers and the two ledgers, so a shop with a busy day is not asking the
/// database to decode a thousand tickets.
pub(super) async fn day<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<DayRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // A backwards range is a mistake, not a query. Answering it with zero would
    // read as a day with no sales, which is a thing an owner would act on.
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    let rows = match state
        .repo
        .takings(caller.tenant, request.from_ms, request.to_ms)
        .await
    {
        Ok(rows) => rows,
        Err(_) => return unavailable(),
    };
    let summary = match state
        .repo
        .day_summary(caller.tenant, request.from_ms, request.to_ms)
        .await
    {
        Ok(summary) => summary,
        Err(_) => return unavailable(),
    };

    let mut sales = 0_u64;
    let mut total_minor = 0_i64;
    let mut refunds = 0_u64;
    let mut refunded_minor = 0_i64;
    let mut tills = Vec::with_capacity(rows.len());
    for row in rows {
        sales = sales.saturating_add(row.sales);
        total_minor = total_minor.saturating_add(row.total_minor);
        refunds = refunds.saturating_add(row.refunds);
        refunded_minor = refunded_minor.saturating_add(row.refunded_minor);
        tills.push(TillTakings {
            terminal: row.terminal,
            sales: row.sales,
            total_minor: row.total_minor,
            needing_attention: row.needing_attention,
        });
    }

    encoded(&DayResponse {
        protocol,
        sales,
        total_minor,
        refunds,
        refunded_minor,
        drawers_counted: summary.drawers_counted,
        expected_cash_minor: summary.expected_cash_minor,
        counted_cash_minor: summary.counted_cash_minor,
        variance_minor: summary.variance_minor,
        charged_minor: summary.charged_minor,
        returned_minor: summary.returned_minor,
        paid_minor: summary.paid_minor,
        written_off_minor: summary.written_off_minor,
        tills,
    })
}

/// Add or correct somebody who buys on account. Owner only.
///
/// The whole list back, so a screen shows what is true rather than what it
/// assumed would be true.
pub(super) async fn put_customer<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PutCustomerRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // Nobody may hold the nil id, and nobody may be nameless: the name is what
    // a cashier picks from and what is shown against what they owe.
    if request.customer.id == 0 || request.customer.name.trim().is_empty() {
        return protocol_error(&ProtocolError::Malformed);
    }

    let record = crate::repo::CustomerRecord {
        id: request.customer.id,
        name: request.customer.name.trim().to_owned(),
        phone: request
            .customer
            .phone
            .map(|phone| phone.trim().to_owned())
            .filter(|phone| !phone.is_empty()),
        active: request.customer.active,
        bin: request
            .customer
            .bin
            .map(|bin| bin.trim().to_owned())
            .filter(|bin| !bin.is_empty()),
        // A negative cap is a shop saying somebody may owe less than nothing,
        // which is not a thing. Read as no cap rather than refused: the screen
        // that sent it has a typo, not a customer who cannot be saved.
        limit_minor: request.customer.limit_minor.max(0),
    };
    if state
        .repo
        .put_customer(caller.tenant, &record)
        .await
        .is_err()
    {
        return unavailable();
    }

    match state.repo.customers(caller.tenant).await {
        Ok(found) => encoded(&CustomersResponse {
            protocol,
            customers: found
                .into_iter()
                .map(|customer| CustomerWire {
                    id: customer.id,
                    name: customer.name,
                    phone: customer.phone,
                    active: customer.active,
                    bin: customer.bin,
                    limit_minor: customer.limit_minor,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Which tills have a drawer open. Owner only.
///
/// The question an owner asks at closing time and could not ask before: a
/// drawer was only ever reported when it closed, so one left open overnight was
/// invisible until somebody noticed the till in the morning.
pub(super) async fn open_drawers<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<OpenDrawersRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.open_drawers(caller.tenant).await {
        Ok(found) => encoded(&OpenDrawersResponse {
            protocol,
            drawers: found
                .into_iter()
                .map(|drawer| OpenDrawerWire {
                    terminal: drawer.terminal,
                    shift: drawer.shift,
                    opened_at_ms: drawer.opened_at_ms,
                    reported_at_ms: drawer.reported_at_ms,
                    opening_float_minor: drawer.opening_float_minor,
                    sales: drawer.sales,
                    cash_sales_minor: drawer.cash_sales_minor,
                    non_cash_sales_minor: drawer.non_cash_sales_minor,
                    cash_in_minor: drawer.cash_in_minor,
                    cash_out_minor: drawer.cash_out_minor,
                    expected_cash_minor: drawer.expected_cash_minor,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// The drawers this shop has closed lately. Owner only.
///
/// This is the reconciliation the counting is for: an owner reading what each
/// till was expected to hold, what was in it, and the difference.
pub(super) async fn shifts<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ShiftsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let found = match state
        .repo
        .closed_shifts(caller.tenant, request.limit.clamp(1, 100))
        .await
    {
        Ok(found) => found,
        Err(_) => return unavailable(),
    };

    // A back office a release behind reads the counts without the names, which
    // is what it could show anyway.
    if protocol == 1 {
        return encoded(&ShiftsResponseV1 {
            protocol,
            shifts: found
                .into_iter()
                .map(|shift| {
                    ClosedShiftWireV1::from(ClosedShiftWire {
                        id: shift.id,
                        terminal: shift.terminal,
                        closed_by: shift.closed_by,
                        closed_by_name: shift.closed_by_name,
                        opened_at_ms: shift.opened_at_ms,
                        closed_at_ms: shift.closed_at_ms,
                        opening_float_minor: shift.opening_float_minor,
                        sales: shift.sales,
                        cash_sales_minor: shift.cash_sales_minor,
                        non_cash_sales_minor: shift.non_cash_sales_minor,
                        cash_in_minor: shift.cash_in_minor,
                        cash_out_minor: shift.cash_out_minor,
                        expected_cash_minor: shift.expected_cash_minor,
                        // Dropped by the conversion below; a back office a
                        // release behind has nowhere to put either of these.
                        expected_from_sales_minor: None,
                        struck_out_cash_minor: None,
                        counted_cash_minor: shift.counted_cash_minor,
                        variance_minor: shift.variance_minor,
                    })
                })
                .collect(),
        });
    }

    {
        // What the shop's own sales say each of these drawers should have held,
        // worked out here rather than taken from the till's report of itself.
        // One question per shift: a hundred at most, and only the owner asks.
        let mut from_sales = Vec::with_capacity(found.len());
        let mut struck_out = Vec::with_capacity(found.len());
        for shift in &found {
            let taken: Option<i64> = match state
                .repo
                .drawer_takings(
                    caller.tenant,
                    shift.terminal,
                    shift.opened_at_ms,
                    shift.closed_at_ms,
                )
                .await
            {
                Ok(taken) => taken,
                Err(_) => return unavailable(),
            };
            // The same sum the till does: what it started with, plus what it
            // took, plus and less what was put in and taken out by hand. A
            // drawer the shop cannot answer for stays unanswered rather than
            // being answered with the float.
            let expected = taken.map(|taken| {
                shift
                    .opening_float_minor
                    .saturating_add(taken)
                    .saturating_add(shift.cash_in_minor)
                    .saturating_sub(shift.cash_out_minor)
            });
            if expected.is_some_and(|expected| expected != shift.expected_cash_minor) {
                tracing::info!(
                    tenant = %caller.tenant,
                    terminal = %shift.terminal,
                    till_said = shift.expected_cash_minor,
                    sales_say = expected.unwrap_or_default(),
                    "a till's expected drawer disagrees with the shop's own sales"
                );
            }
            from_sales.push(expected);
            // What of that window's cash belongs to sales somebody has since
            // struck out, which is the commonest honest reason the two figures
            // above differ once a till has finished sending.
            match state
                .repo
                .struck_out_takings(
                    caller.tenant,
                    shift.terminal,
                    shift.opened_at_ms,
                    shift.closed_at_ms,
                )
                .await
            {
                Ok(taken) => struck_out.push(taken.filter(|cash| *cash != 0)),
                Err(_) => return unavailable(),
            }
        }

        // A back office speaking anything up to 5 reads a drawer without the
        // struck-out cash in its window, which is what it could show anyway.
        // Sending the newer shape would not be read as a missing field; it
        // would be read as different numbers. The shop's own figure for the
        // drawer still travels: that one it has had since version 5.
        if protocol <= 5 {
            return encoded(&openpos_core::protocol::ShiftsResponseV5 {
                protocol,
                shifts: found
                    .into_iter()
                    .zip(from_sales)
                    .map(|(shift, expected_from_sales_minor)| {
                        openpos_core::protocol::ClosedShiftWireV5 {
                            id: shift.id,
                            terminal: shift.terminal,
                            closed_by: shift.closed_by,
                            closed_by_name: shift.closed_by_name,
                            opened_at_ms: shift.opened_at_ms,
                            closed_at_ms: shift.closed_at_ms,
                            opening_float_minor: shift.opening_float_minor,
                            sales: shift.sales,
                            cash_sales_minor: shift.cash_sales_minor,
                            non_cash_sales_minor: shift.non_cash_sales_minor,
                            cash_in_minor: shift.cash_in_minor,
                            cash_out_minor: shift.cash_out_minor,
                            expected_cash_minor: shift.expected_cash_minor,
                            expected_from_sales_minor,
                            counted_cash_minor: shift.counted_cash_minor,
                            variance_minor: shift.variance_minor,
                        }
                    })
                    .collect(),
            });
        }

        encoded(&ShiftsResponse {
            protocol,
            shifts: found
                .into_iter()
                .zip(from_sales)
                .zip(struck_out)
                .map(|((shift, expected_from_sales_minor), struck_out_cash_minor)| ClosedShiftWire {
                    id: shift.id,
                    terminal: shift.terminal,
                    closed_by: shift.closed_by,
                    closed_by_name: shift.closed_by_name,
                    opened_at_ms: shift.opened_at_ms,
                    closed_at_ms: shift.closed_at_ms,
                    opening_float_minor: shift.opening_float_minor,
                    sales: shift.sales,
                    cash_sales_minor: shift.cash_sales_minor,
                    non_cash_sales_minor: shift.non_cash_sales_minor,
                    cash_in_minor: shift.cash_in_minor,
                    cash_out_minor: shift.cash_out_minor,
                    expected_cash_minor: shift.expected_cash_minor,
                    expected_from_sales_minor,
                    struck_out_cash_minor,
                    counted_cash_minor: shift.counted_cash_minor,
                    variance_minor: shift.variance_minor,
                })
                .collect(),
        })
    }
}

/// Who owes the shop money. Owner only.
///
/// A shop here sells on account all day, and until this existed the till could
/// record that money had been given away and nothing added it up. The book
/// stayed on paper beside the machine that was meant to replace it.
pub(super) async fn owed<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<OwedRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .owed(
            caller.tenant,
            // An empty key is the start of the list, whatever number came with
            // it: a screen opening the page sends neither.
            (!request.after_person_key.is_empty())
                .then(|| (request.after_owed_minor, request.after_person_key.clone())),
            request.limit.clamp(1, MAX_OWED_PAGE),
        )
        .await
    {
        Ok(found) => encoded(&OwedResponse {
            protocol,
            owing: found
                .into_iter()
                .map(|owing| OwingWire {
                    person_key: owing.person_key,
                    person_name: owing.person_name,
                    owed_minor: owing.owed_minor,
                    since_ms: owing.since_ms,
                    last_at_ms: owing.last_at_ms,
                    entries: owing.entries,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Take money off what somebody owes. Owner only.
pub(super) async fn take_payment<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<TakePaymentRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // A payment of nothing, or of a negative amount, is a mistake at the keyboard
    // rather than an act. Money owed goes up when goods leave the shop, which is
    // a sale, and there is a route for that.
    if request.amount_minor <= 0 || request.person_key.trim().is_empty() {
        return protocol_error(&ProtocolError::Malformed);
    }
    // A debt struck off without money needs a reason. It is the one entry here
    // that makes money disappear, and one that vanishes without a note leaves
    // the question this book exists to answer unanswerable.
    let reason_given = request
        .note
        .as_deref()
        .is_some_and(|note| !note.trim().is_empty());
    if request.written_off && !reason_given {
        return protocol_error(&ProtocolError::Malformed);
    }

    let payment = crate::repo::AccountPayment {
        id: request.id,
        kind: if request.written_off {
            crate::repo::Settlement::WrittenOff
        } else {
            crate::repo::Settlement::Paid
        },
        person_key: request.person_key.clone(),
        person_name: request.person_name,
        amount_minor: request.amount_minor,
        at_ms: request.at_ms,
        note: request.note,
    };
    let taken = match state.repo.take_payment(caller.tenant, &payment).await {
        Ok(taken) => taken,
        Err(_) => return unavailable(),
    };

    // Read back rather than worked out here, so the screen shows what the book
    // says even where two people are settling accounts at once. Asked for this
    // one person: paging the whole list would report anybody off the end of it
    // as owing nothing.
    match state.repo.balance(caller.tenant, &request.person_key).await {
        Ok(owed_minor) => encoded(&TakePaymentResponse {
            protocol,
            taken,
            owed_minor,
        }),
        Err(_) => unavailable(),
    }
}

/// What makes up one person's balance. Owner only.
///
/// The question asked when somebody disputes the total, and the reason the
/// balance is summed from entries rather than stored.
pub(super) async fn account<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<AccountRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .account(
            caller.tenant,
            &request.person_key,
            // Zero is the newest entry, which is where a screen opening the
            // list starts.
            (request.after_at_ms != 0).then_some((request.after_at_ms, request.after_source_id)),
            request.limit.clamp(1, MAX_ACCOUNT_PAGE),
        )
        .await
    {
        Ok(found) => encoded(&AccountResponse {
            protocol,
            entries: found
                .into_iter()
                .map(|entry| AccountEntryWire {
                    source_id: entry.source_id,
                    is_sale: entry.is_sale,
                    written_off: entry.written_off,
                    amount_minor: entry.amount_minor,
                    at_ms: entry.at_ms,
                    note: entry.note,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Sales carried in from a device that could not send them. Owner only.
///
/// The only way out for a till whose terminal the shop deleted, or one holding
/// sales that has to be re-enrolled as another. Its outbox is the only record of
/// goods that left the shop, and until this existed there was no route that
/// would take them: the credential that proves where a sale came from is exactly
/// what such a device has lost.
pub(super) async fn adopt_sales<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<AdoptSalesRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let carried = request.sales.len();
    match crate::ingest::adopt(state.repo.as_ref(), caller.tenant, &request).await {
        Ok(response) => {
            // Sales carried in by hand off a device that could not send them.
            // Every one of them is waiting on a person by definition, so the
            // line that says they arrived is the start of that job.
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %request.terminal,
                carried,
                adopted = response.adopted.len(),
                needing_attention = response.needing_attention.len(),
                "sales were carried in by hand from a device"
            );
            for held in &response.needing_attention {
                tracing::warn!(
                    tenant = %caller.tenant,
                    sale = %held.id,
                    reason = ?held.reason,
                    "a carried-in sale is waiting for somebody to decide"
                );
            }
            encoded(&response)
        }
        Err(crate::ingest::IngestError::Protocol(error)) => protocol_error(&error),
        // The device keeps its copy and the shop tries again. Telling it
        // otherwise would let somebody wipe the only record of a day's trading.
        Err(crate::ingest::IngestError::Storage) => {
            tracing::error!(
                tenant = %caller.tenant,
                terminal = %request.terminal,
                carried,
                "sales carried in by hand could not be stored; the device keeps them"
            );
            unavailable()
        }
    }
}

/// What has been delivered lately. Owner only.
///
/// A delivery filed under a supplier is only worth filing if somebody can ask
/// which goods came on which challan, and that is the question asked when the
/// invoice and the shelf disagree.
pub(super) async fn deliveries<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<DeliveriesRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // Capped here rather than trusted from the caller: a shop asking for
    // everything since it opened would get a page nobody can read and a query
    // nobody wants to run.
    let limit = request.limit.clamp(1, 100);
    match state.repo.deliveries(caller.tenant, limit).await {
        Ok(found) => encoded(&DeliveriesResponse {
            protocol,
            deliveries: found
                .into_iter()
                .map(|receipt| DeliveryWire {
                    id: receipt.id,
                    supplier_id: receipt.supplier_id,
                    reference: receipt.reference,
                    received_at_ms: receipt.received_at_ms,
                    lines: receipt
                        .lines
                        .into_iter()
                        .map(|line| DeliveredLineWire {
                            item_id: line.item_id,
                            qty_milli: line.qty_milli,
                            unit_cost_minor: line.unit_cost_minor,
                        })
                        .collect(),
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// What the shop believes it holds. Owner only.
///
/// Its own question rather than a field on the catalogue, because a sale is not
/// a catalogue change and must not bump the catalogue cursor: doing that would
/// make every till re-pull every item every time anything sold.
pub(super) async fn on_hand<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<OnHandRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // A cap, because this is one query per item and an owner with a long
    // catalogue should get a slow screen rather than a server on its knees.
    const MOST: usize = 200;
    // Whether the answer covers every item the shop sells. A caller asking
    // about the page on its screen knows the answer is partial; one asking
    // about the whole shelf, to add up what is sitting on it, cannot tell from
    // a list of two hundred figures that there were eight hundred items.
    let mut whole = false;
    let wanted: Vec<u128> = if request.item_ids.is_empty() {
        match state.repo.items_since(caller.tenant, 0, u32::MAX).await {
            Ok(page) => {
                whole = page.upserts.len() <= MOST;
                page.upserts
                    .into_iter()
                    .map(|item| item.id)
                    .take(MOST)
                    .collect()
            }
            Err(_) => return unavailable(),
        }
    } else {
        request.item_ids.into_iter().take(MOST).collect()
    };

    // Asked once for the lot. One at a time was a transaction and three
    // statements per item, and this page is two hundred items.
    let Ok(found) = state.repo.on_hand_many(caller.tenant, &wanted).await else {
        return unavailable();
    };
    let figures: Vec<OnHandEntry> = found
        .into_iter()
        .map(|entry| OnHandEntry {
            item_id: entry.item_id,
            qty_milli: entry.qty_milli,
            counted_at_ms: entry.counted_at_ms,
            unreconciled_milli: entry.unreconciled_milli,
            // Saturating rather than wrapping: a shop with four billion late
            // sales on one item has a bigger problem than a count, and wrapping
            // would report it as none.
            unreconciled_sales: u32::try_from(entry.unreconciled_sales).unwrap_or(u32::MAX),
        })
        .collect();

    encoded(&OnHandResponse {
        protocol,
        on_hand: figures,
        whole,
    })
}

/// The wallets a shop takes, as a report should read them.
///
/// Blank entries dropped, spaces trimmed, and one name kept once: a shop that
/// enters "bKash" and "bkash " has two lines in every report and no way to say
/// which sale went where.
fn tidy_wallets(named: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::with_capacity(named.len());
    for one in named {
        let one = one.trim();
        if one.is_empty() || kept.iter().any(|seen| seen.eq_ignore_ascii_case(one)) {
            continue;
        }
        kept.push(one.to_owned());
    }
    kept
}

/// Give somebody a new PIN. Owner only.
///
/// Separate from amending them, and carrying a credential and nothing else. The
/// key arrives already derived, by the same code the till will check it with, so
/// the PIN itself never reaches this process and cannot be logged by it.
pub(super) async fn set_operator_pin<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<SetOperatorPinRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .set_operator_pin(
            caller.tenant,
            request.operator_id,
            &request.pin_salt,
            request.pin_rounds,
            &request.pin_key,
        )
        .await
    {
        // The list back, without the new credential meaning anything to a
        // reader: what comes back is what every other write here answers with.
        Ok(()) => match state.repo.operators(caller.tenant).await {
            Ok(people) => encoded(&OperatorsResponse {
                protocol,
                operators: people.into_iter().map(wire_operator).collect(),
            }),
            Err(_) => unavailable(),
        },
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(_) => unavailable(),
    }
}

/// Change a person, except their PIN. Owner only.
///
/// Its own route rather than a flag on the upsert, because that one carries the
/// whole person including the derived PIN key, and an owner does not have it: a
/// PIN is hashed on the device where it is set and never travels. Requiring it
/// here would mean knowing a cashier's PIN in order to correct their name or
/// take the drawer away from them.
pub(super) async fn amend_operator<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<AmendOperatorRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let amended = crate::repo::AmendedOperator {
        id: request.operator_id,
        name: request.name,
        max_discount_bp: request.max_discount_bp,
        may_override_price: request.may_override_price,
        may_refund: request.may_refund,
        may_void_line: request.may_void_line,
        may_authorise: request.may_authorise,
        may_open_drawer: request.may_open_drawer,
        may_close_shift: request.may_close_shift,
        active: request.active,
    };

    match state.repo.amend_operator(caller.tenant, &amended).await {
        Ok(()) => match state.repo.operators(caller.tenant).await {
            // The whole list back, so a screen shows what is true rather than
            // what it assumed would be true.
            Ok(people) => encoded(&OperatorsResponse {
                protocol,
                operators: people.into_iter().map(wire_operator).collect(),
            }),
            Err(_) => unavailable(),
        },
        // Nobody by that id. Told apart from a store that is merely down,
        // because retrying will not find them.
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(_) => unavailable(),
    }
}

/// Add or update a person. Owner only.
pub(super) async fn put_operator<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PutOperatorRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // Nobody may have the nil id. It is what a record carries when it means
    // "not this time": a counted drawer with nobody against it says zero, and a
    // person who really was zero would read back as nobody having counted.
    if request.operator.id == 0 {
        return protocol_error(&ProtocolError::Malformed);
    }

    let record = OperatorRecord {
        id: request.operator.id,
        name: request.operator.name,
        pin_salt: request.operator.pin_salt,
        pin_rounds: request.operator.pin_rounds,
        pin_key: request.operator.pin_key,
        max_discount_bp: request.operator.max_discount_bp,
        may_override_price: request.operator.may_override_price,
        may_refund: request.operator.may_refund,
        may_void_line: request.operator.may_void_line,
        may_authorise: request.operator.may_authorise,
        may_open_drawer: request.operator.may_open_drawer,
        may_close_shift: request.operator.may_close_shift,
        active: request.operator.active,
    };

    match state.repo.put_operator(caller.tenant, &record).await {
        // The whole list, as amending one answers. One person back meant the
        // device that added somebody could not show them until its next
        // settings refresh, which is ten minutes: an owner adds a cashier, sees
        // nothing, and reasonably concludes it did not work.
        Ok(()) => match state.repo.operators(caller.tenant).await {
            Ok(people) => encoded(&OperatorsResponse {
                protocol,
                operators: people.into_iter().map(wire_operator).collect(),
            }),
            Err(_) => unavailable(),
        },
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(_) => unavailable(),
    }
}

pub(super) fn wire_operator(record: OperatorRecord) -> OperatorWire {
    OperatorWire {
        id: record.id,
        name: record.name,
        pin_salt: record.pin_salt,
        pin_rounds: record.pin_rounds,
        pin_key: record.pin_key,
        max_discount_bp: record.max_discount_bp,
        may_override_price: record.may_override_price,
        may_refund: record.may_refund,
        may_void_line: record.may_void_line,
        may_authorise: record.may_authorise,
        may_open_drawer: record.may_open_drawer,
        may_close_shift: record.may_close_shift,
        active: record.active,
    }
}

/// Set them. Owner only: this is what every receipt the shop issues will say.
pub(super) async fn put_shop<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PutShopRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let details = ShopDetails {
        name: request.name,
        bin: request.bin,
        address: request.address,
        phone: request.phone,
        // Trimmed and de-duplicated here rather than trusted: two spellings of
        // one wallet are two lines in every report, and the shop cannot tell
        // which sale went where.
        wallets: tidy_wallets(request.wallets),
        // Anything this build does not know is nothing, which is the answer
        // that keeps a till selling.
        stock_rule: request.stock_rule.min(2),
    };
    match state.repo.put_shop_details(caller.tenant, &details).await {
        Ok(()) => encoded(&ShopResponse {
            protocol,
            name: details.name,
            bin: details.bin,
            address: details.address,
            phone: details.phone,
            wallets: details.wallets,
            stock_rule: details.stock_rule,
        }),
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(_) => unavailable(),
    }
}

/// Add or update a supplier.
pub(super) async fn put_supplier<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PutSupplierRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let supplier = Supplier {
        id: request.supplier.id,
        name: request.supplier.name,
        phone: request.supplier.phone,
        bin: request.supplier.bin,
        active: request.supplier.active,
    };
    match state.repo.put_supplier(caller.tenant, &supplier).await {
        Ok(()) => encoded(&SuppliersResponse {
            protocol,
            suppliers: alloc_suppliers(&[supplier]),
        }),
        Err(_) => unavailable(),
    }
}

/// Who the shop buys from.
pub(super) async fn suppliers<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<SuppliersRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.suppliers(caller.tenant).await {
        Ok(found) => encoded(&SuppliersResponse {
            protocol,
            suppliers: alloc_suppliers(&found),
        }),
        Err(_) => unavailable(),
    }
}

fn alloc_suppliers(found: &[Supplier]) -> Vec<SupplierWire> {
    found
        .iter()
        .map(|supplier| SupplierWire {
            id: supplier.id,
            name: supplier.name.clone(),
            phone: supplier.phone.clone(),
            bin: supplier.bin.clone(),
            active: supplier.active,
        })
        .collect()
}

/// Book a delivery, which is the only way stock goes up other than a count.
pub(super) async fn receive_goods<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ReceiveGoodsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let receipt = GoodsReceipt {
        id: request.id,
        supplier_id: request.supplier_id,
        reference: request.reference,
        received_at_ms: request.received_at_ms,
        received_by: caller.terminal,
        note: request.note,
        lines: request
            .lines
            .iter()
            .map(|line| ReceiptLine {
                item_id: line.item_id,
                qty_milli: line.qty_milli,
                unit_cost_minor: line.unit_cost_minor,
            })
            .collect(),
    };

    let recorded = match state.repo.receive_goods(caller.tenant, &receipt).await {
        Ok(recorded) => recorded,
        Err(_) => return unavailable(),
    };

    // A delivery is where a shop learns what it pays, so the catalogue learns
    // it here rather than waiting for somebody to type the same figure into
    // two screens. Nobody does that second step, which is why every margin
    // this shop could read said it did not know.
    //
    // The last delivery's price, plainly: it is what a shopkeeper means by what
    // a thing costs, and an average nobody can reproduce from their own papers
    // is a figure they will not trust. Only when the delivery says: a line
    // booked with no price is somebody recording goods, not a price change.
    //
    // Only on the call that actually booked the delivery. A retry after a
    // dropped reply must not walk the catalogue again.
    if recorded {
        for line in &receipt.lines {
            if line.unit_cost_minor <= 0 {
                continue;
            }
            let Ok(Some((held, _))) = state.repo.item_now(caller.tenant, line.item_id).await else {
                continue;
            };
            if held.cost_minor == line.unit_cost_minor {
                continue;
            }
            let mut priced = held;
            let was = priced.cost_minor;
            priced.cost_minor = line.unit_cost_minor;
            match state.repo.upsert_item(caller.tenant, &priced).await {
                Ok(cursor) => tracing::info!(
                    tenant = %caller.tenant,
                    item = %priced.id,
                    was,
                    now = priced.cost_minor,
                    cursor,
                    "a delivery said what this costs now"
                ),
                // Not a failure of the delivery: the goods are booked and the
                // stock is right. What the shop pays is a day out of date, and
                // saying so beats refusing a delivery that already happened.
                Err(_) => tracing::warn!(
                    tenant = %caller.tenant,
                    item = %priced.id,
                    "the delivery was booked and the catalogue kept the older cost"
                ),
            }
        }
    }

    // The figures are read back whether or not this call wrote anything. A
    // retry that is told "already booked" still needs to know where stock
    // stands, or the only way to find out is to guess.
    let lines: Vec<u128> = receipt.lines.iter().map(|line| line.item_id).collect();
    let Ok(found) = state.repo.on_hand_many(caller.tenant, &lines).await else {
        return unavailable();
    };
    let on_hand: Vec<OnHandEntry> = found
        .into_iter()
        .map(|figure| OnHandEntry {
            item_id: figure.item_id,
            qty_milli: figure.qty_milli,
            counted_at_ms: figure.counted_at_ms,
            unreconciled_milli: figure.unreconciled_milli,
            unreconciled_sales: u32::try_from(figure.unreconciled_sales).unwrap_or(u32::MAX),
        })
        .collect();

    tracing::info!(
        tenant = %caller.tenant,
        receipt = %receipt.id,
        lines = receipt.lines.len(),
        recorded,
        "goods receipt"
    );
    encoded(&ReceiveGoodsResponse {
        protocol,
        recorded,
        on_hand,
    })
}

/// Write off breakage, spoilage, theft, or a count that was wrong.
pub(super) async fn correct_stock<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<CorrectStockRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let correction = StockCorrection {
        id: request.id,
        item_id: request.item_id,
        qty_milli: request.qty_milli,
        reason: request.reason,
        occurred_at_ms: request.occurred_at_ms,
        recorded_by: caller.terminal,
    };

    let recorded = match state.repo.correct_stock(caller.tenant, &correction).await {
        Ok(recorded) => recorded,
        // A blank reason fails identically forever, so it is refused rather
        // than reported as a store that might work later.
        Err(RepoError::Invalid) => return protocol_error(&ProtocolError::Malformed),
        Err(_) => return unavailable(),
    };

    let on_hand = match state.repo.on_hand(caller.tenant, request.item_id).await {
        Ok(figure) => Some(OnHandEntry {
            item_id: figure.item_id,
            qty_milli: figure.qty_milli,
            counted_at_ms: figure.counted_at_ms,
            unreconciled_milli: figure.unreconciled_milli,
            unreconciled_sales: u32::try_from(figure.unreconciled_sales).unwrap_or(u32::MAX),
        }),
        Err(_) => return unavailable(),
    };

    tracing::info!(
        tenant = %caller.tenant,
        item = %request.item_id,
        qty_milli = request.qty_milli,
        recorded,
        "stock corrected"
    );
    encoded(&CorrectStockResponse {
        protocol,
        recorded,
        on_hand,
    })
}

/// Record a count of the shelf.
///
/// The count asserts what was there at `counted_at_ms`; the server decides what
/// that means for on-hand, and replies with its own conclusion rather than
/// echoing the assertion. A device showing what it sent would hide exactly the
/// case worth seeing: a sale that arrived too late to have been counted.
pub(super) async fn record_count<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<RecordCountRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let mut on_hand = Vec::with_capacity(request.lines.len());
    for line in &request.lines {
        let count = StockCount {
            id: line.id,
            item_id: line.item_id,
            counted_milli: line.counted_milli,
            counted_at_ms: request.counted_at_ms,
            counted_by: caller.terminal,
            note: request.note.clone(),
        };
        if state
            .repo
            .record_count(caller.tenant, &count)
            .await
            .is_err()
        {
            return unavailable();
        }
        match state.repo.on_hand(caller.tenant, line.item_id).await {
            Ok(figure) => on_hand.push(OnHandEntry {
                item_id: figure.item_id,
                qty_milli: figure.qty_milli,
                counted_at_ms: figure.counted_at_ms,
                unreconciled_milli: figure.unreconciled_milli,
                unreconciled_sales: u32::try_from(figure.unreconciled_sales).unwrap_or(u32::MAX),
            }),
            Err(_) => return unavailable(),
        }
    }

    let unreconciled: usize = on_hand
        .iter()
        .filter(|entry| entry.unreconciled_sales > 0)
        .count();
    tracing::info!(
        tenant = %caller.tenant,
        lines = request.lines.len(),
        unreconciled,
        "stock count recorded"
    );

    encoded(&RecordCountResponse { protocol, on_hand })
}

/// Issue a code that will enrol a new device.
///
/// Owner only, and a caller may not grant a role above its own. The second rule
/// is trivially satisfied while there are two roles and only owners can reach
/// this route, and it is written down anyway: the day a third role exists, this
/// is the line that would otherwise have been missing.
pub(super) async fn issue_code<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<IssueCodeRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let granted = Role::from_i16(request.role);
    if !caller.role.covers(granted) {
        return protocol_error(&ProtocolError::NotPermitted);
    }

    // The new device gets its own terminal row before the code exists, so a
    // redeemed code always names something real. Doing it the other way round
    // leaves a code that enrols a device into a terminal that was never
    // created, which fails at the worst moment: a shop standing there with a
    // new tablet.
    if state
        .repo
        .register_terminal(caller.tenant, request.terminal_id, &request.label)
        .await
        .is_err()
    {
        return unavailable();
    }

    let valid_for = Duration::from_secs(
        request
            .valid_for_seconds
            .clamp(60, MAX_CODE_LIFETIME.as_secs()),
    );
    let code = EnrolmentCode::generate();
    let grants = Caller {
        tenant: caller.tenant,
        terminal: request.terminal_id,
        role: granted,
    };

    if state
        .repo
        .issue_enrolment_code(grants, &code.hash(), valid_for)
        .await
        .is_err()
    {
        return unavailable();
    }

    tracing::info!(
        tenant = %caller.tenant,
        terminal = %request.terminal_id,
        role = request.role,
        "enrolment code issued"
    );
    encoded(&IssueCodeResponse {
        protocol,
        code: code.into_string(),
        terminal_id: request.terminal_id,
        expires_in_seconds: valid_for.as_secs(),
    })
}

/// Sales the server could not accept as they stood.
///
/// Read-only, and deliberately a POST like everything else here: the body is
/// postcard, and a GET with a postcard body is not something a cache, a proxy or
/// a browser will treat consistently.
/// What the shop made over a period. Owner only.
///
/// The second question an owner asks, after what was taken. A shop that cannot
/// answer it stocks by feel: a sack of rice that moves twice a day at four taka
/// of margin is worth less shelf than soap that moves twice a week at forty.
///
/// The part the shop cannot answer for is reported beside the figure rather
/// than folded into it. Half a margin read as a whole one is worse than no
/// margin at all.
pub(super) async fn made<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<MadeRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // A backwards range is a mistake rather than a question, and answering it
    // with zero reads as a period that made nothing.
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .made(caller.tenant, request.from_ms, request.to_ms)
        .await
    {
        Ok(summary) => encoded(&MadeResponse {
            protocol,
            net_minor: summary.net_minor,
            cost_minor: summary.cost_minor,
            made_minor: summary.made_minor,
            sales: summary.sales,
            sales_without_cost: summary.sales_without_cost,
            net_without_cost_minor: summary.net_without_cost_minor,
        }),
        Err(_) => unavailable(),
    }
}

/// What was on a receipt. Owner only.
///
/// Somebody comes back to the counter with a piece of paper and says they were
/// charged twice, or for something they did not take. The shop held every one
/// of those sales and had no way to look one up: the repair queue answers which
/// sales went wrong, the day answers what was taken, and neither answers what
/// was on this.
///
/// Read out of the bytes the till committed rather than out of a summary, so
/// what the screen shows is what the customer's paper said. A sale whose bytes
/// this build cannot decode is still listed, with what the shop does know about
/// it, because "I cannot read it" is a better answer than an empty screen.
pub(super) async fn receipt<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    what_was_on_it(&state, caller, &body).await
}

/// The same question, asked by a till.
///
/// A customer comes back to the counter with a piece of paper, and the person
/// they hand it to is a cashier rather than the owner at a desk. Until now only
/// the back office could look a receipt up, so a refund at the counter was rung
/// by scanning the goods again at today's catalogue price: a basket sold with
/// ten percent off the ticket came back at full price, and the shop gave away
/// the discount a second time. What the till needs to do better is exactly what
/// this answers, which is what that paper said.
///
/// A till may ask about its own shop and no other, which is the credential's
/// doing rather than this route's: the shop is taken from the credential and
/// every query underneath is scoped to it.
pub(super) async fn receipt_for_a_till<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match super::caller_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    what_was_on_it(&state, caller, &body).await
}

async fn what_was_on_it<R: Repository>(
    state: &AppState<R>,
    caller: Caller,
    body: &Bytes,
) -> Response {
    let request = match decode::<ReceiptRequest>(body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;

    let asked = request.receipt_no.trim();
    if asked.is_empty() {
        return protocol_error(&ProtocolError::Malformed);
    }
    let held = match state.repo.sales_on_receipt(caller.tenant, asked).await {
        Ok(held) => held,
        Err(_) => return unavailable(),
    };
    if held.len() > 1 {
        tracing::info!(
            tenant = %caller.tenant,
            receipt = %asked,
            sales = held.len(),
            "one receipt number carries more than one sale"
        );
    }

    let found: Vec<SaleOnPaperWire> = held.into_iter().map(on_paper).collect();

    // A back office a release behind gets the shape it knows. What it loses is
    // saying why a sale is held in its own language, which it could not do
    // anyway; what it would lose otherwise is the whole reply, because these
    // bodies are positional and it would read a field it does not know as the
    // start of the next one.
    if protocol < 3 {
        return encoded(&ReceiptResponseV2 {
            protocol,
            found: found.into_iter().map(SaleOnPaperWireV2::from).collect(),
        });
    }
    // And a device from before a line said which item it was gets the shape it
    // knows. It has nowhere to put the id, and these bodies are positional: it
    // would read the id as the length of the name and answer somebody holding
    // a receipt with nonsense.
    if protocol < 7 {
        return encoded(&openpos_core::protocol::ReceiptResponseV6 {
            protocol,
            found: found
                .into_iter()
                .map(openpos_core::protocol::SaleOnPaperWireV6::from)
                .collect(),
        });
    }
    encoded(&ReceiptResponse { protocol, found })
}

/// One held sale, read out into what a person at a counter reads.
fn on_paper(sale: crate::repo::SaleOnPaper) -> SaleOnPaperWire {
    let read = openpos_core::storage::wire::decode_sale(
        openpos_core::storage::wire::SALE_SCHEMA,
        &sale.payload,
    )
    .ok()
    .or_else(|| {
        // Every schema this build knows, because a sale rung a year ago is
        // exactly the one somebody comes back about.
        [
            openpos_core::storage::wire::SALE_SCHEMA_V2,
            openpos_core::storage::wire::SALE_SCHEMA_V1,
        ]
        .into_iter()
        .find_map(|schema| openpos_core::storage::wire::decode_sale(schema, &sale.payload).ok())
    });

    let mut wire = SaleOnPaperWire {
        id: sale.id,
        terminal: sale.terminal,
        receipt_no: sale.receipt_no,
        rung_at_ms: sale.rung_at_ms,
        lines: Vec::new(),
        tenders: Vec::new(),
        net_minor: 0,
        vat_minor: 0,
        discount_minor: 0,
        total_minor: sale.total_minor,
        change_minor: 0,
        overrides: Vec::new(),
        held_for: sale.held_for.unwrap_or_default(),
        held_for_kind: postcard::from_bytes(&sale.held_for_bytes).ok(),
        decided: sale.decided.as_ref().map(|(said, _)| said.clone()),
        still_counts: sale.decided.as_ref().is_none_or(|(_, kept)| *kept),
        refunded_minor: sale.refunded_minor,
        refund_of: sale.refund_of,
    };
    let Some(read) = read else {
        // Bytes this build cannot read. What the shop knows from beside them is
        // still worth showing: the number, the till, the hour and the money.
        return wire;
    };
    let ticket = read.ticket;
    // The payload's own summary until the lines below are priced, and then the
    // shop's own reading of them. What the payload asserts about its totals is
    // checked when the sale arrives and quarantined when it disagrees; showing
    // the assertion here would be the one place a shop reads a figure it has
    // already decided is wrong.
    wire.net_minor = ticket.net_minor;
    wire.vat_minor = ticket.vat_minor;
    wire.discount_minor = ticket.discount_minor;
    wire.change_minor = ticket.change_minor;
    wire.overrides = ticket.overrides.clone();
    // Read with the same crate that priced the sale rather than reimplemented
    // here. A discount expressed as a rate has to come back as the taka that
    // came off, which is what the person holding the paper is arguing about,
    // and that arithmetic exists in exactly one place on purpose.
    //
    // The whole ticket rather than each line on its own, because a discount
    // taken off the ticket belongs to the lines it came off. Worked out line by
    // line, a basket with ten percent off showed every line at full price and a
    // total ten percent lower: the lines did not add up to the total on the
    // shop's own screen, and a refund built from them gave back the discount a
    // second time.
    let priced = ticket
        .ticket_discount
        .clone()
        .into_domain()
        .ok()
        .and_then(|discount| {
            let lines: Vec<_> = ticket
                .lines
                .iter()
                .filter_map(|line| line.clone().into_domain().ok())
                .collect();
            (lines.len() == ticket.lines.len()).then_some(lines).and_then(|lines| {
                openpos_core::domain::ticket_totals(&openpos_core::domain::pricing::TicketInput {
                    lines: lines.iter().map(openpos_core::cart::CartLine::as_input).collect(),
                    ticket_discount: discount,
                })
                .ok()
            })
        });
    if let Some(whole) = priced.as_ref() {
        wire.net_minor = whole.net_total.get();
        wire.vat_minor = whole.vat_total.get();
        wire.discount_minor = whole.discount_total.get();
    }
    for (at, line) in ticket.lines.iter().enumerate() {
        let Some(totals) = priced.as_ref().and_then(|whole| whole.lines.get(at)) else {
            continue;
        };
        wire.lines.push(PaperLineWire {
            item_id: line.item_id,
            name: line.name.clone(),
            qty_milli: line.qty_milli,
            unit: line.unit.clone(),
            unit_price_minor: line.unit_price_minor,
            // Everything that came off this line, including its share of what
            // came off the ticket.
            discount_minor: totals.discount.get(),
            vat_bp: line.vat_bp,
            // What the customer pays for this line, tax and all, which is what
            // they are adding up when they say the total is wrong.
            line_total_minor: totals.total.get(),
        });
    }
    for tender in &ticket.tenders {
        wire.tenders.push(PaperTenderWire {
            kind: match &tender.kind {
                openpos_core::storage::wire::TenderKindV1::Cash => String::from("Cash"),
                openpos_core::storage::wire::TenderKindV1::Card => String::from("Card"),
                openpos_core::storage::wire::TenderKindV1::Credit => String::from("On account"),
                openpos_core::storage::wire::TenderKindV1::Wallet(name)
                | openpos_core::storage::wire::TenderKindV1::Other(name) => name.clone(),
            },
            kind_code: String::from(match &tender.kind {
                openpos_core::storage::wire::TenderKindV1::Cash => "cash",
                openpos_core::storage::wire::TenderKindV1::Card => "card",
                openpos_core::storage::wire::TenderKindV1::Credit => "credit",
                openpos_core::storage::wire::TenderKindV1::Wallet(_)
                | openpos_core::storage::wire::TenderKindV1::Other(_) => "wallet",
            }),
            amount_minor: tender.amount_minor,
            reference: tender.reference.clone(),
        });
    }
    wire
}

pub(super) async fn repairs<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<RepairQueueRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    let caller = match require_owner(caller) {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };

    // Clamped rather than refused. A caller asking for everything wants as much
    // as it can have, and an error would leave the queue unreadable rather than
    // merely paged.
    let limit = request.limit.clamp(1, MAX_REPAIR_PAGE);
    match state.repo.repair_queue(caller.tenant, limit).await {
        Ok(queue) => {
            let entries: Vec<RepairEntry> = queue
                .into_iter()
                .map(|item| RepairEntry {
                    id: item.id,
                    receipt_no: item.receipt_no,
                    total_minor: item.total_minor,
                    received_at_ms: item.received_at_ms,
                    reason: item.reason,
                    // Absent for a sale held before the shop kept the reason
                    // itself, and for one whose bytes will not decode: the
                    // sentence beside it is what those are shown as.
                    held_for: postcard::from_bytes(&item.reason_bytes).ok(),
                })
                .collect();

            // A back office a release behind reads the sentence, which is what
            // it could show anyway. Sending the newer shape would not read as a
            // missing field: it would read as a decode failure, and the screen
            // would show an error where the queue should be.
            if protocol < 3 {
                return encoded(&RepairQueueResponseV2 {
                    protocol,
                    entries: entries.into_iter().map(RepairEntryV2::from).collect(),
                });
            }
            encoded(&RepairQueueResponse { protocol, entries })
        }
        Err(_) => unavailable(),
    }
}

/// Where the shop's numbering jumps.
///
/// A shop's receipt numbers are meant to run unbroken, and the question an
/// inspector asks is why they do not. Until now nobody could look: the numbers
/// were in the sales and nothing put them side by side.
pub(super) async fn receipt_gaps<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ReceiptGapsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .receipt_gaps(caller.tenant, request.limit.clamp(1, MAX_REPAIR_PAGE))
        .await
    {
        Ok(found) => encoded(&ReceiptGapsResponse {
            protocol,
            gaps: found
                .into_iter()
                .map(|gap| ReceiptGapWire {
                    terminal: gap.terminal,
                    epoch: gap.epoch,
                    after: gap.after,
                    before: gap.before,
                    missing: gap.missing,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Who allowed what, for the person who has to answer for it.
///
/// The report that closes the loop on every ceiling in the product: a cashier
/// who may not discount can still discount when a supervisor stands there and
/// types a PIN, and until this existed the shop had no way to see how often that
/// happened or who did it.
pub(super) async fn allowed<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<AllowedRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .allowed(
            caller.tenant,
            request.from_ms,
            request.to_ms,
            request.limit.clamp(1, MAX_ALLOWED_PAGE),
        )
        .await
    {
        Ok(found) => {
            let allowed: Vec<AllowedEntry> = found
                .into_iter()
                .map(|one| AllowedEntry {
                    terminal: one.terminal,
                    seq: one.seq,
                    at_ms: one.at_ms,
                    action: one.action,
                    bp: one.bp,
                    operator: one.operator,
                    operator_name: one.operator_name,
                    authorised_by: one.authorised_by,
                    authorised_by_name: one.authorised_by_name,
                    receipt_no: one.receipt_no.clone(),
                })
                .collect();

            // A back office a release behind reads who and when, which is what
            // it could show anyway. Sending the newer shape would not read as a
            // missing field: it would read as a decode failure, and the screen
            // would show an error where the trail should be, on the screen a
            // shop opens when it suspects something.
            if protocol < 5 {
                return encoded(&AllowedResponseV4 {
                    protocol,
                    allowed: allowed.into_iter().map(AllowedEntryV4::from).collect(),
                });
            }
            encoded(&AllowedResponse { protocol, allowed })
        }
        Err(_) => unavailable(),
    }
}

/// What the shop has decided lately.
///
/// The queue only shows what is waiting, so an answer given in error left no
/// screen it could be reached from. A strike-out takes a real debt off
/// somebody's account, so somebody who has just given the wrong answer has to
/// be able to find it again.
pub(super) async fn decided<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<DecidedRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let limit = request.limit.clamp(1, MAX_REPAIR_PAGE);
    match state.repo.decided(caller.tenant, limit).await {
        Ok(found) => encoded(&DecidedResponse {
            protocol,
            decided: found
                .into_iter()
                .map(|one| DecidedEntry {
                    id: one.id,
                    receipt_no: one.receipt_no,
                    total_minor: one.total_minor,
                    reason: one.reason,
                    note: one.note,
                    kept: one.kept,
                    decided_at_ms: one.decided_at_ms,
                    decisions: one.decisions,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Change an answer already given about a sale.
pub(super) async fn decide_again<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<DecideAgainRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    let caller = match require_owner(caller) {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };
    // A reason is required, as it is for the first answer: this is what
    // somebody reads when they ask why a figure moved after the month closed.
    if request.note.trim().is_empty() || request.note.len() > MAX_RESOLUTION_NOTE {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .decide_again(
            caller.tenant,
            request.sale,
            &request.note,
            request.kept,
            request.expected_decisions,
        )
        .await
    {
        Ok(outcome) => {
            let changed = outcome == Decided::Changed;
            if changed {
                // Logged like the first answer, and for the same reason: this is
                // the one back-office action that changes what a later audit
                // sees. The note is not logged; it is stored beside the sale.
                tracing::info!(
                    tenant = %caller.tenant,
                    sale = %request.sale,
                    kept = request.kept,
                    "a sale was decided again"
                );
            }
            encoded(&DecideAgainResponse {
                protocol,
                changed,
                stale: outcome == Decided::Stale,
            })
        }
        Err(_) => unavailable(),
    }
}

/// Take one sale out of the queue.
pub(super) async fn resolve_repair<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // The current shape first, then the one a back office loaded before this
    // existed sends. Those bytes are a strict prefix, so they would otherwise
    // come back "malformed" to somebody who had done nothing wrong.
    let request = match decode::<ResolveRepairRequest>(&body) {
        Ok(request) => request,
        Err(error) => match decode::<ResolveRepairRequestV1>(&body) {
            Ok(old) => ResolveRepairRequest {
                protocol: old.protocol,
                tenant: old.tenant,
                terminal: old.terminal,
                sale: old.sale,
                note: old.note,
                kept: true,
            },
            Err(_) => return protocol_error(&error),
        },
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    let caller = match require_owner(caller) {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };
    if request.note.len() > MAX_RESOLUTION_NOTE {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .resolve_quarantine(caller.tenant, request.sale, &request.note, request.kept)
        .await
    {
        Ok(resolved) => {
            if resolved {
                // Logged because this is the one back-office action that changes
                // what a later audit sees. The note is not logged: it is stored
                // beside the sale, and duplicating it here would scatter the
                // shop's own words across log files nobody reviews.
                tracing::info!(
                    tenant = %caller.tenant,
                    sale = %request.sale,
                    kept = request.kept,
                    "quarantined sale marked resolved"
                );
            }
            encoded(&ResolveRepairResponse { protocol, resolved })
        }
        Err(_) => unavailable(),
    }
}

/// Which tills are alive, and which are generating the support load.
pub(super) async fn terminals<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<TerminalHealthRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    let caller = match require_owner(caller) {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };

    match state.repo.terminal_health(caller.tenant).await {
        Ok(health) => encoded(&TerminalHealthResponse {
            protocol,
            terminals: health
                .into_iter()
                .map(|entry| TerminalHealthEntry {
                    terminal: entry.terminal,
                    label: entry.label,
                    epoch: entry.epoch,
                    enrolled_at_ms: entry.enrolled_at_ms,
                    last_seen_ms: entry.last_seen_ms,
                    sales: entry.sales,
                    open_repairs: entry.open_repairs,
                    role: entry.role,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Create or replace one item, which tills pick up on their next pull.
/// One item as the shop holds it now. Owner only.
///
/// Read before an edit, so a correction is built on what the shop has rather
/// than on a device's copy of the catalogue, which is up to half a minute
/// behind. The sequence comes with it and goes back with the save.
pub(super) async fn item_now<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ItemNowRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.item_now(caller.tenant, request.item_id).await {
        Ok(Some((item, seq))) => encoded(&ItemNowResponse {
            protocol,
            item: Some(item),
            seq,
        }),
        // Withdrawn, or never here. Either way there is nothing to edit, and a
        // screen that says so is better than one that offers a blank form.
        Ok(None) => encoded(&ItemNowResponse {
            protocol,
            item: None,
            seq: 0,
        }),
        Err(_) => unavailable(),
    }
}

/// Add or correct an item. Owner only.
pub(super) async fn upsert_item<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<UpsertItemRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    let caller = match require_owner(caller) {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };

    // Refused here, where it is written, because of what it costs where it is
    // read: a till applies a page of catalogue changes as one batch and refuses
    // the whole batch if any item in it cannot be priced. One impossible rate
    // stored here stops every till in the shop from seeing any price change at
    // all, and nothing at either end says why.
    if let Err(refusal) = priceable(&request.item) {
        tracing::info!(
            tenant = %caller.tenant,
            item = %request.item.id,
            said = %refusal,
            "refused an item no till could price"
        );
        return protocol_error(&priced_for(protocol, refusal));
    }

    // Built on an older copy than the shop holds: somebody else changed this
    // item while it was being edited, and a whole-item save would carry every
    // stale field back over their change. Refused rather than merged, because
    // there is no merging a whole-item save and the older answer would win.
    if request.expected_seq != 0 {
        match state.repo.item_now(caller.tenant, request.item.id).await {
            Ok(Some((_, seq))) if seq != request.expected_seq => {
                return protocol_error(&ProtocolError::Stale);
            }
            // Withdrawn since it was read. An edit that would bring it back is
            // the case this exists for.
            Ok(None) => return protocol_error(&ProtocolError::Stale),
            Ok(Some(_)) => {}
            Err(_) => return unavailable(),
        }
    }

    // A barcode belongs to one item. Two items carrying the same one means a
    // scan rings whichever the till's index happened to keep: the wrong price,
    // the wrong tax rate, the wrong thing off the shelf, and a shop that cannot
    // see why. The replica has said "the back office is responsible for not
    // issuing one" since it was written, and until now nothing was.
    if !request.item.barcodes.is_empty() {
        match state
            .repo
            .barcode_holders(caller.tenant, &request.item.barcodes)
            .await
        {
            Ok(holders) => {
                if let Some((code, holder)) = holders
                    .into_iter()
                    .find(|(_, holder)| *holder != request.item.id)
                {
                    tracing::info!(
                        tenant = %caller.tenant,
                        item = %request.item.id,
                        held_by = %holder,
                        "refused an item whose barcode another item holds"
                    );
                    return protocol_error(&ProtocolError::BarcodeInUse { barcode: code });
                }
            }
            Err(_) => return unavailable(),
        }
    }

    // The item is written under the tenant from the credential, so an item id
    // colliding with another shop's is that shop's business and not this one's.
    match state.repo.upsert_item(caller.tenant, &request.item).await {
        Ok(cursor) => {
            tracing::info!(
                tenant = %caller.tenant,
                item = %request.item.id,
                cursor,
                "catalogue item upserted"
            );
            encoded(&CatalogueEditResponse { protocol, cursor })
        }
        Err(_) => unavailable(),
    }
}

/// Withdraw one item, which reaches tills as a tombstone.
/// Whether a till could price this item at all.
///
/// The same bounds the core applies when it reads one, checked before it is
/// stored rather than after it has been sent to every device: `Bp::vat` refuses
/// a rate over a hundred percent, and a price below nothing would make a line
/// pay the customer.
pub(super) fn priceable(item: &ItemWire) -> Result<(), ProtocolError> {
    if openpos_core::money::Bp::vat(item.vat_bp).is_err() {
        return Err(ProtocolError::RateIsNotARate { bp: item.vat_bp });
    }
    if item.price_minor < 0 {
        return Err(ProtocolError::PriceBelowNothing {
            minor: item.price_minor,
        });
    }
    if item.cost_minor < 0 {
        return Err(ProtocolError::CostBelowNothing {
            minor: item.cost_minor,
        });
    }
    Ok(())
}

/// The same refusal in the shape a caller a version behind can read.
///
/// The three above carry figures, which is what lets a screen say them in the
/// shop's own language. A back office built before they existed cannot decode
/// them at all and would fall back to the status number, so it is handed the
/// sentence it has always had.
pub(super) fn priced_for(protocol: u16, refusal: ProtocolError) -> ProtocolError {
    if protocol >= 4 {
        return refusal;
    }
    ProtocolError::NotAPrice {
        said: format!("{refusal}"),
    }
}

pub(super) async fn delete_item<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<DeleteItemRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    let caller = match require_owner(caller) {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };

    // Asked before anything is written, because a deletion cannot be taken
    // back: the tombstone is on its way to every till by the next pull, and
    // what it removes is the name behind figures the shop still has to answer
    // for. Withdrawing does the part that was wanted and keeps the record.
    match state
        .repo
        .item_has_history(caller.tenant, request.item)
        .await
    {
        Ok(true) => {
            tracing::info!(
                tenant = %caller.tenant,
                item = %request.item,
                "refused to delete an item the shop has traded"
            );
            return protocol_error(&ProtocolError::ItemHasHistory);
        }
        Ok(false) => {}
        Err(_) => return unavailable(),
    }

    // Deleting something that was never there still appends a tombstone. That is
    // deliberate: a till which somehow holds the item drops it, and a till that
    // never did ignores an id it does not know.
    match state.repo.delete_item(caller.tenant, request.item).await {
        Ok(cursor) => {
            tracing::info!(
                tenant = %caller.tenant,
                item = %request.item,
                cursor,
                "catalogue item deleted"
            );
            encoded(&CatalogueEditResponse { protocol, cursor })
        }
        Err(_) => unavailable(),
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

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use openpos_core::protocol::{
        CountedItem, EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse, PROTOCOL_VERSION,
        PullRequest, PullResponse, PushShiftsRequest, PushShiftsRequestV1, PushShiftsResponse,
        QuarantineReason, ReceiptLineWire, TakePaymentRequest, TakePaymentResponse,
    };
    use tower::ServiceExt;

    use super::*;
    // These tests reach the back office through the router, as a device does.
    use crate::http::{AppState, CONTENT_TYPE, router};
    // The setup a till's own routes already needed. Shared rather than copied:
    // two of these would drift, and the one used least would be the one wrong.
    use crate::http::tests::{
        TENANT, TERMINAL, app, app_with_till, item, post_to, repair_request, shop_with_a_repair,
    };
    use crate::repo::{MemoryRepo, StoredSale};

    #[tokio::test]
    async fn the_repair_queue_says_what_the_sale_was_and_what_was_wrong_with_it() {
        let (app, token) = shop_with_a_repair().await;
        let (status, body) = post_to::<_, RepairQueueResponse>(
            app,
            "/v1/back-office/repairs",
            &repair_request(),
            Some(&token),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let queue = body.unwrap();
        assert_eq!(queue.entries.len(), 1);
        let entry = &queue.entries[0];
        assert_eq!(entry.id, 900);
        assert_eq!(entry.receipt_no.as_deref(), Some("T7-000100"));
        assert_eq!(entry.total_minor, 49_450);
        // The queue is worked by a person, so the reason has to read as one:
        // taka and poisha rather than a count of poisha.
        assert!(
            entry.reason.contains("494.50"),
            "the entry must say what disagreed, in money: {}",
            entry.reason
        );
    }

    #[tokio::test]
    async fn a_back_office_left_open_across_the_upgrade_is_still_answered() {
        let (app, token) = shop_with_a_repair().await;
        // Exactly the bytes a screen loaded yesterday sends: a note, and
        // nothing about whether the sale stands.
        let old = openpos_core::protocol::ResolveRepairRequestV1 {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sale: 900,
            note: "checked against the paper receipt".to_owned(),
        };
        let (status, body) = post_to::<_, ResolveRepairResponse>(
            app.clone(),
            "/v1/back-office/repairs/resolve",
            &old,
            Some(&token),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::OK,
            "not malformed: it is a shape we wrote"
        );
        assert!(body.unwrap().resolved);
        // And read as the only thing resolving used to mean: it stands.
        let (_, day) = post_to::<_, DayResponse>(
            app,
            "/v1/back-office/day",
            &DayRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: 0,
                to_ms: u64::MAX,
            },
            Some(&token),
        )
        .await;
        assert_eq!(day.unwrap().sales, 1, "the sale still counts");
    }

    #[tokio::test]
    async fn a_resolved_sale_leaves_the_queue_and_resolving_it_again_says_nothing_moved() {
        let (app, token) = shop_with_a_repair().await;
        let resolve = ResolveRepairRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sale: 900,
            note: "cashier re-rang it, the paper receipt matches".to_owned(),
            kept: true,
        };

        let (status, body) = post_to::<_, ResolveRepairResponse>(
            app.clone(),
            "/v1/back-office/repairs/resolve",
            &resolve,
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.unwrap().resolved);

        let (_, queue) = post_to::<_, RepairQueueResponse>(
            app.clone(),
            "/v1/back-office/repairs",
            &repair_request(),
            Some(&token),
        )
        .await;
        assert!(
            queue.unwrap().entries.is_empty(),
            "a worked queue must actually empty, or nobody can tell what is left"
        );

        // Two people working one queue: the second is told it was already done
        // rather than overwriting the first one's note.
        let (_, again) = post_to::<_, ResolveRepairResponse>(
            app,
            "/v1/back-office/repairs/resolve",
            &resolve,
            Some(&token),
        )
        .await;
        assert!(!again.unwrap().resolved);
    }

    #[tokio::test]
    async fn resolving_a_sale_that_is_not_in_the_queue_changes_nothing() {
        let (app, token) = shop_with_a_repair().await;
        let resolve = ResolveRepairRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sale: 12_345,
            note: "nothing to resolve".to_owned(),
            kept: true,
        };
        let (status, body) = post_to::<_, ResolveRepairResponse>(
            app,
            "/v1/back-office/repairs/resolve",
            &resolve,
            Some(&token),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(!body.unwrap().resolved);
    }

    #[tokio::test]
    async fn a_note_too_long_to_be_one_is_refused() {
        // A client bug looping on a growing string must not write an unbounded
        // value into a row the queue reads back on every load.
        let (app, token) = shop_with_a_repair().await;
        let resolve = ResolveRepairRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            sale: 900,
            note: "x".repeat(MAX_RESOLUTION_NOTE + 1),
            kept: true,
        };
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/repairs/resolve",
            &resolve,
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn terminal_health_counts_the_sales_a_till_sent_and_the_ones_still_open() {
        let (app, token) = shop_with_a_repair().await;
        let request = TerminalHealthRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
        };
        let (status, body) = post_to::<_, TerminalHealthResponse>(
            app,
            "/v1/back-office/terminals",
            &request,
            Some(&token),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let health = body.unwrap();
        assert_eq!(health.terminals.len(), 1);
        let entry = &health.terminals[0];
        assert_eq!(entry.terminal, TERMINAL);
        assert_eq!(entry.epoch, 1);
        assert_eq!(entry.sales, 1);
        assert_eq!(entry.open_repairs, 1, "the queue and the health list agree");
        assert!(entry.enrolled_at_ms > 0);
    }

    #[tokio::test]
    async fn a_terminal_that_has_never_synced_is_shown_as_never_heard_from() {
        let (app, token) = app();
        let request = TerminalHealthRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
        };
        let (_, before) = post_to::<_, TerminalHealthResponse>(
            app.clone(),
            "/v1/back-office/terminals",
            &request,
            Some(&token),
        )
        .await;
        // Absent, not zero. Zero would render as 1970 and read as a fault.
        assert_eq!(before.unwrap().terminals[0].last_seen_ms, None);

        // Any sync counts, including one that carries no sales, because a till
        // open on a quiet day is alive and must not be reported as dead.
        let pull = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        };
        let (status, _) =
            post_to::<_, PullResponse>(app.clone(), "/v1/sync/pull", &pull, Some(&token)).await;
        assert_eq!(status, StatusCode::OK);

        let (_, after) = post_to::<_, TerminalHealthResponse>(
            app,
            "/v1/back-office/terminals",
            &request,
            Some(&token),
        )
        .await;
        assert!(after.unwrap().terminals[0].last_seen_ms.is_some());
    }

    #[tokio::test]
    async fn an_edited_item_reaches_a_till_on_its_next_pull() {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        let app = router(AppState::new(repo));

        let edit = UpsertItemRequest {
            expected_seq: 0,
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            item: item(3),
        };
        let (status, body) = post_to::<_, CatalogueEditResponse>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &edit,
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.unwrap().cursor, 1);

        let pull = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        };
        let (_, page) =
            post_to::<_, PullResponse>(app.clone(), "/v1/sync/pull", &pull, Some(&token)).await;
        let page = page.unwrap();
        assert_eq!(page.upserts, vec![item(3)]);

        // And withdrawing it reaches the till as a tombstone, without which a
        // deleted item lingers on every device that already has it.
        let delete = DeleteItemRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            item: 3,
        };
        let (status, body) = post_to::<_, CatalogueEditResponse>(
            app.clone(),
            "/v1/back-office/catalogue/delete",
            &delete,
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.unwrap().cursor, 2);

        let next = PullRequest {
            cursor: page.cursor,
            ..pull
        };
        let (_, page) = post_to::<_, PullResponse>(app, "/v1/sync/pull", &next, Some(&token)).await;
        assert_eq!(page.unwrap().tombstones, vec![3]);
    }

    /// The back office is behind the same credential as everything else, so an
    /// unauthenticated caller cannot read a shop's takings or edit its prices.
    #[tokio::test]
    async fn the_back_office_refuses_a_caller_with_no_credential() {
        let (app, _) = shop_with_a_repair().await;
        let (queue, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/repairs",
            &repair_request(),
            None,
        )
        .await;
        assert_eq!(queue, StatusCode::UNAUTHORIZED);

        let (health, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/terminals",
            &TerminalHealthRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
            },
            None,
        )
        .await;
        assert_eq!(health, StatusCode::UNAUTHORIZED);

        let (edit, body) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                expected_seq: 0,
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: item(3),
            },
            None,
        )
        .await;
        assert_eq!(edit, StatusCode::UNAUTHORIZED);
        assert_eq!(body, Some(ProtocolError::Unauthenticated));
    }

    #[tokio::test]
    async fn one_shops_credential_cannot_read_another_shops_repair_queue() {
        let repo = MemoryRepo::new();
        let intruder = repo.enrol_with_token(TENANT, TERMINAL);
        repo.enrol(999, 888);
        repo.store_sale(StoredSale {
            tenant: 999,
            terminal: 888,
            id: 901,
            receipt_no: None,
            receipt_epoch: None,
            rung_at_ms: 0,
            total_minor: 10_000,
            payload: vec![],
            quarantine: Some(QuarantineReason::Undecodable),
            stock: vec![],
            vat: Vec::new(),
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();
        let app = router(AppState::new(repo));

        let request = RepairQueueRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 999,
            terminal: 888,
            limit: 50,
        };
        let (status, body) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/repairs",
            &request,
            Some(intruder.as_str()),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::UnknownTerminal));
    }

    #[tokio::test]
    async fn a_count_is_answered_with_what_the_server_concluded() {
        let (app, token) = app();

        let (status, body) = post_to::<_, RecordCountResponse>(
            app,
            "/v1/back-office/stock/count",
            &RecordCountRequest {
                protocol: PROTOCOL_VERSION,
                counted_at_ms: 5_000,
                note: Some("Friday count".to_owned()),
                lines: vec![CountedItem {
                    id: 900,
                    item_id: 2,
                    counted_milli: 40_000,
                }],
            },
            Some(&token),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let reply = body.expect("a reply");
        // Echoing back what was sent would hide the one case worth seeing.
        assert_eq!(reply.on_hand.len(), 1);
        assert_eq!(reply.on_hand[0].item_id, 2);
        assert_eq!(reply.on_hand[0].counted_at_ms, Some(5_000));
    }

    #[tokio::test]
    async fn counting_needs_a_credential() {
        let (app, _token) = app();

        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/stock/count",
            &RecordCountRequest {
                protocol: PROTOCOL_VERSION,
                counted_at_ms: 5_000,
                note: None,
                lines: vec![],
            },
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_delivery_raises_stock_and_a_retry_is_recognised() {
        let (app, token) = app();
        let delivery = ReceiveGoodsRequest {
            protocol: PROTOCOL_VERSION,
            id: 700,
            supplier_id: None,
            reference: Some("CHALLAN-4471".to_owned()),
            received_at_ms: 3_000,
            note: None,
            lines: vec![ReceiptLineWire {
                item_id: 2,
                qty_milli: 60_000,
                unit_cost_minor: 38_000,
            }],
        };

        let (status, body) = post_to::<_, ReceiveGoodsResponse>(
            app.clone(),
            "/v1/back-office/stock/receive",
            &delivery,
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let first = body.expect("a reply");
        assert!(first.recorded);
        assert_eq!(first.on_hand[0].qty_milli, 60_000);

        // A retry after a dropped reply. Told it was already booked, and still
        // told where stock stands: otherwise the only way to find out is to
        // guess.
        let (status, body) = post_to::<_, ReceiveGoodsResponse>(
            app,
            "/v1/back-office/stock/receive",
            &delivery,
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let second = body.expect("a reply");
        assert!(
            !second.recorded,
            "stock booked twice is a shop ordering against goods it lacks"
        );
        assert_eq!(second.on_hand[0].qty_milli, 60_000);
    }

    #[tokio::test]
    async fn suppliers_are_listed_for_the_shop_that_asked() {
        let (app, token) = app();

        let (status, _) = post_to::<_, SuppliersResponse>(
            app.clone(),
            "/v1/back-office/suppliers/put",
            &PutSupplierRequest {
                protocol: PROTOCOL_VERSION,
                supplier: SupplierWire {
                    id: 800,
                    name: "Karim Traders".to_owned(),
                    phone: Some("01700000000".to_owned()),
                    bin: None,
                    active: true,
                },
            },
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, SuppliersResponse>(
            app,
            "/v1/back-office/suppliers",
            &SuppliersRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let listed = body.expect("a reply");
        assert_eq!(listed.suppliers.len(), 1);
        assert_eq!(listed.suppliers[0].name, "Karim Traders");
    }

    #[tokio::test]
    async fn purchasing_needs_a_credential() {
        let (app, _token) = app();

        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/stock/receive",
            &ReceiveGoodsRequest {
                protocol: PROTOCOL_VERSION,
                id: 700,
                supplier_id: None,
                reference: None,
                received_at_ms: 0,
                note: None,
                lines: vec![],
            },
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_till_credential_cannot_reprice_the_shop() {
        let (app, _owner, till) = app_with_till().await;

        // A shop with six tills had six devices that could reprice the whole
        // catalogue, and any one left on a counter was the whole shop.
        let (status, body) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                expected_seq: 0,
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: item(3),
            },
            Some(&till),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::NotPermitted));
    }

    #[tokio::test]
    async fn every_back_office_route_refuses_a_till_credential() {
        let (app, _owner, till) = app_with_till().await;

        // Named individually, because the failure this guards against is a
        // route added later without the check, and a loop over the routes that
        // exist today would not catch that either. This at least fails loudly
        // if one of the current ones loses its guard.
        let routes = [
            "/v1/back-office/repairs",
            "/v1/back-office/terminals",
            "/v1/back-office/catalogue/delete",
            "/v1/back-office/stock/count",
            "/v1/back-office/stock/receive",
            "/v1/back-office/suppliers",
            "/v1/back-office/suppliers/put",
        ];

        for route in routes {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(route)
                        .header(header::CONTENT_TYPE, CONTENT_TYPE)
                        .header(header::AUTHORIZATION, format!("Bearer {till}"))
                        .body(Body::from(
                            postcard::to_allocvec(&SuppliersRequest {
                                protocol: PROTOCOL_VERSION,
                            })
                            .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();

            // Either forbidden, or refused before that for a body this route
            // does not understand. Never OK.
            assert_ne!(response.status(), StatusCode::OK, "{route} accepted a till");
        }
    }

    #[tokio::test]
    async fn an_owner_enrols_a_second_device_end_to_end() {
        let (app, owner, _till) = app_with_till().await;

        let (status, body) = post_to::<_, IssueCodeResponse>(
            app.clone(),
            "/v1/back-office/enrolment-codes",
            &IssueCodeRequest {
                protocol: PROTOCOL_VERSION,
                terminal_id: 500,
                label: "the one by the door".to_owned(),
                role: Role::Till.as_i16(),
                valid_for_seconds: 900,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let issued = body.expect("a code");
        assert_eq!(issued.terminal_id, 500);

        // The new tablet reads the code off the owner's screen.
        let (status, body) = post_to::<_, EnrolResponse>(
            app.clone(),
            "/v1/enrol",
            &EnrolRequest {
                protocol: PROTOCOL_VERSION,
                code: issued.code.clone(),
            },
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let enrolled = body.expect("a credential");
        assert_eq!(
            enrolled.terminal, 500,
            "the new device gets its own identity, not the identity of the one that asked"
        );

        // It can sell.
        let (status, _) = post_to::<_, PullResponse>(
            app.clone(),
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: enrolled.tenant,
                terminal: enrolled.terminal,
                cursor: 0,
                limit: 10,
            },
            Some(&enrolled.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // And it cannot reprice the shop, because the code said till.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                expected_seq: 0,
                protocol: PROTOCOL_VERSION,
                tenant: enrolled.tenant,
                terminal: enrolled.terminal,
                item: item(3),
            },
            Some(&enrolled.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_till_cannot_mint_a_credential_for_anything() {
        let (app, _owner, till) = app_with_till().await;

        // Otherwise the role means nothing: a till that can issue codes can
        // issue itself an owner one.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/enrolment-codes",
            &IssueCodeRequest {
                protocol: PROTOCOL_VERSION,
                terminal_id: 501,
                label: "smuggled".to_owned(),
                role: Role::Till.as_i16(),
                valid_for_seconds: 900,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_code_for_a_till_that_already_exists_brings_that_till_back_rather_than_a_new_one() {
        let (app, owner, _till) = app_with_till().await;

        // A device whose credential the server no longer accepts: revoked, or
        // restored from a backup taken before it enrolled. It looks enrolled to
        // itself and is refused on every request.
        //
        // Issuing a code with a fresh terminal id would give it a fresh ledger
        // and strand every sale the old one had not sent, so the back office
        // issues one for the terminal that is already there.
        let (status, body) = post_to::<_, IssueCodeResponse>(
            app.clone(),
            "/v1/back-office/enrolment-codes",
            &IssueCodeRequest {
                protocol: PROTOCOL_VERSION,
                terminal_id: TERMINAL,
                label: "front counter".to_owned(),
                role: Role::Till.as_i16(),
                valid_for_seconds: 900,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let issued = body.expect("a code");
        assert_eq!(
            issued.terminal_id, TERMINAL,
            "the same till, not another one"
        );

        let (status, body) = post_to::<_, EnrolResponse>(
            app.clone(),
            "/v1/enrol",
            &EnrolRequest {
                protocol: PROTOCOL_VERSION,
                code: issued.code.clone(),
            },
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let credential = body.expect("a credential");
        assert_eq!(credential.terminal, TERMINAL);
        assert_eq!(credential.tenant, TENANT);

        // And the new credential works as that terminal, which is the whole
        // point: the device comes back as itself, holding its own ledger.
        let (status, _) = post_to::<_, LeaseResponse>(
            app,
            "/v1/lease",
            &LeaseRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                count: 10,
            },
            Some(&credential.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_days_takings_counts_refunds_in_the_total_and_says_so_separately() {
        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();

        let day = 1_788_600_000_000_u64;
        for (id, terminal, at_ms, total, quarantine) in [
            (901_u128, TERMINAL, day + 1_000, 49_450_i64, None),
            (902, TERMINAL, day + 2_000, 12_500, None),
            // A refund: a sale with the signs turned round.
            (903, TERMINAL, day + 3_000, -49_450, None),
            // Quarantined, and still in the total: the goods left the shop and
            // the money changed hands, so a figure that omitted it would
            // disagree with the drawer.
            (
                904,
                TERMINAL,
                day + 4_000,
                20_000,
                Some(QuarantineReason::Undecodable),
            ),
            // Yesterday, which this day's figure must not include.
            (905, TERMINAL, day - 100_000, 99_999, None),
        ] {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal,
                id,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: at_ms,
                total_minor: total,
                payload: vec![],
                quarantine,
                stock: vec![],
                vat: Vec::new(),
                overrides: Vec::new(),
                on_account: vec![],
                refund_of: None,
                cash_minor: 0,
                cost_minor: 0,
                cost_known: false,
            })
            .await
            .unwrap();
        }

        let app = router(AppState::new(repo));
        let (status, body) = post_to::<_, DayResponse>(
            app.clone(),
            "/v1/back-office/day",
            &DayRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: day,
                to_ms: day + 86_400_000,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let took = body.expect("a figure");

        assert_eq!(took.sales, 4, "yesterday is not today");
        assert_eq!(took.total_minor, 49_450 + 12_500 - 49_450 + 20_000);
        // A day of thirty two thousand that is eighty one thousand of sales and
        // forty nine of refunds is not a quiet day, and the total alone cannot
        // say which it was.
        assert_eq!(took.refunds, 1);
        assert_eq!(took.refunded_minor, -49_450);
        assert_eq!(took.tills.len(), 1);
        assert_eq!(took.tills[0].needing_attention, 1);

        // A backwards range is a mistake, not a query: answering it with zero
        // reads as a day with no sales, which an owner would act on.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/day",
            &DayRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: day + 1,
                to_ms: day,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// A figure added up from part of a shelf is not a figure for the shelf.
    #[tokio::test]
    async fn asking_for_every_shelf_says_whether_it_answered_for_all_of_them() {
        use openpos_core::protocol::{OnHandRequest, OnHandResponse};

        let (app, owner, _till) = app_with_till().await;

        // This shop sells two things, so the answer covers all of them.
        let (status, body) = post_to::<_, OnHandResponse>(
            app.clone(),
            "/v1/back-office/stock/on-hand",
            &OnHandRequest {
                protocol: PROTOCOL_VERSION,
                item_ids: vec![],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let all = body.expect("an answer");
        assert!(
            all.whole,
            "two items is not more than the server will answer"
        );
        assert_eq!(all.on_hand.len(), 2);

        // Asked about one item, which is a page of a shelf however short the
        // shelf is: a screen adding that up has been given part of it.
        let (status, body) = post_to::<_, OnHandResponse>(
            app,
            "/v1/back-office/stock/on-hand",
            &OnHandRequest {
                protocol: PROTOCOL_VERSION,
                item_ids: vec![1],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let some = body.expect("an answer");
        assert!(
            !some.whole,
            "asked about one of them, told about one of them"
        );
        assert_eq!(some.on_hand.len(), 1);
    }

    /// The list of a shop's devices says which of them is the back office.
    ///
    /// Without it the only code a screen can offer a lost device is a till's,
    /// and a shop whose back office tablet is stolen finds it can bring the
    /// device back as a till and no further: the one owner's code it ever had
    /// was printed in the log the morning the server first started.
    #[tokio::test]
    async fn the_list_of_devices_says_which_one_is_the_back_office() {
        use openpos_core::protocol::{TerminalHealthRequest, TerminalHealthResponse};

        let (app, owner, _till) = app_with_till().await;

        let (status, body) = post_to::<_, TerminalHealthResponse>(
            app,
            "/v1/back-office/terminals",
            &TerminalHealthRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let devices = body.expect("a list").terminals;
        let this_one = devices
            .iter()
            .find(|entry| entry.terminal == TERMINAL)
            .expect("the device this shop was set up with");
        assert_eq!(
            this_one.role, 2,
            "it holds an owner's credential, so it is the back office as well"
        );
    }

    /// A delivery teaches the catalogue what the shop pays.
    ///
    /// Without this the margin can only ever say it does not know: the item
    /// form is not where a shop learns a price, and nobody types the same
    /// figure into two screens.
    #[tokio::test]
    async fn a_delivery_says_what_the_shop_pays_now() {
        use openpos_core::protocol::{
            ItemNowRequest, ItemNowResponse, ReceiptLineWire, ReceiveGoodsRequest,
            ReceiveGoodsResponse,
        };

        let (app, owner, _till) = app_with_till().await;

        let (status, body) = post_to::<_, ReceiveGoodsResponse>(
            app.clone(),
            "/v1/back-office/stock/receive",
            &ReceiveGoodsRequest {
                protocol: PROTOCOL_VERSION,
                id: 6_100,
                supplier_id: None,
                reference: Some(String::from("challan 41")),
                received_at_ms: 1_788_600_000_000,
                note: None,
                lines: vec![ReceiptLineWire {
                    item_id: 1,
                    qty_milli: 10_000,
                    unit_cost_minor: 39_500,
                }],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_some());

        let (status, body) = post_to::<_, ItemNowResponse>(
            app.clone(),
            "/v1/back-office/catalogue/item",
            &ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let held = body.expect("the item").item.expect("this shop sells it");
        assert_eq!(
            held.cost_minor, 39_500,
            "what the delivery charged, on the item every till pulls"
        );

        // A line booked with no price is somebody recording goods, not a price
        // change, and it leaves what the shop pays alone.
        let (status, _) = post_to::<_, ReceiveGoodsResponse>(
            app.clone(),
            "/v1/back-office/stock/receive",
            &ReceiveGoodsRequest {
                protocol: PROTOCOL_VERSION,
                id: 6_101,
                supplier_id: None,
                reference: Some(String::from("challan 42")),
                received_at_ms: 1_788_600_100_000,
                note: None,
                lines: vec![ReceiptLineWire {
                    item_id: 1,
                    qty_milli: 5_000,
                    unit_cost_minor: 0,
                }],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, ItemNowResponse>(
            app,
            "/v1/back-office/catalogue/item",
            &ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body.expect("the item")
                .item
                .expect("this shop sells it")
                .cost_minor,
            39_500,
            "still what the last delivery that said a price charged"
        );
    }

    /// And the same for a receipt it looks up.
    #[tokio::test]
    async fn a_back_office_one_version_behind_reads_a_receipt_it_knows() {
        use openpos_core::protocol::{ReceiptRequest, ReceiptResponseV2};

        let (app, owner, till) = app_with_till().await;

        // Two sales under one number, because one entry's extra field lands at
        // the end of the body where a decoder ignores it: with two, the first
        // one's is read as the start of the second.
        for id in [981_u128, 982] {
            let (status, _) = post_to::<_, openpos_core::protocol::PushResponse>(
                app.clone(),
                "/v1/sync/push",
                &openpos_core::protocol::PushRequest {
                    protocol: PROTOCOL_VERSION,
                    tenant: TENANT,
                    terminal: TERMINAL,
                    sales: vec![openpos_core::protocol::SaleEnvelope {
                        id,
                        schema: openpos_core::storage::wire::SALE_SCHEMA,
                        payload: sale_payload(id, "T1-000700"),
                    }],
                },
                Some(&till),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        let (status, older) = post_to::<_, ReceiptResponseV2>(
            app.clone(),
            "/v1/back-office/receipt",
            &ReceiptRequest {
                protocol: 2,
                receipt_no: String::from("T1-000700"),
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let older = older.expect("a receipt the older shape can read");
        assert_eq!(older.found.len(), 2);
        for sale in &older.found {
            assert_eq!(sale.receipt_no, "T1-000700");
            assert!(
                sale.total_minor > 0,
                "and each is the sale it was meant to be rather than the bytes of the next one: \
                 {sale:?}"
            );
        }
    }

    /// A back office a release behind still gets a queue it can read.
    /// A back office a version behind still gets a refusal it can read.
    ///
    /// The three refusals that carry figures did not exist at protocol 3, and a
    /// build from then cannot decode them at all: it would fall back to the
    /// status number and tell a shopkeeper nothing about what was wrong with
    /// the price they typed. So it is handed the sentence it has always had.
    #[tokio::test]
    async fn a_back_office_one_version_behind_still_hears_why_a_price_was_refused() {
        let (app, owner, _till) = app_with_till().await;

        let mut absurd = item(11);
        absurd.vat_bp = 15_000;
        let (status, refusal) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: 3,
                tenant: TENANT,
                terminal: TERMINAL,
                item: absurd,
                expected_seq: 0,
            },
            Some(&owner),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let refusal = refusal.expect("the shop says why");
        let ProtocolError::NotAPrice { said } = &refusal else {
            panic!("a caller at 3 gets the shape it knows, not {refusal:?}");
        };
        assert!(
            said.contains("150") && said.contains("not a tax rate"),
            "and the sentence still says what was wrong: {said}"
        );
    }

    #[tokio::test]
    async fn a_back_office_one_version_behind_reads_the_queue_it_knows() {
        use openpos_core::protocol::{RepairQueueRequest, RepairQueueResponseV2};

        let (app, owner, till) = app_with_till().await;

        // Two sales the shop holds, and two matters: a field added to the
        // entry shape is one extra byte per entry, and with a single entry it
        // lands at the end where a decoder can ignore it. With two, the first
        // entry's extra byte is read as the start of the second.
        let (status, _) = post_to::<_, openpos_core::protocol::PushResponse>(
            app.clone(),
            "/v1/sync/push",
            &openpos_core::protocol::PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sales: vec![
                    openpos_core::protocol::SaleEnvelope {
                        id: 995,
                        schema: openpos_core::storage::wire::SALE_SCHEMA,
                        payload: alloc_broken_payload(),
                    },
                    openpos_core::protocol::SaleEnvelope {
                        id: 996,
                        schema: openpos_core::storage::wire::SALE_SCHEMA,
                        payload: alloc_broken_payload(),
                    },
                ],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Version 2 is what the release before this one spoke. It gets the
        // shape it knows: the sentence, without the reason beside it. Sending
        // the newer shape would not read as a missing field, it would read as a
        // decode failure, and the screen would show an error where the queue
        // should be.
        let (status, older) = post_to::<_, RepairQueueResponseV2>(
            app.clone(),
            "/v1/back-office/repairs",
            &RepairQueueRequest {
                protocol: 2,
                tenant: TENANT,
                terminal: TERMINAL,
                limit: 10,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let older = older.expect("a queue the older shape can read");
        assert_eq!(older.entries.len(), 2);
        for entry in &older.entries {
            assert!(!entry.reason.is_empty(), "and each says why");
            assert!(
                entry.total_minor >= 0 && entry.received_at_ms > 0,
                "and each is the entry it was meant to be rather than the bytes of the next \
                 one read as this one: {entry:?}"
            );
        }
    }

    /// A till and a back office one release behind still hand over the trail.
    ///
    /// Which receipt a reprint was of is a change to two shapes that travel:
    /// what a till pushes and what the back office reads back. postcard is
    /// positional, so a build that speaks the older version is not looking at a
    /// missing field, it is looking at a decode failure. On the way up that
    /// leaves a device holding the only record of who allowed what while its
    /// pushes fail on a timer; on the way down it puts an error where the
    /// trail should be, on the screen a shop opens when it suspects something.
    #[tokio::test]
    async fn a_till_and_a_back_office_one_version_behind_still_hand_over_the_trail() {
        use openpos_core::protocol::{
            AllowedRequest, AllowedResponse, AllowedResponseV4, AllowedWireV4,
            PushAllowedRequestV4, PushAllowedResponse,
        };

        let (app, owner, till) = app_with_till().await;

        // A till on the version before this one, pushing a reprint and a
        // discount. It has no field for which receipt, and says so by not
        // having one rather than by sending an empty string.
        let (status, sent) = post_to::<_, PushAllowedResponse>(
            app.clone(),
            "/v1/sync/allowed",
            &PushAllowedRequestV4 {
                protocol: 4,
                tenant: TENANT,
                terminal: TERMINAL,
                allowed: vec![
                    AllowedWireV4 {
                        seq: 1,
                        at_ms: 1_788_600_000_000,
                        action: 14,
                        bp: 0,
                        operator: 71,
                        operator_name: String::from("Rahima"),
                        authorised_by: 0,
                        authorised_by_name: String::new(),
                    },
                    AllowedWireV4 {
                        seq: 2,
                        at_ms: 1_788_600_100_000,
                        action: 1,
                        bp: 1_000,
                        operator: 71,
                        operator_name: String::from("Rahima"),
                        authorised_by: 72,
                        authorised_by_name: String::from("Karim"),
                    },
                ],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            sent.expect("the shop took them").stored,
            vec![1, 2],
            "a till a release behind must still be able to hand over what it allowed"
        );

        // This build reads them back with nothing invented for the receipt.
        let (status, now) = post_to::<_, AllowedResponse>(
            app.clone(),
            "/v1/back-office/allowed",
            &AllowedRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: 0,
                to_ms: u64::MAX,
                limit: 10,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let now = now.expect("the trail");
        assert_eq!(now.allowed.len(), 2);
        assert!(
            now.allowed.iter().all(|one| one.receipt_no.is_none()),
            "the device did not know which receipt, and nothing here may decide for it"
        );

        // And a back office a release behind reads who and when, which is what
        // it could show anyway, rather than an error where the trail should be.
        let (status, older) = post_to::<_, AllowedResponseV4>(
            app.clone(),
            "/v1/back-office/allowed",
            &AllowedRequest {
                protocol: 4,
                from_ms: 0,
                to_ms: u64::MAX,
                limit: 10,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let older = older.expect("a trail the older shape can read");
        assert_eq!(older.allowed.len(), 2);
        for entry in &older.allowed {
            assert_eq!(
                entry.operator_name, "Rahima",
                "and each is the entry it was meant to be rather than the bytes of the next one \
                 read as this one: {entry:?}"
            );
        }
        assert_eq!(older.allowed[0].action, 1, "newest first, as ever");
        assert_eq!(older.allowed[1].action, 14);
    }

    /// A payload no build can decode, which is one of the things a shop holds a
    /// sale for.
    fn alloc_broken_payload() -> Vec<u8> {
        vec![0xff, 0xff, 0xff, 0xff]
    }

    /// One impossible rate would stop every till seeing any price at all.
    #[tokio::test]
    async fn an_item_no_till_could_price_is_refused_where_it_is_written() {
        let (app, owner, _till) = app_with_till().await;

        // Five thousand percent. A till applies a page of catalogue changes as
        // one batch and refuses the whole batch if any item in it cannot be
        // priced, so this one row stored here would stop every device in the
        // shop from receiving any price change, with nothing at either end
        // saying why.
        let mut absurd = item(9);
        absurd.vat_bp = 500_000;
        let (status, refusal) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: absurd,
                expected_seq: 0,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let refusal = refusal.expect("the shop says why");
        assert!(
            matches!(refusal, ProtocolError::RateIsNotARate { bp: 500_000 }),
            "the rate itself, not a sentence about it: {refusal:?}"
        );
        // In words, and naming the consequence: a rate nobody reads as absurd
        // is a shop wondering why its tills stopped updating.
        let said = format!("{refusal}");
        assert!(said.contains("5000 percent"), "{said}");
        assert!(said.contains("whole page"), "{said}");

        // And a price below nothing, which would make a line pay the customer.
        let mut backwards = item(10);
        backwards.price_minor = -1;
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: backwards,
                expected_seq: 0,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Neither reached the catalogue.
        let pull = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 50,
        };
        let (_, page) =
            post_to::<_, PullResponse>(app.clone(), "/v1/sync/pull", &pull, Some(&owner)).await;
        let page = page.expect("a page");
        assert!(
            !page.upserts.iter().any(|one| one.id == 9 || one.id == 10),
            "nothing a till cannot price is in the catalogue"
        );
    }

    /// Deleting something the shop has traded takes the name off its books.
    ///
    /// A deletion is a tombstone every till obeys on the next pull, and what it
    /// removes is the name behind figures still in the record: a sale that has
    /// been rung, a delivery that has been booked, a shelf that has been
    /// counted. Withdrawing does the part that was wanted, so this is refused
    /// in words that say so.
    #[tokio::test]
    async fn an_item_the_shop_has_sold_is_not_deleted_but_withdrawn() {
        use openpos_core::protocol::{DeleteItemRequest, PushRequest, PushResponse, SaleEnvelope};

        let (app, owner, till) = app_with_till().await;

        // Nothing has happened to item 2 yet, so it goes. This is the line
        // typed by mistake that the route exists for.
        let (status, _) = post_to::<_, CatalogueEditResponse>(
            app.clone(),
            "/v1/back-office/catalogue/delete",
            &DeleteItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: 2,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "an untraded item can be deleted");

        // Item 1 is the one the sale below rings.
        let (status, body) = post_to::<_, PushResponse>(
            app.clone(),
            "/v1/sync/push",
            &PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sales: vec![SaleEnvelope {
                    id: 991,
                    schema: openpos_core::storage::wire::SALE_SCHEMA,
                    payload: sale_payload(991, "T1-000991"),
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("taken").accepted.len(), 1);

        let (status, refusal) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/catalogue/delete",
            &DeleteItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: 1,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let refusal = refusal.expect("the shop says why");
        assert!(
            matches!(refusal, ProtocolError::ItemHasHistory),
            "and which refusal: {refusal:?}"
        );
        // In words, because a status number cannot say what to do instead.
        assert!(format!("{refusal}").contains("stop selling it"));

        // And it is still there afterwards: a refusal that had already written
        // the tombstone would be worse than no refusal at all.
        let (status, body) = post_to::<_, openpos_core::protocol::ItemNowResponse>(
            app.clone(),
            "/v1/back-office/catalogue/item",
            &openpos_core::protocol::ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.expect("an answer").item.is_some(),
            "still on the books"
        );
    }

    /// What a shop made, and how much of it it cannot answer for.
    #[tokio::test]
    async fn what_a_period_made_is_answered_with_what_it_cannot_answer_for() {
        use openpos_core::protocol::{
            MadeRequest, MadeResponse, PushRequest, PushResponse, SaleEnvelope,
        };

        let (app, owner, till) = app_with_till().await;

        let (status, body) = post_to::<_, PushResponse>(
            app.clone(),
            "/v1/sync/push",
            &PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sales: vec![SaleEnvelope {
                    id: 980,
                    schema: openpos_core::storage::wire::SALE_SCHEMA,
                    payload: sale_payload(980, "T1-000500"),
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("taken").accepted.len(), 1);

        let (status, body) = post_to::<_, MadeResponse>(
            app.clone(),
            "/v1/back-office/made",
            &MadeRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: 0,
                to_ms: u64::MAX,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let made = body.expect("a figure");
        // The line carries a cost of 380.00 against a price of 430.00, so the
        // sack made fifty taka and the tax was never the shop's money.
        assert_eq!(made.sales, 1);
        assert_eq!(made.net_minor, 43_000);
        assert_eq!(made.cost_minor, 38_000);
        assert_eq!(made.made_minor, 5_000, "fifty taka on the sack");
        assert_eq!(made.sales_without_cost, 0);

        // A backwards range is a mistake, not a question: answering it with
        // zero reads as a period that made nothing.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/made",
            &MadeRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: 1_000,
                to_ms: 999,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // And a till may not read what the shop makes.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/made",
            &MadeRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: 0,
                to_ms: u64::MAX,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// Somebody comes back to the counter with a piece of paper.
    ///
    /// The shop held the sale and could not look it up. What it needs to show is
    /// what the paper says: the goods, the money, and anything given back
    /// against it since.
    #[tokio::test]
    async fn a_receipt_somebody_brings_back_can_be_read_out() {
        use openpos_core::protocol::{
            PushRequest, PushResponse, ReceiptRequest, ReceiptResponse, SaleEnvelope,
        };

        let (app, owner, till) = app_with_till().await;

        let (status, body) = post_to::<_, PushResponse>(
            app.clone(),
            "/v1/sync/push",
            &PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sales: vec![SaleEnvelope {
                    id: 970,
                    schema: openpos_core::storage::wire::SALE_SCHEMA,
                    payload: sale_payload(970, "T1-000300"),
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("taken").accepted.len(), 1);

        let (status, body) = post_to::<_, ReceiptResponse>(
            app.clone(),
            "/v1/back-office/receipt",
            &ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: String::from("T1-000300"),
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("an answer").found;
        assert_eq!(found.len(), 1, "one sale carries that number");
        assert_eq!(found[0].total_minor, 49_450);
        assert_eq!(found[0].lines.len(), 1, "read out of the till's own bytes");
        assert_eq!(found[0].lines[0].name, "Rice Miniket 5kg");
        assert_eq!(found[0].lines[0].qty_milli, 1_000);
        assert_eq!(
            found[0].lines[0].line_total_minor, 49_450,
            "what they paid for it"
        );
        assert_eq!(found[0].tenders.len(), 1);
        assert_eq!(found[0].tenders[0].kind, "Cash");
        assert_eq!(found[0].tenders[0].amount_minor, 49_450);
        assert_eq!(found[0].refunded_minor, 0, "nothing has come back yet");
        assert!(found[0].held_for.is_empty(), "and nobody held it");
        assert!(found[0].still_counts);

        // A number nobody has is answered with nothing rather than an error: the
        // ordinary case is a customer reading their own handwriting wrongly.
        let (status, body) = post_to::<_, ReceiptResponse>(
            app.clone(),
            "/v1/back-office/receipt",
            &ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: String::from("T1-999999"),
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.expect("an answer").found.is_empty());

        // The back office's own route stays the back office's. A till asks the
        // same question at /v1/receipt, which exists because the customer
        // brings the paper to the counter and the person handed it is a
        // cashier; this one is kept for back offices built before that.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/receipt",
            &ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: String::from("T1-000300"),
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// A receipt with a discount off the whole ticket adds up.
    ///
    /// Priced line by line, a discount taken off the ticket belonged to none of
    /// them: every line showed at full price under a total ten percent lower,
    /// so the lines on the shop's own screen did not add up to the total on the
    /// shop's own screen. A refund built from those lines gave the discount
    /// away a second time, which is what sent somebody looking.
    #[tokio::test]
    async fn a_receipt_with_something_off_the_ticket_adds_up() {
        use openpos_core::protocol::{
            PushRequest, PushResponse, ReceiptRequest, ReceiptResponse, SaleEnvelope,
        };

        let (app, owner, till) = app_with_till().await;
        let (status, _) = post_to::<_, PushResponse>(
            app.clone(),
            "/v1/sync/push",
            &PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sales: vec![SaleEnvelope {
                    id: 971,
                    schema: openpos_core::storage::wire::SALE_SCHEMA,
                    payload: discounted_sale_payload(971, "T1-000301"),
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, ReceiptResponse>(
            app,
            "/v1/receipt",
            &ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: String::from("T1-000301"),
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("an answer").found;
        let sale = &found[0];

        // 430.00 with ten percent off the ticket: 387.00 and 58.05 of tax.
        assert_eq!(sale.total_minor, 44_505, "what they paid");
        assert_eq!(sale.discount_minor, 4_300, "and what came off");
        assert_eq!(sale.net_minor, 38_700);
        assert_eq!(sale.vat_minor, 5_805);
        assert_eq!(sale.lines.len(), 1);
        assert_eq!(
            sale.lines[0].discount_minor, 4_300,
            "the line carries the share of it that came off the line"
        );
        assert_eq!(
            sale.lines[0].line_total_minor, 44_505,
            "so the lines add up to the total the same screen shows"
        );
        let lines: i64 = sale.lines.iter().map(|line| line.line_total_minor).sum();
        assert_eq!(lines, sale.total_minor);
    }

    /// The same sale, with ten percent off the whole ticket.
    fn discounted_sale_payload(id: u128, receipt: &str) -> Vec<u8> {
        use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
        use openpos_core::ids::Ulid;
        use openpos_core::money::{Bp, Milli, Minor};

        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(
            &openpos_core::replica::Item {
                id: Ulid::from_u128(1),
                code: "RICE5".into(),
                name_en: "Rice Miniket 5kg".into(),
                name_bn: "মিনিকেট চাল ৫ কেজি".into(),
                unit: "Nos".into(),
                price: Minor::new(43_000),
                cost: Minor::new(38_000),
                vat_rate: Bp::new(1_500).unwrap(),
                price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
                vat_base: openpos_core::domain::pricing::VatBase::Discounted,
                barcodes: vec!["8690000000001".into()],
                on_hand: Milli::new(40_000),
                active: true,
                supply: openpos_core::domain::Supply::Standard,
                category: "".into(),
            },
            Milli::ONE,
        )
        .unwrap();
        cart.set_ticket_discount(openpos_core::domain::pricing::Discount::Rate(
            Bp::new(1_000).unwrap(),
        ))
        .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(44_505),
            reference: None,
        });
        let mut ticket = cart
            .close(
                Ulid::from_u128(id),
                Ulid::from_u128(4_242),
                1_788_600_000_000,
            )
            .unwrap();
        ticket.receipt_no = Some(receipt.into());
        openpos_core::storage::wire::encode_sale(&openpos_core::storage::wire::sale_commit(
            &ticket,
            Some(1),
            None,
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn a_counted_drawer_reaches_the_owner_and_keeps_its_variance() {
        let (app, owner, till) = app_with_till().await;

        // What a cashier closing up produces: a float, a day's takings, and a
        // count that is forty taka short of what the till expected.
        let closing = ClosedShiftWire {
            id: 700,
            terminal: TERMINAL,
            closed_by: 91,
            closed_by_name: "Rahima".to_owned(),
            opened_at_ms: 1_788_600_000_000,
            closed_at_ms: 1_788_640_000_000,
            opening_float_minor: 50_000,
            sales: 37,
            cash_sales_minor: 124_500,
            non_cash_sales_minor: 30_000,
            cash_in_minor: 0,
            cash_out_minor: 20_000,
            expected_cash_minor: 154_500,
            counted_cash_minor: 150_500,
            variance_minor: -4_000,
            expected_from_sales_minor: None,
            struck_out_cash_minor: None,
        };
        let (status, body) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: vec![closing.clone()],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "a till reports its own drawer");
        assert_eq!(body.expect("accepted").accepted, vec![700]);

        // Sending it again is ordinary: a dropped reply is the usual reason a
        // till sends one twice, and it must be told it may stop.
        let (status, body) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: vec![closing],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("accepted").accepted, vec![700]);

        // And the owner reads it. This is what the counting was for: somebody
        // who was not standing at the till seeing what it expected, what was in
        // it, and the difference.
        let (status, body) = post_to::<_, ShiftsResponse>(
            app.clone(),
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a list").shifts;
        assert_eq!(found.len(), 1, "stored once, not twice");
        assert_eq!(found[0].expected_cash_minor, 154_500);
        assert_eq!(found[0].counted_cash_minor, 150_500);
        assert_eq!(
            found[0].variance_minor, -4_000,
            "forty taka short, and it says so"
        );
        // And the owner is told who counted it. A variance attached to a till
        // and a time is half of what they want to know.
        assert_eq!(found[0].closed_by_name, "Rahima");
        assert_eq!(found[0].sales, 37);

        // A till may not read the shop's drawers, only report its own.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// A till reporting a smaller expectation than it took hides a shortfall,
    /// and the shop's own sales say so.
    #[tokio::test]
    async fn the_shop_checks_a_counted_drawer_against_its_own_sales() {
        use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
        use openpos_core::ids::Ulid;
        use openpos_core::money::{Bp, Milli, Minor};
        use openpos_core::protocol::{PushRequest, PushResponse, SaleEnvelope};

        let (app, owner, till) = app_with_till().await;

        // Two real sales, rung on this till while the drawer was open. Cash,
        // exact money, so what stayed in the drawer is what was rung.
        let mut sales = Vec::new();
        for (index, id) in [960_u128, 961].into_iter().enumerate() {
            let mut cart = Cart::new(CartLimits::unrestricted());
            cart.add_item(
                &openpos_core::replica::Item {
                    id: Ulid::from_u128(1),
                    code: "RICE5".into(),
                    name_en: "Rice Miniket 5kg".into(),
                    name_bn: "মিনিকেট চাল ৫ কেজি".into(),
                    unit: "Nos".into(),
                    price: Minor::new(43_000),
                    cost: Minor::new(38_000),
                    vat_rate: Bp::new(1_500).unwrap(),
                    price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
                    vat_base: openpos_core::domain::pricing::VatBase::Discounted,
                    barcodes: vec!["8690000000001".into()],
                    on_hand: Milli::new(40_000),
                    active: true,
                    supply: openpos_core::domain::Supply::Standard,
                    category: "".into(),
                },
                Milli::ONE,
            )
            .unwrap();
            cart.add_tender(Tender {
                kind: TenderKind::Cash,
                amount: Minor::new(49_450),
                reference: None,
            });
            let mut ticket = cart
                .close(
                    Ulid::from_u128(id),
                    Ulid::from_u128(TERMINAL),
                    1_788_610_000_000,
                )
                .unwrap();
            ticket.receipt_no = Some(format!("T1-00020{index}").into());
            sales.push(SaleEnvelope {
                id,
                schema: openpos_core::storage::wire::SALE_SCHEMA,
                payload: openpos_core::storage::wire::encode_sale(
                    &openpos_core::storage::wire::sale_commit(&ticket, Some(1), None),
                )
                .unwrap(),
            });
        }
        let (status, body) = post_to::<_, PushResponse>(
            app.clone(),
            "/v1/sync/push",
            &PushRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sales,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("taken").accepted.len(), 2);

        // And the drawer, reported by a till that says it only took one of
        // them. The count matches that story exactly, so the variance the till
        // offers is zero and nothing on the till's own figures is wrong.
        let (status, _) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: vec![ClosedShiftWire {
                    id: 702,
                    terminal: TERMINAL,
                    closed_by: 91,
                    closed_by_name: "Rahima".to_owned(),
                    opened_at_ms: 1_788_600_000_000,
                    closed_at_ms: 1_788_640_000_000,
                    opening_float_minor: 50_000,
                    sales: 1,
                    cash_sales_minor: 49_450,
                    non_cash_sales_minor: 0,
                    cash_in_minor: 0,
                    cash_out_minor: 0,
                    expected_cash_minor: 99_450,
                    expected_from_sales_minor: None,
                    struck_out_cash_minor: None,
                    counted_cash_minor: 99_450,
                    variance_minor: 0,
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, ShiftsResponse>(
            app,
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a list").shifts;
        assert_eq!(found[0].expected_cash_minor, 99_450, "what the till said");
        assert_eq!(
            found[0].expected_from_sales_minor,
            Some(50_000 + 49_450 + 49_450),
            "and what the shop's own sales come to"
        );
        assert_eq!(
            found[0].counted_cash_minor, 99_450,
            "a count that agrees with the till and not with the shop"
        );
        assert_eq!(
            found[0].struck_out_cash_minor, None,
            "and nothing struck out, so nothing to explain the gap with"
        );
    }

    /// A gap the shop made itself is named as the shop's own doing.
    ///
    /// The drawer keeps what the evening recorded, deliberately: a duplicate
    /// that inflated what the till expected is exactly what that evening was
    /// short by, and rewriting it now would erase the evidence. So the shop's
    /// own sales and the till's word part company for good the moment somebody
    /// strikes a sale out, and the screen's only explanation was a till that
    /// has not finished sending. An owner whose till has finished sending was
    /// pointed at the person who counted the drawer.
    #[tokio::test]
    async fn a_sale_struck_out_afterwards_is_named_beside_the_drawer_it_was_in() {
        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        let till = crate::auth::Token::generate();
        repo.store_token_as(
            crate::auth::Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: crate::auth::Role::Till,
            },
            &till.hash(),
            crate::auth::Role::Till,
        )
        .await
        .expect("the in-memory store accepts a token");

        // A cash sale rung while the drawer was open, held for somebody to look
        // at, and then struck out: it was rung twice and this is the second.
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 970,
            receipt_no: Some("T1-000300".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_610_000_000,
            total_minor: 49_450,
            payload: vec![],
            quarantine: Some(QuarantineReason::DuplicateReceiptNumber {
                receipt_no: "T1-000300".to_owned(),
            }),
            stock: vec![],
            vat: Vec::new(),
            overrides: Vec::new(),
            on_account: vec![],
            refund_of: None,
            cash_minor: 49_450,
            cost_minor: 0,
            cost_known: true,
        })
        .await
        .expect("stored");
        let app = router(AppState::new(repo));
        let till = till.into_string();

        let (status, decided) = post_to::<_, ResolveRepairResponse>(
            app.clone(),
            "/v1/back-office/repairs/resolve",
            &ResolveRepairRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                sale: 970,
                note: "rung twice".to_owned(),
                kept: false,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(decided.expect("an answer").resolved);

        // The drawer, as the till reported it: it counted that sale, because
        // when it closed nobody had said the sale did not happen.
        let (status, _) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: vec![ClosedShiftWire {
                    id: 703,
                    terminal: TERMINAL,
                    closed_by: 91,
                    closed_by_name: "Rahima".to_owned(),
                    opened_at_ms: 1_788_600_000_000,
                    closed_at_ms: 1_788_640_000_000,
                    opening_float_minor: 50_000,
                    sales: 1,
                    cash_sales_minor: 49_450,
                    non_cash_sales_minor: 0,
                    cash_in_minor: 0,
                    cash_out_minor: 0,
                    expected_cash_minor: 99_450,
                    expected_from_sales_minor: None,
                    struck_out_cash_minor: None,
                    counted_cash_minor: 99_450,
                    variance_minor: 0,
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, ShiftsResponse>(
            app,
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a list").shifts;
        assert_eq!(found[0].expected_cash_minor, 99_450, "what the till said");
        assert_eq!(
            found[0].expected_from_sales_minor,
            Some(50_000),
            "and the shop's own sales, which no longer count the struck-out one"
        );
        assert_eq!(
            found[0].struck_out_cash_minor,
            Some(49_450),
            "which is exactly the gap, and the screen can now say whose doing it was"
        );
    }

    #[tokio::test]
    async fn a_till_a_release_behind_can_still_hand_over_what_it_counted() {
        let (app, owner, till) = app_with_till().await;

        // A version 1 body: the same drawer, in the shape that build sends,
        // with no room in it for who counted.
        let (status, body) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequestV1 {
                protocol: 1,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: vec![ClosedShiftWireV1 {
                    id: 701,
                    terminal: TERMINAL,
                    opened_at_ms: 1_788_600_000_000,
                    closed_at_ms: 1_788_640_000_000,
                    opening_float_minor: 50_000,
                    sales: 4,
                    cash_sales_minor: 49_450,
                    non_cash_sales_minor: 0,
                    cash_in_minor: 0,
                    cash_out_minor: 0,
                    expected_cash_minor: 99_450,
                    counted_cash_minor: 95_450,
                    variance_minor: -4_000,
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a device a release behind is not left holding the only copy"
        );
        assert_eq!(body.expect("accepted").accepted, vec![701]);

        // The count is kept whole and the name is empty, because that build
        // never wrote one down. The shop is told what happened and cannot be
        // told by whom, which is the truth about it.
        let (status, body) = post_to::<_, ShiftsResponse>(
            app.clone(),
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a list").shifts;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].variance_minor, -4_000);
        assert_eq!(found[0].closed_by, 0);
        assert!(found[0].closed_by_name.is_empty());

        // And a back office a release behind reads the same drawer in its own
        // shape. Sending it the newer one would not read as a missing field; it
        // would read as different numbers.
        let (status, body) = post_to::<_, ShiftsResponseV1>(
            app,
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: 1,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a list").shifts;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].expected_cash_minor, 99_450);
        assert_eq!(found[0].counted_cash_minor, 95_450);
        assert_eq!(found[0].variance_minor, -4_000);
    }

    #[tokio::test]
    async fn a_debt_struck_off_needs_a_reason() {
        let (app, owner, _till) = app_with_till().await;

        // The one entry here that makes money disappear. One that vanishes
        // without a reason leaves the question this book exists to answer
        // unanswerable six months later.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/owed/payment",
            &TakePaymentRequest {
                protocol: PROTOCOL_VERSION,
                id: 5_100,
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor: 10_000,
                at_ms: 1_788_900_000_000,
                note: None,
                written_off: true,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Blank is not a reason either.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/owed/payment",
            &TakePaymentRequest {
                protocol: PROTOCOL_VERSION,
                id: 5_100,
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor: 10_000,
                at_ms: 1_788_900_000_000,
                note: Some("   ".to_owned()),
                written_off: true,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // With one, it goes in, and the balance comes back for this person
        // rather than being looked up in a page they might not be on.
        let (status, body) = post_to::<_, TakePaymentResponse>(
            app,
            "/v1/back-office/owed/payment",
            &TakePaymentRequest {
                protocol: PROTOCOL_VERSION,
                id: 5_100,
                person_key: "karim".to_owned(),
                person_name: "Karim".to_owned(),
                amount_minor: 10_000,
                at_ms: 1_788_900_000_000,
                note: Some("rung twice after the tablet was restored".to_owned()),
                written_off: true,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let reply = body.expect("a reply");
        assert!(reply.taken);
        // Nothing was owed, so striking off leaves them in credit, and the
        // screen is told that rather than being handed a zero.
        assert_eq!(reply.owed_minor, -10_000);
    }

    #[tokio::test]
    async fn a_payment_for_somebody_off_the_end_of_the_list_still_reads_back() {
        let (app, owner, _till) = app_with_till().await;

        // Six hundred accounts, and the one being settled owes the least, so it
        // is off the end of any page the reply could have searched.
        for index in 0..600_u128 {
            let (status, _) = post_to::<_, TakePaymentResponse>(
                app.clone(),
                "/v1/back-office/owed/payment",
                &TakePaymentRequest {
                    protocol: PROTOCOL_VERSION,
                    id: 6_000 + index,
                    person_key: format!("person {index}"),
                    person_name: format!("Person {index}"),
                    amount_minor: 100 + i64::try_from(index).unwrap_or_default(),
                    at_ms: 1_788_900_000_000,
                    note: None,
                    written_off: false,
                },
                Some(&owner),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        let (status, body) = post_to::<_, TakePaymentResponse>(
            app,
            "/v1/back-office/owed/payment",
            &TakePaymentRequest {
                protocol: PROTOCOL_VERSION,
                id: 7_000,
                person_key: "person 0".to_owned(),
                person_name: "Person 0".to_owned(),
                amount_minor: 100,
                at_ms: 1_788_900_100_000,
                note: None,
                written_off: false,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body.expect("a reply").owed_minor,
            -200,
            "asked for this person, not looked up in a page they are not on"
        );
    }

    /// One real sale, encoded as a till writes it.
    ///
    /// Built through the cart rather than hand-assembled, so the totals check on
    /// the server sees the arithmetic it would see from a device.
    fn sale_payload(id: u128, receipt: &str) -> Vec<u8> {
        use openpos_core::cart::{Cart, CartLimits, Tender, TenderKind};
        use openpos_core::ids::Ulid;
        use openpos_core::money::{Bp, Milli, Minor};

        let mut cart = Cart::new(CartLimits::unrestricted());
        cart.add_item(
            &openpos_core::replica::Item {
                id: Ulid::from_u128(1),
                code: "RICE5".into(),
                name_en: "Rice Miniket 5kg".into(),
                name_bn: "মিনিকেট চাল ৫ কেজি".into(),
                unit: "Nos".into(),
                price: Minor::new(43_000),
                cost: Minor::new(38_000),
                vat_rate: Bp::new(1_500).unwrap(),
                price_mode: openpos_core::domain::pricing::PriceMode::Exclusive,
                vat_base: openpos_core::domain::pricing::VatBase::Discounted,
                barcodes: vec!["8690000000001".into()],
                on_hand: Milli::new(40_000),
                active: true,
                supply: openpos_core::domain::Supply::Standard,
                category: "".into(),
            },
            Milli::ONE,
        )
        .unwrap();
        cart.add_tender(Tender {
            kind: TenderKind::Cash,
            amount: Minor::new(49_450),
            reference: None,
        });
        let mut ticket = cart
            .close(
                Ulid::from_u128(id),
                Ulid::from_u128(4_242),
                1_788_600_000_000,
            )
            .unwrap();
        ticket.receipt_no = Some(receipt.into());
        openpos_core::storage::wire::encode_sale(&openpos_core::storage::wire::sale_commit(
            &ticket,
            Some(1),
            None,
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn a_till_that_cannot_send_can_still_be_carried_in() {
        use openpos_core::protocol::{AdoptSalesRequest, AdoptSalesResponse, SaleEnvelope};

        let (app, owner, till) = app_with_till().await;

        // A device whose terminal the shop deleted, holding a sale it rang and
        // printed. Its bytes are the only record of goods that left the shop.
        let carried = SaleEnvelope {
            id: 950,
            schema: openpos_core::storage::wire::SALE_SCHEMA,
            payload: sale_payload(950, "T9-000001"),
        };
        let (status, body) = post_to::<_, AdoptSalesResponse>(
            app.clone(),
            "/v1/back-office/sales/adopt",
            &AdoptSalesRequest {
                protocol: PROTOCOL_VERSION,
                // A terminal this shop no longer lists. The receipts say it, so
                // the sale is filed under it.
                terminal: 4_242,
                sales: vec![carried.clone()],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let taken = body.expect("a reply");
        assert_eq!(taken.adopted, vec![950]);
        // Carried in is itself a reason for somebody to look: the credential
        // that would ordinarily say where a sale came from is what is missing.
        assert_eq!(taken.needing_attention.len(), 1);
        assert!(matches!(
            taken.needing_attention[0].reason,
            QuarantineReason::CarriedIn
        ));

        // Carried in twice, because the first attempt looked like it hung. The
        // device may be wiped once, not once per attempt.
        let (status, body) = post_to::<_, AdoptSalesResponse>(
            app.clone(),
            "/v1/back-office/sales/adopt",
            &AdoptSalesRequest {
                protocol: PROTOCOL_VERSION,
                terminal: 4_242,
                sales: vec![carried.clone()],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let again = body.expect("a reply");
        assert_eq!(again.adopted, vec![950]);
        assert!(
            again.needing_attention.is_empty(),
            "the shop already had it, and pointing at a queue entry that is \
             already there sends somebody looking for it twice"
        );

        // And it is in the queue a person works, once.
        let (status, body) = post_to::<_, RepairQueueResponse>(
            app.clone(),
            "/v1/back-office/repairs",
            &RepairQueueRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                limit: 50,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let queue = body.expect("a queue").entries;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, 950);
        assert!(queue[0].reason.contains("carried in"));

        // A till may not do this. The route exists because a credential is
        // missing, so accepting one that has a credential would be a way for any
        // device to file sales under any terminal it liked.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/sales/adopt",
            &AdoptSalesRequest {
                protocol: PROTOCOL_VERSION,
                terminal: 4_242,
                sales: vec![carried],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_till_can_answer_how_much_do_i_owe() {
        use openpos_core::protocol::{BalancesRequest, BalancesResponse};

        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL);
        let karim = openpos_core::accounts::customer_key(21);

        // Two sales on one written-down person, and one against a name typed at
        // a till. Only the first two can be shown against a record.
        for (id, key, amount) in [
            (901_u128, karim.clone(), 29_450_i64),
            (902, karim.clone(), 10_000),
            (903, "somebody karim".to_owned(), 5_000),
        ] {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal: TERMINAL,
                id,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: 1_788_600_000_000,
                total_minor: amount,
                payload: vec![],
                quarantine: None,
                stock: vec![],
                vat: vec![],
                overrides: Vec::new(),
                on_account: vec![crate::repo::AccountCharge {
                    person_key: key,
                    person_name: "Karim".to_owned(),
                    amount_minor: amount,
                }],
                refund_of: None,
                cash_minor: 0,
                cost_minor: 0,
                cost_known: false,
            })
            .await
            .unwrap();
        }

        let (status, body) = post_to::<_, BalancesResponse>(
            router(AppState::new(repo)),
            "/v1/customers/owed",
            &BalancesRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
            },
            Some(&owner.into_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let owed = body.expect("balances").balances;
        assert_eq!(owed.len(), 1, "only what can be shown against a record");
        assert_eq!(owed[0].customer, 21);
        assert_eq!(owed[0].owed_minor, 39_450);
    }

    #[tokio::test]
    async fn a_barcode_another_item_holds_is_refused_and_says_which() {
        use openpos_core::protocol::{ItemNowRequest, ItemNowResponse};

        let (app, owner, _till) = app_with_till().await;
        let (_, body) = post_to::<_, ItemNowResponse>(
            app.clone(),
            "/v1/back-office/catalogue/item",
            &ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        let held = body.expect("an item").item.expect("it is there");

        // A second item typed with the first one's barcode. Nobody decides to
        // do this; it is a thumb on a keyboard, and the till would then ring
        // whichever of the two its index happened to keep.
        let mut soap = held.clone();
        soap.id = 2;
        soap.code = "SOAP1".to_owned();
        soap.name_en = "Soap".to_owned();
        let (status, refusal) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: soap,
                expected_seq: 0,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let refusal = refusal.expect("the shop says why");
        assert!(
            matches!(refusal, ProtocolError::BarcodeInUse { ref barcode } if barcode == &held.barcodes[0]),
            "and which barcode: {refusal:?}"
        );
        // In words, because a status number cannot say which code is taken.
        assert!(format!("{refusal}").contains(&held.barcodes[0]));

        // Correcting the item that holds it is not a clash with itself.
        let mut renamed = held.clone();
        renamed.name_en = "Rice Miniket 5kg, new sack".to_owned();
        let (status, _) = post_to::<_, CatalogueEditResponse>(
            app,
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: renamed,
                expected_seq: 0,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_correction_built_on_a_stale_copy_is_refused_rather_than_merged() {
        use openpos_core::protocol::{ItemNowRequest, ItemNowResponse};

        let (app, owner, _till) = app_with_till().await;

        // What an owner about to edit an item should be looking at: the shop's
        // copy, not their device's, which is up to half a minute behind.
        let (status, body) = post_to::<_, ItemNowResponse>(
            app.clone(),
            "/v1/back-office/catalogue/item",
            &ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let read = body.expect("an item");
        let held = read.item.expect("the shop has it");
        assert!(read.seq > 0);

        // Somebody at another device changes it while this form is open.
        let mut theirs = held.clone();
        theirs.price_minor = 55_000;
        let (status, _) = post_to::<_, CatalogueEditResponse>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: theirs,
                expected_seq: read.seq,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // The first person presses save. Their copy has the old price and,
        // worse, whatever else has changed since: a whole-item save cannot be
        // merged, so the older answer would win by accident.
        let mut mine = held.clone();
        mine.name_en = "Rice Miniket 5kg, new sack".to_owned();
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: mine.clone(),
                expected_seq: read.seq,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Reading again and saving on top of what the shop now holds works,
        // which is what the person does after being told.
        let (_, body) = post_to::<_, ItemNowResponse>(
            app.clone(),
            "/v1/back-office/catalogue/item",
            &ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        let fresh = body.expect("an item");
        let mut again = fresh.item.expect("still here");
        again.name_en = "Rice Miniket 5kg, new sack".to_owned();
        let (status, _) = post_to::<_, CatalogueEditResponse>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: again,
                expected_seq: fresh.seq,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // And the other person's price survived, which is the whole point.
        let (_, body) = post_to::<_, ItemNowResponse>(
            app,
            "/v1/back-office/catalogue/item",
            &ItemNowRequest {
                protocol: PROTOCOL_VERSION,
                item_id: 1,
            },
            Some(&owner),
        )
        .await;
        let ended = body.expect("an item").item.expect("still here");
        assert_eq!(ended.price_minor, 55_000, "their change is not undone");
        assert_eq!(
            ended.name_en, "Rice Miniket 5kg, new sack",
            "and mine is in"
        );
    }

    #[tokio::test]
    async fn a_lost_tablet_can_be_cut_off_and_what_it_holds_can_still_come_back() {
        use openpos_core::protocol::{
            AdoptSalesRequest, AdoptSalesResponse, RevokeTerminalRequest, RevokeTerminalResponse,
            SaleEnvelope,
        };

        // A shop with two devices: the back office on one terminal and the
        // till on another, which is the arrangement this is about. The owner
        // cutting off a device has to be a different device.
        let counter = 8_u128;
        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        repo.enrol(TENANT, counter);
        repo.upsert_item(TENANT, crate::http::tests::item(1));
        let till_token = crate::auth::Token::generate();
        repo.store_token_as(
            crate::auth::Caller {
                tenant: TENANT,
                terminal: counter,
                role: crate::auth::Role::Till,
            },
            &till_token.hash(),
            crate::auth::Role::Till,
        )
        .await
        .expect("the in-memory store accepts a token");
        let till = till_token.into_string();
        let app = router(AppState::new(repo));

        // The till works, which is the thing being taken away.
        let (status, _) = post_to::<_, PullResponse>(
            app.clone(),
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: counter,
                cursor: 0,
                limit: 10,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, RevokeTerminalResponse>(
            app.clone(),
            "/v1/back-office/terminals/revoke",
            &RevokeTerminalRequest {
                protocol: PROTOCOL_VERSION,
                terminal: counter,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.expect("a reply").withdrawn >= 1);

        // And now it does nothing. This is the whole point: a tablet in
        // somebody else's hands rings no sales into this shop.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: counter,
                cursor: 0,
                limit: 10,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // If it turns up still holding sales, they are read off it and carried
        // in by hand, which is a route that exists and does not need the
        // credential this one no longer has.
        let (status, body) = post_to::<_, AdoptSalesResponse>(
            app.clone(),
            "/v1/back-office/sales/adopt",
            &AdoptSalesRequest {
                protocol: PROTOCOL_VERSION,
                terminal: counter,
                sales: vec![SaleEnvelope {
                    id: 960,
                    schema: openpos_core::storage::wire::SALE_SCHEMA,
                    payload: sale_payload(960, "T1-000900"),
                }],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("a reply").adopted, vec![960]);

        // An owner may not cut off the device they are holding: one press and
        // the shop is a set of tills nobody can issue a code from.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/terminals/revoke",
            &RevokeTerminalRequest {
                protocol: PROTOCOL_VERSION,
                terminal: TERMINAL,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn what_a_supervisor_waived_is_something_the_owner_can_look_at() {
        use openpos_core::protocol::{WaivedRequest, WaivedResponse};

        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        let day = 1_788_600_000_000_u64;

        // A sale with a waiver on it, as the till writes one: the same words
        // that printed on the customer's receipt.
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 940,
            receipt_no: None,
            receipt_epoch: None,
            rung_at_ms: day + 1_000,
            total_minor: 44_500,
            payload: vec![],
            quarantine: None,
            stock: vec![],
            vat: vec![],
            overrides: vec!["Karim allowed a discount of 10%".to_owned()],
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();

        // And an ordinary one, which is not in this answer.
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 941,
            receipt_no: None,
            receipt_epoch: None,
            rung_at_ms: day + 2_000,
            total_minor: 10_000,
            payload: vec![],
            quarantine: None,
            stock: vec![],
            vat: vec![],
            overrides: vec![],
            on_account: vec![],
            refund_of: None,
            cash_minor: 0,
            cost_minor: 0,
            cost_known: false,
        })
        .await
        .unwrap();

        let (status, body) = post_to::<_, WaivedResponse>(
            router(AppState::new(repo)),
            "/v1/back-office/waived",
            &WaivedRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: day,
                to_ms: day + 10_000,
                limit: 50,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let seen = body.expect("a list").waived;
        assert_eq!(seen.len(), 1, "only what somebody had to allow");
        assert_eq!(seen[0].sale_id, 940);
        assert_eq!(seen[0].total_minor, 44_500);
        assert!(
            seen[0].reason.contains("Karim"),
            "on whose say-so, in the words the customer's paper used"
        );
    }

    #[tokio::test]
    async fn what_sold_is_what_left_the_shelf() {
        use openpos_core::protocol::{SoldRequest, SoldResponse};

        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        let day = 1_788_600_000_000_u64;

        // Two sales of rice and one of oil, and a return of one bag of rice.
        // A shop orders against what left the shelf, so the return comes off.
        for (id, at_ms, stock) in [
            (901_u128, day + 1_000, vec![(1_u128, -2_000_i64)]),
            (902, day + 2_000, vec![(1, -1_000), (2, -3_000)]),
            (903, day + 3_000, vec![(1, 1_000)]),
            // Last month, which this question is not about.
            (904, day - 40_000_000_000, vec![(1, -9_000)]),
        ] {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal: TERMINAL,
                id,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: at_ms,
                total_minor: 1_000,
                payload: vec![],
                quarantine: None,
                stock,
                vat: vec![],
                overrides: Vec::new(),
                on_account: vec![],
                refund_of: None,
                cash_minor: 0,
                cost_minor: 0,
                cost_known: false,
            })
            .await
            .unwrap();
        }

        let (status, body) = post_to::<_, SoldResponse>(
            router(AppState::new(repo)),
            "/v1/back-office/sold",
            &SoldRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: day,
                to_ms: day + 10_000,
                limit: 50,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let rows = body.expect("what sold").rows;
        assert_eq!(
            rows.len(),
            2,
            "most sold first, and last month is not in it"
        );
        assert_eq!(rows[0].item_id, 2);
        assert_eq!(rows[0].qty_milli, 3_000);
        assert_eq!(rows[1].item_id, 1);
        assert_eq!(rows[1].qty_milli, 2_000, "three sold and one brought back");
        assert_eq!(rows[1].sales, 3, "over three tickets, the return included");
    }

    #[tokio::test]
    async fn what_the_shop_owes_a_supplier_adds_up_the_same_way_the_till_would() {
        use openpos_core::protocol::{
            PaySupplierRequest, PaySupplierResponse, SupplierOwingRequest, SupplierOwingResponse,
        };

        let (app, owner, till) = app_with_till().await;

        let distributor = 4_242_u128;
        let (status, _) = post_to::<_, SuppliersResponse>(
            app.clone(),
            "/v1/back-office/suppliers/put",
            &PutSupplierRequest {
                protocol: PROTOCOL_VERSION,
                supplier: SupplierWire {
                    id: distributor,
                    name: "Mirpur Distributors".to_owned(),
                    phone: None,
                    bin: None,
                    active: true,
                },
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A quantity and a cost whose product lands on half a poisha: 1.5 at
        // 43.33 is 64.995, and both stores have to round it the same way or a
        // shop gets two answers to one question.
        let (status, _) = post_to::<_, ReceiveGoodsResponse>(
            app.clone(),
            "/v1/back-office/stock/receive",
            &ReceiveGoodsRequest {
                protocol: PROTOCOL_VERSION,
                id: 5_000,
                supplier_id: Some(distributor),
                reference: Some("CH-1".to_owned()),
                received_at_ms: 1_788_600_000_000,
                note: None,
                lines: vec![ReceiptLineWire {
                    item_id: 1,
                    qty_milli: 1_500,
                    unit_cost_minor: 4_333,
                }],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, SupplierOwingResponse>(
            app.clone(),
            "/v1/back-office/suppliers/owed",
            &SupplierOwingRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let owing = body.expect("a list").owing;
        assert_eq!(owing.len(), 1);
        assert_eq!(owing[0].owed_minor, 6_500, "rounded away from zero");
        assert_eq!(owing[0].name, "Mirpur Distributors");

        // Paid, twice, because the first reply was dropped.
        for expected in [true, false] {
            let (status, body) = post_to::<_, PaySupplierResponse>(
                app.clone(),
                "/v1/back-office/suppliers/payment",
                &PaySupplierRequest {
                    protocol: PROTOCOL_VERSION,
                    id: 6_000,
                    supplier_id: distributor,
                    amount_minor: 6_500,
                    paid_at_ms: 1_788_900_000_000,
                    note: None,
                },
                Some(&owner),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let reply = body.expect("a reply");
            assert_eq!(reply.paid, expected);
            assert_eq!(reply.owed_minor, 0, "settled, both times");
        }

        // And the statement the two sides put side by side, which carries the
        // whole balance rather than the period's, because that is the number
        // they are arguing about.
        let (status, body) = post_to::<_, SupplierStatementResponse>(
            app.clone(),
            "/v1/back-office/suppliers/statement",
            &SupplierStatementRequest {
                protocol: PROTOCOL_VERSION,
                supplier_id: distributor,
                from_ms: 0,
                to_ms: 1_799_999_999_999,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let seen = body.expect("a statement");
        assert_eq!(seen.entries.len(), 2, "goods in and money out");
        assert!(seen.entries[0].delivered);
        assert_eq!(seen.entries[0].amount_minor, 6_500);
        assert!(!seen.entries[1].delivered);
        assert_eq!(seen.owed_minor, 0, "settled, and it says so");

        // A till may not see what the shop owes, or pay anybody.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/suppliers/owed",
            &SupplierOwingRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_day_is_one_question_and_one_answer() {
        use openpos_core::protocol::{DayRequest, DayResponse};

        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        let day = 1_788_600_000_000_u64;

        // A day's trading: three sales, one of them a refund.
        for (id, at_ms, total) in [
            (901_u128, day + 1_000, 49_450_i64),
            (902, day + 2_000, 30_000),
            (903, day + 3_000, -12_000),
        ] {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal: TERMINAL,
                id,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: at_ms,
                total_minor: total,
                payload: vec![],
                quarantine: None,
                stock: vec![],
                vat: vec![],
                overrides: Vec::new(),
                on_account: if id == 902 {
                    vec![crate::repo::AccountCharge {
                        person_key: "karim".to_owned(),
                        person_name: "Karim".to_owned(),
                        amount_minor: 30_000,
                    }]
                } else {
                    vec![]
                },
                refund_of: None,
                cash_minor: 0,
                cost_minor: 0,
                cost_known: false,
            })
            .await
            .unwrap();
        }

        // Somebody paid off an older debt, and a doubled one was struck off.
        for (kind, amount) in [
            (crate::repo::Settlement::Paid, 5_000_i64),
            (crate::repo::Settlement::WrittenOff, 2_000),
        ] {
            repo.take_payment(
                TENANT,
                &crate::repo::AccountPayment {
                    id: 7_000 + u128::try_from(amount).unwrap_or_default(),
                    kind,
                    person_key: "karim".to_owned(),
                    person_name: "Karim".to_owned(),
                    amount_minor: amount,
                    at_ms: day + 4_000,
                    note: Some("rung twice".to_owned()),
                },
            )
            .await
            .unwrap();
        }

        // And the drawer was counted, forty taka short.
        repo.put_shifts(
            TENANT,
            &[crate::repo::ClosedShift {
                id: 800,
                terminal: TERMINAL,
                closed_by: 91,
                closed_by_name: "Rahima".to_owned(),
                opened_at_ms: day,
                closed_at_ms: day + 5_000,
                opening_float_minor: 50_000,
                sales: 3,
                cash_sales_minor: 37_450,
                non_cash_sales_minor: 30_000,
                cash_in_minor: 0,
                cash_out_minor: 0,
                expected_cash_minor: 87_450,
                counted_cash_minor: 83_450,
                variance_minor: -4_000,
            }],
        )
        .await
        .unwrap();

        let app = router(AppState::new(repo));
        let (status, body) = post_to::<_, DayResponse>(
            app.clone(),
            "/v1/back-office/day",
            &DayRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: day,
                to_ms: day + 10_000,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let seen = body.expect("a day");

        assert_eq!(seen.sales, 3);
        assert_eq!(seen.total_minor, 67_450, "refunds carry their own sign");
        assert_eq!(seen.refunds, 1);
        assert_eq!(seen.refunded_minor, -12_000);
        assert_eq!(seen.drawers_counted, 1);
        assert_eq!(seen.expected_cash_minor, 87_450);
        assert_eq!(seen.counted_cash_minor, 83_450);
        assert_eq!(seen.variance_minor, -4_000);
        // Three account numbers, not one. A day that nets to nothing because a
        // write-off cancelled a payment is a day somebody should look at.
        assert_eq!(seen.charged_minor, 30_000);
        assert_eq!(seen.paid_minor, 5_000);
        assert_eq!(seen.written_off_minor, 2_000);
        assert_eq!(seen.tills.len(), 1);
        assert_eq!(seen.tills[0].sales, 3);

        // Yesterday, which none of this belongs to.
        let (_, body) = post_to::<_, DayResponse>(
            app.clone(),
            "/v1/back-office/day",
            &DayRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: day - 100_000,
                to_ms: day - 1,
            },
            Some(&owner),
        )
        .await;
        let quiet = body.expect("a day");
        assert_eq!(quiet.sales, 0);
        assert_eq!(quiet.drawers_counted, 0);
        assert_eq!(quiet.charged_minor, 0);
    }

    #[tokio::test]
    async fn a_shop_writes_down_who_buys_on_account_and_the_tills_are_told() {
        use openpos_core::protocol::{
            CustomerWire, CustomersRequest, CustomersResponse, PutCustomerRequest,
        };

        let (app, owner, till) = app_with_till().await;

        let (status, body) = post_to::<_, CustomersResponse>(
            app.clone(),
            "/v1/back-office/customers",
            &PutCustomerRequest {
                protocol: PROTOCOL_VERSION,
                customer: CustomerWire {
                    id: 21,
                    name: "  Karim, flat 3  ".to_owned(),
                    phone: Some(" 01711000000 ".to_owned()),
                    active: true,
                    bin: None,
                    limit_minor: 0,
                },
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let list = body.expect("the whole list back").customers;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Karim, flat 3", "trimmed where it is written");
        assert_eq!(list[0].phone.as_deref(), Some("01711000000"));

        // And the till reads the same list, because a sale on account is
        // written with the internet down and the name has to be there first.
        let (status, body) = post_to::<_, CustomersResponse>(
            app.clone(),
            "/v1/customers",
            &CustomersRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("a list").customers.len(), 1);

        // Nobody nameless, and nobody holding the id that means nobody.
        for wrong in [
            CustomerWire {
                id: 0,
                name: "Nobody".to_owned(),
                phone: None,
                active: true,
                bin: None,
                limit_minor: 0,
            },
            CustomerWire {
                id: 22,
                name: "   ".to_owned(),
                phone: None,
                active: true,
                bin: None,
                limit_minor: 0,
            },
        ] {
            let (status, _) = post_to::<_, ProtocolError>(
                app.clone(),
                "/v1/back-office/customers",
                &PutCustomerRequest {
                    protocol: PROTOCOL_VERSION,
                    customer: wrong,
                },
                Some(&owner),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        // A till may read who buys on account and may not decide it.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/customers",
            &PutCustomerRequest {
                protocol: PROTOCOL_VERSION,
                customer: CustomerWire {
                    id: 23,
                    name: "Somebody the till invented".to_owned(),
                    phone: None,
                    active: true,
                    bin: None,
                    limit_minor: 0,
                },
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn an_owner_can_see_which_tills_still_have_a_drawer_open() {
        use openpos_core::protocol::{
            OpenDrawersRequest, OpenDrawersResponse, ReportDrawerRequest, ReportDrawerResponse,
        };

        let (app, owner, till) = app_with_till().await;

        // A till says what it is holding while the drawer is still open. Before
        // this, the shop heard about a drawer only when somebody closed it, so
        // one left open overnight was invisible until the morning.
        let report = ReportDrawerRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            shift: 800,
            opened_at_ms: 1_788_600_000_000,
            at_ms: 1_788_620_000_000,
            opening_float_minor: 50_000,
            sales: 12,
            cash_sales_minor: 74_500,
            non_cash_sales_minor: 10_000,
            cash_in_minor: 0,
            cash_out_minor: 20_000,
            expected_cash_minor: 104_500,
        };
        let (status, _) = post_to::<_, ReportDrawerResponse>(
            app.clone(),
            "/v1/sync/drawer",
            &report,
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Said again twenty minutes later, with more in it. This is a position
        // rather than a history: one till, one open drawer.
        let (status, _) = post_to::<_, ReportDrawerResponse>(
            app.clone(),
            "/v1/sync/drawer",
            &ReportDrawerRequest {
                at_ms: 1_788_621_200_000,
                sales: 15,
                cash_sales_minor: 90_000,
                expected_cash_minor: 120_000,
                ..report.clone()
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_to::<_, OpenDrawersResponse>(
            app.clone(),
            "/v1/back-office/drawers",
            &OpenDrawersRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let open = body.expect("a list").drawers;
        assert_eq!(open.len(), 1, "one till, one open drawer");
        assert_eq!(open[0].expected_cash_minor, 120_000, "the later figure");
        assert_eq!(open[0].sales, 15);
        assert_eq!(
            open[0].reported_at_ms, 1_788_621_200_000,
            "and how stale it is, which is what an owner is judging"
        );

        // Then somebody counts it and closes it. An open list that still shows
        // a drawer counted an hour ago is a list an owner learns to ignore.
        let (status, _) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: vec![ClosedShiftWire {
                    id: 800,
                    terminal: TERMINAL,
                    closed_by: 91,
                    closed_by_name: "Rahima".to_owned(),
                    opened_at_ms: 1_788_600_000_000,
                    closed_at_ms: 1_788_640_000_000,
                    opening_float_minor: 50_000,
                    sales: 15,
                    cash_sales_minor: 90_000,
                    non_cash_sales_minor: 10_000,
                    cash_in_minor: 0,
                    cash_out_minor: 20_000,
                    expected_cash_minor: 120_000,
                    counted_cash_minor: 119_000,
                    variance_minor: -1_000,
                    expected_from_sales_minor: None,
                    struck_out_cash_minor: None,
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = post_to::<_, OpenDrawersResponse>(
            app.clone(),
            "/v1/back-office/drawers",
            &OpenDrawersRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&owner),
        )
        .await;
        assert!(body.expect("a list").drawers.is_empty(), "closed is closed");

        // A till may say what its own drawer holds and may not read the shop's.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/drawers",
            &OpenDrawersRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn deliveries_come_back_newest_first_with_what_was_on_them() {
        let (app, owner, till) = app_with_till().await;

        for (id, at_ms, reference) in [
            (701_u128, 1_788_600_000_000_u64, "CH-1"),
            (702, 1_788_700_000_000, "CH-2"),
        ] {
            let (status, _) = post_to::<_, ReceiveGoodsResponse>(
                app.clone(),
                "/v1/back-office/stock/receive",
                &ReceiveGoodsRequest {
                    protocol: PROTOCOL_VERSION,
                    id,
                    supplier_id: Some(55),
                    reference: Some(reference.to_owned()),
                    received_at_ms: at_ms,
                    note: None,
                    lines: vec![ReceiptLineWire {
                        item_id: 1,
                        qty_milli: 12_000,
                        unit_cost_minor: 38_000,
                    }],
                },
                Some(&owner),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        let (status, body) = post_to::<_, DeliveriesResponse>(
            app.clone(),
            "/v1/back-office/deliveries",
            &DeliveriesRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a list").deliveries;

        // Newest first: the question a shop asks is what came in this week.
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, 702);
        assert_eq!(found[1].id, 701);
        // And the challan number and the goods together, which is the whole
        // point of filing a delivery: the invoice and the shelf side by side.
        assert_eq!(found[0].reference.as_deref(), Some("CH-2"));
        assert_eq!(found[0].supplier_id, Some(55));
        assert_eq!(found[0].lines.len(), 1);
        assert_eq!(found[0].lines[0].qty_milli, 12_000);
        assert_eq!(found[0].lines[0].unit_cost_minor, 38_000);

        // A till may not read what the shop bought or what it paid.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/deliveries",
            &DeliveriesRequest {
                protocol: PROTOCOL_VERSION,
                limit: 20,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn what_the_shop_holds_moves_when_goods_are_sold_and_when_they_arrive() {
        let (app, owner, till) = app_with_till().await;

        let (status, _) = post_to::<_, CatalogueEditResponse>(
            app.clone(),
            "/v1/back-office/catalogue/upsert",
            &UpsertItemRequest {
                expected_seq: 0,
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                item: item(1),
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let ask = |token: String| {
            let app = app.clone();
            async move {
                let (status, body) = post_to::<_, OnHandResponse>(
                    app,
                    "/v1/back-office/stock/on-hand",
                    &OnHandRequest {
                        protocol: PROTOCOL_VERSION,
                        item_ids: vec![1],
                    },
                    Some(&token),
                )
                .await;
                (status, body)
            }
        };

        // Nothing has moved, and the catalogue's own figure is not the answer:
        // that one is whatever it was when somebody last edited the item.
        let (status, body) = ask(owner.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("a figure").on_hand[0].qty_milli, 0);

        // Goods arrive.
        let (status, _) = post_to::<_, ReceiveGoodsResponse>(
            app.clone(),
            "/v1/back-office/stock/receive",
            &ReceiveGoodsRequest {
                protocol: PROTOCOL_VERSION,
                id: 700,
                supplier_id: None,
                reference: Some("CH-1".to_owned()),
                received_at_ms: 1_788_600_000_000,
                note: None,
                lines: vec![ReceiptLineWire {
                    item_id: 1,
                    qty_milli: 24_000,
                    unit_cost_minor: 38_000,
                }],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = ask(owner.clone()).await;
        assert_eq!(
            body.expect("a figure").on_hand[0].qty_milli,
            24_000,
            "a delivery is the only thing that puts stock up"
        );

        // And a till may not read it: what the shop holds is the owner's
        // business, and a route that forgets the check is how a till ends up
        // able to read the shop.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/stock/on-hand",
            &OnHandRequest {
                protocol: PROTOCOL_VERSION,
                item_ids: vec![1],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn nobody_may_be_given_the_id_that_means_nobody() {
        let (app, owner, _till) = app_with_till().await;

        // Zero is what a drawer counted by an older till carries against the
        // person who counted it. Somebody holding that id would make every one
        // of those drawers look like theirs.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/operators",
            &PutOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator: OperatorWire {
                    id: 0,
                    name: "Nobody".to_owned(),
                    pin_salt: vec![7; 16],
                    pin_rounds: 1_000,
                    pin_key: vec![9; 32],
                    max_discount_bp: 0,
                    may_override_price: false,
                    may_refund: false,
                    may_void_line: false,
                    may_authorise: false,
                    may_open_drawer: true,
                    may_close_shift: true,
                    active: true,
                },
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn somebody_can_be_changed_without_anybody_knowing_their_pin() {
        let (app, owner, _till) = app_with_till().await;

        let person = 4_242_u128;
        let (status, _) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators",
            &PutOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator: OperatorWire {
                    id: person,
                    name: "Rina".to_owned(),
                    pin_salt: vec![7; 16],
                    pin_rounds: 1_000,
                    pin_key: vec![9; 32],
                    max_discount_bp: 0,
                    may_override_price: false,
                    may_refund: false,
                    may_void_line: false,
                    may_authorise: false,
                    may_open_drawer: true,
                    may_close_shift: false,
                    active: true,
                },
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // The upsert carries the whole person including the derived key, and an
        // owner suspending somebody does not have it: a PIN is hashed where it
        // is set and never travels. This route carries no PIN at all.
        let (status, body) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators/amend",
            &AmendOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                name: "Rina".to_owned(),
                max_discount_bp: 0,
                may_override_price: false,
                may_refund: false,
                may_void_line: false,
                may_authorise: false,
                may_open_drawer: true,
                may_close_shift: false,
                active: false,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let people = body.expect("the list comes back");
        let rina = people
            .operators
            .iter()
            .find(|who| who.id == person)
            .expect("still there");
        assert!(!rina.active);
        // Suspended, not deleted: their name still has to resolve on the sales
        // they rang last week.
        assert_eq!(rina.name, "Rina");
        assert_eq!(rina.pin_key, vec![9; 32], "and their PIN is untouched");
        assert_eq!(rina.pin_salt, vec![7; 16]);
        assert_eq!(rina.pin_rounds, 1_000);

        // And back in again.
        let (status, body) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators/amend",
            &AmendOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                name: "Rina".to_owned(),
                max_discount_bp: 0,
                may_override_price: false,
                may_refund: false,
                may_void_line: false,
                may_authorise: false,
                may_open_drawer: true,
                may_close_shift: false,
                active: true,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.expect("a list")
                .operators
                .iter()
                .any(|who| who.id == person && who.active)
        );
    }

    #[tokio::test]
    async fn a_new_pin_replaces_the_old_one_and_touches_nothing_else() {
        let (app, owner, till) = app_with_till().await;

        let person = 4_244_u128;
        let (status, _) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators",
            &PutOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator: OperatorWire {
                    id: person,
                    name: "Rina".to_owned(),
                    pin_salt: vec![7; 16],
                    pin_rounds: 1_000,
                    pin_key: vec![9; 32],
                    max_discount_bp: 2_000,
                    may_override_price: true,
                    may_refund: true,
                    may_void_line: false,
                    may_authorise: false,
                    may_open_drawer: true,
                    may_close_shift: false,
                    active: true,
                },
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A forgotten PIN. It cannot be read back from anywhere, which is the
        // point of hashing it on the device that set it, so the only cure is to
        // replace it - and replacing it must not disturb anything else.
        let (status, body) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators/pin",
            &SetOperatorPinRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                pin_salt: vec![3; 16],
                pin_rounds: 120_000,
                pin_key: vec![4; 32],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let rina = body
            .expect("the list comes back")
            .operators
            .into_iter()
            .find(|who| who.id == person)
            .expect("still there");
        assert_eq!(rina.pin_key, vec![4; 32]);
        assert_eq!(rina.pin_salt, vec![3; 16], "a fresh salt, not the old one");
        assert_eq!(rina.pin_rounds, 120_000);
        assert_eq!(rina.name, "Rina", "and nothing else moved");
        assert_eq!(rina.max_discount_bp, 2_000);
        assert!(rina.may_override_price && rina.may_refund && !rina.may_authorise);

        // A round count that would make the hash cheap is refused. It would be
        // written once and trusted for years, and nobody would look at it again.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/operators/pin",
            &SetOperatorPinRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                pin_salt: vec![3; 16],
                pin_rounds: 1,
                pin_key: vec![4; 32],
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // And a till cannot give anybody a new PIN, least of all itself.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/operators/pin",
            &SetOperatorPinRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                pin_salt: vec![3; 16],
                pin_rounds: 120_000,
                pin_key: vec![4; 32],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn correcting_a_name_carries_what_that_person_may_do() {
        let (app, owner, _till) = app_with_till().await;

        let person = 4_243_u128;
        let supervisor = OperatorWire {
            id: person,
            name: "Rina".to_owned(),
            pin_salt: vec![7; 16],
            pin_rounds: 1_000,
            pin_key: vec![9; 32],
            max_discount_bp: 2_000,
            may_override_price: true,
            may_refund: true,
            may_void_line: true,
            may_authorise: true,
            may_open_drawer: true,
            may_close_shift: true,
            active: true,
        };
        let (status, _) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators",
            &PutOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator: supervisor.clone(),
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A spelling correction, nothing more. The request carries the whole
        // person short of their PIN, so a screen that sent defaults for the
        // permissions would take the drawer, the refunds and the discount
        // ceiling away from a supervisor whose name was tidied.
        let (status, body) = post_to::<_, OperatorsResponse>(
            app,
            "/v1/back-office/operators/amend",
            &AmendOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                name: "Rina Akter".to_owned(),
                max_discount_bp: supervisor.max_discount_bp,
                may_override_price: supervisor.may_override_price,
                may_refund: supervisor.may_refund,
                may_void_line: supervisor.may_void_line,
                may_authorise: supervisor.may_authorise,
                may_open_drawer: supervisor.may_open_drawer,
                may_close_shift: supervisor.may_close_shift,
                active: supervisor.active,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let people = body.expect("the list comes back");
        let rina = people
            .operators
            .iter()
            .find(|who| who.id == person)
            .expect("still there");
        assert_eq!(rina.name, "Rina Akter");
        assert_eq!(rina.max_discount_bp, 2_000);
        assert!(rina.may_refund && rina.may_authorise && rina.may_close_shift);
        assert_eq!(rina.pin_key, vec![9; 32], "and still their own PIN");
    }

    #[tokio::test]
    async fn changing_somebody_who_is_not_there_is_refused_rather_than_ignored() {
        let (app, owner, till) = app_with_till().await;

        // An owner who suspends the wrong person and is told it worked has been
        // told a lie about who can open the drawer.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/operators/amend",
            &AmendOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: 999_999,
                name: "Rina".to_owned(),
                max_discount_bp: 0,
                may_override_price: false,
                may_refund: false,
                may_void_line: false,
                may_authorise: false,
                may_open_drawer: true,
                may_close_shift: false,
                active: false,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // And a till cannot take the drawer away from anybody.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/operators/amend",
            &AmendOperatorRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: 999_999,
                name: "Rina".to_owned(),
                max_discount_bp: 0,
                may_override_price: false,
                may_refund: false,
                may_void_line: false,
                may_authorise: false,
                may_open_drawer: true,
                may_close_shift: false,
                active: false,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_code_cannot_be_left_standing_for_a_week() {
        let (app, owner, _till) = app_with_till().await;

        let (status, body) = post_to::<_, IssueCodeResponse>(
            app,
            "/v1/back-office/enrolment-codes",
            &IssueCodeRequest {
                protocol: PROTOCOL_VERSION,
                terminal_id: 502,
                label: "patient".to_owned(),
                role: Role::Till.as_i16(),
                valid_for_seconds: 7 * 24 * 60 * 60,
            },
            Some(&owner),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        // Forty bits is fine for minutes and thin for a week, and a code that
        // outlives the conversation is a credential lying around.
        assert_eq!(body.expect("a code").expires_in_seconds, 3_600);
    }

    #[tokio::test]
    async fn a_correction_needs_an_owner_and_a_reason() {
        let (app, owner, till) = app_with_till().await;

        let breakage = CorrectStockRequest {
            protocol: PROTOCOL_VERSION,
            id: 900,
            item_id: 2,
            qty_milli: -5_000,
            reason: "five broken in the crate".to_owned(),
            occurred_at_ms: 2_000,
        };

        // A till may not write stock off. Losses a cashier can record without
        // anybody's knowledge are not losses anybody investigates.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/stock/correct",
            &breakage,
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body) = post_to::<_, CorrectStockResponse>(
            app.clone(),
            "/v1/back-office/stock/correct",
            &breakage,
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.expect("a reply").recorded);

        // A retry is recognised rather than writing the loss off twice.
        let (status, body) = post_to::<_, CorrectStockResponse>(
            app,
            "/v1/back-office/stock/correct",
            &breakage,
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.expect("a reply").recorded);
    }
}
