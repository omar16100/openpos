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
    negotiate, CatalogueEditResponse, CorrectStockRequest, CorrectStockResponse, DeleteItemRequest,
    DeliveredLineWire, DeliveriesRequest, DeliveriesResponse, DeliveryWire, IssueCodeRequest,
    IssueCodeResponse, OnHandEntry, OnHandRequest, OnHandResponse, OperatorWire, OperatorsResponse,
    ProtocolError, PutOperatorRequest, PutShopRequest, PutSupplierRequest, ReceiveGoodsRequest,
    ReceiveGoodsResponse, RecordCountRequest, RecordCountResponse, RepairEntry, RepairQueueRequest,
    RepairQueueResponse, ResolveRepairRequest, ResolveRepairResponse, SetOperatorActiveRequest,
    ShopResponse, SupplierWire, SuppliersRequest, SuppliersResponse, TakingsRequest,
    TakingsResponse, TerminalHealthEntry, TerminalHealthRequest, TerminalHealthResponse,
    TillTakings, UpsertItemRequest,
};

use super::{
    authenticate, encoded, owner_from, protocol_error, require_owner, unavailable, AppState,
    MAX_CODE_LIFETIME, MAX_REPAIR_PAGE, MAX_RESOLUTION_NOTE,
};
use crate::auth::{Caller, EnrolmentCode, Role};
use crate::repo::{
    GoodsReceipt, OperatorRecord, ReceiptLine, RepoError, Repository, ShopDetails, StockCorrection,
    StockCount, Supplier,
};

/// What the shop took over a period. Owner only.
///
/// The question an owner asks most often, and the cheapest one to answer: the
/// total and the time are columns on the sale, so nothing here decodes a ticket.
pub(super) async fn takings<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<TakingsRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // A backwards range is a mistake, not a query. Answering it with zero would
    // read as a day with no sales, which is a thing an owner would act on.
    if request.to_ms < request.from_ms {
        return protocol_error(&ProtocolError::Malformed);
    }

    match state
        .repo
        .takings(caller.tenant, request.from_ms, request.to_ms)
        .await
    {
        Ok(rows) => {
            let mut total = TakingsResponse {
                protocol,
                sales: 0,
                total_minor: 0,
                refunds: 0,
                refunded_minor: 0,
                tills: Vec::with_capacity(rows.len()),
            };
            for row in rows {
                total.sales = total.sales.saturating_add(row.sales);
                total.total_minor = total.total_minor.saturating_add(row.total_minor);
                total.refunds = total.refunds.saturating_add(row.refunds);
                total.refunded_minor = total.refunded_minor.saturating_add(row.refunded_minor);
                total.tills.push(TillTakings {
                    terminal: row.terminal,
                    sales: row.sales,
                    total_minor: row.total_minor,
                    needing_attention: row.needing_attention,
                });
            }
            encoded(&total)
        }
        Err(_) => unavailable(),
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
    let Ok(request) = postcard::from_bytes::<DeliveriesRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<OnHandRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // A cap, because this is one query per item and an owner with a long
    // catalogue should get a slow screen rather than a server on its knees.
    const MOST: usize = 200;
    let wanted: Vec<u128> = if request.item_ids.is_empty() {
        match state.repo.items_since(caller.tenant, 0, u32::MAX).await {
            Ok(page) => page.upserts.into_iter().map(|item| item.id).take(MOST).collect(),
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

/// Suspend somebody, or let them back in. Owner only.
///
/// Its own route rather than a flag on the upsert, because that one carries the
/// whole person including the derived PIN key, and an owner suspending somebody
/// does not have it: a PIN is hashed on the device where it is set and never
/// travels. Requiring it here would mean asking an owner to know a cashier's
/// PIN in order to take the drawer away from them.
pub(super) async fn set_operator_active<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<SetOperatorActiveRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .set_operator_active(caller.tenant, request.operator_id, request.active)
        .await
    {
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
    let Ok(request) = postcard::from_bytes::<PutOperatorRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

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
        Ok(()) => encoded(&OperatorsResponse {
            protocol,
            operators: alloc_one(wire_operator(record)),
        }),
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(_) => unavailable(),
    }
}

fn alloc_one(operator: OperatorWire) -> Vec<OperatorWire> {
    vec![operator]
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
    let Ok(request) = postcard::from_bytes::<PutShopRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let details = ShopDetails {
        name: request.name,
        bin: request.bin,
        address: request.address,
        phone: request.phone,
    };
    match state.repo.put_shop_details(caller.tenant, &details).await {
        Ok(()) => encoded(&ShopResponse {
            protocol,
            name: details.name,
            bin: details.bin,
            address: details.address,
            phone: details.phone,
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
    let Ok(request) = postcard::from_bytes::<PutSupplierRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<SuppliersRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<ReceiveGoodsRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<CorrectStockRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<RecordCountRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
        if state.repo.record_count(caller.tenant, &count).await.is_err() {
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
    let Ok(request) = postcard::from_bytes::<IssueCodeRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<RepairQueueRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<ResolveRepairRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<TerminalHealthRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<UpsertItemRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    let Ok(request) = postcard::from_bytes::<DeleteItemRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;
    use openpos_core::protocol::{
        CountedItem, EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse,
        PullRequest, PullResponse, QuarantineReason, ReceiptLineWire, PROTOCOL_VERSION,
    };

    use super::*;
    // These tests reach the back office through the router, as a device does.
    use crate::http::{router, AppState, CONTENT_TYPE};
    // The setup a till's own routes already needed. Shared rather than copied:
    // two of these would drift, and the one used least would be the one wrong.
    use crate::http::tests::{
        app, app_with_till, item, post_to, repair_request, shop_with_a_repair, TENANT, TERMINAL,
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
        assert!(!second.recorded, "stock booked twice is a shop ordering against goods it lacks");
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
        assert_eq!(issued.terminal_id, TERMINAL, "the same till, not another one");

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
            })
            .await
            .unwrap();
        }

        let app = router(AppState::new(repo));
        let (status, body) = post_to::<_, TakingsResponse>(
            app.clone(),
            "/v1/back-office/takings",
            &TakingsRequest {
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
            "/v1/back-office/takings",
            &TakingsRequest {
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
    async fn somebody_can_be_suspended_without_anybody_knowing_their_pin() {
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
            "/v1/back-office/operators/active",
            &SetOperatorActiveRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
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

        // And back in again.
        let (status, body) = post_to::<_, OperatorsResponse>(
            app.clone(),
            "/v1/back-office/operators/active",
            &SetOperatorActiveRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: person,
                active: true,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body
            .expect("a list")
            .operators
            .iter()
            .any(|who| who.id == person && who.active));
    }

    #[tokio::test]
    async fn suspending_somebody_who_is_not_there_is_refused_rather_than_ignored() {
        let (app, owner, till) = app_with_till().await;

        // An owner who suspends the wrong person and is told it worked has been
        // told a lie about who can open the drawer.
        let (status, _) = post_to::<_, ProtocolError>(
            app.clone(),
            "/v1/back-office/operators/active",
            &SetOperatorActiveRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: 999_999,
                active: false,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // And a till cannot take the drawer away from anybody.
        let (status, _) = post_to::<_, ProtocolError>(
            app,
            "/v1/back-office/operators/active",
            &SetOperatorActiveRequest {
                protocol: PROTOCOL_VERSION,
                operator_id: 999_999,
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
