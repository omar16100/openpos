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

use std::net::SocketAddr;
use std::time::Duration;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use openpos_core::protocol::{
    negotiate, CatalogueEditResponse, CorrectStockRequest, CorrectStockResponse, DeleteItemRequest,
    EnrolRequest, EnrolResponse, IssueCodeRequest, IssueCodeResponse, LeaseRequest, LeaseResponse,
    OnHandEntry, OperatorWire, OperatorsRequest, OperatorsResponse, ProtocolError, PullRequest,
    PullResponse, PushRequest, PutOperatorRequest, PutShopRequest, PutSupplierRequest,
    SetOperatorActiveRequest,
    ReceiveGoodsRequest, ReceiveGoodsResponse, RecordCountRequest, RecordCountResponse,
    RenewRequest, RenewResponse, RepairEntry, RepairQueueRequest, RepairQueueResponse,
    ResolveRepairRequest, ResolveRepairResponse, ShopRequest, ShopResponse, SupplierWire,
    SuppliersRequest, SuppliersResponse, TerminalHealthEntry, TerminalHealthRequest,
    TerminalHealthResponse, UpsertItemRequest,
};

use crate::auth::{bearer, Caller, EnrolmentCode, Role, Token, TokenHash};
use crate::ingest::{self, IngestError};
use crate::ratelimit::{Decision, RateLimiter};
use crate::repo::{
    GoodsReceipt, OperatorRecord, ReceiptLine, RepoError, Repository, ShopDetails, StockCorrection,
    StockCount, Supplier,
    TOKEN_LIFETIME, TOKEN_RENEWAL_OVERLAP,
};

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
    Router::new()
        .route("/health", get(health))
        .route("/v1/sync/push", post(push))
        .route("/v1/sync/pull", post(pull))
        .route("/v1/lease", post(lease))
        .route("/v1/enrol", post(enrol))
        .route("/v1/renew", post(renew))
        .route("/v1/back-office/stock/count", post(record_count))
        .route("/v1/back-office/suppliers", post(suppliers))
        .route("/v1/back-office/suppliers/put", post(put_supplier))
        .route("/v1/back-office/stock/receive", post(receive_goods))
        .route("/v1/back-office/enrolment-codes", post(issue_code))
        .route("/v1/back-office/stock/correct", post(correct_stock))
        .route("/v1/shop", post(shop))
        .route("/v1/operators", post(operators))
        .route("/v1/back-office/operators", post(put_operator))
        .route("/v1/back-office/operators/active", post(set_operator_active))
        .route("/v1/back-office/shop", post(put_shop))
        .route("/v1/back-office/repairs", post(repairs))
        .route("/v1/back-office/repairs/resolve", post(resolve_repair))
        .route("/v1/back-office/terminals", post(terminals))
        .route("/v1/back-office/catalogue/upsert", post(upsert_item))
        .route("/v1/back-office/catalogue/delete", post(delete_item))
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
        Ok(None) => Err(protocol_error(&ProtocolError::Unauthenticated)),
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

/// Sales from a till.
async fn push<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<PushRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };
    // Recorded before the batch is stored, not after. The question the health
    // list answers is when the server last heard from this device, and a push
    // that fails on the way to the database is still the device talking.
    note_contact(&state, caller).await;

    match ingest::push(state.repo.as_ref(), &request).await {
        Ok(response) => encoded(&response),
        Err(IngestError::Protocol(error)) => protocol_error(&error),
        // The till keeps its copy and retries. Telling it otherwise would let it
        // drop the only record of a sale that already happened.
        Err(IngestError::Storage) => unavailable(),
    }
}

/// Catalogue changes to a till.
async fn pull<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<PullRequest>(&body) else {
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
        Ok(page) => encoded(&PullResponse {
            protocol,
            cursor: page.cursor,
            upserts: page.upserts,
            tombstones: page.tombstones,
            more: page.more,
        }),
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
    let Ok(request) = postcard::from_bytes::<OperatorsRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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

/// Suspend somebody, or let them back in. Owner only.
///
/// Its own route rather than a flag on the upsert, because that one carries the
/// whole person including the derived PIN key, and an owner suspending somebody
/// does not have it: a PIN is hashed on the device where it is set and never
/// travels. Requiring it here would mean asking an owner to know a cashier's
/// PIN in order to take the drawer away from them.
async fn set_operator_active<R: Repository>(
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
async fn put_operator<R: Repository>(
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

fn wire_operator(record: OperatorRecord) -> OperatorWire {
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

/// The shop's own details, for the top of a receipt.
///
/// Readable by any credential, not just an owner: every till prints receipts,
/// and a till that could not learn its own shop's name would print blank ones.
async fn shop<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<ShopRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
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
        }),
        Err(RepoError::UnknownTerminal) => protocol_error(&ProtocolError::UnknownTerminal),
        Err(_) => unavailable(),
    }
}

/// Set them. Owner only: this is what every receipt the shop issues will say.
async fn put_shop<R: Repository>(
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
async fn put_supplier<R: Repository>(
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
async fn suppliers<R: Repository>(
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
async fn receive_goods<R: Repository>(
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
async fn correct_stock<R: Repository>(
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
async fn record_count<R: Repository>(
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
    let Ok(request) = postcard::from_bytes::<RenewRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };

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
        .renew_token(caller, &previous, &replacement.hash(), TOKEN_RENEWAL_OVERLAP)
        .await
    {
        Ok(()) => encoded(&RenewResponse {
            protocol,
            token: replacement.into_string(),
            expires_in_seconds: TOKEN_LIFETIME.as_secs(),
            previous_valid_for_seconds: TOKEN_RENEWAL_OVERLAP.as_secs(),
        }),
        Err(_) => unavailable(),
    }
}

/// A block of receipt numbers for a till.
async fn lease<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<LeaseRequest>(&body) else {
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
    note_contact(&state, caller).await;

    match state
        .repo
        .issue_lease(caller.tenant, caller.terminal, request.count)
        .await
    {
        Ok(record) => encoded(&LeaseResponse {
            protocol,
            epoch: record.epoch,
            // Short and human readable, because it is printed on every receipt
            // and read aloud over the phone when something is disputed.
            prefix: format!("T{:X}", record.terminal & 0xFFFF),
            first: record.first,
            last: record.last,
        }),
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

/// Issue a code that will enrol a new device.
///
/// Owner only, and a caller may not grant a role above its own. The second rule
/// is trivially satisfied while there are two roles and only owners can reach
/// this route, and it is written down anyway: the day a third role exists, this
/// is the line that would otherwise have been missing.
async fn issue_code<R: Repository>(
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
    let Ok(request) = postcard::from_bytes::<EnrolRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };

    if let Decision::Deny { retry_after } = state.enrolment_limit.check(&key) {
        return protocol_error(&ProtocolError::TooManyAttempts {
            retry_after_seconds: retry_after.as_secs().max(1),
        });
    }
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };

    let caller = match state
        .repo
        .redeem_enrolment_code(&EnrolmentCode::hash_of(&request.code))
        .await
    {
        Ok(Some(caller)) => caller,
        // Unknown, expired and already used are one answer, so probing tells an
        // attacker nothing about which it was.
        Ok(None) => return protocol_error(&ProtocolError::Unauthenticated),
        Err(_) => return unavailable(),
    };

    let token = Token::generate();
    if state.repo.store_token(caller, &token.hash()).await.is_err() {
        return unavailable();
    }

    encoded(&EnrolResponse {
        protocol,
        tenant: caller.tenant,
        terminal: caller.terminal,
        token: token.into_string(),
    })
}

/// Sales the server could not accept as they stood.
///
/// Read-only, and deliberately a POST like everything else here: the body is
/// postcard, and a GET with a postcard body is not something a cache, a proxy or
/// a browser will treat consistently.
async fn repairs<R: Repository>(
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
async fn resolve_repair<R: Repository>(
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
async fn terminals<R: Repository>(
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
async fn upsert_item<R: Repository>(
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
async fn delete_item<R: Repository>(
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
        ProtocolError::Malformed => StatusCode::BAD_REQUEST,
    };
    match postcard::to_allocvec(error) {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, CONTENT_TYPE)], bytes).into_response(),
        Err(_) => status.into_response(),
    }
}

/// Temporary failure. Distinct from a refusal on purpose: a till must retry this
/// one, and must not retry a refusal.
fn unavailable() -> Response {
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
        CountedItem, ItemWire, QuarantineReason, ReceiptLineWire, PROTOCOL_VERSION,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::repo::{MemoryRepo, StoredSale};

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    /// A shop, a terminal, a small catalogue, and the terminal's credential.
    fn app() -> (Router, String) {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));
        repo.delete_item(TENANT, 1);
        (router(AppState::new(repo)), token.into_string())
    }

    fn item(id: u128) -> ItemWire {
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
        }
    }

    async fn post_to<T: serde::Serialize, R: serde::de::DeserializeOwned>(
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
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
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
        let (_, body) =
            post_to::<_, PullResponse>(app, "/v1/sync/pull", &next, Some(&token)).await;
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
        assert_eq!(status, StatusCode::FORBIDDEN, "a terminal belongs to one tenant");
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
        let typed = format!("{} {}", &code.as_str()[..4], code.as_str()[4..].to_lowercase());
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
        let (still_working, _) = post_to::<_, LeaseResponse>(
            app,
            "/v1/lease",
            &request,
            Some(replacement.as_str()),
        )
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
        let (status, body) = post_to::<_, ProtocolError>(
            app,
            "/v1/sync/pull",
            &request,
            Some(intruder.as_str()),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::UnknownTerminal));
    }

    /// A shop with one sale the server could not accept as it stood.
    async fn shop_with_a_repair() -> (Router, String) {
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
        })
        .await
        .unwrap();
        (router(AppState::new(repo)), token.into_string())
    }

    fn repair_request() -> RepairQueueRequest {
        RepairQueueRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            limit: 50,
        }
    }

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
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from((
            [10, 0, 0, 1],
            4000,
        ))));
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

    /// A shop with a till credential as well as the owner one.
    async fn app_with_till() -> (Router, String, String) {
        let repo = MemoryRepo::new();
        let owner = repo.enrol_with_token(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));

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
