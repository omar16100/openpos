//! The HTTP surface.
//!
//! Thin on purpose: decode, call the logic that has its own tests, encode. No
//! decisions are made here that are not about HTTP itself.
//!
//! Bodies are postcard rather than JSON. Tills sync over patchy mobile networks
//! on prepaid data, and a batch of a hundred sales is roughly a third the size
//! encoded this way. The protocol version travels inside the body, so a
//! misrouted or stale client is refused with something specific rather than a
//! parse failure.

mod back_office;

use back_office::{priceable, wire_operator};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use openpos_core::protocol::{
    BalanceWire, BalancesRequest, BalancesResponse, CustomerWire, CustomersRequest,
    CustomersResponse, EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse, OnHandEntry,
    OnHandRequest, OnHandResponse, OperatorsRequest, OperatorsResponse, ProtocolError, PullRequest,
    PullResponse, PushAllowedRequest, PushAllowedRequestV4, PushAllowedResponse,
    PushCustomersRequest, PushCustomersResponse, PushItemsRequest, PushItemsResponse, PushRequest,
    PushShiftsRequest, PushShiftsRequestV1, PushShiftsResponse, RenewRequest, RenewResponse,
    ReportDrawerRequest, ReportDrawerResponse, SettingsRequest, SettingsResponse, ShopRequest,
    ShopResponse, negotiate,
};

use crate::auth::{Caller, EnrolmentCode, Role, Token, TokenHash, bearer};
use crate::ingest::{self, IngestError};
use crate::ratelimit::{Decision, RateLimiter};
use crate::repo::{RepoError, Repository, TOKEN_LIFETIME, TOKEN_RENEWAL_OVERLAP};

/// Content type for postcard bodies, versioned so a future encoding can be
/// introduced without guessing what a client sent.
pub const CONTENT_TYPE: &str = "application/vnd.openpos.v1+postcard";

/// An enrolment request is a protocol number and eight characters. Anything
/// larger is not one, and reading it into memory before deciding that would be
/// the cheapest denial of service available on an unauthenticated route.
const MAX_ENROL_BODY: usize = 1_024;

/// Most a repair queue page may return, whatever the caller asks for.
///
/// A shop whose till has been quarantining every sale for a fortnight has
/// thousands of entries. Serving them in one body would time out the connection
/// the owner is on, so the queue is paged and the ceiling is the server's to set.
const MAX_REPAIR_PAGE: u32 = 200;

/// Most of the owed list, and of one person's account, a page may return.
///
/// Both are paged from a cursor rather than cut off: a shop that lets three
/// hundred families buy on account used to see the first five hundred rows and
/// nothing to say there were more. The ceiling is the server's to set, and what
/// is past it is reached by asking for the next page.
const MAX_OWED_PAGE: u32 = 200;

/// Most of the trail of what was allowed a page may return. A busy shop allows
/// a handful of these a day, so this is a fortnight rather than an afternoon.
const MAX_ALLOWED_PAGE: u32 = 500;
const MAX_ACCOUNT_PAGE: u32 = 200;

/// Most a resolution note may be.
///
/// Generous for a sentence about what the shop decided, and small enough that a
/// client bug looping on a growing string cannot write an unbounded value into a
/// row that is then read back on every load of the queue.
const MAX_RESOLUTION_NOTE: usize = 2_000;

/// Shared state.
///
/// Generic over the repository rather than holding a trait object, because the
/// trait is asynchronous and an async method is not dyn compatible. Generics
/// also mean no lock around the server: a Postgres pool manages its own
/// concurrency, so two shops never wait on each other.
pub struct AppState<R> {
    pub repo: Arc<R>,
    /// Guards enrolment, the one route that accepts a guessable secret.
    pub enrolment_limit: Arc<RateLimiter>,
    /// How many reverse proxies sit in front of this server. Zero unless an
    /// operator says otherwise, because trusting a forwarded header nobody
    /// overwrites is worse than not reading one.
    pub trusted_proxy_hops: usize,
    /// An origin allowed to call this server from a browser, for development.
    ///
    /// Empty in production, and that is the intended shape: the server serves
    /// the till and the admin app itself, so they are the same origin and no
    /// browser ever asks. This exists because `npm run dev` puts the app on a
    /// different port, and a developer who cannot run the two together will
    /// find some worse way to make it work.
    pub dev_allow_origin: Option<String>,
}

impl<R> Clone for AppState<R> {
    fn clone(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
            enrolment_limit: Arc::clone(&self.enrolment_limit),
            trusted_proxy_hops: self.trusted_proxy_hops,
            dev_allow_origin: self.dev_allow_origin.clone(),
        }
    }
}

impl<R: Repository> AppState<R> {
    #[must_use]
    pub fn new(repo: R) -> Self {
        Self {
            repo: Arc::new(repo),
            enrolment_limit: Arc::new(RateLimiter::default()),
            trusted_proxy_hops: 0,
            dev_allow_origin: None,
        }
    }

    /// Override the enrolment limit, for tests and for operators who know their
    /// own traffic.
    #[must_use]
    pub fn with_enrolment_limit(mut self, limiter: RateLimiter) -> Self {
        self.enrolment_limit = Arc::new(limiter);
        self
    }

    /// Allow one browser origin to call this server, for development only.
    #[must_use]
    pub fn with_dev_allow_origin(mut self, origin: Option<String>) -> Self {
        self.dev_allow_origin = origin;
        self
    }

    /// Say how many reverse proxies sit in front, so the rate limiter can find
    /// the real client. Wrong here is worse than absent: too many hops reads an
    /// address the caller wrote.
    #[must_use]
    pub fn with_trusted_proxy_hops(mut self, hops: usize) -> Self {
        self.trusted_proxy_hops = hops;
        self
    }
}

/// Build the router.
///
/// The `/v1/back-office` routes are for the shop owner rather than the till.
/// They authenticate exactly as a till does, with the terminal credential the
/// owner's device was enrolled with, because this build has one kind of
/// identity. That is a real limitation and is written down rather than papered
/// over: any enrolled device in a shop can read that shop's repair queue and
/// edit that shop's prices. Separating the two needs a role on the credential,
/// which is a schema change and a protocol change, not a check bolted on here.
pub fn router<R: Repository + 'static>(state: AppState<R>) -> Router {
    use back_office::{
        account, adopt_sales, allowed, amend_operator, correct_stock, day, decide_again, decided,
        delete_item, deliveries, issue_code, item_now, items_from_tills, made, on_hand,
        open_drawers, owed, pay_supplier, put_customer, put_operator, put_shop, put_supplier,
        receipt, receipt_gaps, receive_goods, record_count, repairs, resolve_repair,
        revoke_terminal, set_operator_pin, shifts, sold, supplier_owing, supplier_statement,
        resend_catalogue, suppliers, take_payment, terminals, unreadable_changes, upsert_item,
        vat, waived,
    };

    Router::new()
        .route("/health", get(health))
        .route("/v1/sync/push", post(push))
        .route("/v1/sync/pull", post(pull))
        .route("/v1/sync/shifts", post(push_shifts))
        .route("/v1/sync/allowed", post(push_allowed))
        .route("/v1/sync/drawer", post(report_drawer))
        .route("/v1/lease", post(lease))
        .route("/v1/stock", post(stock))
        .route("/v1/sync/items", post(push_items))
        .route("/v1/sync/customers", post(push_customers))
        .route("/v1/enrol", post(enrol))
        .route("/v1/renew", post(renew))
        .route("/v1/back-office/stock/count", post(record_count))
        .route("/v1/back-office/suppliers", post(suppliers))
        .route("/v1/back-office/suppliers/put", post(put_supplier))
        .route("/v1/back-office/suppliers/owed", post(supplier_owing))
        .route("/v1/back-office/suppliers/payment", post(pay_supplier))
        .route(
            "/v1/back-office/suppliers/statement",
            post(supplier_statement),
        )
        .route("/v1/back-office/stock/receive", post(receive_goods))
        .route("/v1/back-office/enrolment-codes", post(issue_code))
        .route("/v1/back-office/stock/correct", post(correct_stock))
        .route("/v1/back-office/stock/on-hand", post(on_hand))
        .route("/v1/back-office/deliveries", post(deliveries))
        .route("/v1/back-office/shifts", post(shifts))
        .route("/v1/back-office/drawers", post(open_drawers))
        .route("/v1/back-office/day", post(day))
        .route("/v1/back-office/vat", post(vat))
        .route("/v1/back-office/sold", post(sold))
        .route("/v1/back-office/waived", post(waived))
        .route("/v1/back-office/sales/adopt", post(adopt_sales))
        .route("/v1/back-office/owed", post(owed))
        .route("/v1/back-office/owed/payment", post(take_payment))
        .route("/v1/back-office/owed/account", post(account))
        .route("/v1/shop", post(shop))
        .route("/v1/operators", post(operators))
        .route("/v1/customers", post(customers))
        .route("/v1/settings", post(settings))
        .route("/v1/customers/owed", post(balances))
        .route("/v1/back-office/customers", post(put_customer))
        .route("/v1/back-office/operators", post(put_operator))
        .route("/v1/back-office/operators/amend", post(amend_operator))
        .route("/v1/back-office/operators/pin", post(set_operator_pin))
        .route("/v1/back-office/shop", post(put_shop))
        .route("/v1/back-office/repairs", post(repairs))
        .route("/v1/back-office/repairs/resolve", post(resolve_repair))
        .route("/v1/back-office/repairs/decided", post(decided))
        .route("/v1/back-office/allowed", post(allowed))
        .route("/v1/back-office/receipt-gaps", post(receipt_gaps))
        .route("/v1/back-office/repairs/decide-again", post(decide_again))
        .route("/v1/back-office/terminals", post(terminals))
        .route("/v1/back-office/terminals/revoke", post(revoke_terminal))
        .route("/v1/back-office/catalogue/item", post(item_now))
        .route("/v1/back-office/receipt", post(receipt))
        // The same question from the counter, where the customer is standing
        // with the paper. A refund rung by scanning the goods again gives back
        // today's catalogue price, which is not what they paid for a basket
        // that had a discount on it.
        .route("/v1/receipt", post(back_office::receipt_for_a_till))
        .route("/v1/back-office/made", post(made))
        .route("/v1/back-office/catalogue/upsert", post(upsert_item))
        .route("/v1/back-office/catalogue/delete", post(delete_item))
        .route(
            "/v1/back-office/catalogue/unreadable",
            post(unreadable_changes),
        )
        .route(
            "/v1/back-office/catalogue/resend",
            post(resend_catalogue),
        )
        .route(
            "/v1/back-office/catalogue/from-tills",
            post(items_from_tills),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            allow_dev_origin,
        ))
        .with_state(state)
}

/// Answer a browser's cross-origin questions, when an origin is configured.
///
/// Nothing is allowed unless an operator named exactly one origin, and the
/// answer echoes that name rather than the caller's: a server that reflects
/// whatever origin asked has no origin policy at all, it has the appearance of
/// one, which is worse because it stops anybody looking.
async fn allow_dev_origin<R: Repository>(
    State(state): State<AppState<R>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(allowed) = state.dev_allow_origin.clone() else {
        return next.run(request).await;
    };

    let preflight = request.method() == axum::http::Method::OPTIONS;
    let mut response = if preflight {
        // A preflight never reaches a handler: there is nothing for one to do
        // with it, and routing it would mean every handler needs to know.
        Response::new(axum::body::Body::empty())
    } else {
        next.run(request).await
    };

    let headers = response.headers_mut();
    if let Ok(value) = allowed.parse() {
        headers.insert("access-control-allow-origin", value);
    }
    headers.insert(
        "access-control-allow-headers",
        axum::http::HeaderValue::from_static("authorization, content-type"),
    );
    headers.insert(
        "access-control-allow-methods",
        axum::http::HeaderValue::from_static("POST, GET, OPTIONS"),
    );
    response
}

async fn health() -> &'static str {
    "ok"
}

/// Establish who is calling, from the credential rather than from the body.
///
/// Every handler starts here. The request still carries its own idea of which
/// tenant and terminal it is, and that is checked against the token rather than
/// trusted: a mismatch means a misconfigured device pointed at the wrong shop,
/// which is worth refusing loudly instead of quietly serving the wrong data.
async fn authenticate<R: Repository>(
    state: &AppState<R>,
    headers: &HeaderMap,
    claimed_tenant: u128,
    claimed_terminal: u128,
) -> std::result::Result<Caller, Response> {
    let caller = caller_from(state, headers).await?;
    if caller.tenant != claimed_tenant || caller.terminal != claimed_terminal {
        return Err(protocol_error(&ProtocolError::UnknownTerminal));
    }
    Ok(caller)
}

/// Who is calling, when the route needs a particular kind of device.
///
/// The role is checked here rather than inside each handler, so adding a
/// back-office route is a matter of asking for the right caller rather than
/// remembering a check. A forgotten check is how a till ends up able to reprice
/// the shop.
fn require_owner(caller: Caller) -> std::result::Result<Caller, Box<Response>> {
    if !caller.role.covers(Role::Owner) {
        return Err(Box::new(protocol_error(&ProtocolError::NotPermitted)));
    }
    Ok(caller)
}

async fn owner_from<R: Repository>(
    state: &AppState<R>,
    headers: &HeaderMap,
) -> std::result::Result<Caller, Response> {
    let caller = caller_from(state, headers).await?;
    if !caller.role.covers(Role::Owner) {
        return Err(protocol_error(&ProtocolError::NotPermitted));
    }
    Ok(caller)
}

/// Who is calling, taken from the credential alone.
///
/// For requests that state no identity at all, which is the shape newer routes
/// use: a body that names a tenant is a body whose claim has to be checked, and
/// a check is a thing that can be forgotten on the next route somebody adds.
async fn caller_from<R: Repository>(
    state: &AppState<R>,
    headers: &HeaderMap,
) -> std::result::Result<Caller, Response> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let Some(token) = bearer(presented) else {
        return Err(protocol_error(&ProtocolError::Unauthenticated));
    };

    match state.repo.authenticate(&TokenHash::of(token)).await {
        Ok(Some(caller)) => Ok(caller),
        // A credential this shop does not hold: revoked, expired, or from a
        // device that was wiped and never re-enrolled. The credential itself is
        // never written down, only that one was refused, because a log is read
        // by more people than a database.
        Ok(None) => {
            tracing::warn!("a credential this shop does not hold was presented");
            Err(protocol_error(&ProtocolError::Unauthenticated))
        }
        Err(_) => Err(unavailable()),
    }
}

/// Note that a till just synced, for the terminal health list.
///
/// A failure here is swallowed on purpose. This is telemetry for a support
/// screen, and refusing a sync because a timestamp could not be written would
/// turn a cosmetic problem into a shop that cannot sell. It is logged instead,
/// because a health page that has quietly stopped updating is worse than one
/// that is obviously broken.
async fn note_contact<R: Repository>(state: &AppState<R>, caller: Caller) {
    if state
        .repo
        .mark_terminal_seen(caller.tenant, caller.terminal)
        .await
        .is_err()
    {
        tracing::warn!(
            tenant = %caller.tenant,
            terminal = %caller.terminal,
            "could not record that a terminal synced, terminal health will understate it"
        );
    }
}

/// The version a body says it is, read on its own.
///
/// Every request begins with it, which is what makes reading one shape or
/// another possible at all.
pub(crate) fn version_of(body: &[u8]) -> Result<u16, ProtocolError> {
    let (requested, _) =
        postcard::take_from_bytes::<u16>(body).map_err(|_| ProtocolError::Malformed)?;
    negotiate(requested)
}

/// Read a request, version first.
///
/// postcard is positional, so a body written by a build this server does not
/// speak fails to decode as a whole and the caller is told "malformed", which
/// tells a shop nothing. Every request begins with its protocol version, so
/// that leading number is read on its own and answered before the rest is
/// touched: an old till is told to update rather than left with a mystery.
pub(crate) fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ProtocolError> {
    version_of(body)?;
    postcard::from_bytes(body).map_err(|_| ProtocolError::Malformed)
}

/// Sales from a till.
async fn push<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PushRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // Recorded before the batch is stored, not after. The question the health
    // list answers is when the server last heard from this device, and a push
    // that fails on the way to the database is still the device talking.
    note_contact(&state, caller).await;

    let carried = request.sales.len();
    match ingest::push(state.repo.as_ref(), &request).await {
        Ok(response) => {
            // The one line that answers "did my sales reach the shop", which is
            // the first thing anybody asks. Per batch rather than per sale: a
            // till syncs all day and a line each would bury everything else.
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                carried,
                accepted = response.accepted.len(),
                quarantined = response.quarantined.len(),
                "sales taken from a till"
            );
            // And a line each for the ones somebody has to look at, because
            // that is a job for a person and a count does not say which sale.
            for held in &response.quarantined {
                tracing::warn!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    sale = %held.id,
                    reason = ?held.reason,
                    "a sale was stored and is waiting for somebody to decide"
                );
            }
            encoded(&response)
        }
        Err(IngestError::Protocol(error)) => protocol_error(&error),
        // The till keeps its copy and retries. Telling it otherwise would let it
        // drop the only record of a sale that already happened.
        Err(IngestError::Storage) => {
            tracing::error!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                carried,
                "sales could not be stored; the till keeps them and will try again"
            );
            unavailable()
        }
    }
}

/// Catalogue changes to a till.
async fn pull<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PullRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // A till open all day on a quiet Tuesday pushes nothing and pulls anyway.
    // Counting only pushes would report that shop's terminal as dead, and a
    // false alarm costs the same phone call a real one does.
    note_contact(&state, caller).await;

    // The tenant comes from the credential, never from the body.
    match state
        .repo
        .items_since(caller.tenant, request.cursor, request.limit)
        .await
    {
        Ok(page) => {
            // A catalogue row this build cannot read is passed over and the
            // cursor still moves, so a shop can lose a price change without
            // anybody noticing. The alternative, failing the page, stops every
            // till in the shop syncing for ever. So it is skipped and said out
            // loud: this is the only place that knows it happened as it happens.
            if page.skipped > 0 {
                tracing::warn!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    skipped = page.skipped,
                    cursor = page.cursor,
                    "catalogue changes this build cannot read were passed over; \
                     those prices will not reach this till"
                );
            }
            encoded(&PullResponse {
                protocol,
                cursor: page.cursor,
                upserts: page.upserts,
                tombstones: page.tombstones,
                more: page.more,
            })
        }
        Err(_) => unavailable(),
    }
}

/// The people who may stand at a till.
///
/// Readable by any credential: a till has to know who may sign in, and it has
/// to know it before the internet goes down.
async fn operators<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<OperatorsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match caller_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.operators(caller.tenant).await {
        Ok(found) => encoded(&OperatorsResponse {
            protocol,
            operators: found.into_iter().map(wire_operator).collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// The shop's own details, for the top of a receipt.
///
/// Readable by any credential, not just an owner: every till prints receipts,
/// and a till that could not learn its own shop's name would print blank ones.
async fn shop<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ShopRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match caller_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.shop_details(caller.tenant).await {
        Ok(details) => encoded(&ShopResponse {
            protocol,
            name: details.name,
            bin: details.bin,
            address: details.address,
            phone: details.phone,
            wallets: details.wallets,
            stock_rule: details.stock_rule,
        }),
        Err(RepoError::UnknownTerminal) => protocol_error(&ProtocolError::UnknownTerminal),
        Err(_) => unavailable(),
    }
}

/// What each of them owes.
///
/// A till's route, asked more often than the list of names: a name is written
/// down once and a balance changes every time somebody takes a bag of rice. A
/// cashier is asked "how much do I owe" across the counter and the answer
/// should not be "wait for the back office".
async fn balances<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<BalancesRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.customer_balances(caller.tenant).await {
        Ok(found) => encoded(&BalancesResponse {
            protocol,
            balances: found
                .into_iter()
                .map(|(customer, owed_minor)| BalanceWire {
                    customer,
                    owed_minor,
                })
                .collect(),
        }),
        Err(_) => unavailable(),
    }
}

/// Where the shop's settings stand, as one number.
///
/// A till's route, and the cheapest one here: one row, one column. It exists so
/// a till can ask often without asking for the three lists themselves, which is
/// what makes suspending somebody take half a minute to reach a till rather
/// than ten.
async fn settings<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<SettingsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state.repo.settings_seq(caller.tenant).await {
        Ok(seq) => encoded(&SettingsResponse { protocol, seq }),
        Err(_) => unavailable(),
    }
}

/// Who the shop lets buy on account.
///
/// A till's route, like the people who may sign in, and for the same reason: a
/// sale on account is written with the internet down, and a name typed from
/// memory is how one Karim ends up paying for another Karim's rice.
async fn customers<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<CustomersRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

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

/// What a till has open right now.
///
/// A till's own route, like the counted drawer that follows it. The terminal
/// comes from the credential rather than the body: a device may say what its
/// own drawer holds and nobody else's.
async fn report_drawer<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<ReportDrawerRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    note_contact(&state, caller).await;

    let drawer = crate::repo::OpenDrawer {
        terminal: caller.terminal,
        shift: request.shift,
        opened_at_ms: request.opened_at_ms,
        reported_at_ms: request.at_ms,
        opening_float_minor: request.opening_float_minor,
        sales: request.sales,
        cash_sales_minor: request.cash_sales_minor,
        non_cash_sales_minor: request.non_cash_sales_minor,
        cash_in_minor: request.cash_in_minor,
        cash_out_minor: request.cash_out_minor,
        expected_cash_minor: request.expected_cash_minor,
    };
    match state.repo.put_open_drawer(caller.tenant, &drawer).await {
        Ok(()) => encoded(&ReportDrawerResponse { protocol }),
        Err(_) => unavailable(),
    }
}

/// Drawers a till has counted and closed.
///
/// A till's own route rather than the back office's, because a till is what
/// closes a drawer. The point of the whole thing is that somebody who was not
/// standing at it reconciles the count afterwards, and until this existed the
/// count never left the device.
async fn push_shifts<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Two shapes, because version 1 did not say who counted the drawer. A till
    // a release behind still has to be able to hand over what it counted: the
    // alternative is a device holding the only record of a count nobody can
    // reconstruct until somebody walks to the shop with a new build.
    let request = match version_of(&body) {
        Ok(1) => match decode::<PushShiftsRequestV1>(&body) {
            Ok(old) => PushShiftsRequest {
                protocol: old.protocol,
                tenant: old.tenant,
                terminal: old.terminal,
                shifts: old.shifts.into_iter().map(Into::into).collect(),
            },
            Err(error) => return protocol_error(&error),
        },
        // And versions 2 to 5, which sent a drawer without the struck-out cash
        // in its window. A till never fills that in, so nothing is lost by
        // reading the older shape: it is the shop's own answer about a window,
        // worked out when somebody asks.
        Ok(2..=5) => match decode::<openpos_core::protocol::PushShiftsRequestV5>(&body) {
            Ok(old) => PushShiftsRequest {
                protocol: old.protocol,
                tenant: old.tenant,
                terminal: old.terminal,
                shifts: old.shifts.into_iter().map(Into::into).collect(),
            },
            Err(error) => return protocol_error(&error),
        },
        Ok(_) => match decode::<PushShiftsRequest>(&body) {
            Ok(request) => request,
            Err(error) => return protocol_error(&error),
        },
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // Who the shop says its people are, asked once for the whole push.
    //
    // The id on a count is the till's word and stays that way: the server knows
    // which device holds a credential, never who is standing at it, and a taken
    // device is unenrolled rather than argued with. The *name* is a different
    // thing, because the shop holds its own answer for every id it issued. A
    // till saying "Fatima" against the id the shop has recorded as Rahim's was
    // written down and shown to an owner as fact, and a drawer's name is read
    // months later by somebody deciding whether to trust a person with the
    // till.
    //
    // Asked only when somebody is named at all: a drawer from a build before
    // this was written names nobody, and a shop with no counts to push does not
    // pay for a query.
    let named = request.shifts.iter().any(|shift| shift.closed_by != 0);
    let people = if named {
        state.repo.operators(caller.tenant).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let shop_calls_them = |who: u128| {
        people
            .iter()
            .find(|person| person.id == who)
            .map(|person| person.name.clone())
    };

    let shifts: Vec<crate::repo::ClosedShift> = request
        .shifts
        .into_iter()
        .map(|shift| crate::repo::ClosedShift {
            id: shift.id,
            // The id as reported, for the reason above.
            closed_by: shift.closed_by,
            // The name as the shop holds it, and no name at all for an id the
            // shop has never issued. A name nobody can vouch for is worse than
            // a blank, because a blank reads as "an older build counted this"
            // and a wrong name reads as a person.
            closed_by_name: if shift.closed_by == 0 {
                String::new()
            } else {
                shop_calls_them(shift.closed_by).unwrap_or_default()
            },
            // The terminal from the credential, not from the body: a device may
            // report its own drawer and nobody else's.
            terminal: caller.terminal,
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
        .collect();

    match state.repo.put_shifts(caller.tenant, &shifts).await {
        Ok(accepted) => {
            note_contact(&state, caller).await;
            // A line per drawer, with the variance in it. There are a handful a
            // day per till, and the number an owner rings up about weeks later
            // is exactly this one.
            for shift in &shifts {
                // Said out loud rather than swallowed. A till naming somebody
                // the shop does not have is either a device nobody should
                // trust or a bug in the device's own copy of the people, and
                // both are things an owner's logs should carry.
                if shift.closed_by != 0 && shift.closed_by_name.is_empty() {
                    tracing::warn!(
                        tenant = %caller.tenant,
                        terminal = %caller.terminal,
                        drawer = %shift.id,
                        counted_by = %uuid::Uuid::from_u128(shift.closed_by),
                        "a drawer names somebody this shop has no record of: \
                         the count is kept and the name is not"
                    );
                }
                tracing::info!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    drawer = %shift.id,
                    counted_by = %shift.closed_by_name,
                    expected_minor = shift.expected_cash_minor,
                    counted_minor = shift.counted_cash_minor,
                    variance_minor = shift.variance_minor,
                    "a counted drawer reached the shop"
                );
            }
            encoded(&PushShiftsResponse { protocol, accepted })
        }
        Err(_) => unavailable(),
    }
}

/// What a till allowed, on its way to the shop that has to answer for it.
///
/// Authenticated like any other thing a till sends, and the terminal is taken
/// from the credential rather than the body: a device may report what it
/// allowed and nobody else's.
async fn push_allowed<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Two shapes, because versions up to 4 did not say which receipt a reprint
    // was of. A till a release behind still has to be able to hand over what it
    // allowed: postcard is positional, so its body read as the current shape is
    // a decode failure, and the device would be left holding the only record of
    // who allowed what while its pushes failed on a timer.
    let request = match version_of(&body) {
        Ok(1..=4) => match decode::<PushAllowedRequestV4>(&body) {
            Ok(old) => PushAllowedRequest {
                protocol: old.protocol,
                tenant: old.tenant,
                terminal: old.terminal,
                allowed: old.allowed.into_iter().map(Into::into).collect(),
            },
            Err(error) => return protocol_error(&error),
        },
        Ok(_) => match decode::<PushAllowedRequest>(&body) {
            Ok(request) => request,
            Err(error) => return protocol_error(&error),
        },
        Err(error) => return protocol_error(&error),
    };
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    let allowed: Vec<crate::repo::AllowedAction> = request
        .allowed
        .into_iter()
        .map(|one| crate::repo::AllowedAction {
            // From the credential, not the body.
            terminal: caller.terminal,
            seq: one.seq,
            // Taken as reported, like the drawer's clock: the server cannot
            // know what time the person standing at the till saw, only what the
            // device said it was.
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

    match state
        .repo
        .put_allowed(caller.tenant, caller.terminal, &allowed)
        .await
    {
        Ok(stored) => {
            note_contact(&state, caller).await;
            encoded(&PushAllowedResponse { protocol, stored })
        }
        Err(_) => unavailable(),
    }
}

/// Trade a working credential for a fresh one.
///
/// Authenticated with the credential being replaced, which is what makes this
/// safe without a code: only a device already holding a valid token can ask, and
/// the server takes the identity from that token rather than from the body.
///
/// The old credential is not revoked. It lapses after an overlap, because this
/// reply can be lost on a bad connection and a device that had its only working
/// token revoked the instant the server issued a new one would be left with
/// nothing that authenticates and no way to ask for more: a shop offline until
/// somebody re-enrols the tablet by hand.
async fn renew<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<RenewRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;

    let Some(presented) = bearer(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    ) else {
        return protocol_error(&ProtocolError::Unauthenticated);
    };
    let previous = TokenHash::of(presented);
    let caller = match state.repo.authenticate(&previous).await {
        Ok(Some(caller)) => caller,
        Ok(None) => return protocol_error(&ProtocolError::Unauthenticated),
        Err(_) => return unavailable(),
    };

    let replacement = Token::generate();
    match state
        .repo
        .renew_token(
            caller,
            &previous,
            &replacement.hash(),
            TOKEN_RENEWAL_OVERLAP,
        )
        .await
    {
        Ok(()) => {
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                overlap_seconds = TOKEN_RENEWAL_OVERLAP.as_secs(),
                "a device replaced its credential"
            );
            encoded(&RenewResponse {
                protocol,
                token: replacement.into_string(),
                expires_in_seconds: TOKEN_LIFETIME.as_secs(),
                previous_valid_for_seconds: TOKEN_RENEWAL_OVERLAP.as_secs(),
            })
        }
        // The credential stopped being good between being authenticated and
        // being replaced, which is a shop withdrawing the device while it was
        // asking. Told as what it is rather than as a shop that is briefly
        // unwell: this device is not to come back.
        Err(crate::repo::RepoError::UnknownTerminal) => {
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                "a device asked to replace a credential that had just been withdrawn"
            );
            protocol_error(&ProtocolError::Unauthenticated)
        }
        Err(_) => unavailable(),
    }
}

/// Items a till wrote down at the counter, on their way into the catalogue.
///
/// A delivery arrives during an outage with a barcode in nobody's catalogue.
/// The till writes the item down so the sale can happen; this is where that
/// item becomes the shop's. Marked as a till's work whatever the till says
/// about it: a price typed to get a queue moving is not a price the owner has
/// agreed to, and the back office lists these for somebody to look at.
async fn push_items<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PushItemsRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    note_contact(&state, caller).await;

    let mut stored = Vec::with_capacity(request.items.len());
    for mut item in request.items {
        item.from_a_till = true;

        // A till writes an item down when a delivery arrives during an outage
        // with a barcode in nobody's catalogue. That is the whole of its
        // business with the catalogue: an item the shop already knows is the
        // owner's to change, and the roles exist because a shop with six tills
        // had six devices that could reprice everything.
        //
        // Acknowledged rather than refused, and the difference matters: a till
        // holds an item it wrote until the shop says it has it, so a refusal
        // would be a device sending the same thing for ever. The commonest
        // reason to be here at all is a reply that went missing on the way back
        // from the first attempt.
        match state.repo.catalogue_holds(caller.tenant, item.id).await {
            Ok(true) => {
                tracing::info!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    item = %item.id,
                    "a till sent an item the shop already holds; it is not overwritten"
                );
                stored.push(item.id);
                continue;
            }
            Ok(false) => {}
            Err(_) => return unavailable(),
        }

        // A barcode belongs to one item, which is the rule everywhere else and
        // is not suspended because a till was offline. If the shop has since
        // given this code to something of its own, the shop's item keeps it and
        // this one arrives without it: the sales that name this item still
        // resolve to something, nothing scans two ways, and the list of items a
        // till wrote is where somebody decides what to do about the pair.
        if !item.barcodes.is_empty() {
            match state
                .repo
                .barcode_holders(caller.tenant, &item.barcodes)
                .await
            {
                Ok(holders) => {
                    let taken: Vec<String> = holders
                        .into_iter()
                        .filter(|(_, holder)| *holder != item.id)
                        .map(|(code, _)| code)
                        .collect();
                    if !taken.is_empty() {
                        tracing::warn!(
                            tenant = %caller.tenant,
                            terminal = %caller.terminal,
                            item = %item.id,
                            barcodes = ?taken,
                            "an item a till wrote down carries a barcode the shop already gave to \
                             something else; it is stored without those codes"
                        );
                        item.barcodes.retain(|code| !taken.contains(code));
                    }
                }
                Err(_) => return unavailable(),
            }
        }

        // The same bound the back office is held to, and for the same reason:
        // a till applies a page of catalogue changes as one batch and refuses
        // the whole page if any item in it cannot be priced. One item written
        // at a counter with a rate no arithmetic accepts would stop every
        // device in the shop from seeing any price change at all.
        //
        // Dropped rather than held, like anything else a till cannot fix by
        // sending it again: the sale it was written for is already stored, and
        // the item is one an owner will correct in the back office.
        if let Err(refusal) = priceable(&item) {
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                item = %item.id,
                said = %refusal,
                "an item a till wrote down could not be priced; it is not stored"
            );
            continue;
        }

        match state.repo.upsert_item(caller.tenant, &item).await {
            Ok(cursor) => {
                tracing::info!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    item = %item.id,
                    cursor,
                    "an item a till wrote down at the counter reached the shop"
                );
                stored.push(item.id);
            }
            Err(RepoError::Invalid) => {
                // Nothing the till can fix by sending it again, and holding it
                // for ever would stop everything behind it. Said out loud and
                // dropped from the queue.
                tracing::warn!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    item = %item.id,
                    "an item a till wrote down could not be stored and was refused"
                );
                stored.push(item.id);
            }
            Err(_) => return unavailable(),
        }
    }

    encoded(&PushItemsResponse { protocol, stored })
}

/// People a till wrote down at the counter, on their way into the shop's list.
///
/// Somebody buys on account who is in nobody's list. Writing them down at the
/// till is what keeps two people with one name apart: a sale against a typed
/// name is added up against the spelling, and the second Karim ends up paying
/// for the first one's rice.
///
/// Not marked as a till's work the way an item is: a name and a phone number
/// are what somebody said about themselves, and there is no price here for an
/// owner to disagree with.
async fn push_customers<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<PushCustomersRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    note_contact(&state, caller).await;

    let mut stored = Vec::with_capacity(request.customers.len());
    for customer in request.customers {
        // Nobody may hold the nil id, and nobody may be nameless, which is the
        // rule the back office is held to as well.
        if customer.id == 0 || customer.name.trim().is_empty() {
            tracing::warn!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                "a till sent somebody with no name or no id; it is refused rather than held"
            );
            stored.push(customer.id);
            continue;
        }
        let record = crate::repo::CustomerRecord {
            id: customer.id,
            name: customer.name.trim().to_owned(),
            phone: customer
                .phone
                .map(|phone| phone.trim().to_owned())
                .filter(|phone| !phone.is_empty()),
            active: customer.active,
            bin: customer
                .bin
                .map(|bin| bin.trim().to_owned())
                .filter(|bin| !bin.is_empty()),
            // Never read for somebody the shop already holds: the write below
            // keeps what the owner decided. Zero is what a person written down
            // at a counter starts with, which is no cap.
            limit_minor: 0,
        };
        match state
            .repo
            .write_customer_from_a_till(caller.tenant, &record)
            .await
        {
            Ok(()) => {
                tracing::info!(
                    tenant = %caller.tenant,
                    terminal = %caller.terminal,
                    customer = %record.id,
                    "somebody a till wrote down reached the shop"
                );
                stored.push(record.id);
            }
            Err(_) => return unavailable(),
        }
    }

    encoded(&PushCustomersResponse { protocol, stored })
}

/// What the shop believes is on the shelves, for a till.
///
/// The same question the back office asks, answered for a till, because a till
/// that has been told to warn or refuse needs the figure where the deciding
/// happens and with the line down. Not the catalogue's copy: that is whatever
/// somebody last typed on an item record and it never moves.
///
/// A window at a time. One figure costs one query, and a shop with a long
/// catalogue asking for all of it every few minutes would be paying for a
/// megabyte to enforce a rule about a dozen items.
async fn stock<R: Repository>(
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
    let caller = match caller_from(&state, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // A till asks about what it holds, so an empty list is a caller mistake
    // rather than a request for everything: the back office's version of this
    // takes the first page of the catalogue, and a till doing that would watch
    // the same two hundred items for ever.
    const MOST: usize = 200;
    let wanted: Vec<u128> = request.item_ids.into_iter().take(MOST).collect();
    // Asked once for the lot rather than one at a time. This is the call a till
    // makes every five minutes to keep the figure behind a stock refusal from
    // going stale, and one at a time it was six hundred round trips: a shop with
    // eight hundred lines took twenty minutes to get round its own catalogue,
    // and the refusal at the far end was that far behind the shelf.
    let figures: Vec<OnHandEntry> = match state.repo.on_hand_many(caller.tenant, &wanted).await {
        Ok(found) => found
            .into_iter()
            .map(|entry| OnHandEntry {
                item_id: entry.item_id,
                qty_milli: entry.qty_milli,
                counted_at_ms: entry.counted_at_ms,
                unreconciled_milli: entry.unreconciled_milli,
                unreconciled_sales: u32::try_from(entry.unreconciled_sales).unwrap_or(u32::MAX),
            })
            .collect(),
        // An item the shop has since withdrawn is not an error to a till holding
        // a catalogue a moment out of date.
        Err(RepoError::UnknownTerminal) => Vec::new(),
        Err(_) => return unavailable(),
    };
    note_contact(&state, caller).await;
    encoded(&OnHandResponse {
        protocol,
        on_hand: figures,
        // A till asks about the items in front of it, never about the shelf as
        // a whole, so this answer is never all of anything.
        whole: false,
    })
}

/// A block of receipt numbers for a till.
async fn lease<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match decode::<LeaseRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    note_contact(&state, caller).await;

    match state
        .repo
        .issue_lease(caller.tenant, caller.terminal, request.count)
        .await
    {
        Ok(record) => {
            // Receipt numbers are the thing a shop is audited on, so which
            // device was given which of them is written down as it happens
            // rather than worked out afterwards from what was printed.
            tracing::info!(
                tenant = %caller.tenant,
                terminal = %caller.terminal,
                epoch = record.epoch,
                first = record.first,
                last = record.last,
                "a block of receipt numbers was issued"
            );
            encoded(&LeaseResponse {
                protocol,
                epoch: record.epoch,
                // Short and human readable, because it is printed on every
                // receipt and read aloud over the phone when something is
                // disputed.
                prefix: format!("T{:X}", record.terminal & 0xFFFF),
                first: record.first,
                last: record.last,
            })
        }
        Err(RepoError::UnknownTerminal) => protocol_error(&ProtocolError::UnknownTerminal),
        // A lease request cannot be malformed in a way the store rejects, but
        // matching it explicitly means the day one can, this line is a compile
        // error rather than a silent 503 a client retries forever.
        Err(RepoError::Invalid) => protocol_error(&ProtocolError::Malformed),
        Err(RepoError::Backend) => unavailable(),
    }
}

/// Which client a request came from, for rate limiting.
///
/// The socket peer address is the only value a caller cannot choose, so it is
/// the default and the fallback. It is also useless on its own in either
/// documented deployment: Caddy fronts the self-host image and Cloudflare fronts
/// the hosted tier, and behind either one every request in the world arrives
/// from the proxy's address and shares a single bucket of ten attempts a minute.
/// One attacker, or ordinary internet background noise, then locks every shop
/// out of enrolling a tablet.
///
/// So `X-Forwarded-For` is read, but only when the operator has said how many
/// proxies sit in front, and only that many entries from the right. Entries
/// further left were written by whoever was calling and are worth nothing: a
/// header trusted blindly lets an attacker mint a fresh budget per request by
/// inventing an address, which is worse than one shared bucket.
///
/// `hops` of zero, the default, means no proxy and no header.
fn client_key(request: &Request, hops: usize) -> String {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(
            // No peer address at all. Sharing one bucket is the safe direction:
            // it throttles, where a unique key per unknown caller would not
            // throttle at all.
            || "unknown".to_owned(),
            |ConnectInfo(address)| address.ip().to_string(),
        );

    if hops == 0 {
        return peer;
    }

    let forwarded = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    // Rightmost is the address the nearest proxy saw. Counting in from the right
    // by the number of proxies configured lands on the client, and anything left
    // of that is caller-supplied.
    forwarded
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .rev()
        .nth(hops.saturating_sub(1))
        .map_or(peer, ToOwned::to_owned)
}

/// Longest a code may be left standing.
///
/// An hour. A shop enrols a tablet with the owner present, and a code that
/// outlives the conversation is a credential lying around: forty bits is fine
/// for minutes and thin for a week.
const MAX_CODE_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Trade a short code for a real credential.
///
/// The only route that takes no token, because it is how a device gets one. It
/// also takes no tenant and no terminal: both come from the code, so a device
/// cannot enrol itself into a shop it was not invited to.
///
/// Rate limited per client. See [`client_key`] for what "per client" means when
/// a proxy is in front, which in both documented deployments it is.
async fn enrol<R: Repository>(State(state): State<AppState<R>>, http: Request) -> Response {
    let key = client_key(&http, state.trusted_proxy_hops);

    // The body is read and parsed before any budget is spent. Anything else
    // lets a flood of unparseable requests exhaust a shop's enrolment attempts
    // without ever having guessed at a code.
    let body = match axum::body::to_bytes(http.into_body(), MAX_ENROL_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return protocol_error(&ProtocolError::Malformed),
    };
    let request = match decode::<EnrolRequest>(&body) {
        Ok(request) => request,
        Err(error) => return protocol_error(&error),
    };

    if let Decision::Deny { retry_after } = state.enrolment_limit.check(&key) {
        return protocol_error(&ProtocolError::TooManyAttempts {
            retry_after_seconds: retry_after.as_secs().max(1),
        });
    }
    // Already negotiated by decode(), which would not have got here.
    let protocol = request.protocol;

    let caller = match state
        .repo
        .redeem_enrolment_code(&EnrolmentCode::hash_of(&request.code))
        .await
    {
        Ok(Some(caller)) => caller,
        // Unknown, expired and already used are one answer, so probing tells an
        // attacker nothing about which it was.
        Ok(None) => {
            // Said here even though the caller is told nothing: a shop reading
            // its own log should be able to see somebody guessing at codes.
            tracing::warn!(
                from = %key,
                "an enrolment code was offered and is not one this shop is holding"
            );
            return protocol_error(&ProtocolError::Unauthenticated);
        }
        Err(_) => return unavailable(),
    };

    let token = Token::generate();
    if state.repo.store_token(caller, &token.hash()).await.is_err() {
        return unavailable();
    }
    // A device joining a shop is worth a line. The credential it was given is
    // not in it and never will be: a log is read by more people than a database.
    tracing::info!(
        tenant = %caller.tenant,
        terminal = %caller.terminal,
        "a device enrolled and was given a credential"
    );

    encoded(&EnrolResponse {
        protocol,
        tenant: caller.tenant,
        terminal: caller.terminal,
        token: token.into_string(),
    })
}

fn encoded<T: serde::Serialize>(value: &T) -> Response {
    match postcard::to_allocvec(value) {
        Ok(bytes) => ([(header::CONTENT_TYPE, CONTENT_TYPE)], bytes).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A refusal the client can act on.
///
/// The reason travels in the body in the same encoding as everything else, so a
/// till can tell "you are too old, upgrade" apart from "that terminal is not
/// yours", rather than seeing an opaque 400 and retrying forever.
fn protocol_error(error: &ProtocolError) -> Response {
    let status = match error {
        ProtocolError::UnsupportedVersion { .. } => StatusCode::UPGRADE_REQUIRED,
        ProtocolError::UnknownTerminal => StatusCode::FORBIDDEN,
        ProtocolError::Unauthenticated => StatusCode::UNAUTHORIZED,
        ProtocolError::TooManyAttempts { .. } => StatusCode::TOO_MANY_REQUESTS,
        // Forbidden rather than unauthorized: the credential is genuine and
        // presenting a different one is not the answer, so a client that
        // retries after re-enrolling is wasting everybody's time.
        ProtocolError::NotPermitted => StatusCode::FORBIDDEN,
        // Somebody else changed it first. The same status a browser uses for
        // "your copy is out of date", and a refusal the caller can act on by
        // reading again rather than by retrying the same bytes.
        ProtocolError::Stale => StatusCode::CONFLICT,
        // The same status, and for the same reason: what the shop holds
        // disagrees with what was sent, and the answer is to look rather than
        // to send it again.
        ProtocolError::BarcodeInUse { .. } => StatusCode::CONFLICT,
        // And again: the shop holds a history this request assumes is not
        // there. Sending the same bytes again will not change that, and the
        // act that was wanted is a different one.
        ProtocolError::ItemHasHistory => StatusCode::CONFLICT,
        // What was sent cannot be a price or a rate. The caller has to change
        // what it sent rather than send it again, which is what this status
        // means.
        ProtocolError::NotAPrice { .. }
        | ProtocolError::RateIsNotARate { .. }
        | ProtocolError::PriceBelowNothing { .. }
        | ProtocolError::CostBelowNothing { .. } => StatusCode::BAD_REQUEST,
        ProtocolError::Malformed => StatusCode::BAD_REQUEST,
    };
    match postcard::to_allocvec(error) {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, CONTENT_TYPE)], bytes).into_response(),
        Err(_) => status.into_response(),
    }
}

/// Temporary failure. Distinct from a refusal on purpose: a till must retry this
/// one, and must not retry a refusal.
///
/// Says where it came from. Sixty-nine call sites answered a shop with a bare
/// 503 and wrote nothing down, so the whole of what anybody supporting a shop
/// had to go on was a till saying it could not reach the server. The location is
/// the line that gave up, which is what the person reading the log needs; the
/// handlers that carry a shop's money say more than this on their way past.
#[track_caller]
fn unavailable() -> Response {
    tracing::error!(
        at = %core::panic::Location::caller(),
        "a request could not be served from storage; the caller is told to retry"
    );
    StatusCode::SERVICE_UNAVAILABLE.into_response()
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
    use axum::http::Request;
    use http_body_util::BodyExt;
    use openpos_core::protocol::{
        ItemWire, PROTOCOL_VERSION, QuarantineReason, RepairQueueRequest,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::repo::{MemoryRepo, StoredSale};

    pub(super) const TENANT: u128 = 42;
    pub(super) const TERMINAL: u128 = 7;

    /// A shop, a terminal, a small catalogue, and the terminal's credential.
    pub(super) fn app() -> (Router, String) {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));
        repo.delete_item(TENANT, 1);
        (router(AppState::new(repo)), token.into_string())
    }

    pub(super) fn item(id: u128) -> ItemWire {
        ItemWire {
            id,
            code: format!("SKU{id:03}"),
            name_en: "Rice Miniket 5kg".to_owned(),
            name_bn: "মিনিকেট চাল ৫ কেজি".to_owned(),
            unit: "Nos".to_owned(),
            price_minor: 43_000,
            cost_minor: 38_000,
            vat_bp: 1_500,
            price_inclusive: false,
            vat_on_undiscounted: false,
            barcodes: vec![format!("869000000{id:04}")],
            on_hand_milli: 40_000,
            active: true,
            from_a_till: false,
            supply: 0,
            category: String::new(),
        }
    }

    /// A till that passed over a change gets it when the shop says everything
    /// again.
    ///
    /// A till follows the catalogue by a cursor and a row it could not read is
    /// a row it will never be offered again: the cursor moved on, which is the
    /// price of not stopping every till in the shop over one bad row. Seven
    /// rows of one shop went that way, and the shop was left selling those
    /// items at whatever price each till already held.
    #[tokio::test]
    async fn saying_the_list_again_puts_every_item_after_a_tills_cursor() {
        use openpos_core::protocol::{ResendCatalogueRequest, ResendCatalogueResponse};

        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL).into_string();
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));
        // Sold and then withdrawn: its current state is a tombstone, and a till
        // that missed that is a till still selling something the shop stopped.
        repo.upsert_item(TENANT, item(3));
        repo.delete_item(TENANT, 3);
        // A second edit of the same item, so the count is items rather than
        // changes: a shop that has corrected one price fifty times sends one
        // row for it, not fifty.
        let mut dearer = item(1);
        dearer.price_minor = 45_000;
        let cursor = repo.upsert_item(TENANT, dearer);

        let app = router(AppState::new(repo));
        let (status, sent) = post_to::<_, ResendCatalogueResponse>(
            app.clone(),
            "/v1/back-office/catalogue/resend",
            &ResendCatalogueRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            sent.expect("it says how many").sent,
            3,
            "one row per item the shop has ever had, whatever state it is in now"
        );

        // Now a till standing where the old cursor was: it is offered all three
        // again, and the one that was withdrawn arrives as a tombstone rather
        // than as something to sell.
        let (status, page) = post_to::<_, PullResponse>(
            app,
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                cursor,
                limit: 50,
            },
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let page = page.expect("the catalogue after that cursor");
        assert_eq!(page.upserts.len(), 2, "the two it still sells");
        assert_eq!(page.tombstones, vec![3], "and the one it stopped");
        assert_eq!(
            page.upserts
                .iter()
                .find(|one| one.id == 1)
                .map(|one| one.price_minor),
            Some(45_000),
            "at the price the shop holds now, not the one it first had"
        );
    }

    pub(super) async fn post_to<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        app: Router,
        path: &str,
        body: &T,
        token: Option<&str>,
    ) -> (StatusCode, Option<R>) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, CONTENT_TYPE);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = builder
            .body(Body::from(postcard::to_allocvec(body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, postcard::from_bytes::<R>(&bytes).ok())
    }

    #[tokio::test]
    async fn reports_health_without_a_credential() {
        // Health is the one unauthenticated route: a load balancer has no token
        // and needs to know whether the process is alive.
        let (app, _) = app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn pulls_catalogue_changes_from_a_cursor() {
        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, PullResponse>(app, "/v1/sync/pull", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::OK);
        let page = body.unwrap();
        assert_eq!(page.upserts.len(), 2);
        assert_eq!(page.tombstones, vec![1]);
        assert_eq!(page.cursor, 3);
        assert!(!page.more);
    }

    #[tokio::test]
    async fn pages_a_large_catalogue() {
        let first = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 2,
        };
        let (app, token) = app();
        let (_, body) =
            post_to::<_, PullResponse>(app.clone(), "/v1/sync/pull", &first, Some(&token)).await;
        let page = body.unwrap();
        assert_eq!(page.upserts.len(), 2);
        assert!(page.more, "a till must know to ask again");

        let next = PullRequest {
            cursor: page.cursor,
            ..first
        };
        let (_, body) = post_to::<_, PullResponse>(app, "/v1/sync/pull", &next, Some(&token)).await;
        let page = body.unwrap();
        assert_eq!(page.tombstones, vec![1]);
        assert!(!page.more);
    }

    #[tokio::test]
    async fn issues_a_lease_block() {
        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 500,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, LeaseResponse>(app, "/v1/lease", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::OK);
        let lease = body.unwrap();
        assert_eq!((lease.first, lease.last), (1, 500));
        assert_eq!(lease.epoch, 1);
        assert_eq!(lease.prefix, "T7");
    }

    #[tokio::test]
    async fn tells_an_old_client_to_upgrade_rather_than_failing_opaquely() {
        let request = LeaseRequest {
            protocol: 99,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, ProtocolError>(app, "/v1/lease", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
        assert!(matches!(
            body,
            Some(ProtocolError::UnsupportedVersion { requested: 99, .. })
        ));
    }

    #[tokio::test]
    async fn refuses_a_terminal_that_is_not_enrolled() {
        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: 999,
            cursor: 0,
            limit: 10,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, ProtocolError>(app, "/v1/sync/pull", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::UnknownTerminal));
    }

    #[tokio::test]
    async fn a_different_shop_sees_nothing() {
        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 999,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        };
        let (app, token) = app();
        let (status, _) =
            post_to::<_, ProtocolError>(app, "/v1/sync/pull", &request, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "a terminal belongs to one tenant"
        );
    }

    #[tokio::test]
    async fn a_new_tablet_trades_a_code_for_a_credential() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Owner,
            },
            &code.hash(),
            std::time::Duration::from_secs(900),
        )
        .await
        .unwrap();
        let app = router(AppState::new(repo));

        // Typed by a person, with the grouping and case they actually use.
        let typed = format!(
            "{} {}",
            &code.as_str()[..4],
            code.as_str()[4..].to_lowercase()
        );
        let request = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: typed,
        };
        let (status, body) =
            post_to::<_, EnrolResponse>(app.clone(), "/v1/enrol", &request, None).await;

        assert_eq!(status, StatusCode::OK);
        let enrolled = body.unwrap();
        assert_eq!(enrolled.tenant, TENANT);
        assert_eq!(enrolled.terminal, TERMINAL);
        assert_eq!(enrolled.token.len(), 64);

        // The credential it was handed actually works.
        let lease = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (status, _) =
            post_to::<_, LeaseResponse>(app, "/v1/lease", &lease, Some(&enrolled.token)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_enrolment_code_works_exactly_once() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Owner,
            },
            &code.hash(),
            std::time::Duration::from_secs(900),
        )
        .await
        .unwrap();
        let app = router(AppState::new(repo));

        let request = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: code.as_str().to_owned(),
        };
        let (first, _) =
            post_to::<_, EnrolResponse>(app.clone(), "/v1/enrol", &request, None).await;
        let (second, _) = post_to::<_, ProtocolError>(app, "/v1/enrol", &request, None).await;

        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::UNAUTHORIZED, "a code is single use");
    }

    #[tokio::test]
    async fn a_revoked_credential_stops_working() {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        let hash = token.hash();
        let token = token.into_string();

        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };

        let app = router(AppState::new(repo));
        let (before, _) =
            post_to::<_, LeaseResponse>(app.clone(), "/v1/lease", &request, Some(&token)).await;
        assert_eq!(before, StatusCode::OK);

        // The tablet is lost, so the shop withdraws its credential.
        let repo = MemoryRepo::new();
        let replacement = repo.enrol_with_token(TENANT, TERMINAL);
        repo.revoke_token(&hash).await.unwrap();
        let app = router(AppState::new(repo));

        let (after, _) =
            post_to::<_, ProtocolError>(app.clone(), "/v1/lease", &request, Some(&token)).await;
        assert_eq!(after, StatusCode::UNAUTHORIZED);

        // And the replacement device carries on.
        let (still_working, _) =
            post_to::<_, LeaseResponse>(app, "/v1/lease", &request, Some(replacement.as_str()))
                .await;
        assert_eq!(still_working, StatusCode::OK);
    }

    #[tokio::test]
    async fn guessing_enrolment_codes_is_cut_off() {
        use crate::ratelimit::RateLimiter;
        use std::time::Duration;

        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Owner,
            },
            &code.hash(),
            Duration::from_secs(900),
        )
        .await
        .unwrap();

        let app = router(
            AppState::new(repo).with_enrolment_limit(RateLimiter::new(3, Duration::from_secs(60))),
        );

        // Three wrong guesses are refused as unauthenticated.
        for attempt in 0..3 {
            let request = EnrolRequest {
                protocol: PROTOCOL_VERSION,
                code: format!("WRONG{attempt:03}"),
            };
            let (status, _) =
                post_to::<_, ProtocolError>(app.clone(), "/v1/enrol", &request, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "guess {attempt}");
        }

        // The fourth is not even looked at.
        let request = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: "WRONG999".to_owned(),
        };
        let (status, body) =
            post_to::<_, ProtocolError>(app.clone(), "/v1/enrol", &request, None).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(matches!(
            body,
            Some(ProtocolError::TooManyAttempts { retry_after_seconds })
                if retry_after_seconds > 0
        ));

        // And the limit holds even for the code that would have worked, which is
        // the cost of a shared bucket and the reason the budget is generous.
        let real = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: code.as_str().to_owned(),
        };
        let (status, _) = post_to::<_, ProtocolError>(app, "/v1/enrol", &real, None).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn refuses_a_request_with_no_credential() {
        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (app, _) = app();
        let (status, body) = post_to::<_, ProtocolError>(app, "/v1/lease", &request, None).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, Some(ProtocolError::Unauthenticated));
    }

    #[tokio::test]
    async fn refuses_a_credential_it_does_not_know() {
        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (app, _) = app();
        let (status, _) =
            post_to::<_, ProtocolError>(app, "/v1/lease", &request, Some("not a real token")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// The reason this exists: before authentication, a body could claim to be
    /// any shop and the server believed it.
    #[tokio::test]
    async fn a_valid_credential_cannot_be_used_to_claim_another_shop() {
        let repo = MemoryRepo::new();
        let intruder = repo.enrol_with_token(TENANT, TERMINAL);
        repo.enrol(999, 888);
        repo.upsert_item(999, item(7));
        let app = router(AppState::new(repo));

        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 999,
            terminal: 888,
            cursor: 0,
            limit: 100,
        };
        let (status, body) =
            post_to::<_, ProtocolError>(app, "/v1/sync/pull", &request, Some(intruder.as_str()))
                .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::UnknownTerminal));
    }

    /// A shop with one sale the server could not accept as it stood.
    pub(super) async fn shop_with_a_repair() -> (Router, String) {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        repo.store_sale(StoredSale {
            tenant: TENANT,
            terminal: TERMINAL,
            id: 900,
            receipt_no: Some("T7-000100".to_owned()),
            receipt_epoch: Some(1),
            rung_at_ms: 1_788_600_000_000,
            total_minor: 49_450,
            payload: vec![1, 2, 3],
            quarantine: Some(QuarantineReason::TotalsMismatch {
                stored_minor: 1,
                recomputed_minor: 49_450,
            }),
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
        (router(AppState::new(repo)), token.into_string())
    }

    pub(super) fn repair_request() -> RepairQueueRequest {
        RepairQueueRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            limit: 50,
        }
    }

    #[tokio::test]
    async fn rejects_a_body_that_is_not_a_request() {
        let (app, _) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/v1/sync/push")
            .body(Body::from(vec![0xFF_u8; 8]))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// Build a bare request carrying a forwarded header, for the keying tests.
    fn forwarded(header: &str) -> Request<Body> {
        let mut request = Request::new(Body::empty());
        if !header.is_empty() {
            request.headers_mut().insert(
                "x-forwarded-for",
                header.parse().expect("a valid header value"),
            );
        }
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4000))));
        request
    }

    #[test]
    fn without_a_configured_proxy_the_header_is_ignored() {
        // Trusting a header nobody overwrites lets a caller invent an address
        // and mint a fresh budget for every request, which is worse than one
        // shared bucket.
        assert_eq!(client_key(&forwarded("1.2.3.4"), 0), "10.0.0.1");
    }

    #[test]
    fn behind_one_proxy_the_client_is_the_rightmost_entry() {
        // Everything left of the rightmost entry was written by whoever was
        // calling. The rightmost is what the proxy itself observed.
        assert_eq!(client_key(&forwarded("9.9.9.9, 1.2.3.4"), 1), "1.2.3.4");
    }

    #[test]
    fn a_missing_header_behind_a_proxy_falls_back_to_the_socket() {
        // A direct connection to a server configured for a proxy. Throttling by
        // socket address is the safe direction.
        assert_eq!(client_key(&forwarded(""), 1), "10.0.0.1");
    }

    #[test]
    fn two_clients_behind_one_proxy_do_not_share_a_budget() {
        // The whole point. Behind Caddy or Cloudflare both of these arrive from
        // the proxy's address, and one attacker would otherwise lock every shop
        // out of enrolling a tablet.
        assert_ne!(
            client_key(&forwarded("1.2.3.4"), 1),
            client_key(&forwarded("5.6.7.8"), 1)
        );
    }

    #[tokio::test]
    async fn an_unparseable_flood_does_not_spend_a_shops_enrolment_budget() {
        use crate::ratelimit::RateLimiter;
        use std::time::Duration;

        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Owner,
            },
            &code.hash(),
            Duration::from_secs(900),
        )
        .await
        .unwrap();
        let app = router(
            AppState::new(repo).with_enrolment_limit(RateLimiter::new(3, Duration::from_secs(60))),
        );

        // Rubbish that never gets as far as guessing at a code.
        for _ in 0..5 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/enrol")
                        .body(Body::from(vec![0xFF, 0xFE, 0xFD]))
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        // The shop's real code still works: spending budget on requests that
        // were never guesses would let anyone deny enrolment for free.
        let real = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: code.as_str().to_owned(),
        };
        let (status, _) = post_to::<_, EnrolResponse>(app, "/v1/enrol", &real, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_till_can_trade_a_working_credential_for_a_fresh_one() {
        let (app, token) = app();

        let (status, body) = post_to::<_, RenewResponse>(
            app.clone(),
            "/v1/renew",
            &RenewRequest {
                protocol: PROTOCOL_VERSION,
            },
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let renewed = body.expect("a new credential");
        assert_ne!(renewed.token, token);
        assert!(renewed.expires_in_seconds > 0);

        // The new one works.
        let (status, _) = post_to::<_, PullResponse>(
            app.clone(),
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                cursor: 0,
                limit: 10,
            },
            Some(&renewed.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // And so does the old one, for now. This reply can be lost on a bad
        // connection, and a till whose only working credential was revoked the
        // instant the server issued a new one is a shop offline until somebody
        // re-enrols the tablet by hand.
        assert!(renewed.previous_valid_for_seconds > 0);
        let (status, _) = post_to::<_, PullResponse>(
            app,
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                cursor: 0,
                limit: 10,
            },
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn renewal_needs_a_credential_and_will_not_take_a_guess() {
        let (app, _token) = app();

        for presented in [None, Some("not-a-real-token")] {
            let (status, _) = post_to::<_, ProtocolError>(
                app.clone(),
                "/v1/renew",
                &RenewRequest {
                    protocol: PROTOCOL_VERSION,
                },
                presented,
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "presented {presented:?}");
        }
    }

    /// A shop with a till credential as well as the owner one.
    pub(super) async fn app_with_till() -> (Router, String, String) {
        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));
        // Somebody who may stand at the till, because a shop has people and a
        // drawer is counted by one of them. This used not to matter: a drawer
        // carried whatever name the device typed, so the tests named a cashier
        // who was in no shop's records and nothing noticed. A count now takes
        // the shop's own name for the id it was counted by, which is what makes
        // the name worth reading months later, and a shop with nobody in it can
        // no longer produce a named count.
        repo.put_operator(
            TENANT,
            &crate::repo::OperatorRecord {
                id: 91,
                name: String::from("Rahima"),
                pin_salt: vec![7; 16],
                pin_rounds: 100_000,
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
        )
        .await
        .expect("the in-memory store accepts a person");

        let till = Token::generate();
        repo.store_token_as(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Till,
            },
            &till.hash(),
            Role::Till,
        )
        .await
        .expect("the in-memory store accepts a token");

        (
            router(AppState::new(repo)),
            owner.into_string(),
            till.into_string(),
        )
    }

    #[tokio::test]
    async fn a_till_credential_still_rings_sales_and_syncs() {
        let (app, _owner, till) = app_with_till().await;

        // The point is to narrow what a device may be used for, not to break
        // the one thing it is for.
        let (status, _) = post_to::<_, PullResponse>(
            app,
            "/v1/sync/pull",
            &PullRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                cursor: 0,
                limit: 10,
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// A till writes down what the shop has never heard of, and nothing else.
    ///
    /// Raised in review. The route exists because a delivery arrives during an
    /// outage with a barcode in nobody's catalogue and the sale has to happen,
    /// and it took whatever a till sent, including an id the shop already held.
    /// Any till in the shop could reprice, rename or re-tax the whole catalogue
    /// by sending back the items it had pulled, which is the thing the roles
    /// were added to stop: a shop with six tills had six devices that could
    /// reprice everything, and any one left on a counter was the whole shop.
    #[tokio::test]
    async fn a_till_may_write_down_a_new_item_and_may_not_rewrite_the_shops() {
        let repo = MemoryRepo::new();
        let till = Token::generate();
        repo.store_token_as(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Till,
            },
            &till.hash(),
            Role::Till,
        )
        .await
        .expect("the in-memory store accepts a token");
        // The shop's own item, at the shop's own price.
        repo.upsert_item(TENANT, item(1));
        let state = AppState::new(repo);
        let repo = std::sync::Arc::clone(&state.repo);
        let app = router(state);
        let till = till.into_string();

        // The same id, at a price nobody in the back office agreed to.
        let mut repriced = item(1);
        repriced.price_minor = 1;
        repriced.name_en = "Rice, mine now".to_owned();
        // And one the shop has never heard of, which is what this route is for.
        let fresh = item(77);

        let (status, body) = post_to::<_, PushItemsResponse>(
            app,
            "/v1/sync/items",
            &PushItemsRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                items: vec![repriced, fresh],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let stored = body.expect("an answer").stored;
        assert_eq!(
            stored.len(),
            2,
            "both are acknowledged: a till holds an item until the shop says it has it, and one \
             sent again for ever is a device that never stops"
        );

        let held = repo
            .item_now(TENANT, 1)
            .await
            .expect("the store answers")
            .expect("the shop still has its own item");
        assert_eq!(held.0.price_minor, 43_000, "the shop's price stands");
        assert_eq!(held.0.name_en, "Rice Miniket 5kg", "and the shop's name");

        let written = repo
            .item_now(TENANT, 77)
            .await
            .expect("the store answers")
            .expect("the item the till wrote down is the shop's now");
        assert!(written.0.from_a_till, "and it is marked as a till's work");
    }

    /// A till may correct a name, and may not touch what the owner decided.
    ///
    /// Raised in review. A till sends no credit cap, so the plain write put a
    /// zero over one, and zero means no cap: any till in the shop could take an
    /// owner's limit off anybody by writing down somebody it already had. The
    /// same write could flip whether they may buy at all.
    #[tokio::test]
    async fn a_till_writing_somebody_down_leaves_their_cap_alone() {
        let repo = MemoryRepo::new();
        let till = Token::generate();
        repo.store_token_as(
            Caller {
                tenant: TENANT,
                terminal: TERMINAL,
                role: Role::Till,
            },
            &till.hash(),
            Role::Till,
        )
        .await
        .expect("the in-memory store accepts a token");
        // Somebody the owner wrote down, with a cap on what they may owe.
        repo.put_customer(
            TENANT,
            &crate::repo::CustomerRecord {
                id: 500,
                name: "Karim Uddin".to_owned(),
                phone: Some("01711000000".to_owned()),
                active: true,
                bin: None,
                limit_minor: 200_000,
            },
        )
        .await
        .expect("stored");
        let state = AppState::new(repo);
        let repo = std::sync::Arc::clone(&state.repo);
        let app = router(state);
        let till = till.into_string();

        let (status, body) = post_to::<_, PushCustomersResponse>(
            app,
            "/v1/sync/customers",
            &PushCustomersRequest {
                protocol: PROTOCOL_VERSION,
                tenant: TENANT,
                terminal: TERMINAL,
                customers: vec![openpos_core::protocol::CustomerWire {
                    id: 500,
                    name: "Karim Uddin, flat 3".to_owned(),
                    phone: Some("01711000001".to_owned()),
                    active: false,
                    bin: None,
                    // A till sends what it holds, which for somebody it never
                    // set a cap on is nothing. The route ignores it either way.
                    limit_minor: 0,
                }],
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.expect("an answer").stored, vec![500]);

        let held = repo
            .customers(TENANT)
            .await
            .expect("the store answers")
            .into_iter()
            .find(|known| known.id == 500)
            .expect("still there");
        assert_eq!(held.name, "Karim Uddin, flat 3", "the correction stands");
        assert_eq!(held.phone.as_deref(), Some("01711000001"));
        assert_eq!(held.limit_minor, 200_000, "and the owner's cap stands");
        assert!(held.active, "and so does the owner's answer about buying at all");
    }

    /// A cashier can ask what a receipt said, because that is who is handed it.
    ///
    /// Only the back office could look a receipt up, so a refund at the counter
    /// was rung by scanning the goods again at today's catalogue price. A
    /// basket sold with ten percent off the ticket came back at full price and
    /// the shop gave the discount away a second time; the shop's own guard
    /// catches the whole basket coming back, and a single line of it fits
    /// under the total and passes.
    #[tokio::test]
    async fn a_till_can_ask_what_was_on_a_receipt_of_its_own_shop() {
        let (app, _owner, till) = app_with_till().await;

        let (status, body) = post_to::<_, openpos_core::protocol::ReceiptResponse>(
            app,
            "/v1/receipt",
            &openpos_core::protocol::ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: "T1-000100".to_owned(),
            },
            Some(&till),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.expect("an answer").found.is_empty(),
            "this shop has no such receipt, which is an answer rather than a refusal"
        );
    }

    #[tokio::test]
    async fn a_device_with_no_credential_cannot_ask_what_was_on_a_receipt() {
        let (app, _owner, _till) = app_with_till().await;

        let (status, body) = post_to::<_, ProtocolError>(
            app,
            "/v1/receipt",
            &openpos_core::protocol::ReceiptRequest {
                protocol: PROTOCOL_VERSION,
                receipt_no: "T1-000100".to_owned(),
            },
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, Some(ProtocolError::Unauthenticated));
    }
}
