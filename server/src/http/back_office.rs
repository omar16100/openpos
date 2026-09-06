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
    AccountEntryWire, AccountRequest, AccountResponse, AdoptSalesRequest, AmendOperatorRequest,
    CatalogueEditResponse, ClosedShiftWire, ClosedShiftWireV1, CorrectStockRequest,
    CorrectStockResponse, CustomerWire, CustomersResponse, DayRequest, DayResponse,
    DeleteItemRequest, DeliveredLineWire, DeliveriesRequest, DeliveriesResponse, DeliveryWire,
    IssueCodeRequest, IssueCodeResponse, OnHandEntry, OnHandRequest, OnHandResponse,
    OpenDrawerWire, OpenDrawersRequest, OpenDrawersResponse, OperatorWire, OperatorsResponse,
    OwedRequest, OwedResponse, OwingWire, PaySupplierRequest, PaySupplierResponse, ProtocolError,
    PutCustomerRequest, PutOperatorRequest, PutShopRequest, PutSupplierRequest,
    ReceiveGoodsRequest, ReceiveGoodsResponse, RecordCountRequest, RecordCountResponse,
    RepairEntry, RepairQueueRequest, RepairQueueResponse, ResolveRepairRequest,
    ResolveRepairResponse, RevokeTerminalRequest, RevokeTerminalResponse, SetOperatorPinRequest,
    ShiftsRequest, ShiftsResponse, ShiftsResponseV1, ShopResponse, SoldRequest, SoldResponse,
    SoldWire, SupplierEntryWire, SupplierOwingRequest, SupplierOwingResponse, SupplierOwingWire,
    SupplierStatementRequest, SupplierStatementResponse, SupplierWire, SuppliersRequest,
    SuppliersResponse, TakePaymentRequest, TakePaymentResponse, TerminalHealthEntry,
    TerminalHealthRequest, TerminalHealthResponse, TillTakings, UnreadableChangeWire,
    UnreadableChangesRequest, UnreadableChangesResponse, UpsertItemRequest, VatRequest,
    VatResponse, VatRowWire,
};

use super::{
    AppState, MAX_CODE_LIFETIME, MAX_REPAIR_PAGE, MAX_RESOLUTION_NOTE, authenticate, decode,
    encoded, owner_from, protocol_error, require_owner, unavailable,
};
use crate::auth::{Caller, EnrolmentCode, Role};
use crate::repo::{
    GoodsReceipt, OperatorRecord, ReceiptLine, RepoError, Repository, ShopDetails, StockCorrection,
    StockCount, Supplier,
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
        Ok(withdrawn) => encoded(&RevokeTerminalResponse {
            protocol,
            withdrawn: u32::try_from(withdrawn).unwrap_or(u32::MAX),
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
        Ok(rows) => encoded(&VatResponse {
            protocol,
            rows: rows
                .into_iter()
                .map(|row| VatRowWire {
                    vat_bp: row.vat_bp,
                    net_minor: row.net_minor,
                    vat_minor: row.vat_minor,
                    sales: row.sales,
                })
                .collect(),
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
    // is what it could show anyway. Sending the newer shape would not be read
    // as a missing field; it would be read as different numbers.
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
                        counted_cash_minor: shift.counted_cash_minor,
                        variance_minor: shift.variance_minor,
                    })
                })
                .collect(),
        });
    }

    {
        encoded(&ShiftsResponse {
            protocol,
            shifts: found
                .into_iter()
                .map(|shift| ClosedShiftWire {
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
        .owed(caller.tenant, request.limit.clamp(1, 500))
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
            request.limit.clamp(1, 200),
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

    match crate::ingest::adopt(state.repo.as_ref(), caller.tenant, &request).await {
        Ok(response) => encoded(&response),
        Err(crate::ingest::IngestError::Protocol(error)) => protocol_error(&error),
        // The device keeps its copy and the shop tries again. Telling it
        // otherwise would let somebody wipe the only record of a day's trading.
        Err(crate::ingest::IngestError::Storage) => unavailable(),
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
    let wanted: Vec<u128> = if request.item_ids.is_empty() {
        match state.repo.items_since(caller.tenant, 0, u32::MAX).await {
            Ok(page) => page
                .upserts
                .into_iter()
                .map(|item| item.id)
                .take(MOST)
                .collect(),
            Err(_) => return unavailable(),
        }
    } else {
        request.item_ids.into_iter().take(MOST).collect()
    };

    let mut figures = Vec::with_capacity(wanted.len());
    for item in wanted {
        match state.repo.on_hand(caller.tenant, item).await {
            Ok(entry) => figures.push(OnHandEntry {
                item_id: entry.item_id,
                qty_milli: entry.qty_milli,
                counted_at_ms: entry.counted_at_ms,
                unreconciled_milli: entry.unreconciled_milli,
                // Saturating rather than wrapping: a shop with four billion
                // late sales on one item has a bigger problem than a count, and
                // wrapping would report it as none.
                unreconciled_sales: u32::try_from(entry.unreconciled_sales).unwrap_or(u32::MAX),
            }),
            Err(_) => return unavailable(),
        }
    }

    encoded(&OnHandResponse {
        protocol,
        on_hand: figures,
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
    };
    match state.repo.put_shop_details(caller.tenant, &details).await {
        Ok(()) => encoded(&ShopResponse {
            protocol,
            name: details.name,
            bin: details.bin,
            address: details.address,
            phone: details.phone,
            wallets: details.wallets,
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

    // The figures are read back whether or not this call wrote anything. A
    // retry that is told "already booked" still needs to know where stock
    // stands, or the only way to find out is to guess.
    let mut on_hand = Vec::with_capacity(receipt.lines.len());
    for line in &receipt.lines {
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
        Ok(queue) => encoded(&RepairQueueResponse {
            protocol,
            entries: queue
                .into_iter()
                .map(|item| RepairEntry {
                    id: item.id,
                    receipt_no: item.receipt_no,
                    total_minor: item.total_minor,
                    received_at_ms: item.received_at_ms,
                    reason: item.reason,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Take one sale out of the queue.
pub(super) async fn resolve_repair<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ResolveRepairRequest>(&body) {
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
    if request.note.len() > MAX_RESOLUTION_NOTE {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .resolve_quarantine(caller.tenant, request.sale, &request.note)
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
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Create or replace one item, which tills pick up on their next pull.
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
        // The queue is worked by a person, so the reason has to read as one.
        assert!(
            entry.reason.contains("49450"),
            "the entry must say what disagreed: {}",
            entry.reason
        );
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
            on_account: vec![],
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
                on_account: vec![],
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
                on_account: vec![crate::repo::AccountCharge {
                    person_key: key,
                    person_name: "Karim".to_owned(),
                    amount_minor: amount,
                }],
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
                on_account: vec![],
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
                on_account: if id == 902 {
                    vec![crate::repo::AccountCharge {
                        person_key: "karim".to_owned(),
                        person_name: "Karim".to_owned(),
                        amount_minor: 30_000,
                    }]
                } else {
                    vec![]
                },
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
            },
            CustomerWire {
                id: 22,
                name: "   ".to_owned(),
                phone: None,
                active: true,
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
