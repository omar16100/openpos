//! The catalogue and the shelves: what the shop sells, what it holds, what
//! came in and who it came from.


use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use openpos_core::protocol::{
    CatalogueEditResponse, CorrectStockRequest,
    CorrectStockResponse,
    DeleteItemRequest, DeliveredLineWire, DeliveriesRequest, DeliveriesResponse, DeliveriesResponseV7,
    DeliveryWire, ItemNowRequest, ItemNowResponse, ItemWire, OnHandEntry, OnHandRequest, OnHandResponse, ProtocolError, PutSupplierRequest,
    ReceiveGoodsRequest, ReceiveGoodsResponse, RecordCountRequest, RecordCountResponse, SupplierWire, SuppliersRequest,
    SuppliersResponse, TillItemsRequest, TillItemsResponse,
    ResendCatalogueRequest, ResendCatalogueResponse, UnreadableChangeWire,
    UnreadableChangesRequest, UnreadableChangesResponse,
    UpsertItemRequest,
};

use crate::http::{
    AppState, authenticate, decode, encoded, owner_from,
    protocol_error, require_owner, unavailable,
};
use crate::repo::{
    GoodsReceipt, ReceiptLine, RepoError, Repository,
    StockCorrection, StockCount, Supplier,
};

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
pub(crate) async fn items_from_tills<R: Repository>(
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

pub(crate) async fn unreadable_changes<R: Repository>(
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
pub(crate) async fn resend_catalogue<R: Repository>(
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

/// What has been delivered lately. Owner only.
///
/// A delivery filed under a supplier is only worth filing if somebody can ask
/// which goods came on which challan, and that is the question asked when the
/// invoice and the shelf disagree.
pub(crate) async fn deliveries<R: Repository>(
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
        Ok(found) => {
            let deliveries: Vec<DeliveryWire> = found
                .into_iter()
                .map(|receipt| {
                    let lines: Vec<DeliveredLineWire> = receipt
                        .lines
                        .into_iter()
                        .map(|line| DeliveredLineWire {
                            item_id: line.item_id,
                            qty_milli: line.qty_milli,
                            unit_cost_minor: line.unit_cost_minor,
                        })
                        .collect();
                    DeliveryWire {
                        id: receipt.id,
                        supplier_id: receipt.supplier_id,
                        reference: receipt.reference,
                        received_at_ms: receipt.received_at_ms,
                        cost_minor: super::money::what_it_cost(&lines),
                        lines,
                    }
                })
                .collect();
            // Older screens are answered on the shape they can read. The total
            // is new here, and postcard is positional: a v7 reader handed a v8
            // body reads the total as the beginning of the next delivery.
            if protocol < 8 {
                return encoded(&DeliveriesResponseV7 {
                    protocol,
                    deliveries: deliveries.into_iter().map(Into::into).collect(),
                });
            }
            encoded(&DeliveriesResponse {
                protocol,
                deliveries,
            })
        }
        Err(_) => unavailable(),
    }
}

/// What the shop believes it holds. Owner only.
///
/// Its own question rather than a field on the catalogue, because a sale is not
/// a catalogue change and must not bump the catalogue cursor: doing that would
/// make every till re-pull every item every time anything sold.
pub(crate) async fn on_hand<R: Repository>(
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

/// Add or update a supplier.
pub(crate) async fn put_supplier<R: Repository>(
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
pub(crate) async fn suppliers<R: Repository>(
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
pub(crate) async fn receive_goods<R: Repository>(
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
pub(crate) async fn correct_stock<R: Repository>(
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
pub(crate) async fn record_count<R: Repository>(
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

/// Create or replace one item, which tills pick up on their next pull.
/// One item as the shop holds it now. Owner only.
///
/// Read before an edit, so a correction is built on what the shop has rather
/// than on a device's copy of the catalogue, which is up to half a minute
/// behind. The sequence comes with it and goes back with the save.
pub(crate) async fn item_now<R: Repository>(
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
pub(crate) async fn upsert_item<R: Repository>(
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
pub(crate) fn priceable(item: &ItemWire) -> Result<(), ProtocolError> {
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
pub(crate) fn priced_for(protocol: u16, refusal: ProtocolError) -> ProtocolError {
    if protocol >= 4 {
        return refusal;
    }
    ProtocolError::NotAPrice {
        said: format!("{refusal}"),
    }
}

pub(crate) async fn delete_item<R: Repository>(
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

    use super::super::proof::sale_payload;

    
    use axum::http::StatusCode;
    // Every shape that travels, because these tests ask the routes the way a
    // device does and a device's request is one of them. A glob rather than a
    // list: the list was what `use super::*` used to hand over, and keeping it
    // by hand is a line to edit every time a route gains a shape.
    use openpos_core::protocol::*;
    

    use crate::auth::Role;
    
    // The trait the memory store answers through, which `use super::*` used to
    // bring in with everything else.
    // These tests reach the back office through the router, as a device does.
    use crate::http::{AppState, router};
    // The setup a till's own routes already needed. Shared rather than copied:
    // two of these would drift, and the one used least would be the one wrong.
    use crate::http::tests::{
        TENANT, TERMINAL, app, app_with_till, item, post_to,
    };
    use crate::repo::MemoryRepo;

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
        // And what it cost in all, added up here rather than on the screen:
        // twelve at 380.00 is 4,560.00.
        assert_eq!(found[0].cost_minor, Some(456_000));

        // A screen a release behind is answered on the shape it can read. These
        // bodies are positional, so a v7 reader handed the total would take it
        // as the start of the next delivery and show the shop nonsense.
        let (status, older) = post_to::<_, DeliveriesResponseV7>(
            app.clone(),
            "/v1/back-office/deliveries",
            &DeliveriesRequest {
                protocol: 7,
                limit: 20,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let older = older.expect("a list").deliveries;
        assert_eq!(older.len(), 2);
        assert_eq!(older[0].id, 702);
        assert_eq!(older[0].lines[0].unit_cost_minor, 38_000);

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
