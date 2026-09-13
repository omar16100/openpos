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

mod looking;
#[cfg(test)]
mod proof;
mod money;
mod people;
mod stock;

// The handlers, whichever file they are written in. The routes name them from
// here, so splitting this module by what a shop is asking about changed no
// route and no caller.
pub(super) use looking::{
    adopt_sales, allowed, decide_again, decided, open_drawers, receipt, receipt_for_a_till,
    receipt_gaps, repairs, resolve_repair, shifts,
};
pub(super) use money::{
    account, day, made, owed, pay_supplier, sold, supplier_owing, supplier_statement, take_payment,
    vat, waived,
};
pub(super) use people::{
    amend_operator, issue_code, put_customer, put_operator, put_shop, revoke_terminal,
    set_operator_pin, terminals,
};
pub(super) use people::wire_operator;
pub(super) use stock::priceable;
pub(super) use stock::{
    correct_stock, delete_item, deliveries, item_now, items_from_tills, on_hand, put_supplier,
    receive_goods, record_count, resend_catalogue, suppliers, unreadable_changes, upsert_item,
};

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

    
    use axum::http::StatusCode;
    // Every shape that travels, because these tests ask the routes the way a
    // device does and a device's request is one of them. A glob rather than a
    // list: the list was what `use super::*` used to hand over, and keeping it
    // by hand is a line to edit every time a route gains a shape.
    use openpos_core::protocol::*;
    

    use crate::auth::Role;
    
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
                // A sale as a shop stored one before the schema was kept.
                payload_schema: None,
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
}
