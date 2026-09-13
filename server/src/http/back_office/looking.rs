//! Everything that needs somebody to look: the drawers, the sales the shop
//! could not take on trust, the answers already given, and the paper a customer
//! has brought back to the counter.


use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use openpos_core::protocol::{
    AdoptSalesRequest, AllowedEntry,
    AllowedEntryV4, AllowedRequest, AllowedResponse, AllowedResponseV4, ClosedShiftWire, ClosedShiftWireV1,
    DecideAgainRequest, DecideAgainResponse, DecidedEntry, DecidedRequest, DecidedResponse, OpenDrawerWire, OpenDrawersRequest,
    OpenDrawersResponse,
    PaperLineWire, PaperTenderWire, ProtocolError, ReceiptGapWire,
    ReceiptGapsRequest, ReceiptGapsResponse, ReceiptRequest, ReceiptResponse, ReceiptResponseV2,
    RepairEntry, RepairEntryV2, RepairQueueRequest, RepairQueueResponse, RepairQueueResponseV2,
    ResolveRepairRequest, ResolveRepairRequestV1, ResolveRepairResponse, SaleOnPaperWire, SaleOnPaperWireV2,
    ShiftsRequest, ShiftsResponse, ShiftsResponseV1,
};

use crate::http::{
    AppState, MAX_ALLOWED_PAGE,
    MAX_REPAIR_PAGE, MAX_RESOLUTION_NOTE, authenticate, decode, encoded, owner_from,
    protocol_error, require_owner, unavailable,
};
use crate::auth::Caller;
use crate::repo::{
    Decided, Repository,
};

/// Which tills have a drawer open. Owner only.
///
/// The question an owner asks at closing time and could not ask before: a
/// drawer was only ever reported when it closed, so one left open overnight was
/// invisible until somebody noticed the till in the morning.
pub(crate) async fn open_drawers<R: Repository>(
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
pub(crate) async fn shifts<R: Repository>(
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
                        refunds: 0,
                        refunded_cash_minor: 0,
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
        let mut refunds = Vec::with_capacity(found.len());
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
            // And what came back while the drawer was open. From the shop's own
            // sales like the two above: the drawer's cash figure is already net
            // of these, so without this a drawer short against a day's selling
            // reads the same whether goods came back or not.
            match state
                .repo
                .refunds_in_window(
                    caller.tenant,
                    shift.terminal,
                    shift.opened_at_ms,
                    shift.closed_at_ms,
                )
                .await
            {
                Ok(given_back) => refunds.push(given_back),
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

        // A back office speaking 12 or less reads a drawer without what came
        // back in it, which is what it could show anyway. The shape is
        // positional, so sending the newer one would not read as a missing
        // field: it would read as different numbers.
        if protocol <= 12 {
            return encoded(&openpos_core::protocol::ShiftsResponseV12 {
                protocol,
                shifts: found
                    .into_iter()
                    .zip(from_sales)
                    .zip(struck_out)
                    .map(|((shift, expected_from_sales_minor), struck_out_cash_minor)| {
                        openpos_core::protocol::ClosedShiftWireV12 {
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
                .zip(refunds)
                .map(|(((shift, expected_from_sales_minor), struck_out_cash_minor), given_back)| ClosedShiftWire {
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
                    refunds: given_back.0,
                    refunded_cash_minor: given_back.1,
                })
                .collect(),
        })
    }
}

/// Sales carried in from a device that could not send them. Owner only.
///
/// The only way out for a till whose terminal the shop deleted, or one holding
/// sales that has to be re-enrolled as another. Its outbox is the only record of
/// goods that left the shop, and until this existed there was no route that
/// would take them: the credential that proves where a sale came from is exactly
/// what such a device has lost.
pub(crate) async fn adopt_sales<R: Repository>(
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
pub(crate) async fn receipt<R: Repository>(
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
pub(crate) async fn receipt_for_a_till<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match crate::http::caller_from(&state, &headers).await {
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

pub(crate) async fn repairs<R: Repository>(
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
pub(crate) async fn receipt_gaps<R: Repository>(
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
pub(crate) async fn allowed<R: Repository>(
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
pub(crate) async fn decided<R: Repository>(
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
pub(crate) async fn decide_again<R: Repository>(
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
pub(crate) async fn resolve_repair<R: Repository>(
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

    use super::super::proof::{alloc_broken_payload, sale_payload};

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    // Every shape that travels, because these tests ask the routes the way a
    // device does and a device's request is one of them. A glob rather than a
    // list: the list was what `use super::*` used to hand over, and keeping it
    // by hand is a line to edit every time a route gains a shape.
    use openpos_core::protocol::*;
    use tower::ServiceExt;

    
    use crate::http::MAX_RESOLUTION_NOTE;
    // The trait the memory store answers through, which `use super::*` used to
    // bring in with everything else.
    use crate::repo::Repository;
    // These tests reach the back office through the router, as a device does.
    use crate::http::{AppState, CONTENT_TYPE, router};
    // The setup a till's own routes already needed. Shared rather than copied:
    // two of these would drift, and the one used least would be the one wrong.
    use crate::http::tests::{
        TENANT, TERMINAL, app_with_till, item, post_to, repair_request, shop_with_a_repair,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
            refunds: 0,
            refunded_cash_minor: 0,
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
                    refunds: 0,
                    refunded_cash_minor: 0,
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
            // A sale as a shop stored one before the schema was kept.
            payload_schema: None,
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
                    refunds: 0,
                    refunded_cash_minor: 0,
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

    /// A shop with two counted drawers in it, because one is not enough to
    /// catch a positional shape going wrong. See the test below.
    async fn two_counted_drawers() -> (axum::Router, String) {
        let (app, owner, till) = app_with_till().await;
        let drawers: Vec<ClosedShiftWire> = [(700_u128, 95_450_i64), (701, 100_000)]
            .into_iter()
            .map(|(id, counted)| ClosedShiftWire {
                id,
                terminal: TERMINAL,
                closed_by: 91,
                closed_by_name: "Rahima".to_owned(),
                opened_at_ms: 1_788_600_000_000,
                closed_at_ms: 1_788_640_000_000,
                opening_float_minor: 50_000,
                sales: 3,
                cash_sales_minor: 49_450,
                non_cash_sales_minor: 0,
                cash_in_minor: 0,
                cash_out_minor: 0,
                expected_cash_minor: 99_450,
                counted_cash_minor: counted,
                variance_minor: counted - 99_450,
                refunds: 0,
                refunded_cash_minor: 0,
                expected_from_sales_minor: None,
                struck_out_cash_minor: None,
            })
            .collect();
        let (status, _) = post_to::<_, PushShiftsResponse>(
            app.clone(),
            "/v1/sync/shifts",
            &PushShiftsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                shifts: drawers,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        (app, owner)
    }

    /// A back office a release behind reads a drawer without what came back in
    /// it, which is what it could show anyway.
    ///
    /// Two drawers on purpose, and the reason is the shape rather than the
    /// figures. These bodies are positional, so a field appended to the end of
    /// an entry lands at the end of the body when there is one entry, where a
    /// decoder can shrug it off, and between the entries when there are two,
    /// where everything after it reads as something else. The same test with
    /// one drawer passed with the compatibility branch deleted, which is how
    /// this was learned.
    #[tokio::test]
    async fn a_back_office_a_release_behind_still_reads_its_counted_drawers() {
        let (app, owner) = two_counted_drawers().await;

        let (status, body) = post_to::<_, openpos_core::protocol::ShiftsResponseV12>(
            app,
            "/v1/back-office/shifts",
            &ShiftsRequest {
                protocol: 12,
                limit: 20,
            },
            Some(&owner),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let found = body.expect("a body version 12 can decode").shifts;
        assert_eq!(found.len(), 2, "both drawers, and not one of them twice");
        let mut counted: Vec<i64> = found.iter().map(|one| one.counted_cash_minor).collect();
        counted.sort_unstable();
        assert_eq!(
            counted,
            vec![95_450, 100_000],
            "the second entry is the second drawer, not the first one's tail read as a drawer"
        );
        assert!(
            found.iter().all(|one| one.opening_float_minor == 50_000),
            "and the fields it did know still mean what they meant"
        );
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
                    refunds: 0,
                    refunded_cash_minor: 0,
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
}
