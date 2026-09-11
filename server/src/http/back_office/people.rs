//! Who the shop is, who may stand at a till, and which devices are its own.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use openpos_core::protocol::{
    AmendOperatorRequest, CustomerWire, CustomersResponse,
    IssueCodeRequest, IssueCodeResponse, OperatorWire, OperatorsResponse, ProtocolError,
    PutCustomerRequest, PutOperatorRequest, PutShopRequest, PutShopRequestV8, RevokeTerminalRequest,
    RevokeTerminalResponse, SetOperatorPinRequest, ShopResponse, ShopResponseV8, TerminalHealthEntry,
    TerminalHealthRequest, TerminalHealthResponse,
};

use crate::http::{
    AppState, MAX_CODE_LIFETIME, authenticate, decode, encoded, owner_from,
    protocol_error, require_owner, unavailable,
};
use crate::auth::{Caller, EnrolmentCode, Role};
use crate::repo::{
    OperatorRecord, RepoError, Repository, ShopDetails,
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
pub(crate) async fn revoke_terminal<R: Repository>(
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

/// Add or correct somebody who buys on account. Owner only.
///
/// The whole list back, so a screen shows what is true rather than what it
/// assumed would be true.
pub(crate) async fn put_customer<R: Repository>(
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

/// Give somebody a new PIN. Owner only.
///
/// Separate from amending them, and carrying a credential and nothing else. The
/// key arrives already derived, by the same code the till will check it with, so
/// the PIN itself never reaches this process and cannot be logged by it.
pub(crate) async fn set_operator_pin<R: Repository>(
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
pub(crate) async fn amend_operator<R: Repository>(
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
pub(crate) async fn put_operator<R: Repository>(
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

pub(crate) fn wire_operator(record: OperatorRecord) -> OperatorWire {
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
pub(crate) async fn put_shop<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Two shapes, because versions up to 8 could not say which languages a shop
    // offers. A back office is served by the shop's own server, so the two ship
    // together, except that it keeps a copy of itself to work with the line
    // down: that copy is a build in the field and this is the body it sends
    // when somebody corrects the shop's address on it.
    let older = matches!(crate::http::version_of(&body), Ok(1..=8));
    let request = if older {
        match decode::<PutShopRequestV8>(&body) {
            // Filled in below, once the shop has been read, because what a
            // build that cannot say means is "leave it as it is".
            Ok(old) => old.with_the_languages_it_already_had(Vec::new()),
            Err(error) => return protocol_error(&error),
        }
    } else {
        match decode::<PutShopRequest>(&body) {
            Ok(request) => request,
            Err(error) => return protocol_error(&error),
        }
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match owner_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // What the shop already offers, for a build that could not say. Read before
    // the write rather than merged after it: empty means "offer every
    // language", which is a decision, and a screen that has never heard of the
    // setting must not make it on a shop's behalf. Somebody correcting an
    // address would otherwise turn a language back on at every till in the
    // shop and have no way of knowing they had.
    let languages = if older {
        // Refused rather than defaulted when the shop cannot be read. Empty is
        // not "we do not know", it is "offer every language", and writing it
        // because a query failed for a second would turn a database hiccup into
        // a decision the shop never made: a shop that had turned Bangla off
        // would find it back at every till, with nothing to say why. A save
        // that fails is a save somebody presses again.
        match state.repo.shop_details(caller.tenant).await {
            Ok(held) => held.languages,
            Err(_) => return unavailable(),
        }
    } else {
        tidy_languages(request.languages)
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
        // Tidied above, or kept as the shop already had it when the build that
        // sent this could not say.
        languages,
    };
    match state.repo.put_shop_details(caller.tenant, &details).await {
        Ok(()) => {
            let reply = ShopResponse {
                protocol,
                name: details.name,
                bin: details.bin,
                address: details.address,
                phone: details.phone,
                wallets: details.wallets,
                stock_rule: details.stock_rule,
                languages: details.languages,
            };
            // A screen a release behind is answered on the shape it can read.
            if protocol < 9 {
                return encoded(&ShopResponseV8::from(reply));
            }
            encoded(&reply)
        }
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(_) => unavailable(),
    }
}

/// The wallets a shop takes, as a report should read them.
///
/// Blank entries dropped, spaces trimmed, and one name kept once: a shop that
/// enters "bKash" and "bkash " has two lines in every report and no way to say
/// which sale went where.
fn tidy_languages(named: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::with_capacity(named.len());
    for one in named {
        // Lowercased, because a language code is not a name: `BN` and `bn` are
        // one language, and a shop that sent both would have a list that reads
        // as two.
        let one = one.trim().to_ascii_lowercase();
        if one.is_empty() || kept.contains(&one) {
            continue;
        }
        kept.push(one);
    }
    kept
}

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

/// Issue a code that will enrol a new device.
///
/// Owner only, and a caller may not grant a role above its own. The second rule
/// is trivially satisfied while there are two roles and only owners can reach
/// this route, and it is written down anyway: the day a third role exists, this
/// is the line that would otherwise have been missing.
pub(crate) async fn issue_code<R: Repository>(
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

/// Which tills are alive, and which are generating the support load.
pub(crate) async fn terminals<R: Repository>(
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
        TENANT, TERMINAL, app, app_with_till, post_to, shop_with_a_repair,
    };
    use crate::repo::MemoryRepo;

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
                    pin_rounds: openpos_core::auth::LEAST_PIN_ROUNDS,
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
                    pin_rounds: openpos_core::auth::LEAST_PIN_ROUNDS,
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
        assert_eq!(rina.pin_rounds, openpos_core::auth::LEAST_PIN_ROUNDS);

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
                    pin_rounds: openpos_core::auth::LEAST_PIN_ROUNDS,
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
            pin_rounds: openpos_core::auth::LEAST_PIN_ROUNDS,
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
}
