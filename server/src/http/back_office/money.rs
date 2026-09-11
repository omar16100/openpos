//! What the shop took, what it owes and what it is owed.
//!
//! Every figure here is added up where the rest of the money is, in integer
//! poisha, and handed over whole: a screen that sums rows is a second answer to
//! a question the shop has already answered.


use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use openpos_core::protocol::{
    AccountEntryWire, AccountRequest, AccountResponse, DayRequest, DayResponse, DeliveredLineWire, MadeRequest,
    MadeResponse, OwedRequest, OwedResponse, OwingWire, PaySupplierRequest, PaySupplierResponse, ProtocolError, SoldRequest, SoldResponse,
    SoldWire, SupplierEntryWire, SupplierOwingRequest, SupplierOwingResponse, SupplierOwingWire,
    SupplierStatementRequest, SupplierStatementResponse, TakePaymentRequest, TakePaymentResponse, TillTakings, VatRequest, VatResponse, VatResponseV7, VatRowWire, WaivedRequest,
    WaivedResponse,
    WaivedWire,
};

use crate::http::{
    AppState, MAX_ACCOUNT_PAGE, MAX_OWED_PAGE, decode, encoded, owner_from,
    protocol_error, unavailable,
};
use crate::repo::Repository;

/// What supervisors waived over a period. Owner only.
///
/// A cashier's ceiling exists so that giving money away is somebody's decision
/// rather than everybody's habit. That only means anything if the decisions can
/// be looked at afterwards: the reason is on the customer's receipt, and this is
/// the shop's side of the same sentence.
pub(crate) async fn waived<R: Repository>(
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
pub(crate) async fn sold<R: Repository>(
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
pub(crate) async fn supplier_owing<R: Repository>(
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

/// What passed between the shop and one supplier over a period. Owner only.
///
/// The statement two people put side by side when the shop's figure and the
/// distributor's disagree, which is the conversation the whole ledger exists
/// for. Deliveries in and payments out, oldest first.
pub(crate) async fn supplier_statement<R: Repository>(
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
pub(crate) async fn pay_supplier<R: Repository>(
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
pub(crate) async fn vat<R: Repository>(
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
        Ok(summary) => {
            let rows: Vec<VatRowWire> = summary
                .rows
                .into_iter()
                .map(|row| VatRowWire {
                    vat_bp: row.vat_bp,
                    net_minor: row.net_minor,
                    vat_minor: row.vat_minor,
                    sales: row.sales,
                    supply: row.supply,
                })
                .collect();
            let reply = VatResponse {
                protocol,
                vat_minor: what_the_rows_come_to(&rows),
                rows,
                waiting_sales: summary.waiting_sales,
                waiting_vat_minor: summary.waiting_vat_minor,
            };
            // A screen a release behind is answered on the shape it can read.
            if protocol < 8 {
                return encoded(&VatResponseV7::from(reply));
            }
            encoded(&reply)
        }
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
pub(crate) async fn day<R: Repository>(
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

/// Who owes the shop money. Owner only.
///
/// A shop here sells on account all day, and until this existed the till could
/// record that money had been given away and nothing added it up. The book
/// stayed on paper beside the machine that was meant to replace it.
pub(crate) async fn owed<R: Repository>(
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
pub(crate) async fn take_payment<R: Repository>(
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
pub(crate) async fn account<R: Repository>(
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

/// What a month's VAT comes to, in the arithmetic the rest of the money uses.
///
/// A refund carries its own sign and subtracts, which is what a return wants,
/// so this is a signed sum and not an absolute one. Nothing rather than a wrong
/// figure if it will not add up: see the delivery below.
fn what_the_rows_come_to(rows: &[VatRowWire]) -> i64 {
    let mut running = openpos_core::money::Minor::ZERO;
    for row in rows {
        let Ok(next) = running.checked_add(openpos_core::money::Minor::new(row.vat_minor)) else {
            return 0;
        };
        running = next;
    }
    running.get()
}

/// What a delivery cost in all, in the arithmetic the rest of the money uses.
///
/// Nothing rather than a panic in a read-only screen, and nothing rather than a
/// zero: figures this cannot add up are ones no shop has, and a delivery worth
/// nothing and a delivery nobody could add up are different things. The lines
/// are beside it either way.
pub(super) fn what_it_cost(lines: &[DeliveredLineWire]) -> Option<i64> {
    let mut running = openpos_core::money::Minor::ZERO;
    for line in lines {
        let cost = openpos_core::money::Minor::new(line.unit_cost_minor)
            .mul_qty(openpos_core::money::Milli::new(line.qty_milli))
            .ok()?;
        running = running.checked_add(cost).ok()?;
    }
    Some(running.get())
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
pub(crate) async fn made<R: Repository>(
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
    

    
    
    // The trait the memory store answers through, which `use super::*` used to
    // bring in with everything else.
    use crate::repo::Repository;
    // These tests reach the back office through the router, as a device does.
    use crate::http::{AppState, router};
    // The setup a till's own routes already needed. Shared rather than copied:
    // two of these would drift, and the one used least would be the one wrong.
    use crate::http::tests::{
        TENANT, TERMINAL, app_with_till, post_to,
    };
    use crate::repo::{MemoryRepo, StoredSale};

    #[tokio::test]
    async fn a_months_tax_comes_back_added_up_by_the_shop() {
        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();

        let month = 1_788_600_000_000_u64;
        // Two rates and a refund, because a return carries its own sign and
        // subtracts: a total that took the absolute of each row would say a
        // shop owed the revenue for goods it had taken back.
        for (id, at_ms, vat) in [
            (911_u128, month + 1_000, vec![(1_500_u32, 43_000_i64, 6_450_i64, 0_u8)]),
            (912, month + 2_000, vec![(0, 500_000, 0, 1)]),
            (913, month + 3_000, vec![(1_500, -21_500, -3_225, 0)]),
        ] {
            repo.store_sale(StoredSale {
                tenant: TENANT,
                terminal: TERMINAL,
                id,
                receipt_no: None,
                receipt_epoch: None,
                rung_at_ms: at_ms,
                total_minor: 0,
                payload: vec![],
                quarantine: None,
                stock: vec![],
                vat,
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
        let (status, body) = post_to::<_, VatResponse>(
            app.clone(),
            "/v1/back-office/vat",
            &VatRequest {
                protocol: PROTOCOL_VERSION,
                from_ms: month,
                to_ms: month + 86_400_000,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let owed = body.expect("a figure");

        // 64.50 charged less 32.25 given back, and nothing at all on the
        // zero-rated row. Added up here rather than on the screen: this is the
        // figure an owner copies onto a return.
        assert_eq!(owed.vat_minor, 6_450 - 3_225);
        assert_eq!(
            owed.vat_minor,
            owed.rows.iter().map(|row| row.vat_minor).sum::<i64>(),
            "the total is the rows, or one of the two is wrong"
        );

        // And a screen a release behind reads the shape it knows.
        let (status, older) = post_to::<_, VatResponseV7>(
            app,
            "/v1/back-office/vat",
            &VatRequest {
                protocol: 7,
                from_ms: month,
                to_ms: month + 86_400_000,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let older = older.expect("a figure");
        assert_eq!(older.rows.len(), owed.rows.len());
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
}
